//! Audio file handling — duration probing, format-specific processing, and visualization.

pub mod eq;
pub mod viz;

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::time::Duration;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

// ── Duration probe ─────────────────────────────────────────────────────────────

pub fn probe_duration(path: &Path) -> Option<Duration> {
    let file = File::open(path).ok()?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    let probed = symphonia::default::get_probe()
        .format(&hint, mss, &FormatOptions::default(), &MetadataOptions::default())
        .ok()?;
    let track = probed.format.default_track()?;
    let tb       = track.codec_params.time_base?;
    let n_frames = track.codec_params.n_frames?;
    let t = tb.calc_time(n_frames);
    Some(Duration::from_secs_f64(t.seconds as f64 + t.frac))
}

// ── FLAC seektable builder ─────────────────────────────────────────────────────

/// Returns true if the FLAC file already has a non-empty SEEKTABLE block.
/// Reads only the metadata section — a handful of bytes at most.
pub fn flac_has_seektable(path: &Path) -> bool {
    let mut f = match File::open(path) { Ok(f) => f, Err(_) => return false };
    let mut b4 = [0u8; 4];
    if f.read_exact(&mut b4).is_err() || &b4 != b"fLaC" { return false; }
    loop {
        if f.read_exact(&mut b4).is_err() { return false; }
        let is_last = b4[0] & 0x80 != 0;
        let btype   = b4[0] & 0x7F;
        let blen    = u32::from_be_bytes([0, b4[1], b4[2], b4[3]]) as i64;
        if btype == 3 && blen > 0 { return true; }
        if is_last { return false; }
        if f.seek(SeekFrom::Current(blen)).is_err() { return false; }
    }
}

/// Scans every audio packet (frame-demux only, no decoding) to collect accurate
/// byte offsets, then writes a SEEKTABLE block into the file's PADDING block.
///
/// This is the accurate version that walks packet headers — takes ~1 s on a
/// large FLAC but is called from a background thread so it never blocks the UI.
/// On subsequent loads `flac_has_seektable` returns true and this is skipped.
pub fn build_flac_seektable(path: &Path) -> io::Result<()> {
    // ── Phase 1: read STREAMINFO and find PADDING block ────────────────────
    let (sample_rate, pad_hdr_pos, pad_data_len, pad_is_last) = {
        let mut f = File::open(path)?;
        let mut b4 = [0u8; 4];
        f.read_exact(&mut b4)?;
        if &b4 != b"fLaC" { return Ok(()); }

        let mut sample_rate = 0u32;
        let mut pad: Option<(u64, u64, bool)> = None;

        loop {
            let hdr_pos = f.stream_position()?;
            f.read_exact(&mut b4)?;
            let is_last = b4[0] & 0x80 != 0;
            let btype   = b4[0] & 0x7F;
            let blen    = u32::from_be_bytes([0, b4[1], b4[2], b4[3]]) as u64;
            match btype {
                0 => {
                    let mut si = vec![0u8; blen as usize];
                    f.read_exact(&mut si)?;
                    if si.len() >= 13 {
                        sample_rate = ((si[10] as u32) << 12)
                            | ((si[11] as u32) << 4)
                            | ((si[12] as u32) >> 4);
                    }
                }
                1 if blen >= 18 && pad.is_none() => {
                    pad = Some((hdr_pos, blen, is_last));
                    f.seek(SeekFrom::Current(blen as i64))?;
                }
                3 => return Ok(()), // seektable already present
                _ => { f.seek(SeekFrom::Current(blen as i64))?; }
            }
            if is_last { break; }
        }

        match pad {
            Some((pos, len, last)) if sample_rate > 0 => (sample_rate, pos, len, last),
            _ => return Ok(()),
        }
    };

    // ── Phase 2: walk audio packets to collect accurate byte offsets ────────
    //
    // symphonia's packet demuxer reads just the frame headers (a few bytes each)
    // without decoding any audio. We accumulate the encoded size of each packet
    // to track the exact byte offset of each frame within the audio stream.
    let file = File::open(path)?;
    let mss  = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    hint.with_extension("flac");
    let probed = symphonia::default::get_probe()
        .format(&hint, mss, &FormatOptions::default(), &MetadataOptions::default())
        .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;
    let mut fmt = probed.format;

    let n_slots = (pad_data_len / 18) as usize;
    if n_slots == 0 { return Ok(()); }

    let interval  = sample_rate as u64; // one seekpoint per second
    let mut points: Vec<(u64, u64, u16)> = Vec::new(); // (sample, byte_off, frame_samples)
    let mut byte_off: u64 = 0;
    let mut next_target: u64 = 0;

    loop {
        match fmt.next_packet() {
            Ok(pk) => {
                if pk.ts >= next_target && points.len() < n_slots {
                    points.push((pk.ts, byte_off, pk.dur.min(u16::MAX as u64) as u16));
                    next_target = (pk.ts / interval + 1) * interval;
                }
                byte_off += pk.data.len() as u64;
            }
            Err(_) => break,
        }
    }
    if points.is_empty() { return Ok(()); }

    // ── Phase 3: write SEEKTABLE over the PADDING block ────────────────────
    //
    // SEEKTABLE entries are 18 bytes. The PADDING may not divide evenly by 18;
    // any leftover ≥ 4 bytes becomes a small trailing PADDING block.
    // If the leftover is 1-3 bytes (too small for a block header) we sacrifice
    // one slot and combine those bytes into a valid trailing PADDING block.
    let remainder = pad_data_len % 18;
    let used_slots: usize = if remainder > 0 && remainder < 4 {
        ((pad_data_len / 18).saturating_sub(1)) as usize
    } else {
        (pad_data_len / 18) as usize
    };
    let leftover: u64 = if remainder == 0 { 0 }
        else if remainder >= 4 { remainder }
        else { remainder + 18 };

    let seektable_data_len = (used_slots as u32) * 18;
    let actual_points = points.len().min(used_slots);

    let mut wf = std::fs::OpenOptions::new().write(true).open(path)?;
    wf.seek(SeekFrom::Start(pad_hdr_pos))?;

    // SEEKTABLE block header (type = 3)
    let st_last: u8 = if leftover == 0 && pad_is_last { 0x80 } else { 0 };
    wf.write_all(&[
        st_last | 3,
        (seektable_data_len >> 16) as u8,
        (seektable_data_len >>  8) as u8,
         seektable_data_len        as u8,
    ])?;

    for &(sn, so, fs) in &points[..actual_points] {
        let mut e = [0u8; 18];
        e[ 0.. 8].copy_from_slice(&sn.to_be_bytes());
        e[ 8..16].copy_from_slice(&so.to_be_bytes());
        e[16..18].copy_from_slice(&fs.to_be_bytes());
        wf.write_all(&e)?;
    }
    // Fill remaining slots with placeholder entries
    let mut ph = [0u8; 18];
    ph[..8].fill(0xFF);
    for _ in actual_points..used_slots {
        wf.write_all(&ph)?;
    }

    // Trailing PADDING block for any leftover bytes
    if leftover >= 4 {
        let pd_data = leftover - 4;
        let pd_last: u8 = if pad_is_last { 0x80 } else { 0 };
        wf.write_all(&[
            pd_last | 1,
            (pd_data >> 16) as u8,
            (pd_data >>  8) as u8,
             pd_data        as u8,
        ])?;
        wf.write_all(&vec![0u8; pd_data as usize])?;
    }

    Ok(())
}

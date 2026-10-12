//! AAC-LC encoding for iPod pushes.
//!
//! The device plays MP3 and AAC (`.m4a`). When the user picks a target
//! bitrate, anything the device cannot play (FLAC, OGG, WAV) is encoded, and
//! anything above that bitrate is re-encoded. The container is a minimal
//! ISOBMFF (`ftyp`/`mdat`/`moov`) written in one pass with `mdat` first so the
//! chunk offset is known while packets are written.

use rusty_aac::encode::{audio_specific_config_bytes, AacEncoder, AacEncoderConfig, EncodedPacket};
use rusty_aac::Error;
use std::io::{BufWriter, Seek, Write};
use std::path::Path;
use symphonia::core::audio::{AudioBufferRef, Signal};

/// Encode `src` to AAC-LC at `bitrate_bps`, writing an `.m4a` file to `dest`.
///
/// Returns the encoded length in milliseconds.
///
/// - Decodes `src` with symphonia to interleaved f32.
/// - Downmixes to stereo when the source has more than two channels.
/// - Pushes into `AacEncoder`, calls `finish()`, drains `next_packet()` until `Error::Eof`.
/// - Writes `ftyp`, then `mdat`, then `moov`. `mdat` comes first so the one
///   chunk offset is known while the packets are written — no second pass.
pub fn encode_file(src: &Path, bitrate_bps: u32, dest: &Path) -> Result<u32, String> {
    // Open and decode with symphonia.
    let mss = symphonia::core::io::MediaSourceStream::new(
        Box::new(std::fs::File::open(src).map_err(|e| format!("open {}: {}", src.display(), e))?),
        Default::default(),
    );
    let probed = symphonia::default::get_probe()
        .format(
            &Default::default(),
            mss,
            &Default::default(),
            &Default::default(),
        )
        .map_err(|e| format!("probe {}: {}", src.display(), e))?;

    let mut format = probed.format;
    let track = format
        .default_track()
        .ok_or_else(|| format!("{}: no default track", src.display()))?;
    let codec_params = track.codec_params.clone();

    let sample_rate = codec_params
        .sample_rate
        .ok_or_else(|| format!("{}: no sample rate", src.display()))?;
    let channels = codec_params
        .channels
        .ok_or_else(|| format!("{}: no channel layout", src.display()))?;

    let _channel_count = channels.count();

    // Build the decoder.
    let mut decoder = symphonia::default::get_codecs()
        .make(
            &codec_params,
            &symphonia::core::codecs::DecoderOptions::default(),
        )
        .map_err(|e| format!("decoder for {}: {}", src.display(), e))?;

    // AAC encoder setup.
    let config = AacEncoderConfig {
        bitrate_bps,
        ..Default::default()
    };
    let mut encoder = AacEncoder::new(config);

    let mut packets = Vec::new();
    let mut total_samples: u64 = 0;

    // Decode loop.
    while let Ok(packet) = format.next_packet() {
        let decoded = decoder
            .decode(&packet)
            .map_err(|e| format!("decode {}: {}", src.display(), e))?;

        let frames = decoded.frames();
        let ch = decoded.spec().channels.count();

        if ch == 0 {
            continue;
        }

        // Extract f32 channel slices from AudioBufferRef (any format).
        let mut chan_data: Vec<Vec<f32>> = Vec::with_capacity(ch);
        match decoded {
            AudioBufferRef::F32(buf) => {
                for c in 0..ch {
                    chan_data.push(buf.chan(c).to_vec());
                }
            }
            AudioBufferRef::S16(buf) => {
                for c in 0..ch {
                    chan_data.push(
                        buf.chan(c)
                            .iter()
                            .map(|&s| s as f32 / i16::MAX as f32)
                            .collect(),
                    );
                }
            }
            AudioBufferRef::S32(buf) => {
                for c in 0..ch {
                    chan_data.push(
                        buf.chan(c)
                            .iter()
                            .map(|&s| s as f32 / i32::MAX as f32)
                            .collect(),
                    );
                }
            }
            AudioBufferRef::U8(buf) => {
                for c in 0..ch {
                    chan_data.push(
                        buf.chan(c)
                            .iter()
                            .map(|&s| (s as f32 - 128.0) / 128.0)
                            .collect(),
                    );
                }
            }
            AudioBufferRef::F64(buf) => {
                for c in 0..ch {
                    chan_data.push(buf.chan(c).iter().map(|&s| s as f32).collect());
                }
            }
            AudioBufferRef::U16(buf) => {
                for c in 0..ch {
                    chan_data.push(
                        buf.chan(c)
                            .iter()
                            .map(|&s| s as f32 / u16::MAX as f32)
                            .collect(),
                    );
                }
            }
            AudioBufferRef::U24(ref _buf) => {
                // U24 is rare; fall back to planar copy via make_equivalent.
                let owned = decoded.make_equivalent::<f32>();
                for c in 0..ch {
                    chan_data.push(owned.chan(c).to_vec());
                }
            }
            AudioBufferRef::U32(buf) => {
                for c in 0..ch {
                    chan_data.push(
                        buf.chan(c)
                            .iter()
                            .map(|&s| s as f32 / u32::MAX as f32)
                            .collect(),
                    );
                }
            }
            AudioBufferRef::S8(_) | AudioBufferRef::S24(_) => {
                // Rare formats; fall back to make_equivalent.
                let owned = decoded.make_equivalent::<f32>();
                for c in 0..ch {
                    chan_data.push(owned.chan(c).to_vec());
                }
            }
        }

        // Convert to interleaved f32. Use decoded channel count `ch` for downmix.
        let mut interleaved = Vec::with_capacity(frames * ch.min(2) as usize);
        if ch >= 2 {
            // Stereo or more: take first two channels, or downmix all to stereo.
            if ch == 2 {
                for (a, b) in chan_data[0].iter().zip(chan_data[1].iter()).take(frames) {
                    interleaved.push(*a);
                    interleaved.push(*b);
                }
            } else {
                // Downmix to stereo: average all channels into L and R.
                for i in 0..frames {
                    let mut l = 0.0;
                    let mut r = 0.0;
                    for (c, samples) in chan_data.iter().enumerate().take(ch) {
                        let sample = samples[i];
                        if c % 2 == 0 {
                            l += sample;
                        } else {
                            r += sample;
                        }
                    }
                    interleaved.push(l / (ch.div_ceil(2) as f32));
                    interleaved.push(r / (ch / 2).max(1) as f32);
                }
            }
        } else {
            // Mono: duplicate to stereo.
            for s in chan_data[0].iter().take(frames) {
                interleaved.push(*s);
                interleaved.push(*s);
            }
        }

        total_samples += frames as u64;

        // Push to encoder (first push sets sample rate and channels).
        encoder
            .push_pcm(&interleaved, 2, sample_rate)
            .map_err(|e| format!("AAC push_pcm: {}", e))?;
    }

    // Flush encoder.
    encoder.finish();
    loop {
        match encoder.next_packet() {
            Ok(EncodedPacket { data, .. }) => {
                if !data.is_empty() {
                    packets.push(data);
                }
            }
            Err(Error::Eof) => {
                break;
            }
            Err(e) => return Err(format!("AAC next_packet: {}", e)),
        }
    }

    let duration_ms = ((total_samples * 1000) / sample_rate as u64) as u32;

    // Write ISOBMFF container: ftyp -> mdat -> moov.
    let file =
        std::fs::File::create(dest).map_err(|e| format!("create {}: {}", dest.display(), e))?;
    let mut writer = BufWriter::new(file);

    // ftyp (isom, iso2, mp41)
    write_box(&mut writer, b"ftyp", |w| {
        w.write_all(b"isom").map_err(|e| e.to_string())?;
        w.write_all(&0u32.to_be_bytes())
            .map_err(|e| e.to_string())?; // minor version
        w.write_all(b"isom").map_err(|e| e.to_string())?;
        w.write_all(b"iso2").map_err(|e| e.to_string())?;
        w.write_all(b"mp41").map_err(|e| e.to_string())?;
        Ok(())
    })?;

    // mdat - we'll write packets here, then come back for moov.
    // Reserve space for mdat header (8 bytes) - we'll patch the size after.
    let mdat_start = writer.stream_position().map_err(|e| e.to_string())?;
    writer.write_all(&[0u8; 8]).map_err(|e| e.to_string())?; // placeholder size + type
    let mdat_data_start = writer.stream_position().map_err(|e| e.to_string())?;

    for pkt in &packets {
        writer.write_all(pkt).map_err(|e| e.to_string())?;
    }

    let mdat_end = writer.stream_position().map_err(|e| e.to_string())?;
    let mdat_size = mdat_end - mdat_data_start + 8; // include header

    // Patch mdat size.
    writer
        .seek(std::io::SeekFrom::Start(mdat_start))
        .map_err(|e| e.to_string())?;
    writer
        .write_all(&(mdat_size as u32).to_be_bytes())
        .map_err(|e| e.to_string())?;
    writer.write_all(b"mdat").map_err(|e| e.to_string())?;
    writer
        .seek(std::io::SeekFrom::Start(mdat_end))
        .map_err(|e| e.to_string())?;

    // moov
    let mut moov_data = Vec::new();
    {
        let mut w = std::io::Cursor::new(&mut moov_data);

        // mvhd
        write_box(&mut w, b"mvhd", |w| {
            w.write_all(&0u8.to_be_bytes()).map_err(|e| e.to_string())?; // version
            w.write_all(&[0u8; 3]).map_err(|e| e.to_string())?; // flags
            w.write_all(&now_mac().to_be_bytes())
                .map_err(|e| e.to_string())?; // creation time
            w.write_all(&now_mac().to_be_bytes())
                .map_err(|e| e.to_string())?; // modification time
            w.write_all(&sample_rate.to_be_bytes())
                .map_err(|e| e.to_string())?; // timescale
            w.write_all(&(duration_ms as u32).to_be_bytes())
                .map_err(|e| e.to_string())?; // duration (in timescale units)
            w.write_all(&0x00010000u32.to_be_bytes())
                .map_err(|e| e.to_string())?; // rate 1.0
            w.write_all(&0x0100u16.to_be_bytes())
                .map_err(|e| e.to_string())?; // volume 1.0
            w.write_all(&[0u8; 10]).map_err(|e| e.to_string())?; // reserved
                                                                 // matrix
            w.write_all(&[
                0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x40, 0x00, 0x00, 0x00, 0x00, 0x00,
                0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                0x00, 0x40, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x40,
            ])
            .map_err(|e| e.to_string())?;
            w.write_all(&0u64.to_be_bytes())
                .map_err(|e| e.to_string())?; // next track id
            Ok(())
        })?;

        // trak
        write_box(&mut w, b"trak", |w| {
            // tkhd
            write_box(w, b"tkhd", |w| {
                w.write_all(&0u8.to_be_bytes()).map_err(|e| e.to_string())?; // version
                w.write_all(&[0u8; 3]).map_err(|e| e.to_string())?; // flags
                w.write_all(&now_mac().to_be_bytes())
                    .map_err(|e| e.to_string())?;
                w.write_all(&now_mac().to_be_bytes())
                    .map_err(|e| e.to_string())?;
                w.write_all(&1u32.to_be_bytes())
                    .map_err(|e| e.to_string())?; // track id
                w.write_all(&[0u8; 4]).map_err(|e| e.to_string())?; // reserved
                w.write_all(&(duration_ms as u32).to_be_bytes())
                    .map_err(|e| e.to_string())?; // duration
                w.write_all(&[0u8; 8]).map_err(|e| e.to_string())?; // reserved
                w.write_all(&[0u8; 2]).map_err(|e| e.to_string())?; // layer
                w.write_all(&[0u8; 2]).map_err(|e| e.to_string())?; // alternate group
                w.write_all(&0x0100u16.to_be_bytes())
                    .map_err(|e| e.to_string())?; // volume
                w.write_all(&[0u8; 2]).map_err(|e| e.to_string())?; // reserved
                                                                    // matrix
                w.write_all(&[
                    0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x40, 0x00, 0x00, 0x00, 0x00,
                    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                    0x00, 0x00, 0x00, 0x40, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                    0x00, 0x00, 0x40,
                ])
                .map_err(|e| e.to_string())?;
                w.write_all(&0u32.to_be_bytes())
                    .map_err(|e| e.to_string())?; // width
                w.write_all(&0u32.to_be_bytes())
                    .map_err(|e| e.to_string())?; // height
                Ok(())
            })?;

            // mdia
            write_box(w, b"mdia", |w| {
                // mdhd
                write_box(w, b"mdhd", |w| {
                    w.write_all(&0u8.to_be_bytes()).map_err(|e| e.to_string())?;
                    w.write_all(&[0u8; 3]).map_err(|e| e.to_string())?;
                    w.write_all(&now_mac().to_be_bytes())
                        .map_err(|e| e.to_string())?;
                    w.write_all(&now_mac().to_be_bytes())
                        .map_err(|e| e.to_string())?;
                    w.write_all(&sample_rate.to_be_bytes())
                        .map_err(|e| e.to_string())?;
                    w.write_all(&(duration_ms as u32).to_be_bytes())
                        .map_err(|e| e.to_string())?;
                    w.write_all(&0x55C4u16.to_be_bytes())
                        .map_err(|e| e.to_string())?; // language (und)
                    w.write_all(&0u16.to_be_bytes())
                        .map_err(|e| e.to_string())?; // quality
                    Ok(())
                })?;

                // hdlr
                write_box(w, b"hdlr", |w| {
                    w.write_all(&0u8.to_be_bytes()).map_err(|e| e.to_string())?;
                    w.write_all(&[0u8; 3]).map_err(|e| e.to_string())?;
                    w.write_all(&[0u8; 4]).map_err(|e| e.to_string())?; // component type
                    w.write_all(b"soun").map_err(|e| e.to_string())?; // component subtype
                    w.write_all(&[0u8; 12]).map_err(|e| e.to_string())?; // manufacturer, flags, mask
                    w.write_all(b"SoundHandler\0").map_err(|e| e.to_string())?; // component name
                    Ok(())
                })?;

                // minf
                write_box(w, b"minf", |w| {
                    // smhd
                    write_box(w, b"smhd", |w| {
                        w.write_all(&0u8.to_be_bytes()).map_err(|e| e.to_string())?;
                        w.write_all(&[0u8; 3]).map_err(|e| e.to_string())?;
                        w.write_all(&[0u8; 2]).map_err(|e| e.to_string())?; // balance
                        w.write_all(&[0u8; 2]).map_err(|e| e.to_string())?; // reserved
                        Ok(())
                    })?;

                    // dinf
                    write_box(w, b"dinf", |w| {
                        write_box(w, b"dref", |w| {
                            w.write_all(&0u8.to_be_bytes()).map_err(|e| e.to_string())?;
                            w.write_all(&[0u8; 3]).map_err(|e| e.to_string())?;
                            w.write_all(&1u32.to_be_bytes())
                                .map_err(|e| e.to_string())?; // entry count
                            write_box(w, b"url ", |w| {
                                w.write_all(&0u8.to_be_bytes()).map_err(|e| e.to_string())?;
                                w.write_all(&[0u8; 3]).map_err(|e| e.to_string())?; // flags: self-contained
                                Ok(())
                            })?;
                            Ok(())
                        })?;
                        Ok(())
                    })?;

                    // stbl
                    write_box(w, b"stbl", |w| {
                        // stsd
                        write_box(w, b"stsd", |w| {
                            w.write_all(&0u8.to_be_bytes()).map_err(|e| e.to_string())?;
                            w.write_all(&[0u8; 3]).map_err(|e| e.to_string())?;
                            w.write_all(&1u32.to_be_bytes())
                                .map_err(|e| e.to_string())?; // entry count
                                                              // mp4a sample entry
                            w.write_all(&[0u8; 6]).map_err(|e| e.to_string())?; // reserved
                            w.write_all(&1u16.to_be_bytes())
                                .map_err(|e| e.to_string())?; // data reference index
                            w.write_all(&[0u8; 8]).map_err(|e| e.to_string())?; // reserved
                            w.write_all(&2u16.to_be_bytes())
                                .map_err(|e| e.to_string())?; // channel count
                            w.write_all(&0x0100u16.to_be_bytes())
                                .map_err(|e| e.to_string())?; // sample size (16-bit)
                            w.write_all(&[0u8; 4]).map_err(|e| e.to_string())?; // pre_defined, reserved
                            w.write_all(&sample_rate.to_be_bytes())
                                .map_err(|e| e.to_string())?; // sample rate (16.16)
                            w.write_all(&0u16.to_be_bytes())
                                .map_err(|e| e.to_string())?; // reserved
                                                              // esds
                            let asc = audio_specific_config_bytes(sample_rate, 2);
                            write_box(w, b"esds", |w| {
                                w.write_all(&0u8.to_be_bytes()).map_err(|e| e.to_string())?;
                                w.write_all(&[0u8; 3]).map_err(|e| e.to_string())?;
                                // ES_Descr
                                w.write_all(&[0x03]).map_err(|e| e.to_string())?; // tag
                                write_varint(w, 27 + asc.len() as u64)?;
                                w.write_all(&0x0001u16.to_be_bytes())
                                    .map_err(|e| e.to_string())?; // ES_ID
                                w.write_all(&[0x00]).map_err(|e| e.to_string())?; // streamDependenceFlag, URL_Flag, OCRstreamFlag
                                                                                  // DecoderConfigDescr
                                w.write_all(&[0x04]).map_err(|e| e.to_string())?; // tag
                                write_varint(w, 13 + asc.len() as u64)?;
                                w.write_all(&[0x40]).map_err(|e| e.to_string())?; // objectType: AAC
                                w.write_all(&[0x15, 0x00, 0x00])
                                    .map_err(|e| e.to_string())?; // streamType: audio, upStream=0, reserved
                                w.write_all(&bitrate_bps.to_be_bytes())
                                    .map_err(|e| e.to_string())?; // maxBitrate
                                w.write_all(&bitrate_bps.to_be_bytes())
                                    .map_err(|e| e.to_string())?; // avgBitrate
                                                                  // DecSpecificInfo
                                w.write_all(&[0x05]).map_err(|e| e.to_string())?; // tag
                                write_varint(w, asc.len() as u64)?;
                                w.write_all(&asc).map_err(|e| e.to_string())?;
                                Ok(())
                            })?;
                            Ok(())
                        })?;

                        // stts
                        write_box(w, b"stts", |w| {
                            w.write_all(&0u8.to_be_bytes()).map_err(|e| e.to_string())?;
                            w.write_all(&[0u8; 3]).map_err(|e| e.to_string())?;
                            w.write_all(&1u32.to_be_bytes())
                                .map_err(|e| e.to_string())?; // entry count
                                                              // Each AAC frame is 1024 samples
                            let samples_per_frame = 1024u32;
                            w.write_all(&packets.len().to_be_bytes())
                                .map_err(|e| e.to_string())?;
                            w.write_all(&samples_per_frame.to_be_bytes())
                                .map_err(|e| e.to_string())?;
                            Ok(())
                        })?;

                        // stsc
                        write_box(w, b"stsc", |w| {
                            w.write_all(&0u8.to_be_bytes()).map_err(|e| e.to_string())?;
                            w.write_all(&[0u8; 3]).map_err(|e| e.to_string())?;
                            w.write_all(&1u32.to_be_bytes())
                                .map_err(|e| e.to_string())?; // entry count
                            w.write_all(&1u32.to_be_bytes())
                                .map_err(|e| e.to_string())?; // first chunk
                            w.write_all(&1u32.to_be_bytes())
                                .map_err(|e| e.to_string())?; // samples per chunk = 1
                            w.write_all(&1u32.to_be_bytes())
                                .map_err(|e| e.to_string())?; // sample description index
                            Ok(())
                        })?;

                        // stsz
                        write_box(w, b"stsz", |w| {
                            w.write_all(&0u8.to_be_bytes()).map_err(|e| e.to_string())?;
                            w.write_all(&[0u8; 3]).map_err(|e| e.to_string())?;
                            w.write_all(&0u32.to_be_bytes())
                                .map_err(|e| e.to_string())?; // sample size (0 = variable)
                            w.write_all(&packets.len().to_be_bytes())
                                .map_err(|e| e.to_string())?; // sample count
                            for pkt in &packets {
                                w.write_all(&(pkt.len() as u32).to_be_bytes())
                                    .map_err(|e| e.to_string())?;
                            }
                            Ok(())
                        })?;

                        // stco
                        write_box(w, b"stco", |w| {
                            w.write_all(&0u8.to_be_bytes()).map_err(|e| e.to_string())?;
                            w.write_all(&[0u8; 3]).map_err(|e| e.to_string())?;
                            w.write_all(&1u32.to_be_bytes())
                                .map_err(|e| e.to_string())?; // entry count
                                                              // mdat_data_start is the offset of the first packet
                            w.write_all(&(mdat_data_start as u32).to_be_bytes())
                                .map_err(|e| e.to_string())?;
                            Ok(())
                        })?;

                        Ok(())
                    })?;

                    Ok(())
                })?;

                Ok(())
            })?;

            Ok(())
        })?;
    }

    write_box(&mut writer, b"moov", |w| {
        w.write_all(&moov_data).map_err(|e| e.to_string())
    })?;

    writer.flush().map_err(|e| e.to_string())?;
    Ok(duration_ms)
}

/// Seconds since Mac epoch for mvhd/mdhd/tkhd.
fn now_mac() -> u32 {
    let unix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default();
    unix.saturating_add(2_082_844_800) as u32
}

/// Write a box with the given type and payload.
fn write_box<W: Write + Seek>(
    w: &mut W,
    typ: &[u8; 4],
    f: impl FnOnce(&mut W) -> Result<(), String>,
) -> Result<(), String> {
    let start = w.stream_position().map_err(|e| e.to_string())?;
    w.write_all(&[0u8; 4]).map_err(|e| e.to_string())?; // size placeholder
    w.write_all(typ).map_err(|e| e.to_string())?;
    f(w)?;
    let end = w.stream_position().map_err(|e| e.to_string())?;
    let size = end - start;
    w.seek(std::io::SeekFrom::Start(start))
        .map_err(|e| e.to_string())?;
    w.write_all(&(size as u32).to_be_bytes())
        .map_err(|e| e.to_string())?;
    w.seek(std::io::SeekFrom::Start(end))
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Write a variable-length integer (MP4 style).
fn write_varint<W: Write>(w: &mut W, mut v: u64) -> Result<(), String> {
    let mut bytes = Vec::new();
    loop {
        bytes.push((v & 0x7F) as u8);
        v >>= 7;
        if v == 0 {
            break;
        }
    }
    bytes.reverse();
    for (i, b) in bytes.iter().enumerate() {
        let mut out = *b;
        if i < bytes.len() - 1 {
            out |= 0x80;
        }
        w.write_all(&[out]).map_err(|e| e.to_string())?;
    }
    Ok(())
}

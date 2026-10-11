//! Finding a connected iPod and the key its database is signed with.
//!
//! # Detection needs no new code
//!
//! A mass-storage iPod is a FAT partition on a USB stick, so it is already in
//! `/proc/self/mounts` and already in `library::Volume::mounted_volumes()` — it
//! appears in the Library sidebar's **Volumes** section with no work at all.
//! The only thing missing was knowing *which* volume is an iPod, which is one
//! file:
//!
//! ```text
//! <mount>/iPod_Control/iTunes/iTunesDB
//! ```
//!
//! That path is the same on every mass-storage iPod, and on an iPod touch too,
//! which is why it is a better test than the volume label: a user who mounts
//! their music on a USB stick called "IPOD" is still not an iPod.
//!
//! # The GUID, and why `SysInfo` is not enough
//!
//! `hash58` is keyed on the device's FireWire GUID. The usual home for it is
//! `iPod_Control/Device/SysInfo`, as a line reading
//! `FirewireGuid: 0x000A27002158F9D4`.
//!
//! **The device this was written against has a zero-byte `SysInfo`.** So does
//! every device that has never been synced by iTunes, and the iPod Classic with
//! an empty `SysInfo` is the common case on Linux, not the exotic one. A reader
//! that only looks there cannot sign anything, and the failure it produces is a
//! checksum error on the device that says nothing about where the key came from.
//!
//! The GUID is the USB serial number, and the kernel already publishes it:
//! `udevadm info -q property -n /dev/sde1` reports `ID_SERIAL_SHORT`. So the
//! lookup is `SysInfo` first, then `udevadm`, and the fallback is the path that
//! actually runs. `tests/fixtures/ipod/SysInfo` is empty for the same reason —
//! so the fallback stays exercised by the one device we can test against.

use crate::ipod::hash58::Guid;
use std::path::{Path, PathBuf};
use std::process::Command;

/// A mounted iPod, and the key to sign its database with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Device {
    /// The mount point — the volume root.
    pub root: PathBuf,
    pub guid: Guid,
    /// Where this device's backups are kept — **off the device**.
    ///
    /// A field rather than an argument because four functions commit a file and
    /// none of them should have to be able to forget where the safety net goes —
    /// and because a test then sets it to its own directory and is *unable* to
    /// write into the developer's real cache. That is the same rule
    /// `library_db::TrackDb::at` follows for `library.json`, and it is not
    /// optional: there is no constructor that leaves it unset.
    pub backups: PathBuf,
}

impl Device {
    /// Where the database lives.
    pub fn database(&self) -> PathBuf {
        database_at(&self.root)
    }

    /// The directory holding `F00`…`F49`.
    pub fn music_dir(&self) -> PathBuf {
        self.root.join("iPod_Control").join("Music")
    }

    /// Where a copy of `file` is kept — **off the device**, in the app's own
    /// cache directory.
    ///
    /// **Not beside the original, which is where a `.bak` conventionally goes and
    /// is exactly wrong here.** The device is the thing that gets unplugged and
    /// put in a drawer, so a safety net stored on it is one nobody finds when
    /// they need it; and it is FAT, so every copy is a full rewrite of a 1.9 MB
    /// file rather than a cheap rename. Worse, it is a file the user can see and
    /// is expected to tidy up — and the failure mode of a backup nobody deletes
    /// is that it becomes clutter that trains the eye to ignore the next one.
    ///
    /// Keyed on the **FireWire GUID**, not the volume label: the label is what a
    /// user changes (`25-ssd-2` → whatever they called the disk) and it is not
    /// unique, so a rename would orphan a backup and two same-named volumes would
    /// overwrite each other's. One file per device, overwritten — the backup is
    /// the state before the *last* write, which is what undoes the thing the user
    /// just did; keeping a history here is a separate decision and not this
    /// function's.
    pub fn backup_of(&self, file: &Path) -> PathBuf {
        let key: String = self.guid.iter().map(|b| format!("{b:02x}")).collect();
        self.backups
            .join(key)
            .join(file.file_name().unwrap_or_default())
    }
}

/// The real `<cache>/tplay/ipod/` — what [`Device::at`] puts in the field, and
/// what a caller outside this module needs to know about to find a backup.
pub fn backup_root() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("tplay")
        .join("ipod")
}

/// The database path inside an iPod mount.
pub fn database_at(root: &Path) -> PathBuf {
    root.join("iPod_Control").join("iTunes").join("iTunesDB")
}

/// Whether `root` is an iPod this app can write to.
///
/// The file's existence is the test, not the volume label: every mass-storage
/// iPod has this path and a USB stick named after one does not.
pub fn is_ipod(root: &Path) -> bool {
    database_at(root).is_file()
}

/// The first mounted iPod, or `None`.
///
/// Scans the same volume list the sidebar draws, so the device the button acts
/// on is a device the user can also click on.
pub fn find() -> Option<Device> {
    crate::library::Volume::mounted_volumes()
        .iter()
        .find_map(|volume| at(&volume.path))
}

/// The iPod at `root`, with its GUID resolved.
///
/// `None` for a volume that is not an iPod, and for an iPod whose GUID cannot
/// be found — there is no way to write a database without it, and a write that
/// looks like it worked until the device refuses to show music is worse than
/// one that never starts.
pub fn at(root: &Path) -> Option<Device> {
    if !is_ipod(root) {
        return None;
    }
    Some(Device {
        root: root.to_path_buf(),
        guid: guid_at(root)?,
        backups: backup_root(),
    })
}

/// The GUID for an iPod at `root`: `SysInfo` first, then udev.
pub fn guid_at(root: &Path) -> Option<Guid> {
    guid_from_sysinfo(&root.join("iPod_Control").join("Device").join("SysInfo"))
        .or_else(|| guid_from_udev(root))
}

// ── udisks: a device that is plugged in, mounted or not ──────────────────────

/// A plugged-in iPod, whether or not anything has mounted it yet.
///
/// **The mount point is not how you find one.** `iPod_Control` lives on the
/// filesystem, so a device with nothing mounted has nothing to read — and the
/// honest answer is that *no amount of reading the block device tells you*, since
/// an unmounted FAT stick carries no marker at all. What does know is
/// **udisks**, from the USB product string: it lists the drive as
/// `Apple_iPod_<serial>`, and that serial is the FireWire GUID. So the device is
/// identifiable, and keyable, before one byte of its filesystem is readable.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plugged {
    /// The whole disk, e.g. `/dev/sde`. What `power-off` takes.
    pub disk: PathBuf,
    /// The partition holding the music, e.g. `/dev/sde1`. What `mount` takes.
    pub partition: PathBuf,
    /// The FireWire GUID, as udisks spells it (`000A27002158F9D4`).
    pub serial: String,
    /// The filesystem label, if the partition carries one.
    pub label: String,
    /// Where it is mounted, or `None` when nothing has mounted it.
    pub mount: Option<PathBuf>,
}

impl Plugged {
    /// What the sidebar calls it. Falls back to the model's own name, so a
    /// device with no label is still named rather than drawn blank.
    pub fn name(&self) -> &str {
        if self.label.is_empty() {
            "iPod"
        } else {
            &self.label
        }
    }
}

/// Every iPod udisks can see, mounted or not.
///
/// **Three `udisksctl` calls, because each step needs one thing.** `status` is
/// the only cheap enumeration and it is a plain table; `info -b` is the only
/// flat key/value listing — `dump` carries the same data but spreads a
/// property's value over continuation lines, which is a parser nobody should
/// trust for a device they cannot replace. Both are read-only, and both are only
/// called when the sidebar is actually drawn.
///
/// `udisksctl -p` is deliberately not used: it wants exactly the object paths
/// `dump` prints, and on this machine it rejects them.
pub fn plugged() -> Vec<Plugged> {
    let Some(status) = run(&["status"]) else {
        return Vec::new();
    };
    parse_status(&status)
        .into_iter()
        .filter_map(|drive| plugged_for(&drive))
        .collect()
}

/// One drive's partition, label and mount state.
fn plugged_for(drive: &Drive) -> Option<Plugged> {
    let disk = run(&["info", "-b", &drive.device])?;
    let partition = partition_device(&disk)?;
    let info = run(&["info", "-b", &partition.to_string_lossy()])?;
    Some(Plugged {
        disk: PathBuf::from(&drive.device),
        partition,
        serial: drive.serial.clone(),
        label: property(&info, "IdLabel").unwrap_or_default(),
        mount: property(&info, "MountPoints").map(PathBuf::from),
    })
}

/// Mount it, and say where it landed.
///
/// The printed path is the happy path; the `info` lookup is the fallback, because
/// a polkit agent that grants without prompting puts the path somewhere else.
pub fn mount(device: &Plugged) -> Result<PathBuf, String> {
    let out = udisks(&[
        "mount",
        "-b",
        &device.partition.to_string_lossy(),
        "--no-user-interaction",
    ])?;
    if let Some(path) = out
        .lines()
        .map(str::trim)
        .find(|l| l.starts_with('/') && l.contains('/'))
    {
        return Ok(PathBuf::from(path));
    }
    let info = udisks(&["info", "-b", &device.partition.to_string_lossy()])?;
    property(&info, "MountPoints")
        .map(PathBuf::from)
        .ok_or_else(|| "the iPod was mounted but tplay could not find where".into())
}

/// Unmount and power off, so the device can be unplugged.
///
/// **Power-off is the half that matters and the half people forget.** An unmount
/// alone leaves the drive spinning up, and pulling the cable then is what
/// corrupts a FAT volume — so eject is both steps, and it is one button because
/// the second only means anything after the first.
pub fn eject(device: &Plugged) -> Result<(), String> {
    udisks(&[
        "unmount",
        "-b",
        &device.partition.to_string_lossy(),
        "--no-user-interaction",
    ])?;
    // A drive that will not power off — another partition still mounted — is
    // reported, not swallowed: the user has to know the cable is not yet safe.
    udisks(&[
        "power-off",
        "-b",
        &device.disk.to_string_lossy(),
        "--no-user-interaction",
    ])
    .map(|_| ())
}

/// Run `udisksctl`, and fail on anything but success.
fn udisks(args: &[&str]) -> Result<String, String> {
    let out = std::process::Command::new("udisksctl")
        .args(args)
        .output()
        .map_err(|e| format!("udisksctl could not be run ({e}); is udisks2 installed?"))?;
    if out.status.success() {
        return Ok(String::from_utf8_lossy(&out.stdout).into_owned());
    }
    // udisksctl writes the real reason to stderr and exits non-zero — a polkit
    // refusal arrives as text, so this is the only message there is.
    let stderr = String::from_utf8_lossy(&out.stderr);
    Err(stderr
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("udisksctl failed")
        .to_string())
}

/// Run a read-only `udisksctl`, or `None` when it is absent or fails.
///
/// Read-only on purpose: discovery runs while a pane is being drawn, and it must
/// never be able to bring up a polkit prompt of its own.
fn run(args: &[&str]) -> Option<String> {
    let out = std::process::Command::new("udisksctl")
        .args(args)
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// One drive row of `udisksctl status`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Drive {
    pub model: String,
    /// The FireWire GUID, which is the USB serial on every iPod measured.
    pub serial: String,
    /// e.g. `/dev/sde`.
    pub device: String,
}

/// Every drive whose model says iPod.
///
/// **The model is the only thing that identifies a device** — it comes from the
/// USB product string, so it is present whether or not anything is mounted, and
/// it is the whole reason a mounted check is not a discovery check. Matching is
/// `iPod` as a substring rather than equality, because the family spells itself
/// variously (`Apple iPod`, `Apple iPod Nano`, `Apple MP3 Player` — the last is
/// the exception this does not reach, and it is a pre-2004 device).
///
/// **The columns are read from the right, not counted from the left.** The table
/// is fixed-column, and three of its four fields are single words — but the model
/// is not: the real output spells it `Apple iPod`, and `WDC  WDS100T2B0A-00SM50`
/// puts two words in one field with the spacing of the column rather than of the
/// text. Counting four whitespace-separated fields therefore splits the model and
/// drops the one row that matters most, which is what the first version did on the
/// real machine.
pub fn parse_status(text: &str) -> Vec<Drive> {
    let mut drives = Vec::new();
    // The header is skipped by its own last column, which is `DEVICE` and which
    // no data row spells — more reliable than counting lines, because udisks
    // prints a rule under it whose width depends on the terminal.
    let mut past_header = false;
    for line in text.lines() {
        let trimmed = line.trim_end();
        if !past_header {
            past_header = trimmed.split_whitespace().next_back() == Some("DEVICE");
            continue;
        }
        if trimmed.trim().is_empty() || trimmed.trim_start().starts_with('-') {
            continue;
        }
        let columns: Vec<&str> = trimmed.split_whitespace().collect();
        // The last three are revision, serial and device; the model is the rest.
        let Some(model_len) = columns.len().checked_sub(3) else {
            continue;
        };
        if model_len == 0 {
            continue;
        }
        let model = columns[..model_len].join(" ");
        if !model.to_lowercase().contains("ipod") {
            continue;
        }
        drives.push(Drive {
            model,
            serial: columns[columns.len() - 2].to_string(),
            device: format!("/dev/{}", columns[columns.len() - 1]),
        });
    }
    drives
}

/// One `Key: value` from a `udisksctl info` listing.
///
/// **`None` for a blank value**, which is how udisks spells "unset" — and an
/// unset `MountPoints` on an unmounted device is the answer this whole feature
/// turns on, so treating it as the empty string rather than absent would be the
/// one mistake that mattered. A key must be followed by two spaces to count,
/// which is what separates a property from an object path ending in a colon.
pub fn property(info: &str, key: &str) -> Option<String> {
    info.lines()
        .find_map(|line| {
            let (name, rest) = line.split_once(':')?;
            (name.trim() == key && rest.starts_with("  ")).then(|| rest.trim().to_string())
        })
        .filter(|value| !value.is_empty())
}

/// The partition holding the music, from a drive's `Partitions:`.
///
/// The value is a udisks object path and udisksctl's `-p` **rejects it**, so the
/// device name is its last segment. `None` when there is none — a whole-disk
/// device, or a partition table udisks does not report — which is a device with
/// nothing to mount.
pub fn partition_device(info: &str) -> Option<PathBuf> {
    let last = property(info, "Partitions")?
        .split_whitespace()
        .next_back()?
        .to_string();
    let name = last.rsplit('/').next()?.trim_matches(QUOTE);
    (!name.is_empty()).then(|| PathBuf::from("/dev").join(name))
}

/// udisks quotes object paths in some outputs and not others, so the last
/// segment is stripped of quoting rather than assuming either.
const QUOTE: char = '\u{27}';

/// `SysInfo`'s `FirewireGuid` line.
///
/// Returns `None` for a missing file, an empty file, and a file without the
/// line. All three are the same situation to a caller — this device does not
/// carry the key here — and distinguishing them only moves the error message.
pub fn guid_from_sysinfo(sysinfo: &Path) -> Option<Guid> {
    let text = std::fs::read_to_string(sysinfo).ok()?;
    let line = text
        .lines()
        .find(|l| l.trim().to_ascii_lowercase().starts_with("firewireguid"))?;
    let hex = line.split(':').nth(1)?.trim();
    let hex = hex.strip_prefix("0x").or_else(|| hex.strip_prefix("0X"))?;
    parse_guid(hex)
}

/// Sixteen hex digits into eight bytes.
fn parse_guid(hex: &str) -> Option<Guid> {
    let hex = hex.trim();
    if hex.len() != 16 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let mut guid = [0u8; 8];
    for (i, byte) in guid.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(guid)
}

/// The USB serial number, which is the GUID, read through udev.
///
/// `/proc/self/mounts` gives the mount point this app was handed and udev gives
/// the properties of a device node, so the mount is matched back to its node
/// first. Only the mount point is available here, and an iPod's GUID is the same
/// value in both places, so no other lookup is needed.
pub fn guid_from_udev(root: &Path) -> Option<Guid> {
    let device = device_node_for(root)?;
    let output = Command::new("udevadm")
        .args(["info", "-q", "property", "-n", &device])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let properties = String::from_utf8(output.stdout).ok()?;
    for key in ["ID_SERIAL_SHORT=", "ID_USB_SERIAL_SHORT="] {
        if let Some(value) = properties.lines().find_map(|l| l.strip_prefix(key)) {
            if let Some(guid) = parse_guid(value) {
                return Some(guid);
            }
        }
    }
    None
}

/// `/dev/<name>` for whatever device is mounted at `root`.
fn device_node_for(root: &Path) -> Option<String> {
    let mounts = std::fs::read_to_string("/proc/self/mounts").ok()?;
    for line in mounts.lines() {
        let mut fields = line.split_whitespace();
        let (Some(device), Some(mountpoint)) = (fields.next(), fields.next()) else {
            continue;
        };
        // /proc escapes the characters a vfat label can contain; the mount point
        // in the file is octal-escaped while the path this was given is not.
        if unescape_mount(mountpoint) == root {
            return Some(device.to_string());
        }
    }
    None
}

/// `/proc/self/mounts` writes space, tab, newline and backslash as `\040`,
/// `\011`, `\012` and `\134`.
fn unescape_mount(field: &str) -> PathBuf {
    let bytes = field.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\' && i + 3 < bytes.len() {
            if let Ok(v) = u8::from_str_radix(&field[i + 1..i + 4], 8) {
                out.push(v);
                i += 4;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    PathBuf::from(String::from_utf8_lossy(&out).into_owned())
}

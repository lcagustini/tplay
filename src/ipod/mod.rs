//! Talking to a mass-storage iPod.
//!
//! An iPod Classic in disk mode is a FAT partition on a USB stick. This module
//! copies albums and playlists onto one and writes the database the device reads
//! them from.
//!
//! # What "talking to an iPod" actually means here
//!
//! The device does not read a folder of music. It reads `iTunesDB`, a tree of
//! length-prefixed binary chunks, and shows nothing that the tree does not
//! name. The firmware **never rebuilds it** — every tool that appears to, from
//! gtkpod to MediaMonkey, rebuilds it on the *computer* and writes it back. So a
//! file dropped into `iPod_Control/Music/F00/` is not a track that was added; it
//! is a file the device will not notice, ever.
//!
//! Four submodules, in the order the work happens:
//!
//! - [`device`] — find a mounted iPod and the key its database is signed with.
//! - [`db`] — the chunk tree. A reader and a writer, nothing else.
//! - [`hash58`] — the signature. The device shows no music without it.
//! - [`push`] — the work: copy the audio, add the records, sign, write.
//!
//! # The one thing that cannot be tested without the device
//!
//! Nothing in here has been run against an iPod. Every claim below was measured
//! from a real database (see `tests/fixtures/ipod/README.md`) and every one of
//! those measurements is a test, so a writer that produces a file the *firmware*
//! rejects is still possible. The checks that exist are: the format round-trips
//! byte for byte, and the signature reproduces a real device's own signature.
//! What is not checked is whether the firmware reads what this writes.
//!
//! # Devices this does not cover
//!
//! An iPod touch, an iPhone, and a Nano 6th generation or later are **not**
//! mass storage. They need `usbmuxd` and `libimobiledevice`, and their database
//! is a different one again — a signed SQLite library rather than this tree, and
//! the signature is `hashAB`, a white-box AES blob that cannot be reproduced from
//! a published description. `hash58::sign` refuses a scheme it cannot write
//! rather than producing a file the device will silently ignore.

pub mod artwork;
pub mod db;
pub mod device;
pub mod hash58;
pub mod push;
pub mod remove;

/// The device type, re-exported so a caller says `ipod::Device` rather than
/// reaching into a submodule for the type the module is named after.
pub use device::Device;
/// The copy in progress. `Summary` is not re-exported: nothing outside this
/// module names it, and `Job::result` returns it without the caller spelling it.
pub use push::Job;

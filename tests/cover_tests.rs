//! `library::read_cover` — embedded art first, cover-file fallback beside the
//! track, None when neither exists. Pure logic, no audio device needed.

use tplay::library;

use lofty::config::WriteOptions;
use lofty::file::{AudioFile, TaggedFileExt};
use lofty::picture::{MimeType, Picture};
use lofty::tag::{Tag, TagType};

#[path = "common.rs"]
mod common;

/// A tiny real PNG (2×2) so fixtures carry actual image bytes.
fn tiny_png() -> Vec<u8> {
    let mut buf = std::io::Cursor::new(Vec::new());
    image::DynamicImage::new_rgba8(2, 2)
        .write_to(&mut buf, image::ImageFormat::Png)
        .unwrap();
    buf.into_inner()
}

#[test]
fn embedded_picture_wins() {
    let dir = common::test_dir("read_cover_embedded");
    let track = dir.join("track.flac");
    // Header-only FLAC is enough — lofty saves metadata without audio frames.
    common::write_minimal_flac(&track);

    let png = tiny_png();
    let mut tagged = lofty::read_from_path(&track).unwrap();
    let pic = Picture::unchecked(png.clone())
        .mime_type(MimeType::Png)
        .build();
    if let Some(tag) = tagged.primary_tag_mut() {
        tag.push_picture(pic);
    } else {
        let mut tag = Tag::new(TagType::VorbisComments);
        tag.push_picture(pic);
        tagged.insert_tag(tag);
    }
    tagged
        .save_to_path(&track, WriteOptions::default())
        .unwrap();

    assert_eq!(library::read_cover(&track), Some(png.clone()));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn folder_jpg_fallback_when_tags_have_no_art() {
    let dir = common::test_dir("read_cover_folder");
    let track = dir.join("song.wav");
    common::write_wav(&track);
    // No embedded art; a folder.jpg beside the file stands in.
    let art = tiny_png();
    std::fs::write(dir.join("folder.jpg"), &art).unwrap();

    assert_eq!(library::read_cover(&track), Some(art));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn no_art_anywhere_returns_none() {
    let dir = common::test_dir("read_cover_none");
    let track = dir.join("song.wav");
    common::write_wav(&track);
    std::fs::write(dir.join("notes.txt"), b"not art").unwrap();

    assert_eq!(library::read_cover(&track), None);
    let _ = std::fs::remove_dir_all(&dir);
}

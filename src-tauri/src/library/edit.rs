//! Writing metadata back to the files.
//!
//! Everything here goes into standard tags — ID3v2 for mp3, Vorbis comments for
//! flac, MP4 atoms for m4a — so a fix made in hark! is a fix Doppler, MPD, VLC
//! and the phones all see. The one deliberate exception is the perfect-album
//! flag, which has no standard home and lives in `.hark/library.json`.
//!
//! Cover art is written *twice*: embedded in every track, because Doppler on
//! iOS ignores folder images, and as `cover.jpg` beside them, because MPD and
//! most Linux players prefer the file. 41 albums here have no art at all, so
//! this is the path that fixes them.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use lofty::config::{ParseOptions, WriteOptions};
use lofty::file::TaggedFileExt;
use lofty::picture::{MimeType, Picture, PictureType};
use lofty::prelude::{Accessor, ItemKey, TagExt};
use lofty::probe::Probe;
use lofty::tag::Tag;
use serde::Deserialize;

use super::art;
use super::norm::nfc;

/// Longest edge for a written cover.
///
/// Large enough to still look right if this collection ever meets a retina
/// display, small enough that embedding it in all 48 tracks of a box set
/// doesn't add hundreds of megabytes.
const COVER_MAX_EDGE: u32 = 1000;
const COVER_QUALITY: u8 = 92;
pub const COVER_FILENAME: &str = "cover.jpg";

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackEdit {
    /// Absolute path of the file this edit applies to.
    pub path: String,
    pub title: String,
    pub track_no: Option<u32>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AlbumEdit {
    pub artist: String,
    pub title: String,
    pub year: Option<u32>,
    pub tracks: Vec<TrackEdit>,
}

/// How far past the end of an ID3v2 tag we let lofty hunt for the first audio
/// frame before it gives up on identifying the file.
///
/// lofty's default is 1024 bytes, and that is not enough for this collection.
/// Writing a tag re-detects the file type from *content* (reading only needs
/// the extension, which is why these files load and play fine but refuse to
/// save), and detection works by skipping the declared tag and expecting an
/// MPEG frame sync. 46 files here — all of Lamp's "Soyokaze Apartment 201",
/// Radiohead's "In Rainbows" and "Kid A", The Animals' "The Best of The
/// Animals" — carry a second stacked ID3 tag or a block of padding between the
/// first tag and the audio, pushing the sync as far as 22706 bytes out.
///
/// 64KiB clears the worst case here with room to spare while still being a
/// bound rather than an open-ended scan.
const MAX_JUNK_BYTES: usize = 64 * 1024;

fn write_options() -> WriteOptions {
    WriteOptions::default().parse_options(ParseOptions::new().max_junk_bytes(MAX_JUNK_BYTES))
}

/// Save a tag, dropping comments if that is the only thing standing in the way.
///
/// lofty validates frames on write that it was happy to read. Two tracks of
/// Godspeed's "F♯A♯∞" carry a `COMM` frame whose language field is three NUL
/// bytes instead of three ASCII letters, and it refuses to write the file at
/// all because of it — so art could never be embedded there.
///
/// A comment that is already malformed is worth less than the cover art and
/// the tags being written, so the second attempt drops comments rather than
/// losing the whole save. Everything else in the tag is preserved, and a
/// failure that isn't about comments still surfaces as an error.
fn save_tag(tag: &mut Tag, path: &Path) -> Result<()> {
    match tag.save_to_path(path, write_options()) {
        Ok(()) => Ok(()),
        Err(first) => {
            if tag.get_string(ItemKey::Comment).is_none() {
                return Err(first).with_context(|| format!("writing tags to {}", path.display()));
            }
            tag.remove_key(ItemKey::Comment);
            tag.save_to_path(path, write_options()).with_context(|| {
                format!(
                    "writing tags to {} (even after dropping a malformed comment; \
                     the original failure was: {first})",
                    path.display()
                )
            })
        }
    }
}

/// Write album-level and track-level tags.
///
/// `ALBUMARTIST` is set but `ARTIST` is deliberately left alone: 1124 files in
/// this collection have no album artist, which is exactly what scatters
/// compilations in tag-driven players — while per-track `ARTIST` on a
/// compilation is correct data that must not be flattened to one name.
pub fn write_tags(edit: &AlbumEdit) -> Result<usize> {
    let mut written = 0;

    for track in &edit.tracks {
        let path = PathBuf::from(&track.path);
        if !path.is_file() {
            continue;
        }

        let mut tagged = Probe::open(&path)
            .with_context(|| format!("opening {}", path.display()))?
            .read()
            .with_context(|| format!("reading tags from {}", path.display()))?;

        let tag_type = tagged.primary_tag_type();
        if tagged.primary_tag_mut().is_none() {
            tagged.insert_tag(Tag::new(tag_type));
        }
        let tag = tagged
            .primary_tag_mut()
            .context("tag missing immediately after insertion")?;

        tag.set_album(nfc(edit.title.trim()));
        tag.insert_text(ItemKey::AlbumArtist, nfc(edit.artist.trim()));

        let title = nfc(track.title.trim());
        if !title.is_empty() {
            tag.set_title(title);
        }
        match track.track_no {
            Some(n) => tag.set_track(n),
            None => tag.remove_track(),
        }
        match edit.year {
            Some(y) => {
                tag.insert_text(ItemKey::RecordingDate, y.to_string());
            }
            None => {
                tag.remove_key(ItemKey::RecordingDate);
            }
        }

        save_tag(tag, &path)?;
        written += 1;
    }

    Ok(written)
}

/// What installing a cover actually managed to do.
///
/// `failed` exists because this used to be a bare count: one track refusing to
/// take the art looked identical to an album that simply had fewer tracks, and
/// two Godspeed tracks silently kept no art for exactly that reason.
#[derive(Debug, Default)]
pub struct CoverReport {
    pub embedded: usize,
    /// One `path: reason` line per track that would not take the art.
    pub failed: Vec<String>,
}

/// Install a cover image for an album: `cover.jpg` plus embedded art in every
/// track.
///
/// `source` is any image file. It is re-encoded to a bounded JPEG first so a
/// 12MP scan doesn't get embedded forty times over.
///
/// A track that refuses the embed is recorded rather than aborting the run —
/// the rest of the album still gets its art, and `cover.jpg` is already down.
pub fn write_cover(
    album_dir: &Path,
    track_paths: &[PathBuf],
    source: &Path,
) -> Result<CoverReport> {
    let raw = std::fs::read(source)
        .with_context(|| format!("reading {}", source.display()))?;
    let jpeg = art::encode_cover(&raw, COVER_MAX_EDGE, COVER_QUALITY)
        .with_context(|| format!("{} is not a readable image", source.display()))?;

    // The folder image first: if embedding fails partway, at least the album
    // has a cover that every Linux player will pick up.
    std::fs::write(album_dir.join(COVER_FILENAME), &jpeg)
        .with_context(|| format!("writing {}", album_dir.join(COVER_FILENAME).display()))?;

    let mut report = CoverReport::default();
    for path in track_paths {
        if !path.is_file() {
            continue;
        }
        match embed_picture(path, &jpeg) {
            Ok(()) => report.embedded += 1,
            Err(e) => report.failed.push(format!("{}: {e:#}", path.display())),
        }
    }

    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use lofty::file::TaggedFileExt;
    use lofty::prelude::Accessor;
    use lofty::probe::Probe;

    /// Tiny silent files so the tests exercise real taggers rather than mocks.
    const MP3: &[u8] = include_bytes!("../../tests/fixtures/silence.mp3");
    const FLAC: &[u8] = include_bytes!("../../tests/fixtures/silence.flac");

    fn fixture(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("hark-edit-test-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn jpeg(w: u32, h: u32) -> Vec<u8> {
        use image::codecs::jpeg::JpegEncoder;
        use image::{DynamicImage, RgbImage};
        let img = DynamicImage::ImageRgb8(RgbImage::from_pixel(w, h, image::Rgb([200, 160, 40])));
        let mut out = Vec::new();
        JpegEncoder::new_with_quality(&mut out, 90)
            .encode_image(&img.to_rgb8())
            .unwrap();
        out
    }

    fn read_tag(path: &Path) -> lofty::tag::Tag {
        let tagged = Probe::open(path).unwrap().read().unwrap();
        tagged
            .primary_tag()
            .or_else(|| tagged.first_tag())
            .cloned()
            .unwrap()
    }

    fn album_edit(dir: &Path, files: &[&str]) -> AlbumEdit {
        AlbumEdit {
            artist: "Björk".into(),
            title: "Debut".into(),
            year: Some(1993),
            tracks: files
                .iter()
                .enumerate()
                .map(|(i, f)| TrackEdit {
                    path: dir.join(f).to_string_lossy().into_owned(),
                    title: format!("Track {}", i + 1),
                    track_no: Some(i as u32 + 1),
                })
                .collect(),
        }
    }

    #[test]
    fn writes_standard_tags_to_mp3_and_flac() {
        // Both tag formats, because the whole point of writing tags rather
        // than a private database is that other players can read them.
        let dir = fixture("formats");
        std::fs::write(dir.join("a.mp3"), MP3).unwrap();
        std::fs::write(dir.join("b.flac"), FLAC).unwrap();

        let written = write_tags(&album_edit(&dir, &["a.mp3", "b.flac"])).unwrap();
        assert_eq!(written, 2);

        for (file, title) in [("a.mp3", "Track 1"), ("b.flac", "Track 2")] {
            let tag = read_tag(&dir.join(file));
            assert_eq!(tag.album().as_deref(), Some("Debut"), "{file}");
            assert_eq!(tag.title().as_deref(), Some(title), "{file}");
            assert_eq!(
                tag.get_string(ItemKey::AlbumArtist),
                Some("Björk"),
                "{file}: album_artist is the tag 1124 files here are missing"
            );
        }
    }

    #[test]
    fn leaves_per_track_artist_alone() {
        // Flattening ARTIST to the album artist would destroy real data on a
        // compilation, so only ALBUMARTIST is written.
        let dir = fixture("artist");
        let path = dir.join("a.mp3");
        std::fs::write(&path, MP3).unwrap();
        {
            let mut tagged = Probe::open(&path).unwrap().read().unwrap();
            let tag_type = tagged.primary_tag_type();
            tagged.insert_tag(Tag::new(tag_type));
            let tag = tagged.primary_tag_mut().unwrap();
            tag.set_artist("Some Guest Vocalist".to_string());
            tag.save_to_path(&path, WriteOptions::default()).unwrap();
        }

        write_tags(&album_edit(&dir, &["a.mp3"])).unwrap();

        let tag = read_tag(&path);
        assert_eq!(tag.artist().as_deref(), Some("Some Guest Vocalist"));
        assert_eq!(tag.get_string(ItemKey::AlbumArtist), Some("Björk"));
    }

    /// Build an mp3 with `junk` bytes wedged between the ID3v2 tag and the
    /// audio, the way 46 files in this collection are — a second stacked tag
    /// or a run of padding left by whatever tagged them.
    ///
    /// Such a file reads and plays fine, because reading takes the file type
    /// from the extension. Writing re-detects it from content, and lofty's
    /// stock 1024-byte search gives up before reaching the audio.
    fn mp3_with_junk_before_audio(junk: usize) -> Vec<u8> {
        let size = ((MP3[6] as usize & 0x7f) << 21)
            | ((MP3[7] as usize & 0x7f) << 14)
            | ((MP3[8] as usize & 0x7f) << 7)
            | (MP3[9] as usize & 0x7f);
        let tag_end = 10 + size;
        let mut out = Vec::with_capacity(MP3.len() + junk);
        out.extend_from_slice(&MP3[..tag_end]);
        out.extend(std::iter::repeat_n(0u8, junk));
        out.extend_from_slice(&MP3[tag_end..]);
        out
    }

    #[test]
    fn writes_tags_through_padding_between_tag_and_audio() {
        let dir = fixture("junk-gap");
        let path = dir.join("padded.mp3");
        // Comfortably past lofty's 1024-byte default, and past the 22706 bytes
        // that Lamp's "Soyokaze Apartment 201" actually carries.
        std::fs::write(&path, mp3_with_junk_before_audio(30_000)).unwrap();

        let edit = album_edit(&dir, &["padded.mp3"]);
        let written = write_tags(&edit).expect("a padded mp3 must still be writable");

        assert_eq!(written, 1);
        let tag = read_tag(&path);
        assert_eq!(tag.album().as_deref(), Some("Debut"));
        assert_eq!(tag.title().as_deref(), Some("Track 1"));
    }

    #[test]
    fn writes_cover_both_as_a_file_and_embedded() {
        // Embedded for Doppler on iOS, which ignores folder images; cover.jpg
        // for MPD and most Linux players, which prefer the file.
        let dir = fixture("cover");
        std::fs::write(dir.join("a.mp3"), MP3).unwrap();
        std::fs::write(dir.join("b.flac"), FLAC).unwrap();
        let source = dir.join("source.png");
        std::fs::write(&source, jpeg(1400, 1400)).unwrap();

        let report = write_cover(
            &dir,
            &[dir.join("a.mp3"), dir.join("b.flac")],
            &source,
        )
        .unwrap();

        assert_eq!(report.embedded, 2);
        assert!(report.failed.is_empty(), "{:?}", report.failed);
        assert!(dir.join(COVER_FILENAME).is_file(), "cover.jpg was not written");

        for file in ["a.mp3", "b.flac"] {
            let tag = read_tag(&dir.join(file));
            let pictures = tag.pictures();
            assert_eq!(pictures.len(), 1, "{file}");
            assert_eq!(pictures[0].pic_type(), PictureType::CoverFront, "{file}");
            assert!(!pictures[0].data().is_empty(), "{file}");
        }
    }

    #[test]
    fn re_saving_art_replaces_rather_than_accumulates() {
        // Otherwise every edit would grow the file by another full-size image.
        let dir = fixture("replace-art");
        let path = dir.join("a.mp3");
        std::fs::write(&path, MP3).unwrap();
        let source = dir.join("source.jpg");
        std::fs::write(&source, jpeg(800, 800)).unwrap();

        write_cover(&dir, &[path.clone()], &source).unwrap();
        write_cover(&dir, &[path.clone()], &source).unwrap();

        assert_eq!(read_tag(&path).pictures().len(), 1, "art accumulated");
    }

    #[test]
    fn a_missing_cover_source_is_an_error_and_writes_nothing() {
        let dir = fixture("missing-cover");
        std::fs::write(dir.join("a.mp3"), MP3).unwrap();
        assert!(write_cover(&dir, &[dir.join("a.mp3")], &dir.join("nope.jpg")).is_err());
        assert!(!dir.join(COVER_FILENAME).exists());
    }

    #[test]
    fn a_track_that_vanished_is_skipped_not_fatal() {
        let dir = fixture("vanished");
        std::fs::write(dir.join("a.mp3"), MP3).unwrap();
        let written = write_tags(&album_edit(&dir, &["a.mp3", "gone.flac"])).unwrap();
        assert_eq!(written, 1);
    }
}

fn embed_picture(path: &Path, jpeg: &[u8]) -> Result<()> {
    let mut tagged = Probe::open(path)?.read()?;

    let tag_type = tagged.primary_tag_type();
    if tagged.primary_tag_mut().is_none() {
        tagged.insert_tag(Tag::new(tag_type));
    }
    let tag = tagged
        .primary_tag_mut()
        .context("tag missing immediately after insertion")?;

    // Replace rather than accumulate: re-saving art repeatedly would otherwise
    // grow every file without bound.
    tag.remove_picture_type(PictureType::CoverFront);
    tag.push_picture(
        Picture::unchecked(jpeg.to_vec())
            .pic_type(PictureType::CoverFront)
            .mime_type(MimeType::Jpeg)
            .build(),
    );

    save_tag(tag, path)
}

//! Reading tags off disk. Writing lives in `edit.rs`.
//!
//! Tags are the display detail; the folder tree is the grouping truth (see
//! `scan.rs`). That split exists because 1124 of the 2105 files in this
//! collection have no `album_artist` at all, so tag-based grouping would
//! scatter compilations that are already correctly foldered.

use std::path::Path;
use std::time::Duration;

use lofty::config::ParseOptions;
use lofty::file::{AudioFile, TaggedFileExt};
use lofty::prelude::{Accessor, ItemKey};
use lofty::probe::Probe;

use super::norm::nfc;

/// What a single audio file tells us about itself.
#[derive(Debug, Clone, Default)]
pub struct TrackTags {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub album_artist: Option<String>,
    pub track_no: Option<u32>,
    pub disc_no: Option<u32>,
    pub year: Option<u32>,
    pub duration: Option<Duration>,
    /// Whether this file carries embedded cover art. Recorded at scan time so
    /// the art protocol knows which file to extract from without re-probing
    /// every track in the album.
    pub has_picture: bool,
}

/// File extensions we treat as audio. Matches what the collection actually
/// contains (1378 mp3, 618 flac, 109 m4a) plus the obvious neighbours.
pub const AUDIO_EXTENSIONS: &[&str] = &["mp3", "flac", "m4a", "ogg", "opus", "wav", "aiff", "aif"];

pub fn is_audio_file(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| AUDIO_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

/// Read tags from one file. A file that fails to parse is not an error worth
/// aborting a 2000-file scan for — it comes back as defaults and still appears
/// in the grid under its filename.
pub fn read(path: &Path) -> TrackTags {
    let probe = match Probe::open(path) {
        Ok(p) => p.options(ParseOptions::new().read_properties(true)),
        Err(_) => return TrackTags::default(),
    };

    let tagged = match probe.read() {
        Ok(t) => t,
        Err(_) => return TrackTags::default(),
    };

    let duration = Some(tagged.properties().duration());

    let tag = match tagged.primary_tag().or_else(|| tagged.first_tag()) {
        Some(t) => t,
        None => {
            return TrackTags {
                duration,
                ..Default::default()
            }
        }
    };

    let text = |s: Option<std::borrow::Cow<'_, str>>| -> Option<String> {
        s.map(|v| nfc(v.trim())).filter(|v| !v.is_empty())
    };

    TrackTags {
        title: text(tag.title()),
        artist: text(tag.artist()),
        album: text(tag.album()),
        album_artist: tag
            .get_string(ItemKey::AlbumArtist)
            .map(|v| nfc(v.trim()))
            .filter(|v| !v.is_empty()),
        track_no: tag.track(),
        disc_no: tag.disk(),
        // `date()` reads RecordingDate and falls back to Year, parsing leniently
        // — which matters here because this collection's `DATE` values range
        // from a bare "2016" to a full "2016-01-08".
        year: tag.date().map(|ts| u32::from(ts.year)),
        duration,
        has_picture: !tag.pictures().is_empty(),
    }
}

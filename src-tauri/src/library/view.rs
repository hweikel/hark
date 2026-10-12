//! The shape the frontend sees.
//!
//! The whole library — every album, every track title — goes over in one
//! payload at startup. For this collection that is roughly 300KB of JSON, which
//! costs a few milliseconds once and buys instant client-side search over song
//! names with no round trips per keystroke.

use serde::Serialize;

use super::scan::{Album, AlbumKind, ArtSource, Library, Track};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackView {
    pub title: String,
    pub artist: String,
    pub track_no: Option<u32>,
    pub disc_no: Option<u32>,
    pub duration_secs: Option<f64>,
    /// Absolute path, opaque to the frontend — handed back when asking to play.
    pub path: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TileView {
    pub id: String,
    pub kind: AlbumKind,
    pub artist: String,
    pub title: String,
    pub year: Option<u32>,
    pub perfect: bool,
    /// `None` for Singles tiles, which have no folder and so can't be flagged
    /// or renamed. Lets the frontend hide controls that don't apply.
    pub rel_path: Option<String>,
    pub art_id: u32,
    pub has_art: bool,
    pub tracks: Vec<TrackView>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryView {
    pub root: String,
    /// False when the configured music directory doesn't exist — the grid comes
    /// up empty and asks for a path instead of looking broken.
    pub exists: bool,
    /// Scan counter, appended to art URLs so the webview can't serve a cover
    /// cached before the library changed underneath it.
    pub generation: u64,
    pub tiles: Vec<TileView>,
}

impl From<&Track> for TrackView {
    fn from(t: &Track) -> Self {
        TrackView {
            title: t.title.clone(),
            artist: t.artist.clone(),
            track_no: t.track_no,
            disc_no: t.disc_no,
            duration_secs: t.duration.map(|d| d.as_secs_f64()),
            path: t.path.to_string_lossy().into_owned(),
        }
    }
}

impl From<&Album> for TileView {
    fn from(a: &Album) -> Self {
        TileView {
            id: a.id.clone(),
            kind: a.kind,
            artist: a.artist.clone(),
            title: a.title.clone(),
            year: a.year,
            perfect: a.perfect,
            rel_path: a.rel_path.clone(),
            art_id: a.art_id,
            has_art: !matches!(a.art, ArtSource::None),
            tracks: a.tracks.iter().map(TrackView::from).collect(),
        }
    }
}

impl LibraryView {
    pub fn from_library(library: &Library, exists: bool, generation: u64) -> Self {
        LibraryView {
            root: library.root.to_string_lossy().into_owned(),
            exists,
            generation,
            tiles: library.albums.iter().map(TileView::from).collect(),
        }
    }
}

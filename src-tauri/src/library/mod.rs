pub mod art;
pub mod edit;
pub mod journal;
pub mod norm;
pub mod playlist;
pub mod rename;
pub mod scan;
pub mod store;
pub mod tags;
pub mod view;

use std::path::{Path, PathBuf};

pub use scan::{Album, AlbumKind, ArtSource, Library, Track};

/// Report the sample rate and channel count a file will actually decode to.
/// Used by the `play_probe` verification binary.
pub fn decode_spec_of(path: &Path) -> anyhow::Result<crate::audio::decode::StreamSpec> {
    Ok(crate::audio::decode::TrackDecoder::open(path)?.spec)
}

/// Where the music lives, in priority order:
///   1. an explicit path argument (`hark /mnt/t7/music`)
///   2. `$HARK_MUSIC_DIR`
///   3. `~/music`
///
/// A missing directory is not an error — the grid comes up empty and asks for a
/// path, which is also the first-run experience on a new machine.
pub fn resolve_music_dir(arg: Option<&str>) -> Option<PathBuf> {
    if let Some(p) = arg {
        return Some(PathBuf::from(shellexpand_home(p)));
    }
    if let Ok(p) = std::env::var("HARK_MUSIC_DIR") {
        if !p.trim().is_empty() {
            return Some(PathBuf::from(shellexpand_home(&p)));
        }
    }
    std::env::var("HOME").ok().map(|h| PathBuf::from(h).join("music"))
}

fn shellexpand_home(p: &str) -> String {
    match p.strip_prefix("~/") {
        Some(rest) => match std::env::var("HOME") {
            Ok(home) => format!("{home}/{rest}"),
            Err(_) => p.to_string(),
        },
        None => p.to_string(),
    }
}

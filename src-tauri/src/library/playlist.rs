//! Playlists as Extended M3U files in `<root>/Playlists/`.
//!
//! This is the fix for the habit that prompted the app: playlists used to be
//! fake compilation *albums*, which forced a song to exist twice if it belonged
//! both to a playlist and to the real album it came from. A playlist here is a
//! list of references — no audio is copied, and a track can appear in as many
//! playlists as it likes while still living on its own album.
//!
//! M3U is used because it is the one playlist format everything already reads:
//! Doppler, VLC, MPD, foobar, a phone. Paths are written relative to the
//! playlist file (`../Björk/Debut/01 Human Behaviour.flac`), which is the
//! convention other players expect and what makes a playlist survive the trip
//! through B2 to another machine.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};

use super::norm::{nfc, sanitize_filename};
use super::store::PLAYLISTS_DIR;

const EXTENSION: &str = "m3u8";

#[derive(Debug, Clone)]
pub struct Playlist {
    /// Display name — the filename without its extension.
    pub name: String,
    pub path: PathBuf,
    /// Absolute paths of tracks that currently exist on disk.
    pub entries: Vec<PathBuf>,
    /// Entries that didn't resolve to a file, kept verbatim.
    ///
    /// Preserved rather than dropped so that adding one song to a playlist
    /// can't silently delete references to music that is merely missing right
    /// now — an album not yet synced to this machine, say.
    pub unresolved: Vec<String>,
    /// A custom cover sitting next to the playlist, `Playlists/<name>.jpg`.
    pub art: Option<PathBuf>,
}

pub fn playlists_dir(root: &Path) -> PathBuf {
    root.join(PLAYLISTS_DIR)
}

/// Read every playlist in the collection. A missing `Playlists/` directory is
/// the normal state until the first playlist is made.
pub fn load_all(root: &Path) -> Vec<Playlist> {
    let dir = playlists_dir(root);
    let Ok(entries) = fs::read_dir(&dir) else {
        return Vec::new();
    };

    let mut playlists: Vec<Playlist> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| is_playlist_file(p))
        .filter_map(|p| load(&p).ok())
        .collect();

    playlists.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    playlists
}

fn is_playlist_file(path: &Path) -> bool {
    path.is_file()
        && path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| {
                let e = e.to_ascii_lowercase();
                e == "m3u8" || e == "m3u"
            })
            .unwrap_or(false)
}

pub fn load(path: &Path) -> Result<Playlist> {
    let text = fs::read_to_string(path)
        .with_context(|| format!("reading playlist {}", path.display()))?;
    let dir = path.parent().unwrap_or(Path::new("."));

    let mut entries = Vec::new();
    let mut unresolved = Vec::new();

    for line in text.lines() {
        let line = line.trim();
        // `#EXTINF` and friends are comments; the real entries are bare paths.
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        let candidate = if Path::new(line).is_absolute() {
            PathBuf::from(line)
        } else {
            dir.join(line)
        };

        match candidate.canonicalize() {
            Ok(resolved) if resolved.is_file() => entries.push(resolved),
            _ => unresolved.push(line.to_string()),
        }
    }

    let name = path
        .file_stem()
        .and_then(|s| s.to_str())
        .map(nfc)
        .ok_or_else(|| anyhow!("playlist has no name: {}", path.display()))?;

    let art = find_playlist_art(dir, &name);

    Ok(Playlist {
        name,
        path: path.to_path_buf(),
        entries,
        unresolved,
        art,
    })
}

fn find_playlist_art(dir: &Path, name: &str) -> Option<PathBuf> {
    for ext in ["jpg", "jpeg", "png", "webp"] {
        let candidate = dir.join(format!("{name}.{ext}"));
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

/// Write a playlist out, preserving any entries that didn't resolve.
pub fn save(root: &Path, playlist: &Playlist) -> Result<()> {
    let dir = playlists_dir(root);
    fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;

    let mut text = String::from("#EXTM3U\n");
    for entry in &playlist.entries {
        text.push_str(&relative_to(&dir, entry));
        text.push('\n');
    }
    for raw in &playlist.unresolved {
        text.push_str(raw);
        text.push('\n');
    }

    // Written atomically: a half-written playlist would lose the references
    // that are the playlist's entire content.
    let tmp = playlist.path.with_extension("m3u8.tmp");
    fs::write(&tmp, text.as_bytes())
        .with_context(|| format!("writing {}", tmp.display()))?;
    fs::rename(&tmp, &playlist.path)
        .with_context(|| format!("replacing {}", playlist.path.display()))?;
    Ok(())
}

/// Path of `target` relative to the playlist directory.
///
/// Both live under the music root, so this is always `../<artist>/...`. Falls
/// back to an absolute path if the two somehow share no prefix, because a
/// working absolute path beats a broken relative one.
///
/// Entries are canonicalized, so the root has to be too: a root reached
/// through a symlink (macOS's `/var` → `/private/var`, or a music folder
/// symlinked onto another disk) otherwise shares no prefix with its own tracks.
fn relative_to(playlist_dir: &Path, target: &Path) -> String {
    let parent = match playlist_dir.parent() {
        Some(p) => p.canonicalize().unwrap_or_else(|_| p.to_path_buf()),
        None => return target.to_string_lossy().into_owned(),
    };
    match target.strip_prefix(&parent) {
        Ok(rest) => format!("../{}", rest.to_string_lossy()),
        Err(_) => target.to_string_lossy().into_owned(),
    }
}

/// Turn a user-supplied playlist name into its file path.
pub fn path_for(root: &Path, name: &str) -> PathBuf {
    playlists_dir(root).join(format!("{}.{EXTENSION}", sanitize_filename(name)))
}

/// Append tracks to a playlist, creating it if necessary.
///
/// Tracks already present are skipped. Returns how many were actually added, so
/// the UI can say "already there" rather than silently doing nothing.
pub fn add_tracks(root: &Path, name: &str, tracks: &[PathBuf]) -> Result<usize> {
    let path = path_for(root, name);

    let mut playlist = if path.exists() {
        load(&path)?
    } else {
        Playlist {
            name: sanitize_filename(name),
            path: path.clone(),
            entries: Vec::new(),
            unresolved: Vec::new(),
            art: None,
        }
    };

    let mut added = 0;
    for track in tracks {
        let resolved = track.canonicalize().unwrap_or_else(|_| track.clone());
        if playlist.entries.iter().any(|e| e == &resolved) {
            continue;
        }
        playlist.entries.push(resolved);
        added += 1;
    }

    if added > 0 {
        save(root, &playlist)?;
    }
    Ok(added)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("hark-playlist-test-{name}"));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        root
    }

    fn touch(path: &Path) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, b"x").unwrap();
    }

    #[test]
    fn writes_relative_paths_so_playlists_survive_syncing() {
        let root = fixture("relative");
        let track = root.join("Björk/Debut/01 Human Behaviour.flac");
        touch(&track);

        add_tracks(&root, "Night Driving", &[track]).unwrap();

        let text = fs::read_to_string(root.join("Playlists/Night Driving.m3u8")).unwrap();
        assert!(text.starts_with("#EXTM3U"));
        assert!(
            text.contains("../Björk/Debut/01 Human Behaviour.flac"),
            "expected a relative path, got:\n{text}"
        );
        assert!(
            !text.contains(root.to_str().unwrap()),
            "absolute paths would break on the other machine:\n{text}"
        );
    }

    #[test]
    fn no_audio_is_copied_only_referenced() {
        let root = fixture("references");
        let track = root.join("A/B/song.mp3");
        touch(&track);
        add_tracks(&root, "Mix", &[track.clone()]).unwrap();

        let playlist_dir = root.join("Playlists");
        let copied: Vec<_> = fs::read_dir(&playlist_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".mp3"))
            .collect();
        assert!(copied.is_empty(), "playlists must reference, not duplicate");
        assert!(track.exists(), "the original must be untouched");
    }

    #[test]
    fn round_trips_through_disk() {
        let root = fixture("roundtrip");
        let a = root.join("A/B/1.mp3");
        let b = root.join("C/D/2.flac");
        touch(&a);
        touch(&b);

        add_tracks(&root, "Mix", &[a.clone(), b.clone()]).unwrap();
        let loaded = load(&path_for(&root, "Mix")).unwrap();

        assert_eq!(loaded.entries.len(), 2);
        assert_eq!(loaded.name, "Mix");
        assert!(loaded.entries[0].ends_with("1.mp3"));
        assert!(loaded.entries[1].ends_with("2.flac"));
    }

    #[test]
    fn a_song_can_be_on_a_playlist_and_still_on_its_album() {
        // The whole point: no duplication, no either/or.
        let root = fixture("both");
        let track = root.join("Radiohead/Kid A/01 Everything.mp3");
        touch(&track);

        add_tracks(&root, "Best Of", &[track.clone()]).unwrap();

        assert!(track.exists());
        let loaded = load(&path_for(&root, "Best Of")).unwrap();
        assert_eq!(loaded.entries[0].canonicalize().unwrap(), track.canonicalize().unwrap());
    }

    #[test]
    fn duplicates_are_skipped() {
        let root = fixture("dupes");
        let track = root.join("A/B/1.mp3");
        touch(&track);

        assert_eq!(add_tracks(&root, "Mix", &[track.clone()]).unwrap(), 1);
        assert_eq!(
            add_tracks(&root, "Mix", &[track.clone()]).unwrap(),
            0,
            "adding the same song twice should be a no-op"
        );
        assert_eq!(load(&path_for(&root, "Mix")).unwrap().entries.len(), 1);
    }

    #[test]
    fn missing_entries_are_preserved_not_silently_dropped() {
        // An album that hasn't synced to this machine yet must not be deleted
        // from the playlist just because another song was added.
        let root = fixture("missing");
        let present = root.join("A/B/1.mp3");
        touch(&present);

        let dir = playlists_dir(&root);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("Mix.m3u8"),
            "#EXTM3U\n../Gone/Album/vanished.flac\n",
        )
        .unwrap();

        add_tracks(&root, "Mix", &[present]).unwrap();

        let text = fs::read_to_string(dir.join("Mix.m3u8")).unwrap();
        assert!(
            text.contains("../Gone/Album/vanished.flac"),
            "unresolved entry was dropped:\n{text}"
        );
        assert!(text.contains("1.mp3"));
    }

    #[test]
    fn comments_are_ignored_when_reading() {
        let root = fixture("comments");
        let track = root.join("A/B/1.mp3");
        touch(&track);
        let dir = playlists_dir(&root);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("Mix.m3u8"),
            "#EXTM3U\n#EXTINF:120,Someone - Something\n../A/B/1.mp3\n",
        )
        .unwrap();

        let loaded = load(&dir.join("Mix.m3u8")).unwrap();
        assert_eq!(loaded.entries.len(), 1);
        assert!(loaded.unresolved.is_empty());
    }

    #[test]
    fn playlist_names_are_sanitized_into_filenames() {
        let root = fixture("naming");
        let track = root.join("A/B/1.mp3");
        touch(&track);
        add_tracks(&root, "Songs: 2024/2025", &[track]).unwrap();

        // The slash must not become a directory.
        assert!(root.join("Playlists/Songs_ 2024 2025.m3u8").is_file());
    }

    #[test]
    fn custom_cover_is_found_next_to_the_playlist() {
        let root = fixture("art");
        let track = root.join("A/B/1.mp3");
        touch(&track);
        add_tracks(&root, "Mix", &[track]).unwrap();
        touch(&root.join("Playlists/Mix.jpg"));

        let loaded = load(&path_for(&root, "Mix")).unwrap();
        assert!(loaded.art.is_some());
    }

    #[test]
    fn no_playlists_directory_is_not_an_error() {
        let root = fixture("none");
        assert!(load_all(&root).is_empty());
    }
}

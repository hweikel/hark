//! Turning a directory tree into a library.
//!
//! The shape on disk is `root/artist/album/song`, and that shape — not the
//! tags — is what hark! groups by. Two reasons, both measured on this actual
//! collection: 1124 of 2105 files carry no `album_artist`, so tag grouping
//! scatters compilations; and the folder tree is what Henry curated by hand, so
//! it reflects intent that tags written by a dozen different rippers don't.
//!
//! Tags still supply everything *within* an album: track titles, numbers, disc
//! numbers, durations, release year.
//!
//! Two subtrees are structurally invisible here: `Playlists/` is a sibling
//! concept rendered in its own zone, and `.hark/` is hark!'s own state. Neither
//! may ever appear as an artist or an album.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use rayon::prelude::*;
use serde::Serialize;

use super::norm::{nfc, sort_key};
use super::playlist;
use super::store::{Store, HARK_DIR, PLAYLISTS_DIR};
use super::tags::{self, TrackTags};

/// Cover-art filenames recognized in an album folder, in preference order.
const COVER_NAMES: &[&str] = &["cover", "folder", "front", "album"];
const COVER_EXTENSIONS: &[&str] = &["jpg", "jpeg", "png", "webp"];

/// Prefix for synthesized Singles tiles. The unit separator can't occur in a
/// path component, so a Singles id can never collide with a real album path.
const SINGLES_PREFIX: &str = "singles\u{1f}";
const PLAYLIST_PREFIX: &str = "playlist\u{1f}";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AlbumKind {
    /// A real folder under `root/artist/`.
    Folder,
    /// Synthesized from loose tracks sitting directly in an artist folder.
    /// 21 such files across 16 artists in this collection.
    Singles,
    /// An `.m3u8` in `Playlists/`. Behaves like an album everywhere in the UI;
    /// the only difference is that its tracks live on other albums.
    Playlist,
}

/// Where an album's cover image comes from. Resolved during the scan, but not
/// *read* until the art protocol asks for it — decoding 212 JPEGs up front
/// would add seconds to launch for images that may never scroll into view.
#[derive(Debug, Clone)]
pub enum ArtSource {
    /// A `cover.jpg` (or similar) sitting next to the tracks.
    File(PathBuf),
    /// Embedded in this specific track's tags.
    Embedded(PathBuf),
    /// Up to four sources composited into a 2×2 grid. Used for playlists with
    /// no custom cover, so a new playlist is never a blank hole in the grid.
    Mosaic(Vec<ArtSource>),
    None,
}

#[derive(Debug, Clone)]
pub struct Track {
    pub path: PathBuf,
    pub title: String,
    pub artist: String,
    pub track_no: Option<u32>,
    pub disc_no: Option<u32>,
    pub duration: Option<Duration>,
    pub has_picture: bool,
}

#[derive(Debug, Clone)]
pub struct Album {
    /// Stable identity. For `Folder`, the NFC relative path (`Björk/Debut`) —
    /// the same string used as a key in `library.json`. For `Singles`, a
    /// namespaced synthetic id.
    pub id: String,
    pub kind: AlbumKind,
    /// `Some` for real folders, `None` for synthesized Singles tiles. This is
    /// the thing a rename changes and the thing `library.json` keys on.
    pub rel_path: Option<String>,
    pub artist: String,
    pub title: String,
    pub artist_sort: String,
    pub title_sort: String,
    pub year: Option<u32>,
    pub perfect: bool,
    pub art: ArtSource,
    pub tracks: Vec<Track>,
    /// The album's real directory on disk, exactly as the filesystem spells it.
    ///
    /// `rel_path` is NFC and is the right key for `library.json`; this is the
    /// only thing safe to pass to `fs::rename` or to join a `cover.jpg` onto.
    /// `None` for Singles piles and playlists, which own no folder.
    pub dir: Option<PathBuf>,
    /// Short opaque handle used in `hark://art/<art_id>` URLs.
    ///
    /// Album ids are paths full of slashes and non-ASCII characters; round
    /// tripping those through a URL is a percent-encoding bug waiting to
    /// happen. An integer assigned after sorting sidesteps it entirely.
    pub art_id: u32,
}

pub struct Library {
    pub root: PathBuf,
    pub albums: Vec<Album>,
}

/// Which album a file belongs to, decided purely from its location.
///
/// Carries both the NFC display names *and* the real on-disk directory. Those
/// are not interchangeable: the folder for Björk is stored decomposed on this
/// collection, so `root.join("Björk")` — the NFC spelling — names a path that
/// does not exist. NFC is for identity (ids, `library.json` keys, display);
/// the raw path is the only thing safe to hand to the filesystem.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum AlbumKey {
    Folder {
        dir: PathBuf,
        artist: String,
        album: String,
    },
    /// Loose tracks directly under an artist folder.
    Singles { dir: PathBuf, artist: String },
}

pub fn scan(root: &Path) -> Library {
    let store = Store::load(root);

    let candidates = collect_candidates(root);

    // The expensive part: opening 2000+ files to read headers and tags. Done in
    // one parallel pass rather than per-album so every core stays busy even
    // though album sizes range from 1 track to 48.
    let scanned: Vec<(AlbumKey, PathBuf, TrackTags)> = candidates
        .into_par_iter()
        .map(|(key, path)| {
            let tags = tags::read(&path);
            (key, path, tags)
        })
        .collect();

    let mut grouped: HashMap<AlbumKey, Vec<(PathBuf, TrackTags)>> = HashMap::new();
    for (key, path, tags) in scanned {
        grouped.entry(key).or_default().push((path, tags));
    }

    let mut albums: Vec<Album> = grouped
        .into_iter()
        .map(|(key, files)| build_album(key, files, &store))
        .collect();

    sort_albums(&mut albums);

    // Playlists are appended after sorting and carry their own ordering: they
    // occupy their own zone at the top of the grid, so interleaving them with
    // albums by artist would be meaningless.
    let playlist_tiles = build_playlists(root, &albums);
    albums.splice(0..0, playlist_tiles);

    // Assigned after sorting so the handles are dense and in grid order.
    for (index, album) in albums.iter_mut().enumerate() {
        album.art_id = index as u32;
    }

    Library {
        root: root.to_path_buf(),
        albums,
    }
}

/// Turn the `.m3u8` files in `Playlists/` into tiles.
///
/// Track metadata is borrowed from the albums the entries point at, which have
/// already been scanned — so a playlist costs no extra tag reads, and a song
/// shows the same title whether you reach it through its album or a playlist.
fn build_playlists(root: &Path, albums: &[Album]) -> Vec<Album> {
    // Index every known track by path so playlist entries resolve in one pass
    // instead of rescanning 2000 files per playlist.
    let mut by_path: HashMap<&Path, &Track> = HashMap::new();
    for album in albums {
        for track in &album.tracks {
            by_path.insert(track.path.as_path(), track);
        }
    }

    playlist::load_all(root)
        .into_iter()
        .map(|playlist| {
            let tracks: Vec<Track> = playlist
                .entries
                .iter()
                .map(|path| match by_path.get(path.as_path()) {
                    Some(known) => (*known).clone(),
                    // A referenced file that exists but isn't in the album
                    // scan — e.g. something parked outside artist/album.
                    None => Track {
                        title: filename_stem(path),
                        artist: String::new(),
                        track_no: None,
                        disc_no: None,
                        duration: None,
                        has_picture: false,
                        path: path.clone(),
                    },
                })
                .collect();

            let art = match &playlist.art {
                Some(file) => ArtSource::File(file.clone()),
                None => mosaic_from(&tracks, &by_path),
            };

            let count = tracks.len();
            Album {
                id: format!("{PLAYLIST_PREFIX}{}", playlist.name),
                kind: AlbumKind::Playlist,
                rel_path: None,
                artist_sort: String::new(),
                title_sort: sort_key(&playlist.name),
                // The muted line under the tile would otherwise be empty; the
                // song count is the one fact about a playlist worth that space.
                artist: if count == 1 {
                    "1 song".to_string()
                } else {
                    format!("{count} songs")
                },
                title: playlist.name,
                year: None,
                perfect: false,
                art,
                tracks,
                dir: None,
                art_id: 0,
            }
        })
        .collect()
}

/// Build a 2×2 cover from the first four *distinct* albums on a playlist.
///
/// Distinct matters: a playlist drawn mostly from one record would otherwise
/// show the same sleeve four times.
fn mosaic_from(tracks: &[Track], by_path: &HashMap<&Path, &Track>) -> ArtSource {
    let _ = by_path;
    let mut seen: Vec<PathBuf> = Vec::new();
    let mut sources: Vec<ArtSource> = Vec::new();

    for track in tracks {
        let Some(dir) = track.path.parent().map(Path::to_path_buf) else {
            continue;
        };
        if seen.contains(&dir) {
            continue;
        }
        let source = match find_cover_file(&dir) {
            Some(file) => ArtSource::File(file),
            None if track.has_picture => ArtSource::Embedded(track.path.clone()),
            None => continue,
        };
        seen.push(dir);
        sources.push(source);
        if sources.len() == 4 {
            break;
        }
    }

    match sources.len() {
        0 => ArtSource::None,
        // One usable cover is better shown whole than stretched across a grid.
        1 => sources.pop().unwrap(),
        _ => ArtSource::Mosaic(sources),
    }
}

/// Walk the tree and decide what belongs where, without opening any files.
fn collect_candidates(root: &Path) -> Vec<(AlbumKey, PathBuf)> {
    let mut out = Vec::new();

    let artist_dirs = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(_) => return out,
    };

    for artist_entry in artist_dirs.filter_map(|e| e.ok()) {
        let artist_path = artist_entry.path();
        if !artist_path.is_dir() {
            // Audio sitting at the very root has no artist to group it under.
            // There is none today; if some appears, it stays invisible rather
            // than inventing a bogus artist for it.
            continue;
        }

        let artist_name = match file_name_of(&artist_path) {
            Some(n) => n,
            None => continue,
        };
        if is_reserved_dir(&artist_name) {
            continue;
        }

        let entries = match std::fs::read_dir(&artist_path) {
            Ok(e) => e,
            Err(_) => continue,
        };

        for entry in entries.filter_map(|e| e.ok()) {
            let path = entry.path();

            if path.is_dir() {
                let album_name = match file_name_of(&path) {
                    Some(n) => n,
                    None => continue,
                };
                if is_reserved_dir(&album_name) {
                    continue;
                }
                // Recursive so multi-disc layouts (`album/CD1/*.flac`) fold into
                // the one album. Disc ordering then comes from the disc tag.
                for file in walkdir::WalkDir::new(&path)
                    .into_iter()
                    .filter_map(|e| e.ok())
                    .filter(|e| e.file_type().is_file())
                {
                    if tags::is_audio_file(file.path()) {
                        out.push((
                            AlbumKey::Folder {
                                dir: path.clone(),
                                artist: artist_name.clone(),
                                album: album_name.clone(),
                            },
                            file.path().to_path_buf(),
                        ));
                    }
                }
            } else if tags::is_audio_file(&path) {
                out.push((
                    AlbumKey::Singles {
                        dir: artist_path.clone(),
                        artist: artist_name.clone(),
                    },
                    path,
                ));
            }
        }
    }

    out
}

/// `Playlists/` and `.hark/` are structurally invisible to the album scan, as
/// are dotfiles generally (which also disposes of the 34 `.DS_Store` files).
fn is_reserved_dir(name: &str) -> bool {
    name.starts_with('.') || name == HARK_DIR || name == PLAYLISTS_DIR
}

fn file_name_of(path: &Path) -> Option<String> {
    path.file_name().and_then(|n| n.to_str()).map(nfc)
}

fn build_album(
    key: AlbumKey,
    files: Vec<(PathBuf, TrackTags)>,
    store: &Store,
) -> Album {
    let (artist, title, kind, rel_path, id, dir) = match &key {
        AlbumKey::Folder { dir, artist, album } => {
            let rel = format!("{artist}/{album}");
            (
                artist.clone(),
                album.clone(),
                AlbumKind::Folder,
                Some(rel.clone()),
                rel,
                Some(dir.clone()),
            )
        }
        AlbumKey::Singles { artist, .. } => (
            artist.clone(),
            "Singles".to_string(),
            AlbumKind::Singles,
            None,
            format!("{SINGLES_PREFIX}{artist}"),
            None,
        ),
    };

    // Decide the year before the tags are consumed into tracks.
    let year = most_common_year(&key, files.iter().map(|(_, t)| t.year));

    let mut tracks: Vec<Track> = files
        .into_iter()
        .map(|(path, t)| {
            let title = t.title.unwrap_or_else(|| filename_stem(&path));
            Track {
                title,
                artist: t.artist.unwrap_or_else(|| artist.clone()),
                track_no: t.track_no,
                disc_no: t.disc_no,
                duration: t.duration,
                has_picture: t.has_picture,
                path,
            }
        })
        .collect();

    sort_tracks(&mut tracks);

    let art = resolve_art(&key, &tracks);
    let perfect = rel_path
        .as_deref()
        .map(|p| store.is_perfect(p))
        .unwrap_or(false);

    Album {
        artist_sort: sort_key(&artist),
        title_sort: sort_key(&title),
        id,
        kind,
        rel_path,
        artist,
        title,
        year,
        perfect,
        art,
        tracks,
        dir,
        art_id: 0, // assigned in `scan` once the final order is known
    }
}

/// Disc, then track number, then filename. Files with no track number sort after
/// numbered ones rather than jumping to the front.
fn sort_tracks(tracks: &mut [Track]) {
    tracks.sort_by(|a, b| {
        a.disc_no
            .unwrap_or(1)
            .cmp(&b.disc_no.unwrap_or(1))
            .then(a.track_no.unwrap_or(u32::MAX).cmp(&b.track_no.unwrap_or(u32::MAX)))
            .then_with(|| natural_cmp(&a.path, &b.path))
    });
}

fn natural_cmp(a: &Path, b: &Path) -> std::cmp::Ordering {
    a.file_name().cmp(&b.file_name())
}

fn filename_stem(path: &Path) -> String {
    path.file_stem()
        .and_then(|s| s.to_str())
        .map(nfc)
        .unwrap_or_else(|| "Unknown".to_string())
}

/// The year the most tracks agree on.
///
/// Taking the first track's year would misdate any album carrying one stray
/// remaster or compilation tag. A Singles pile spans years by definition, so it
/// gets none and sorts last within its artist.
fn most_common_year(key: &AlbumKey, years: impl Iterator<Item = Option<u32>>) -> Option<u32> {
    if matches!(key, AlbumKey::Singles { .. }) {
        return None;
    }

    let mut counts: HashMap<u32, usize> = HashMap::new();
    for year in years.flatten() {
        *counts.entry(year).or_default() += 1;
    }

    counts
        .into_iter()
        // Ties go to the earlier year: an album tagged half 1967 and half 2009
        // is a 1967 album with a remaster's tags on some tracks.
        .max_by(|(year_a, count_a), (year_b, count_b)| {
            count_a.cmp(count_b).then(year_b.cmp(year_a))
        })
        .map(|(year, _)| year)
}

/// Prefer a cover file next to the tracks, then embedded art.
///
/// Both get written on save — a file for Linux players and MPD, embedded for
/// Doppler on iOS, which ignores folder images.
fn resolve_art(key: &AlbumKey, tracks: &[Track]) -> ArtSource {
    if let AlbumKey::Folder { dir, .. } = key {
        if let Some(found) = find_cover_file(dir) {
            return ArtSource::File(found);
        }
    }

    // First track in play order that actually carries a picture. 41 albums here
    // have none at all and will fall through to `None`.
    for track in tracks {
        if track.has_picture {
            return ArtSource::Embedded(track.path.clone());
        }
    }

    ArtSource::None
}

fn find_cover_file(dir: &Path) -> Option<PathBuf> {
    let entries: Vec<_> = std::fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_file())
        .collect();

    for name in COVER_NAMES {
        for ext in COVER_EXTENSIONS {
            let wanted = format!("{name}.{ext}");
            for entry in &entries {
                if let Some(actual) = entry.file_name().to_str() {
                    if actual.eq_ignore_ascii_case(&wanted) {
                        return Some(entry.path());
                    }
                }
            }
        }
    }
    None
}

/// Artist first (article-stripped), then year, then title.
///
/// Year before title means a single artist's tiles read as a discography rather
/// than an alphabetized pile.
fn sort_albums(albums: &mut [Album]) {
    albums.sort_by(|a, b| {
        a.artist_sort
            .cmp(&b.artist_sort)
            .then(a.year.unwrap_or(u32::MAX).cmp(&b.year.unwrap_or(u32::MAX)))
            .then_with(|| a.title_sort.cmp(&b.title_sort))
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn fixture(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("hark-scan-test-{name}"));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        root
    }

    fn touch(path: &Path) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, b"").unwrap();
    }

    #[test]
    fn groups_by_folder_not_by_tag() {
        let root = fixture("grouping");
        touch(&root.join("Radiohead/Kid A/01 Everything In Its Right Place.mp3"));
        touch(&root.join("Radiohead/Kid A/02 Kid A.mp3"));
        touch(&root.join("Radiohead/Amnesiac/01 Packt.mp3"));

        let lib = scan(&root);
        assert_eq!(lib.albums.len(), 2);
        let kid_a = lib.albums.iter().find(|a| a.title == "Kid A").unwrap();
        assert_eq!(kid_a.tracks.len(), 2);
        assert_eq!(kid_a.rel_path.as_deref(), Some("Radiohead/Kid A"));
    }

    #[test]
    fn loose_tracks_become_one_singles_tile_per_artist() {
        let root = fixture("singles");
        // Mirrors the real collection: Sophie Hunger has two loose tracks.
        touch(&root.join("Sophie Hunger/Le vent nous portera.mp3"));
        touch(&root.join("Sophie Hunger/Wakzer Für Niemand.mp3"));
        touch(&root.join("Judy Collins/Sons of.mp3"));

        let lib = scan(&root);
        let singles: Vec<_> = lib
            .albums
            .iter()
            .filter(|a| a.kind == AlbumKind::Singles)
            .collect();
        assert_eq!(singles.len(), 2, "one tile per artist, not per track");

        let hunger = singles.iter().find(|a| a.artist == "Sophie Hunger").unwrap();
        assert_eq!(hunger.tracks.len(), 2);
        assert_eq!(hunger.title, "Singles");
        assert!(hunger.rel_path.is_none(), "Singles have no folder of their own");
    }

    #[test]
    fn an_artist_can_have_both_albums_and_singles() {
        let root = fixture("mixed");
        touch(&root.join("CW Stoneking/Gon' Boogaloo/01 The Zombie.mp3"));
        touch(&root.join("CW Stoneking/The Love Me or Die.mp3"));

        let lib = scan(&root);
        assert_eq!(lib.albums.len(), 2);
        assert!(lib.albums.iter().any(|a| a.kind == AlbumKind::Folder));
        assert!(lib.albums.iter().any(|a| a.kind == AlbumKind::Singles));
    }

    #[test]
    fn playlists_and_hark_dirs_never_become_albums() {
        let root = fixture("reserved");
        // An audio file parked in Playlists/ must not turn into an album, and
        // .hark/ must not turn into an artist.
        touch(&root.join("Playlists/stray.mp3"));
        touch(&root.join(".hark/library.json"));
        touch(&root.join("Radiohead/Kid A/01 a.mp3"));

        let lib = scan(&root);
        let albums: Vec<_> = lib
            .albums
            .iter()
            .filter(|a| a.kind != AlbumKind::Playlist)
            .collect();
        assert_eq!(albums.len(), 1, "only the real album should appear");
        assert_eq!(albums[0].artist, "Radiohead");
        assert!(
            !lib.albums.iter().any(|a| a.artist == "Playlists" || a.artist == ".hark"),
            "a reserved directory leaked in as an artist"
        );
    }

    #[test]
    fn a_playlist_becomes_its_own_tile_not_an_album() {
        let root = fixture("playlist-tile");
        let track = root.join("Radiohead/Kid A/01 Everything.mp3");
        touch(&track);
        super::playlist::add_tracks(&root, "Night Driving", &[track.clone()]).unwrap();

        let lib = scan(&root);
        let playlists: Vec<_> = lib
            .albums
            .iter()
            .filter(|a| a.kind == AlbumKind::Playlist)
            .collect();

        assert_eq!(playlists.len(), 1);
        assert_eq!(playlists[0].title, "Night Driving");
        assert_eq!(playlists[0].tracks.len(), 1);
        assert_eq!(playlists[0].artist, "1 song");
        assert!(playlists[0].rel_path.is_none(), "playlists have no album folder");
        // The song is still on its album — that non-duplication is the point.
        assert!(lib
            .albums
            .iter()
            .any(|a| a.kind == AlbumKind::Folder && a.title == "Kid A"));
    }

    #[test]
    fn playlist_tiles_sort_before_albums_and_among_themselves() {
        let root = fixture("playlist-order");
        let track = root.join("A/B/1.mp3");
        touch(&track);
        super::playlist::add_tracks(&root, "Zebra", &[track.clone()]).unwrap();
        super::playlist::add_tracks(&root, "Antelope", &[track.clone()]).unwrap();

        let lib = scan(&root);
        let names: Vec<_> = lib
            .albums
            .iter()
            .take(2)
            .map(|a| a.title.as_str())
            .collect();
        assert_eq!(names, vec!["Antelope", "Zebra"]);
    }

    #[test]
    fn ds_store_and_other_junk_is_ignored() {
        let root = fixture("junk");
        touch(&root.join(".DS_Store"));
        touch(&root.join("Radiohead/.DS_Store"));
        touch(&root.join("Radiohead/Kid A/.DS_Store"));
        touch(&root.join("Radiohead/Kid A/notes.txt"));
        touch(&root.join("Radiohead/Kid A/01 a.mp3"));

        let lib = scan(&root);
        assert_eq!(lib.albums.len(), 1);
        assert_eq!(lib.albums[0].tracks.len(), 1, "only the audio file counts");
    }

    #[test]
    fn the_two_bjorks_collapse_into_one_tile() {
        // The decomposed spelling is what macOS hands back for this folder.
        let root = fixture("bjork");
        touch(&root.join("Bjo\u{0308}rk/Debut/01 Human Behaviour.flac"));

        let lib = scan(&root);
        assert_eq!(lib.albums.len(), 1);
        // Whatever the filesystem said, the id is NFC — so it matches a
        // library.json entry written from a precomposed tag.
        assert_eq!(lib.albums[0].id, "Bj\u{00f6}rk/Debut");
        assert_eq!(lib.albums[0].artist, "Bj\u{00f6}rk");
    }

    #[test]
    fn multi_disc_folders_fold_into_one_album() {
        let root = fixture("multidisc");
        touch(&root.join("Artist/Album/CD1/01 a.mp3"));
        touch(&root.join("Artist/Album/CD2/01 b.mp3"));

        let lib = scan(&root);
        assert_eq!(lib.albums.len(), 1);
        assert_eq!(lib.albums[0].tracks.len(), 2);
    }

    #[test]
    fn albums_sort_by_article_stripped_artist() {
        let root = fixture("sorting");
        touch(&root.join("The Strokes/Is This It/01 a.mp3"));
        touch(&root.join("Aurora/All My Demons/01 a.mp3"));
        touch(&root.join("Tycho/Dive/01 a.mp3"));

        let lib = scan(&root);
        let order: Vec<_> = lib.albums.iter().map(|a| a.artist.as_str()).collect();
        // Aurora, then The Strokes (under S), then Tycho.
        assert_eq!(order, vec!["Aurora", "The Strokes", "Tycho"]);
    }

    #[test]
    fn cover_file_wins_over_embedded_and_is_case_insensitive() {
        let root = fixture("cover");
        touch(&root.join("Artist/Album/01 a.mp3"));
        touch(&root.join("Artist/Album/Cover.JPG"));

        let lib = scan(&root);
        match &lib.albums[0].art {
            ArtSource::File(p) => assert!(p.to_string_lossy().contains("Cover.JPG")),
            other => panic!("expected a cover file, got {other:?}"),
        }
    }

    #[test]
    fn albums_with_no_art_at_all_are_not_an_error() {
        // 41 albums in the real collection are in this state.
        let root = fixture("noart");
        touch(&root.join("Artist/Album/01 a.mp3"));

        let lib = scan(&root);
        assert!(matches!(lib.albums[0].art, ArtSource::None));
    }

    #[test]
    fn a_missing_root_yields_an_empty_library_not_a_panic() {
        let lib = scan(Path::new("/nonexistent/path/to/music"));
        assert!(lib.albums.is_empty());
    }

    #[test]
    fn year_is_the_consensus_not_the_first_track() {
        let key = AlbumKey::Folder { dir: PathBuf::from("/A/B"), artist: "A".into(), album: "B".into() };
        // One stray remaster tag must not redate the album.
        let years = vec![Some(1967), Some(1967), Some(2009), Some(1967)];
        assert_eq!(most_common_year(&key, years.into_iter()), Some(1967));
    }

    #[test]
    fn year_ties_go_to_the_earlier_year() {
        let key = AlbumKey::Folder { dir: PathBuf::from("/A/B"), artist: "A".into(), album: "B".into() };
        let years = vec![Some(1967), Some(2009)];
        assert_eq!(most_common_year(&key, years.into_iter()), Some(1967));
    }

    #[test]
    fn singles_piles_have_no_year() {
        let key = AlbumKey::Singles { dir: PathBuf::from("/Sophie Hunger"), artist: "Sophie Hunger".into() };
        let years = vec![Some(2010), Some(2015)];
        assert_eq!(most_common_year(&key, years.into_iter()), None);
    }

    #[test]
    fn untagged_albums_have_no_year() {
        let key = AlbumKey::Folder { dir: PathBuf::from("/A/B"), artist: "A".into(), album: "B".into() };
        let years = vec![None, None];
        assert_eq!(most_common_year(&key, years.into_iter()), None);
    }
}

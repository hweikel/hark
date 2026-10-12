//! Application state, Tauri commands, and the `hark://` art protocol.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::RwLock;

use tauri::http::{Request, Response, StatusCode};
use tauri::{Manager, State};

use crate::audio::{Command, PlaybackState, Player};
use crate::library::art::{self, ArtCache};
use crate::library::edit;
use crate::library::playlist;
use crate::library::rename;
use crate::library::scan::{self, AlbumKind, ArtSource, Library};
use crate::library::store::Store;
use crate::library::view::LibraryView;

pub struct AppState {
    pub library: RwLock<Library>,
    pub art: ArtCache,
    pub player: Player,
    /// Bumped on every rescan and embedded in art URLs.
    ///
    /// Art handles are dense indices, so a rescan usually hands out the *same*
    /// id for a different tile — or the same id for a tile whose cover changed,
    /// as when a playlist gains an album. Without this, the webview's HTTP
    /// cache happily serves the old image forever.
    generation: AtomicU64,
}

impl AppState {
    pub fn new(root: PathBuf, player: Player) -> Self {
        AppState {
            library: RwLock::new(scan::scan(&root)),
            art: ArtCache::default(),
            player,
            generation: AtomicU64::new(1),
        }
    }

    fn view(&self) -> LibraryView {
        let library = self.library.read().expect("library lock poisoned");
        let exists = library.root.is_dir();
        LibraryView::from_library(
            &library,
            exists,
            self.generation.load(Ordering::Relaxed),
        )
    }

    /// Replace the library after something on disk changed.
    fn reload(&self, root: &PathBuf) {
        let scanned = scan::scan(root);
        if let Ok(mut library) = self.library.write() {
            *library = scanned;
        }
        self.art.clear();
        self.generation.fetch_add(1, Ordering::Relaxed);
    }

    /// Look up an album's art source by its `hark://` handle.
    fn art_source(&self, art_id: u32) -> Option<ArtSource> {
        let library = self.library.read().ok()?;
        library
            .albums
            .iter()
            .find(|a| a.art_id == art_id)
            .map(|a| a.art.clone())
    }
}

#[tauri::command]
pub fn get_library(state: State<'_, AppState>) -> LibraryView {
    state.view()
}

/// Re-read the collection from disk. Used after an external change, and after
/// the user supplies a path on first run.
#[tauri::command]
pub fn rescan(state: State<'_, AppState>, root: Option<String>) -> LibraryView {
    let next_root = match root {
        Some(r) if !r.trim().is_empty() => PathBuf::from(expand_home(r.trim())),
        _ => state
            .library
            .read()
            .expect("library lock poisoned")
            .root
            .clone(),
    };

    state.reload(&next_root);
    state.view()
}

/// Mark or unmark an album as perfect, promoting it into the marquee.
///
/// Singles tiles have no folder, so there is nothing to key a flag on and
/// nothing to curate — they're the fodder pile by definition.
#[tauri::command]
pub fn set_perfect(
    state: State<'_, AppState>,
    id: String,
    perfect: bool,
) -> Result<LibraryView, String> {
    let (root, rel_path) = {
        let library = state.library.read().map_err(|_| "library lock poisoned")?;
        let album = library
            .albums
            .iter()
            .find(|a| a.id == id)
            .ok_or_else(|| format!("no album with id {id}"))?;
        let rel = album
            .rel_path
            .clone()
            .ok_or("Singles piles can't be marked as perfect albums")?;
        (library.root.clone(), rel)
    };

    let mut store = Store::load(&root);
    store
        .set_perfect(&rel_path, perfect)
        .map_err(|e| format!("{e:#}"))?;

    // Reflect it in memory rather than rescanning the whole collection for a
    // one-bit change.
    {
        let mut library = state.library.write().map_err(|_| "library lock poisoned")?;
        if let Some(album) = library.albums.iter_mut().find(|a| a.id == id) {
            album.perfect = perfect;
        }
    }

    Ok(state.view())
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveReport {
    pub tracks_written: usize,
    pub covers_embedded: usize,
    /// Tracks that would not take the embedded art. Non-zero means the album
    /// has `cover.jpg` but some files will still look bare in Doppler.
    pub covers_failed: usize,
    /// `Some("from → to")` when the folder actually moved.
    pub renamed: Option<String>,
}

/// What the edit modal does when you hit Save.
///
/// Tags, cover art, the perfect flag and any folder rename all happen in one
/// call, in that order: write the data first, move the folder last. If the
/// rename fails (a collision), the metadata edits still stand rather than being
/// silently lost.
#[tauri::command]
pub fn save_album(
    state: State<'_, AppState>,
    id: String,
    edit: edit::AlbumEdit,
    cover_path: Option<String>,
    perfect: bool,
) -> Result<SaveReport, String> {
    let (root, rel_path, album_dir, track_paths) = {
        let library = state.library.read().map_err(|_| "library lock poisoned")?;
        let album = library
            .albums
            .iter()
            .find(|a| a.id == id)
            .ok_or_else(|| format!("no album with id {id}"))?;
        let rel = album
            .rel_path
            .clone()
            .ok_or("only real album folders can be edited")?;
        // The album's real directory, not `root.join(rel)`: those differ for
        // any folder whose name is stored decomposed.
        let dir = album
            .dir
            .clone()
            .ok_or("only real album folders can be edited")?;
        let paths: Vec<PathBuf> = album.tracks.iter().map(|t| t.path.clone()).collect();
        (library.root.clone(), rel, dir, paths)
    };

    let tracks_written = edit::write_tags(&edit).map_err(|e| format!("{e:#}"))?;

    let cover = match cover_path.as_deref().filter(|p| !p.trim().is_empty()) {
        Some(source) => edit::write_cover(&album_dir, &track_paths, Path::new(source))
            .map_err(|e| format!("{e:#}"))?,
        None => edit::CoverReport::default(),
    };

    Store::load(&root)
        .set_perfect(&rel_path, perfect)
        .map_err(|e| format!("{e:#}"))?;

    let renamed = match rename::rename_album(&root, &album_dir, &rel_path, &edit.artist, &edit.title)
        .map_err(|e| format!("{e:#}"))?
    {
        rename::Outcome::Unchanged => None,
        rename::Outcome::Renamed { from, to } => Some(format!("{from} → {to}")),
    };

    state.reload(&root);

    Ok(SaveReport {
        tracks_written,
        covers_embedded: cover.embedded,
        covers_failed: cover.failed.len(),
        renamed,
    })
}

/// Where saving would move this album, so the modal can show it before you
/// commit. Returns `None` when nothing would move.
#[tauri::command]
pub fn planned_rename(
    state: State<'_, AppState>,
    id: String,
    artist: String,
    title: String,
) -> Option<String> {
    let library = state.library.read().ok()?;
    let album = library.albums.iter().find(|a| a.id == id)?;
    let from = album.rel_path.as_ref()?;
    let to = rename::planned_path(&artist, &title);
    if &to == from {
        None
    } else {
        Some(format!("{from} → {to}"))
    }
}

/// Add tracks to a playlist, creating it if it doesn't exist.
///
/// `tile_id` plus an optional `track_index`: with an index, one song is added;
/// without, the whole album or playlist is. Returns how many were actually
/// added so the UI can distinguish "done" from "already there".
#[tauri::command]
pub fn playlist_add(
    state: State<'_, AppState>,
    name: String,
    tile_id: String,
    track_index: Option<usize>,
) -> Result<usize, String> {
    let name = name.trim().to_string();
    if name.is_empty() {
        return Err("a playlist needs a name".into());
    }

    let (root, tracks) = {
        let library = state.library.read().map_err(|_| "library lock poisoned")?;
        let album = library
            .albums
            .iter()
            .find(|a| a.id == tile_id)
            .ok_or_else(|| format!("no tile with id {tile_id}"))?;

        let tracks: Vec<PathBuf> = match track_index {
            Some(index) => album
                .tracks
                .get(index)
                .map(|t| vec![t.path.clone()])
                .ok_or_else(|| format!("no track {index} on {tile_id}"))?,
            None => album.tracks.iter().map(|t| t.path.clone()).collect(),
        };
        (library.root.clone(), tracks)
    };

    let added = playlist::add_tracks(&root, &name, &tracks).map_err(|e| format!("{e:#}"))?;

    if added > 0 {
        // A new or changed playlist changes the grid, so the library has to be
        // rebuilt — playlist tiles are derived from what's on disk.
        state.reload(&root);
    }

    Ok(added)
}

/// Save the playlist `e` modal: name, entry order and removals, and cover.
///
/// `tracks` is the playlist's new contents in order, as absolute paths taken
/// from the tile. `cover_path` sets a custom cover; `clear_cover` drops it and
/// brings back the mosaic. Returns the tile id afterwards, which changes with
/// the name.
#[tauri::command]
pub fn save_playlist(
    state: State<'_, AppState>,
    id: String,
    name: String,
    tracks: Vec<String>,
    cover_path: Option<String>,
    clear_cover: bool,
) -> Result<String, String> {
    let (root, current) = {
        let library = state.library.read().map_err(|_| "library lock poisoned")?;
        let tile = library
            .albums
            .iter()
            .find(|a| a.id == id && a.kind == AlbumKind::Playlist)
            .ok_or_else(|| format!("no playlist with id {id}"))?;
        (library.root.clone(), tile.title.clone())
    };

    let cover = match cover_path.as_deref().map(str::trim).filter(|p| !p.is_empty()) {
        Some(source) => playlist::CoverChange::Set(PathBuf::from(source)),
        None if clear_cover => playlist::CoverChange::Clear,
        None => playlist::CoverChange::Keep,
    };
    let entries: Vec<PathBuf> = tracks.iter().map(PathBuf::from).collect();

    let saved = playlist::update(&root, &current, &name, &entries, cover)
        .map_err(|e| format!("{e:#}"))?;

    let new_id = scan::playlist_id(&saved);
    state.player.rename_tile(&id, &new_id);
    state.reload(&root);
    Ok(new_id)
}

/// Names of existing playlists, for the `a` picker.
#[tauri::command]
pub fn playlist_names(state: State<'_, AppState>) -> Vec<String> {
    let Ok(library) = state.library.read() else {
        return Vec::new();
    };
    library
        .albums
        .iter()
        .filter(|a| a.kind == AlbumKind::Playlist)
        .map(|a| a.title.clone())
        .collect()
}

/// Start a tile playing from a given track.
///
/// Playlists resolve to the same thing an album does — a list of file paths —
/// which is the whole point of making them real playlist objects instead of
/// folders full of duplicated songs.
#[tauri::command]
pub fn play_tile(state: State<'_, AppState>, id: String, start: usize) -> Result<(), String> {
    let tracks: Vec<PathBuf> = {
        let library = state.library.read().map_err(|_| "library lock poisoned")?;
        let album = library
            .albums
            .iter()
            .find(|a| a.id == id)
            .ok_or_else(|| format!("no tile with id {id}"))?;
        album.tracks.iter().map(|t| t.path.clone()).collect()
    };

    if tracks.is_empty() {
        return Err("nothing to play".into());
    }

    state.player.send(Command::Load {
        tile_id: id,
        tracks,
        start: start.min(usize::MAX),
    });
    Ok(())
}

#[tauri::command]
pub fn playback_toggle(state: State<'_, AppState>) {
    state.player.send(Command::Toggle);
}

#[tauri::command]
pub fn playback_next(state: State<'_, AppState>) {
    state.player.send(Command::Next);
}

#[tauri::command]
pub fn playback_previous(state: State<'_, AppState>) {
    state.player.send(Command::Previous);
}

#[tauri::command]
pub fn playback_play_index(state: State<'_, AppState>, index: usize) {
    state.player.send(Command::PlayIndex(index));
}

#[tauri::command]
pub fn playback_set_shuffle(state: State<'_, AppState>, shuffle: bool) {
    state.player.send(Command::SetShuffle(shuffle));
}

#[tauri::command]
pub fn playback_state(state: State<'_, AppState>) -> PlaybackState {
    state.player.state()
}

fn expand_home(p: &str) -> String {
    match p.strip_prefix("~/") {
        Some(rest) => match std::env::var("HOME") {
            Ok(home) => format!("{home}/{rest}"),
            Err(_) => p.to_string(),
        },
        None => p.to_string(),
    }
}

/// Serve `hark://localhost/art/<art_id>?size=<px>`.
///
/// Asynchronous because decoding and Lanczos-resizing a 1200px JPEG takes long
/// enough that doing it on the main thread would visibly hitch scrolling.
pub fn handle_art_request<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    request: Request<Vec<u8>>,
    responder: tauri::UriSchemeResponder,
) {
    let app = app.clone();
    let uri = request.uri().clone();

    std::thread::spawn(move || {
        let response = match render(&app, &uri) {
            Ok(bytes) => Response::builder()
                .status(StatusCode::OK)
                .header("Content-Type", "image/jpeg")
                // Safe to cache hard: art handles are reassigned on every
                // rescan, so a stale entry can't outlive its album.
                .header("Cache-Control", "private, max-age=31536000")
                .body(bytes),
            Err(status) => Response::builder().status(status).body(Vec::new()),
        };

        if let Ok(response) = response {
            responder.respond(response);
        }
    });
}

fn render<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    uri: &tauri::http::Uri,
) -> Result<Vec<u8>, StatusCode> {
    let state = app.state::<AppState>();

    let art_id: u32 = uri
        .path()
        .rsplit('/')
        .next()
        .and_then(|s| s.parse().ok())
        .ok_or(StatusCode::BAD_REQUEST)?;

    let size = art::clamp_size(
        uri.query()
            .and_then(|q| {
                q.split('&')
                    .find_map(|pair| pair.strip_prefix("size="))
                    .and_then(|v| v.parse().ok())
            })
            .unwrap_or(360),
    );

    if let Some(cached) = state.art.get(art_id, size) {
        return Ok(cached);
    }

    let source = state.art_source(art_id).ok_or(StatusCode::NOT_FOUND)?;
    if matches!(source, ArtSource::None) {
        // 41 albums are in this state. The frontend knows from `hasArt` and
        // shouldn't ask, but a 404 keeps it honest.
        return Err(StatusCode::NOT_FOUND);
    }

    let bytes = art::render_tile(&source, size).map_err(|_| StatusCode::NOT_FOUND)?;
    state.art.put(art_id, size, bytes.clone());
    Ok(bytes)
}

import { invoke } from "@tauri-apps/api/core";
import type { AlbumEdit, LibraryView, PlaybackState, SaveReport } from "./types";

export const getLibrary = () => invoke<LibraryView>("get_library");

export const rescan = (root?: string) => invoke<LibraryView>("rescan", { root });

export const setPerfect = (id: string, perfect: boolean) =>
  invoke<LibraryView>("set_perfect", { id, perfect });

export const saveAlbum = (
  id: string,
  edit: AlbumEdit,
  coverPath: string | null,
  perfect: boolean,
) => invoke<SaveReport>("save_album", { id, edit, coverPath, perfect });

export const plannedRename = (id: string, artist: string, title: string) =>
  invoke<string | null>("planned_rename", { id, artist, title });

export const playlistNames = () => invoke<string[]>("playlist_names");

export const playlistAdd = (name: string, tileId: string, trackIndex: number | null) =>
  invoke<number>("playlist_add", { name, tileId, trackIndex });

/** Returns the playlist's tile id afterwards, which changes with its name. */
export const savePlaylist = (
  id: string,
  name: string,
  tracks: string[],
  coverPath: string | null,
  clearCover: boolean,
) => invoke<string>("save_playlist", { id, name, tracks, coverPath, clearCover });

export const playTile = (id: string, start: number) =>
  invoke<void>("play_tile", { id, start });

export const playbackToggle = () => invoke<void>("playback_toggle");
export const playbackNext = () => invoke<void>("playback_next");
export const playbackPrevious = () => invoke<void>("playback_previous");
export const playbackPlayIndex = (index: number) =>
  invoke<void>("playback_play_index", { index });
export const playbackSetShuffle = (shuffle: boolean) =>
  invoke<void>("playback_set_shuffle", { shuffle });
export const playbackState = () => invoke<PlaybackState>("playback_state");

/**
 * Tauri serves custom schemes as `<scheme>://localhost/...` on Linux and macOS,
 * the two platforms hark! targets. Built by hand rather than with
 * `convertFileSrc`, which percent-encodes the whole path into one segment.
 *
 * `v` is the library's scan generation. Art ids are dense indices, so the same
 * id routinely means a different tile — or the same tile with different art —
 * after a rescan. Without this the webview's cache serves the old cover.
 */
export const artUrl = (artId: number, size: number, generation: number) =>
  `hark://localhost/art/${artId}?size=${size}&v=${generation}`;

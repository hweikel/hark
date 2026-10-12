export type TileKind = "folder" | "singles" | "playlist";

export interface Track {
  title: string;
  artist: string;
  trackNo: number | null;
  discNo: number | null;
  durationSecs: number | null;
  path: string;
}

export interface Tile {
  id: string;
  kind: TileKind;
  artist: string;
  title: string;
  year: number | null;
  perfect: boolean;
  /** null for Singles piles, which have no folder to flag or rename. */
  relPath: string | null;
  artId: number;
  hasArt: boolean;
  tracks: Track[];
}

export interface LibraryView {
  root: string;
  exists: boolean;
  /** Scan counter; art URLs include it so stale covers can't be cached. */
  generation: number;
  tiles: Tile[];
}

export interface PlaybackState {
  /** null before anything has been played — the now-playing row doesn't exist yet. */
  tileId: string | null;
  index: number | null;
  playing: boolean;
  shuffle: boolean;
}

export interface TrackEdit {
  path: string;
  title: string;
  trackNo: number | null;
}

export interface AlbumEdit {
  artist: string;
  title: string;
  year: number | null;
  tracks: TrackEdit[];
}

export interface SaveReport {
  tracksWritten: number;
  coversEmbedded: number;
  /** Tracks that refused the embedded art; cover.jpg still landed. */
  coversFailed: number;
  /** "from → to" when the folder actually moved, else null. */
  renamed: string | null;
}

import { artUrl, playbackNext, playbackPlayIndex, playbackPrevious, playbackSetShuffle, playbackToggle } from "./api";
import type { PlaybackState, Tile } from "./types";

/**
 * Transport glyphs, from the Font Awesome range of JetBrains Mono Nerd Font
 * (already installed system-wide via `fonts.packages`).
 *
 * Plain Unicode was the obvious first choice and the wrong one: this font has
 * no glyph for ⇋, ⇄, ⏸, ⏭ or any of the usual media symbols, so they fell
 * through to whatever fallback the webview could find and rendered as specks.
 * These five are verified present.
 */
const ICON = {
  previous: "\u{f048}",
  play: "\u{f04b}",
  pause: "\u{f04c}",
  next: "\u{f051}",
  shuffle: "\u{f074}",
} as const;

/**
 * The sticky now-playing row.
 *
 * Absent entirely until something is played — on launch there is only the grid.
 * Once it exists it stays, paused or finished, and the grid scrolls underneath.
 */
export class NowPlaying {
  private root: HTMLElement;
  private tile: Tile | null = null;
  private state: PlaybackState | null = null;
  private generation = 0;

  constructor(root: HTMLElement) {
    this.root = root;
  }

  update(state: PlaybackState, tile: Tile | null, generation: number) {
    this.state = state;
    this.tile = tile;
    this.generation = generation;
    this.render();
  }

  get isActive(): boolean {
    return this.state?.tileId != null;
  }

  /** The song currently loaded, for `A` (add this song to a playlist). */
  get currentTrack(): { tileId: string; index: number; title: string } | null {
    const { state, tile } = this;
    if (!state?.tileId || state.index == null || !tile) return null;
    const track = tile.tracks[state.index];
    if (!track) return null;
    return { tileId: state.tileId, index: state.index, title: track.title };
  }

  private render() {
    const state = this.state;
    const tile = this.tile;

    if (!state || state.tileId == null || !tile) {
      this.root.replaceChildren();
      this.root.hidden = true;
      return;
    }
    this.root.hidden = false;

    const cover = document.createElement("div");
    cover.className = "np-cover";
    if (tile.hasArt) {
      const img = document.createElement("img");
      img.src = artUrl(tile.artId, 180, this.generation);
      img.alt = "";
      img.draggable = false;
      cover.append(img);
    } else {
      cover.classList.add("empty");
      const text = document.createElement("span");
      text.className = "cover-text";
      text.textContent = tile.title;
      cover.append(text);
    }

    const side = document.createElement("div");
    side.className = "np-side";

    const heading = document.createElement("div");
    heading.className = "np-heading";
    const title = document.createElement("span");
    title.className = "np-title";
    title.textContent = tile.title;
    const artist = document.createElement("span");
    artist.className = "np-artist";
    artist.textContent = tile.artist;
    heading.append(title, artist);

    const controls = document.createElement("div");
    controls.className = "np-controls";
    controls.append(
      button(ICON.previous, "previous", () => playbackPrevious()),
      button(
        state.playing ? ICON.pause : ICON.play,
        state.playing ? "pause" : "play",
        () => playbackToggle(),
      ),
      button(ICON.next, "next", () => playbackNext()),
      button(ICON.shuffle, "shuffle", () => playbackSetShuffle(!state.shuffle), state.shuffle),
    );

    side.append(heading, controls);

    const list = document.createElement("ol");
    list.className = "np-tracks";
    tile.tracks.forEach((track, index) => {
      const item = document.createElement("li");
      item.className = "np-track";
      if (index === state.index) item.classList.add("current");

      const number = document.createElement("span");
      number.className = "np-num";
      number.textContent = String(index + 1);

      const name = document.createElement("span");
      name.className = "np-name";
      name.textContent = track.title;

      const time = document.createElement("span");
      time.className = "np-time";
      time.textContent = formatDuration(track.durationSecs);

      item.append(number, name, time);
      // Single click, not double: the track list is already a list of things
      // to play, so there is nothing else a click could mean.
      item.addEventListener("click", () => playbackPlayIndex(index));
      list.append(item);
    });

    this.root.replaceChildren(cover, side, list);

    // Keep the playing track in view as an album advances on its own.
    const current = list.querySelector(".np-track.current");
    current?.scrollIntoView({ block: "nearest" });
  }
}

function button(
  glyph: string,
  label: string,
  onClick: () => void,
  active = false,
): HTMLButtonElement {
  const element = document.createElement("button");
  element.className = "np-button";
  element.classList.toggle("on", active);
  element.textContent = glyph;
  element.title = label;
  element.addEventListener("click", onClick);
  return element;
}

function formatDuration(seconds: number | null): string {
  if (seconds == null || !Number.isFinite(seconds)) return "";
  const total = Math.round(seconds);
  const minutes = Math.floor(total / 60);
  return `${minutes}:${String(total % 60).padStart(2, "0")}`;
}

import { listen } from "@tauri-apps/api/event";
import { getLibrary, playTile, playbackState, playbackToggle, rescan } from "./api";
import { Grid } from "./grid";
import { NowPlaying } from "./nowplaying";
import { EditModal } from "./modal";
import { PlaylistModal } from "./playlistmodal";
import { PlaylistPicker } from "./picker";
import type { LibraryView, PlaybackState, Tile } from "./types";

const app = document.querySelector<HTMLDivElement>("#app")!;

let grid: Grid | null = null;
let nowPlaying: NowPlaying | null = null;
let searchInput: HTMLInputElement | null = null;
let countLabel: HTMLElement | null = null;
let tiles: Tile[] = [];
let playback: PlaybackState | null = null;
let generation = 0;
const picker = new PlaylistPicker();
const modal = new EditModal();
const playlistModal = new PlaylistModal();

async function boot() {
  // Playback state changes originate in the audio thread — a track ending is
  // not something the UI initiated — so the frontend listens rather than polls.
  await listen<PlaybackState>("playback", (event) => {
    playback = event.payload;
    paintNowPlaying();
  });

  render(await getLibrary());
  playback = await playbackState();
  paintNowPlaying();
}

function render(library: LibraryView) {
  if (!library.exists) {
    renderSetup(library);
    return;
  }
  renderLibrary(library);
}

/** First run on a machine where the music directory isn't where we looked. */
function renderSetup(library: LibraryView) {
  app.replaceChildren();
  grid = null;
  nowPlaying = null;
  searchInput = null;

  const section = document.createElement("section");
  section.className = "setup";

  const inner = document.createElement("div");
  inner.className = "setup-inner";

  const heading = document.createElement("h1");
  heading.textContent = "hark!";

  const message = document.createElement("p");
  message.textContent = `No music found at ${library.root}. Point hark! at a folder of artist/album/song.`;

  const form = document.createElement("form");
  const input = document.createElement("input");
  input.type = "text";
  input.placeholder = "~/music";
  input.spellcheck = false;
  const button = document.createElement("button");
  button.type = "submit";
  button.textContent = "open";
  form.append(input, button);

  form.addEventListener("submit", async (event) => {
    event.preventDefault();
    const path = input.value.trim();
    if (!path) return;
    render(await rescan(path));
  });

  inner.append(heading, message, form);
  section.append(inner);
  app.append(section);
  input.focus();
}

function renderLibrary(library: LibraryView) {
  app.replaceChildren();
  tiles = library.tiles;
  generation = library.generation;

  const bar = document.createElement("div");
  bar.className = "searchbar";
  const input = document.createElement("input");
  input.type = "text";
  input.placeholder = "search";
  input.spellcheck = false;
  input.autocapitalize = "off";
  input.autocomplete = "off";
  const count = document.createElement("span");
  count.className = "count";
  bar.append(input, count);
  searchInput = input;
  countLabel = count;

  const npRoot = document.createElement("section");
  npRoot.id = "nowplaying";
  npRoot.hidden = true;

  const gridRoot = document.createElement("main");
  gridRoot.id = "grid";

  app.append(bar, npRoot, gridRoot);

  nowPlaying = new NowPlaying(npRoot);
  grid = new Grid(gridRoot, {
    onPlay: (tile) => play(tile.id, 0),
    onActivate: () => {},
  });
  grid.setTiles(library.tiles, generation);
  updateCount(gridRoot, library.tiles.length);

  input.addEventListener("input", () => {
    grid!.setQuery(input.value);
    updateCount(gridRoot, library.tiles.length);
  });

  paintNowPlaying();
}

/**
 * A failed play is worth hearing about: an unreadable file or a busy audio
 * device would otherwise look exactly like nothing happening.
 */
function play(id: string, start: number) {
  playTile(id, start).catch((error) => {
    console.error(`hark: could not play ${id}:`, error);
  });
}

function paintNowPlaying() {
  if (!nowPlaying || !playback) return;
  const tile = tiles.find((t) => t.id === playback!.tileId) ?? null;
  nowPlaying.update(playback, tile, generation);
}

function updateCount(gridRoot: HTMLElement, total: number) {
  if (!countLabel) return;
  const visible = gridRoot.querySelectorAll(".tile").length;
  countLabel.textContent = visible === total ? `${total}` : `${visible}/${total}`;
}

/**
 * `a` adds the album or playlist you're looking at; `A` adds the song you're
 * hearing.
 *
 * Originally sketched as one context-sensitive key, which doesn't survive the
 * obvious case: with music playing while you browse the grid, a single `a`
 * can't mean both "this song" and "that record". Two keys, both always
 * available, and the picker names its target so there's never a guess.
 */
async function addActiveTile() {
  const active = grid?.active;
  if (!active) return;
  await picker.open(active.id, null, `${active.artist} — ${active.title}`, refreshLibrary);
}

async function addPlayingTrack() {
  const current = nowPlaying?.currentTrack;
  if (!current) return;
  await picker.open(current.tileId, current.index, `“${current.title}”`, refreshLibrary);
}

/** Playlists are files on disk, so adding to one changes what the grid shows. */
async function refreshLibrary() {
  const library = await rescan();
  tiles = library.tiles;
  generation = library.generation;
  grid?.setTiles(library.tiles, generation);
  const gridRoot = document.querySelector<HTMLElement>("#grid");
  if (gridRoot) updateCount(gridRoot, library.tiles.length);
  // An edit can change the playing tile's id (a renamed playlist).
  playback = await playbackState();
  paintNowPlaying();
}

document.addEventListener("keydown", (event) => {
  // While an overlay is open it owns the keyboard entirely.
  if (picker.isOpen || modal.isOpen || playlistModal.isOpen) return;

  const typingInSearch = document.activeElement === searchInput;

  // `/` focuses search. Letters stay free for single-key commands, so the grid
  // deliberately does not steal them the way a notes app would.
  if (event.key === "/" && !typingInSearch) {
    event.preventDefault();
    searchInput?.focus();
    return;
  }

  if (event.key === "Escape") {
    if (searchInput && searchInput.value) {
      searchInput.value = "";
      grid?.setQuery("");
      const gridRoot = document.querySelector<HTMLElement>("#grid");
      if (gridRoot) updateCount(gridRoot, tiles.length);
    }
    searchInput?.blur();
    return;
  }

  // Space toggles whatever is loaded, and does nothing at all when nothing is
  // — including while typing a query, where it has to stay a space character.
  if (event.key === " " && !typingInSearch) {
    if (playback?.tileId != null) {
      event.preventDefault();
      void playbackToggle();
    }
    return;
  }

  if (typingInSearch && event.key !== "Enter") {
    // The search field owns its own typing, including arrows for text editing.
    return;
  }

  if (event.key === "e") {
    const active = grid?.active;
    if (active?.kind === "playlist") {
      event.preventDefault();
      playlistModal.open(active, generation, () => void refreshLibrary());
    } else if (active && active.relPath !== null) {
      // Singles piles have no album folder to rename or tag.
      event.preventDefault();
      modal.open(active, () => void refreshLibrary());
    }
    return;
  }

  if (event.key === "a") {
    event.preventDefault();
    void addActiveTile();
    return;
  }

  if (event.key === "A") {
    event.preventDefault();
    void addPlayingTrack();
    return;
  }

  switch (event.key) {
    case "ArrowLeft":
      event.preventDefault();
      grid?.move("left");
      break;
    case "ArrowRight":
      event.preventDefault();
      grid?.move("right");
      break;
    case "ArrowUp":
      event.preventDefault();
      grid?.move("up");
      break;
    case "ArrowDown":
      event.preventDefault();
      grid?.move("down");
      break;
    case "Enter": {
      const active = grid?.active;
      if (active) {
        event.preventDefault();
        searchInput?.blur();
        play(active.id, 0);
      }
      break;
    }
  }
});

boot();

import { artUrl } from "./api";
import { matches, normalizeQuery, tokensFor } from "./search";
import type { Tile } from "./types";

/** Tile render size requested from the art protocol, per zone. */
const ART_SIZE = { marquee: 360, lower: 180 } as const;

interface Zone {
  name: "playlists" | "perfect" | "rest";
  element: HTMLElement;
  tiles: Tile[];
}

export interface GridCallbacks {
  onPlay(tile: Tile): void;
  onActivate(tile: Tile | null): void;
}

export class Grid {
  private root: HTMLElement;
  private callbacks: GridCallbacks;
  private tiles: Tile[] = [];
  private tokens = new Map<string, string[]>();
  private visible: Tile[] = [];
  private generation = 0;
  private zones: Zone[] = [];
  private activeId: string | null = null;
  private query = "";

  constructor(root: HTMLElement, callbacks: GridCallbacks) {
    this.root = root;
    this.callbacks = callbacks;
  }

  setTiles(tiles: Tile[], generation: number) {
    this.tiles = tiles;
    this.generation = generation;
    this.tokens = new Map(tiles.map((t) => [t.id, tokensFor(t)]));
    // Keep the active tile across a rescan when it still exists.
    if (this.activeId && !tiles.some((t) => t.id === this.activeId)) {
      this.activeId = null;
    }
    this.render();
  }

  setQuery(query: string) {
    this.query = normalizeQuery(query);
    this.render();
  }

  get active(): Tile | null {
    return this.tiles.find((t) => t.id === this.activeId) ?? null;
  }

  setActive(id: string | null) {
    this.activeId = id;
    this.paintActive();
    this.callbacks.onActivate(this.active);
  }

  private render() {
    this.visible = this.query
      ? this.tiles.filter((t) => matches(this.tokens.get(t.id) ?? [], this.query))
      : this.tiles;

    // Playlists first, then the marquee of perfect albums, then everything
    // else. Each zone preserves the backend's artist-then-year ordering.
    const grouped: Record<Zone["name"], Tile[]> = {
      playlists: [],
      perfect: [],
      rest: [],
    };
    for (const tile of this.visible) {
      if (tile.kind === "playlist") grouped.playlists.push(tile);
      else if (tile.perfect) grouped.perfect.push(tile);
      else grouped.rest.push(tile);
    }

    this.root.replaceChildren();
    this.zones = [];

    if (this.visible.length === 0) {
      const empty = document.createElement("p");
      empty.className = "empty-result";
      empty.textContent = this.tiles.length === 0 ? "" : "nothing matches";
      this.root.append(empty);
      this.ensureActiveIsVisible();
      return;
    }

    for (const name of ["playlists", "perfect", "rest"] as const) {
      const tiles = grouped[name];
      if (tiles.length === 0) continue;

      // A hairline only ever sits *between* populated zones, so an empty
      // marquee on first run doesn't draw a stray line across the window.
      if (this.zones.length > 0) {
        const rule = document.createElement("div");
        rule.className = "hairline";
        this.root.append(rule);
      }

      const section = document.createElement("section");
      section.className = `zone zone-${name}`;
      for (const tile of tiles) {
        section.append(this.renderTile(tile, name === "rest"));
      }
      this.root.append(section);
      this.zones.push({ name, element: section, tiles });
    }

    this.ensureActiveIsVisible();
    this.paintActive();
  }

  private renderTile(tile: Tile, lower: boolean): HTMLElement {
    const element = document.createElement("div");
    element.className = "tile";
    element.dataset.id = tile.id;

    const cover = document.createElement("div");
    cover.className = "cover";
    if (tile.hasArt) {
      const img = document.createElement("img");
      img.loading = "lazy";
      img.decoding = "async";
      img.draggable = false;
      img.src = artUrl(tile.artId, lower ? ART_SIZE.lower : ART_SIZE.marquee, this.generation);
      img.alt = "";
      // 41 albums have no art and a handful more may fail to decode; fall back
      // to the text tile rather than showing a broken image.
      img.addEventListener("error", () => {
        cover.replaceChildren(this.fallbackCover(tile));
        cover.classList.add("empty");
      });
      cover.append(img);
    } else {
      cover.classList.add("empty");
      cover.append(this.fallbackCover(tile));
    }

    const label = document.createElement("div");
    label.className = "label";
    const title = document.createElement("span");
    title.className = "label-title";
    title.textContent = tile.title;
    const artist = document.createElement("span");
    artist.className = "label-artist";
    artist.textContent = tile.artist;
    label.append(title, artist);

    element.append(cover, label);

    element.addEventListener("click", () => this.setActive(tile.id));
    element.addEventListener("dblclick", () => {
      this.setActive(tile.id);
      this.callbacks.onPlay(tile);
    });

    return element;
  }

  private fallbackCover(tile: Tile): HTMLElement {
    const text = document.createElement("span");
    text.className = "cover-text";
    text.textContent = tile.title;
    return text;
  }

  /**
   * Keep the selection on something the user can actually see.
   *
   * Ordered by zone, not by the backend's artist ordering — otherwise the
   * opening selection is whichever album sorts first overall, which on first
   * run sits buried in the lower zone rather than at the top of the grid.
   */
  private ensureActiveIsVisible() {
    const flat = this.zones.flatMap((z) => z.tiles);
    if (flat.length === 0) {
      // Nothing matches the search: there is no active tile, so `e` and `a`
      // correctly do nothing until the query is cleared.
      if (this.activeId !== null) {
        this.activeId = null;
        this.callbacks.onActivate(null);
      }
      return;
    }
    if (!this.activeId || !flat.some((t) => t.id === this.activeId)) {
      this.activeId = flat[0].id;
      this.callbacks.onActivate(this.active);
    }
  }

  private paintActive() {
    for (const element of this.root.querySelectorAll<HTMLElement>(".tile")) {
      element.classList.toggle("active", element.dataset.id === this.activeId);
    }
  }

  /** Arrow-key navigation across the flattened, currently-visible grid. */
  move(direction: "left" | "right" | "up" | "down") {
    if (this.visible.length === 0) return;

    const flat = this.zones.flatMap((z) => z.tiles);
    const index = flat.findIndex((t) => t.id === this.activeId);
    if (index === -1) {
      this.setActive(flat[0].id);
      this.scrollActiveIntoView();
      return;
    }

    let next = index;
    if (direction === "left") next = index - 1;
    else if (direction === "right") next = index + 1;
    else {
      // Vertical movement uses the real column count of the zone the active
      // tile is in — zones have different tile sizes, so a single global
      // stride would skip rows in the lower zone.
      const step = this.columnsAround(index, flat);
      next = direction === "down" ? index + step : index - step;
    }

    if (next < 0 || next >= flat.length) return;
    this.setActive(flat[next].id);
    this.scrollActiveIntoView();
  }

  private columnsAround(index: number, flat: Tile[]): number {
    const id = flat[index].id;
    const zone = this.zones.find((z) => z.tiles.some((t) => t.id === id));
    if (!zone) return 1;
    const columns = getComputedStyle(zone.element)
      .gridTemplateColumns.split(" ")
      .filter(Boolean).length;
    return Math.max(1, columns);
  }

  private scrollActiveIntoView() {
    const element = this.root.querySelector<HTMLElement>(".tile.active");
    element?.scrollIntoView({ block: "nearest" });
  }
}

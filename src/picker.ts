import { playlistAdd, playlistNames } from "./api";

/**
 * The `a` overlay: pick a playlist, or type a new name.
 *
 * Deliberately not a view or a tab — it opens over the grid, takes one
 * decision, and closes. Type to filter, Enter to commit, Esc to abandon.
 */
export class PlaylistPicker {
  private element: HTMLElement | null = null;

  get isOpen(): boolean {
    return this.element !== null;
  }

  async open(tileId: string, trackIndex: number | null, label: string, onDone: () => void) {
    this.close();
    const names = await playlistNames();

    const overlay = document.createElement("div");
    overlay.className = "overlay";

    const panel = document.createElement("div");
    panel.className = "picker";

    const heading = document.createElement("div");
    heading.className = "picker-heading";
    heading.textContent = `add ${label} to…`;

    const input = document.createElement("input");
    input.className = "picker-input";
    input.type = "text";
    input.placeholder = "playlist name";
    input.spellcheck = false;
    input.autocomplete = "off";

    const list = document.createElement("ul");
    list.className = "picker-list";

    const status = document.createElement("div");
    status.className = "picker-status";

    let filtered: string[] = names;
    let highlighted = 0;

    const paint = () => {
      const query = input.value.trim().toLowerCase();
      filtered = names.filter((n) => n.toLowerCase().includes(query));
      if (highlighted >= filtered.length) highlighted = Math.max(0, filtered.length - 1);

      list.replaceChildren();

      // An exact-match name means Enter adds to it; otherwise Enter creates.
      const exact = names.some((n) => n.toLowerCase() === query);
      if (query && !exact) {
        const item = document.createElement("li");
        item.className = "picker-item picker-new";
        item.textContent = `new playlist “${input.value.trim()}”`;
        if (filtered.length === 0) item.classList.add("on");
        item.addEventListener("click", () => void commit(input.value.trim()));
        list.append(item);
      }

      filtered.forEach((name, index) => {
        const item = document.createElement("li");
        item.className = "picker-item";
        if (index === highlighted && (filtered.length > 0)) item.classList.add("on");
        item.textContent = name;
        item.addEventListener("click", () => void commit(name));
        list.append(item);
      });
    };

    const commit = async (name: string) => {
      if (!name) return;
      try {
        const added = await playlistAdd(name, tileId, trackIndex);
        this.close();
        onDone();
        if (added === 0) status.textContent = "already there";
      } catch (error) {
        status.textContent = String(error);
      }
    };

    input.addEventListener("input", paint);
    input.addEventListener("keydown", (event) => {
      // Stop the grid's global shortcuts from firing while the picker owns input.
      event.stopPropagation();
      if (event.key === "Escape") {
        event.preventDefault();
        this.close();
        onDone();
      } else if (event.key === "ArrowDown") {
        event.preventDefault();
        highlighted = Math.min(highlighted + 1, Math.max(0, filtered.length - 1));
        paint();
      } else if (event.key === "ArrowUp") {
        event.preventDefault();
        highlighted = Math.max(highlighted - 1, 0);
        paint();
      } else if (event.key === "Enter") {
        event.preventDefault();
        const typed = input.value.trim();
        const exact = names.find((n) => n.toLowerCase() === typed.toLowerCase());
        if (exact) void commit(exact);
        else if (typed && filtered.length === 0) void commit(typed);
        else if (filtered.length > 0) void commit(filtered[highlighted]);
        else if (typed) void commit(typed);
      }
    });

    overlay.addEventListener("mousedown", (event) => {
      if (event.target === overlay) {
        this.close();
        onDone();
      }
    });

    panel.append(heading, input, list, status);
    overlay.append(panel);
    document.body.append(overlay);
    this.element = overlay;

    paint();
    input.focus();
  }

  close() {
    this.element?.remove();
    this.element = null;
  }
}

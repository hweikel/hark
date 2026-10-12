import { artUrl, savePlaylist } from "./api";
import type { Tile, Track } from "./types";

/**
 * The `e` modal for a playlist: rename it, reorder or remove songs, and give
 * it a cover of its own (or take that away, back to the mosaic).
 *
 * A separate modal from the album one because almost nothing carries over: a
 * playlist owns no tags, no folder and no year, and its songs belong to other
 * albums, so editing their titles here would quietly rewrite those albums.
 * What it does own is a name, an order and a cover.
 */
export class PlaylistModal {
  private element: HTMLElement | null = null;

  get isOpen(): boolean {
    return this.element !== null;
  }

  open(tile: Tile, generation: number, onSaved: () => void) {
    this.close();

    const overlay = document.createElement("div");
    overlay.className = "overlay";

    const panel = document.createElement("form");
    panel.className = "modal";

    const heading = document.createElement("div");
    heading.className = "modal-heading";
    heading.textContent = `playlist — ${tile.title}`;

    const name = field("name", tile.title);

    const coverRow = document.createElement("div");
    coverRow.className = "playlist-cover";
    const preview = document.createElement("img");
    preview.className = "playlist-cover-preview";
    preview.alt = "";
    preview.src = artUrl(tile.artId, 180, generation);
    const coverFields = document.createElement("div");
    coverFields.className = "playlist-cover-fields";
    const cover = field("cover image", "");
    cover.input.placeholder = "path to a jpg, png or webp — leave blank to keep";

    const mosaic = document.createElement("label");
    mosaic.className = "modal-check";
    const mosaicBox = document.createElement("input");
    mosaicBox.type = "checkbox";
    const mosaicText = document.createElement("span");
    mosaicText.textContent = "use the mosaic — drop any custom cover";
    mosaic.append(mosaicBox, mosaicText);

    // A typed path and "back to the mosaic" contradict each other.
    cover.input.addEventListener("input", () => {
      mosaicBox.disabled = cover.input.value.trim() !== "";
      if (mosaicBox.disabled) mosaicBox.checked = false;
    });

    coverFields.append(cover.row, mosaic);
    coverRow.append(preview, coverFields);

    // Rows are kept, not deleted, when removed: a struck-through row with an
    // undo is kinder than a song vanishing on a misclick.
    const rows: { track: Track; removed: boolean }[] = tile.tracks.map((track) => ({
      track,
      removed: false,
    }));

    const tracksBox = document.createElement("div");
    tracksBox.className = "modal-tracks";

    const summary = document.createElement("div");
    summary.className = "modal-rename";

    const renderTracks = () => {
      tracksBox.replaceChildren(
        ...rows.map((row, index) => {
          const line = document.createElement("div");
          line.className = "playlist-track";
          line.classList.toggle("removed", row.removed);

          const num = document.createElement("span");
          num.className = "np-num";
          num.textContent = String(index + 1);

          const label = document.createElement("span");
          label.className = "playlist-track-name";
          label.textContent = row.track.artist
            ? `${row.track.title} — ${row.track.artist}`
            : row.track.title;
          label.title = label.textContent;

          const move = (delta: number) => {
            const target = index + delta;
            if (target < 0 || target >= rows.length) return;
            [rows[index], rows[target]] = [rows[target], rows[index]];
            renderTracks();
          };

          line.append(
            num,
            label,
            iconButton("↑", "move up", () => move(-1), index === 0),
            iconButton("↓", "move down", () => move(1), index === rows.length - 1),
            iconButton(row.removed ? "undo" : "✕", row.removed ? "keep" : "remove", () => {
              row.removed = !row.removed;
              renderTracks();
            }),
          );
          return line;
        }),
      );

      const removed = rows.filter((r) => r.removed).length;
      summary.textContent = removed
        ? `removing ${removed} of ${rows.length} songs`
        : `${rows.length} songs`;
      summary.classList.toggle("moving", removed > 0);
    };
    renderTracks();

    const status = document.createElement("div");
    status.className = "modal-status";

    const actions = document.createElement("div");
    actions.className = "modal-actions";
    const save = document.createElement("button");
    save.type = "submit";
    save.textContent = "save";
    const cancel = document.createElement("button");
    cancel.type = "button";
    cancel.textContent = "cancel";
    cancel.addEventListener("click", () => this.close());
    actions.append(save, cancel);

    panel.append(heading, name.row, coverRow, summary, tracksBox, status, actions);

    panel.addEventListener("submit", async (event) => {
      event.preventDefault();
      save.disabled = true;
      status.textContent = "saving…";
      try {
        await savePlaylist(
          tile.id,
          name.input.value.trim(),
          rows.filter((r) => !r.removed).map((r) => r.track.path),
          cover.input.value.trim() || null,
          mosaicBox.checked,
        );
        this.close();
        onSaved();
      } catch (error) {
        status.textContent = String(error);
        save.disabled = false;
      }
    });

    // Esc closes — and nothing typed here may leak to the grid's handler.
    panel.addEventListener("keydown", (event) => {
      event.stopPropagation();
      if (event.key === "Escape") {
        event.preventDefault();
        this.close();
      }
    });

    overlay.addEventListener("mousedown", (event) => {
      if (event.target === overlay) this.close();
    });

    overlay.append(panel);
    document.body.append(overlay);
    this.element = overlay;

    name.input.focus();
    name.input.select();
  }

  close() {
    this.element?.remove();
    this.element = null;
  }
}

function field(label: string, value: string) {
  const row = document.createElement("label");
  row.className = "modal-field";
  const text = document.createElement("span");
  text.textContent = label;
  const input = document.createElement("input");
  input.type = "text";
  input.value = value;
  input.spellcheck = false;
  row.append(text, input);
  return { row, input };
}

function iconButton(text: string, label: string, onClick: () => void, disabled = false) {
  const button = document.createElement("button");
  // Not "submit": these live inside the form, and Enter must still mean save.
  button.type = "button";
  button.className = "playlist-track-button";
  button.textContent = text;
  button.title = label;
  button.disabled = disabled;
  button.addEventListener("click", onClick);
  return button;
}

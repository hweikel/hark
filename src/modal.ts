import { plannedRename, saveAlbum } from "./api";
import type { Tile } from "./types";

/**
 * The `e` modal: edit an album's metadata, art, and perfect flag.
 *
 * Saving writes real tags into the files, so the thing you fix here is fixed
 * for Doppler and the phones too. Because the folder tree is what hark! groups
 * by, changing the artist or album also moves the folder — which is why the
 * modal always shows the literal rename before you commit to it.
 */
export class EditModal {
  private element: HTMLElement | null = null;

  get isOpen(): boolean {
    return this.element !== null;
  }

  open(tile: Tile, onSaved: () => void) {
    this.close();

    const overlay = document.createElement("div");
    overlay.className = "overlay";

    const panel = document.createElement("form");
    panel.className = "modal";

    const heading = document.createElement("div");
    heading.className = "modal-heading";
    heading.textContent = tile.relPath ?? tile.title;

    const artist = field("artist", tile.artist);
    const title = field("album", tile.title);
    const year = field("year", tile.year == null ? "" : String(tile.year));
    year.input.inputMode = "numeric";

    const cover = field("cover image", "");
    cover.input.placeholder = "path to a jpg or png — leave blank to keep";

    const perfect = document.createElement("label");
    perfect.className = "modal-check";
    const perfectBox = document.createElement("input");
    perfectBox.type = "checkbox";
    perfectBox.checked = tile.perfect;
    const perfectText = document.createElement("span");
    perfectText.textContent = "perfect album — show in the marquee";
    perfect.append(perfectBox, perfectText);

    // The rename preview. Updated live, because an automatic folder move you
    // didn't expect is the one genuinely scary thing this modal can do.
    const renamePreview = document.createElement("div");
    renamePreview.className = "modal-rename";

    const refreshPreview = async () => {
      try {
        const planned = await plannedRename(tile.id, artist.input.value, title.input.value);
        renamePreview.textContent = planned ? `folder: ${planned}` : "folder: unchanged";
        renamePreview.classList.toggle("moving", planned !== null);
      } catch {
        renamePreview.textContent = "";
      }
    };
    artist.input.addEventListener("input", () => void refreshPreview());
    title.input.addEventListener("input", () => void refreshPreview());

    const tracksBox = document.createElement("div");
    tracksBox.className = "modal-tracks";
    const trackRows = tile.tracks.map((track, index) => {
      const row = document.createElement("div");
      row.className = "modal-track";

      const num = document.createElement("input");
      num.type = "text";
      num.className = "modal-track-no";
      num.value = track.trackNo == null ? String(index + 1) : String(track.trackNo);
      num.inputMode = "numeric";

      const name = document.createElement("input");
      name.type = "text";
      name.className = "modal-track-title";
      name.value = track.title;

      row.append(num, name);
      tracksBox.append(row);
      return { path: track.path, num, name };
    });

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

    panel.append(
      heading,
      artist.row,
      title.row,
      year.row,
      cover.row,
      perfect,
      renamePreview,
      tracksBox,
      status,
      actions,
    );

    panel.addEventListener("submit", async (event) => {
      event.preventDefault();
      save.disabled = true;
      status.textContent = "saving…";
      try {
        const report = await saveAlbum(
          tile.id,
          {
            artist: artist.input.value.trim(),
            title: title.input.value.trim(),
            year: parseYear(year.input.value),
            tracks: trackRows.map((row) => ({
              path: row.path,
              title: row.name.value.trim(),
              trackNo: parseTrackNo(row.num.value),
            })),
          },
          cover.input.value.trim() || null,
          perfectBox.checked,
        );
        this.close();
        onSaved();
        console.info(
          `hark: wrote ${report.tracksWritten} tracks` +
            (report.coversEmbedded ? `, embedded art in ${report.coversEmbedded}` : "") +
            (report.coversFailed ? `, ${report.coversFailed} refused the art` : "") +
            (report.renamed ? `, renamed ${report.renamed}` : ""),
        );
      } catch (error) {
        // A refused rename (collision) lands here with the metadata already
        // written, which the message says plainly.
        status.textContent = String(error);
        save.disabled = false;
      }
    });

    // Esc closes, as specified — and must not leak to the grid's handler.
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

    void refreshPreview();
    artist.input.focus();
    artist.input.select();
  }

  close() {
    this.element?.remove();
    this.element = null;
  }
}

function field(label: string, value: string) {
  const row = document.createElement("label");
  row.className = "modal-field";
  const name = document.createElement("span");
  name.textContent = label;
  const input = document.createElement("input");
  input.type = "text";
  input.value = value;
  input.spellcheck = false;
  row.append(name, input);
  return { row, input };
}

function parseYear(text: string): number | null {
  const n = Number.parseInt(text.trim(), 10);
  return Number.isFinite(n) && n > 0 ? n : null;
}

function parseTrackNo(text: string): number | null {
  const n = Number.parseInt(text.trim(), 10);
  return Number.isFinite(n) && n > 0 ? n : null;
}

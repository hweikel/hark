# Working on hark!

A music player that respects perfect albums. See `README.md` for what it is and
why it makes the choices it does; this file is the operational stuff an agent
needs that isn't obvious from the code.

## Commands

```sh
nix develop                                   # rust, cargo-tauri, webkitgtk — all pinned
cargo tauri dev                               # run the app, from the repo root
cd src-tauri && cargo test                    # 103 tests
cd src-tauri && cargo tauri build --no-bundle # release binary -> src-tauri/target/release/hark
npm run build                                 # tsc --noEmit && vite build
./packaging/install-local.sh                  # put hark on PATH + in the launcher
```

The release binary embeds the frontend, so a frontend change needs
`cargo tauri build`, not just `npm run build`, before the installed `hark`
reflects it.

---

## Setting up a second machine — read this before syncing anything

**Do not make `hark sync --init` the first thing you run on a machine that
already holds its own copy of the collection.** It will quietly overwrite the
good data on B2 with that machine's stale copy.

`hark sync --init` runs `rclone bisync --resync`, and rclone's help says:

> `--resync` — Equivalent to `--resync-mode path1`

hark passes the local collection as path1 and the remote as path2, so **local
wins every conflict**. For a machine that is simultaneously ahead (albums only
it has) and behind (everything edited elsewhere since), the two halves resolve
differently:

| | result of a naive `--init` |
|---|---|
| file only on that machine | uploaded — correct |
| file only on B2 | downloaded — correct |
| file on both, contents differ | **that machine's older copy is pushed to B2** |

That last row is every track whose tags or embedded art were edited elsewhere.
Verified by simulation, not inferred: with B2 holding a freshly tagged track
and the second machine holding the stale one, a naive `--init` left the stale
bytes on B2.

There is a second, quieter failure in the same situation. A machine that has
not synced recently still has the **old folder names** (decomposed `Björk`,
`Mount Kimbie/…`, `In Focus?`) *and* a stale `.hark/renames.jsonl`, so the
rename replay has nothing to replay. bisync then reads those old-named folders
as content only that machine has, and uploads them — recreating the duplicate
folders the NFC pass existed to remove.

### The order that works

Tested end to end. Steps 0–2 are plain rclone and need no hark build, which
matters because the dev shell does not currently work on macOS (see below).

```sh
# 0. hark's filter rules (one tiny file)
rclone copy b2-music:innercasmalia/music/.hark ~/music/.hark

# 1. push ONLY what B2 lacks. --ignore-existing is the safety: it cannot
#    overwrite a file that is already up there, so nothing edited elsewhere
#    is at risk. This is what rescues the albums only this machine has.
rclone copy ~/music b2-music:innercasmalia/music \
  --filter-from ~/music/.hark/sync-filters.txt --ignore-existing --progress --dry-run

# 2. now make this machine an exact copy of B2 — which by now includes the
#    albums step 1 just pushed.
rclone sync b2-music:innercasmalia/music ~/music \
  --filter-from ~/music/.hark/sync-filters.txt --progress --dry-run

# 3. both sides already agree, so the baseline is trivially correct
hark sync --init
```

Run 1 and 2 with `--dry-run` first and read the output. **Step 1 must precede
step 2**: step 2 deletes anything local that is not on B2, so this machine's
new albums have to be up there already.

Afterwards the watermark is set and ordinary `hark sync` on either machine is
safe — bisync's steady state is what it is good at. The hazard is specific to
onboarding a machine with a divergent copy.

Two notes:

- Step 2 re-downloads the renamed albums, because `rclone sync` cannot know a
  folder moved — it deletes and re-fetches (~2GB). Replaying the journalled
  renames locally first would make those free `mv`s, but there is no
  `hark sync --renames-only` mode yet.
- The first `hark sync` afterwards prints a "skipped" line per journal entry.
  Expected: step 2 already moved those folders.

### macOS is not yet a supported dev platform

`flake.nix` hardcodes `system = "x86_64-linux"` and its `buildInputs` are
Linux-only (`webkitgtk_4_1`, `gtk3`, `alsa-lib`). `nix develop` will not
evaluate on a Mac. Building hark there needs a darwin branch in the flake with
a different webview/audio stack — Tauri on macOS uses the system WebKit and
there is no gtk. Until then, a Mac can still be brought into a correct synced
state with steps 0–2 above, which are pure rclone.

---

## Invariants worth not breaking

**NFC is identity; on-disk bytes are for the filesystem.** `rel_path` and the
keys in `library.json` are NFC. `Album::dir` is the real directory as the
filesystem spells it. Reconstructing a path with `root.join(rel_path)` breaks
on every folder stored decomposed — this was a real bug. Only `dir` is safe to
pass to `fs::rename` or to join `cover.jpg` onto.

**Renames are journalled, then replayed — and bisync must be told.** Replaying
a rename makes both sides agree on paths, but rclone bisync compares against a
*cached listing* that still names the old path, so it sees the folder as new on
both sides. If the album was also re-tagged in the same session the contents
differ too, and bisync calls that a conflict and parks the real filename as
`.conflict1`/`.conflict2` on both ends. That destroyed 43 tracks once.
`crate::bisync` repoints those listings after each replay, and only for a side
that genuinely moved — claiming a move that did not happen would make bisync
read the new path as deleted and propagate that deletion.

**`--max-delete` is a percentage in bisync**, not a count, unlike every other
rclone subcommand. It is set to `1` (~21 files here). A trip is a halt, not
data loss; rerun with `--force` after looking at what it wanted to remove.

**lofty validates on write what it happily read.** Two separate classes of file
in this collection cannot be written naively, and both failed *silently* or
with a misleading message before being handled in `library/edit.rs`:

- Writing re-detects the file type from *content* (reading only uses the
  extension), and the search for the first MPEG frame gives up after
  `max_junk_bytes`, default 1024. Files with a second stacked ID3 tag or
  padding push that as far as 22706 bytes out. Hence `MAX_JUNK_BYTES = 64KiB`.
- A `COMM` frame whose language is `\x00\x00\x00` rather than three ASCII
  characters makes lofty refuse the whole file. `save_tag` retries once with
  comments dropped.

Both paths report failures now; `write_cover` returns a `CoverReport` with a
`failed` list rather than a bare count, because "one track refused the art"
used to look exactly like "this album has fewer tracks".

**Audio: the hardware here is 48kHz-only.** hark opens the device at the file's
native rate and that part works, but every sink on this ThinkPad advertises a
single rate, so something always resamples. Measured transparent (flat to
15kHz, artifacts at -97dB). Do not claim native-rate playback is achieved on
this machine. See the Audio section of `README.md`.

# Working on hark!

A music player that respects perfect albums. See `README.md` for what it is and
why it makes the choices it does; this file is the operational stuff an agent
needs that isn't obvious from the code.

## Commands

```sh
nix develop                                   # per-platform shell; Linux pulls webkitgtk
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

## Setting up the mac mini — the whole runbook

Two independent tracks. **Track A (data) is the one with a trap in it**; track B
(build) is just unfinished work. They don't depend on each other, so an agent
can start on B while A's transfers run.

### One repo, both machines — yes

`main` serves both. Everything platform-specific is confined to `flake.nix`,
which exposes a dev shell for `x86_64-linux`, `aarch64-darwin` and
`x86_64-darwin`. The Rust and TypeScript are already portable — Tauri, cpal and
symphonia all abstract the platform, and nothing is hand-gated by `#[cfg]`
today. If that changes, use `#[cfg(target_os = "…")]` rather than branching the
repo. `Cargo.lock` is shared and already resolves both platforms' dependency
sets.

Two things to expect, neither a problem:

- `src-tauri/gen/schemas/` is generated per platform. The Linux machine
  committed `linux-schema.json`; macOS will produce `macos-schema.json`.
  Committing both is fine — just don't let one machine delete the other's.
- The collection itself and its `.hark/` state are **not** in the repo. They
  travel via B2. The sync watermark is deliberately per-machine, in
  `~/.local/state/hark/`, so each machine tracks what it has replayed.

---

## Track A: the data — read this before syncing anything

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

Tested end to end on a two-machine simulation against a scratch bucket.

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

Steps 0–2 are pure rclone and need no hark build, so track A can be finished
before track B is started. Only step 3 needs the binary.

---

## Track B: making it build on macOS

`flake.nix` now exposes `aarch64-darwin` and `x86_64-darwin` dev shells
alongside Linux. **They evaluate, but they have never been built or run** —
they were written on the Linux machine, which cannot test them. Treat the
darwin branch as a starting point to iterate on, not as known-good.

What was done for you: the gtk/webkit/alsa stack is now `lib.optionals isLinux`,
because macOS gets its webview from the system WKWebView and its audio from
CoreAudio, neither of which nixpkgs vendors. The Linux-only env exports
(`WEBKIT_DISABLE_DMABUF_RENDERER`, `GDK_BACKEND`, the gsettings paths) are
likewise Linux-only now. The Linux shell was re-verified unchanged after the
refactor: same env, 103 tests, release build.

Prerequisites: Nix with flakes enabled, and Xcode command line tools
(`xcode-select --install`) — the shell warns if they're missing.

Climb this ladder; each rung needs less of the platform than the next:

```sh
cd src-tauri && cargo test                     # 1. no GUI, no audio device
cargo run --bin scan_report -- ~/music         # 2. library layer against real files
cargo run --bin play_probe -- <album-dir> 4    # 3. CoreAudio output
cd .. && cargo tauri dev                       # 4. the actual window
cargo tauri build --no-bundle                  # 5. release binary
```

Friction to expect, in rough order of likelihood:

- **`cargo-tauri` from the pinned nixpkgs revision** may not have a darwin
  build on that exact rev. If it doesn't, the npm `@tauri-apps/cli` is the
  fallback — it is deliberately *not* used on Linux, but using it on macOS only
  is an acceptable split.
- **Apple SDK framework linking** (WebKit, CoreAudio, AppKit). These come from
  the Xcode tools, not nixpkgs. If the linker can't find them, the usual
  escapes are the `apple-sdk` package or dropping to a rustup toolchain outside
  the nix shell for the build step.
- **Codesigning.** Stay on `--no-bundle` at first; it produces a plain binary
  and sidesteps signing entirely. A `.app` bundle needs a signing identity.
- **`cargo tauri build` is slow the first time** (~4 min on the ThinkPad) and
  webkitgtk is *not* being built on macOS, so it should be faster, not slower.

Do not assume the audio findings transfer. The 48kHz-only conclusion below is a
fact about *this ThinkPad's* `sof-hda-dsp` sink. The mac mini's hardware may
genuinely honour 44.1kHz and 96kHz, in which case `play_probe` plus `pw-top`'s
macOS equivalent (Audio MIDI Setup, or `system_profiler SPAudioDataType`) is
worth a look — hark already requests the file's native rate.

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

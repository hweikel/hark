# Working on hark!

A music player that respects perfect albums. See `README.md` for what it is and
why it makes the choices it does; this file is the operational stuff an agent
needs that isn't obvious from the code.

## Commands

Linux (Nix):

```sh
nix develop                                   # rust, cargo-tauri, webkitgtk — all pinned
cargo tauri dev                               # run the app, from the repo root
cd src-tauri && cargo test                    # 103 tests
cd src-tauri && cargo tauri build --no-bundle # release binary -> src-tauri/target/release/hark
npm run build                                 # tsc --noEmit && vite build
./packaging/install-local.sh                  # put hark on PATH + in the launcher
```

macOS (rustup + npm, no Nix — see Track B):

```sh
npm install && npm run tauri dev
cd src-tauri && cargo test
npm run tauri build                           # hark.app, then install it — see "Installing on macOS"
```

The release binary embeds the frontend, so a frontend change needs
`cargo tauri build`, not just `npm run build`, before the installed `hark`
reflects it.

---

## Setting up the mac mini — the whole runbook

Two independent tracks. **Track A (data) is the one with a trap in it**; track B
(build) is just unfinished work. They don't depend on each other, so an agent
can start on B while A's transfers run.

**Status, 2026-10-12: both tracks are done on the mac mini.** What it took,
beyond what is written below:

- **The collection lives at `~/music-hark`, not `~/music`.** APFS is
  case-insensitive, so `~/music` *is* `~/Music` — the Music app's library. The
  runbook as written would have uploaded that library to B2 and then
  `rclone sync` would have tried to mirror B2 over it. hark now defaults to
  `~/music-hark` on macOS (`library::resolve_music_dir`). The mac's divergent
  copy had been sitting in `~/music_backup` (Doppler's library folder) and was
  renamed into place.
- **There is no `b2-music` remote in the mac's rclone.conf** — just
  `b2-innercasmalia`, which there already has `hard_delete = false`. A
  `b2-music` alias pointing at it was added so the default remote resolves.
- **The journalled renames were replayed locally before step 1** (see the
  note under step 1 — without that, step 1 itself re-uploads the old names).
- Only 16 files were genuinely mac-only (BIALYSTOCKS, two Le Petit Chou
  albums, one Unknown Artist track) and went up in step 1. Step 2 then pulled
  864 re-tagged files plus 36 new ones (9.3 GiB); none of the replaced local
  copies was newer than B2's. `rclone check` afterwards: 0 differences.

### One repo, both machines — yes

`main` serves both, and the split is only in how each machine *gets a
toolchain* — Linux through `flake.nix`, macOS through rustup and npm. The
source is shared whole: Tauri, cpal and symphonia abstract the platform, and
nothing is hand-gated by `#[cfg]` today. If that changes, use
`#[cfg(target_os = "…")]` rather than branching the repo. `Cargo.lock`,
`rust-toolchain.toml` and `package.json` are common to both, and `Cargo.lock`
already resolves both platforms' dependency sets.

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

Three notes:

- **Step 1 has the same old-names trap as a naive `--init`.** `--ignore-existing`
  protects files that are on B2 *at the same path* — but a folder the journal
  renamed (`Mount Kimbie/The Sunset Violent` → `King Krule/…`,
  `In Focus?` → `In Focus`) is not at the same path, so step 1 would upload it
  under its old name and recreate the duplicate. On the mac mini that was 35 of
  the 51 files step 1 wanted to push. Replay the journal locally first (plain
  `mv` for real renames; for NFC-only ones on APFS, rename through a temp name,
  since a direct rename between two normalizations of one name is a no-op), and
  read step 1's dry-run against `.hark/renames.jsonl`.
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

**Use rustup and the npm Tauri CLI. Do not install Nix for this.**

Nix earns its place on the Linux machine for one reason: pinning `webkitgtk`,
which is a multi-hour source build if the pin ever slips. That reason does not
exist on macOS, where Tauri uses the system WKWebView and cpal uses CoreAudio —
nothing heavy to pin, and nothing nixpkgs can vendor anyway. Installing Nix on
macOS is its own small project (APFS volume, daemon), and at the end of it you
would be the first person ever to run the darwin shell in this flake. That is
two unknowns stacked underneath a first-time build of an unfamiliar app.

rustup plus `@tauri-apps/cli` is Tauri's own documented macOS path, so when
something breaks the error text matches what everyone else has written about.

### Setup

```sh
xcode-select --install          # WKWebView, CoreAudio, the linker
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
# node 24 via brew/fnm/asdf, whichever you prefer

cd hark && npm install          # installs @tauri-apps/cli + the darwin binary
npm run tauri dev               # the app
```

Two things in the repo exist so you don't have to decide:

- **`rust-toolchain.toml`** pins 1.95.0, the same compiler the Linux flake
  provides. rustup reads it and installs that version automatically. The
  nixpkgs cargo on Linux ignores the file, so the two machines agree without
  either one bending.
- **`@tauri-apps/cli` is already in `devDependencies`** with `npm run tauri`
  wired up, and `package-lock.json` already carries `cli-darwin-arm64`, so
  `npm install` is all it takes. Linux is unaffected — it still uses
  `cargo tauri` from nixpkgs. The two CLIs are 2.11.0 and 2.12.2 respectively;
  both drive the `tauri = "2"` crate and the skew does not matter.

### Verification ladder

Each rung needs more of the platform than the one above, so failures localise:

```sh
cd src-tauri && cargo test                     # 1. no GUI, no audio device
cargo run --bin scan_report -- ~/music         # 2. library layer, real files
cargo run --bin play_probe -- <album-dir> 4    # 3. CoreAudio output
cd .. && npm run tauri dev                     # 4. the actual window
npm run tauri build -- --no-bundle             # 5. release binary
```

Get rung 1 green before touching anything else — it needs no window and no
audio device, so it proves the library, tag and sync layers are fine and
isolates whatever follows to the platform shell.

Friction to expect:

- **Codesigning.** Stay on `--no-bundle` at first: a plain binary, no signing
  identity needed. A `.app` bundle is a separate errand.
- **Gatekeeper** may quarantine the built binary on first run.
- **`src-tauri/gen/schemas/macOS-schema.json`** will appear. Commit it.

What actually happened on the mac mini (macOS 15.6, M-series, rustup 1.95.0,
npm 11): every rung passed. Rung 1 initially failed 4 of 103, all real
platform differences, now fixed:

- APFS is normalization-insensitive, so moving `Bjo◌̈rk/Debut` to `Björk/Debut`
  reused the decomposed artist folder instead of creating an NFC one.
  `rename::respell_sole_parent` respells it via a temp name.
- macOS's temp dir is behind a symlink (`/var` → `/private/var`); playlist
  entries are canonicalized but the root was not, so `relative_to` wrote
  absolute paths. It now canonicalizes both.
- One test's precondition ("the NFC path must not exist") cannot hold on APFS
  and is skipped there.

**Do not commit `package-lock.json` churn from the mac.** npm 11 there strips
the `libc` fields from the Linux optional deps; the Linux side needs them.
`git checkout package-lock.json` after `npm install`.

### Installing on macOS

hark runs as a normal app from `/Applications/hark.app` — Spotlight,
Launchpad, Dock. Updating it after a code change is a rebuild plus a copy:

```sh
cd ~/hark && git checkout package-lock.json   # if npm install touched it
npm run tauri build
rm -rf /Applications/hark.app
ditto src-tauri/target/release/bundle/macos/hark.app /Applications/hark.app
```

Quit the running hark first, or the old binary keeps running until you do.
The release binary embeds the frontend, so this is needed for UI changes too;
`npm run tauri dev` is the loop for iterating, this is for installing.

What makes the bundle, all macOS-only so Linux is untouched:

- **`src-tauri/tauri.macos.conf.json`** — Tauri merges it over
  `tauri.conf.json` only when building on macOS (a JSON merge patch, so its
  `targets` and `icon` arrays *replace* the base ones). It swaps the Linux
  `deb` target for `app`, points at the mac icon, and sets
  `signingIdentity: "-"`.
- **Ad-hoc signing (`"-"`).** Without it only the binary is linker-signed and
  the bundle is not sealed (`codesign --verify --deep --strict` fails). Ad-hoc
  is enough for an app built and run on the same machine — a locally built
  bundle carries no quarantine flag, so Gatekeeper does not check it. Shipping
  it to another Mac would need a Developer ID and notarization.
- **`src-tauri/icons/macos/`** — the Dock icon: a plain red circle (#FF3B30,
  824px diameter on a 1024 canvas, Apple's icon-grid margin), as `icon.icns`
  plus PNGs. Generated with CoreGraphics + `sips` + `iconutil`; to change it,
  regenerate `icon.png` at 1024×1024 and rebuild the rest:

  ```sh
  cd src-tauri/icons/macos && mkdir hark.iconset
  for z in 16 32 128 256 512; do
    sips -z $z $z icon.png --out hark.iconset/icon_${z}x${z}.png
    sips -z $((z*2)) $((z*2)) icon.png --out hark.iconset/icon_${z}x${z}@2x.png
  done
  iconutil -c icns hark.iconset -o icon.icns && rm -r hark.iconset
  sips -z 32 32 icon.png --out 32x32.png && sips -z 128 128 icon.png --out 128x128.png
  ```

If the Dock shows a stale or blank icon after an update, it is the icon cache:
`killall Dock`. To confirm what is actually installed:
`codesign --verify --deep --strict /Applications/hark.app`.

### If you would rather use Nix anyway

`flake.nix` does expose `aarch64-darwin` and `x86_64-darwin` shells — the
gtk/webkit/alsa stack and the Linux-only env exports sit behind
`lib.optionals isLinux`. All three systems evaluate. But **the darwin shells
have never been built or run**, since they were written on the Linux machine.
The likely snags are `cargo-tauri` not having a darwin build on that exact
pinned nixpkgs revision, and Apple SDK framework linking. It is a fine thing to
fix later; it is a bad thing to fight on day one.

The audio findings did not transfer, in the good direction. The 48kHz-only
conclusion below is a fact about *the ThinkPad's* `sof-hda-dsp` sink. On the
mac mini, `play_probe` on a 44.1kHz FLAC switched the built-in speakers from
48000 to 44100 Hz for the duration (`system_profiler SPAudioDataType`, read
mid-playback) — so there, native-rate playback is real.

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

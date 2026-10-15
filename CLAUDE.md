# Working on hark!

A music player that respects perfect albums. `README.md` says what it is and why
it makes the choices it does; `RELEASING.md` covers cutting a release and
installing one. This file is the operational stuff an agent needs that isn't
obvious from the code.

**The NixOS ThinkPad is the only machine that builds hark!** The mac mini
installs releases and never clones the repo, so a change is not "shipped" until
it is tagged — see `RELEASING.md`.

## Commands

```sh
nix develop                                   # rust 1.95, cargo-tauri, webkitgtk, picom — all pinned
cargo tauri dev                               # run the app, from the repo root
picom -b --config packaging/picom.conf        # only needed to retry transparency on Linux — see below
cd src-tauri && cargo test                    # 112 tests, no GUI and no audio device needed
cd src-tauri && cargo tauri build --no-bundle # release binary -> src-tauri/target/release/hark
npm run build                                 # tsc --noEmit && vite build
nix build .#hark && ./result/bin/hark         # the packaged build, as installed
```

Verification binaries, from `src-tauri/`:

```sh
cargo run --bin scan_report -- ~/music        # what the scanner sees
cargo run --bin play_probe -- <album-dir> 4   # decode + play, no UI
```

Releasing and installing (details in `RELEASING.md`):

```sh
git tag -a v0.1.1 -m v0.1.1 && git push --follow-tags   # builds both platforms, publishes a Release
nix profile install github:hweikel/hark/v0.1.1#hark     # the laptop's launcher entry
```

The release binary embeds the frontend, so a frontend-only change still needs a
`cargo tauri build` (or `nix build`) before an installed `hark` reflects it.

## Architecture

Rust/Tauri 2 backend, TypeScript frontend with no framework, one window.

- **`cli.rs`** parses the invocation. `hark sync`, `hark normalize` and `--help`
  run and exit; anything else is `run_gui` in `lib.rs`, which builds the Tauri
  app. The GUI and the CLI are the same binary.
- **`app.rs`** holds `AppState` (library, `Store`, `ArtCache`, `Player`) behind
  an `RwLock` and every `#[tauri::command]`. It also serves the `hark://`
  protocol: covers are decoded and downscaled in Rust and handed to the webview
  at tile size, because full-size bitmaps in the renderer cost gigabytes.
- **`library/`** — `scan.rs` walks the collection (folders are the grouping
  truth, tags are the detail), `tags.rs` reads via lofty, `edit.rs` writes,
  `art.rs` extracts and caches covers, `store.rs` is `.hark/library.json`,
  `playlist.rs` is M3U, `rename.rs` moves folders and `journal.rs` records the
  moves, `norm.rs` is all the NFC handling, `view.rs` is the payload the
  frontend gets — the whole library in one ~300KB JSON at startup, which is
  what makes search instant and client-side.
- **`audio/`** — one thread owning the decoder and the device. `decode.rs` is
  symphonia, `output.rs` opens cpal at the file's own sample rate, `player.rs`
  owns play/pause/next/previous/shuffle and pushes `PlaybackState` out as
  `playback` events. Track changes originate here, not in the UI: a song ending
  is a backend event.
- **`sync.rs` / `bisync.rs`** shell out to rclone to reconcile the collection
  with B2. `sync.rs` is the command surface (`--check`, `--init`, `--force`);
  `bisync.rs` is the listing surgery that makes journalled renames safe.
- **`src/`** — `main.ts` wires keys and events, `grid.ts` draws tiles,
  `nowplaying.ts`, `search.ts` (prefix matching, deliberately dumb),
  `modal.ts` / `playlistmodal.ts` / `picker.ts` the three overlays,
  `api.ts` the command wrappers, `style.css` the whole design.

## Invariants worth not breaking

**NFC is identity; on-disk bytes are for the filesystem.** `rel_path` and the
keys in `library.json` are NFC. `Album::dir` is the real directory as the
filesystem spells it. Reconstructing a path with `root.join(rel_path)` breaks on
every folder stored decomposed — this was a real bug. Only `dir` is safe to pass
to `fs::rename` or to join `cover.jpg` onto.

**`library.json` is the one non-standard file, and it stays hand-editable.**
`{ "version": 1, "perfect": [...] }`, entries relative to the music root, NFC,
slash-separated, kept sorted so diffs stay legible. Everything else hark! knows
goes somewhere another program already reads: tags in the files, M3U playlists,
`cover.jpg`.

**M3U paths are relative to the playlist file**, which is what other players
expect and what lets a playlist survive the trip through B2. Entries that don't
resolve are *preserved verbatim*, never dropped — an album that is merely
missing on this machine must not be deleted from a playlist by the act of
adding one song to it.

**lofty validates on write what it happily read.** Two classes of file here
cannot be written naively, and both used to fail silently or with a misleading
message. Writing re-detects the type from *content* (reading only uses the
extension) and gives up looking for the first MPEG frame after
`max_junk_bytes`, default 1024 — files with a second stacked ID3 tag push that
22706 bytes out, hence `MAX_JUNK_BYTES = 64KiB` in `library/edit.rs`. And a
`COMM` frame whose language is `\x00\x00\x00` makes lofty refuse the whole file,
so `save_tag` retries once with comments dropped. Both paths report failures:
`write_cover` returns a `CoverReport` with a `failed` list, because "one track
refused the art" used to look exactly like "this album has fewer tracks".

**Renames are journalled, then replayed — and bisync must be told.** Replaying a
rename makes both sides agree on paths, but rclone bisync compares against a
*cached listing* that still names the old path, so it sees the folder as new on
both sides. If the album was also re-tagged in the same session, bisync calls
that a conflict and parks the real filename as `.conflict1`/`.conflict2` on both
ends. That destroyed 43 tracks once. `crate::bisync` repoints those listings
after each replay, and only for a side that genuinely moved — claiming a move
that did not happen would make bisync read the new path as deleted and propagate
that deletion.

**`--max-delete` is a percentage in bisync**, not a count, unlike every other
rclone subcommand. It is set to `1` (~21 files here). A trip is a halt, not data
loss; rerun with `--force` after looking at what it wanted to remove.

**`sync --init` lets the local side win every conflict.** It runs
`rclone bisync --resync`, which is `--resync-mode path1`, and hark passes local
as path1 — so an out-of-date machine would push stale copies over B2. `--init`
therefore runs `rclone check` first and refuses when the two sides disagree,
naming the difference; `--force` is how you say you have looked and this side is
right. The guard exists because the documentation wasn't enough: anyone reading
a runbook top to bottom can run the second command without acting on the first.

**A production build needs the `custom-protocol` feature.** `tauri`'s build
script does `dev = !custom_protocol`, so a binary built without it ignores its
own embedded frontend and tries to load `build.devUrl` — the window comes up
saying *Could not connect to localhost: Connection refused*. `cargo tauri build`
and `tauri-action` pass the feature for you; a bare `cargo build --release`,
which is what the flake's `buildRustPackage` runs, does not — hence
`buildFeatures = [ "tauri/custom-protocol" ]` there (the dependency-qualified
name, since this crate declares no passthrough feature of its own). This costs an hour the first
time it bites, because the failure looks exactly like a rendering bug.

**Audio: the hardware here is 48kHz-only.** hark opens the device at the file's
native rate and that part works, but every sink on this ThinkPad advertises a
single rate, so something always resamples. Measured transparent (flat to
15kHz, artifacts at -97dB). Do not claim native-rate playback is achieved on
this machine — on the mac mini it genuinely is. See the Audio section of
`README.md`.

## Linux vs macOS

The source is shared whole; Tauri, cpal and symphonia abstract the platform.
Where a difference is unavoidable it is `#[cfg(target_os = "…")]`, never a
branch in the repo.

- **The collection lives at `~/music` on Linux, `~/music-hark` on macOS**
  (`library::resolve_music_dir`). APFS is case-insensitive, so `~/music` *is*
  `~/Music` — the Music app's library, which a sync would have uploaded and then
  mirrored over.
- **APFS is normalization-insensitive**, so moving `Bjo◌̈rk/Debut` to
  `Björk/Debut` reuses the decomposed artist folder instead of creating an NFC
  one. `rename::respell_sole_parent` renames through a temp name to force it. A
  direct rename between two normalizations of one name is a no-op there.
- **macOS's temp dir is behind a symlink** (`/var` → `/private/var`), so
  `playlist::relative_to` canonicalizes both the root and the entry; one test's
  precondition ("the NFC path must not exist") cannot hold on APFS and is
  skipped there.
- **Self-update is macOS-only** (`src-tauri/src/update.rs`,
  `tauri-plugin-updater` as a `cfg(target_os = "macos")` dependency). The Linux
  build is installed into the read-only Nix store and must not try to overwrite
  itself. A consequence: `cargo test` here never compiles `update.rs` — the
  macOS CI job is the first compiler to see it.
- **Bundling** is configured in `src-tauri/tauri.macos.conf.json`, which Tauri
  merges over `tauri.conf.json` only on macOS (a JSON merge patch, so its
  `targets` and `icon` arrays *replace* the base ones). It swaps the Linux `deb`
  target for `app`, turns on `createUpdaterArtifacts` (Tauri's Linux updater
  only knows AppImage, so this cannot live in the base config), points at
  `icons/macos/` and sets `signingIdentity: "-"` — ad-hoc signing, which is
  enough for a locally installed app and is not Apple notarization.
- **The translucent window is both platforms now, and the blocker was never
  the compositor.** The base config sets the window `transparent` and
  `app.macOSPrivateApi: true` — which in turn requires the `macos-private-api`
  feature on the `tauri` crate, or tauri-build refuses to compile with a
  feature-allowlist error. Linux was opaque for a release because of one
  environment variable, not because of WebKitGTK, i3 or picom:
  **`WEBKIT_DISABLE_DMABUF_RENDERER=1` forces the unaccelerated renderer, and a
  transparent web view on that path never paints its background.** WebKitGTK
  reads the variable's *value*, so the fix is `0`, not unsetting it — it is set
  in two places (the `preFixup` wrapper and the dev shell's `shellHook`) and
  both must agree, or `cargo tauri dev` disagrees with the installed build.
  That was the whole bug: the window surface accepted alpha, the page declined
  to use it, and the measurement — the real UI reading flat `--ink` over a
  bright wallpaper — was correct but blamed the wrong layer. Verified on
  WebKitGTK 2.54.1 / i3 4.24 / picom, by screenshotting the window over a
  wallpaper and seeing it through the page.

  **A compositor is still required, just not sufficient.** i3 does not
  composite; without `picom -b --config packaging/picom.conf` running, the 50%
  background renders black rather than showing the desktop. Sample pixels with
  `import -screen -window root` — plain `import -window root` grabs the
  *uncomposited* root and will tell you the desktop is black.
- **The toolchain differs, the compiler does not.** `rust-toolchain.toml` pins
  1.95.0 for rustup (which is what CI uses); the nixpkgs cargo ignores the file
  and the flake pins the same version. `src-tauri/gen/schemas/` is generated per
  platform — `linux-schema.json` and `macOS-schema.json` are both committed;
  don't let a build delete the other one.

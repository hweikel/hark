# hark!

A music player that respects perfect albums.

Some records are meant to be played whole; others are folders that exist
because you wanted one song off them. Every player flattens that distinction.
hark! doesn't: albums you mark **perfect** get the marquee, everything else is
still there but visibly demoted, and playlists are real objects — references,
not copies — so a song can sit on a playlist *and* on the album it came from.

One window, one view. Night-sky ink, thin gold hairlines, no chrome competing
with the cover art.

## Running it

```sh
nix develop            # rust, cargo-tauri, webkitgtk — all pinned
cargo tauri dev        # from the repo root
```

Setting up a **second machine** that already holds its own copy of the
collection? Read `CLAUDE.md` first — `hark sync --init` resolves conflicts in
favour of the local side, so running it there first overwrites whatever was
edited elsewhere. There is a tested order that doesn't.

Other useful commands, from `src-tauri/`:

```sh
cargo test                                  # unit tests
cargo run --bin scan_report -- ~/music      # what the scanner sees
cargo run --bin play_probe -- <album-dir>   # decode + play, no UI
```

## Using it

```
hark [PATH]                  open a collection ($HARK_MUSIC_DIR, else ~/music; ~/music-hark on macOS)
hark sync [--check|--init]   reconcile with the remote
hark normalize [--check]     rewrite folder names into Unicode NFC
hark --help
```

**Keys** — `space` play/pause · `e` edit the active album · `a` add that album
or playlist to a playlist · `A` add the song currently playing · arrows move,
`Enter` plays · `/` focuses search, `Esc` clears it. Double-click a tile to
play it. Quit with your window manager (`$mod+Shift+q`).

Search is deliberately dumb: prefix matching over artist, album, and song
titles, ignoring a leading "The". Misspell something and you find nothing.

## What it writes to disk

Everything hark! knows how to say goes somewhere another program already
understands, so a fix made here is a fix Doppler, MPD, VLC and your phone all
see.

```
music/
  Björk/Debut/01 Human Behaviour.flac   ← ID3v2 / Vorbis comments / MP4 atoms
  Björk/Debut/cover.jpg                 ← written alongside embedded art
  Playlists/
    Night Driving.m3u8                  ← relative paths: ../Björk/Debut/…
    Night Driving.jpg                   ← optional custom cover
  .hark/
    library.json                        ← the perfect-album flags
    renames.jsonl                       ← append-only rename journal
    sync-filters.txt                    ← rclone filter rules
```

Cover art is written **twice** on purpose — embedded in every track because
Doppler on iOS ignores folder images, and as `cover.jpg` because MPD and most
Linux players prefer the file.

`library.json` is the one non-standard file, because "perfect album" has no
standard home. It is a sorted list of folder paths, kept readable so you can
edit it by hand or have an agent do it:

```json
{ "version": 1, "perfect": ["Radiohead/Kid A"] }
```

`Playlists/` and `.hark/` are structurally invisible to the album scan — they
can never show up as an artist or an album.

## Three decisions worth knowing about

**Folders are the grouping truth, tags are the detail.** 1124 of the 2105 files
in this collection have no `album_artist`, so grouping by tags scatters
compilations that are already correctly foldered. Tags still supply track
titles, numbers, discs, durations and year.

**Everything is NFC-normalized.** This collection was partly assembled on
macOS, whose filesystem hands back decomposed filenames while the tags inside
those same files are precomposed. `Björk` the folder and `Björk` the tag were
different byte strings; 22 folders were affected. Anything that doesn't
normalize ends up with two of each artist — on disk, in the UI, and on B2.
Identity (ids, `library.json` keys, display) is NFC; filesystem access uses the
real on-disk bytes. Those are not interchangeable and conflating them is a bug.

**Renames are recorded, not inferred.** Changing an album's artist or title
moves its folder. That is only safe because hark! writes down what it did:
`hark sync` replays each move on B2 server-side — a `b2_copy_file` and a
delete, so no bytes cross your connection — instead of letting rclone guess
from a directory listing and produce a duplicate folder plus a full re-upload.
Renames are also sanitized (`:` → `_`, matching the convention already in the
collection), refused outright on a collision rather than merged, and routed
through a temp name when they differ only by case.

## Audio

Decoded natively with symphonia and played through cpal at **the file's own
sample rate** — not a fixed 48kHz. Covers MP3, FLAC, AAC and ALAC. Verified:
the stream registers as `1/44100`, `1/48000` or `1/96000` to match the file,
at `F32LE`, with no xruns.

**On this ThinkPad that is as far as it goes, and it does not matter.** Every
sink the machine has — Speaker and all three HDMI — advertises exactly one rate
in its `EnumFormat`: 48000, because the `sof-hda-dsp` topology pins the PCM
there. Something must resample, and widening PipeWire's `clock.allowed-rates`
does not change that; it only moves *where*. Stock (`[ 48000 ]`), hark's 44.1k
stream is converted by the graph adapter. Widened, the graph genuinely follows
to 44.1k or 96k — and then the ALSA sink converts instead. One resample either
way, same converter.

Measured, rather than assumed — a 44.1k → 48k sweep through PipeWire's
resampler at its default `resample.quality = 4`:

```
passband  100 Hz – 15 kHz   ±0.00 dB      worst aliasing image   -97 dB
          15 – 18 kHz       -0.18 dB      median artifact        -97 dB
          18 – 19 kHz       -2.45 dB
          19 – 20 kHz       -8.01 dB      (16-bit noise floor is -96 dB)
```

Flat across the whole audible band, with artifacts at or below the noise floor
of the 16-bit material most of this collection came from. The only measurable
change is above 18kHz. So `packaging/pipewire-rates.nix` buys **nothing** here;
keep it for the day an external DAC that advertises more than one rate gets
plugged in, which is the case it was written for.

Note also that a graph rate is global: if a browser tab is holding a 48kHz
stream open, 44.1kHz content is resampled while it plays regardless.

## Not built, on purpose

Queue, volume controls, gapless, crossfade, ReplayGain, streaming, lyrics,
scrobbling, ratings, play counts, smart playlists, a library database, phone
builds. The audio path is structured so gapless could be added later without a
rewrite.

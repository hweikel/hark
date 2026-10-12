//! `hark sync` — one command instead of remembering rclone invocations.
//!
//! The problem this solves: add an album from Bandcamp on the mac mini, forget
//! to copy it, and the two machines drift. The fix is `rclone bisync`, with two
//! pieces of care around it.
//!
//! **Renames are replayed, not inferred.** rclone normally has to work out that
//! a folder moved by diffing two listings, and when it can't it deletes 500MB
//! and re-uploads it under the new name — or worse, leaves both. hark! wrote
//! down every rename it performed (see `library/journal.rs`), so sync replays
//! them explicitly: server-side on B2 (a `b2_copy_file` + delete, so no bytes
//! cross the wire) and locally on whichever machine hasn't caught up yet.
//!
//! Making the two sides agree is not sufficient on its own, which cost 43
//! tracks on 2026-10-12 before it was understood. bisync compares both sides
//! against a *cached listing* of how they looked last run, so after a replay it
//! sees the moved folder as new on both sides — and if the album was re-tagged
//! in the same session, as a conflict, which parks the real filename as
//! `.conflict1`/`.conflict2` on both ends. So the replay also repoints those
//! listings; see `crate::bisync`.
//!
//! **Deletions are bounded.** `--max-delete` aborts a run that wants to remove
//! more than a couple of albums, which is the shape of every sync accident
//! worth worrying about.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};

use crate::bisync;
use crate::library::journal::{self, RenameRecord};
use crate::library::store::HARK_DIR;

/// Default remote. The `b2-music` entry points at the same bucket as
/// `b2-innercasmalia` but with `hard_delete = false`, so a deletion becomes a
/// restorable hidden version rather than being gone for good.
const DEFAULT_REMOTE: &str = "b2-music:innercasmalia/music";

/// Abort rather than propagate a deletion spree. Two or three albums is a
/// normal cleanup; thirty means something went wrong.
///
/// This is a **percentage of the files on a side**, not a count — bisync
/// reinterprets `--max-delete` that way, unlike every other rclone subcommand,
/// where it is a number of files. That is easy to misread: the 25 this started
/// as looked like "25 files" but meant ~528 of the 2114 here. 1% is ~21 files,
/// which is the guard the line above actually describes.
///
/// A trip is not data loss, just a halt — rerun with `--force` once you have
/// looked at what it wanted to remove.
const MAX_DELETE: &str = "1";

const FILTERS_FILE: &str = "sync-filters.txt";

/// Filter rules. rclone takes the first match, and the default is to include,
/// so only exclusions are needed.
const FILTERS: &str = "\
# Written by hark!. Edit freely — it is not regenerated if present.
# macOS and Windows litter.
- .DS_Store
- ._*
- Thumbs.db
- desktop.ini
# Half-written files from an interrupted atomic save.
- *.tmp
# rclone's own bisync workdir, if it ever lands inside the collection.
- .bisync/**
";

pub struct Options {
    /// Show what would happen, change nothing.
    pub check: bool,
    /// Establish the bisync baseline. Required once, before the first real sync.
    pub init: bool,
    pub remote: String,
}

impl Options {
    pub fn new(check: bool, init: bool, remote: Option<String>) -> Self {
        Options {
            check,
            init,
            remote: remote
                .or_else(|| std::env::var("HARK_REMOTE").ok())
                .filter(|r| !r.trim().is_empty())
                .unwrap_or_else(|| DEFAULT_REMOTE.to_string()),
        }
    }
}

pub fn run(root: &Path, options: &Options) -> Result<()> {
    if !root.is_dir() {
        bail!("{} is not a directory", root.display());
    }
    ensure_rclone()?;
    ensure_remote(&options.remote)?;
    let filters = ensure_filters(root)?;

    println!("collection: {}", root.display());
    println!("remote:     {}", options.remote);
    if options.check {
        println!("mode:       --check (nothing will be changed)\n");
    } else if options.init {
        println!("mode:       --init (establishing the bisync baseline)\n");
    } else {
        println!();
    }

    replay_renames(root, options)?;
    bisync(root, options, &filters)?;

    if !options.check {
        journal::mark_replayed(root).context("recording sync watermark")?;
    }
    Ok(())
}

fn ensure_rclone() -> Result<()> {
    let found = Command::new("rclone")
        .arg("version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if !found {
        bail!("rclone is not on PATH");
    }
    Ok(())
}

fn ensure_remote(remote: &str) -> Result<()> {
    let name = remote.split(':').next().unwrap_or(remote);
    let output = Command::new("rclone")
        .arg("listremotes")
        .output()
        .context("running rclone listremotes")?;
    let remotes = String::from_utf8_lossy(&output.stdout);
    if !remotes.lines().any(|l| l.trim_end_matches(':') == name) {
        bail!(
            "rclone remote `{name}` is not configured.\n\
             Create it pointing at the same bucket as your existing remote but with \
             hard_delete = false, so sync deletions stay recoverable:\n  \
             rclone config create {name} b2 account=<id> key=<key> hard_delete=false"
        );
    }
    Ok(())
}

fn ensure_filters(root: &Path) -> Result<PathBuf> {
    let dir = root.join(HARK_DIR);
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(FILTERS_FILE);
    if !path.exists() {
        std::fs::write(&path, FILTERS)
            .with_context(|| format!("writing {}", path.display()))?;
    }
    Ok(path)
}

/// Apply journal entries this machine hasn't acted on yet.
///
/// Two halves, both idempotent: move the folder locally if it is still under
/// its old name, and move it on the remote if the remote still has the old
/// path. Either may already be done — after a rename made on this machine the
/// local half is a no-op; after syncing from the other machine the remote half
/// is.
fn replay_renames(root: &Path, options: &Options) -> Result<()> {
    let pending = journal::pending(root);
    if pending.is_empty() {
        println!("renames to replay: none");
        return Ok(());
    }

    // Looked up once: both are process-wide constants, and `cache_dir` shells
    // out to rclone. A failure here must not sink the sync — the listings are
    // an optimisation, and bisync still behaves correctly without them, just
    // more expensively.
    let listings = (!options.check)
        .then(bisync::cache_dir)
        .and_then(|r| match r {
            Ok(dir) => Some((dir, bisync::session_name(root, &options.remote))),
            Err(e) => {
                println!("note: could not locate rclone's cache ({e}); bisync will re-examine the renamed folders");
                None
            }
        });

    println!("renames to replay: {}", pending.len());
    for record in &pending {
        println!("  {} → {}", record.from, record.to);
        let local_moved = replay_local(root, record, options)?;
        let remote_moved = replay_remote(record, options)?;

        // Only for the sides that really ended up at the new path. Telling
        // bisync a move happened when it didn't would have it read the old
        // path as new and the new one as deleted, and propagate that deletion.
        if let Some((cache, session)) = &listings {
            let sides = [
                (bisync::Side::Local, local_moved),
                (bisync::Side::Remote, remote_moved),
            ];
            let mut total = 0;
            for (side, moved) in sides {
                if !moved {
                    continue;
                }
                total += bisync::record_rename(cache, session, side, &record.from, &record.to)
                    .with_context(|| format!("updating bisync listing for {}", record.to))?;
            }
            if total > 0 {
                println!("    bisync: {total} listing entries repointed");
            }
        }
    }
    println!();
    Ok(())
}

/// Returns whether this side now sits at the new path, which is what decides
/// if bisync's listing may be repointed.
fn replay_local(root: &Path, record: &RenameRecord, options: &Options) -> Result<bool> {
    let from = root.join(&record.from);
    let to = root.join(&record.to);

    if !from.is_dir() || to.exists() {
        return Ok(to.is_dir()); // Already applied here — or never existed here.
    }
    if options.check {
        println!("    local:  would move {} → {}", record.from, record.to);
        return Ok(false);
    }

    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::rename(&from, &to)
        .with_context(|| format!("moving {} to {}", from.display(), to.display()))?;
    println!("    local:  moved");
    Ok(true)
}

/// Move a folder on the remote.
///
/// B2 has no native move, so rclone implements this as a server-side
/// `b2_copy_file` followed by a delete — the bytes never leave Backblaze, which
/// is the entire reason the journal exists.
fn replay_remote(record: &RenameRecord, options: &Options) -> Result<bool> {
    let from = format!("{}/{}", options.remote, record.from);
    let to = format!("{}/{}", options.remote, record.to);

    let mut command = Command::new("rclone");
    command.arg("move").arg(&from).arg(&to);
    if options.check {
        command.arg("--dry-run");
    }

    let output = command.output().context("running rclone move")?;
    if output.status.success() {
        println!(
            "    remote: {}",
            if options.check { "would move (server-side)" } else { "moved (server-side)" }
        );
        return Ok(!options.check);
    }

    // A missing source usually means the remote already has the new path —
    // not worth failing the whole sync over. But "usually" is not good enough
    // to then rewrite bisync's baseline on, so ask the remote directly.
    let stderr = String::from_utf8_lossy(&output.stderr);
    println!("    remote: skipped ({})", stderr.lines().last().unwrap_or("").trim());
    Ok(!options.check && remote_has(&to))
}

/// Whether a path exists on the remote. Used only to decide if a skipped move
/// was already done, so a listing failure is simply "assume not".
fn remote_has(path: &str) -> bool {
    Command::new("rclone")
        .arg("lsf")
        .arg("--max-depth")
        .arg("1")
        .arg(path)
        .output()
        .map(|o| o.status.success() && !o.stdout.is_empty())
        .unwrap_or(false)
}

fn bisync(root: &Path, options: &Options, filters: &Path) -> Result<()> {
    let mut command = Command::new("rclone");
    command
        .arg("bisync")
        .arg(root)
        .arg(&options.remote)
        .arg("--filter-from")
        .arg(filters)
        .arg("--max-delete")
        .arg(MAX_DELETE)
        .arg("--create-empty-src-dirs=false")
        .arg("--progress");

    if options.init {
        command.arg("--resync");
    }
    if options.check {
        command.arg("--dry-run");
    }

    println!("running: rclone bisync …");
    let status = command.status().context("running rclone bisync")?;

    if !status.success() {
        if options.init {
            bail!("bisync --resync failed");
        }
        bail!(
            "bisync failed.\n\
             If this is the first run against this remote, establish the baseline with:\n  \
             hark sync --init"
        );
    }
    Ok(())
}

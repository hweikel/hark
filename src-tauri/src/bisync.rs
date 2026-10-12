//! Keeping rclone bisync's baseline honest about renames we performed ourselves.
//!
//! bisync decides what changed by diffing the two sides against a cached
//! listing of how they looked last run. Replaying a rename server-side moves
//! the folder on both sides but leaves that cache pointing at the old path, so
//! the next run sees a file that is *new on both sides*. When the contents also
//! match it reconciles silently and nobody notices. When they differ — the
//! album was renamed and re-tagged in the same session — bisync calls it a
//! conflict and renames both copies to `.conflict1`/`.conflict2`, which takes
//! the real filename out of circulation on both sides.
//!
//! That happened to 43 tracks across 5 albums on 2026-10-12. The fix is to tell
//! bisync what we did: rewrite the old path to the new one inside its listings,
//! so the baseline describes the world as it now is.
//!
//! **A listing is only rewritten for a side that actually moved.** Claiming a
//! move that didn't happen is worse than claiming nothing: bisync would read
//! the old path as new-on-that-side and the new path as deleted, and propagate
//! that deletion to the other side.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};

/// Which of the two sides a listing describes. bisync names them positionally:
/// path1 is the first argument it was given, path2 the second.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Side {
    /// The local collection.
    Local,
    /// The remote.
    Remote,
}

impl Side {
    fn suffix(self) -> &'static str {
        match self {
            Side::Local => "path1",
            Side::Remote => "path2",
        }
    }
}

/// bisync's name for a given pair of endpoints.
///
/// Mirrors rclone's own canonicalisation: strip the surrounding slashes, then
/// flatten `/` and `:` to underscores, joining the two sides with `..`. For
/// `/home/hweikel/music` against `b2-music:innercasmalia/music` that gives
/// `home_hweikel_music..b2-music_innercasmalia_music`.
pub fn session_name(root: &Path, remote: &str) -> String {
    fn canon(s: &str) -> String {
        s.trim_matches('/')
            .replace(['/', ':'], "_")
    }
    format!(
        "{}..{}",
        canon(&root.to_string_lossy()),
        canon(remote)
    )
}

/// Where rclone keeps its cache, asked of rclone rather than guessed, so an
/// overridden `--cache-dir` or `RCLONE_CACHE_DIR` is still found.
pub fn cache_dir() -> Result<PathBuf> {
    let output = Command::new("rclone")
        .arg("config")
        .arg("paths")
        .output()
        .context("running rclone config paths")?;
    let text = String::from_utf8_lossy(&output.stdout);
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("Cache dir:") {
            return Ok(PathBuf::from(rest.trim()));
        }
    }
    anyhow::bail!("rclone config paths did not report a cache dir")
}

fn listing_path(cache: &Path, session: &str, side: Side) -> PathBuf {
    cache
        .join("bisync")
        .join(format!("{session}.{}.lst", side.suffix()))
}

/// Escape a path the way Go's `%q` does, which is what rclone writes listings
/// with. Only `\` and `"` need handling — everything printable, including the
/// Korean and Japanese folder names in this collection, is stored verbatim.
fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str(r"\\"),
            '"' => out.push_str("\\\""),
            _ => out.push(c),
        }
    }
    out
}

/// Repoint every entry under `from` at `to` in one listing's text.
///
/// Returns the rewritten text and how many entries moved. Lines are left
/// byte-identical unless they match, so comments, the header and every
/// unrelated entry survive untouched.
fn rewrite_listing(text: &str, from: &str, to: &str) -> (String, usize) {
    let esc_from = escape(from);
    let esc_to = escape(to);
    let mut moved = 0;
    let mut out = String::with_capacity(text.len());

    for line in text.split_inclusive('\n') {
        let body = line.trim_end_matches(['\n', '\r']);
        let newline = &line[body.len()..];

        // The path is the final field and the only quoted one.
        let (Some(open), Some(close)) = (body.find('"'), body.rfind('"')) else {
            out.push_str(line);
            continue;
        };
        if close <= open {
            out.push_str(line);
            continue;
        }

        let quoted = &body[open + 1..close];
        let rewritten = if quoted == esc_from {
            Some(esc_to.clone())
        } else if let Some(rest) = quoted.strip_prefix(&format!("{esc_from}/")) {
            Some(format!("{esc_to}/{rest}"))
        } else {
            None
        };

        match rewritten {
            Some(path) => {
                moved += 1;
                out.push_str(&body[..open + 1]);
                out.push_str(&path);
                out.push_str(&body[close..]);
                out.push_str(newline);
            }
            None => out.push_str(line),
        }
    }

    (out, moved)
}

/// Apply a completed rename to one side's cached listing.
///
/// A missing listing is not an error: it just means bisync has no baseline yet,
/// which is the state before `hark sync --init`.
pub fn record_rename(cache: &Path, session: &str, side: Side, from: &str, to: &str) -> Result<usize> {
    let path = listing_path(cache, session, side);
    if !path.is_file() {
        return Ok(0);
    }

    let text = std::fs::read_to_string(&path)
        .with_context(|| format!("reading {}", path.display()))?;
    let (rewritten, moved) = rewrite_listing(&text, from, to);
    if moved == 0 {
        return Ok(0);
    }

    // Write beside the original and rename over it, so an interrupted write
    // cannot leave bisync with a half-listing — it would resync the world.
    let tmp = path.with_extension("lst.hark-tmp");
    std::fs::write(&tmp, rewritten).with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, &path)
        .with_context(|| format!("replacing {}", path.display()))?;

    Ok(moved)
}

#[cfg(test)]
mod tests {
    use super::*;

    const LISTING: &str = concat!(
        "# bisync listing v1 from 2026-10-12T16:45:29.947184586+0000\n",
        "- 1758 - - 2026-10-12T16:34:34.574762249+0000 \".hark/library.json\"\n",
        "- 8214574 - - 2025-08-26T13:40:17.000000000+0000 \"Mount Kimbie/Love What Survives/1 Four Years.mp3\"\n",
        "- 5297713 - - 2025-08-26T13:40:18.000000000+0000 \"Mount Kimbie/Love What Survives/2 Blue Train.mp3\"\n",
        "- 9512319 - - 2024-04-13T00:00:00.000000000+0000 \"Mount Kimbie/The Sunset Violent/1 The Trail.mp3\"\n",
        "- 1024 - - 2024-04-13T00:00:00.000000000+0000 \"Mountain Goats/Goths/1 Rain.mp3\"\n",
    );

    #[test]
    fn session_name_matches_rclones_own() {
        // Verified against the file rclone actually created in ~/.cache.
        assert_eq!(
            session_name(Path::new("/home/hweikel/music"), "b2-music:innercasmalia/music"),
            "home_hweikel_music..b2-music_innercasmalia_music"
        );
    }

    #[test]
    fn moves_every_entry_under_the_renamed_folder() {
        let (out, moved) = rewrite_listing(
            LISTING,
            "Mount Kimbie/Love What Survives",
            "King Krule/Love What Survives",
        );
        assert_eq!(moved, 2);
        assert!(out.contains("\"King Krule/Love What Survives/1 Four Years.mp3\""));
        assert!(out.contains("\"King Krule/Love What Survives/2 Blue Train.mp3\""));
        // The other album under the same old artist is untouched.
        assert!(out.contains("\"Mount Kimbie/The Sunset Violent/1 The Trail.mp3\""));
    }

    #[test]
    fn leaves_the_header_and_unrelated_entries_byte_identical() {
        let (out, _) = rewrite_listing(LISTING, "Mount Kimbie/Love What Survives", "King Krule/Love What Survives");
        assert!(out.starts_with("# bisync listing v1 from 2026-10-12T16:45:29.947184586+0000\n"));
        assert!(out.contains("- 1758 - - 2026-10-12T16:34:34.574762249+0000 \".hark/library.json\"\n"));
        assert_eq!(out.lines().count(), LISTING.lines().count());
    }

    /// "Mount Kimbie" must not drag "Mountain Goats" along with it.
    #[test]
    fn a_prefix_that_is_not_a_path_boundary_does_not_match() {
        let (out, moved) = rewrite_listing(LISTING, "Mount", "Moved");
        assert_eq!(moved, 0);
        assert_eq!(out, LISTING);
    }

    #[test]
    fn rewrites_non_ascii_paths() {
        let listing = concat!(
            "# bisync listing v1\n",
            "- 10 - - 2026-01-01T00:00:00.000000000+0000 \"Bjo\u{308}rk/Debut/1 Human Behaviour.mp3\"\n",
        );
        let (out, moved) = rewrite_listing(listing, "Bjo\u{308}rk", "Bj\u{f6}rk");
        assert_eq!(moved, 1);
        assert!(out.contains("\"Bj\u{f6}rk/Debut/1 Human Behaviour.mp3\""));
    }

    #[test]
    fn handles_a_quote_in_the_folder_name() {
        let listing = "- 10 - - 2026-01-01T00:00:00.000000000+0000 \"Say \\\"Yes\\\"/a.mp3\"\n";
        let (out, moved) = rewrite_listing(listing, "Say \"Yes\"", "Say Yes");
        assert_eq!(moved, 1);
        assert!(out.contains("\"Say Yes/a.mp3\""));
    }

    #[test]
    fn a_missing_listing_is_not_an_error() {
        let dir = std::env::temp_dir().join("hark-bisync-missing");
        let _ = std::fs::remove_dir_all(&dir);
        let moved = record_rename(&dir, "nope", Side::Local, "a", "b").unwrap();
        assert_eq!(moved, 0);
    }

    #[test]
    fn record_rename_replaces_the_listing_in_place() {
        let dir = std::env::temp_dir().join("hark-bisync-write");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("bisync")).unwrap();
        let path = dir.join("bisync").join("sess.path2.lst");
        std::fs::write(&path, LISTING).unwrap();

        let moved = record_rename(
            &dir,
            "sess",
            Side::Remote,
            "Mount Kimbie/Love What Survives",
            "King Krule/Love What Survives",
        )
        .unwrap();

        assert_eq!(moved, 2);
        let after = std::fs::read_to_string(&path).unwrap();
        assert!(after.contains("\"King Krule/Love What Survives/1 Four Years.mp3\""));
        assert!(!after.contains("\"Mount Kimbie/Love What Survives/"));
        // And nothing was left lying around next to it.
        assert!(!dir.join("bisync").join("sess.path2.lst.hark-tmp").exists());
    }
}

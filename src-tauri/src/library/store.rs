//! `.hark/library.json` — the only piece of state with no standard to lean on.
//!
//! Everything else hark! knows how to say lives in a format some other program
//! already understands: tags in the files, playlists as M3U, cover art as
//! `cover.jpg`. "Perfect album" has no such home, so it gets one small file,
//! kept deliberately readable so an agent (or a text editor) can work on it:
//!
//! ```json
//! { "version": 1, "perfect": ["Radiohead/Kid A", "Björk/Debut"] }
//! ```
//!
//! Entries are album folder paths relative to the music root, NFC-normalized,
//! slash-separated, and kept sorted so diffs stay legible across syncs.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use super::norm::nfc;

pub const HARK_DIR: &str = ".hark";
pub const PLAYLISTS_DIR: &str = "Playlists";
const LIBRARY_FILE: &str = "library.json";
const VERSION: u32 = 1;

#[derive(Serialize, Deserialize)]
struct LibraryFile {
    version: u32,
    perfect: Vec<String>,
}

pub struct Store {
    path: PathBuf,
    perfect: BTreeSet<String>,
}

impl Store {
    /// Load the sidecar, or start empty if it doesn't exist yet.
    ///
    /// A missing file is the normal first-run state and means "nothing is
    /// perfect yet" — the marquee starts empty and gets earned.
    pub fn load(root: &Path) -> Self {
        let path = root.join(HARK_DIR).join(LIBRARY_FILE);
        let perfect = fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str::<LibraryFile>(&s).ok())
            .map(|f| f.perfect.into_iter().map(|p| nfc(&p)).collect())
            .unwrap_or_default();
        Store { path, perfect }
    }

    pub fn is_perfect(&self, rel_path: &str) -> bool {
        self.perfect.contains(&nfc(rel_path))
    }

    pub fn set_perfect(&mut self, rel_path: &str, value: bool) -> Result<()> {
        let key = nfc(rel_path);
        let changed = if value {
            self.perfect.insert(key)
        } else {
            self.perfect.remove(&key)
        };
        if changed {
            self.save()?;
        }
        Ok(())
    }

    /// Move a flag from one album path to another.
    ///
    /// Called in the same operation as a folder rename so a perfect flag can
    /// never be orphaned by a path that no longer exists. A no-op when the album
    /// wasn't flagged, which keeps the caller from having to check first.
    pub fn rekey(&mut self, from: &str, to: &str) -> Result<()> {
        if self.perfect.remove(&nfc(from)) {
            self.perfect.insert(nfc(to));
            self.save()?;
        }
        Ok(())
    }

    /// Write atomically: a torn `library.json` would silently lose the entire
    /// curation history, which is the one thing here that can't be rebuilt from
    /// the files on disk.
    fn save(&self) -> Result<()> {
        let dir = self
            .path
            .parent()
            .context("library.json has no parent directory")?;
        fs::create_dir_all(dir)
            .with_context(|| format!("creating {}", dir.display()))?;

        let file = LibraryFile {
            version: VERSION,
            perfect: self.perfect.iter().cloned().collect(),
        };
        let json = serde_json::to_string_pretty(&file)?;

        let tmp = self.path.with_extension("json.tmp");
        fs::write(&tmp, json.as_bytes())
            .with_context(|| format!("writing {}", tmp.display()))?;
        fs::rename(&tmp, &self.path)
            .with_context(|| format!("replacing {}", self.path.display()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("hark-store-test-{name}"));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn first_run_has_nothing_perfect() {
        let root = temp_root("first-run");
        let store = Store::load(&root);
        assert!(!store.is_perfect("Radiohead/Kid A"));
    }

    #[test]
    fn flags_round_trip_through_disk() {
        let root = temp_root("round-trip");
        let mut store = Store::load(&root);
        store.set_perfect("Radiohead/Kid A", true).unwrap();

        let reloaded = Store::load(&root);
        assert!(reloaded.is_perfect("Radiohead/Kid A"));
        assert!(!reloaded.is_perfect("Radiohead/Amnesiac"));
    }

    #[test]
    fn flags_survive_unicode_form_mismatch() {
        // Flag written from a precomposed tag, read back from a decomposed path.
        let root = temp_root("unicode");
        let mut store = Store::load(&root);
        store.set_perfect("Bj\u{00f6}rk/Debut", true).unwrap();

        let reloaded = Store::load(&root);
        assert!(reloaded.is_perfect("Bjo\u{0308}rk/Debut"));
    }

    #[test]
    fn rekey_moves_a_flag_and_leaves_others_alone() {
        let root = temp_root("rekey");
        let mut store = Store::load(&root);
        store.set_perfect("Bjork/Debut", true).unwrap();
        store.set_perfect("Radiohead/Kid A", true).unwrap();

        store.rekey("Bjork/Debut", "Bj\u{00f6}rk/Debut").unwrap();

        let reloaded = Store::load(&root);
        assert!(!reloaded.is_perfect("Bjork/Debut"));
        assert!(reloaded.is_perfect("Bj\u{00f6}rk/Debut"));
        assert!(reloaded.is_perfect("Radiohead/Kid A"));
    }

    #[test]
    fn rekey_of_an_unflagged_album_is_a_no_op() {
        let root = temp_root("rekey-noop");
        let mut store = Store::load(&root);
        store.rekey("Nobody/Nothing", "Nobody/Something").unwrap();
        assert!(!Store::load(&root).is_perfect("Nobody/Something"));
    }

    #[test]
    fn file_is_human_readable_and_sorted() {
        let root = temp_root("readable");
        let mut store = Store::load(&root);
        store.set_perfect("Radiohead/Kid A", true).unwrap();
        store.set_perfect("Björk/Debut", true).unwrap();

        let text = fs::read_to_string(root.join(HARK_DIR).join(LIBRARY_FILE)).unwrap();
        assert!(text.contains("\"version\": 1"));
        // Sorted, so syncing two machines produces a legible diff.
        let bjork = text.find("Björk/Debut").unwrap();
        let radiohead = text.find("Radiohead/Kid A").unwrap();
        assert!(bjork < radiohead);
    }

    #[test]
    fn no_tmp_file_is_left_behind() {
        let root = temp_root("no-tmp");
        let mut store = Store::load(&root);
        store.set_perfect("A/B", true).unwrap();
        let leftovers: Vec<_> = fs::read_dir(root.join(HARK_DIR))
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains("tmp"))
            .collect();
        assert!(leftovers.is_empty(), "atomic write left a temp file behind");
    }
}

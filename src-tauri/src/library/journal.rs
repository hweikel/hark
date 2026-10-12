//! The rename journal: `.hark/renames.jsonl`.
//!
//! This file is the whole reason folder renames are safe to do automatically.
//!
//! Normally a sync tool has to *infer* that a rename happened by diffing two
//! directory listings, and when it guesses wrong you get a duplicate folder on
//! the remote and a 500MB re-upload. hark! never has to guess — it performed
//! the rename, so it writes down what it did, and `hark sync` replays those
//! moves on B2 server-side before syncing file contents at all.
//!
//! Append-only, one JSON object per line, so it stays readable and an
//! interrupted write can only ever lose the last entry.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use super::store::HARK_DIR;

const JOURNAL_FILE: &str = "renames.jsonl";
const STATE_FILE: &str = "sync-state.json";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RenameRecord {
    /// RFC3339-ish UTC timestamp. Informational — ordering comes from position
    /// in the file, which is what the watermark counts.
    pub ts: String,
    /// Album path relative to the music root, before the rename.
    pub from: String,
    /// Album path relative to the music root, after.
    pub to: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct SyncState {
    /// How many journal lines `hark sync` has already replayed to the remote.
    #[serde(default)]
    renames_replayed: usize,
}

pub fn journal_path(root: &Path) -> PathBuf {
    root.join(HARK_DIR).join(JOURNAL_FILE)
}

/// Where this machine records how far it has got through the journal.
///
/// Deliberately *outside* the music directory. The journal itself syncs with
/// the collection — that's what lets the other machine learn about a rename —
/// but how far each machine has replayed is a per-machine fact. Keeping the
/// watermark in `.hark/` would sync it too, and the second machine would
/// conclude it had already applied renames it has never seen.
///
/// Takes the base directory explicitly so tests keep their bookkeeping inside
/// their own fixture rather than writing into the real `~/.local/state`.
fn state_path_in(root: &Path, base: &Path) -> PathBuf {
    let mut key = root.to_string_lossy().replace('/', "_");
    key.retain(|c| c.is_alphanumeric() || c == '_' || c == '-' || c == '.');
    base.join("hark").join(format!("{key}-{STATE_FILE}"))
}

fn default_state_dir() -> PathBuf {
    std::env::var("XDG_STATE_HOME")
        .ok()
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var("HOME").ok().map(|h| PathBuf::from(h).join(".local/state")))
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Record a completed rename. Called *after* the rename succeeds on disk.
pub fn append(root: &Path, from: &str, to: &str) -> Result<()> {
    let path = journal_path(root);
    let dir = path.parent().context("journal has no parent directory")?;
    fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;

    let record = RenameRecord {
        ts: timestamp(),
        from: from.to_string(),
        to: to.to_string(),
    };
    let mut line = serde_json::to_string(&record)?;
    line.push('\n');

    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .with_context(|| format!("opening {}", path.display()))?;
    file.write_all(line.as_bytes())
        .with_context(|| format!("appending to {}", path.display()))?;
    Ok(())
}

pub fn read_all(root: &Path) -> Vec<RenameRecord> {
    let Ok(text) = fs::read_to_string(journal_path(root)) else {
        return Vec::new();
    };
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

/// Renames recorded but not yet replayed to the remote.
pub fn pending(root: &Path) -> Vec<RenameRecord> {
    pending_in(root, &default_state_dir())
}

/// Mark every journal entry as replayed. Called after a successful sync.
pub fn mark_replayed(root: &Path) -> Result<()> {
    mark_replayed_in(root, &default_state_dir())
}

fn pending_in(root: &Path, state_dir: &Path) -> Vec<RenameRecord> {
    let all = read_all(root);
    let done = load_state(root, state_dir).renames_replayed.min(all.len());
    all[done..].to_vec()
}

fn mark_replayed_in(root: &Path, state_dir: &Path) -> Result<()> {
    let total = read_all(root).len();
    save_state(
        root,
        state_dir,
        &SyncState {
            renames_replayed: total,
        },
    )
}

fn load_state(root: &Path, state_dir: &Path) -> SyncState {
    fs::read_to_string(state_path_in(root, state_dir))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_state(root: &Path, state_dir: &Path, state: &SyncState) -> Result<()> {
    let path = state_path_in(root, state_dir);
    let dir = path.parent().context("state file has no parent")?;
    fs::create_dir_all(dir)?;
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, serde_json::to_string_pretty(state)?)?;
    fs::rename(&tmp, &path)?;
    Ok(())
}

/// UTC timestamp without pulling in a date library for one string.
fn timestamp() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    // Civil-from-days (Howard Hinnant's algorithm).
    let days = (secs / 86_400) as i64;
    let tod = secs % 86_400;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };

    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        tod / 3600,
        (tod % 3600) / 60,
        tod % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("hark-journal-test-{name}"));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn appends_and_reads_back() {
        let root = fixture("append");
        append(&root, "Bjork/Debut", "Björk/Debut").unwrap();
        append(&root, "A/B", "A/C").unwrap();

        let all = read_all(&root);
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].from, "Bjork/Debut");
        assert_eq!(all[0].to, "Björk/Debut");
        assert_eq!(all[1].to, "A/C");
    }

    #[test]
    fn one_json_object_per_line_so_it_stays_readable() {
        let root = fixture("format");
        append(&root, "A/B", "A/C").unwrap();
        append(&root, "D/E", "D/F").unwrap();

        let text = fs::read_to_string(journal_path(&root)).unwrap();
        let lines: Vec<_> = text.lines().filter(|l| !l.is_empty()).collect();
        assert_eq!(lines.len(), 2);
        for line in lines {
            let parsed: RenameRecord = serde_json::from_str(line).unwrap();
            assert!(parsed.ts.ends_with('Z'), "timestamp should be UTC: {parsed:?}");
        }
    }

    #[test]
    fn pending_tracks_what_sync_has_not_replayed() {
        let root = fixture("pending");
        // Keep the watermark inside the fixture: the real one lives in
        // ~/.local/state and tests have no business writing there.
        let state = root.join("state");

        append(&root, "A/1", "A/2").unwrap();
        assert_eq!(pending_in(&root, &state).len(), 1);

        mark_replayed_in(&root, &state).unwrap();
        assert_eq!(
            pending_in(&root, &state).len(),
            0,
            "replayed renames shouldn't repeat"
        );

        append(&root, "B/1", "B/2").unwrap();
        let next = pending_in(&root, &state);
        assert_eq!(next.len(), 1);
        assert_eq!(next[0].from, "B/1");
    }

    #[test]
    fn the_watermark_lives_outside_the_collection() {
        // It must not sync: it is a per-machine fact, and syncing it would make
        // the second machine think it had already applied renames it has never
        // seen.
        let root = fixture("watermark-location");
        let state = root.join("state");
        mark_replayed_in(&root, &state).unwrap();

        assert!(!root.join(HARK_DIR).join(STATE_FILE).exists());
        assert!(state_path_in(&root, &state).starts_with(&state));
    }

    #[test]
    fn separate_collections_get_separate_watermarks() {
        let base = fixture("two-collections").join("state");
        let a = state_path_in(Path::new("/home/x/music"), &base);
        let b = state_path_in(Path::new("/mnt/t7/music"), &base);
        assert_ne!(a, b);
    }

    #[test]
    fn a_missing_journal_is_not_an_error() {
        let root = fixture("missing");
        assert!(read_all(&root).is_empty());
        assert!(pending_in(&root, &root.join("state")).is_empty());
    }

    #[test]
    fn a_corrupt_line_does_not_lose_the_rest() {
        let root = fixture("corrupt");
        append(&root, "A/1", "A/2").unwrap();
        let path = journal_path(&root);
        let mut text = fs::read_to_string(&path).unwrap();
        text.push_str("{ this is not json\n");
        fs::write(&path, text).unwrap();
        append(&root, "B/1", "B/2").unwrap();

        let all = read_all(&root);
        assert_eq!(all.len(), 2, "valid entries should survive a torn line");
    }

    #[test]
    fn timestamp_is_plausible() {
        let ts = timestamp();
        // Format: YYYY-MM-DDTHH:MM:SSZ
        assert_eq!(ts.len(), 20, "unexpected timestamp shape: {ts}");
        let year: i32 = ts[..4].parse().unwrap();
        assert!((2024..2100).contains(&year), "implausible year in {ts}");
        assert_eq!(&ts[4..5], "-");
        assert_eq!(&ts[10..11], "T");
    }
}

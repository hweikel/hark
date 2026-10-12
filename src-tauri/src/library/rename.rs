//! Moving an album folder safely.
//!
//! Folder renames happen automatically when you change an album's artist or
//! title, which is only tolerable because of five specific guards:
//!
//! 1. **NFC** — every path hark! writes is normalized, so the same album can't
//!    end up spelled two ways across macOS and NixOS (the existing two-Björks
//!    bug).
//! 2. **Sanitized** — tag text becomes a path component that is legal on every
//!    OS and on B2, using the `_`-for-`:` convention already in this collection.
//! 3. **Case-only renames go through a temp name**, because on a
//!    case-insensitive filesystem `mv beatles Beatles` is either a no-op or an
//!    error, and on a case-sensitive one it is a real move.
//! 4. **Collisions are refused, never merged.** If a *different* folder already
//!    occupies the target, the rename fails loudly rather than mixing two
//!    albums together.
//! 5. **Journalled**, so `hark sync` can replay the move on B2 server-side
//!    instead of letting rclone infer it from a listing and produce duplicates.
//!
//! The perfect-album flag is re-keyed in the same operation, so it can never be
//! orphaned by a path that no longer exists.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};

use super::journal;
use super::norm::{nfc, sanitize_filename};
use super::store::Store;

#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The sanitized target matched where the album already is.
    Unchanged,
    Renamed { from: String, to: String },
}

/// Where an album *would* move to, without touching anything.
///
/// Used by the edit modal to show the literal rename before you commit to it.
pub fn planned_path(artist: &str, album: &str) -> String {
    format!(
        "{}/{}",
        sanitize_filename(artist),
        sanitize_filename(album)
    )
}

/// Rename an album folder to the location implied by `artist` and `album`.
///
/// `from_dir` is the album's real path on disk; `from_rel` is its NFC identity
/// (the `library.json` key). Those differ whenever a folder name is stored
/// decomposed, as this collection's Björk directory is — so reconstructing the
/// path from `from_rel` would name something that does not exist.
pub fn rename_album(
    root: &Path,
    from_dir: &Path,
    from_rel: &str,
    artist: &str,
    album: &str,
) -> Result<Outcome> {
    let from_rel = nfc(from_rel);
    let to_rel = planned_path(artist, album);

    // Compared in NFC: a decomposed folder whose NFC name already matches the
    // target still needs renaming, so that it lands on disk normalized.
    if to_rel == from_rel && from_dir == root.join(&to_rel) {
        return Ok(Outcome::Unchanged);
    }

    let from_path = from_dir.to_path_buf();
    let to_path = root.join(&to_rel);

    if !from_path.is_dir() {
        bail!("{} is not a directory", from_path.display());
    }

    if to_path.exists() {
        if is_same_dir(&from_path, &to_path)? {
            // Guard 3: differs only by case (or Unicode form) on a filesystem
            // that considers the two names equal. A direct rename would be a
            // no-op or an error, so go via a name neither spelling collides
            // with.
            rename_via_temp(&from_path, &to_path)?;
        } else {
            // Guard 4.
            bail!(
                "{to_rel} already exists and is a different album — \
                 refusing to merge two folders"
            );
        }
    } else {
        if let Some(parent) = to_path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        fs::rename(&from_path, &to_path)
            .with_context(|| format!("renaming {} to {}", from_path.display(), to_path.display()))?;
    }

    // An artist folder left behind with nothing in it is clutter that would
    // show up as an empty artist on the next scan.
    prune_empty_parent(root, &from_path);
    respell_sole_parent(root, &to_path)?;

    // Guard 5, and the flag re-key, in the same operation as the move.
    journal::append(root, &from_rel, &to_rel)?;
    Store::load(root).rekey(&from_rel, &to_rel)?;

    Ok(Outcome::Renamed {
        from: from_rel,
        to: to_rel,
    })
}

/// Whether two paths name the same directory on disk.
///
/// On a case-insensitive filesystem `Bjork` and `BJORK` resolve to one inode;
/// on a case-sensitive one they are two. Canonicalizing answers both.
fn is_same_dir(a: &Path, b: &Path) -> Result<bool> {
    let (Ok(a), Ok(b)) = (a.canonicalize(), b.canonicalize()) else {
        return Ok(false);
    };
    Ok(a == b)
}

fn rename_via_temp(from: &Path, to: &Path) -> Result<()> {
    let parent = from
        .parent()
        .ok_or_else(|| anyhow!("{} has no parent", from.display()))?;

    let mut temp = parent.join(".hark-rename-tmp");
    let mut counter = 0;
    while temp.exists() {
        counter += 1;
        temp = parent.join(format!(".hark-rename-tmp-{counter}"));
    }

    fs::rename(from, &temp)
        .with_context(|| format!("staging {} as {}", from.display(), temp.display()))?;

    if let Some(target_parent) = to.parent() {
        fs::create_dir_all(target_parent)?;
    }

    if let Err(e) = fs::rename(&temp, to) {
        // Put it back rather than leaving the album under a hidden name.
        let _ = fs::rename(&temp, from);
        return Err(e).with_context(|| format!("renaming into {}", to.display()));
    }
    Ok(())
}

/// Filesystem litter that shouldn't keep an otherwise-empty artist folder
/// alive. This collection carries 34 `.DS_Store` files from its time on macOS.
const JUNK_FILES: &[&str] = &[".DS_Store", "Thumbs.db", "desktop.ini", ".directory"];

fn is_junk(name: &str) -> bool {
    // `._Foo` are macOS AppleDouble resource forks.
    JUNK_FILES.contains(&name) || name.starts_with("._")
}

/// Remove the old artist directory if the move left nothing real behind.
///
/// "Nothing real" has to tolerate junk: a stray `.DS_Store` would otherwise
/// keep the *old* spelling of an artist folder alive next to the new one — which
/// is how a decomposed `Björk` and a precomposed `Björk` end up side by side,
/// the exact split this rename was fixing. Only known litter is removed; a
/// directory holding anything else is left strictly alone.
fn prune_empty_parent(root: &Path, moved_from: &Path) {
    let Some(parent) = moved_from.parent() else {
        return;
    };
    if parent == root {
        return;
    }

    let Ok(entries) = fs::read_dir(parent) else {
        return;
    };

    let mut junk = Vec::new();
    for entry in entries.filter_map(|e| e.ok()) {
        let name = entry.file_name().to_string_lossy().into_owned();
        if entry.path().is_file() && is_junk(&name) {
            junk.push(entry.path());
        } else {
            // Something real lives here; leave the whole directory alone.
            return;
        }
    }

    for path in junk {
        let _ = fs::remove_file(path);
    }
    let _ = fs::remove_dir(parent);
}

/// Give the artist folder its NFC spelling on a normalization-insensitive
/// filesystem (APFS).
///
/// On Linux, moving `Bjo◌̈rk/Debut` to `Björk/Debut` creates a fresh NFC
/// `Björk` and `prune_empty_parent` removes the old one. On APFS the two names
/// are the same directory, so `create_dir_all` reuses the decomposed folder and
/// the album lands straight back inside it. Respelling it via a temp name gives
/// the same end state as Linux. Only done when the moved album is the folder's
/// sole occupant — the case where Linux would have pruned the old spelling —
/// so no other album's path changes without a journal entry.
fn respell_sole_parent(root: &Path, to_path: &Path) -> Result<()> {
    let (Some(parent), Some(album)) = (to_path.parent(), to_path.file_name()) else {
        return Ok(());
    };
    let (Some(grand), Some(wanted)) = (parent.parent(), parent.file_name()) else {
        return Ok(());
    };
    if parent == root {
        return Ok(());
    }
    let Some(wanted) = wanted.to_str() else {
        return Ok(());
    };

    let Some(on_disk) = fs::read_dir(grand)?
        .filter_map(|e| e.ok())
        .map(|e| e.file_name())
        .find(|n| n.to_str().map(nfc).as_deref() == Some(wanted))
    else {
        return Ok(());
    };
    if on_disk == wanted {
        return Ok(());
    }

    let sole = fs::read_dir(parent)?.filter_map(|e| e.ok()).all(|e| {
        let name = e.file_name();
        name == album || is_junk(&name.to_string_lossy())
    });
    if sole {
        rename_via_temp(&grand.join(&on_disk), parent)?;
    }
    Ok(())
}

/// Rewrite every artist/album folder name into NFC.
///
/// A one-time repair for the 72 files whose folder name is decomposed because
/// they were last touched on macOS. Journalled like any other rename, so the
/// same server-side-move path carries the fix to B2.
pub fn normalize_tree_to_nfc(root: &Path, dry_run: bool) -> Result<Vec<(String, String)>> {
    let mut changes = Vec::new();

    let artists: Vec<PathBuf> = fs::read_dir(root)
        .with_context(|| format!("reading {}", root.display()))?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();

    for artist_path in artists {
        let Some(name) = artist_path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if name.starts_with('.') || name == "Playlists" {
            continue;
        }

        // Albums first: renaming the artist folder would invalidate these paths.
        let albums: Vec<PathBuf> = fs::read_dir(&artist_path)
            .map(|entries| {
                entries
                    .filter_map(|e| e.ok())
                    .map(|e| e.path())
                    .filter(|p| p.is_dir())
                    .collect()
            })
            .unwrap_or_default();

        for album_path in albums {
            if let Some(change) = plan_nfc_rename(root, &album_path) {
                if !dry_run {
                    apply_nfc_rename(root, &album_path, &change.1)?;
                }
                changes.push(change);
            }
        }

        if let Some(change) = plan_nfc_rename(root, &artist_path) {
            if !dry_run {
                apply_nfc_rename(root, &artist_path, &change.1)?;
            }
            changes.push(change);
        }
    }

    Ok(changes)
}

fn plan_nfc_rename(root: &Path, path: &Path) -> Option<(String, String)> {
    let name = path.file_name()?.to_str()?;
    let normalized = nfc(name);
    if normalized == name {
        return None;
    }
    let rel_from = path.strip_prefix(root).ok()?.to_str()?.to_string();
    let rel_to = path
        .parent()?
        .join(&normalized)
        .strip_prefix(root)
        .ok()?
        .to_str()?
        .to_string();
    Some((rel_from, rel_to))
}

fn apply_nfc_rename(root: &Path, path: &Path, rel_to: &str) -> Result<()> {
    let to_path = root.join(rel_to);
    let rel_from = path
        .strip_prefix(root)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default();

    if to_path.exists() && !is_same_dir(path, &to_path)? {
        bail!("{rel_to} already exists as a different directory");
    }
    if is_same_dir(path, &to_path).unwrap_or(false) {
        rename_via_temp(path, &to_path)?;
    } else {
        fs::rename(path, &to_path)
            .with_context(|| format!("normalizing {} to {rel_to}", path.display()))?;
    }

    journal::append(root, &rel_from, rel_to)?;
    Store::load(root).rekey(&rel_from, rel_to)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("hark-rename-test-{name}"));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        root
    }

    fn album(root: &Path, rel: &str) {
        let dir = root.join(rel);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("01 track.mp3"), b"audio").unwrap();
    }

    #[test]
    fn renames_the_folder_and_journals_it() {
        let root = fixture("basic");
        album(&root, "Bjork/Debut");

        let outcome = rename_album(&root, &root.join("Bjork/Debut"), "Bjork/Debut", "Björk", "Debut").unwrap();
        assert_eq!(
            outcome,
            Outcome::Renamed {
                from: "Bjork/Debut".into(),
                to: "Björk/Debut".into()
            }
        );
        assert!(root.join("Björk/Debut/01 track.mp3").is_file());
        assert!(!root.join("Bjork").exists(), "empty artist dir should be pruned");

        let entries = journal::read_all(&root);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].from, "Bjork/Debut");
        assert_eq!(entries[0].to, "Björk/Debut");
    }

    #[test]
    fn carries_the_perfect_flag_across_the_rename() {
        let root = fixture("flag");
        album(&root, "Bjork/Debut");
        Store::load(&root).set_perfect("Bjork/Debut", true).unwrap();

        rename_album(&root, &root.join("Bjork/Debut"), "Bjork/Debut", "Björk", "Debut").unwrap();

        let store = Store::load(&root);
        assert!(store.is_perfect("Björk/Debut"), "flag was orphaned by the rename");
        assert!(!store.is_perfect("Bjork/Debut"));
    }

    #[test]
    fn refuses_to_merge_into_a_different_album() {
        let root = fixture("collision");
        album(&root, "A/One");
        album(&root, "A/Two");

        let err = rename_album(&root, &root.join("A/One"), "A/One", "A", "Two").unwrap_err();
        assert!(
            err.to_string().contains("refusing to merge"),
            "unexpected error: {err}"
        );
        // Both must be intact.
        assert!(root.join("A/One/01 track.mp3").is_file());
        assert!(root.join("A/Two/01 track.mp3").is_file());
        assert!(journal::read_all(&root).is_empty(), "a refused rename must not journal");
    }

    #[test]
    fn a_no_op_rename_changes_nothing() {
        let root = fixture("noop");
        album(&root, "A/B");

        assert_eq!(rename_album(&root, &root.join("A/B"), "A/B", "A", "B").unwrap(), Outcome::Unchanged);
        assert!(journal::read_all(&root).is_empty());
    }

    #[test]
    fn sanitizes_illegal_characters_out_of_the_new_name() {
        let root = fixture("sanitize");
        album(&root, "A/B");

        rename_album(&root, &root.join("A/B"), "A/B", "AC/DC", "Live: 1978").unwrap();

        // The slash must not have created a nested directory.
        assert!(root.join("AC DC/Live_ 1978/01 track.mp3").is_file());
        assert!(!root.join("AC").exists());
    }

    #[test]
    fn case_only_rename_works() {
        let root = fixture("case");
        album(&root, "the beatles/revolver");

        rename_album(&root, &root.join("the beatles/revolver"), "the beatles/revolver", "The Beatles", "Revolver").unwrap();

        assert!(root.join("The Beatles/Revolver/01 track.mp3").is_file());
        let entries = journal::read_all(&root);
        assert_eq!(entries[0].to, "The Beatles/Revolver");
    }

    #[test]
    fn moving_to_a_new_artist_creates_the_parent() {
        let root = fixture("newartist");
        album(&root, "Unknown/Mystery Album");

        rename_album(&root, &root.join("Unknown/Mystery Album"), "Unknown/Mystery Album", "Agnes Obel", "Aventine").unwrap();

        assert!(root.join("Agnes Obel/Aventine/01 track.mp3").is_file());
        assert!(!root.join("Unknown").exists());
    }

    #[test]
    fn a_stray_ds_store_does_not_keep_the_old_artist_folder_alive() {
        // Regression: a leftover .DS_Store made the old (decomposed) spelling
        // of an artist folder survive the rename, leaving two Björks on disk —
        // reintroducing the exact split being repaired.
        let root = fixture("junk");
        let decomposed = "Bjo\u{0308}rk";
        album(&root, &format!("{decomposed}/Debut"));
        fs::write(root.join(decomposed).join(".DS_Store"), b"junk").unwrap();

        rename_album(
            &root,
            &root.join(decomposed).join("Debut"),
            "Bj\u{00f6}rk/Debut",
            "Bj\u{00f6}rk",
            "Debut",
        )
        .unwrap();

        let artists: Vec<String> = fs::read_dir(&root)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n != ".hark")
            .collect();
        assert_eq!(
            artists,
            vec!["Bj\u{00f6}rk".to_string()],
            "exactly one Björk should remain"
        );
    }

    #[test]
    fn an_artist_folder_holding_a_real_file_is_never_pruned() {
        // Only known litter justifies removing a directory; anything else and
        // we keep our hands off entirely.
        let root = fixture("keepreal");
        album(&root, "A/One");
        fs::write(root.join("A").join("notes.txt"), b"important").unwrap();

        rename_album(&root, &root.join("A/One"), "A/One", "B", "One").unwrap();

        assert!(root.join("A/notes.txt").is_file(), "a real file was destroyed");
    }

    #[test]
    fn an_artist_folder_with_other_albums_is_not_pruned() {
        let root = fixture("keepparent");
        album(&root, "A/One");
        album(&root, "A/Two");

        rename_album(&root, &root.join("A/One"), "A/One", "B", "One").unwrap();

        assert!(root.join("A/Two/01 track.mp3").is_file(), "sibling album was destroyed");
        assert!(root.join("B/One/01 track.mp3").is_file());
    }

    #[test]
    fn renaming_a_missing_album_is_an_error_not_a_panic() {
        let root = fixture("missing");
        assert!(rename_album(&root, &root.join("Nope/Nothing"), "Nope/Nothing", "A", "B").is_err());
    }

    #[test]
    fn nfc_normalization_pass_fixes_decomposed_folders() {
        let root = fixture("nfc");
        // Decomposed spelling, as macOS hands it back.
        album(&root, "Bjo\u{0308}rk/Debut");

        let planned = normalize_tree_to_nfc(&root, true).unwrap();
        assert_eq!(planned.len(), 1, "expected one folder to need normalizing");

        let applied = normalize_tree_to_nfc(&root, false).unwrap();
        assert_eq!(applied.len(), 1);
        assert_eq!(applied[0].1, "Bj\u{00f6}rk");

        // The precomposed path now exists.
        let names: Vec<String> = fs::read_dir(&root)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| !n.starts_with('.'))
            .collect();
        assert_eq!(names, vec!["Bj\u{00f6}rk".to_string()]);
        assert!(root.join("Bj\u{00f6}rk/Debut/01 track.mp3").is_file());
    }

    #[test]
    fn nfc_pass_is_a_no_op_on_an_already_normalized_tree() {
        let root = fixture("nfc-clean");
        album(&root, "Radiohead/Kid A");
        assert!(normalize_tree_to_nfc(&root, false).unwrap().is_empty());
        assert!(journal::read_all(&root).is_empty());
    }

    #[test]
    fn renames_a_folder_whose_name_is_stored_decomposed() {
        // Regression: the album's NFC identity ("Björk/Debut") and its real
        // path on disk ("Bjo◌̈rk/Debut") are different byte strings on Linux.
        // Rebuilding the path from the identity named something that didn't
        // exist, so the rename failed *after* the tags had already been
        // written — a half-applied save.
        let root = fixture("decomposed");
        let decomposed = "Bjo\u{0308}rk";
        album(&root, &format!("{decomposed}/Debut"));

        let real_dir = root.join(decomposed).join("Debut");
        assert!(real_dir.is_dir());
        // APFS resolves either spelling to the same folder, so there the
        // precondition cannot hold; the rename must still succeed.
        if !cfg!(target_os = "macos") {
            assert!(
                !root.join("Bj\u{00f6}rk/Debut").exists(),
                "precomposed path must not exist, or the test proves nothing"
            );
        }

        let outcome = rename_album(
            &root,
            &real_dir,
            "Bj\u{00f6}rk/Debut",
            "Bj\u{00f6}rk",
            "Debut Deluxe",
        )
        .unwrap();

        assert_eq!(
            outcome,
            Outcome::Renamed {
                from: "Bj\u{00f6}rk/Debut".into(),
                to: "Bj\u{00f6}rk/Debut Deluxe".into()
            }
        );
        assert!(root.join("Bj\u{00f6}rk/Debut Deluxe/01 track.mp3").is_file());
    }

    #[test]
    fn a_decomposed_folder_is_normalized_even_when_the_name_is_otherwise_unchanged() {
        // Saving an album with no text edits should still land the folder in
        // NFC, which is what stops the two-Björks split recurring.
        let root = fixture("decomposed-noop");
        let decomposed = "Bjo\u{0308}rk";
        album(&root, &format!("{decomposed}/Debut"));

        let outcome = rename_album(
            &root,
            &root.join(decomposed).join("Debut"),
            "Bj\u{00f6}rk/Debut",
            "Bj\u{00f6}rk",
            "Debut",
        )
        .unwrap();

        assert!(matches!(outcome, Outcome::Renamed { .. }));
        let names: Vec<String> = fs::read_dir(&root)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| !n.starts_with('.'))
            .collect();
        assert_eq!(names, vec!["Bj\u{00f6}rk".to_string()]);
    }

    #[test]
    fn planned_path_shows_what_the_rename_will_do() {
        assert_eq!(planned_path("Björk", "Debut"), "Björk/Debut");
        assert_eq!(planned_path("AC/DC", "Back in Black"), "AC DC/Back in Black");
    }
}

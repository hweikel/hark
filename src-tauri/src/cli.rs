//! Argument handling.
//!
//! `hark` with no arguments is the app — that is the overwhelmingly common
//! case and it must stay a single word typed into a launcher. The subcommands
//! are maintenance jobs you run deliberately.

use std::path::PathBuf;

use crate::library;

pub enum Invocation {
    /// Launch the window against this collection.
    Gui(PathBuf),
    Sync {
        root: PathBuf,
        check: bool,
        init: bool,
        remote: Option<String>,
    },
    /// Rewrite folder names into Unicode NFC.
    Normalize { root: PathBuf, check: bool },
    Help,
}

pub const HELP: &str = "\
hark! — a music player that respects perfect albums

usage:
  hark [PATH]                  open the collection (default: $HARK_MUSIC_DIR, else ~/music; ~/music-hark on macOS)
  hark sync [--check|--init]   reconcile the collection with the remote
  hark normalize [--check]     rewrite folder names into Unicode NFC
  hark --help

sync:
  --check      show what would happen, change nothing
  --init       establish the bisync baseline (required once, before the first sync)
  --remote R   override the rclone remote (default: $HARK_REMOTE)

normalize:
  Repairs folders whose names are stored decomposed — typically anything last
  touched on macOS, where `Björk` is written as `o` plus a combining umlaut.
  Left alone, the same artist exists twice as far as the filesystem and B2 are
  concerned. Journalled like any other rename, so `hark sync` carries the fix
  to the remote as a server-side move.
";

pub fn parse<I: IntoIterator<Item = String>>(args: I) -> Invocation {
    let args: Vec<String> = args.into_iter().collect();
    let first = args.first().map(String::as_str);

    match first {
        Some("--help") | Some("-h") | Some("help") => Invocation::Help,

        Some("sync") => {
            let rest = &args[1..];
            Invocation::Sync {
                root: resolve(path_argument(rest)),
                check: has_flag(rest, "--check") || has_flag(rest, "--dry-run"),
                init: has_flag(rest, "--init") || has_flag(rest, "--resync"),
                remote: value_of(rest, "--remote"),
            }
        }

        Some("normalize") => {
            let rest = &args[1..];
            Invocation::Normalize {
                root: resolve(path_argument(rest)),
                check: has_flag(rest, "--check") || has_flag(rest, "--dry-run"),
            }
        }

        _ => Invocation::Gui(resolve(first.map(str::to_string))),
    }
}

fn resolve(explicit: Option<String>) -> PathBuf {
    library::resolve_music_dir(explicit.as_deref())
        .unwrap_or_else(|| PathBuf::from("music"))
}

fn has_flag(args: &[String], flag: &str) -> bool {
    args.iter().any(|a| a == flag)
}

fn value_of(args: &[String], flag: &str) -> Option<String> {
    let index = args.iter().position(|a| a == flag)?;
    args.get(index + 1).cloned()
}

/// The first bare argument, i.e. not a flag and not a flag's value.
fn path_argument(args: &[String]) -> Option<String> {
    let mut skip_next = false;
    for arg in args {
        if skip_next {
            skip_next = false;
            continue;
        }
        if arg == "--remote" {
            skip_next = true;
            continue;
        }
        if !arg.starts_with('-') {
            return Some(arg.clone());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn no_arguments_opens_the_app() {
        assert!(matches!(parse(args(&[])), Invocation::Gui(_)));
    }

    #[test]
    fn a_bare_path_opens_that_collection() {
        match parse(args(&["/mnt/t7/music"])) {
            Invocation::Gui(path) => assert_eq!(path, PathBuf::from("/mnt/t7/music")),
            other => panic!("expected Gui, got {}", describe(&other)),
        }
    }

    #[test]
    fn sync_defaults_to_doing_the_real_thing() {
        match parse(args(&["sync"])) {
            Invocation::Sync { check, init, .. } => {
                assert!(!check);
                assert!(!init);
            }
            other => panic!("expected Sync, got {}", describe(&other)),
        }
    }

    #[test]
    fn sync_check_and_init_are_recognised() {
        match parse(args(&["sync", "--check"])) {
            Invocation::Sync { check, .. } => assert!(check),
            other => panic!("expected Sync, got {}", describe(&other)),
        }
        match parse(args(&["sync", "--init"])) {
            Invocation::Sync { init, .. } => assert!(init),
            other => panic!("expected Sync, got {}", describe(&other)),
        }
    }

    #[test]
    fn sync_takes_a_remote_override() {
        match parse(args(&["sync", "--remote", "other:bucket/music"])) {
            Invocation::Sync { remote, .. } => {
                assert_eq!(remote.as_deref(), Some("other:bucket/music"))
            }
            other => panic!("expected Sync, got {}", describe(&other)),
        }
    }

    #[test]
    fn a_remote_value_is_not_mistaken_for_the_collection_path() {
        match parse(args(&["sync", "--remote", "other:bucket/music", "/mnt/t7/music"])) {
            Invocation::Sync { root, remote, .. } => {
                assert_eq!(root, PathBuf::from("/mnt/t7/music"));
                assert_eq!(remote.as_deref(), Some("other:bucket/music"));
            }
            other => panic!("expected Sync, got {}", describe(&other)),
        }
    }

    #[test]
    fn normalize_is_recognised_with_and_without_check() {
        assert!(matches!(
            parse(args(&["normalize"])),
            Invocation::Normalize { check: false, .. }
        ));
        assert!(matches!(
            parse(args(&["normalize", "--check"])),
            Invocation::Normalize { check: true, .. }
        ));
    }

    #[test]
    fn help_is_recognised() {
        assert!(matches!(parse(args(&["--help"])), Invocation::Help));
        assert!(matches!(parse(args(&["-h"])), Invocation::Help));
    }

    fn describe(invocation: &Invocation) -> &'static str {
        match invocation {
            Invocation::Gui(_) => "Gui",
            Invocation::Sync { .. } => "Sync",
            Invocation::Normalize { .. } => "Normalize",
            Invocation::Help => "Help",
        }
    }
}

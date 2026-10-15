pub mod app;
pub mod audio;
pub mod bisync;
pub mod cli;
pub mod library;
pub mod sync;
// Self-update exists for the mac, which installs releases instead of building
// them. The Linux build comes from the flake and lives in the read-only Nix
// store, so it must not try to overwrite itself.
#[cfg(target_os = "macos")]
pub mod update;

use std::path::PathBuf;

use tauri::{Emitter, Manager};

/// Entry point. Subcommands run and exit; anything else opens the window.
pub fn run() {
    match cli::parse(std::env::args().skip(1)) {
        cli::Invocation::Help => println!("{}", cli::HELP),

        cli::Invocation::Gui(root) => run_gui(root),

        cli::Invocation::Sync {
            root,
            check,
            init,
            force,
            remote,
        } => {
            let options = sync::Options::new(check, init, force, remote);
            if let Err(e) = sync::run(&root, &options) {
                eprintln!("hark sync: {e:#}");
                std::process::exit(1);
            }
        }

        cli::Invocation::Normalize { root, check } => {
            match library::rename::normalize_tree_to_nfc(&root, check) {
                Ok(changes) if changes.is_empty() => {
                    println!("Every folder name is already NFC. Nothing to do.");
                }
                Ok(changes) => {
                    println!(
                        "{} folder{} {} normalized:",
                        changes.len(),
                        if changes.len() == 1 { "" } else { "s" },
                        if check { "would be" } else { "were" }
                    );
                    for (from, to) in &changes {
                        println!("  {from} → {to}");
                    }
                    if check {
                        println!("\nRun without --check to apply.");
                    } else {
                        println!("\nRecorded in .hark/renames.jsonl; run `hark sync` to carry this to the remote.");
                    }
                }
                Err(e) => {
                    eprintln!("hark normalize: {e:#}");
                    std::process::exit(1);
                }
            }
        }
    }
}

fn run_gui(root: PathBuf) {
    let builder = tauri::Builder::default();

    #[cfg(target_os = "macos")]
    let builder = builder.plugin(tauri_plugin_updater::Builder::new().build());

    builder
        .register_asynchronous_uri_scheme_protocol("hark", |ctx, request, responder| {
            app::handle_art_request(ctx.app_handle(), request, responder);
        })
        .setup(move |handle| {
            let app_handle = handle.handle().clone();

            #[cfg(target_os = "macos")]
            update::check_on_launch(handle.handle());

            // The audio thread pushes state changes out as events rather than
            // the frontend polling — track changes originate down here when a
            // song ends, not from anything the UI did.
            let player = audio::Player::spawn(move |state| {
                let _ = app_handle.emit("playback", state);
            });
            handle.manage(app::AppState::new(root.clone(), player));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            app::get_library,
            app::rescan,
            app::set_perfect,
            app::play_tile,
            app::playback_toggle,
            app::playback_next,
            app::playback_previous,
            app::playback_play_index,
            app::playback_set_shuffle,
            app::playback_state,
            app::playlist_add,
            app::playlist_names,
            app::save_playlist,
            app::save_album,
            app::planned_rename,
        ])
        .run(tauri::generate_context!())
        .expect("error while running hark!");
}

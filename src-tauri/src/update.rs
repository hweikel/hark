//! Self-update from GitHub Releases. macOS only.
//!
//! The mac mini does not build hark! — it installs what the release workflow
//! published, and this is how it learns there is something newer. The Linux
//! side is installed from the flake, into a read-only Nix store path, so the
//! whole module is `#[cfg(target_os = "macos")]` at the `lib.rs` end: see
//! `RELEASING.md`.
//!
//! The signature on the downloaded bundle is minisign, verified against the
//! `pubkey` in `tauri.conf.json`, and has nothing to do with Apple
//! codesigning — which is why this works without a Developer ID.

use tauri::AppHandle;
use tauri_plugin_updater::UpdaterExt;

/// Check once, at launch, and install anything newer.
///
/// Returns immediately: the check happens on the async runtime so a slow or
/// unreachable GitHub cannot delay the window, and a failure is reported on
/// stderr rather than propagated. An update that cannot be fetched is not a
/// reason to stop playing music.
pub fn check_on_launch(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        if let Err(e) = check(&app).await {
            eprintln!("hark: update check failed: {e}");
        }
    });
}

async fn check(app: &AppHandle) -> tauri_plugin_updater::Result<()> {
    let Some(update) = app.updater()?.check().await? else {
        return Ok(());
    };

    eprintln!(
        "hark: updating {} -> {}",
        update.current_version, update.version
    );
    update.download_and_install(|_, _| {}, || {}).await?;

    // The new bundle is on disk, but this process is still the old binary —
    // on macOS the updater replaces the .app and leaves relaunching to us.
    eprintln!("hark: restarting into {}", update.version);
    app.restart();
}

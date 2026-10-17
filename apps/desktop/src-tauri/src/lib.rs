//! stockwatcher desktop: a Tauri v2 shell around the web app (`apps/web`).
//!
//! Everything here is plumbing with no UI of its own. The window shows the
//! web build; the plugins below are registered so the UI can use them later.
//! The webview is granted only `core:default` (see `capabilities/`): the
//! updater runs entirely in Rust, so no plugin is exposed to JavaScript.

mod updater;

use tauri::Manager;
use tauri_plugin_log::{RotationStrategy, Target, TargetKind};

/// Rotating support log in the OS log directory
/// (`%LOCALAPPDATA%\io.wavystyle.stockwatcher\logs` on Windows).
const LOG_MAX_BYTES: u128 = 5 * 1024 * 1024;
const LOG_FILES_KEPT: usize = 5;

pub fn run() {
    tauri::Builder::default()
        // Registered first: a second launch forwards to this process and
        // exits before any other plugin (or the window-state restore) runs.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.unminimize();
                let _ = window.show();
                let _ = window.set_focus();
            }
        }))
        .plugin(
            tauri_plugin_log::Builder::new()
                .targets([
                    Target::new(TargetKind::Stdout),
                    Target::new(TargetKind::LogDir { file_name: None }),
                ])
                .level(log::LevelFilter::Info)
                .max_file_size(LOG_MAX_BYTES)
                .rotation_strategy(RotationStrategy::KeepSome(LOG_FILES_KEPT))
                .build(),
        )
        .plugin(tauri_plugin_window_state::Builder::default().build())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_os::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            log::info!(
                "stockwatcher {} starting ({} {})",
                app.package_info().version,
                std::env::consts::OS,
                std::env::consts::ARCH
            );
            updater::spawn(app.handle().clone());
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("failed to start the stockwatcher desktop app");
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    // Guards the values other systems depend on: the VPS serves updates at
    // this URL, installed apps trust only this key, and the release job
    // rewrites only `version`.
    #[test]
    fn tauri_conf_matches_the_release_pipeline() {
        let conf: Value = serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        assert_eq!(conf["identifier"], "io.wavystyle.stockwatcher");
        assert_eq!(conf["productName"], "stockwatcher");
        let version = conf["version"].as_str().unwrap();
        assert!(version.starts_with("0.1."), "version {version}");

        let updater = &conf["plugins"]["updater"];
        assert_eq!(
            updater["endpoints"],
            json!(["https://stockwatcher.wavystyle.io/updates/desktop/latest.json"])
        );
        assert!(updater["pubkey"].as_str().is_some_and(|k| k.len() > 100));
        assert_eq!(updater["requireSignedVersion"], true);

        let bundle = &conf["bundle"];
        assert_eq!(bundle["createUpdaterArtifacts"], true);
        assert_eq!(bundle["targets"], json!(["nsis", "msi"]));
    }

    #[test]
    fn webview_gets_core_permissions_only() {
        let cap: Value = serde_json::from_str(include_str!("../capabilities/main.json")).unwrap();
        assert_eq!(cap["permissions"], json!(["core:default"]));
        assert_eq!(cap["windows"], json!(["main"]));
        assert!(cap.get("remote").is_none(), "no remote origin may use IPC");
    }
}

//! Background self-update, with no UI of its own.
//!
//! On start and every 30 minutes: check the endpoint in `tauri.conf.json`,
//! download a newer release in the background, and verify its minisign
//! signature (`Update::download` refuses a bad signature and, because
//! `requireSignedVersion` is on, a signature made for a different version).
//! Then emit [`READY_EVENT`] for a future in-app banner and ask "Restart to
//! update?" in a native dialog. Nothing installs or relaunches without that
//! consent. "Later" means this version is not offered again in this
//! session; the next launch offers it again.
//!
//! Debug builds skip all of this: `tauri dev` runs the committed 0.1.0,
//! which every published release would replace.

use std::collections::HashSet;
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Runtime};
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};
use tauri_plugin_updater::UpdaterExt;

pub const CHECK_INTERVAL: Duration = Duration::from_secs(30 * 60);
/// Emitted to the webview once an update is downloaded and verified.
pub const READY_EVENT: &str = "stockwatcher://update-ready";

type BoxError = Box<dyn std::error::Error + Send + Sync>;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct UpdateReady {
    version: String,
    current_version: String,
    notes: Option<String>,
}

/// Versions the user said "Later" to in this session.
#[derive(Debug, Default)]
struct Declined(HashSet<String>);

impl Declined {
    fn should_offer(&self, version: &str) -> bool {
        !self.0.contains(version)
    }

    fn decline(&mut self, version: &str) {
        self.0.insert(version.to_owned());
    }
}

pub fn spawn<R: Runtime>(app: AppHandle<R>) {
    if cfg!(debug_assertions) {
        log::info!("updater: disabled in debug builds");
        return;
    }
    tauri::async_runtime::spawn(async move {
        let mut declined = Declined::default();
        loop {
            if let Err(err) = check_once(&app, &mut declined).await {
                // Offline, endpoint down, bad signature: retried next cycle.
                log::warn!("updater: {err}");
            }
            tokio::time::sleep(CHECK_INTERVAL).await;
        }
    });
}

async fn check_once<R: Runtime>(
    app: &AppHandle<R>,
    declined: &mut Declined,
) -> Result<(), BoxError> {
    let Some(update) = app.updater()?.check().await? else {
        log::info!("updater: up to date");
        return Ok(());
    };
    if !declined.should_offer(&update.version) {
        return Ok(());
    }

    log::info!(
        "updater: downloading {} (running {})",
        update.version,
        update.current_version
    );
    let bytes = update.download(|_, _| {}, || {}).await?;
    log::info!("updater: {} downloaded, signature verified", update.version);

    app.emit(
        READY_EVENT,
        UpdateReady {
            version: update.version.clone(),
            current_version: update.current_version.clone(),
            notes: update.body.clone(),
        },
    )?;

    if ask_to_restart(app, &update.version).await {
        log::info!("updater: installing {}", update.version);
        // Windows: starts the installer and exits this process; the
        // installer relaunches the app. Elsewhere the restart below does.
        update.install(bytes)?;
        app.restart();
    }
    log::info!("updater: {} postponed by the user", update.version);
    declined.decline(&update.version);
    Ok(())
}

/// The only consent UI: a native dialog, so nothing is added to the web app.
async fn ask_to_restart<R: Runtime>(app: &AppHandle<R>, version: &str) -> bool {
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog()
        .message(format!(
            "stockwatcher {version} is downloaded and verified.\n\nRestart now to update?"
        ))
        .title("Update ready")
        .kind(MessageDialogKind::Info)
        .buttons(MessageDialogButtons::OkCancelCustom(
            "Restart to update".into(),
            "Later".into(),
        ))
        .show(move |restart| {
            let _ = tx.send(restart);
        });
    rx.await.unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_declined_version_is_not_offered_again() {
        let mut declined = Declined::default();
        assert!(declined.should_offer("0.1.7"));
        declined.decline("0.1.7");
        assert!(!declined.should_offer("0.1.7"));
        assert!(declined.should_offer("0.1.8"), "a newer release is offered");
    }

    #[test]
    fn checks_every_thirty_minutes() {
        assert_eq!(CHECK_INTERVAL.as_secs(), 1800);
    }
}

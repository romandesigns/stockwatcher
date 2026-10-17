//! Background self-update: fully automatic, no UI and no prompt.
//!
//! Owner decision (2026-10-16): every push to `main` must reach installed
//! desktop apps without anyone clicking anything. So on start and every
//! [`CHECK_INTERVAL`]: check the endpoint in `tauri.conf.json`, download a
//! newer release, verify its minisign signature (`Update::download`
//! refuses a bad signature and, because `requireSignedVersion` is on, a
//! signature made for a different version), then install it straight away.
//! On Windows `install` starts the installer in `passive` mode (a small
//! progress bar, no input) and exits this process; the installer relaunches
//! the app. Elsewhere the `restart` below does it.
//!
//! [`READY_EVENT`] is emitted just before installing so a future UI can
//! save state first. Debug builds skip all of this: `tauri dev` runs the
//! committed 0.1.0, which every published release would replace.

use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Runtime};
use tauri_plugin_updater::UpdaterExt;

/// latest.json is a few hundred bytes served with no-cache, so checking
/// often costs nothing and keeps "push to main" to "updated" at a few
/// minutes.
pub const CHECK_INTERVAL: Duration = Duration::from_secs(2 * 60);
/// Emitted to the webview just before an update installs.
pub const READY_EVENT: &str = "stockwatcher://update-installing";

type BoxError = Box<dyn std::error::Error + Send + Sync>;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct UpdateInstalling {
    version: String,
    current_version: String,
    notes: Option<String>,
}

pub fn spawn<R: Runtime>(app: AppHandle<R>) {
    if cfg!(debug_assertions) {
        log::info!("updater: disabled in debug builds");
        return;
    }
    tauri::async_runtime::spawn(async move {
        loop {
            if let Err(err) = check_once(&app).await {
                // Offline, endpoint down, bad signature: retried next cycle.
                log::warn!("updater: {err}");
            }
            tokio::time::sleep(CHECK_INTERVAL).await;
        }
    });
}

async fn check_once<R: Runtime>(app: &AppHandle<R>) -> Result<(), BoxError> {
    let Some(update) = app.updater()?.check().await? else {
        log::debug!("updater: up to date");
        return Ok(());
    };

    log::info!(
        "updater: downloading {} (running {})",
        update.version,
        update.current_version
    );
    let bytes = update.download(|_, _| {}, || {}).await?;
    log::info!(
        "updater: {} downloaded, signature verified, installing",
        update.version
    );

    // Best effort: a missing listener must not block the update.
    let _ = app.emit(
        READY_EVENT,
        UpdateInstalling {
            version: update.version.clone(),
            current_version: update.current_version.clone(),
            notes: update.body.clone(),
        },
    );

    update.install(bytes)?;
    app.restart();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checks_every_two_minutes() {
        assert_eq!(CHECK_INTERVAL.as_secs(), 120);
    }
}

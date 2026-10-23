//! Runners system notifications (owner, 2026-10-23: the phone's Runners notifications on the
//! desktop too, and a click opens that ticker's chart).
//!
//! The web app decides what to show (apps/web `notify/runnersNotify.ts`: the server's
//! `runners.notice`, this device's switches, once per id) and hands each one to
//! [`runners_notify`]. That is the only thing the webview can ask for: the notification plugin's
//! own JavaScript commands stay ungranted (`capabilities/main.json` is still `core:default`), so a
//! page cannot post arbitrary notifications, only a Runners notice with a valid ticker.
//!
//! **Click.** The plugin reports a click on a notification (`tap`) to [`listen`]'s handler, which
//! restores and focuses the window (it may be minimized or behind others) and runs the page's
//! `window.__stockwatcherOpenChart(symbol)`: the dashboard's "load into the selected chart",
//! the same action as clicking a ticker on the dashboard.
//!
//! **Windows limits** (tauri-plugin-notification 2.7, notify-rust / WinRT toasts): a click is
//! reported only while the toast is on screen; once it moved to the Action Center a click opens
//! nothing in the app. In a dev build (`target\debug`) the toast carries PowerShell's identity and
//! icon, because only the installed app has the AppUserModelID (`io.wavystyle.stockwatcher`) that
//! gives it the stockwatcher icon and name.

use std::sync::Mutex;

use serde_json::Value;
use tauri::{AppHandle, Manager, Runtime};
use tauri_plugin_notification::NotificationExt;

/// Toast text beyond this is cut (the server's lines are well under it).
const MAX_TITLE: usize = 120;
const MAX_BODY: usize = 300;
/// Notifications whose click is still awaited, by notification id (the oldest go first).
const KEEP: usize = 200;

/// The ticker of each shown notification, by its id: the click handler's fallback when the
/// platform does not hand the notification's extras back.
#[derive(Default)]
pub struct Shown(Mutex<Vec<(i32, String)>>);

/// A ticker as the wire spells it (the phone's `route.ts` rule): `^[A-Z][A-Z0-9.-]{0,11}$`.
pub fn valid_symbol(s: &str) -> bool {
    let mut chars = s.chars();
    s.len() <= 12
        && chars.next().is_some_and(|c| c.is_ascii_uppercase())
        && chars.all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '.' || c == '-')
}

fn clip(s: &str, max: usize) -> String {
    s.chars().filter(|c| !c.is_control()).take(max).collect()
}

/// The script a click runs in the page: open `symbol`'s chart (None for an invalid ticker, so
/// nothing from a notification is ever spliced into script text unchecked).
pub fn open_chart_script(symbol: &str) -> Option<String> {
    valid_symbol(symbol).then(|| {
        format!(
            "window.__stockwatcherOpenChart && window.__stockwatcherOpenChart({})",
            Value::from(symbol)
        )
    })
}

/// A stable notification id from the notice id (the plugin wants an i32).
fn notification_id(id: &str) -> i32 {
    // FNV-1a, folded into the positive i32 range.
    let mut h: u32 = 0x811c_9dc5;
    for b in id.bytes() {
        h ^= u32::from(b);
        h = h.wrapping_mul(0x0100_0193);
    }
    (h & 0x7fff_ffff) as i32
}

/// Shows one Runners notice as a system notification. Called by the web app only.
#[tauri::command]
pub fn runners_notify<R: Runtime>(
    app: AppHandle<R>,
    shown: tauri::State<'_, Shown>,
    id: String,
    title: String,
    body: String,
    symbol: String,
) -> Result<(), String> {
    if !valid_symbol(&symbol) {
        return Err("not a ticker".into());
    }
    let nid = notification_id(&id);
    {
        let mut list = shown.0.lock().unwrap_or_else(|e| e.into_inner());
        list.retain(|(n, _)| *n != nid);
        list.push((nid, symbol.clone()));
        let over = list.len().saturating_sub(KEEP);
        list.drain(..over);
    }
    app.notification()
        .builder()
        .id(nid)
        .title(clip(&title, MAX_TITLE))
        .body(clip(&body, MAX_BODY))
        .extra("symbol", &symbol)
        .show()
        .map_err(|e| {
            log::warn!("runners notification not shown: {e}");
            e.to_string()
        })
}

/// The ticker a performed action is about: the notification's `symbol` extra, else the one
/// recorded for its id.
fn symbol_of(p: &tauri_plugin_notification::ActionPerformed, shown: &Shown) -> Option<String> {
    let n = p.notification()?;
    if let Some(s) = n.extra().get("symbol").and_then(Value::as_str) {
        return Some(s.to_owned());
    }
    let list = shown.0.lock().unwrap_or_else(|e| e.into_inner());
    list.iter()
        .rev()
        .find(|(id, _)| *id == n.id())
        .map(|(_, s)| s.clone())
}

/// Registers the click handler: restore and focus the window, then open the ticker's chart.
pub fn listen<R: Runtime>(app: &AppHandle<R>) {
    app.manage(Shown::default());
    let handle = app.clone();
    let res = app.notification().on_action(move |p| {
        let Some(window) = handle.get_webview_window("main") else {
            return;
        };
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
        let shown = handle.state::<Shown>();
        match symbol_of(p, &shown).as_deref().and_then(open_chart_script) {
            Some(script) => {
                if let Err(e) = window.eval(script) {
                    log::warn!("runners notification click: chart not opened: {e}");
                }
            }
            None => log::info!("runners notification click without a ticker"),
        }
    });
    if let Err(e) = res {
        log::warn!("runners notifications: clicks will not open charts: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_tickers_reach_the_page() {
        assert!(valid_symbol("CJMB"));
        assert!(valid_symbol("BRK.B"));
        assert!(valid_symbol("A"));
        assert!(!valid_symbol(""));
        assert!(!valid_symbol("cjmb"));
        assert!(!valid_symbol("1ABC"));
        assert!(!valid_symbol("ABCDEFGHIJKLM"));
        assert!(!valid_symbol("A\");alert(1);//"));
        assert_eq!(
            open_chart_script("CJMB").as_deref(),
            Some("window.__stockwatcherOpenChart && window.__stockwatcherOpenChart(\"CJMB\")")
        );
        assert_eq!(open_chart_script("X'); evil()"), None);
    }

    #[test]
    fn ids_are_stable_and_positive() {
        let a = notification_id("runners-new-2026-10-23-CJMB-7");
        assert_eq!(a, notification_id("runners-new-2026-10-23-CJMB-7"));
        assert_ne!(a, notification_id("runners-breakout-2026-10-23-CJMB-7"));
        assert!(a >= 0);
    }

    #[test]
    fn text_is_clipped_and_cleaned() {
        assert_eq!(clip("a\nb\u{7}c", 10), "abc");
        assert_eq!(clip("CJMB · Fast move · $2.41", 4), "CJMB");
    }
}

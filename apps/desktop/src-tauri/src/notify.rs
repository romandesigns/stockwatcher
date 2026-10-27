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
//! **Copy** (owner, 2026-10-23: a YMAT limit never filled while the ticker was being typed into
//! the broker). Every Runners toast has a "Copy" button ([`COPY_ACTION`], an action type the
//! plugin turns into a toast button on Windows) that puts the ticker on the clipboard from Rust
//! (tauri-plugin-clipboard-manager, not exposed to the page) and leaves the window where it is,
//! so the next window brought forward is the broker's. The body keeps its line break: the
//! server puts the "Alert $2.41 · +5% = $2.53" reference on a line of its own.
//!
//! **Windows limits** (tauri-plugin-notification 2.7, notify-rust / WinRT toasts): a click is
//! reported only while the toast is on screen; once it moved to the Action Center a click opens
//! nothing in the app, and the "Copy" button copies nothing either. In a dev build (`target\debug`) the toast carries PowerShell's identity and
//! icon, because only the installed app has the AppUserModelID (`io.wavystyle.stockwatcher`) that
//! gives it the stockwatcher icon and name.

use std::sync::Mutex;

use serde_json::Value;
use tauri::{AppHandle, Manager, Runtime};
use tauri_plugin_clipboard_manager::ClipboardExt;
use tauri_plugin_notification::{Action, ActionType, NotificationExt};

/// The action type every Runners toast carries, and its "Copy" button's action id.
const ACTION_TYPE: &str = "runners";
pub const COPY_ACTION: &str = "copy";

/// Toast text beyond this is cut (the server's lines are well under it).
const MAX_TITLE: usize = 120;
const MAX_BODY: usize = 300;
/// Notifications whose click is still awaited, by notification id (the oldest go first).
const KEEP: usize = 200;
const MAX_CONTEXT_BYTES: usize = 16 * 1024;

/// The ticker of each shown notification, by its id: the click handler's fallback when the
/// platform does not hand the notification's extras back.
#[derive(Default)]
pub struct Shown(Mutex<Vec<(i32, String, Option<Value>)>>);

/// A ticker as the wire spells it (the phone's `route.ts` rule): `^[A-Z][A-Z0-9.-]{0,11}$`.
pub fn valid_symbol(s: &str) -> bool {
    let mut chars = s.chars();
    s.len() <= 12
        && chars.next().is_some_and(|c| c.is_ascii_uppercase())
        && chars.all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '.' || c == '-')
}

/// `s` without control characters but its line breaks, cut at `max`.
fn clip(s: &str, max: usize) -> String {
    s.chars()
        .filter(|c| *c == '\n' || !c.is_control())
        .take(max)
        .collect()
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

/// Optional event evidence is encoded as JSON, never executable script text. The page validates
/// its geometry and symbol again; older notices retain the ticker-only route.
fn open_chart_script_with_context(symbol: &str, context: Option<&Value>) -> Option<String> {
    let Some(context) = context.filter(|c| valid_context(symbol, c)) else {
        return open_chart_script(symbol);
    };
    valid_symbol(symbol).then(|| {
        format!(
            "window.__stockwatcherOpenChart && window.__stockwatcherOpenChart({}, {})",
            Value::from(symbol),
            context
        )
    })
}

fn valid_context(symbol: &str, context: &Value) -> bool {
    context.is_object()
        && context["symbol"].as_str() == Some(symbol)
        && context["event_id"]
            .as_str()
            .is_some_and(|id| !id.is_empty() && id.len() <= 256)
        && context.to_string().len() <= MAX_CONTEXT_BYTES
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
    notice_context: Option<Value>,
) -> Result<(), String> {
    if !valid_symbol(&symbol) {
        return Err("not a ticker".into());
    }
    let nid = notification_id(&id);
    let context = notice_context.filter(|c| valid_context(&symbol, c));
    {
        let mut list = shown.0.lock().unwrap_or_else(|e| e.into_inner());
        list.retain(|(n, _, _)| *n != nid);
        list.push((nid, symbol.clone(), context.clone()));
        let over = list.len().saturating_sub(KEEP);
        list.drain(..over);
    }
    let mut notification = app
        .notification()
        .builder()
        .id(nid)
        .title(clip(&title, MAX_TITLE))
        .body(clip(&body, MAX_BODY))
        .extra("symbol", &symbol)
        .action_type_id(ACTION_TYPE);
    if let Some(context) = context {
        notification = notification.extra("noticeContext", context);
    }
    notification.show().map_err(|e| {
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
        .find(|(id, _, _)| *id == n.id())
        .map(|(_, s, _)| s.clone())
}

fn context_of(p: &tauri_plugin_notification::ActionPerformed, shown: &Shown) -> Option<Value> {
    let n = p.notification()?;
    n.extra()
        .get("noticeContext")
        .filter(|c| c.is_object())
        .cloned()
        .or_else(|| {
            let list = shown.0.lock().unwrap_or_else(|e| e.into_inner());
            list.iter()
                .rev()
                .find(|(id, _, _)| *id == n.id())
                .and_then(|(_, _, c)| c.clone())
        })
}

/// What a performed action does: the "Copy" button copies the ticker; anything else (a click on
/// the toast itself, `tap`) opens its chart.
#[derive(Debug, PartialEq, Eq)]
enum Act {
    Copy,
    Open,
}

fn act(action_id: &str) -> Act {
    if action_id == COPY_ACTION {
        Act::Copy
    } else {
        Act::Open
    }
}

/// Registers the "Copy" button and the click handler: "Copy" puts the ticker on the clipboard and
/// leaves the window alone; a click restores and focuses the window, then opens the ticker's
/// chart.
pub fn listen<R: Runtime>(app: &AppHandle<R>) {
    app.manage(Shown::default());
    let copy = ActionType::builder(ACTION_TYPE)
        .actions(vec![Action::builder(COPY_ACTION, "Copy").build()])
        .build();
    if let Err(e) = app.notification().register_action_types(vec![copy]) {
        log::warn!("runners notifications: no Copy button: {e}");
    }
    let handle = app.clone();
    let res = app.notification().on_action(move |p| {
        if act(p.action_id()) == Act::Copy {
            let shown = handle.state::<Shown>();
            // Only a ticker reaches the clipboard (the same rule as the chart's).
            match symbol_of(p, &shown).filter(|s| valid_symbol(s)) {
                Some(s) => {
                    if let Err(e) = handle.clipboard().write_text(s) {
                        log::warn!("runners notification copy failed: {e}");
                    }
                }
                None => log::info!("runners notification copy without a ticker"),
            }
            return;
        }
        let Some(window) = handle.get_webview_window("main") else {
            return;
        };
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
        let shown = handle.state::<Shown>();
        let context = context_of(p, &shown);
        match symbol_of(p, &shown)
            .as_deref()
            .and_then(|symbol| open_chart_script_with_context(symbol, context.as_ref()))
        {
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
    fn optional_context_is_json_encoded_bounded_and_bound_to_the_ticker() {
        let context = serde_json::json!({"event_id":"runners-breakout-1","symbol":"CJMB","breakout":{"rule":"x\");evil();//","bar_end_ms":1000}});
        let script = open_chart_script_with_context("CJMB", Some(&context)).unwrap();
        assert!(script.ends_with(&format!(", {context})")));
        assert!(script.contains("\\\""));
        assert_eq!(
            open_chart_script_with_context("INVALID!", Some(&context)),
            None
        );
        assert_eq!(
            open_chart_script_with_context("TDIC", Some(&context)),
            open_chart_script("TDIC")
        );
        let oversize = serde_json::json!({"event_id":"a","symbol":"CJMB","junk":"x".repeat(MAX_CONTEXT_BYTES)});
        assert_eq!(
            open_chart_script_with_context("CJMB", Some(&oversize)),
            open_chart_script("CJMB")
        );
        assert_eq!(
            open_chart_script_with_context("CJMB", None),
            open_chart_script("CJMB")
        );
    }

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
    fn the_copy_button_copies_and_a_click_opens() {
        assert_eq!(act(COPY_ACTION), Act::Copy);
        assert_eq!(act("tap"), Act::Open);
        assert_eq!(act(""), Act::Open);
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
        // The line break before the reference line survives; other control characters do not.
        assert_eq!(clip("a\nb\u{7}c\r", 10), "a\nbc");
        assert_eq!(clip("CJMB · Fast move · $2.41", 4), "CJMB");
    }
}

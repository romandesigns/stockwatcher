//! The runtime detector switches (DEPLOY-PLAN 2026-11-21, "Kill rules": switch a shipped detector
//! off at once, during market hours, with no restart; a restart drops live prints, the 2026-10-30
//! incident). One synced settings document, `detectors`, keyed by detector id:
//!
//! ```json
//! { "x-float-1": { "on": false } }
//! ```
//!
//! - **Default on.** A detector with no entry, or an entry without a boolean `on` (or a bare
//!   boolean), is on: shipping an item turns it on, the switch exists to turn it off.
//! - **Read live.** The alerts task re-reads the document on every `settings.changed` frame for
//!   it ([`NS`]), the same path as `alerts` and `alerts.prefs`, so a flip applies to the next
//!   notice. Clients write it with an entry `patch` (one key per detector), so two items flipped
//!   from two devices never erase each other, and it is NOT inside `alerts.prefs`, which clients
//!   write as a whole value.
//! - **Held, never dropped.** A switched-off detector keeps running; its notice still reaches
//!   [`super::AlertsTask::runner`], where both paths (desk and phone) are decided
//!   `held: switched_off` and ledgered with the full text. So a kill keeps the would-be alerts
//!   for the daily checklist, and a replaced kind can run "silently" by the same mechanism.
//! - **Per-device switches are separate.** `DeskKinds` / the phone's switches choose what one
//!   device shows; this one decides, for everybody, whether the detector sends at all.
//!
//! Which notice kind belongs to which detector is [`detector_of`]: only the research-v2 items
//! shipped behind a switch have an id. Every other kind answers `on` always.

use std::collections::HashSet;

use serde_json::Value;

use crate::rocket::NoticeKind;

/// The settings namespace (client-writable: `super::model::server_owned` is false for it).
pub const NS: &str = "detectors";

/// A shipped detector: its id (the finalist's, as the research and the ledger name it) and its
/// version (the frozen config's sha256 prefix, or the code commit).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Detector {
    pub id: &'static str,
    pub version: &'static str,
}

/// The detector behind a notice kind, `None` for a kind with no runtime switch.
pub fn detector_of(kind: NoticeKind) -> Option<Detector> {
    match kind {
        NoticeKind::XFloat => Some(Detector {
            id: crate::xfloat_live::logic::DETECTOR,
            version: crate::xfloat_live::logic::VERSION,
        }),
        _ => None,
    }
}

/// The parsed document: the ids switched off.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Switches {
    off: HashSet<String>,
}

impl Switches {
    /// The document as stored (`None`: never written). Anything unreadable is on.
    pub fn parse(doc: Option<&Value>) -> Self {
        let off = doc
            .and_then(Value::as_object)
            .map(|m| {
                m.iter()
                    .filter(|(_, v)| {
                        let on = match v {
                            Value::Bool(b) => Some(*b),
                            Value::Object(o) => o.get("on").and_then(Value::as_bool),
                            _ => None,
                        };
                        on == Some(false)
                    })
                    .map(|(k, _)| k.clone())
                    .collect()
            })
            .unwrap_or_default();
        Switches { off }
    }

    /// Whether a notice of `kind` may be sent.
    pub fn on(&self, kind: NoticeKind) -> bool {
        detector_of(kind).is_none_or(|d| !self.off.contains(d.id))
    }

    /// The ids switched off (the startup log line).
    pub fn off(&self) -> Vec<&str> {
        let mut v: Vec<&str> = self.off.iter().map(String::as_str).collect();
        v.sort_unstable();
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_detector_is_on_unless_its_entry_says_off() {
        let x = NoticeKind::XFloat;
        assert!(Switches::parse(None).on(x));
        assert!(Switches::parse(Some(&json!({}))).on(x));
        assert!(Switches::parse(Some(&json!({"x-float-1": {"on": true}}))).on(x));
        assert!(Switches::parse(Some(&json!({"x-float-1": {"note": "?"}}))).on(x));
        assert!(Switches::parse(Some(&json!("garbage"))).on(x));
        assert!(!Switches::parse(Some(&json!({"x-float-1": {"on": false}}))).on(x));
        assert!(!Switches::parse(Some(&json!({"x-float-1": false}))).on(x));
        // A kind without a detector id is never switched by this document.
        let all_off = Switches::parse(Some(&json!({"x-float-1": false, "n1": false})));
        assert!(all_off.on(NoticeKind::N1));
        assert_eq!(all_off.off(), vec!["n1", "x-float-1"]);
    }

    #[test]
    fn the_namespace_is_valid_and_client_writable() {
        assert!(crate::settings::valid_namespace(NS));
        assert!(!super::super::model::server_owned(NS));
    }
}

//! Per-detector delivery switches that take effect at once, with no restart (StockWatcher
//! research v2 DEPLOY-PLAN, owner 2026-11-21/22: "a runtime switch per item that needs no
//! restart"; the kill rules switch an item off during market hours, when a restart would drop
//! live prints, as on 2026-10-30).
//!
//! # What it is
//!
//! The synced settings namespace [`NS`] (`detectors`), one entry per detector id:
//! `{"run": {"on": false}, "x-halt-1": {"on": true}}` (a bare `true` / `false` is read the same).
//! An absent entry, or an entry that is not one of those shapes, takes the detector's default
//! ([`Detector::default_on`]). Clients write it with an entry-level patch
//! (`settings.op {ns: "detectors", op: {patch: {set: {"x-halt-1": {"on": false}}}}}`), so two
//! switches flipped from two devices both land. It is deliberately its own namespace, not part of
//! `alerts.prefs`: the clients write that one as a whole value and would erase entries they do not
//! know. It is client-writable (`super::model::server_owned` does not list it).
//!
//! The alerts task reads it at start and again the moment a `settings.changed` frame for it
//! arrives (`super::Task::reload`), like the rules and preferences.
//!
//! # What "off" does
//!
//! A detector that is switched off **keeps running**: its notices still reach the alerts task,
//! which holds them on both paths (desk and phone) as [`super::runners::Hold::SwitchedOff`]
//! (`switched_off`) before any other limit, and writes both decision lines to the notice ledger
//! as usual. So:
//!
//! - switching an item off at once (the DEPLOY-PLAN kill rules) loses nothing from the record;
//! - a **cut** kind (the `run` cut below) "keeps computing silently and logs its would-be
//!   alerts", so the replacement can be compared with it head to head on the ledger;
//! - a held notice is not marked sent, so switching the detector back on delivers its next
//!   event normally (not one already held).
//!
//! The per-device switches (Settings → Alerts, `super::DeskKinds`) are unchanged and still
//! apply when the detector is on.
//!
//! # The detectors
//!
//! Only the kinds a v2 item ships or cuts are listed ([`ALL`]); every other kind has no
//! detectors entry and is never held by this switch.
//!
//! - **`run`** ([`RUN`]): the "Run +20%" Runners notice ([`NoticeKind::Run`]), **off by
//!   default**: research v2's scorecard (`30-w2/finalists/sc2-1.json`, verdict CUT on Train, own
//!   precision 18.9 %, recall Δ 0, lead Δ 0; confirmed on Validation, same recall, +0.1 pp
//!   precision) removes it. The cut is leave-one-KIND-out on the alert table, so only the `run`
//!   kind is held: the board's Run row stays (later paths on the stock are still follow-ups) and
//!   a follow-up whose first or new path is Run is a [`NoticeKind::FollowUp`], not held. Nothing
//!   is pruned in `RocketBoard::notice` either (a prune there leaves no ledger line).
//! - **`x-halt-1`** ([`X_HALT_1`]): research v2's "resumed up after a halt"
//!   ([`NoticeKind::ResumedUp`], `crate::xhalt_live`), on by default.

use serde_json::Value;

use crate::rocket::NoticeKind;

/// The settings namespace.
pub const NS: &str = "detectors";

/// One switchable detector.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Detector {
    /// The switch's key in [`NS`] and the ledger's `detector`.
    pub id: &'static str,
    /// The frozen definition it runs: research id, config or finalist sha256 prefix and code
    /// commit. The ledger's `detector_version`.
    pub version: &'static str,
    /// The kinds it sends.
    pub kinds: &'static [NoticeKind],
    /// Its state with no entry in [`NS`].
    pub default_on: bool,
}

/// The "Run +20%" notice, cut by research v2 (module docs).
pub const RUN: Detector = Detector {
    id: "run",
    version: "sc2-1@4618772d (CUT; live kind unchanged)",
    kinds: &[NoticeKind::Run],
    default_on: false,
};

/// Research v2's x-halt-1 (module docs).
pub const X_HALT_1: Detector = Detector {
    id: crate::xhalt_live::DETECTOR,
    version: crate::xhalt_live::logic::VERSION,
    kinds: &[NoticeKind::ResumedUp],
    default_on: true,
};

/// Research v2's x-float-1 "rotation ignition" ([`NoticeKind::XFloat`], `crate::xfloat_live`).
pub const X_FLOAT_1: Detector = Detector {
    id: crate::xfloat_live::logic::DETECTOR,
    version: crate::xfloat_live::logic::VERSION,
    kinds: &[NoticeKind::XFloat],
    default_on: true,
};

/// Research v2's x-gap-1, pre-market new session high on volume ([`NoticeKind::XGap`],
/// `crate::gap_ctx`).
pub const X_GAP_1: Detector = Detector {
    id: crate::gap_ctx::logic::XGAP_ID,
    version: crate::gap_ctx::logic::XGAP_VERSION,
    kinds: &[NoticeKind::XGap],
    default_on: true,
};

/// Research v2's x-ctx-1, day 2: yesterday's runner breaks yesterday's high
/// ([`NoticeKind::XCtx`], `crate::gap_ctx`).
pub const X_CTX_1: Detector = Detector {
    id: crate::gap_ctx::logic::XCTX_ID,
    version: crate::gap_ctx::logic::XCTX_VERSION,
    kinds: &[NoticeKind::XCtx],
    default_on: true,
};

/// Research v2's rw-mp5-1: MP5 Signals crossings gated by A-G9 with the 5 s B0 dedup
/// (`crate::mp5_gate`). It sends no kind of its own, so [`of`] never names it: the alerts task
/// asks for it by itself when it releases a parked Signals crossing, and off holds every Signals
/// crossing `switched_off` (none reached anyone before it shipped: `DETECTION_PANEL` was 0).
pub const RW_MP5_1: Detector = Detector {
    id: crate::mp5_gate::logic::DETECTOR,
    version: "933ca78580fded2c9e18facd24873a48b5e50ac60a1f3b7e4bbe194d1a25bc7e@0294ddac",
    kinds: &[],
    default_on: true,
};

/// Research v2's b-good-1 confidence tag (`crate::bgood`): it sends no notice, so [`of`] never
/// names it; off, a scored tag is still ledgered (`confidence` line, its switch `off`) but is
/// neither shown nor marks the Runners row. Version: the finalist record's sha256 prefix, the
/// same 12 digits `crate::bgood::version` reads from the embedded model.
pub const B_GOOD_1: Detector = Detector {
    id: crate::bgood::TAG_ID,
    version: "d8a1f8ff993e",
    kinds: &[],
    default_on: true,
};

/// Research v2's b-good-2 model picks ([`NoticeKind::BGood`], `crate::bgood`).
pub const B_GOOD_2: Detector = Detector {
    id: crate::bgood::PICK_ID,
    version: "e4273799d70e",
    kinds: &[NoticeKind::BGood],
    default_on: true,
};

pub const ALL: [Detector; 8] = [
    RUN, X_HALT_1, X_FLOAT_1, X_GAP_1, X_CTX_1, RW_MP5_1, B_GOOD_1, B_GOOD_2,
];

/// The detector that sends `kind`, if it is switchable.
pub fn of(kind: NoticeKind) -> Option<&'static Detector> {
    ALL.iter().find(|d| d.kinds.contains(&kind))
}

/// The switches as stored: an entry's explicit state, or none (the default).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Switches {
    explicit: Vec<(&'static str, bool)>,
}

impl Switches {
    /// Parses the namespace's value (absent or not an object = every default).
    pub fn parse(v: Option<&Value>) -> Self {
        let mut explicit = Vec::new();
        if let Some(Value::Object(m)) = v {
            for d in ALL {
                let on = match m.get(d.id) {
                    Some(Value::Bool(b)) => Some(*b),
                    Some(Value::Object(e)) => e.get("on").and_then(Value::as_bool),
                    _ => None,
                };
                if let Some(on) = on {
                    explicit.push((d.id, on));
                }
            }
        }
        Switches { explicit }
    }

    /// Whether `d` is on.
    pub fn on(&self, d: &Detector) -> bool {
        self.explicit
            .iter()
            .find(|(id, _)| *id == d.id)
            .map_or(d.default_on, |(_, on)| *on)
    }

    /// Whether a notice of `kind` is held because its detector is off.
    pub fn holds(&self, kind: NoticeKind) -> bool {
        of(kind).is_some_and(|d| !self.on(d))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn defaults_run_cut_and_x_halt_on() {
        let s = Switches::parse(None);
        assert!(s.holds(NoticeKind::Run), "the run cut is the default");
        assert!(!s.holds(NoticeKind::ResumedUp));
        // Only the listed kinds are switchable: the follow-up a Run path makes is not held.
        for k in NoticeKind::ALL {
            if !matches!(k, NoticeKind::Run | NoticeKind::ResumedUp) {
                assert!(of(k).is_none() && !s.holds(k), "{k:?}");
            }
        }
    }

    #[test]
    fn entries_override_and_bad_shapes_take_the_default() {
        let s = Switches::parse(Some(&json!({"run": {"on": true}, "x-halt-1": false})));
        assert!(!s.holds(NoticeKind::Run) && s.holds(NoticeKind::ResumedUp));
        let s = Switches::parse(Some(
            &json!({"run": "yes", "x-halt-1": {"on": 1}, "zzz": true}),
        ));
        assert_eq!(s, Switches::default());
        assert_eq!(Switches::parse(Some(&json!([1, 2]))), Switches::default());
    }
}

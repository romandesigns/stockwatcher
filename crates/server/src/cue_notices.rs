//! Ross and Signals push notices (owner, 2026-10-23: "every panel event backed by evidence must
//! notify on every client"). They travel the Runners pipeline (`crate::alerts::runners`): same
//! channel, limits, text module and clients; this module only decides WHAT happened.
//!
//! # What
//!
//! Both panels read the same detector: a Ross row's cue is the engine's `micro_pullback` (MP5)
//! episode under `detection-v2`, the very episode the Signals (Detection) panel lists
//! (`stock_context::cue`). So:
//!
//! - **Crossing** (on by default): a `micro_pullback` transition to `crossing_provisional` (the
//!   cue's "Crossing", a post-arm print above the trigger). Decided from the engine's transition
//!   journal the moment it is drained ([`CueNotices::transitions`]), not from a 15 s panel
//!   snapshot.
//! - **At trigger** (opt-in, off by default): an armed episode whose latest fresh price is below
//!   the trigger by at most the policy's `near_trigger_bp` (the cue's "At trigger", the Detection
//!   Panel's "Near trigger"). Read from the Ross panel's latest snapshot ([`CueNotices::ross_panel`])
//!   and the Detection snapshot's assessments ([`CueNotices::signals_snapshot`]).
//!
//! # One episode, one notice
//!
//! A Ross member's episode is also on the Signals panel whenever the Detection Panel is on. It is
//! announced ONCE, as the Ross kind ([`NoticeKind::RossCrossing`] / [`NoticeKind::RossTrigger`]),
//! with [`CueNotice::also_signals`] set; a device or desk gets it when either its Ross or its
//! Signals switch for that stage is on (`crate::alerts::DeskKinds::wants`). The Signals kinds are
//! only for stocks that are NOT Ross members. Ross was picked as the one name because the Ross
//! panel is the curated list (five pillars) the owner reads first; the episode, levels and ids are
//! identical either way. A Runners-watched stock's MP5 episode (the engine runs it for the flagged
//! symbols' levels) with the Detection Panel off is on neither panel and announces nothing.
//!
//! Each `(stage, episode)` is announced at most once per process ([`CueNotices::seen`]); the
//! delivery side also refuses an event id it already sent, persisted, so a restart cannot repeat
//! one. Warm-up transitions (a restored prefix) never notify.

use std::collections::HashSet;

use stock_context::{
    CueState, DetectionSnapshot, DetectionTransition, Engine, Family, PanelSnapshot, State,
};

use crate::rocket::{CueNotice, Notice, NoticeDetail, NoticeKind};

/// Bound on the episodes remembered (a day has a few hundred at most); past it the memory starts
/// over and the delivery side's persisted event ids still hold repeats back.
const SEEN_MAX: usize = 50_000;
/// Notices held while nobody takes them (a board ticked without a market, a test).
const OUT_MAX: usize = 256;

#[derive(Debug, Default)]
pub struct CueNotices {
    out: Vec<Notice>,
    seen: HashSet<(NoticeKind, String)>,
}

fn dollars(micros: i64) -> f64 {
    micros as f64 / 1_000_000.0
}

fn member(engine: &Engine, symbol: &str) -> bool {
    engine
        .current()
        .find(|d| d.symbol == symbol)
        .is_some_and(|d| d.membership.active().is_some())
}

impl CueNotices {
    /// The notices decided since the last call, oldest first.
    pub fn take(&mut self) -> Vec<Notice> {
        std::mem::take(&mut self.out)
    }

    #[allow(clippy::too_many_arguments)]
    fn push(
        &mut self,
        kind: NoticeKind,
        episode_id: &str,
        symbol: &str,
        at_ns: i64,
        price: Option<i64>,
        trigger: Option<i64>,
        also_signals: bool,
        exec_ns: Option<i64>,
    ) {
        if !self.seen.insert((kind, episode_id.to_string())) {
            return;
        }
        if self.seen.len() > SEEN_MAX {
            self.seen.clear();
            self.seen.insert((kind, episode_id.to_string()));
        }
        if self.out.len() >= OUT_MAX {
            return;
        }
        self.out.push(Notice {
            kind,
            alert_id: episode_id.to_string(),
            symbol: symbol.to_string(),
            at_ns,
            price: price.map_or(f64::NAN, dollars),
            from_flag_pct: None,
            reasons: Vec::new(),
            since_start_min: None,
            exec_ns,
            detail: Some(NoticeDetail::Cue(CueNotice {
                episode_id: episode_id.to_string(),
                trigger: trigger.map(dollars),
                also_signals,
            })),
        });
    }

    /// The crossings among drained transitions (module docs, "What").
    pub fn transitions(&mut self, engine: &Engine, ts: &[DetectionTransition]) {
        let signals = engine.detection_policy().is_some();
        self.transitions_with(ts, signals, |s| member(engine, s));
    }

    /// [`CueNotices::transitions`] with the Signals panel's state and the membership given.
    pub fn transitions_with(
        &mut self,
        ts: &[DetectionTransition],
        signals: bool,
        member: impl Fn(&str) -> bool,
    ) {
        for t in ts {
            if t.family != Family::MicroPullback
                || t.to != State::CrossingProvisional
                || t.from == Some(State::CrossingProvisional)
                || t.warmup
            {
                continue;
            }
            let is_member = member(&t.symbol);
            let kind = if is_member {
                NoticeKind::RossCrossing
            } else if signals {
                NoticeKind::SignalsCrossing
            } else {
                continue;
            };
            let trigger = t
                .level
                .as_ref()
                .filter(|l| l.kind == "trigger")
                .map(|l| l.price);
            let price = t.trade.as_ref().map(|x| x.price);
            let at = t
                .trade
                .as_ref()
                .map_or(t.observed_at, |x| x.admit.max(t.observed_at));
            self.push(
                kind,
                &t.episode_id,
                &t.symbol,
                at,
                price,
                trigger,
                is_member && signals,
                t.trade.as_ref().map(|x| x.exec),
            );
        }
    }

    /// Ross members at the trigger (module docs, "At trigger"), from the latest panel snapshot.
    /// A member's crossing seen here first (a transition the Detection Panel's journal took) is
    /// also announced, once.
    pub fn ross_panel(&mut self, panel: &PanelSnapshot, signals: bool) {
        for r in &panel.rows {
            let c = &r.setup;
            let Some(ep) = c.episode_id.as_deref() else {
                continue;
            };
            if c.stale {
                continue;
            }
            let kind = match c.state {
                CueState::Near => NoticeKind::RossTrigger,
                CueState::Crossing => NoticeKind::RossCrossing,
                _ => continue,
            };
            self.push(
                kind,
                ep,
                &r.symbol,
                c.state_since.unwrap_or(panel.cutoff).max(panel.cutoff),
                c.price,
                c.trigger.as_ref().map(|l| l.price),
                signals,
                None,
            );
        }
    }

    /// Signals episodes at the trigger, from a Detection snapshot (non-members only: a member's
    /// is the Ross notice).
    pub fn signals_snapshot(&mut self, engine: &Engine, snap: &DetectionSnapshot) {
        for e in &snap.active {
            if e.family != Family::MicroPullback || e.warmup || e.state != State::Armed {
                continue;
            }
            let Some(a) = e.current.as_ref() else {
                continue;
            };
            if a.stale || a.near_trigger != Some(true) || member(engine, &e.symbol) {
                continue;
            }
            self.push(
                NoticeKind::SignalsTrigger,
                &e.episode_id,
                &e.symbol,
                snap.as_of.unwrap_or(e.last_material_at),
                Some(a.price),
                a.trigger,
                false,
                Some(a.price_exec),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use stock_context::Certainty;

    fn tr(ep: &str, symbol: &str, from: State, to: State) -> DetectionTransition {
        DetectionTransition {
            seq: 1,
            transition_id: format!("{ep}:t1"),
            episode_id: ep.into(),
            symbol: symbol.into(),
            family: Family::MicroPullback,
            variant: "micro_pullback_mp5".into(),
            from: Some(from),
            to,
            event: "crossing",
            reason: "post_arm_print_above_trigger",
            certainty: Certainty::Provisional,
            observed_at: 1_761_200_000_000_000_000,
            bar_end: None,
            cutoff: None,
            trade: None,
            features: None,
            level: None,
            anchors: None,
            checks: Vec::new(),
            correction_of: None,
            warmup: false,
            default_visible: true,
            related_episode_ids: Vec::new(),
        }
    }

    /// One notice per episode and stage; a Ross member's crossing is the Ross kind (with
    /// `also_signals` when the Detection Panel is on), a non-member's the Signals kind, and
    /// with the Detection Panel off a non-member's is on no panel and announces nothing.
    #[test]
    fn crossings_once_per_episode_ross_wins_the_dedupe() {
        let mut c = CueNotices::default();
        let member = |s: &str| s == "ROSS";
        let ross = tr("e1", "ROSS", State::Armed, State::CrossingProvisional);
        let sig = tr("e2", "SIG", State::Armed, State::CrossingProvisional);
        c.transitions_with(&[ross.clone(), sig.clone()], true, member);
        // The same transitions drained again (the other journal, a replayed frame): nothing.
        c.transitions_with(&[ross.clone(), sig.clone()], true, member);
        let n = c.take();
        assert_eq!(n.len(), 2, "{n:?}");
        assert_eq!(n[0].kind, NoticeKind::RossCrossing);
        assert_eq!(n[0].alert_id, "e1");
        assert!(matches!(&n[0].detail, Some(NoticeDetail::Cue(x)) if x.also_signals));
        assert_eq!(n[1].kind, NoticeKind::SignalsCrossing);
        assert!(matches!(&n[1].detail, Some(NoticeDetail::Cue(x)) if !x.also_signals));
        // Detection Panel off: a member still notifies (not also on Signals), a non-member not.
        let mut off = CueNotices::default();
        let ross3 = tr("e3", "ROSS", State::Armed, State::CrossingProvisional);
        let sig4 = tr("e4", "SIG", State::Armed, State::CrossingProvisional);
        off.transitions_with(&[ross3, sig4], false, member);
        let n = off.take();
        assert_eq!(n.len(), 1);
        assert!(matches!(&n[0].detail, Some(NoticeDetail::Cue(x)) if !x.also_signals));
        // A warm-up, another family, a later stage: nothing.
        let mut w = tr("e5", "ROSS", State::Armed, State::CrossingProvisional);
        w.warmup = true;
        let mut fam = tr("e6", "ROSS", State::Armed, State::CrossingProvisional);
        fam.family = Family::UpwardAcceleration;
        let other = tr(
            "e7",
            "ROSS",
            State::CrossingProvisional,
            State::TriggerObserved,
        );
        off.transitions_with(&[w, fam, other], true, member);
        assert!(off.take().is_empty());
    }
}

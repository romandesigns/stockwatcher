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
//!
//! # Pullback (owner, 2026-10-25)
//!
//! "Pullback setting up" brings the same MP5 episode forward to the moment it ARMS (trigger and
//! stop fixed), for stocks with context: a Runners row earlier the same day (the engine already
//! runs MP5 for them, `Engine::set_watched`) or a Ross member. Not for every stock: MP5 on its own
//! was no better than a volatility-matched baseline (research/setup-compare), and the context is
//! what the owner trusts. Research only ever measured it as an entry, where it did not beat buying
//! at the alert (research/pullback-after-alert), so the text describes, it never says "buy".
//!
//! - **Setting up** ([`NoticeKind::Pullback`]): the `armed` transition, Runners stocks and Ross
//!   members alike (it is new for both).
//! - **Crossing** of that episode: a Ross member's stays [`NoticeKind::RossCrossing`] (already
//!   announced, never twice); a Runners stock that is not a member gets
//!   [`NoticeKind::PullbackCrossing`], which stands for the Signals crossing too when the Detection
//!   Panel is on (`also_signals`), exactly as the Ross kind does. Invalidated or expired episodes
//!   push nothing (the row's levels show it).

use std::collections::{BTreeSet, HashSet};

use stock_context::{
    CueState, DetectionSnapshot, DetectionTransition, Engine, Family, MicroAnchors, PanelSnapshot,
    State,
};

use crate::rocket::{CueNotice, Notice, NoticeDetail, NoticeKind, PullbackNotice};

/// Bound on the episodes remembered (a day has a few hundred at most); past it the memory starts
/// over and the delivery side's persisted event ids still hold repeats back.
const SEEN_MAX: usize = 50_000;
/// Notices held while nobody takes them (a board ticked without a market, a test).
const OUT_MAX: usize = 256;

#[derive(Debug, Default)]
pub struct CueNotices {
    out: Vec<Notice>,
    seen: HashSet<(NoticeKind, String)>,
    /// Today's Runners symbols (`crate::rocket`'s flagged rows, the engine's watched set): the
    /// pullback notices' context, beside Ross membership.
    watched: BTreeSet<String>,
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

    /// Today's Runners symbols, replacing the previous set (the market hands the engine the same
    /// list, `crate::context::ContextBoard::set_watched`).
    pub fn set_watched(&mut self, symbols: &[String]) {
        self.watched = symbols.iter().cloned().collect();
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
        let detail = NoticeDetail::Cue(CueNotice {
            episode_id: episode_id.to_string(),
            trigger: trigger.map(dollars),
            also_signals,
        });
        self.emit(kind, episode_id, symbol, at_ns, price, exec_ns, detail);
    }

    #[allow(clippy::too_many_arguments)]
    fn emit(
        &mut self,
        kind: NoticeKind,
        episode_id: &str,
        symbol: &str,
        at_ns: i64,
        price: Option<i64>,
        exec_ns: Option<i64>,
        detail: NoticeDetail,
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
            detail: Some(detail),
        });
    }

    /// The crossings and pullback setups among drained transitions (module docs, "What" and
    /// "Pullback").
    pub fn transitions(&mut self, engine: &Engine, ts: &[DetectionTransition]) {
        let signals = engine.detection_policy().is_some();
        let watched = self.watched.clone();
        self.transitions_with(
            ts,
            signals,
            |s| member(engine, s),
            |s| watched.contains(s),
            // An arming happens at a barrier, without a print of its own: the price is the cue's
            // latest fresh one, as the Runners row shows it.
            |s, tau| engine.watched_cue(s, tau).and_then(|c| c.price),
        );
    }

    /// [`CueNotices::transitions`] with the Signals panel's state, the membership, the Runners
    /// set and the latest price given.
    pub fn transitions_with(
        &mut self,
        ts: &[DetectionTransition],
        signals: bool,
        member: impl Fn(&str) -> bool,
        watched: impl Fn(&str) -> bool,
        latest_price: impl Fn(&str, i64) -> Option<i64>,
    ) {
        for t in ts {
            if t.family != Family::MicroPullback || t.warmup || t.from == Some(t.to) {
                continue;
            }
            match t.to {
                State::Armed if member(&t.symbol) || watched(&t.symbol) => {
                    let price = latest_price(&t.symbol, t.observed_at);
                    let detail = NoticeDetail::Pullback(pullback_facts(t, false));
                    self.emit(
                        NoticeKind::Pullback,
                        &t.episode_id,
                        &t.symbol,
                        t.observed_at,
                        price,
                        None,
                        detail,
                    );
                }
                State::CrossingProvisional => self.crossing(t, signals, &member, &watched),
                _ => {}
            }
        }
    }

    fn crossing(
        &mut self,
        t: &DetectionTransition,
        signals: bool,
        member: &impl Fn(&str) -> bool,
        watched: &impl Fn(&str) -> bool,
    ) {
        let is_member = member(&t.symbol);
        let price = t.trade.as_ref().map(|x| x.price);
        let at = t
            .trade
            .as_ref()
            .map_or(t.observed_at, |x| x.admit.max(t.observed_at));
        let exec = t.trade.as_ref().map(|x| x.exec);
        if !is_member && watched(&t.symbol) {
            // A Runners stock's setup reaching its trigger: the setting-up notice's follow-up,
            // standing for the Signals crossing too (one episode, one notice).
            let detail = NoticeDetail::Pullback(pullback_facts(t, signals));
            self.emit(
                NoticeKind::PullbackCrossing,
                &t.episode_id,
                &t.symbol,
                at,
                price,
                exec,
                detail,
            );
            return;
        }
        let kind = if is_member {
            NoticeKind::RossCrossing
        } else if signals {
            NoticeKind::SignalsCrossing
        } else {
            return;
        };
        let trigger = t
            .level
            .as_ref()
            .filter(|l| l.kind == "trigger")
            .map(|l| l.price);
        self.push(
            kind,
            &t.episode_id,
            &t.symbol,
            at,
            price,
            trigger,
            is_member && signals,
            exec,
        );
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

/// The setup's facts from the episode's anchors and trigger level (module docs, "Pullback").
fn pullback_facts(t: &DetectionTransition, also_signals: bool) -> PullbackNotice {
    let a: Option<&MicroAnchors> = t.anchors.as_ref();
    let trigger = t
        .level
        .as_ref()
        .filter(|l| l.kind == "trigger")
        .map(|l| l.price)
        .or(a.map(|a| a.h0))
        .filter(|p| *p > 0);
    PullbackNotice {
        episode_id: t.episode_id.clone(),
        trigger: trigger.map(dollars),
        stop: a.map(|a| a.pullback_low).filter(|p| *p > 0).map(dollars),
        dip_pct: a.map(|a| a.retreat_bp as f64 / 100.0),
        retraced_pct: a.map(|a| a.depth_milli as f64 / 10.0),
        dip_volume_ratio: a
            .and_then(|a| a.dv_rate_ratio_milli)
            .map(|m| m as f64 / 1000.0),
        also_signals,
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
        // No Runners rows here: the crossings behave exactly as before the pullback notices.
        let none = |_: &str| false;
        let no_price = |_: &str, _: i64| None;
        let ross = tr("e1", "ROSS", State::Armed, State::CrossingProvisional);
        let sig = tr("e2", "SIG", State::Armed, State::CrossingProvisional);
        c.transitions_with(&[ross.clone(), sig.clone()], true, member, none, no_price);
        // The same transitions drained again (the other journal, a replayed frame): nothing.
        c.transitions_with(&[ross.clone(), sig.clone()], true, member, none, no_price);
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
        off.transitions_with(&[ross3, sig4], false, member, none, no_price);
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
        off.transitions_with(&[w, fam, other], true, member, none, no_price);
        assert!(off.take().is_empty());
    }

    /// An arming's anchors as MP5 fixes them: H0 $3.42 (the trigger), Lp $3.21 (the stop), a
    /// 6.1 % dip that retraced 31 % of the impulse on 0.4× its dollar-volume rate.
    fn armed(ep: &str, symbol: &str) -> DetectionTransition {
        let mut t = tr(ep, symbol, State::PullbackForming, State::Armed);
        t.event = "armed";
        t.reason = "levels_fixed";
        t.anchors = Some(MicroAnchors {
            l0: 3_000_000,
            l0_bar_end: 0,
            h0: 3_420_000,
            h0_bar_end: 0,
            impulse_bp: 1400,
            impulse_slots: 8,
            impulse_coverage_milli: 1000,
            frozen_at: 0,
            pullback_low: 3_210_000,
            pullback_low_bar_end: 0,
            depth_milli: 310,
            retreat_bp: 614,
            pullback_bars: 3,
            pullback_slots: 3,
            dv_rate_ratio_milli: Some(400),
            armed_at: Some(1),
        });
        t
    }

    /// Setting up: only stocks with context (a Runners row today, or a Ross member), once per
    /// episode, never for a warm-up arming; the facts are the episode's own.
    #[test]
    fn pullback_setting_up_needs_context_and_fires_once() {
        let mut c = CueNotices::default();
        let member = |s: &str| s == "ROSS";
        let watched = |s: &str| s == "RUNR";
        let price = |_: &str, _: i64| Some(3_300_000);
        let runr = armed("p1", "RUNR");
        let ross = armed("p2", "ROSS");
        let bare = armed("p3", "BARE");
        let mut warm = armed("p4", "RUNR");
        warm.warmup = true;
        c.transitions_with(
            &[runr.clone(), ross, bare, warm],
            true,
            member,
            watched,
            price,
        );
        c.transitions_with(&[runr], true, member, watched, price);
        let n = c.take();
        assert_eq!(n.len(), 2, "{n:?}");
        assert!(n.iter().all(|x| x.kind == NoticeKind::Pullback));
        assert_eq!(
            (n[0].symbol.as_str(), n[1].symbol.as_str()),
            ("RUNR", "ROSS")
        );
        assert_eq!(n[0].price, 3.3);
        let Some(NoticeDetail::Pullback(f)) = &n[0].detail else {
            panic!("pullback facts: {:?}", n[0].detail);
        };
        assert_eq!((f.trigger, f.stop), (Some(3.42), Some(3.21)));
        assert_eq!((f.dip_pct, f.retraced_pct), (Some(6.14), Some(31.0)));
        assert_eq!(f.dip_volume_ratio, Some(0.4));
    }

    /// The crossing of a setup: a Ross member's stays the Ross notice (never also a pullback
    /// one); a Runners stock's is the pullback follow-up, standing for the Signals crossing too.
    #[test]
    fn pullback_crossing_follows_up_without_doubling_ross_or_signals() {
        let mut c = CueNotices::default();
        let member = |s: &str| s == "ROSS";
        let watched = |s: &str| s == "RUNR" || s == "ROSS";
        let price = |_: &str, _: i64| None;
        let mut runr = armed("p1", "RUNR");
        runr.from = Some(State::Armed);
        runr.to = State::CrossingProvisional;
        let mut ross = armed("p2", "ROSS");
        ross.from = Some(State::Armed);
        ross.to = State::CrossingProvisional;
        c.transitions_with(&[runr, ross], true, member, watched, price);
        let n = c.take();
        let kinds: Vec<_> = n.iter().map(|x| (x.symbol.as_str(), x.kind)).collect();
        assert_eq!(
            kinds,
            vec![
                ("RUNR", NoticeKind::PullbackCrossing),
                ("ROSS", NoticeKind::RossCrossing),
            ]
        );
        assert!(matches!(&n[0].detail, Some(NoticeDetail::Pullback(f)) if f.also_signals));
        // Detection Panel off: the follow-up stands for nothing else.
        let mut off = CueNotices::default();
        let mut x = armed("p3", "RUNR");
        x.from = Some(State::Armed);
        x.to = State::CrossingProvisional;
        off.transitions_with(&[x], false, member, watched, price);
        let n = off.take();
        assert!(matches!(&n[0].detail, Some(NoticeDetail::Pullback(f)) if !f.also_signals));
        // An invalidated setup pushes nothing.
        let mut gone = armed("p4", "RUNR");
        gone.from = Some(State::Armed);
        gone.to = State::Invalidated;
        off.transitions_with(&[gone], false, member, watched, price);
        assert!(off.take().is_empty());
    }
}

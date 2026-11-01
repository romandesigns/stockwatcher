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
//! one. Warm-up transitions (a restored prefix) never notify; a micro-pullback is warm-up only when
//! it could already have ARMED inside that prefix (`stock_context::detect::micro`), not merely
//! because its impulse began there.
//!
//! Across a restart the memory comes back from the logs (`crate::notice_memory`), and episodes
//! are numbered past the previous process's, so a setup still open at the restart returns under
//! a NEW id. It is recognized by its symbol and H0 bar ([`CueNotices::alias`]) and keyed by its
//! old id, so the panel's "At trigger" or its crossing is not sent twice (the new event id would
//! get past the delivery ledger), and its crossing stays the follow-up of the setup announced.
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
//! - **Setting up** ([`NoticeKind::Pullback`]): the `armed` transition, and only the FIRST one per
//!   stock and trading day: for a Runners stock within [`PULLBACK_WINDOW_NS`] of the first time it
//!   was flagged that day, for a Ross member at any time. Every arming on a context stock would be
//!   ~367 notices a day (research/pullback-context, EXPLORE, 147 days); the first one within 30
//!   minutes of the alert is ~37, and the setups it keeps went on to continue a little more often
//!   after crossing (39 % vs 34 %, descriptive, not a validated edge).
//! - **Crossing** of that episode: a Ross member's stays [`NoticeKind::RossCrossing`] (already
//!   announced, never twice); a setup this module announced on a non-member gets
//!   [`NoticeKind::PullbackCrossing`], which stands for the Signals crossing too when the Detection
//!   Panel is on (`also_signals`), exactly as the Ross kind does. Invalidated or expired episodes
//!   push nothing (the row's levels show it).
//! - **A setup that crosses at once is still two notices.** From 2026-10-26 a crossing within
//!   30 s of its own setting-up notice was not pushed (XTND: setup 10:28:02 ET, crossing 10:28:04).
//!   Removed on 2026-10-30 (owner: "I don't want to keep missing alerts because they weren't
//!   delivered to my phone"): the setup says the trigger is set, the crossing says price went
//!   through it, and those are different events; every distinct event is delivered
//!   (`crate::alerts::runners`). The phone stacks a stock's pushes together.

use std::collections::{BTreeMap, HashMap, HashSet};

use chrono::NaiveDate;
use market_events::UnixNanos;
use market_events::session::trading_date;

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
/// How long after a stock is first flagged that day its first setup still notifies (module docs,
/// "Pullback").
pub const PULLBACK_WINDOW_NS: i64 = 30 * 60 * 1_000_000_000;

/// Why an armed micro-pullback setup did or did not get its "setting up" notice: the reasons
/// [`CueNotices::transitions_with`] decides by, recorded where it decides them, so the ticker
/// page's missed setups (`crate::insights`) never re-implement the rules.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SetupVerdict {
    /// Announced.
    Notified,
    /// Its recognition read a window reaching into a restored prefix (a restart): warm-up
    /// transitions never notify.
    Warmup,
    /// Neither a Runners row that day nor a Ross member.
    NoContext,
    /// A Runners row, but armed more than [`PULLBACK_WINDOW_NS`] after the stock was first
    /// flagged (by this process) that day; not a Ross member.
    WindowClosed { since: i64 },
    /// The stock already had its one setting-up notice that trading day.
    OnePerDay,
}

impl SetupVerdict {
    pub fn code(self) -> &'static str {
        match self {
            SetupVerdict::Notified => "notified",
            SetupVerdict::Warmup => "warmup",
            SetupVerdict::NoContext => "no_context",
            SetupVerdict::WindowClosed { .. } => "window_closed",
            SetupVerdict::OnePerDay => "one_per_day",
        }
    }
}

/// One armed episode's [`SetupVerdict`], at the arming.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SetupDecision {
    pub episode_id: String,
    pub symbol: String,
    pub at: i64,
    pub verdict: SetupVerdict,
}

#[derive(Debug, Default)]
pub struct CueNotices {
    out: Vec<Notice>,
    seen: HashSet<(NoticeKind, String)>,
    /// Today's Runners symbols (`crate::rocket`'s flagged rows, the engine's watched set) and when
    /// each was first flagged: the pullback notices' context, beside Ross membership.
    watched: BTreeMap<String, i64>,
    /// Stocks whose setting-up notice went out, per trading day: one each (module docs).
    setup_days: HashSet<(String, NaiveDate)>,
    /// Episodes announced as setting up: only their crossing is the pullback follow-up.
    setup_episodes: HashSet<String>,
    /// The first arming of each episode and its verdict, when recording ([`CueNotices::recording`]:
    /// the ticker page's replay; the live market never records).
    decisions: Option<(Vec<SetupDecision>, HashSet<String>)>,
    /// Setups a previous process of this run announced, by `(symbol, H0 bar end)`, with the id it
    /// gave them ([`crate::notice_memory::NoticeMemory::inherited`]).
    inherited: HashMap<(String, i64), String>,
    /// This process's id for an inherited setup -> the previous process's. A restart restores the
    /// day's bars, so a setup that was armed (or near its trigger) before it is opened again, and
    /// the episode floors (`stock_context::Engine::set_episode_floors`) give it a NEW number: its
    /// "At trigger" or crossing would otherwise be a second notice with a fresh event id that
    /// neither `seen` nor the delivery ledger recognizes. Every memory below is keyed by the
    /// canonical (first) id.
    alias: HashMap<String, String>,
}

fn dollars(micros: i64) -> f64 {
    micros as f64 / 1_000_000.0
}

pub(crate) fn member(engine: &Engine, symbol: &str) -> bool {
    engine
        .current()
        .find(|d| d.symbol == symbol)
        .is_some_and(|d| d.membership.active().is_some())
}

impl CueNotices {
    /// A decider that also records each setup's [`SetupVerdict`] (`crate::insights`).
    pub fn recording() -> Self {
        CueNotices {
            decisions: Some((Vec::new(), HashSet::new())),
            ..Default::default()
        }
    }

    /// The verdicts recorded since the last call (empty unless [`CueNotices::recording`]).
    pub fn take_setup_decisions(&mut self) -> Vec<SetupDecision> {
        self.decisions
            .as_mut()
            .map(|(d, _)| std::mem::take(d))
            .unwrap_or_default()
    }

    /// Records the FIRST arming of an episode (a re-arm after a failed crossing is the same
    /// setup, decided again by the same rules, and never notifies twice).
    fn record(&mut self, t: &DetectionTransition, verdict: SetupVerdict) {
        if let Some((out, seen)) = self.decisions.as_mut()
            && seen.insert(t.episode_id.clone())
        {
            out.push(SetupDecision {
                episode_id: t.episode_id.clone(),
                symbol: t.symbol.clone(),
                at: t.observed_at,
                verdict,
            });
        }
    }

    /// The notices decided since the last call, oldest first.
    pub fn take(&mut self) -> Vec<Notice> {
        std::mem::take(&mut self.out)
    }

    /// Today's Runners symbols with the time each was first flagged (its first row today,
    /// `crate::rocket::RocketBoard::take_watch`), replacing the previous set (the market hands the
    /// engine the same list, `crate::context::ContextBoard::set_watched`). The time is the row's,
    /// not this process's first sight of it: the rows come back from the forward log after a
    /// restart, so the 30-minute window does not start over at a deploy.
    pub fn set_watched(&mut self, flagged: &[(String, i64)]) {
        self.watched = flagged.iter().cloned().collect();
    }

    /// What a previous process of the same run already announced today
    /// ([`crate::notice_memory`]): its episodes are not announced again and its setting-up notices
    /// still count as each stock's one for the day, so a restart never sends a second "first of
    /// the day".
    pub fn restore(&mut self, m: &crate::notice_memory::NoticeMemory) {
        self.seen.extend(m.seen.iter().cloned());
        self.setup_days.extend(m.setup_days.iter().cloned());
        self.setup_episodes.extend(m.setup_episodes.iter().cloned());
        self.inherited.extend(
            m.inherited
                .iter()
                .map(|(s, h0, id)| ((s.clone(), *h0), id.clone())),
        );
    }

    /// Notes that this process's `episode_id` is the setup `(symbol, h0_bar_end)` a previous
    /// process announced under another id ([`CueNotices::alias`]).
    fn learn(&mut self, symbol: &str, episode_id: &str, h0_bar_end: Option<i64>) {
        let Some(h0) = h0_bar_end else {
            return;
        };
        if self.inherited.is_empty() || self.alias.contains_key(episode_id) {
            return;
        }
        if let Some(old) = self.inherited.get(&(symbol.to_string(), h0))
            && old != episode_id
        {
            self.alias.insert(episode_id.to_string(), old.clone());
        }
    }

    /// The id the memories key `episode_id` by: the previous process's for an inherited setup.
    fn canonical<'a>(&'a self, episode_id: &'a str) -> &'a str {
        self.alias
            .get(episode_id)
            .map_or(episode_id, String::as_str)
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
        let key = self.canonical(episode_id).to_string();
        if !self.seen.insert((kind, key.clone())) {
            return;
        }
        if self.seen.len() > SEEN_MAX {
            self.seen.clear();
            self.seen.insert((kind, key));
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
            |s| watched.get(s).copied(),
            // An arming happens at a barrier, without a print of its own: the price is the cue's
            // latest fresh one, as the Runners row shows it.
            |s, tau| engine.watched_cue(s, tau).and_then(|c| c.price),
        );
    }

    /// [`CueNotices::transitions`] with the Signals panel's state, the membership, when each
    /// Runners stock was first flagged and the latest price given.
    pub fn transitions_with(
        &mut self,
        ts: &[DetectionTransition],
        signals: bool,
        member: impl Fn(&str) -> bool,
        watched_since: impl Fn(&str) -> Option<i64>,
        latest_price: impl Fn(&str, i64) -> Option<i64>,
    ) {
        for t in ts {
            if t.family != Family::MicroPullback {
                continue;
            }
            // Before the warm-up skip: an inherited setup's first transitions are usually warm-up.
            self.learn(
                &t.symbol,
                &t.episode_id,
                t.anchors.as_ref().map(|a| a.h0_bar_end),
            );
            if t.from == Some(t.to) {
                continue;
            }
            if t.warmup {
                if t.to == State::Armed {
                    self.record(t, SetupVerdict::Warmup);
                }
                continue;
            }
            match t.to {
                State::Armed => {
                    let since = watched_since(&t.symbol);
                    let fresh = since.is_some_and(|since| {
                        t.observed_at >= since && t.observed_at - since <= PULLBACK_WINDOW_NS
                    });
                    if !(fresh || member(&t.symbol)) {
                        let verdict = match since {
                            Some(since) => SetupVerdict::WindowClosed { since },
                            None => SetupVerdict::NoContext,
                        };
                        self.record(t, verdict);
                        continue;
                    }
                    let day = trading_date(UnixNanos(t.observed_at));
                    if !self.setup_days.insert((t.symbol.clone(), day)) {
                        self.record(t, SetupVerdict::OnePerDay);
                        continue;
                    }
                    self.record(t, SetupVerdict::Notified);
                    let key = self.canonical(&t.episode_id).to_string();
                    self.setup_episodes.insert(key);
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
                State::CrossingProvisional => self.crossing(t, signals, &member),
                _ => {}
            }
        }
    }

    fn crossing(&mut self, t: &DetectionTransition, signals: bool, member: &impl Fn(&str) -> bool) {
        let is_member = member(&t.symbol);
        let price = t.trade.as_ref().map(|x| x.price);
        let at = t
            .trade
            .as_ref()
            .map_or(t.observed_at, |x| x.admit.max(t.observed_at));
        let exec = t.trade.as_ref().map(|x| x.exec);
        if !is_member && self.setup_episodes.contains(self.canonical(&t.episode_id)) {
            // A setup this module announced reaching its trigger: the setting-up notice's follow-up,
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
            self.learn(&r.symbol, ep, c.h0_bar_end);
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
            if e.family != Family::MicroPullback {
                continue;
            }
            self.learn(
                &e.symbol,
                &e.episode_id,
                e.anchors.as_ref().map(|a| a.h0_bar_end),
            );
            if e.warmup || e.state != State::Armed {
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
        wick: a.and_then(|a| a.wick.clone()),
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
        let none = |_: &str| None;
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
            wick: None,
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

    /// When the test transitions happen (`tr`'s `observed_at`).
    const AT: i64 = 1_761_200_000_000_000_000;
    const MIN: i64 = 60 * 1_000_000_000;

    /// Setting up: only stocks with context (a Runners stock within 30 minutes of being flagged,
    /// or a Ross member), the first setup per stock and day only, never a warm-up arming; the facts
    /// are the episode's own.
    #[test]
    fn pullback_setting_up_needs_context_and_fires_once() {
        let mut c = CueNotices::default();
        let member = |s: &str| s == "ROSS";
        // RUNR was flagged 5 minutes before the arming, LATE 40 minutes before.
        let since = |s: &str| match s {
            "RUNR" => Some(AT - 5 * MIN),
            "LATE" => Some(AT - 40 * MIN),
            _ => None,
        };
        let price = |_: &str, _: i64| Some(3_300_000);
        let runr = armed("p1", "RUNR");
        let ross = armed("p2", "ROSS");
        let bare = armed("p3", "BARE");
        let mut warm = armed("p4", "RUNR");
        warm.warmup = true;
        let late = armed("p5", "LATE");
        c.transitions_with(
            &[warm, runr.clone(), ross, bare, late],
            true,
            member,
            since,
            price,
        );
        // The same arming again, and a second setup on RUNR the same day: nothing more.
        let again = armed("p6", "RUNR");
        c.transitions_with(&[runr, again], true, member, since, price);
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

    /// The window starts at the stock's first row, not when this process first saw the list: a
    /// restart 20 minutes after the flag (the rows restored from the forward log) leaves 10.
    #[test]
    fn the_window_starts_at_the_flag_even_after_a_restart() {
        let member = |_: &str| false;
        let price = |_: &str, _: i64| None;
        // A fresh process at AT: the list arrives with each row's own flag time.
        let mut c = CueNotices::default();
        c.set_watched(&[("LATE".into(), AT - 20 * MIN), ("NEW".into(), AT)]);
        let watched = c.watched.clone();
        let since = |s: &str| watched.get(s).copied();
        let late = DetectionTransition {
            observed_at: AT + 15 * MIN,
            ..armed("l1", "LATE")
        };
        let new = DetectionTransition {
            observed_at: AT + 15 * MIN,
            ..armed("n1", "NEW")
        };
        c.transitions_with(&[late, new], false, member, since, price);
        let n = c.take();
        // 35 minutes after its flag: outside the window, though only 15 after the restart.
        assert_eq!(n.len(), 1, "{n:?}");
        assert_eq!(n[0].symbol, "NEW");
    }

    /// A restart mid-day: the new process restores what the old one announced
    /// ([`crate::notice_memory`]), so the stock's second setup of the day stays quiet and an
    /// announced episode is not announced again.
    #[test]
    fn a_restart_restores_the_first_setup_of_the_day() {
        let member = |s: &str| s == "ROSS";
        let since = |_: &str| Some(AT - MIN);
        let price = |_: &str, _: i64| None;
        let first = "live:d:INO:micro_pullback:1";
        let mut before = CueNotices::default();
        before.transitions_with(&[armed(first, "INO")], false, member, since, price);
        assert_eq!(before.take().len(), 1);
        // What the ledger recorded, read back by the next process.
        let memory = crate::notice_memory::NoticeMemory {
            seen: vec![(NoticeKind::Pullback, first.into())],
            setup_days: vec![("INO".into(), trading_date(UnixNanos(AT)))],
            setup_episodes: vec![first.into()],
            ..Default::default()
        };
        let mut after = CueNotices::default();
        after.restore(&memory);
        let second = DetectionTransition {
            observed_at: AT + 5 * MIN,
            ..armed("live:d:INO:micro_pullback:2", "INO")
        };
        after.transitions_with(&[second], false, member, since, price);
        assert!(
            after.take().is_empty(),
            "the day's first setup already went out"
        );
        // Another stock is unaffected.
        let other = armed("live:d:OTHR:micro_pullback:1", "OTHR");
        after.transitions_with(&[other], false, member, since, price);
        assert_eq!(after.take().len(), 1);
    }

    /// A setup a previous process announced comes back under a new id after a restart (the
    /// episode floors); recognized by its symbol and H0 bar, its crossing is not announced
    /// again, and a setup at another H0 is a new one.
    #[test]
    fn an_inherited_setup_is_not_announced_again_under_its_new_id() {
        let member = |s: &str| s == "ROSS";
        let old = "live:d:ROSS:micro_pullback:1";
        let memory = crate::notice_memory::NoticeMemory {
            seen: vec![(NoticeKind::RossCrossing, old.into())],
            inherited: vec![("ROSS".into(), 777, old.into())],
            ..Default::default()
        };
        let mut c = CueNotices::default();
        c.restore(&memory);
        let anchored = |ep: &str, h0: i64| {
            let mut t = tr(ep, "ROSS", State::Armed, State::CrossingProvisional);
            t.anchors = Some(MicroAnchors {
                h0_bar_end: h0,
                ..armed(ep, "ROSS").anchors.unwrap()
            });
            t
        };
        c.transitions_with(
            &[anchored("live:d:ROSS:micro_pullback:2", 777)],
            false,
            member,
            |_| None,
            |_, _| None,
        );
        assert!(c.take().is_empty(), "process 1 announced this crossing");
        c.transitions_with(
            &[anchored("live:d:ROSS:micro_pullback:3", 888)],
            false,
            member,
            |_| None,
            |_, _| None,
        );
        let n = c.take();
        assert_eq!(n.len(), 1, "{n:?}");
        assert_eq!(n[0].alert_id, "live:d:ROSS:micro_pullback:3");
    }

    /// The episode crossing its trigger five minutes after it armed (a separate event).
    fn crossed(t: DetectionTransition) -> DetectionTransition {
        DetectionTransition {
            from: Some(State::Armed),
            to: State::CrossingProvisional,
            observed_at: t.observed_at + 5 * MIN,
            ..t
        }
    }

    /// XTND 2026-10-26: a setup that crosses 2 s after its notice. Since 2026-10-30 the crossing is
    /// its own notice (every distinct event is delivered), for a Runners stock and a Ross member
    /// alike, as it is 31 s later.
    #[test]
    fn a_crossing_right_after_its_setup_is_its_own_notice() {
        let mut c = CueNotices::default();
        let member = |s: &str| s == "ROSS";
        let since = |s: &str| (s == "XTND" || s == "ROSS" || s == "SLOW").then_some(AT - MIN);
        let price = |_: &str, _: i64| None;
        let soon = |t: DetectionTransition, ns: i64| DetectionTransition {
            from: Some(State::Armed),
            to: State::CrossingProvisional,
            observed_at: t.observed_at + ns,
            ..t
        };
        let setups = [
            armed("x1", "XTND"),
            armed("r1", "ROSS"),
            armed("s1", "SLOW"),
        ];
        c.transitions_with(&setups, true, member, since, price);
        assert_eq!(c.take().len(), 3);
        let s = 1_000_000_000;
        c.transitions_with(
            &[
                soon(armed("x1", "XTND"), 2 * s),
                soon(armed("r1", "ROSS"), 2 * s),
                soon(armed("s1", "SLOW"), 31 * s),
            ],
            true,
            member,
            since,
            price,
        );
        let n = c.take();
        let kinds: Vec<_> = n.iter().map(|x| (x.symbol.as_str(), x.kind)).collect();
        assert_eq!(
            kinds,
            vec![
                ("XTND", NoticeKind::PullbackCrossing),
                ("ROSS", NoticeKind::RossCrossing),
                ("SLOW", NoticeKind::PullbackCrossing),
            ]
        );
        // Each once: the same crossing drained again is the same event.
        c.transitions_with(
            &[soon(armed("x1", "XTND"), 3 * s)],
            true,
            member,
            since,
            price,
        );
        assert!(c.take().is_empty());
    }

    /// The crossing of an announced setup: a Ross member's stays the Ross notice (never also a
    /// pullback one); a Runners stock's is the pullback follow-up, standing for the Signals crossing
    /// too. A crossing whose setup was not announced is the plain Signals one.
    #[test]
    fn pullback_crossing_follows_up_without_doubling_ross_or_signals() {
        let mut c = CueNotices::default();
        let member = |s: &str| s == "ROSS";
        let since = |s: &str| (s == "RUNR" || s == "ROSS" || s == "OLD").then_some(AT - MIN);
        let price = |_: &str, _: i64| None;
        c.transitions_with(
            &[armed("p1", "RUNR"), armed("p2", "ROSS")],
            true,
            member,
            since,
            price,
        );
        c.take();
        let unannounced = crossed(armed("p9", "OLD"));
        c.transitions_with(
            &[
                crossed(armed("p1", "RUNR")),
                crossed(armed("p2", "ROSS")),
                unannounced,
            ],
            true,
            member,
            since,
            price,
        );
        let n = c.take();
        let kinds: Vec<_> = n.iter().map(|x| (x.symbol.as_str(), x.kind)).collect();
        assert_eq!(
            kinds,
            vec![
                ("RUNR", NoticeKind::PullbackCrossing),
                ("ROSS", NoticeKind::RossCrossing),
                ("OLD", NoticeKind::SignalsCrossing),
            ]
        );
        assert!(matches!(&n[0].detail, Some(NoticeDetail::Pullback(f)) if f.also_signals));
        // Detection Panel off: the follow-up stands for nothing else.
        let mut off = CueNotices::default();
        off.transitions_with(&[armed("p3", "RUNR")], false, member, since, price);
        off.take();
        off.transitions_with(&[crossed(armed("p3", "RUNR"))], false, member, since, price);
        let n = off.take();
        assert!(matches!(&n[0].detail, Some(NoticeDetail::Pullback(f)) if !f.also_signals));
        // An invalidated setup pushes nothing.
        let mut gone = armed("p3", "RUNR");
        gone.from = Some(State::Armed);
        gone.to = State::Invalidated;
        off.transitions_with(&[gone], false, member, since, price);
        assert!(off.take().is_empty());
    }
}

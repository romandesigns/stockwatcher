//! One pass over a symbol's recorded day: the chart engine (verdicts, halts, LULD bands, 1-minute
//! bars) and the production context engine with its micro-pullback detector, fed the way the live
//! market feeds them, plus the cue notices' own decider recording why each armed setup did or did
//! not notify (`crate::cue_notices::SetupVerdict`).
//!
//! # Feeding
//!
//! Exactly the reference analysis of INO 2026-10-30 (`ino_replay.rs`, the accepted reproduction
//! of the live run): every event in recorded (arrival) order; a print is marked by its delivery
//! time (`receive_ts`, falling back to the execution time on a day without one, i.e. an import);
//! between prints the clock ticks every second at `+250 ms`, as the live market's idle ticks do,
//! so barriers with no print of their own are still decided. The print's verdict is the chart
//! engine's, as in the live market and the restart restore (`crate::history`).
//!
//! # What the live run had that a replay does not, and how it is modelled
//!
//! - **Runners context.** The engine watches the symbol (micro-pullback for a Runners row) from
//!   its first Runners notice of the day (the ledger), as `Engine::set_watched` did live.
//! - **Restarts.** At every boot of the day (the admission journal's segment headers, else the
//!   server log's `listening` lines) the live process started over: its cue decider was new (so
//!   the 30-minute window counts from the boot and the one-per-day rule forgets earlier notices),
//!   and a restored prefix marked the symbol `restored_through` its last recorded delivery
//!   before the boot (warm-up). Both are reproduced here, as the server of THAT day restarted
//!   ([`RestartModel`]): until 2026-11-01 a micro-pullback whose impulse began inside the
//!   restored prefix was warm-up however live its arming, and a new process forgot the day's
//!   notices; from then on the arming moment decides warm-up (`stock_context::detect::micro`) and
//!   the notice memory (`crate::notice_memory`) carries the one-per-day rule and the Runners
//!   window across the restart. A replay judging an old day by today's rules would call a setup
//!   that day's server really did mute "notified", and the ledger would contradict it.
//! - **The live journal wins.** Where the day's cue transition log has the episode, its recorded
//!   `warmup` flag replaces the modelled one: the restore's proof (admission journal) decided
//!   whether a prefix was warm, and the log is what actually happened.
//! - **Ross membership** needs the day's pillar evidence (float, news, closes…), which this pass
//!   does not load: a symbol is a member when the engine says so or the ledger holds a Ross
//!   notice for it that day. Stated in the package (`member_basis`).
//! - **The Detection Panel** is off in production (`DETECTION_PANEL`, default off), so there are
//!   no Signals notices here either.

use std::collections::HashMap;

use chart_engine::wire::{CandleWire, ChartUpdate, HaltWire, LuldWire};
use chart_engine::{ChartEngine, Timeframe};
use market_events::session::SessionKind;
use market_events::{MarketEvent, UnixNanos};
use stock_context::{
    Coverage, DetectionTransition, Engine, Fidelity, PanelPolicy, Policy, RunInfo, Verdict,
};

use crate::cue_notices::{CueNotices, SetupDecision};
use crate::quotes::Counted;

const SEC: i64 = 1_000_000_000;
/// The idle tick's offset into each second (module docs, "Feeding").
const TICK_OFFSET: i64 = 250_000_000;
/// How long the clock runs on after the last print of a finished day: long enough to settle the
/// last episodes (expiry, the last bars), as the reference run did.
const RUN_OUT: i64 = 600 * SEC;

/// One print as the page measures it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Print {
    pub exec: i64,
    pub admit: i64,
    pub price: i64,
    pub size: u64,
    pub ohlc: bool,
    pub volume: bool,
    pub session: Option<SessionKind>,
}

/// How the server that ran the day behaved across a restart (module docs, "Restarts").
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RestartModel {
    /// Before the notice memory and the arming warm-up rule: a boot starts a new decider (window
    /// and one-per-day memory start over) and a micro-pullback whose L0 bar starts inside the
    /// restored prefix is warm-up (the old `micro::search` rule, `r >= l0_bar_end - bar`).
    #[default]
    Forgetful,
    /// With them: the decider keeps what the previous process announced, the Runners window
    /// keeps its first flag time, and warm-up is the engine's own (by the arming moment).
    Remembering,
}

impl RestartModel {
    /// The first ET trading day whose server restarts with the notice memory and the arming
    /// warm-up rule. Releases never restart the server 04:00-20:00 ET on a trading day
    /// (docs/CONVENTIONS.md, "Deploy window"), so the earliest is the trading day after the
    /// release. Move this date if that release ships later: it decides how old days are judged.
    pub const REMEMBERING_FROM: (i32, u32, u32) = (2026, 11, 2);

    /// The model for `day`.
    pub fn for_day(day: chrono::NaiveDate) -> RestartModel {
        let (y, m, d) = Self::REMEMBERING_FROM;
        match chrono::NaiveDate::from_ymd_opt(y, m, d) {
            Some(from) if day >= from => RestartModel::Remembering,
            _ => RestartModel::Forgetful,
        }
    }
}

/// What the pass needs besides the events.
#[derive(Clone, Debug, Default)]
pub struct ReplayInput {
    pub symbol: String,
    /// The symbol's first Runners notice that day (UTC ns): the engine watches it from then.
    pub watch_from: Option<i64>,
    /// Boots of the server that day (UTC ns, ascending).
    pub boots: Vec<i64>,
    /// Episode id -> the `warmup` flag the live cue transition log recorded for it.
    pub live_warm: HashMap<String, bool>,
    /// The ledger names the symbol a Ross member that day.
    pub ledger_member: bool,
    /// Stop the clock here (UTC ns; today's page: now). `None`: run out the finished day.
    pub until: Option<i64>,
    /// How that day's server restarted ([`RestartModel::for_day`]).
    pub restarts: RestartModel,
}

/// What the pass found.
#[derive(Debug, Default)]
pub struct ReplayOut {
    /// Every trade, in exec order.
    pub prints: Vec<Print>,
    pub transitions: Vec<DetectionTransition>,
    pub decisions: Vec<SetupDecision>,
    pub halts: Vec<HaltWire>,
    pub luld: Vec<LuldWire>,
    pub bars: Vec<CandleWire>,
    pub events: u64,
    /// The engine itself judged the symbol a Ross member at some point.
    pub engine_member: bool,
}

struct Pass {
    symbol: String,
    chart: ChartEngine,
    ctx: Engine,
    cue: CueNotices,
    input: ReplayInput,
    /// Next boot to apply (index into `input.boots`).
    next_boot: usize,
    watching: bool,
    /// When this process first saw the symbol as a Runners row (the cue window's start).
    since: Option<i64>,
    last_admit_before: Option<i64>,
    /// The restored prefix's end since the latest boot (the old warm-up rule reads it).
    restored_through: Option<i64>,
    /// The detection grid's bar (`Policy::current().body.grid`).
    bar_ns: i64,
    out: ReplayOut,
    seen_decisions: std::collections::HashSet<String>,
}

impl Pass {
    fn new(input: ReplayInput) -> Pass {
        let run = RunInfo {
            // "live", like the live board, so episode ids match the live journal's.
            run_id: "live".into(),
            source: "live".into(),
            fidelity: Fidelity::RecordedLive,
            numerator_coverage: Coverage::Unverified,
        };
        let policy = Policy::current();
        let bar_ns = policy.body.grid.bar_secs * SEC;
        Pass {
            symbol: input.symbol.clone(),
            chart: ChartEngine::default(),
            ctx: crate::context::production_engine(policy, PanelPolicy::v1(), run),
            cue: CueNotices::recording(),
            input,
            next_boot: 0,
            watching: false,
            since: None,
            last_admit_before: None,
            restored_through: None,
            bar_ns,
            out: ReplayOut::default(),
            seen_decisions: Default::default(),
        }
    }

    /// The scheduled changes due by `now`: the Runners flag, then any boot.
    fn schedule(&mut self, now: i64) {
        if !self.watching && self.input.watch_from.is_some_and(|w| now >= w) {
            self.watching = true;
            self.since = Some(now);
            self.ctx.set_watched(vec![self.symbol.clone()]);
        }
        while let Some(&boot) = self.input.boots.get(self.next_boot) {
            if now < boot {
                break;
            }
            self.next_boot += 1;
            // A new process and the restored prefix warm. Before the notice memory, also a new
            // decider (its window and one-per-day memory start over) and the symbol listed again
            // from the boot; with it, the decider restores what the previous one announced and
            // the Runners row keeps its first flag time ([`RestartModel`]).
            self.drain();
            if self.input.restarts == RestartModel::Forgetful {
                self.cue = CueNotices::recording();
                if self.watching {
                    self.since = Some(boot);
                }
            }
            if let Some(rt) = self.last_admit_before {
                self.ctx.set_restored_through(&self.symbol, Some(rt));
                self.restored_through = Some(rt);
            }
        }
    }

    fn advance(&mut self, now: i64) {
        self.schedule(now);
        let _ = self.ctx.advance_to(UnixNanos(now));
        let ups = self.chart.advance_clock(UnixNanos(now));
        self.take_updates(&ups);
        self.drain();
    }

    /// Ticks the idle clock through every second boundary before `admit`.
    fn tick_until(&mut self, clock: i64, admit: i64) {
        if clock <= 0 {
            return;
        }
        let mut s = (clock.div_euclid(SEC) + 1) * SEC + TICK_OFFSET;
        while s < admit {
            self.advance(s);
            s += SEC;
        }
    }

    fn take_updates(&mut self, ups: &[ChartUpdate]) {
        for u in ups {
            match u {
                ChartUpdate::Halt(h) if h.symbol == self.symbol => {
                    match self.out.halts.iter_mut().find(|x| x.start == h.halt.start) {
                        Some(x) => *x = h.halt.clone(),
                        None => self.out.halts.push(h.halt.clone()),
                    }
                }
                ChartUpdate::Luld(l) if l.symbol == self.symbol => {
                    self.out.luld.push(l.point.clone())
                }
                _ => {}
            }
        }
    }

    /// The engine's setup log through the cue decider (module docs).
    fn drain(&mut self) {
        let mut ts = self.ctx.take_setup_log();
        if ts.is_empty() {
            return;
        }
        for t in &mut ts {
            // The old rule is wider than the engine's (an arming inside the prefix implies an L0
            // inside it), so OR-ing it on reproduces it exactly.
            if self.input.restarts == RestartModel::Forgetful
                && t.family == stock_context::Family::MicroPullback
                && let (Some(rt), Some(a)) = (self.restored_through, t.anchors.as_ref())
                && rt >= a.l0_bar_end - self.bar_ns
            {
                t.warmup = true;
            }
            if let Some(w) = self.input.live_warm.get(&t.episode_id) {
                t.warmup = *w;
            }
        }
        let since = self.since;
        let ledger_member = self.input.ledger_member;
        let engine = &self.ctx;
        let engine_member = std::cell::Cell::new(false);
        self.cue.transitions_with(
            &ts,
            false,
            |s| {
                let m = crate::cue_notices::member(engine, s);
                engine_member.set(engine_member.get() | m);
                m || ledger_member
            },
            |_| since,
            |_, _| None,
        );
        self.out.engine_member |= engine_member.get();
        let _ = self.cue.take();
        for d in self.cue.take_setup_decisions() {
            // The first arming across restarts too: a decider started by a boot sees a re-arm of
            // an episode the previous one already judged.
            if self.seen_decisions.insert(d.episode_id.clone()) {
                self.out.decisions.push(d);
            }
        }
        self.out.transitions.extend(ts);
    }

    fn apply(&mut self, e: &MarketEvent) {
        let admit = market_providers::recorder::receive_time(e).0;
        let (ups, rev) = self.chart.apply_revising(e);
        match e {
            MarketEvent::Trade(t) => {
                let counted = match ups.first() {
                    Some(ChartUpdate::Tape(row)) => Counted::from_tape(&row.counted),
                    _ => Counted::from_effect(t),
                };
                let _ = self.ctx.apply(
                    t,
                    Verdict {
                        ohlc: counted.ohlc,
                        volume: counted.volume,
                    },
                    UnixNanos(admit),
                );
                self.out.prints.push(Print {
                    exec: t.event_ts.0,
                    admit,
                    price: t.price.micros(),
                    size: t.size,
                    ohlc: counted.ohlc,
                    volume: counted.volume,
                    session: t.session.map(|s| s.kind),
                });
                self.last_admit_before =
                    Some(self.last_admit_before.map_or(admit, |a| a.max(admit)));
            }
            MarketEvent::Correction(_) | MarketEvent::Cancel(_) => {
                crate::context::apply_revision_to(&mut self.ctx, e, rev.as_ref(), UnixNanos(admit));
            }
            _ => {}
        }
        self.take_updates(&ups);
    }
}

/// Runs the pass over `events` (one symbol's day, recorded order). Pure but for the engines: the
/// same events and input give the same result.
pub fn run(events: impl IntoIterator<Item = MarketEvent>, input: ReplayInput) -> ReplayOut {
    let until = input.until;
    let mut p = Pass::new(input);
    let mut clock: i64 = 0;
    for e in events {
        if e.symbol() != Some(p.symbol.as_str()) {
            continue;
        }
        let admit = market_providers::recorder::receive_time(&e).0;
        if until.is_some_and(|u| admit > u) {
            break;
        }
        p.out.events += 1;
        p.tick_until(clock, admit);
        p.schedule(admit);
        p.apply(&e);
        p.drain();
        clock = clock.max(admit);
    }
    if clock > 0 {
        let end = match until {
            Some(u) => u.min(clock + RUN_OUT),
            None => clock + RUN_OUT,
        };
        let mut s = (clock.div_euclid(SEC) + 1) * SEC;
        while s <= end {
            p.advance(s);
            s += SEC;
        }
    }
    p.drain();
    p.out.bars = p
        .chart
        .day_candles(&p.symbol, Timeframe::M1)
        .unwrap_or_default();
    p.out.prints.sort_by_key(|x| (x.exec, x.admit));
    p.out
}

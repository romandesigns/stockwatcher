//! One symbol-day through the production pipeline (protocol section 3).
//!
//! `ReplayCore` over the symbol's archived stream gives the event / 1 s mark sequence a
//! production replay delivers. Every event goes through a `ChartEngine` (the print's `counted`
//! verdict, halts) and then into one `stock_context::Engine` per `micro_pullback_v0` variant, with
//! the same glue `crates/server/src/{market,context}.rs` uses. `bull_flag_v0` is the real
//! `BullFlag` stepped at every barrier with the inputs `Engine::publish_panel` builds for a
//! member, read through `Engine::setup_inputs`. Nothing in either rule is reimplemented here.
//!
//! The glue that is copied rather than shared (it is `pub(crate)` in the server):
//! `revision_of` (server `context.rs`), `revision_admit` (server `market.rs`) and
//! `Counted::from_tape` (server `quotes.rs`).

use chart_engine::wire::ChartUpdate;
use chart_engine::{ChartEngine, Revision};
use chrono::NaiveDate;
use market_archive::{Archive, DayData};
use market_events::conditions::trade_effect;
use market_events::session::{SessionKind, classify, trading_date};
use market_events::{MarketEvent, UnixNanos};
use market_replay::source::{EventSource, EventStream};
use market_replay::{FeedItem, ReplayCore, marks::DEFAULT_MARK_STEP_NS};
use std::collections::{BTreeMap, HashSet, VecDeque};
use std::sync::Arc;
use stock_context::setup::{Bar, BullFlag, SetupState, SetupView, Step};
use stock_context::{
    BoundedLog, CorrectedPrint, Coverage, DayHigh, DetectionPolicy, Engine, Family, Fidelity,
    LastPrint, PanelPolicy, Policy, PrintRevision, RevisionTarget, RunInfo, SetupInputs,
    SetupPolicy, State, Verdict,
};

pub const SEC: i64 = 1_000_000_000;
pub const BAR: i64 = 15 * SEC;
pub const SETTLE: i64 = 2 * SEC;
/// The research band, micro-dollars.
pub const BAND: (i64, i64) = (150_000, 25_000_000);

pub fn in_band(p: i64) -> bool {
    (BAND.0..=BAND.1).contains(&p)
}

/// `pre` / `regular` / `after` / `overnight` / `none` for the instant `t`.
pub fn session_of(t: i64) -> &'static str {
    match classify(UnixNanos(t)).map(|s| s.kind) {
        Some(SessionKind::Pre) => "pre",
        Some(SessionKind::Regular) => "regular",
        Some(SessionKind::After) => "after",
        Some(SessionKind::Overnight) => "overnight",
        None => "none",
    }
}

pub fn scored_session(t: i64) -> bool {
    matches!(session_of(t), "pre" | "regular")
}

fn ceil_to(t: i64, step: i64) -> i64 {
    t.div_euclid(step) * step + if t.rem_euclid(step) == 0 { 0 } else { step }
}

/// One symbol of one archive day, read through the archive's own readers (the import when the
/// day is final, else its sealed live recording), exactly as `ArchiveDaySource` opens it.
struct OneSymbol {
    archive: Archive,
    day: NaiveDate,
    symbol: String,
    live: bool,
}

impl EventSource for OneSymbol {
    fn open(&self) -> Result<Vec<EventStream>, String> {
        let r = if self.live {
            self.archive.live_reader(self.day, &self.symbol, None)
        } else {
            self.archive.reader(self.day, &self.symbol)
        }
        .map_err(|e| e.to_string())?;
        Ok(vec![Box::new(
            r.map(|x| x.map(|v| v.event).map_err(|e| e.to_string())),
        )])
    }
    fn day(&self) -> NaiveDate {
        self.day
    }
    fn symbols(&self) -> Vec<String> {
        vec![self.symbol.clone()]
    }
    fn fixture(&self) -> bool {
        false
    }
    fn label(&self) -> String {
        format!("research {} {}", self.day, self.symbol)
    }
}

/// Which version of a day the archive holds: `Some(false)` final import, `Some(true)` sealed live
/// recording, `None` nothing replayable.
pub fn day_version(archive: &Archive, day: NaiveDate) -> Option<bool> {
    match archive.day_data(day) {
        Some(DayData::Final) => Some(false),
        Some(DayData::Provisional { sealed: true }) => Some(true),
        _ => None,
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Print {
    pub exec: i64,
    pub admit: i64,
    pub price: i64,
}

/// A completed engine bar of the pre or regular session.
#[derive(Clone, Copy, Debug)]
pub struct BarRec {
    pub end: i64,
    /// `(open, high, low, close)` when it had an OHLC-counted print.
    pub ohlc: Option<(i64, i64, i64, i64)>,
}

/// One signal before scoring.
#[derive(Clone, Debug)]
pub struct RawSignal {
    /// `bull`, `micro`, `b0`, `b0all`.
    pub det: &'static str,
    pub var: String,
    /// `trigger` or `cue`.
    pub kind: &'static str,
    pub ep: String,
    pub t0: i64,
    /// Prints executed before this belong to the decision (`P0`); the success bars start here.
    pub bar_end: i64,
    pub cue_t: Option<i64>,
    pub armed_t: Option<i64>,
}

pub struct SymDay {
    pub events: u64,
    pub trades: u64,
    pub eligible: bool,
    /// Prints (OHLC-counted, chart verdict), sorted by execution.
    pub prints: Vec<Print>,
    pub bars: Vec<BarRec>,
    pub signals: Vec<RawSignal>,
    /// Transitions or entries the bounded journals dropped (must be 0).
    pub dropped: u64,
    /// The first micro variant's episodes (pullback-context study).
    pub micro_tracks: Vec<(String, Track)>,
}

#[derive(Default, Clone, Debug)]
pub struct Track {
    pub armed: Option<i64>,
    pub cue: Option<i64>,
    pub trig: Option<(i64, i64)>,
    // Pullback-context study (docs/research/pullback-context): the armed levels, the crossing
    // itself (not the near cue), and the counted day volume at arming (completed bars).
    pub trigger: Option<i64>,
    pub stop: Option<i64>,
    pub impulse_bp: Option<i64>,
    pub depth_milli: Option<i64>,
    pub armed_vol: Option<u128>,
    pub cross: Option<(i64, i64)>,
}

struct Pipeline<'a> {
    symbol: String,
    chart: ChartEngine,
    engines: Vec<Engine>,
    micro_ids: Vec<String>,
    bull_cfg: &'a [(String, SetupPolicy)],
    bulls: Vec<BullFlag>,
    bull_prev: Vec<Option<SetupState>>,
    bull_eps: Vec<BTreeMap<String, Track>>,
    micro_eps: Vec<BTreeMap<String, Track>>,
    /// Micro episodes currently armed / crossing per variant (the near check needs a snapshot).
    micro_live: Vec<HashSet<String>>,
    setup_log: BoundedLog<stock_context::SetupLogEntry>,
    next_b: Option<i64>,
    prints: Vec<Print>,
    bars: Vec<BarRec>,
    eligible: bool,
    trades: u64,
    events: u64,
    // B0 state.
    b0_first_bar: Option<i64>,
    b0_vol: u128,
    b0_high: Option<i64>,
    b0_done: bool,
    b0_last_all: Option<i64>,
    b0: Vec<RawSignal>,
    prefix: String,
}

fn new_engine(day: NaiveDate, det: Option<&DetectionPolicy>) -> Engine {
    // As the server's replay board: run `replay:<day>`, reconstructed history, unverified coverage.
    let run = RunInfo {
        run_id: format!("replay:{day}"),
        source: "replay".to_string(),
        fidelity: Fidelity::ReconstructedHistory,
        numerator_coverage: Coverage::Unverified,
    };
    // Main has one pullback detector (614ffe0): no separate setup policy any more.
    let e = Engine::new(Policy::current(), run).with_panel_policy(PanelPolicy::v1());
    match det {
        Some(p) => e.with_detection_policy(p.clone()),
        None => e,
    }
}

/// Server `quotes.rs` `Counted::from_tape`.
fn from_tape(counted: &str) -> Verdict {
    match counted {
        "ohlc_and_volume" => Verdict {
            ohlc: true,
            volume: true,
        },
        "volume_only" => Verdict {
            ohlc: false,
            volume: true,
        },
        _ => Verdict {
            ohlc: false,
            volume: false,
        },
    }
}

/// Server `market.rs` `revision_admit`.
fn revision_admit(e: &MarketEvent) -> UnixNanos {
    match e {
        MarketEvent::Correction(c) => c.receive_ts.unwrap_or(c.event_ts),
        MarketEvent::Cancel(c) => c.receive_ts.unwrap_or(c.event_ts),
        other => other.event_ts(),
    }
}

/// Server `context.rs` `revision_of`.
fn revision_of(e: &MarketEvent, resolved: Option<&Revision>) -> Option<PrintRevision> {
    let (symbol, target, corrected) = match e {
        MarketEvent::Correction(c) => (
            c.symbol.clone(),
            RevisionTarget {
                id: c.original_trade_id(),
                price: c.original_price.micros(),
                size: c.original_size,
            },
            Some(CorrectedPrint {
                price: c.corrected_price.micros(),
                size: c.corrected_size,
                verdict: {
                    let eff = trade_effect(&c.corrected_conditions, c.tape);
                    Verdict {
                        ohlc: eff.updates_ohlc,
                        volume: eff.updates_volume,
                    }
                },
            }),
        ),
        MarketEvent::Cancel(c) => (
            c.symbol.clone(),
            RevisionTarget {
                id: c.target_trade_id(),
                price: c.price.micros(),
                size: c.size,
            },
            None,
        ),
        _ => return None,
    };
    Some(PrintRevision {
        symbol,
        target,
        corrected,
        last: resolved.and_then(|r| r.last).map(|(p, ts)| LastPrint {
            exec: ts.nanos(),
            price: p.micros(),
        }),
        day_high: resolved.and_then(|r| r.day_high).map(|h| DayHigh {
            price: h.price.micros(),
            complete: h.complete,
        }),
    })
}

impl Pipeline<'_> {
    fn halts(&mut self, ups: &[ChartUpdate]) {
        for u in ups {
            if let ChartUpdate::Halt(h) = u {
                for e in &mut self.engines {
                    e.set_halted(&h.symbol, h.halt.end.is_none());
                }
            }
        }
    }

    fn collect(&mut self) {
        for k in 0..self.engines.len() {
            if self.micro_ids.get(k).is_none() {
                continue;
            }
            for t in self.engines[k].take_detection_transitions() {
                if t.family != Family::MicroPullback {
                    continue;
                }
                let ep = self.micro_eps[k].entry(t.episode_id.clone()).or_default();
                match t.to {
                    State::Armed => {
                        ep.armed.get_or_insert(t.observed_at);
                        if let Some(a) = t.anchors.as_ref()
                            && ep.trigger.is_none()
                        {
                            ep.trigger = Some(a.h0);
                            ep.stop = Some(a.pullback_low);
                            ep.impulse_bp = Some(a.impulse_bp);
                            ep.depth_milli = Some(a.depth_milli);
                        }
                        ep.armed_vol.get_or_insert(self.b0_vol);
                        self.micro_live[k].insert(t.episode_id.clone());
                    }
                    State::CrossingProvisional => {
                        ep.cue.get_or_insert(t.observed_at);
                        if ep.cross.is_none() {
                            let px = t.trade.as_ref().map_or(0, |x| x.price);
                            ep.cross = Some((t.observed_at, px));
                        }
                    }
                    State::TriggerObserved => {
                        ep.trig
                            .get_or_insert((t.observed_at, t.bar_end.unwrap_or(t.observed_at)));
                    }
                    _ => {}
                }
                if t.to.is_terminal() {
                    self.micro_live[k].remove(&t.episode_id);
                }
            }
        }
    }

    fn mark(&mut self, m: i64) {
        let b0 = *self.next_b.get_or_insert(ceil_to(m, BAR));
        let mut b = b0;
        while b + SETTLE <= m {
            self.barrier(b);
            b += BAR;
        }
        self.next_b = Some(b);
        let ups = self.chart.advance_clock(UnixNanos(m));
        self.halts(&ups);
        for e in &mut self.engines {
            let _ = e.advance_to(UnixNanos(m));
        }
        self.collect();
    }

    fn barrier(&mut self, b: i64) {
        let tau = b + SETTLE;
        for e in &mut self.engines {
            let _ = e.advance_to(UnixNanos(tau));
        }
        self.collect();
        // Micro EARLY CUE (near): the engine's own snapshot assessment, at this barrier.
        for k in 0..self.micro_ids.len() {
            if self.micro_live[k].is_empty() {
                continue;
            }
            if let Some(snap) = self.engines[k].detection_snapshot() {
                for ep in &snap.active {
                    if ep.family == Family::MicroPullback
                        && ep.current.as_ref().and_then(|c| c.near_trigger) == Some(true)
                    {
                        self.micro_eps[k]
                            .entry(ep.episode_id.clone())
                            .or_default()
                            .cue
                            .get_or_insert(tau);
                    }
                }
            }
        }
        let date = trading_date(UnixNanos(b));
        let Some(inp) = self.engines[0].setup_inputs(&self.symbol) else {
            return;
        };
        if inp.date != Some(date) {
            // `publish_panel`: not this date's facts, the tracker starts over.
            for bf in &mut self.bulls {
                if !bf.is_idle() {
                    bf.leave();
                }
            }
            return;
        }
        // The bar completing now (engine bars), for B0, outcomes and the reference runs.
        let fresh = inp.bars.back().filter(|t| t.end == b).copied();
        let views = bull_views(
            &inp,
            b,
            &mut self.bulls,
            self.bull_cfg,
            &self.symbol,
            &self.prefix,
            &mut self.setup_log,
        );
        for (k, view) in views.into_iter().enumerate() {
            let prev = self.bull_prev[k].replace(view.state);
            if let Some(id) = view.setup_id.as_ref() {
                let ep = self.bull_eps[k].entry(id.clone()).or_default();
                match view.state {
                    SetupState::Forming => {
                        ep.armed.get_or_insert(tau);
                    }
                    SetupState::Near | SetupState::Crossing => {
                        ep.armed.get_or_insert(tau);
                        ep.cue.get_or_insert(tau);
                    }
                    SetupState::Triggered if prev != Some(SetupState::Triggered) => {
                        ep.trig.get_or_insert((tau, b));
                    }
                    _ => {}
                }
            }
        }
        let _ = self.setup_log.take();
        if let Some(bar) = fresh
            && scored_session(b - 1)
        {
            self.b0_step(b, &bar);
            self.bars.push(BarRec {
                end: b,
                ohlc: bar.ohlc.map(|o| (o.open, o.high, o.low, o.close)),
            });
        }
    }

    /// B0 (protocol section 5) on the engine bar completing at `b`.
    fn b0_step(&mut self, b: i64, bar: &Bar) {
        let tau = b + SETTLE;
        if let (Some(f), Some(o), Some(hi)) = (self.b0_first_bar, bar.ohlc, self.b0_high) {
            let slots_prev = (b - BAR - f) / BAR + 1;
            let enough = (b - f) / BAR >= 20 && slots_prev > 0;
            if enough
                && o.high > hi
                && bar.volume as u128 * slots_prev as u128 >= 2 * self.b0_vol
                && self.b0_vol > 0
            {
                let sig = |det: &'static str| RawSignal {
                    det,
                    var: "B0".into(),
                    kind: "trigger",
                    ep: format!("{}:{det}:{b}", self.prefix),
                    t0: tau,
                    bar_end: b,
                    cue_t: None,
                    armed_t: None,
                };
                if !self.b0_done {
                    self.b0_done = true;
                    self.b0.push(sig("b0"));
                }
                if self.b0_last_all.is_none_or(|l| tau - l >= 120 * SEC) {
                    self.b0_last_all = Some(tau);
                    self.b0.push(sig("b0all"));
                }
            }
        }
        if self.b0_first_bar.is_none() && bar.volume > 0 {
            self.b0_first_bar = Some(b);
        }
        if self.b0_first_bar.is_some() {
            self.b0_vol += bar.volume as u128;
        }
        if let Some(o) = bar.ohlc {
            self.b0_high = Some(self.b0_high.map_or(o.high, |h| h.max(o.high)));
        }
    }

    fn event(&mut self, e: MarketEvent) {
        self.events += 1;
        match &e {
            MarketEvent::Trade(t) => {
                self.trades += 1;
                let (ups, _) = self.chart.apply_revising(&e);
                if let Some(ChartUpdate::Tape(row)) = ups.first() {
                    if std::env::var("SR_DEBUG").is_ok() {
                        eprintln!("COUNTED {} {} {:?}", t.symbol, row.counted, row.reason);
                    }
                    let v = from_tape(&row.counted);
                    let admit = t.receive_ts.unwrap_or(t.event_ts);
                    for en in &mut self.engines {
                        let _ = en.apply(t, v, admit);
                    }
                    if v.ohlc {
                        let p = t.price.micros();
                        self.prints.push(Print {
                            exec: t.event_ts.nanos(),
                            admit: admit.nanos(),
                            price: p,
                        });
                        if in_band(p)
                            && matches!(
                                t.session.map(|s| s.kind),
                                Some(SessionKind::Pre | SessionKind::Regular)
                            )
                        {
                            self.eligible = true;
                        }
                    }
                }
                self.halts(&ups);
                self.collect();
            }
            MarketEvent::Correction(_) | MarketEvent::Cancel(_) => {
                let (ups, rev) = self.chart.apply_revising(&e);
                let at = revision_admit(&e);
                if let Some(r) = revision_of(&e, rev.as_ref()) {
                    for en in &mut self.engines {
                        let _ = en.apply_revision(&r, at);
                    }
                }
                self.halts(&ups);
                self.collect();
            }
            MarketEvent::Status(s) => {
                let at = s.receive_ts.unwrap_or(s.event_ts);
                for en in &mut self.engines {
                    let _ = match s.kind {
                        market_events::StatusKind::Halt => {
                            en.interrupt_detection(s.symbol.as_deref(), at, "halt")
                        }
                        market_events::StatusKind::Resume => {
                            en.resume_detection(s.symbol.as_deref(), at)
                        }
                        _ => Vec::new(),
                    };
                }
                let (ups, _) = self.chart.apply_revising(&e);
                self.halts(&ups);
                self.collect();
            }
            MarketEvent::Quote(_) => {}
        }
    }
}

/// Steps every `bull_flag_v0` tracker at barrier `b` exactly as `Engine::publish_panel` steps a
/// member's (same `Step` fields, no bars and a reset while the inputs are untrusted). The caller
/// has already checked that `inp` is for the barrier's trading date (`publish_panel` resets the
/// tracker otherwise).
pub fn bull_views(
    inp: &SetupInputs<'_>,
    b: i64,
    bulls: &mut [BullFlag],
    cfg: &[(String, SetupPolicy)],
    symbol: &str,
    prefix: &str,
    log: &mut BoundedLog<stock_context::SetupLogEntry>,
) -> Vec<SetupView> {
    let no_bars: VecDeque<Bar> = VecDeque::new();
    let bars: &VecDeque<Bar> = if inp.untrusted { &no_bars } else { inp.bars };
    let day_slots = inp
        .first_bar
        .filter(|&f| f <= b)
        .map_or(0, |f| ((b - f) / BAR + 1) as u64);
    let mut out = Vec::with_capacity(bulls.len());
    for (bf, (_, policy)) in bulls.iter_mut().zip(cfg) {
        if inp.untrusted && !bf.is_idle() {
            bf.leave();
        }
        let mut view = bf.step(
            policy,
            BAR / SEC,
            &Step {
                bar_end: b,
                cutoff: b + SETTLE,
                bar_ns: BAR,
                bars,
                last_price: inp.last_price,
                halted: inp.halted,
                day_volume: inp.day_volume,
                day_slots,
                symbol,
                prefix,
            },
            log,
        );
        if inp.untrusted {
            view.reason = if inp.history_pending {
                "history_pending"
            } else {
                "inputs_untrusted"
            };
        }
        out.push(view);
    }
    out
}

pub struct Variants {
    pub bulls: Vec<(String, SetupPolicy)>,
    pub micros: Vec<(String, DetectionPolicy)>,
}

pub fn run_symbol_day(
    archive: &Archive,
    day: NaiveDate,
    symbol: &str,
    live: bool,
    v: &Variants,
) -> Result<SymDay, String> {
    let src = Arc::new(OneSymbol {
        archive: archive.clone(),
        day,
        symbol: symbol.to_string(),
        live,
    });
    let mut core = ReplayCore::new(src, DEFAULT_MARK_STEP_NS)?;
    let mut engines: Vec<Engine> = v
        .micros
        .iter()
        .map(|(_, p)| new_engine(day, Some(p)))
        .collect();
    if engines.is_empty() {
        engines.push(new_engine(day, None));
    }
    let nb = v.bulls.len();
    let nm = v.micros.len();
    let mut p = Pipeline {
        symbol: symbol.to_string(),
        chart: ChartEngine::default(),
        engines,
        micro_ids: v.micros.iter().map(|(id, _)| id.clone()).collect(),
        bull_cfg: &v.bulls,
        bulls: (0..nb).map(|_| BullFlag::default()).collect(),
        bull_prev: vec![None; nb],
        bull_eps: (0..nb).map(|_| BTreeMap::new()).collect(),
        micro_eps: (0..nm).map(|_| BTreeMap::new()).collect(),
        micro_live: (0..nm).map(|_| HashSet::new()).collect(),
        setup_log: BoundedLog::new(1 << 16),
        next_b: None,
        prints: Vec::new(),
        bars: Vec::new(),
        eligible: false,
        trades: 0,
        events: 0,
        b0_first_bar: None,
        b0_vol: 0,
        b0_high: None,
        b0_done: false,
        b0_last_all: None,
        b0: Vec::new(),
        prefix: format!("replay:{day}:{symbol}"),
    };
    let mut items = Vec::new();
    loop {
        items.clear();
        let more = core.step(&mut items)?;
        for it in items.drain(..) {
            match it {
                FeedItem::Advance(m) => p.mark(m.nanos()),
                FeedItem::Event(e) => p.event(e),
                _ => {}
            }
        }
        if !more {
            break;
        }
    }
    p.mark(core.end().nanos());

    let mut dropped = p.setup_log.dropped();
    for e in &p.engines {
        dropped += e.detection_telemetry().transitions_dropped;
    }
    let mut signals = std::mem::take(&mut p.b0);
    let emit =
        |out: &mut Vec<RawSignal>, det: &'static str, var: &str, eps: &BTreeMap<String, Track>| {
            for (id, t) in eps {
                if let Some((t0, b)) = t.trig {
                    out.push(RawSignal {
                        det,
                        var: var.to_string(),
                        kind: "trigger",
                        ep: id.clone(),
                        t0,
                        bar_end: b,
                        // A cue counts only if it came before the trigger.
                        cue_t: t.cue.filter(|&c| c <= t0),
                        armed_t: t.armed.filter(|&a| a <= t0),
                    });
                }
                if let Some(c) = t.cue
                    && t.trig.is_none_or(|(t0, _)| c <= t0)
                {
                    // A barrier cue decided at cutoff `c` belongs to the bar ending `c − settle`; a
                    // trade-driven one (micro crossing) to everything executed up to it.
                    let bar_end = if (c - SETTLE).rem_euclid(BAR) == 0 {
                        c - SETTLE
                    } else {
                        c + 1
                    };
                    out.push(RawSignal {
                        det,
                        var: var.to_string(),
                        kind: "cue",
                        ep: id.clone(),
                        t0: c,
                        bar_end,
                        cue_t: Some(c),
                        armed_t: t.armed.filter(|&a| a <= c),
                    });
                }
            }
        };
    for (k, (id, _)) in v.bulls.iter().enumerate() {
        emit(&mut signals, "bull", id, &p.bull_eps[k]);
    }
    for (k, (id, _)) in v.micros.iter().enumerate() {
        emit(&mut signals, "micro", id, &p.micro_eps[k]);
    }
    let mut prints = std::mem::take(&mut p.prints);
    prints.sort_by_key(|x| x.exec);
    let micro_tracks = p
        .micro_eps
        .first()
        .map(|m| m.iter().map(|(k, t)| (k.clone(), t.clone())).collect())
        .unwrap_or_default();
    Ok(SymDay {
        micro_tracks,
        events: p.events,
        trades: p.trades,
        eligible: p.eligible,
        prints,
        bars: std::mem::take(&mut p.bars),
        signals,
        dropped,
    })
}

#[cfg(test)]
mod tests {
    //! The harness's `bull_flag_v0` must be the member's `bull_flag_v0`: the fixture of
    //! `stock-context/tests/setups.rs` (a member that forms a flag, crosses, triggers and
    //! re-arms), stepped both ways on one engine, barrier by barrier.
    use super::*;
    use market_events::session::et;
    use market_events::{Price, Trade};
    use stock_context::{
        Authenticity, CloseBasis, DailyBaseline, Favorability, FloatObservation, InstrumentId,
        NewsItem, PrevClose, SessionVolume, Tag,
    };

    const VOL: Verdict = Verdict {
        ohlc: true,
        volume: true,
    };

    fn day() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 10, 16).unwrap()
    }
    fn t(h: u32, m: u32, s: u32) -> i64 {
        et(day(), h, m, s, 0).unwrap().nanos()
    }
    fn end(k: i64) -> i64 {
        t(9, 30, 0) + (k + 1) * BAR
    }
    fn print(e: &mut Engine, exec: i64, micros: i64, size: u64) {
        e.apply(
            &Trade::new("FLAG", UnixNanos(exec), Price::from_micros(micros), size, 1),
            VOL,
            UnixNanos(exec + 100_000_000),
        );
    }
    fn bar(e: &mut Engine, k: i64, ohlc: [i64; 4], vol: u64) {
        let e0 = end(k) - BAR;
        let [o, h, l, c] = ohlc.map(|x| x * 10_000);
        print(e, e0 + SEC, o, vol / 4);
        print(e, e0 + 3 * SEC, h, vol / 4);
        print(e, e0 + 5 * SEC, l, vol / 4);
        print(e, e0 + 13 * SEC, c, vol - 3 * (vol / 4));
    }
    fn evidence(e: &mut Engine) {
        let inst = InstrumentId::symbol("FLAG");
        let prev = NaiveDate::from_ymd_opt(2026, 10, 15).unwrap();
        e.add_prev_close(PrevClose {
            id: "close".into(),
            version: 1,
            instrument: inst.clone(),
            date: prev,
            close_micros: 4_000_000,
            basis: CloseBasis::SplitAdjusted,
            actions_checked_through: Some(day()),
            actions: Default::default(),
            observed_at: t(4, 0, 0),
        });
        let mut sessions = Vec::new();
        let mut d = prev;
        while sessions.len() < 30 {
            if market_events::is_trading_day(d) {
                sessions.push(SessionVolume {
                    date: d,
                    volume: 100,
                    complete: true,
                    comparable: true,
                });
            }
            d = d.pred_opt().unwrap();
        }
        e.add_daily(DailyBaseline {
            id: "daily".into(),
            version: 1,
            instrument: inst.clone(),
            sessions,
            basis: Default::default(),
            observed_at: t(4, 0, 0),
        });
        e.add_float(FloatObservation {
            id: "float".into(),
            version: 1,
            instrument: inst.clone(),
            shares: 5_000_000,
            provider: "fixture".into(),
            effective_as_of: None,
            observed_at: t(4, 0, 0),
            quality: vec![],
        });
        e.add_news(NewsItem {
            id: "news".into(),
            version: 1,
            instrument: inst,
            event_type: "other".into(),
            source: "issuer".into(),
            credible_source: true,
            relevant: true,
            substantive: true,
            authenticity: Authenticity::IssuerAnnounced,
            favorability: Favorability::Favorable,
            published_at: Some(t(7, 0, 0)),
            observed_at: t(7, 0, 1),
            tag: Tag::Present,
            content_key: "fixture".into(),
            revision_at: Some(0),
            rules: "fixture".into(),
        });
    }

    #[test]
    fn harness_bull_flag_matches_the_members_bull_flag_every_barrier() {
        let mut e = Engine::new(
            Policy::v1(),
            RunInfo {
                run_id: "fixture".into(),
                source: "replay".into(),
                fidelity: Fidelity::SyntheticFixture,
                numerator_coverage: Coverage::Complete,
            },
        );
        evidence(&mut e);
        let bars: Vec<([i64; 4], u64)> = vec![
            ([500, 500, 500, 500], 400),
            ([500, 500, 500, 500], 400),
            ([500, 500, 500, 500], 400),
            ([500, 500, 500, 500], 400),
            ([500, 510, 500, 510], 2_000),
            ([510, 530, 510, 528], 2_000),
            ([525, 525, 518, 520], 500),
            ([520, 522, 517, 519], 500),
            ([519, 521, 518, 521], 400),
            ([521, 527, 520, 526], 300),
            ([526, 532, 526, 531], 2_000),
        ];
        let cfg = vec![("BF0".to_string(), SetupPolicy::v0())];
        let mut bulls = vec![BullFlag::default()];
        let mut log = BoundedLog::new(4096);
        let mut seen = Vec::new();
        for k in 0..30 {
            let ohlc = bars.get(k as usize).copied().unwrap_or(([531; 4], 400));
            bar(&mut e, k, ohlc.0, ohlc.1);
            let _ = e.advance_to(UnixNanos(end(k) + SETTLE));
            let Some(row) = e
                .panel()
                .and_then(|p| p.rows.iter().find(|r| r.symbol == "FLAG").cloned())
            else {
                continue;
            };
            let inp = e.setup_inputs("FLAG").unwrap();
            assert_eq!(inp.date, Some(day()));
            let mine = bull_views(
                &inp,
                end(k),
                &mut bulls,
                &cfg,
                "FLAG",
                "fixture:2026-10-16:FLAG",
                &mut log,
            );
            assert_eq!(mine[0], row.setup, "barrier {k}");
            seen.push(mine[0].state);
        }
        for s in [
            SetupState::Forming,
            SetupState::Near,
            SetupState::Crossing,
            SetupState::Triggered,
            SetupState::Watching,
        ] {
            assert!(seen.contains(&s), "the fixture reaches {s:?}: {seen:?}");
        }
    }
}

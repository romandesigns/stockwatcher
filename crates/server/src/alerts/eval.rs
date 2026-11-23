//! The alert evaluator: pure and deterministic. It sees the live market's eligible trades,
//! halts and modelled LULD bands (handed over by [`crate::alerts`]'s task, off the ingest path)
//! and decides, per alert, fire / hold back / nothing, exactly by the design's Strategy board
//! (docs/architecture/chart-packages.md "Alerts", the rules restated here because they are
//! what the tests pin):
//!
//! - **Eligible trades only.** The market hands over only prints whose `Trade::effect()` may set
//!   OHLC (the candles' and the last price's own condition rule), so an alert fires only on a
//!   price the chart would show. Odd lots, average-price and volume-only prints never arrive.
//! - **A cross, not a touch.** "Crosses above L" fires when an eligible trade lands at or above
//!   L and the previous eligible trade of that symbol was below it (below: mirrored). Without a
//!   previous trade there is no cross.
//! - **Staleness guard.** When the previous eligible trade is older than 10 minutes (the trade is
//!   in the regular session) or 30 minutes (outside it), or the trade itself is that much older
//!   than the wall clock, the comparison is stale: a condition met on it is NOT fired, it is
//!   recorded as held back ("stale").
//! - **Sessions.** A condition met outside the alert's sessions (pre 4:00–9:30, regular, after
//!   16:00–20:00, overnight 20:00–4:00 ET) is ignored silently: the person said not then.
//! - **Halts.** While the symbol is halted only Halted/Resumed alerts fire; others are held
//!   ("halted", once per halt). There are no eligible trades during a halt in practice; this
//!   covers a late print or a feed that keeps going.
//! - **Every time → cooling.** After firing, an "every time" alert waits out its gap (5–60 min)
//!   AND for the price to move back 0.5% from where it fired (against the direction it fired:
//!   an upward fire needs a print at or below fire × 0.995). Only both re-arm it, so a stock
//!   wobbling on a level cannot fire every few minutes. A condition met while cooling is held
//!   back ("gap", once per cooling). Halt alerts re-arm on the gap alone (no price to move).
//! - **Once → done.** Expiry (end of day / week, computed by the client into `expiresAt`) moves
//!   any live alert to expired on [`Evaluator::tick`].
//! - **Late and corrected prints.** An eligible print whose time is more than
//!   [`LATE_TOLERANCE_MS`] before the newest one seen, or a correction's replacement print,
//!   is late: it never moves the symbol's state (the newest in-order trade stays the
//!   reference), but when it would have crossed a price alert that is still on the other side,
//!   the alert fires once, marked late, with the print's own time. Other kinds ignore late prints.
//! - **Level conditions are edges.** % moves and the LULD distance fire when they become true
//!   (false → true), so a stock sitting 6% up does not fire on every print. A rule created or
//!   edited while running starts from "false" (it fires at once if the condition already holds);
//!   rules loaded at startup start unprimed (the first evaluation only learns), so a restart
//!   never re-fires everything that is currently true.
//!
//! Wall time (`now`) drives cooling, expiry and staleness against the clock; the trade's own time
//! drives the windows. Both come in as arguments, so the tests are exact.

use std::collections::{HashMap, HashSet, VecDeque};

use chrono::NaiveDate;
use market_events::SessionKind;

use super::geometry::{self, Shape};
use super::model::{
    AlertState, Condition, CrossDir, Fire, MoveDir, MoveWindow, Rule, Session, Status,
};

pub const MIN_MS: i64 = 60_000;
/// Staleness thresholds: the previous eligible trade (or the trade vs the clock) older than this.
pub const STALE_REGULAR_MS: i64 = 10 * MIN_MS;
pub const STALE_OUTSIDE_MS: i64 = 30 * MIN_MS;
/// Re-arming needs the price to move back this fraction from the fire price.
pub const REARM_MOVE: f64 = 0.005;
/// Prints from different venues arrive slightly out of order; only a print this much older than
/// the newest one seen is "late".
pub const LATE_TOLERANCE_MS: i64 = 2_000;
/// The start (ms) of the `tf_ms` chart candle holding `ts` (ms): the chart's own interval, which
/// for 1h is clipped at the 09:30 open (chart_engine::Timeframe::start_of), so a drawing alert set
/// on a 1h chart reads the slot its candle sits in. Plain division for anything else.
fn bucket_of(ts: i64, tf_ms: i64) -> i64 {
    let h1 = chart_engine::Timeframe::H1;
    if tf_ms == h1.secs() * 1000 {
        return h1
            .start_of(market_events::UnixNanos::from_millis(ts))
            .millis();
    }
    ts.div_euclid(tf_ms) * tf_ms
}

/// % move windows are measured on 15 s buckets (the engine's finest timeframe).
pub const BUCKET_MS: i64 = 15_000;
const MAX_WINDOW_MS: i64 = 15 * MIN_MS;
const EPS: f64 = 1e-9;

/// One item from the live market.
#[derive(Clone, Debug, PartialEq)]
pub enum Input {
    /// An eligible print (Trade::effect().updates_ohlc). `session` is the print's session.
    Trade {
        symbol: String,
        price: f64,
        ts: i64,
        session: Option<(NaiveDate, SessionKind)>,
    },
    /// A correction's replacement print (eligible), always late.
    Corrected {
        symbol: String,
        price: f64,
        ts: i64,
        session: Option<(NaiveDate, SessionKind)>,
    },
    /// A halt (`halted: true`) or a resume.
    Status {
        symbol: String,
        halted: bool,
        ts: i64,
        session: Option<(NaiveDate, SessionKind)>,
        /// The provider's reason code (e.g. "LUDP"), for the text.
        reason: String,
    },
    /// The modelled LULD band moved (chart-engine luld.rs); None = no band (outside regular).
    Luld {
        symbol: String,
        ts: i64,
        up: Option<f64>,
        down: Option<f64>,
    },
    /// The Halt watcher's pressure for a symbol a pressure alert watches (crate::pressure),
    /// scored once a second at the market clock `ts`; `bull` = buy side.
    Pressure {
        symbol: String,
        pressure: u8,
        bull: bool,
        ts: i64,
        session: Option<(NaiveDate, SessionKind)>,
    },
}

impl Input {
    pub fn symbol(&self) -> &str {
        match self {
            Input::Trade { symbol, .. }
            | Input::Corrected { symbol, .. }
            | Input::Status { symbol, .. }
            | Input::Luld { symbol, .. }
            | Input::Pressure { symbol, .. } => symbol,
        }
    }
}

/// What the market already knows about a symbol when an alert starts watching it mid-day (the
/// engine's 15 s candles and halts), so a new alert does not wait a session to learn the open.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Seed {
    pub date: Option<NaiveDate>,
    /// The latest eligible price and its time.
    pub last: Option<(i64, f64)>,
    /// Today's regular open.
    pub open: Option<f64>,
    /// Today's high since 04:00.
    pub high: Option<f64>,
    /// 15 s buckets `(start ms, low, high)`, oldest first (only the last 15 minutes matter).
    pub buckets: Vec<(i64, f64, f64)>,
    pub halted: bool,
    pub luld: Option<(f64, f64)>,
}

/// What the evaluator needs from the market besides its stream.
pub trait MarketView {
    /// The previous trading day's official close before `date` (daily-closes.json).
    fn prev_close(&self, symbol: &str, date: NaiveDate) -> Option<f64>;
    /// What the engine already holds for `symbol` (None: nothing yet).
    fn seed(&self, symbol: &str) -> Option<Seed>;
    /// The chart's slot times for `symbol` at the timeframe `tf_ms`, extended hours shown
    /// (super::geometry::build_slots over the engine's candles and gap runs, the forming candle
    /// included). Only asked for sloped drawing alerts, once per new candle. None: not known
    /// (the line is then not evaluated).
    fn slots(&self, _symbol: &str, _tf_ms: i64) -> Option<Vec<i64>> {
        None
    }
}

/// How a drawing alert was met, for its sound (distinct tones, owner decision 2026-11-09) and
/// the phone's notification channel: the price came up into the drawing, came down into it, or
/// met a position's stop (the after-hours mental stop, whichever way).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Up,
    Down,
    Stop,
}

impl Tone {
    pub fn as_str(self) -> &'static str {
        match self {
            Tone::Up => "up",
            Tone::Down => "down",
            Tone::Stop => "stop",
        }
    }
}

/// Why a met condition was not fired.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HoldReason {
    Stale,
    Halted,
    Gap,
}

impl HoldReason {
    pub fn code(self) -> &'static str {
        match self {
            HoldReason::Stale => "stale",
            HoldReason::Halted => "halted",
            HoldReason::Gap => "gap",
        }
    }
}

/// A decision about one alert.
#[derive(Clone, Debug, PartialEq)]
pub enum Outcome {
    Fired {
        alert_id: String,
        symbol: String,
        /// The print's (or status's) own time.
        t: i64,
        price: Option<f64>,
        text: String,
        late: bool,
        /// The state after the fire (cooling or done).
        status: Status,
        until: Option<i64>,
        /// A drawing alert's tone (which way the price met the drawing, or its stop); None for
        /// every other kind.
        tone: Option<Tone>,
    },
    Held {
        alert_id: String,
        symbol: String,
        t: i64,
        price: Option<f64>,
        reason: HoldReason,
        text: String,
        /// Gap holds: when the alert may fire again (the gap's end; the move back may still be due).
        until: Option<i64>,
    },
}

/// Per-symbol market state, kept only for watched symbols.
#[derive(Clone, Debug, Default)]
struct Sym {
    date: Option<NaiveDate>,
    /// The newest in-order eligible trade.
    last: Option<(i64, f64)>,
    open: Option<f64>,
    high: Option<f64>,
    buckets: VecDeque<(i64, f64, f64)>,
    halted: bool,
    luld: Option<(f64, f64)>,
}

impl Sym {
    fn from_seed(s: Seed) -> Sym {
        Sym {
            date: s.date,
            last: s.last,
            open: s.open,
            high: s.high,
            buckets: s.buckets.into_iter().collect(),
            halted: s.halted,
            luld: s.luld,
        }
    }

    fn roll_date(&mut self, date: NaiveDate) {
        if self.date != Some(date) {
            self.date = Some(date);
            self.open = None;
            self.high = None;
            self.buckets.clear();
            self.luld = None;
        }
    }

    fn add_bucket(&mut self, ts: i64, price: f64) {
        let start = ts.div_euclid(BUCKET_MS) * BUCKET_MS;
        match self.buckets.back_mut() {
            Some(b) if b.0 == start => {
                b.1 = b.1.min(price);
                b.2 = b.2.max(price);
            }
            Some(b) if b.0 > start => {
                // Within the late tolerance but older than the newest bucket: fold it into the
                // bucket it belongs to when still held, else drop it (it is past every window).
                if let Some(b) = self.buckets.iter_mut().rev().find(|b| b.0 == start) {
                    b.1 = b.1.min(price);
                    b.2 = b.2.max(price);
                }
            }
            _ => self.buckets.push_back((start, price, price)),
        }
        while self
            .buckets
            .front()
            .is_some_and(|b| b.0 < start - MAX_WINDOW_MS)
        {
            self.buckets.pop_front();
        }
    }

    /// Low and high over `[ts - window, ts]` (buckets starting in it, plus the one holding its start).
    fn range(&self, ts: i64, window: i64) -> Option<(f64, f64)> {
        let from = (ts - window).div_euclid(BUCKET_MS) * BUCKET_MS;
        let mut out: Option<(f64, f64)> = None;
        for b in self.buckets.iter().filter(|b| b.0 >= from && b.0 <= ts) {
            out = Some(match out {
                None => (b.1, b.2),
                Some((lo, hi)) => (lo.min(b.1), hi.max(b.2)),
            });
        }
        out
    }
}

/// Per-alert memory that is not persisted (a restart relearns it from the next print).
#[derive(Clone, Debug, Default)]
struct Runtime {
    /// Level conditions: whether it held at the last evaluation; None = not primed yet.
    was: Option<bool>,
    /// A gap hold was already recorded in this cooling period.
    gap_noted: bool,
    /// A halted hold was already recorded during this halt.
    halt_noted: bool,
}

/// A drawing alert's view of its drawing this print: whether it is met and how.
struct Touch {
    met: bool,
    up: bool,
    text: String,
    tone: Tone,
}

pub struct Evaluator {
    rules: HashMap<String, Rule>,
    by_symbol: HashMap<String, Vec<String>>,
    states: HashMap<String, AlertState>,
    runtime: HashMap<String, Runtime>,
    syms: HashMap<String, Sym>,
    /// States changed since the last [`Evaluator::take_changes`] (to persist), and removed ids.
    dirty: HashSet<String>,
    removed: HashSet<String>,
    /// Drawing alerts: each watched symbol's watchable drawings (`drawings:<SYMBOL>`, set by
    /// [`Evaluator::set_drawings`]); a symbol absent here has not been read yet.
    drawings: HashMap<String, HashMap<String, Shape>>,
    /// The geometry each drawing alert last saw, to tell an edit of its drawing.
    seen_shape: HashMap<String, Shape>,
    /// Slot times per (symbol, timeframe ms), with the candle start they were fetched for.
    slot_cache: HashMap<(String, i64), (i64, Vec<i64>)>,
}

impl Default for Evaluator {
    fn default() -> Self {
        Self::new()
    }
}

fn session_ok(rule: &Rule, s: Option<(NaiveDate, SessionKind)>) -> bool {
    match s {
        Some((_, k)) => rule.sessions.contains(&Session::of(k)),
        // Outside every session (a weekend print): never in the sessions anyone picked.
        None => false,
    }
}

fn stale_after(s: Option<(NaiveDate, SessionKind)>) -> i64 {
    match s {
        Some((_, SessionKind::Regular)) => STALE_REGULAR_MS,
        _ => STALE_OUTSIDE_MS,
    }
}

/// `2.5` → "2.50", `0.1904` → "0.1904": the tick grid's precision (cents ≥ $1, 4 dp below).
pub fn fmt_price(p: f64) -> String {
    if p >= 1.0 {
        format!("{p:.2}")
    } else {
        format!("{p:.4}")
    }
}

fn pct(from: f64, to: f64) -> f64 {
    (to - from) / from * 100.0
}

impl Evaluator {
    pub fn new() -> Self {
        Evaluator {
            rules: HashMap::new(),
            by_symbol: HashMap::new(),
            states: HashMap::new(),
            runtime: HashMap::new(),
            syms: HashMap::new(),
            dirty: HashSet::new(),
            removed: HashSet::new(),
            drawings: HashMap::new(),
            seen_shape: HashMap::new(),
            slot_cache: HashMap::new(),
        }
    }

    /// The stored states, read once at startup (before the first [`Evaluator::set_rules`]).
    pub fn load_states(&mut self, states: impl IntoIterator<Item = (String, AlertState)>) {
        self.states = states.into_iter().collect();
    }

    pub fn state(&self, id: &str) -> Option<&AlertState> {
        self.states.get(id)
    }

    pub fn rule(&self, id: &str) -> Option<&Rule> {
        self.rules.get(id)
    }

    pub fn rules(&self) -> impl Iterator<Item = &Rule> {
        self.rules.values()
    }

    /// Replaces the rule set (the `alerts` namespace changed). A new rule, or one edited since its
    /// state was written (`editedAt` > `basis`), starts over armed. `initial`: the rules loaded at
    /// startup, whose level conditions start unprimed (see the module docs). Symbols that gain
    /// their first alert are seeded from the market.
    pub fn set_rules(&mut self, rules: Vec<Rule>, initial: bool, view: &dyn MarketView) {
        let ids: HashSet<String> = rules.iter().map(|r| r.id.clone()).collect();
        for id in self.states.keys().filter(|k| !ids.contains(*k)) {
            self.removed.insert(id.clone());
        }
        self.states.retain(|k, _| ids.contains(k));
        self.runtime.retain(|k, _| ids.contains(k));
        for id in &ids {
            self.removed.remove(id);
        }
        let mut by_symbol: HashMap<String, Vec<String>> = HashMap::new();
        let mut map = HashMap::new();
        for r in rules {
            let fresh = match self.states.get(&r.id) {
                None => true,
                Some(s) => s.basis < r.edited_at,
            };
            // A rule whose condition changed while running is "edited": it learns from false.
            let changed = self.rules.get(&r.id).is_some_and(|old| *old != r);
            if fresh {
                self.states
                    .insert(r.id.clone(), AlertState::armed(r.edited_at));
                self.dirty.insert(r.id.clone());
            }
            if fresh || changed || !self.runtime.contains_key(&r.id) {
                // Loaded at startup: unprimed (learn first). Created or edited while running:
                // primed with "false", so a condition that already holds fires at once.
                let was = (!initial).then_some(false);
                self.runtime.insert(
                    r.id.clone(),
                    Runtime {
                        was,
                        ..Runtime::default()
                    },
                );
            }
            by_symbol
                .entry(r.symbol.clone())
                .or_default()
                .push(r.id.clone());
            map.insert(r.id.clone(), r);
        }
        for v in by_symbol.values_mut() {
            v.sort();
        }
        // Market state only for watched symbols; a newly watched one is seeded from the engine.
        self.syms.retain(|s, _| by_symbol.contains_key(s));
        for s in by_symbol.keys() {
            if !self.syms.contains_key(s) {
                let sym = view.seed(s).map(Sym::from_seed).unwrap_or_default();
                self.syms.insert(s.clone(), sym);
            }
        }
        self.rules = map;
        self.by_symbol = by_symbol;
        let wanted: HashSet<String> = self.drawing_symbols().into_iter().collect();
        self.drawings.retain(|s, _| wanted.contains(s));
        self.slot_cache.retain(|(s, _), _| wanted.contains(s));
        self.seen_shape.retain(|id, _| ids.contains(id));
        self.sync_drawing_states();
    }

    /// Symbols with a drawing alert: the alerts task reads (and follows) their
    /// `drawings:<SYMBOL>` namespaces for [`Evaluator::set_drawings`].
    pub fn drawing_symbols(&self) -> Vec<String> {
        let mut v: Vec<String> = self
            .rules
            .values()
            .filter(|r| r.condition.is_drawing())
            .map(|r| r.symbol.clone())
            .collect();
        v.sort();
        v.dedup();
        v
    }

    /// One symbol's drawings document (`drawings:<SYMBOL>`; None = the namespace is empty or
    /// gone, so every drawing alert on it is orphaned).
    pub fn set_drawings(&mut self, symbol: &str, doc: Option<&serde_json::Value>) {
        self.drawings
            .insert(symbol.to_owned(), geometry::parse_drawings(doc));
        self.sync_drawing_states();
    }

    /// Drawing alerts against their drawings (docs: chart-packages.md "Drawing alerts"):
    /// - **Deleted** → orphaned: the alert stays (an undo brings the drawing back and it carries
    ///   on) but watches nothing, and its state says so for the clients.
    /// - **Edited** (moved, re-angled, a position's stop dragged) → the alert follows the new
    ///   geometry, the same as dragging a price alert's line: a finished or cooling alert starts
    ///   over armed, and the level is learned before it fires (a line dropped onto the current
    ///   price does not fire on the next print; it fires when the price comes back to it).
    fn sync_drawing_states(&mut self) {
        let mut ids: Vec<String> = self
            .rules
            .values()
            .filter(|r| r.condition.is_drawing())
            .map(|r| r.id.clone())
            .collect();
        ids.sort();
        for id in ids {
            let rule = &self.rules[&id];
            let Condition::Drawing { drawing_id, .. } = &rule.condition else {
                continue;
            };
            // Not read yet: nothing is known, nothing changes.
            let Some(map) = self.drawings.get(&rule.symbol) else {
                continue;
            };
            let shape = map.get(drawing_id).cloned();
            let Some(st) = self.states.get(&id).cloned() else {
                continue;
            };
            let orphaned = shape.is_none();
            if st.orphaned != orphaned {
                self.set_state(&id, AlertState { orphaned, ..st });
            }
            let Some(shape) = shape else { continue };
            let edited = self.seen_shape.get(&id).is_some_and(|old| *old != shape);
            self.seen_shape.insert(id.clone(), shape);
            if edited {
                let st = self.states[&id].clone();
                if matches!(st.status, Status::Done | Status::Cooling) {
                    self.rearm(&id, st);
                }
                if let Some(rt) = self.runtime.get_mut(&id) {
                    rt.was = None;
                    rt.gap_noted = false;
                }
            }
        }
    }

    /// States to write to `alerts.state` (set) and ids to remove from it, since the last call.
    pub fn take_changes(&mut self) -> (Vec<(String, AlertState)>, Vec<String>) {
        let set = self
            .dirty
            .drain()
            .filter_map(|id| self.states.get(&id).map(|s| (id, s.clone())))
            .collect();
        (set, self.removed.drain().collect())
    }

    fn set_state(&mut self, id: &str, s: AlertState) {
        if self.states.get(id) != Some(&s) {
            self.states.insert(id.to_owned(), s);
            self.dirty.insert(id.to_owned());
        }
    }

    /// Wall-clock housekeeping: expiry, the snooze flag, and re-arming a cooling alert whose
    /// price already moved back once its gap ends (no print needed). The alerts task calls it
    /// once a second and right after the rules change, so each flip reaches the clients' copy of
    /// `alerts.state` within a second: they show what it says and never decide these by their own
    /// clock (crate::time_marks).
    pub fn tick(&mut self, now: i64) {
        let ids: Vec<String> = self.rules.keys().cloned().collect();
        for id in ids {
            let (Some(rule), Some(st)) = (self.rules.get(&id), self.states.get(&id).cloned())
            else {
                continue;
            };
            if matches!(st.status, Status::Done | Status::Expired) {
                if st.snoozed != Some(false) {
                    self.set_state(
                        &id,
                        AlertState {
                            snoozed: Some(false),
                            ..st
                        },
                    );
                }
                continue;
            }
            if rule.expires_at.is_some_and(|e| now >= e) {
                self.set_state(
                    &id,
                    AlertState {
                        status: Status::Expired,
                        until: None,
                        moved_back: false,
                        snoozed: Some(false),
                        ..st
                    },
                );
                continue;
            }
            let snoozed = rule.snooze_until.is_some_and(|u| now < u);
            let rearms_on_gap = rule.condition.rearms_on_gap();
            let st = if st.snoozed == Some(snoozed) {
                st
            } else {
                let next = AlertState {
                    snoozed: Some(snoozed),
                    ..st
                };
                self.set_state(&id, next.clone());
                next
            };
            if st.status == Status::Cooling
                && st.until.is_some_and(|u| now >= u)
                && (st.moved_back || rearms_on_gap)
            {
                self.rearm(&id, st);
            }
        }
    }

    fn rearm(&mut self, id: &str, st: AlertState) {
        self.set_state(
            id,
            AlertState {
                status: Status::Armed,
                until: None,
                moved_back: false,
                ..st
            },
        );
        if let Some(rt) = self.runtime.get_mut(id) {
            rt.gap_noted = false;
            // Re-armed by a move back: the level condition is false now by construction, so the
            // next time it holds is a new edge.
            if rt.was.is_some() {
                rt.was = Some(false);
            }
        }
    }

    /// One input. `now` is the wall clock (ms).
    pub fn on_input(&mut self, input: Input, now: i64, view: &dyn MarketView) -> Vec<Outcome> {
        let mut out = Vec::new();
        if !self.syms.contains_key(input.symbol()) {
            return out;
        }
        match input {
            Input::Trade {
                symbol,
                price,
                ts,
                session,
            } => self.on_trade(&symbol, price, ts, session, now, view, &mut out),
            Input::Corrected {
                symbol,
                price,
                ts,
                session,
            } => self.on_late(&symbol, price, ts, session, now, &mut out),
            Input::Status {
                symbol,
                halted,
                ts,
                session,
                reason,
            } => self.on_status(&symbol, halted, ts, session, &reason, now, &mut out),
            Input::Luld {
                symbol,
                ts,
                up,
                down,
            } => {
                let sym = self.syms.get_mut(&symbol).expect("watched");
                if let Some(date) =
                    market_events::classify(market_events::UnixNanos::from_millis(ts))
                        .map(|s| s.date)
                {
                    sym.roll_date(date);
                }
                sym.luld = up.zip(down);
            }
            Input::Pressure {
                symbol,
                pressure,
                bull,
                ts,
                session,
            } => self.on_pressure(&symbol, pressure, bull, ts, session, now, &mut out),
        }
        out
    }

    /// A Halt watcher reading (once a second for a watched symbol): an edge at the alert's
    /// minimum on its side. Not stale-checked (the reading is the market's own, at its clock);
    /// held while halted like every other non-halt alert.
    #[allow(clippy::too_many_arguments)]
    fn on_pressure(
        &mut self,
        symbol: &str,
        pressure: u8,
        bull: bool,
        ts: i64,
        session: Option<(NaiveDate, SessionKind)>,
        now: i64,
        out: &mut Vec<Outcome>,
    ) {
        let halted = self.syms[symbol].halted;
        for id in self.ids_of(symbol) {
            let rule = self.rules[&id].clone();
            let Condition::Pressure { side, min } = rule.condition else {
                continue;
            };
            let Some(mut st) = self.states.get(&id).cloned() else {
                continue;
            };
            if st.status == Status::Cooling && st.until.is_some_and(|u| now >= u) {
                self.rearm(&id, st);
                st = self.states[&id].clone();
            }
            let side_ok = match side {
                MoveDir::Up => bull,
                MoveDir::Down => !bull,
                MoveDir::Either => true,
            };
            let met = self.edge(&id, side_ok && pressure >= min);
            if !met || !self.live(&id, now) || !session_ok(&rule, session) {
                continue;
            }
            let text = format!(
                "{} pressure reached {pressure} (Halt watcher)",
                if bull { "Buy" } else { "Sell" }
            );
            self.decide(
                &id, &rule, st, ts, None, bull, text, false, halted, false, None, now, out,
            );
        }
    }

    /// Symbols with a pressure alert: the market scores only these for the evaluator.
    pub fn pressure_symbols(&self) -> Vec<String> {
        let mut v: Vec<String> = self
            .rules
            .values()
            .filter(|r| matches!(r.condition, Condition::Pressure { .. }))
            .map(|r| r.symbol.clone())
            .collect();
        v.sort();
        v.dedup();
        v
    }

    fn ids_of(&self, symbol: &str) -> Vec<String> {
        self.by_symbol.get(symbol).cloned().unwrap_or_default()
    }

    /// Live, armed-or-cooling, not snoozed: may fire or hold. Done/expired/snoozed: silent.
    fn live(&self, id: &str, now: i64) -> bool {
        let (Some(r), Some(s)) = (self.rules.get(id), self.states.get(id)) else {
            return false;
        };
        !matches!(s.status, Status::Done | Status::Expired)
            && !r.snooze_until.is_some_and(|u| now < u)
            && !r.expires_at.is_some_and(|e| now >= e)
    }

    #[allow(clippy::too_many_arguments)]
    fn on_trade(
        &mut self,
        symbol: &str,
        price: f64,
        ts: i64,
        session: Option<(NaiveDate, SessionKind)>,
        now: i64,
        view: &dyn MarketView,
        out: &mut Vec<Outcome>,
    ) {
        let sym = self.syms.get(symbol).expect("watched");
        if sym.last.is_some_and(|(lt, _)| ts + LATE_TOLERANCE_MS < lt) {
            return self.on_late(symbol, price, ts, session, now, out);
        }
        let sym = self.syms.get_mut(symbol).expect("watched");
        if let Some((date, _)) = session {
            sym.roll_date(date);
        }
        let prev = sym.last;
        let threshold = stale_after(session);
        let stale = prev.is_some_and(|(lt, _)| ts - lt > threshold) || now - ts > threshold;
        // Day statistics, from 04:00 (overnight prints neither set nor break the high of day).
        let day_session = matches!(
            session,
            Some((
                _,
                SessionKind::Pre | SessionKind::Regular | SessionKind::After
            ))
        );
        let prev_high = if day_session { sym.high } else { None };
        if day_session {
            sym.high = Some(sym.high.map_or(price, |h| h.max(price)));
        }
        if matches!(session, Some((_, SessionKind::Regular))) && sym.open.is_none() {
            sym.open = Some(price);
        }
        sym.add_bucket(ts, price);
        let halted = sym.halted;
        let snapshot = sym.clone();
        let prev_close = session.and_then(|(d, _)| view.prev_close(symbol, d));

        for id in self.ids_of(symbol) {
            let rule = self.rules[&id].clone();
            let Some(mut st) = self.states.get(&id).cloned() else {
                continue;
            };
            if rule.condition.rearms_on_gap() {
                continue;
            }
            // Cooling: watch for the move back, and re-arm once the gap has passed too.
            if st.status == Status::Cooling {
                if let (Some(fp), Some(up)) = (st.fired_price, st.fired_up) {
                    // A touch has no side to come back from: a drawing alert re-arms once the
                    // price has moved 0.5% away from where it fired, either way.
                    let back = if rule.condition.is_drawing() {
                        (price - fp).abs() >= fp * REARM_MOVE - EPS
                    } else if up {
                        price <= fp * (1.0 - REARM_MOVE) + EPS
                    } else {
                        price >= fp * (1.0 + REARM_MOVE) - EPS
                    };
                    if back && !st.moved_back {
                        st.moved_back = true;
                        self.set_state(&id, st.clone());
                    }
                }
                if st.moved_back && st.until.is_some_and(|u| now >= u) {
                    self.rearm(&id, st.clone());
                    st = self.states[&id].clone();
                }
            }
            let (met, up, text, tone) = if rule.condition.is_drawing() {
                match self.drawing_touch(&rule, prev, price, ts, view) {
                    Some(t) => (t.met, t.up, t.text, Some(t.tone)),
                    None => continue,
                }
            } else {
                match self.condition(&rule, &snapshot, prev, prev_high, prev_close, price, ts) {
                    Some((m, u, t)) => (m, u, t, None),
                    None => continue,
                }
            };
            if !met {
                continue;
            }
            if !self.live(&id, now) || !session_ok(&rule, session) {
                continue;
            }
            self.decide(
                &id,
                &rule,
                st,
                ts,
                Some(price),
                up,
                text,
                stale,
                halted,
                false,
                tone,
                now,
                out,
            );
        }
        let sym = self.syms.get_mut(symbol).expect("watched");
        sym.last = Some((ts, price));
    }

    /// The chart's slots for a sloped drawing alert, fetched once per new candle per
    /// (symbol, timeframe). `bucket` is the start of the trade's candle.
    fn slots(
        &mut self,
        symbol: &str,
        tf_ms: i64,
        bucket: i64,
        view: &dyn MarketView,
    ) -> Option<&[i64]> {
        let key = (symbol.to_owned(), tf_ms);
        let fresh = self.slot_cache.get(&key).is_some_and(|(b, _)| *b == bucket);
        if !fresh {
            let slots = view.slots(symbol, tf_ms)?;
            self.slot_cache.insert(key.clone(), (bucket, slots));
        }
        self.slot_cache.get(&key).map(|(_, s)| s.as_slice())
    }

    /// A drawing alert against this print: touching = within the tolerance of the drawing's level
    /// at the trade's candle (super::geometry), or jumped clean through it since the previous
    /// eligible print (an illiquid after-hours stock can skip the band entirely, and a stop it
    /// skipped is exactly the one that must not stay silent). An edge, like the other level
    /// conditions: it fires when it becomes met. None = nothing to evaluate (the drawing is gone,
    /// the slots unknown); a level that is not there now (a trend line outside its anchors)
    /// counts as not met.
    fn drawing_touch(
        &mut self,
        rule: &Rule,
        prev: Option<(i64, f64)>,
        price: f64,
        ts: i64,
        view: &dyn MarketView,
    ) -> Option<Touch> {
        let Condition::Drawing {
            drawing_id,
            part,
            tolerance,
            tf_ms,
        } = &rule.condition
        else {
            return None;
        };
        let shape = self.drawings.get(&rule.symbol)?.get(drawing_id)?.clone();
        let bucket = bucket_of(ts, *tf_ms);
        let level = if shape.sloped() {
            let slots = self.slots(&rule.symbol, *tf_ms, bucket, view)?;
            geometry::level_at(&shape, *part, slots, bucket as f64, *tf_ms)
        } else {
            geometry::level_at(&shape, *part, &[], bucket as f64, *tf_ms)
        };
        let Some(level) = level else {
            self.edge(&rule.id, false);
            return Some(Touch {
                met: false,
                up: true,
                text: String::new(),
                tone: Tone::Up,
            });
        };
        let touch = geometry::touching(price, level, *tolerance);
        let before = prev.map(|(_, p)| p);
        let through = before.is_some_and(|b| {
            (b < level - EPS && price > level + EPS) || (b > level + EPS && price < level - EPS)
        });
        let met = self.edge(&rule.id, touch || through);
        let reference = before.unwrap_or(price);
        let up = if (reference - level).abs() <= EPS {
            price >= level
        } else {
            reference < level
        };
        let tone = match part {
            geometry::Part::Stop => Tone::Stop,
            _ if up => Tone::Up,
            _ => Tone::Down,
        };
        let verb = if touch { "Touched" } else { "Crossed through" };
        let text = format!(
            "{verb} {} {} at {} from {}",
            shape.label(*part),
            fmt_price(level),
            fmt_price(price),
            if up { "below" } else { "above" }
        );
        Some(Touch {
            met,
            up,
            text,
            tone,
        })
    }

    /// Whether `rule`'s condition is met by this print, which way it fired (up = true) and the
    /// text. Level conditions are edge-triggered here (the runtime's `was`); None = not evaluable.
    #[allow(clippy::too_many_arguments)]
    fn condition(
        &mut self,
        rule: &Rule,
        sym: &Sym,
        prev: Option<(i64, f64)>,
        prev_high: Option<f64>,
        prev_close: Option<f64>,
        price: f64,
        ts: i64,
    ) -> Option<(bool, bool, String)> {
        let p = fmt_price(price);
        match &rule.condition {
            Condition::Price { dir, price: level } => {
                let (_, before) = prev?;
                Some(match dir {
                    CrossDir::Above => (
                        before < *level - EPS && price >= *level - EPS,
                        true,
                        format!("Crossed above {} at {p}", fmt_price(*level)),
                    ),
                    CrossDir::Below => (
                        before > *level + EPS && price <= *level + EPS,
                        false,
                        format!("Crossed below {} at {p}", fmt_price(*level)),
                    ),
                })
            }
            Condition::HighOfDay => {
                let h = prev_high?;
                Some((
                    price > h + EPS,
                    true,
                    format!("New high of day {p} (was {})", fmt_price(h)),
                ))
            }
            Condition::Move {
                dir,
                pct: want,
                window,
            } => {
                let (up_pct, down_pct, from_up, from_down, label) = match window {
                    MoveWindow::Minutes(m) => {
                        let (lo, hi) = sym.range(ts, *m as i64 * MIN_MS)?;
                        (
                            pct(lo, price),
                            -pct(hi, price),
                            lo,
                            hi,
                            format!("in {m} min"),
                        )
                    }
                    MoveWindow::Open => {
                        let o = sym.open?;
                        (
                            pct(o, price),
                            -pct(o, price),
                            o,
                            o,
                            "from the open".to_string(),
                        )
                    }
                    MoveWindow::PrevClose => {
                        let c = prev_close?;
                        (
                            pct(c, price),
                            -pct(c, price),
                            c,
                            c,
                            "from the previous close".to_string(),
                        )
                    }
                };
                let up_met = matches!(dir, MoveDir::Up | MoveDir::Either) && up_pct >= *want - EPS;
                let down_met =
                    matches!(dir, MoveDir::Down | MoveDir::Either) && down_pct >= *want - EPS;
                let now_met = up_met || down_met;
                let edge = self.edge(&rule.id, now_met);
                let text = if up_met {
                    format!("Up {up_pct:.1}% {label}, {} → {p}", fmt_price(from_up))
                } else {
                    format!(
                        "Down {down_pct:.1}% {label}, {} → {p}",
                        fmt_price(from_down)
                    )
                };
                Some((edge, up_met, text))
            }
            Condition::Luld { side, pct: within } => {
                let (upb, downb) = sym.luld?;
                let to_up = (upb - price) / upb * 100.0;
                let to_down = (price - downb) / downb * 100.0;
                let up_met = matches!(side, MoveDir::Up | MoveDir::Either)
                    && to_up >= -EPS
                    && to_up <= *within + EPS;
                let down_met = matches!(side, MoveDir::Down | MoveDir::Either)
                    && to_down >= -EPS
                    && to_down <= *within + EPS;
                let edge = self.edge(&rule.id, up_met || down_met);
                let text = if up_met {
                    format!(
                        "Within {to_up:.1}% of the limit-up band (modelled) at {p} · band {}",
                        fmt_price(upb)
                    )
                } else {
                    format!(
                        "Within {to_down:.1}% of the limit-down band (modelled) at {p} · band {}",
                        fmt_price(downb)
                    )
                };
                Some((edge, up_met, text))
            }
            Condition::Halt | Condition::Pressure { .. } | Condition::Drawing { .. } => None,
        }
    }

    /// Records `now_met` and reports a false → true edge (unprimed: learn, never fire).
    fn edge(&mut self, id: &str, now_met: bool) -> bool {
        let rt = self.runtime.entry(id.to_owned()).or_default();
        let was = rt.was.replace(now_met);
        matches!(was, Some(false)) && now_met
    }

    /// Fire, or hold back with the reason. `st` is the alert's state now.
    #[allow(clippy::too_many_arguments)]
    fn decide(
        &mut self,
        id: &str,
        rule: &Rule,
        st: AlertState,
        t: i64,
        price: Option<f64>,
        up: bool,
        text: String,
        stale: bool,
        halted: bool,
        late: bool,
        tone: Option<Tone>,
        now: i64,
        out: &mut Vec<Outcome>,
    ) {
        let symbol = rule.symbol.clone();
        if st.status == Status::Cooling {
            let rt = self.runtime.entry(id.to_owned()).or_default();
            if !rt.gap_noted {
                rt.gap_noted = true;
                out.push(Outcome::Held {
                    alert_id: id.to_owned(),
                    symbol,
                    t,
                    price,
                    reason: HoldReason::Gap,
                    text,
                    until: st.until,
                });
            }
            return;
        }
        if halted && !rule.condition.is_halt() {
            let rt = self.runtime.entry(id.to_owned()).or_default();
            if !rt.halt_noted {
                rt.halt_noted = true;
                out.push(Outcome::Held {
                    alert_id: id.to_owned(),
                    symbol,
                    t,
                    price,
                    reason: HoldReason::Halted,
                    text,
                    until: None,
                });
            }
            return;
        }
        if stale {
            out.push(Outcome::Held {
                alert_id: id.to_owned(),
                symbol,
                t,
                price,
                reason: HoldReason::Stale,
                text,
                until: None,
            });
            return;
        }
        let (status, until) = match rule.fire {
            Fire::Once => (Status::Done, None),
            Fire::Every { gap_min } => (Status::Cooling, Some(now + gap_min as i64 * MIN_MS)),
        };
        self.set_state(
            id,
            AlertState {
                status,
                until,
                moved_back: false,
                basis: st.basis,
                fired_at: Some(t),
                fired_price: price,
                fired_up: Some(up),
                count: st.count + 1,
                // A snoozed alert never fires (`live`).
                snoozed: Some(false),
                orphaned: st.orphaned,
            },
        );
        if let Some(rt) = self.runtime.get_mut(id) {
            rt.gap_noted = false;
        }
        out.push(Outcome::Fired {
            alert_id: id.to_owned(),
            symbol,
            t,
            price,
            text,
            late,
            status,
            until,
            tone,
        });
    }

    /// A late or corrected print: never moves the symbol's state; fires a price alert once when it
    /// would have crossed and the alert is still on the other side of the newest price.
    fn on_late(
        &mut self,
        symbol: &str,
        price: f64,
        ts: i64,
        session: Option<(NaiveDate, SessionKind)>,
        now: i64,
        out: &mut Vec<Outcome>,
    ) {
        let Some(current) = self.syms.get(symbol).and_then(|s| s.last.map(|(_, p)| p)) else {
            return;
        };
        let halted = self.syms[symbol].halted;
        for id in self.ids_of(symbol) {
            let rule = self.rules[&id].clone();
            let Condition::Price { dir, price: level } = rule.condition else {
                continue;
            };
            let Some(st) = self.states.get(&id).cloned() else {
                continue;
            };
            let (crossed, up, word) = match dir {
                CrossDir::Above => (price >= level - EPS && current < level - EPS, true, "above"),
                CrossDir::Below => (
                    price <= level + EPS && current > level + EPS,
                    false,
                    "below",
                ),
            };
            if !crossed
                || st.status != Status::Armed
                || !self.live(&id, now)
                || !session_ok(&rule, session)
            {
                continue;
            }
            let text = format!(
                "Crossed {word} {} at {} (late print)",
                fmt_price(level),
                fmt_price(price)
            );
            self.decide(
                &id,
                &rule,
                st,
                ts,
                Some(price),
                up,
                text,
                false,
                halted,
                true,
                None,
                now,
                out,
            );
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn on_status(
        &mut self,
        symbol: &str,
        halted: bool,
        ts: i64,
        session: Option<(NaiveDate, SessionKind)>,
        reason: &str,
        now: i64,
        out: &mut Vec<Outcome>,
    ) {
        let sym = self.syms.get_mut(symbol).expect("watched");
        if sym.halted == halted {
            // A repeated halt (or a resume with nothing open) is not news.
            return;
        }
        sym.halted = halted;
        for id in self.ids_of(symbol) {
            if !halted && let Some(rt) = self.runtime.get_mut(&id) {
                rt.halt_noted = false;
            }
            let rule = self.rules[&id].clone();
            if !rule.condition.is_halt() {
                continue;
            }
            let Some(mut st) = self.states.get(&id).cloned() else {
                continue;
            };
            if st.status == Status::Cooling && st.until.is_some_and(|u| now >= u) {
                self.rearm(&id, st);
                st = self.states[&id].clone();
            }
            if !self.live(&id, now) || !session_ok(&rule, session) {
                continue;
            }
            let reason = reason.trim();
            let text = if halted {
                if reason.is_empty() {
                    "Halted".to_string()
                } else {
                    format!("Halted · {reason}")
                }
            } else {
                "Resumed trading".to_string()
            };
            self.decide(
                &id, &rule, st, ts, None, halted, text, false, false, false, None, now, out,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::alerts::model::{DEFAULT_SESSIONS, Expiry};
    use chrono::NaiveTime;

    struct View {
        prev_close: Option<f64>,
        seed: Option<Seed>,
    }
    impl MarketView for View {
        fn prev_close(&self, _: &str, _: NaiveDate) -> Option<f64> {
            self.prev_close
        }
        fn seed(&self, _: &str) -> Option<Seed> {
            self.seed.clone()
        }
    }
    const NO_VIEW: View = View {
        prev_close: None,
        seed: None,
    };

    fn day() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 10, 15).unwrap()
    }
    /// Epoch ms of an ET wall time on the test day.
    fn et(h: u32, m: u32, s: u32) -> i64 {
        market_events::session::et_instant(day(), NaiveTime::from_hms_opt(h, m, s).unwrap())
            .unwrap()
            .millis()
    }
    fn sess(ts: i64) -> Option<(NaiveDate, SessionKind)> {
        market_events::classify(market_events::UnixNanos::from_millis(ts)).map(|s| (s.date, s.kind))
    }
    fn trade(price: f64, ts: i64) -> Input {
        Input::Trade {
            symbol: "XLAB".into(),
            price,
            ts,
            session: sess(ts),
        }
    }
    fn rule(id: &str, condition: Condition, fire: Fire) -> Rule {
        Rule {
            id: id.into(),
            symbol: "XLAB".into(),
            condition,
            fire,
            sessions: vec![
                Session::Pre,
                Session::Regular,
                Session::After,
                Session::Overnight,
            ],
            expiry: Expiry::Never,
            expires_at: None,
            notify_app: true,
            notify_phone: true,
            sound: true,
            snooze_until: None,
            created_at: 1,
            edited_at: 1,
        }
    }
    fn above(level: f64, fire: Fire) -> Rule {
        rule(
            "a",
            Condition::Price {
                dir: CrossDir::Above,
                price: level,
            },
            fire,
        )
    }
    fn ev(rules: Vec<Rule>) -> Evaluator {
        let mut e = Evaluator::new();
        e.set_rules(rules, false, &NO_VIEW);
        e
    }
    /// Feeds trades at `ts` (wall clock = trade time) and returns the outcomes.
    fn feed(e: &mut Evaluator, items: &[(f64, i64)]) -> Vec<Outcome> {
        items
            .iter()
            .flat_map(|(p, t)| e.on_input(trade(*p, *t), *t, &NO_VIEW))
            .collect()
    }
    fn fired(o: &[Outcome]) -> usize {
        o.iter()
            .filter(|x| matches!(x, Outcome::Fired { .. }))
            .count()
    }
    fn held(o: &[Outcome]) -> Vec<HoldReason> {
        o.iter()
            .filter_map(|x| {
                if let Outcome::Held { reason, .. } = x {
                    Some(*reason)
                } else {
                    None
                }
            })
            .collect()
    }

    #[test]
    fn a_cross_needs_the_previous_trade_on_the_other_side() {
        let t0 = et(10, 0, 0);
        let mut e = ev(vec![above(2.50, Fire::Once)]);
        // The first trade has nothing before it: at the level, but no cross.
        assert_eq!(fired(&feed(&mut e, &[(2.55, t0)])), 0);
        // Above → above: a touch, not a cross.
        assert_eq!(fired(&feed(&mut e, &[(2.60, t0 + 1000)])), 0);
        let mut e = ev(vec![above(2.50, Fire::Once)]);
        let o = feed(&mut e, &[(2.49, t0), (2.50, t0 + 1000)]);
        assert_eq!(fired(&o), 1, "below → at the level is a cross");
        match &o[0] {
            Outcome::Fired {
                text,
                status,
                price,
                ..
            } => {
                assert_eq!(text, "Crossed above 2.50 at 2.50");
                assert_eq!(*status, Status::Done, "once → done");
                assert_eq!(*price, Some(2.50));
            }
            x => panic!("{x:?}"),
        }
        // Done: nothing more, whatever the price does.
        assert_eq!(
            fired(&feed(&mut e, &[(2.40, t0 + 2000), (2.60, t0 + 3000)])),
            0
        );
        assert_eq!(e.state("a").unwrap().status, Status::Done);
        // Below, mirrored; a level exactly touched from above counts.
        let mut e = ev(vec![rule(
            "b",
            Condition::Price {
                dir: CrossDir::Below,
                price: 2.10,
            },
            Fire::Once,
        )]);
        assert_eq!(fired(&feed(&mut e, &[(2.20, t0), (2.10, t0 + 1)])), 1);
    }

    #[test]
    fn rearm_needs_the_gap_and_a_half_percent_move_back() {
        let t0 = et(10, 0, 0);
        let mut e = ev(vec![above(2.50, Fire::Every { gap_min: 15 })]);
        assert_eq!(fired(&feed(&mut e, &[(2.49, t0), (2.51, t0 + 1000)])), 1);
        let st = e.state("a").unwrap().clone();
        assert_eq!(st.status, Status::Cooling);
        assert_eq!(st.until, Some(t0 + 1000 + 15 * MIN_MS));
        // Wobbling on the level inside the gap, never 0.5% back (2.51 × 0.995 = 2.4975): held
        // once ("gap"), never fired.
        let o = feed(
            &mut e,
            &[
                (2.499, t0 + 2000),
                (2.51, t0 + 3000),
                (2.499, t0 + 4000),
                (2.52, t0 + 5000),
            ],
        );
        assert_eq!(fired(&o), 0);
        assert_eq!(
            held(&o),
            vec![HoldReason::Gap],
            "one gap hold per cooling period"
        );
        assert!(!e.state("a").unwrap().moved_back);
        // The gap passes, but the price still never moved back: still cooling.
        let t1 = t0 + 16 * MIN_MS;
        let o = feed(&mut e, &[(2.499, t1), (2.51, t1 + 1000)]);
        assert_eq!(fired(&o), 0);
        assert_eq!(e.state("a").unwrap().status, Status::Cooling);
        // A real move back re-arms (gap long passed); the next cross fires again.
        let o = feed(&mut e, &[(2.497, t1 + 2000), (2.52, t1 + 3000)]);
        assert_eq!(fired(&o), 1);
        assert_eq!(e.state("a").unwrap().count, 2);
    }

    #[test]
    fn move_back_before_the_gap_rearms_on_the_tick() {
        let t0 = et(10, 0, 0);
        let mut e = ev(vec![above(2.50, Fire::Every { gap_min: 5 })]);
        feed(&mut e, &[(2.49, t0), (2.51, t0 + 1000)]);
        feed(&mut e, &[(2.40, t0 + 2000)]);
        assert!(e.state("a").unwrap().moved_back);
        e.tick(t0 + 4 * MIN_MS);
        assert_eq!(
            e.state("a").unwrap().status,
            Status::Cooling,
            "gap not over"
        );
        e.tick(t0 + 1000 + 5 * MIN_MS);
        assert_eq!(e.state("a").unwrap().status, Status::Armed);
        assert_eq!(fired(&feed(&mut e, &[(2.55, t0 + 6 * MIN_MS)])), 1);
    }

    #[test]
    fn stale_comparisons_are_held_not_fired() {
        // 41 minutes since the last print outside regular hours (> 30): held as stale.
        let t0 = et(4, 0, 0);
        let mut e = ev(vec![above(2.50, Fire::Once)]);
        let o = feed(&mut e, &[(2.40, t0), (2.60, t0 + 41 * MIN_MS)]);
        assert_eq!(held(&o), vec![HoldReason::Stale]);
        assert_eq!(
            e.state("a").unwrap().status,
            Status::Armed,
            "a hold changes nothing"
        );
        // Regular hours: 10 minutes is the limit.
        let t1 = et(11, 0, 0);
        let mut e = ev(vec![above(2.50, Fire::Once)]);
        let o = feed(&mut e, &[(2.40, t1), (2.60, t1 + 11 * MIN_MS)]);
        assert_eq!(held(&o), vec![HoldReason::Stale]);
        let mut e = ev(vec![above(2.50, Fire::Once)]);
        assert_eq!(
            fired(&feed(&mut e, &[(2.40, t1), (2.60, t1 + 9 * MIN_MS)])),
            1
        );
        // A print far older than the wall clock is stale too.
        let mut e = ev(vec![above(2.50, Fire::Once)]);
        e.on_input(trade(2.40, t1), t1, &NO_VIEW);
        let o = e.on_input(trade(2.60, t1 + 1000), t1 + 1000 + 20 * MIN_MS, &NO_VIEW);
        assert_eq!(held(&o), vec![HoldReason::Stale]);
    }

    #[test]
    fn sessions_filter_silently() {
        let mut r = above(2.50, Fire::Once);
        r.sessions = vec![Session::Regular];
        let mut e = ev(vec![r]);
        let pre = et(8, 0, 0);
        assert!(
            feed(&mut e, &[(2.40, pre), (2.60, pre + 1000)]).is_empty(),
            "pre-market cross ignored, not held"
        );
        let reg = et(9, 31, 0);
        assert_eq!(fired(&feed(&mut e, &[(2.40, reg), (2.60, reg + 1000)])), 1);
        assert_eq!(DEFAULT_SESSIONS.len(), 3);
    }

    #[test]
    fn halts_fire_halt_alerts_and_hold_the_rest() {
        let t0 = et(10, 0, 0);
        let mut e = ev(vec![
            above(2.50, Fire::Once),
            rule("h", Condition::Halt, Fire::Every { gap_min: 5 }),
        ]);
        feed(&mut e, &[(2.40, t0)]);
        let halt = Input::Status {
            symbol: "XLAB".into(),
            halted: true,
            ts: t0 + 1000,
            session: sess(t0 + 1000),
            reason: "LUDP".into(),
        };
        let o = e.on_input(halt.clone(), t0 + 1000, &NO_VIEW);
        assert_eq!(fired(&o), 1);
        assert!(matches!(&o[0], Outcome::Fired { text, .. } if text == "Halted · LUDP"));
        // A repeated halt is not news.
        assert!(e.on_input(halt, t0 + 1500, &NO_VIEW).is_empty());
        // A print crossing during the halt: the price alert is held ("halted"), once.
        let o = feed(
            &mut e,
            &[(2.60, t0 + 2000), (2.40, t0 + 3000), (2.61, t0 + 4000)],
        );
        assert_eq!(held(&o), vec![HoldReason::Halted]);
        // The resume comes inside the halt alert's gap (5 min): held as gap, after it: fired.
        let resume = |t: i64| Input::Status {
            symbol: "XLAB".into(),
            halted: false,
            ts: t,
            session: sess(t),
            reason: String::new(),
        };
        let o = e.on_input(resume(t0 + 2 * MIN_MS), t0 + 2 * MIN_MS, &NO_VIEW);
        assert_eq!(held(&o), vec![HoldReason::Gap]);
        let halt2 = Input::Status {
            symbol: "XLAB".into(),
            halted: true,
            ts: t0 + 7 * MIN_MS,
            session: sess(t0 + 7 * MIN_MS),
            reason: String::new(),
        };
        let o = e.on_input(halt2, t0 + 7 * MIN_MS, &NO_VIEW);
        assert!(matches!(&o[..], [Outcome::Fired { text, .. }] if text == "Halted"));
    }

    #[test]
    fn late_and_corrected_prints_fire_once_marked_late() {
        let t0 = et(10, 0, 0);
        let mut e = ev(vec![above(2.50, Fire::Every { gap_min: 15 })]);
        feed(&mut e, &[(2.40, t0), (2.45, t0 + 10_000)]);
        // A print from 9 s before the newest one (beyond the tolerance) at 2.55: would have crossed.
        let o = e.on_input(trade(2.55, t0 + 1000), t0 + 11_000, &NO_VIEW);
        assert_eq!(fired(&o), 1);
        assert!(
            matches!(&o[0], Outcome::Fired { late: true, t, text, .. } if *t == t0 + 1000 && text.ends_with("(late print)"))
        );
        // It did not move the reference: the newest price is still 2.45, so the next in-order
        // cross is held by the cooling gap, not treated as above → above.
        let o = feed(&mut e, &[(2.56, t0 + 12_000)]);
        assert_eq!(held(&o), vec![HoldReason::Gap]);
        // A correction's replacement print, on a fresh armed alert.
        let mut e = ev(vec![above(2.50, Fire::Once)]);
        feed(&mut e, &[(2.40, t0)]);
        let o = e.on_input(
            Input::Corrected {
                symbol: "XLAB".into(),
                price: 2.52,
                ts: t0 + 500,
                session: sess(t0),
            },
            t0 + 600,
            &NO_VIEW,
        );
        assert!(matches!(&o[..], [Outcome::Fired { late: true, .. }]));
        assert_eq!(e.state("a").unwrap().status, Status::Done, "fires once");
        // Within the tolerance it is just another print.
        let mut e = ev(vec![above(2.50, Fire::Once)]);
        let o = feed(&mut e, &[(2.40, t0 + 5000), (2.55, t0 + 4000)]);
        assert!(matches!(&o[..], [Outcome::Fired { late: false, .. }]));
    }

    #[test]
    fn expiry_and_snooze() {
        let t0 = et(10, 0, 0);
        let mut r = above(2.50, Fire::Once);
        r.expires_at = Some(t0 + MIN_MS);
        r.expiry = Expiry::EndOfDay;
        let mut s = rule(
            "s",
            Condition::Price {
                dir: CrossDir::Above,
                price: 2.50,
            },
            Fire::Once,
        );
        s.snooze_until = Some(t0 + 30 * MIN_MS);
        let mut e = ev(vec![r, s]);
        e.tick(t0);
        assert_eq!(e.state("a").unwrap().status, Status::Armed);
        // The snooze is the evaluator's verdict too, so no client judges it by its own clock.
        assert_eq!(e.state("a").unwrap().snoozed, Some(false));
        assert_eq!(e.state("s").unwrap().snoozed, Some(true));
        e.tick(t0 + MIN_MS);
        assert_eq!(e.state("a").unwrap().status, Status::Expired);
        assert_eq!(e.state("a").unwrap().snoozed, Some(false));
        e.tick(t0 + 30 * MIN_MS);
        assert_eq!(
            e.state("s").unwrap().snoozed,
            Some(false),
            "the snooze ran out"
        );
        let o = feed(&mut e, &[(2.40, t0 + 2 * MIN_MS), (2.60, t0 + 3 * MIN_MS)]);
        assert!(o.is_empty(), "expired and snoozed alerts stay silent");
        assert_eq!(e.state("s").unwrap().status, Status::Armed);
        let o = feed(
            &mut e,
            &[(2.40, t0 + 31 * MIN_MS), (2.60, t0 + 32 * MIN_MS)],
        );
        assert!(matches!(&o[..], [Outcome::Fired { alert_id, .. }] if alert_id == "s"));
    }

    #[test]
    fn an_edit_rearms_and_changes_are_reported() {
        let t0 = et(10, 0, 0);
        let mut e = ev(vec![above(2.50, Fire::Once)]);
        let (set, removed) = e.take_changes();
        assert_eq!(set.len(), 1);
        assert!(removed.is_empty());
        feed(&mut e, &[(2.49, t0), (2.51, t0 + 1)]);
        assert_eq!(e.state("a").unwrap().status, Status::Done);
        // Dragging the line (a newer editedAt) starts it over armed.
        let mut moved = above(2.60, Fire::Once);
        moved.edited_at = 2;
        e.set_rules(vec![moved], false, &NO_VIEW);
        assert_eq!(e.state("a").unwrap().status, Status::Armed);
        assert_eq!(fired(&feed(&mut e, &[(2.59, t0 + 2), (2.61, t0 + 3)])), 1);
        e.take_changes();
        e.set_rules(Vec::new(), false, &NO_VIEW);
        assert_eq!(e.take_changes().1, vec!["a".to_string()]);
    }

    #[test]
    fn percent_moves_are_edges_over_windows_and_references() {
        let t0 = et(10, 0, 0);
        let r = rule(
            "m",
            Condition::Move {
                dir: MoveDir::Up,
                pct: 5.0,
                window: MoveWindow::Minutes(5),
            },
            Fire::Every { gap_min: 5 },
        );
        let mut e = ev(vec![r]);
        let o = feed(
            &mut e,
            &[(1.00, t0), (1.02, t0 + 60_000), (1.05, t0 + 120_000)],
        );
        assert_eq!(fired(&o), 1);
        assert!(
            matches!(&o[0], Outcome::Fired { text, .. } if text == "Up 5.0% in 5 min, 1.00 → 1.05")
        );
        // Six minutes later the 1.00 low is out of the window: 1.06 is only +1% on it.
        let mut e = ev(vec![rule(
            "m",
            Condition::Move {
                dir: MoveDir::Up,
                pct: 5.0,
                window: MoveWindow::Minutes(5),
            },
            Fire::Once,
        )]);
        assert_eq!(
            fired(&feed(&mut e, &[(1.00, t0), (1.05, t0 + 6 * MIN_MS)])),
            0
        );
        // From the previous close: an edge, then quiet while it stays true.
        let r = rule(
            "c",
            Condition::Move {
                dir: MoveDir::Down,
                pct: 10.0,
                window: MoveWindow::PrevClose,
            },
            Fire::Every { gap_min: 5 },
        );
        let view = View {
            prev_close: Some(1.0),
            seed: None,
        };
        let mut e = Evaluator::new();
        e.set_rules(vec![r], false, &view);
        let mut outs = Vec::new();
        for (p, t) in [(0.95, t0), (0.89, t0 + 1000), (0.88, t0 + 2000)] {
            outs.extend(e.on_input(trade(p, t), t, &view));
        }
        assert_eq!(fired(&outs), 1);
        // Since the open: the first regular print is the open.
        let mut e = ev(vec![rule(
            "o",
            Condition::Move {
                dir: MoveDir::Either,
                pct: 3.0,
                window: MoveWindow::Open,
            },
            Fire::Once,
        )]);
        let o = feed(
            &mut e,
            &[
                (2.00, et(9, 30, 0)),
                (2.05, et(9, 31, 0)),
                (1.93, et(9, 32, 0)),
            ],
        );
        assert!(
            matches!(&o[..], [Outcome::Fired { text, .. }] if text.starts_with("Down 3.5% from the open"))
        );
    }

    #[test]
    fn startup_rules_learn_before_they_fire() {
        let t0 = et(10, 0, 0);
        let view = View {
            prev_close: Some(1.0),
            seed: None,
        };
        let mut e = Evaluator::new();
        e.load_states(vec![("c".to_string(), AlertState::armed(1))]);
        let r = rule(
            "c",
            Condition::Move {
                dir: MoveDir::Up,
                pct: 10.0,
                window: MoveWindow::PrevClose,
            },
            Fire::Once,
        );
        e.set_rules(vec![r], true, &view);
        // Already +20% when the server comes up: learned, not fired.
        assert!(e.on_input(trade(1.20, t0), t0, &view).is_empty());
        assert!(e.on_input(trade(1.25, t0 + 1), t0 + 1, &view).is_empty());
        assert_eq!(
            e.take_changes().0.len(),
            0,
            "a stored state is kept, not rewritten"
        );
    }

    #[test]
    fn luld_and_high_of_day() {
        let t0 = et(10, 0, 0);
        let mut e = ev(vec![
            rule(
                "l",
                Condition::Luld {
                    side: MoveDir::Up,
                    pct: 2.0,
                },
                Fire::Once,
            ),
            rule("h", Condition::HighOfDay, Fire::Once),
        ]);
        feed(&mut e, &[(2.00, t0)]);
        e.on_input(
            Input::Luld {
                symbol: "XLAB".into(),
                ts: t0,
                up: Some(2.20),
                down: Some(1.80),
            },
            t0,
            &NO_VIEW,
        );
        let o = feed(&mut e, &[(2.10, t0 + 1000)]);
        assert!(
            matches!(&o[..], [Outcome::Fired { alert_id, text, .. }] if alert_id == "h" && text == "New high of day 2.10 (was 2.00)")
        );
        let o = feed(&mut e, &[(2.17, t0 + 2000)]);
        assert!(
            matches!(&o[..], [Outcome::Fired { alert_id, text, .. }] if alert_id == "l" && text.contains("limit-up band (modelled)"))
        );
    }

    #[test]
    fn a_new_symbol_is_seeded_from_the_market() {
        let t0 = et(10, 0, 0);
        let view = View {
            prev_close: None,
            seed: Some(Seed {
                date: Some(day()),
                last: Some((t0 - 5000, 2.45)),
                open: Some(2.0),
                high: Some(2.48),
                ..Seed::default()
            }),
        };
        let mut e = Evaluator::new();
        e.set_rules(
            vec![
                above(2.50, Fire::Once),
                rule("h", Condition::HighOfDay, Fire::Once),
            ],
            false,
            &view,
        );
        let o = e.on_input(trade(2.51, t0), t0, &view);
        assert_eq!(
            fired(&o),
            2,
            "the seeded last price makes this a cross, the seeded high a new high"
        );
    }

    #[test]
    fn pressure_fires_on_the_edge_and_rearms_on_the_gap() {
        let t0 = et(10, 0, 0);
        let r = rule(
            "p",
            Condition::Pressure {
                side: MoveDir::Up,
                min: 80,
            },
            Fire::Every { gap_min: 5 },
        );
        let mut e = ev(vec![r]);
        assert_eq!(e.pressure_symbols(), vec!["XLAB".to_string()]);
        let p = |v: u8, bull: bool, t: i64| Input::Pressure {
            symbol: "XLAB".into(),
            pressure: v,
            bull,
            ts: t,
            session: sess(t),
        };
        assert!(e.on_input(p(70, true, t0), t0, &NO_VIEW).is_empty());
        let o = e.on_input(p(82, true, t0 + 1000), t0 + 1000, &NO_VIEW);
        assert!(
            matches!(&o[..], [Outcome::Fired { text, .. }] if text == "Buy pressure reached 82 (Halt watcher)")
        );
        // Still high: no new edge. Sell-side pressure never counts for a buy alert.
        assert!(
            e.on_input(p(90, true, t0 + 2000), t0 + 2000, &NO_VIEW)
                .is_empty()
        );
        assert!(
            e.on_input(p(95, false, t0 + 3000), t0 + 3000, &NO_VIEW)
                .is_empty()
        );
        // After the gap, a new edge fires again (no price to move back).
        let t1 = t0 + 7 * MIN_MS;
        assert!(e.on_input(p(85, true, t1), t1, &NO_VIEW).len() == 1);
    }

    #[test]
    fn unwatched_symbols_cost_nothing() {
        let mut e = ev(vec![above(2.50, Fire::Once)]);
        let o = e.on_input(
            Input::Trade {
                symbol: "OTHER".into(),
                price: 3.0,
                ts: 1,
                session: None,
            },
            1,
            &NO_VIEW,
        );
        assert!(o.is_empty());
        assert!(!e.syms.contains_key("OTHER"));
        assert_eq!(fmt_price(0.1904), "0.1904");
        assert_eq!(fmt_price(2.5), "2.50");
    }

    // ---- Drawing alerts ------------------------------------------------------------------

    use crate::alerts::geometry::{Part, Tolerance};
    use serde_json::json;

    /// A market view that knows the chart's slots (sloped lines).
    struct Slots(Vec<i64>);
    impl MarketView for Slots {
        fn prev_close(&self, _: &str, _: NaiveDate) -> Option<f64> {
            None
        }
        fn seed(&self, _: &str) -> Option<Seed> {
            None
        }
        fn slots(&self, _: &str, _: i64) -> Option<Vec<i64>> {
            Some(self.0.clone())
        }
    }

    fn on_drawing(id: &str, drawing: &str, part: Part, fire: Fire) -> Rule {
        rule(
            id,
            Condition::Drawing {
                drawing_id: drawing.into(),
                part,
                tolerance: Tolerance::Ticks(1),
                tf_ms: MIN_MS,
            },
            fire,
        )
    }

    fn hline(id: &str, p: f64) -> (String, serde_json::Value) {
        (
            id.to_string(),
            json!({"z": 0, "d": {"id": id, "kind": "hline", "a": {"t": 0, "p": p}, "b": {"t": 0, "p": p}}}),
        )
    }

    fn doc(entries: Vec<(String, serde_json::Value)>) -> serde_json::Value {
        serde_json::Value::Object(entries.into_iter().collect())
    }

    /// An evaluator with these rules and XLAB's drawings.
    fn drawn(rules: Vec<Rule>, drawings: serde_json::Value) -> Evaluator {
        let mut e = ev(rules);
        e.set_drawings("XLAB", Some(&drawings));
        e
    }

    fn fired_tones(o: &[Outcome]) -> Vec<(String, Option<Tone>)> {
        o.iter()
            .filter_map(|x| match x {
                Outcome::Fired { alert_id, tone, .. } => Some((alert_id.clone(), *tone)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_touch_within_the_tolerance_fires_outside_it_does_not() {
        // The owner's screenshot: a line at 1.85 after hours, one tick ($0.01) of tolerance.
        let t0 = et(17, 0, 0);
        let mut e = drawn(
            vec![on_drawing("d", "h", Part::Line, Fire::Once)],
            doc(vec![hline("h", 1.85)]),
        );
        let o = feed(&mut e, &[(1.90, t0), (1.87, t0 + 1000)]);
        assert!(o.is_empty(), "two ticks away is not a touch");
        let o = feed(&mut e, &[(1.86, t0 + 2000)]);
        assert_eq!(fired_tones(&o), vec![("d".into(), Some(Tone::Down))]);
        match &o[0] {
            Outcome::Fired {
                text,
                price,
                status,
                ..
            } => {
                assert_eq!(text, "Touched line 1.85 at 1.86 from above");
                assert_eq!(*price, Some(1.86));
                assert_eq!(*status, Status::Done);
            }
            x => panic!("{x:?}"),
        }
        // Coming up into it from below is the up tone.
        let mut e = drawn(
            vec![on_drawing("u", "h", Part::Line, Fire::Once)],
            doc(vec![hline("h", 1.85)]),
        );
        let o = feed(&mut e, &[(1.80, t0), (1.85, t0 + 1000)]);
        assert_eq!(fired_tones(&o), vec![("u".into(), Some(Tone::Up))]);
        // A percentage tolerance: 1% of 1.85 is 0.0185.
        let mut r = on_drawing("p", "h", Part::Line, Fire::Once);
        r.condition = Condition::Drawing {
            drawing_id: "h".into(),
            part: Part::Line,
            tolerance: Tolerance::Pct(1.0),
            tf_ms: MIN_MS,
        };
        let mut e = drawn(vec![r], doc(vec![hline("h", 1.85)]));
        assert!(feed(&mut e, &[(1.90, t0), (1.87, t0 + 1000)]).is_empty());
        assert_eq!(fired(&feed(&mut e, &[(1.868, t0 + 2000)])), 1);
    }

    #[test]
    fn a_jump_clean_through_the_line_counts() {
        // An illiquid after-hours print skipping the band entirely must not stay silent.
        let t0 = et(18, 30, 0);
        let mut e = drawn(
            vec![on_drawing("d", "h", Part::Line, Fire::Once)],
            doc(vec![hline("h", 1.85)]),
        );
        let o = feed(&mut e, &[(1.95, t0), (1.70, t0 + 1000)]);
        assert!(
            matches!(&o[..], [Outcome::Fired { text, tone: Some(Tone::Down), .. }] if text == "Crossed through line 1.85 at 1.70 from above")
        );
    }

    #[test]
    fn a_ray_follows_the_slots_past_b_and_a_trend_stops_at_its_anchors() {
        let t0 = et(17, 0, 0);
        let m = MIN_MS;
        // 17:00, :01, :02, a gap run from :03 to :10 (one slot), :10, :11.
        let view = Slots(vec![
            t0,
            t0 + m,
            t0 + 2 * m,
            t0 + 3 * m,
            t0 + 10 * m,
            t0 + 11 * m,
        ]);
        let shapes = doc(vec![
            (
                "r".into(),
                json!({"z": 0, "d": {"id": "r", "kind": "ray", "a": {"t": t0, "p": 1.00}, "b": {"t": t0 + 2 * m, "p": 1.10}}}),
            ),
            (
                "t".into(),
                json!({"z": 1, "d": {"id": "t", "kind": "trend", "a": {"t": t0, "p": 1.00}, "b": {"t": t0 + 2 * m, "p": 1.10}}}),
            ),
        ]);
        let mut e = Evaluator::new();
        e.set_rules(
            vec![
                on_drawing("ray", "r", Part::Line, Fire::Once),
                on_drawing("trend", "t", Part::Line, Fire::Once),
            ],
            false,
            &view,
        );
        e.set_drawings("XLAB", Some(&shapes));
        let mut run = |p: f64, t: i64| e.on_input(trade(p, t), t, &view);
        // At 17:11 the ray is 5 slots from a: 1.25 (in time it would be 11 minutes: 1.55).
        assert!(run(1.40, t0 + 11 * m).is_empty());
        let o = run(1.25, t0 + 11 * m + 5000);
        assert_eq!(
            fired_tones(&o),
            vec![("ray".into(), Some(Tone::Down))],
            "the ray, beyond b; the trend line ended at b"
        );
        // Into candles not printed yet: one slot per minute past the last (17:13 → 1.35).
        let mut e2 = Evaluator::new();
        e2.set_rules(
            vec![on_drawing("ray", "r", Part::Line, Fire::Once)],
            false,
            &view,
        );
        e2.set_drawings("XLAB", Some(&shapes));
        assert!(
            e2.on_input(trade(1.20, t0 + 13 * m), t0 + 13 * m, &view)
                .is_empty()
        );
        let o = e2.on_input(trade(1.35, t0 + 13 * m + 1), t0 + 13 * m + 1, &view);
        assert_eq!(fired(&o), 1);
        // The trend line inside its anchors: 17:01 is 1.05.
        let mut e3 = Evaluator::new();
        e3.set_rules(
            vec![on_drawing("trend", "t", Part::Line, Fire::Once)],
            false,
            &view,
        );
        e3.set_drawings("XLAB", Some(&shapes));
        let o = e3.on_input(trade(1.05, t0 + m + 1), t0 + m + 1, &view);
        assert_eq!(fired(&o), 1, "touching inside the anchors");
        // No slots known: a sloped line is not evaluated (never guessed on the clock).
        let mut e4 = ev(vec![on_drawing("ray", "r", Part::Line, Fire::Once)]);
        e4.set_drawings("XLAB", Some(&shapes));
        assert!(feed(&mut e4, &[(1.20, t0 + 11 * m), (1.25, t0 + 11 * m + 1)]).is_empty());
    }

    #[test]
    fn a_positions_entry_target_and_stop_are_each_watchable() {
        let t0 = et(19, 0, 0);
        let pos = doc(vec![(
            "p".into(),
            json!({"z": 0, "d": {"id": "p", "kind": "long", "a": {"t": 0, "p": 2.00}, "b": {"t": 1, "p": 2.40}, "c": {"t": 1, "p": 1.85}}}),
        )]);
        let mut e = drawn(
            vec![
                on_drawing("stop", "p", Part::Stop, Fire::Once),
                on_drawing("target", "p", Part::Target, Fire::Once),
                on_drawing("entry", "p", Part::Entry, Fire::Once),
            ],
            pos,
        );
        let o = feed(&mut e, &[(2.10, t0), (2.00, t0 + 1000)]);
        assert_eq!(fired_tones(&o), vec![("entry".into(), Some(Tone::Down))]);
        let o = feed(&mut e, &[(1.90, t0 + 2000), (1.85, t0 + 3000)]);
        assert_eq!(
            fired_tones(&o),
            vec![("stop".into(), Some(Tone::Stop))],
            "the stop has its own tone whichever way"
        );
        assert!(
            matches!(&o[0], Outcome::Fired { text, .. } if text == "Touched long stop 1.85 at 1.85 from above")
        );
        let o = feed(&mut e, &[(2.39, t0 + 4000)]);
        assert_eq!(fired_tones(&o), vec![("target".into(), Some(Tone::Up))]);
    }

    #[test]
    fn drawing_alerts_keep_the_sessions_halts_and_staleness_rules() {
        let mut r = on_drawing("d", "h", Part::Line, Fire::Once);
        r.sessions = vec![Session::After, Session::Overnight];
        let mut e = drawn(vec![r], doc(vec![hline("h", 1.85)]));
        let reg = et(15, 0, 0);
        assert!(
            feed(&mut e, &[(1.90, reg), (1.85, reg + 1000)]).is_empty(),
            "a regular-session touch is ignored silently"
        );
        let night = et(21, 0, 0);
        assert_eq!(
            fired(&feed(&mut e, &[(1.90, night), (1.85, night + 1000)])),
            1,
            "overnight is selectable"
        );
        // Halted: held, once.
        let t0 = et(17, 0, 0);
        let mut e = drawn(
            vec![on_drawing("d", "h", Part::Line, Fire::Once)],
            doc(vec![hline("h", 1.85)]),
        );
        feed(&mut e, &[(1.90, t0)]);
        e.on_input(
            Input::Status {
                symbol: "XLAB".into(),
                halted: true,
                ts: t0 + 500,
                session: sess(t0 + 500),
                reason: "LUDP".into(),
            },
            t0 + 500,
            &NO_VIEW,
        );
        let o = feed(&mut e, &[(1.85, t0 + 1000)]);
        assert_eq!(held(&o), vec![HoldReason::Halted]);
        assert_eq!(e.state("d").unwrap().status, Status::Armed);
        // Stale: 31 minutes since the last after-hours print.
        let mut e = drawn(
            vec![on_drawing("d", "h", Part::Line, Fire::Once)],
            doc(vec![hline("h", 1.85)]),
        );
        let o = feed(&mut e, &[(1.90, t0), (1.85, t0 + 31 * MIN_MS)]);
        assert_eq!(held(&o), vec![HoldReason::Stale]);
    }

    #[test]
    fn every_time_rearms_after_the_gap_and_a_half_percent_away_either_way() {
        // At 4.00 one tick (0.01) is a quarter of 0.5% (0.02): room to leave the band and come
        // back without having moved away enough to re-arm.
        let t0 = et(17, 0, 0);
        let mut e = drawn(
            vec![on_drawing("d", "h", Part::Line, Fire::Every { gap_min: 5 })],
            doc(vec![hline("h", 4.00)]),
        );
        assert_eq!(fired(&feed(&mut e, &[(4.10, t0), (4.00, t0 + 1000)])), 1);
        assert_eq!(e.state("d").unwrap().status, Status::Cooling);
        // Off the band and back inside the gap: held once.
        let o = feed(
            &mut e,
            &[
                (4.015, t0 + 2000),
                (4.00, t0 + 3000),
                (4.015, t0 + 4000),
                (4.005, t0 + 5000),
            ],
        );
        assert_eq!(held(&o), vec![HoldReason::Gap]);
        assert!(!e.state("d").unwrap().moved_back);
        // The gap passes while the price sits on the line: still cooling (it never moved away).
        let t1 = t0 + 6 * MIN_MS;
        assert!(feed(&mut e, &[(4.009, t1)]).is_empty());
        assert_eq!(e.state("d").unwrap().status, Status::Cooling);
        // 0.5% away ABOVE the fire price (it fired coming down): a touch has no side, so this
        // re-arms it, and the next touch fires.
        feed(&mut e, &[(4.021, t1 + 1000)]);
        assert_eq!(e.state("d").unwrap().status, Status::Armed);
        assert_eq!(fired(&feed(&mut e, &[(4.00, t1 + 2000)])), 1);
        assert_eq!(e.state("d").unwrap().count, 2);
    }

    #[test]
    fn a_deleted_drawing_orphans_the_alert_and_an_undo_brings_it_back() {
        let t0 = et(17, 0, 0);
        let mut e = drawn(
            vec![on_drawing("d", "h", Part::Line, Fire::Once)],
            doc(vec![hline("h", 1.85)]),
        );
        assert!(!e.state("d").unwrap().orphaned);
        e.set_drawings("XLAB", Some(&doc(vec![hline("other", 1.0)])));
        assert!(e.state("d").unwrap().orphaned);
        assert!(
            e.take_changes()
                .0
                .iter()
                .any(|(id, s)| id == "d" && s.orphaned)
        );
        assert!(
            feed(&mut e, &[(1.90, t0), (1.85, t0 + 1000)]).is_empty(),
            "a deleted drawing watches nothing"
        );
        e.set_drawings("XLAB", None);
        assert!(
            e.state("d").unwrap().orphaned,
            "the namespace gone: still orphaned"
        );
        e.set_drawings("XLAB", Some(&doc(vec![hline("h", 1.85)])));
        assert!(!e.state("d").unwrap().orphaned);
        assert_eq!(
            fired(&feed(&mut e, &[(1.90, t0 + 2000), (1.85, t0 + 3000)])),
            1
        );
    }

    #[test]
    fn an_edited_drawing_moves_the_alert_and_rearms_it() {
        let t0 = et(17, 0, 0);
        let mut e = drawn(
            vec![on_drawing("d", "h", Part::Line, Fire::Once)],
            doc(vec![hline("h", 1.85)]),
        );
        assert_eq!(fired(&feed(&mut e, &[(1.90, t0), (1.85, t0 + 1000)])), 1);
        assert_eq!(e.state("d").unwrap().status, Status::Done);
        // Dragged to 1.95 (the price is at 1.85): armed again, following the new level.
        e.set_drawings("XLAB", Some(&doc(vec![hline("h", 1.95)])));
        assert_eq!(e.state("d").unwrap().status, Status::Armed);
        assert!(feed(&mut e, &[(1.85, t0 + 2000)]).is_empty());
        let o = feed(&mut e, &[(1.95, t0 + 3000)]);
        assert!(
            matches!(&o[..], [Outcome::Fired { text, .. }] if text == "Touched line 1.95 at 1.95 from below")
        );
        // Dropped right onto the price: learned first, so it does not fire on the next print.
        e.set_drawings("XLAB", Some(&doc(vec![hline("h", 1.96)])));
        assert!(feed(&mut e, &[(1.96, t0 + 4000)]).is_empty());
        assert!(feed(&mut e, &[(1.90, t0 + 5000)]).is_empty());
        assert_eq!(fired(&feed(&mut e, &[(1.96, t0 + 6000)])), 1);
        // An unchanged document changes nothing.
        let before = e.state("d").cloned();
        e.set_drawings("XLAB", Some(&doc(vec![hline("h", 1.96)])));
        assert_eq!(e.state("d").cloned(), before);
    }

    #[test]
    fn price_alerts_are_untouched_by_drawings() {
        let t0 = et(17, 0, 0);
        let mut e = drawn(vec![above(2.50, Fire::Once)], doc(vec![hline("h", 2.50)]));
        let o = feed(&mut e, &[(2.49, t0), (2.50, t0 + 1000)]);
        assert!(matches!(&o[..], [Outcome::Fired { tone: None, .. }]));
        assert!(e.drawing_symbols().is_empty());
    }
}

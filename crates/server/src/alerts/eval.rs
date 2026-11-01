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

pub struct Evaluator {
    rules: HashMap<String, Rule>,
    by_symbol: HashMap<String, Vec<String>>,
    states: HashMap<String, AlertState>,
    runtime: HashMap<String, Runtime>,
    syms: HashMap<String, Sym>,
    /// States changed since the last [`Evaluator::take_changes`] (to persist), and removed ids.
    dirty: HashSet<String>,
    removed: HashSet<String>,
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
                &id, &rule, st, ts, None, bull, text, false, halted, false, now, out,
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
                    let back = if up {
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
            let (met, up, text) =
                match self.condition(&rule, &snapshot, prev, prev_high, prev_close, price, ts) {
                    Some(m) => m,
                    None => continue,
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
                now,
                out,
            );
        }
        let sym = self.syms.get_mut(symbol).expect("watched");
        sym.last = Some((ts, price));
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
            Condition::Halt | Condition::Pressure { .. } => None,
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
                &id, &rule, st, ts, None, halted, text, false, false, false, now, out,
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
}

//! Rocket watch (EXPERIMENTAL, `ROCKET_WATCH`, on by default): symbols a strategy flags as likely
//! to start an explosive bullish leg soon, so the owner sees them before the move.
//!
//! A "rocket" is the research bake-off's canonical event (`reports/rockets/definition.json`): a
//! leg of at least +30 % within 15 minutes from a start price in `$0.15–$25`, on 15 s bar closes,
//! with a liquidity floor. An alert is a HIT when such a leg starts 60–600 s after it.
//!
//! # Pieces
//!
//! - [`features`]: per-symbol rolling state on the research's 15 s grid, fed every print the chart
//!   engine counted (O(1) per print, under the core lock like the quote and pressure boards), and
//!   the research's 31 causal features at each completed barrier.
//! - [`scorer`]: the [`RocketScorer`] seam. The shipped scorer is `placeholder-b2` (the bake-off's
//!   volume-acceleration baseline); the research leader replaces it by changing
//!   [`scorer::production_scorer`].
//! - [`RocketBoard`] (here): the alert policy, the outcomes, the track record and the panel frame.
//! - [`log`]: the forward log on the archive volume (live market only).
//!
//! # Alert policy
//!
//! At each completed 15 s barrier (checked by the quotes task once per wall second, never on the
//! ingest path), every unit symbol is scored; candidates at or above the threshold become alerts,
//! strongest first, at most one per symbol per 30 minutes ([`DEDUP_NS`]).
//!
//! - **Budget** (default for a scorer without a frozen threshold): the threshold adapts so the day
//!   ends near `ROCKET_ALERTS_PER_DAY` alerts (default 10), which is also a hard cap. Each
//!   candidate's best score per symbol per 30-minute block approximates "one alert per episode";
//!   by time-of-day fraction `e` of 04:00–20:00, today's estimate is the `ceil(N·e)`-th largest
//!   of those. It is blended with the prior (the median of the last five days' `ideal_threshold`,
//!   the score that would have given exactly N that day, from the forward log), the prior
//!   weighing less as the day goes (all today's from 12:00). With no prior (the first day) no
//!   alert fires in the first hour. Never below the scorer's `floor`.
//! - **Fixed**: the scorer's frozen threshold (`RocketScorer::fixed_threshold`), or
//!   `ROCKET_THRESHOLD=<number>`; `ROCKET_THRESHOLD=budget` forces the adaptive one. Capped at
//!   `max(5·N, 50)` a day as a runaway guard.
//!
//! # Outcomes
//!
//! Each alert's outcome is computed after the fact from the same bars, causally (it only reads
//! bars that completed after the alert): the first canonical +30 % mark after the alert, its
//! start, and the max gain / max drawdown over the 30 minutes after it. `rocketed` once the start
//! of that leg is 60–600 s after the alert (the bake-off's HIT), `late` for 600–1,800 s, `moved`
//! when it went +30 % on a leg that started too early to count, `faded` when 45 minutes passed
//! without one. Settled outcomes go to the forward log.

pub mod features;
pub mod log;
pub mod scorer;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::NaiveDate;
use market_events::UnixNanos;
use serde_json::{Value, json};

pub use features::{FEATS, Features, NF, ix};
pub use log::{DaySummary, Loaded, RocketLog};
pub use scorer::{PlaceholderB2, RocketScorer, production_scorer, standard_reasons};

use features::{BAR_NS, Barrier, NSLOT, Tape};

pub const MIN_NS: i64 = 60_000_000_000;
/// At most one alert per symbol per 30 minutes.
pub const DEDUP_NS: i64 = 30 * MIN_NS;
pub const DEFAULT_BUDGET: u32 = 10;
/// Whatever the policy, never more alerts than this in a day (memory and log bound).
pub const MAX_ALERTS_PER_DAY: usize = 200;
/// When an outcome is final: a leg starting 30 minutes after the alert has its +30 % mark at most
/// 15 minutes later.
const SETTLE_NS: i64 = 45 * MIN_NS;
/// Max gain / drawdown window after the alert.
const GAIN_WINDOW_NS: i64 = 30 * MIN_NS;
/// Rows sent to the panel (newest first).
pub const ROWS: usize = 50;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Threshold {
    Budget,
    Fixed(f64),
}

/// `ROCKET_ALERTS_PER_DAY`, `ROCKET_THRESHOLD`.
#[derive(Clone, Debug, PartialEq)]
pub struct Policy {
    pub budget: u32,
    /// `None`: the scorer decides (its fixed threshold, else the budget).
    pub threshold: Option<Threshold>,
}

impl Default for Policy {
    fn default() -> Self {
        Policy {
            budget: DEFAULT_BUDGET,
            threshold: None,
        }
    }
}

impl Policy {
    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Result<Policy, String> {
        let var = |k: &str| {
            get(k)
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
        };
        let mut p = Policy::default();
        if let Some(v) = var("ROCKET_ALERTS_PER_DAY") {
            p.budget = v
                .parse::<u32>()
                .ok()
                .filter(|n| (1..=MAX_ALERTS_PER_DAY as u32).contains(n))
                .ok_or_else(|| {
                    format!("ROCKET_ALERTS_PER_DAY must be 1..={MAX_ALERTS_PER_DAY}: {v:?}")
                })?;
        }
        if let Some(v) = var("ROCKET_THRESHOLD") {
            p.threshold = Some(if v.eq_ignore_ascii_case("budget") {
                Threshold::Budget
            } else {
                Threshold::Fixed(v.parse::<f64>().ok().filter(|x| x.is_finite()).ok_or_else(
                    || format!("ROCKET_THRESHOLD is not a number or \"budget\": {v:?}"),
                )?)
            });
        }
        Ok(p)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Mark {
    pub start_ns: i64,
    pub mark_ns: i64,
    pub start_price: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Outcome {
    /// Bars through this key were looked at.
    checked: u32,
    pub high: f64,
    pub low: f64,
    pub mark: Option<Mark>,
    pub settled: bool,
    pub partial: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Alert {
    pub id: String,
    pub symbol: Arc<str>,
    pub at_ns: i64,
    pub price: f64,
    pub score: f64,
    pub threshold: f64,
    pub reasons: Vec<String>,
    pub strategy: String,
    pub version: String,
    pub outcome: Outcome,
}

impl Alert {
    fn start_offset(&self) -> Option<i64> {
        self.outcome.mark.map(|m| m.start_ns - self.at_ns)
    }
    /// The bake-off's HIT: a rocket starting 60–600 s after the alert.
    pub fn hit10(&self) -> bool {
        self.start_offset()
            .is_some_and(|d| (MIN_NS..=10 * MIN_NS).contains(&d))
    }
    /// The same out to 30 minutes.
    pub fn hit30(&self) -> bool {
        self.start_offset()
            .is_some_and(|d| (MIN_NS..=30 * MIN_NS).contains(&d))
    }
    pub fn max_gain_pct(&self) -> Option<f64> {
        (self.outcome.high.is_finite() && self.price > 0.0)
            .then(|| (self.outcome.high / self.price - 1.0) * 100.0)
    }
    pub fn max_drawdown_pct(&self) -> Option<f64> {
        (self.outcome.low.is_finite() && self.price > 0.0)
            .then(|| (self.outcome.low / self.price - 1.0) * 100.0)
    }
    /// `pending` | `rocketed` | `late` | `moved` | `faded` (module docs, "Outcomes").
    pub fn state(&self) -> &'static str {
        if self.hit10() {
            "rocketed"
        } else if self.hit30() {
            "late"
        } else if self.outcome.mark.is_some() {
            "moved"
        } else if self.outcome.settled {
            "faded"
        } else {
            "pending"
        }
    }

    fn alert_line(&self, date: NaiveDate, mode: &str) -> String {
        json!({
            "kind": "alert", "v": 1, "id": self.id, "date": date.to_string(),
            "symbol": &*self.symbol, "alert_ns": self.at_ns, "price": self.price,
            "score": self.score, "threshold": finite(self.threshold), "mode": mode,
            "reasons": self.reasons, "strategy": self.strategy, "version": self.version,
        })
        .to_string()
    }

    fn outcome_line(&self, date: NaiveDate, settled_ns: i64) -> String {
        let m = self.outcome.mark;
        json!({
            "kind": "outcome", "v": 1, "id": self.id, "date": date.to_string(),
            "symbol": &*self.symbol, "alert_ns": self.at_ns, "settled_ns": settled_ns,
            "rocket10": self.hit10(), "rocket30": self.hit30(),
            "start_ns": m.map(|m| m.start_ns), "mark_ns": m.map(|m| m.mark_ns),
            "start_price": m.map(|m| m.start_price),
            "max_gain_pct": self.max_gain_pct().map(round2),
            "max_drawdown_pct": self.max_drawdown_pct().map(round2),
            "partial": self.outcome.partial,
        })
        .to_string()
    }
}

/// The end (epoch ns) of the slot `key` on the day starting at `t0` with ordinal `ord`.
fn end_of(t0: i64, ord: u32, key: u32) -> i64 {
    let slot = i64::from(key) - i64::from(features::key_of(ord, 0));
    t0 + (slot + 1) * BAR_NS
}

fn finite(x: f64) -> Option<f64> {
    x.is_finite().then_some(x)
}

fn round2(x: f64) -> f64 {
    (x * 100.0).round() / 100.0
}

/// A symbol's last price and previous close (dollars), for the panel rows.
pub type Quote = (Option<f64>, Option<f64>);

/// What one barrier cost, for the logs and the cost test.
#[derive(Clone, Copy, Debug, Default)]
pub struct Cost {
    pub elapsed: Duration,
    pub symbols: usize,
    pub scored: usize,
}

pub struct RocketBoard {
    enabled: bool,
    pub(crate) tape: Tape,
    scorer: Arc<dyn RocketScorer>,
    policy: Policy,
    date: Option<NaiveDate>,
    t0: i64,
    ord: u32,
    processed: u32,
    alerts: Vec<Alert>,
    last_alert: HashMap<Arc<str>, i64>,
    block: Option<i64>,
    block_max: HashMap<Arc<str>, f64>,
    /// The day's largest finished block maxima (at most `budget`).
    top: Vec<f64>,
    priors: Vec<(NaiveDate, String, String, f64)>,
    days: Vec<DaySummary>,
    day_written: bool,
    threshold: f64,
    pending: Vec<(NaiveDate, String)>,
    revision: u64,
    last_cost: Option<Cost>,
}

impl RocketBoard {
    pub fn new(enabled: bool, scorer: Arc<dyn RocketScorer>, policy: Policy) -> Self {
        RocketBoard {
            enabled,
            tape: Tape::default(),
            scorer,
            policy,
            date: None,
            t0: 0,
            ord: 0,
            processed: 0,
            alerts: Vec::new(),
            last_alert: HashMap::new(),
            block: None,
            block_max: HashMap::new(),
            top: Vec::new(),
            priors: Vec::new(),
            days: Vec::new(),
            day_written: false,
            threshold: f64::INFINITY,
            pending: Vec::new(),
            revision: 0,
            last_cost: None,
        }
    }

    /// Off: takes no prints, ticks nothing.
    pub fn disabled() -> Self {
        Self::new(false, Arc::new(PlaceholderB2), Policy::default())
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    pub fn alerts(&self) -> &[Alert] {
        &self.alerts
    }

    pub fn last_cost(&self) -> Option<Cost> {
        self.last_cost
    }

    /// Changes whenever an alert or an outcome does.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// The effective threshold mode.
    pub fn mode(&self) -> Threshold {
        match self.policy.threshold {
            Some(t) => t,
            None => match self.scorer.fixed_threshold() {
                Some(x) => Threshold::Fixed(x),
                None => Threshold::Budget,
            },
        }
    }

    fn cap(&self) -> usize {
        let n = self.policy.budget as usize;
        match self.mode() {
            Threshold::Budget => n,
            Threshold::Fixed(_) => (5 * n).max(50),
        }
        .min(MAX_ALERTS_PER_DAY)
    }

    /// One print with the chart engine's verdict ([`features::Tape::apply_trade`]). O(1).
    pub fn apply_trade(&mut self, t: &market_events::Trade, counted: crate::quotes::Counted) {
        if self.enabled {
            self.tape.apply_trade(t, counted);
        }
    }

    /// The forward-log lines produced since the last take, by trading date.
    pub fn take_log(&mut self) -> Vec<(NaiveDate, String)> {
        std::mem::take(&mut self.pending)
    }

    /// The median of the last five days' ideal thresholds for this scorer.
    fn prior(&self) -> Option<f64> {
        let mut v: Vec<f64> = self
            .priors
            .iter()
            .rev()
            .filter(|(_, s, ver, t)| {
                s == self.scorer.id() && ver == self.scorer.version() && t.is_finite()
            })
            .take(5)
            .map(|p| p.3)
            .collect();
        if v.is_empty() {
            return None;
        }
        v.sort_by(f64::total_cmp);
        Some(v[v.len() / 2])
    }

    fn flush_block(&mut self) {
        let n = self.policy.budget.max(1) as usize;
        self.top.extend(self.block_max.drain().map(|(_, v)| v));
        self.top.sort_by(|a, b| b.total_cmp(a));
        self.top.truncate(n);
    }

    fn kth(&self, k: usize) -> Option<f64> {
        let mut vals: Vec<f64> = self.top.clone();
        vals.extend(self.block_max.values().copied());
        vals.sort_by(|a, b| b.total_cmp(a));
        vals.get(k.max(1) - 1).copied()
    }

    fn compute_threshold(&self, slot: i64) -> f64 {
        match self.mode() {
            Threshold::Fixed(t) => t,
            Threshold::Budget => {
                let n = f64::from(self.policy.budget.max(1));
                let e = (slot + 1) as f64 / NSLOT as f64;
                let today = self.kth((n * e).ceil() as usize);
                let est = match (self.prior(), today) {
                    (Some(p), Some(t)) => {
                        let w = (e * 2.0).min(1.0);
                        p * (1.0 - w) + t * w
                    }
                    (Some(p), None) => p,
                    (None, Some(t)) if slot >= 240 => t,
                    _ => f64::INFINITY,
                };
                est.max(self.scorer.floor())
            }
        }
    }

    /// The day ended (20:00 ET) or a new one began: settle what is open and write the day line.
    fn finish_day(&mut self, now: i64) {
        let Some(date) = self.date else { return };
        if self.day_written {
            return;
        }
        self.day_written = true;
        for i in 0..self.alerts.len() {
            if !self.alerts[i].outcome.settled {
                self.alerts[i].outcome.settled = true;
                self.alerts[i].outcome.partial = true;
                let line = self.alerts[i].outcome_line(date, now);
                self.pending.push((date, line));
            }
        }
        self.flush_block();
        let ideal = self
            .top
            .get(self.policy.budget.max(1) as usize - 1)
            .copied();
        let s = self.summary(date);
        self.pending.push((
            date,
            json!({
                "kind": "day", "v": 1, "date": date.to_string(), "alerts": s.alerts,
                "hits10": s.hits10, "hits30": s.hits30, "ideal_threshold": ideal.and_then(finite),
                "strategy": self.scorer.id(), "version": self.scorer.version(),
            })
            .to_string(),
        ));
        if let Some(t) = ideal.and_then(finite) {
            self.priors.push((
                date,
                self.scorer.id().to_string(),
                self.scorer.version().to_string(),
                t,
            ));
        }
        self.revision += 1;
    }

    fn summary(&self, date: NaiveDate) -> DaySummary {
        DaySummary {
            date,
            alerts: self.alerts.len() as u32,
            hits10: self.alerts.iter().filter(|a| a.hit10()).count() as u32,
            hits30: self.alerts.iter().filter(|a| a.hit30()).count() as u32,
        }
    }

    fn roll(&mut self, date: NaiveDate, now: i64) {
        if let Some(prev) = self.date {
            self.finish_day(now);
            let s = self.summary(prev);
            self.days.retain(|d| d.date != prev);
            self.days.push(s);
            let skip = self.days.len().saturating_sub(log::HISTORY_DAYS);
            self.days.drain(..skip);
        }
        let (t0, ord) = self.tape.day(date);
        self.date = Some(date);
        self.t0 = t0;
        self.ord = ord;
        self.processed = 0;
        self.alerts.clear();
        self.last_alert.clear();
        self.block = None;
        self.block_max.clear();
        self.top.clear();
        self.day_written = false;
        self.threshold = f64::INFINITY;
        self.revision += 1;
    }

    /// Processes the latest completed 15 s barrier at the market clock `now` (epoch ns), if it
    /// was not processed yet: scores, alerts, outcomes. Returns true when the alerts or their
    /// outcomes changed. `prev_close` is the quote board's previous close.
    pub fn tick(&mut self, now: i64, prev_close: impl Fn(&str) -> Option<f64>) -> bool {
        if !self.enabled {
            return false;
        }
        let rev = self.revision;
        // The tape may already hold prints a little past the clock (a replay at speed, a clock
        // frame behind the feed). The barrier is the slot before the one the newest print is in,
        // so a print past the barrier is always in the slot in progress, never in a finished
        // one; a print stamped implausibly far ahead (over a minute) cannot drag it along.
        let now = now.max(self.tape.latest_ts.min(now + MIN_NS));
        let date = market_events::session::trading_date(UnixNanos(now));
        if self.date != Some(date) {
            self.roll(date, now);
        }
        let mut slot = (now - self.t0).div_euclid(BAR_NS) - 1;
        if slot < 0 {
            return self.revision != rev;
        }
        let end_of_day = slot >= NSLOT - 1 && now >= self.t0 + NSLOT * BAR_NS;
        slot = slot.min(NSLOT - 1);
        let b = Barrier::new(self.t0, self.ord, slot);
        if b.key > self.processed {
            self.processed = b.key;
            self.barrier(&b, &prev_close);
        }
        if end_of_day {
            self.finish_day(now);
        }
        self.revision != rev
    }

    fn barrier(&mut self, b: &Barrier, prev_close: &dyn Fn(&str) -> Option<f64>) {
        let start = Instant::now();
        let mut cands: Vec<(f64, Features)> = Vec::new();
        let mut live = 0usize;
        let scorer = self.scorer.clone();
        for s in self.tape.syms.iter_mut() {
            if !features::settle(s, b) {
                continue;
            }
            let s: &features::Sym = s;
            live += 1;
            if let Some(f) = features::features(s, b, || prev_close(&s.symbol))
                && let Some(sc) = scorer.score(&f)
                && sc.is_finite()
            {
                cands.push((sc, f));
            }
        }
        let scored = cands.len();
        // Block maxima for the adaptive threshold (module docs).
        let block = b.slot / 120;
        if self.block != Some(block) {
            self.flush_block();
            self.block = Some(block);
        }
        for (sc, f) in &cands {
            let e = self.block_max.entry(f.symbol.clone()).or_insert(*sc);
            if *sc > *e {
                *e = *sc;
            }
        }
        self.threshold = self.compute_threshold(b.slot);
        cands.sort_by(|a, b| b.0.total_cmp(&a.0));
        let date = self.date.expect("rolled before a barrier");
        let mode = match self.mode() {
            Threshold::Budget => "budget",
            Threshold::Fixed(_) => "fixed",
        };
        for (sc, f) in cands {
            if sc < self.threshold || self.alerts.len() >= self.cap() {
                break;
            }
            if self
                .last_alert
                .get(&f.symbol)
                .is_some_and(|t| b.at_ns - t < DEDUP_NS)
            {
                continue;
            }
            let a = Alert {
                id: format!("{date}-{}-{}", f.symbol, b.slot),
                symbol: f.symbol.clone(),
                at_ns: b.at_ns,
                price: f.price,
                score: sc,
                threshold: self.threshold,
                reasons: scorer.reasons(&f),
                strategy: scorer.id().to_string(),
                version: scorer.version().to_string(),
                outcome: Outcome {
                    checked: b.key,
                    high: f64::NAN,
                    low: f64::NAN,
                    mark: None,
                    settled: false,
                    partial: false,
                },
            };
            self.pending.push((date, a.alert_line(date, mode)));
            self.last_alert.insert(f.symbol.clone(), b.at_ns);
            self.alerts.push(a);
            self.revision += 1;
        }
        self.outcomes(b, date);
        self.last_cost = Some(Cost {
            elapsed: start.elapsed(),
            symbols: live,
            scored,
        });
    }

    /// Advances every open outcome through barrier `b` (module docs, "Outcomes").
    fn outcomes(&mut self, b: &Barrier, date: NaiveDate) {
        for i in 0..self.alerts.len() {
            if self.alerts[i].outcome.settled {
                continue;
            }
            let before = self.alerts[i].outcome.clone();
            let at = self.alerts[i].at_ns;
            let (t0, ord) = (self.t0, self.ord);
            // A symbol with no state yet (a restart before its first print or history load):
            // leave `checked` where it is so the bars since the alert are read once they exist.
            let Some(s) = self.tape.sym(&self.alerts[i].symbol) else {
                continue;
            };
            {
                for (key, _close, high, low) in
                    features::bars_between(s, self.alerts[i].outcome.checked, b.key)
                {
                    let end = end_of(t0, ord, key);
                    let o = &mut self.alerts[i].outcome;
                    if end <= at + GAIN_WINDOW_NS {
                        o.high = if o.high.is_nan() {
                            high
                        } else {
                            o.high.max(high)
                        };
                        o.low = if o.low.is_nan() { low } else { o.low.min(low) };
                    }
                    if o.mark.is_none()
                        && let Some((start, l)) = features::mark_at(s, key)
                    {
                        o.mark = Some(Mark {
                            start_ns: end_of(t0, ord, start),
                            mark_ns: end,
                            start_price: l,
                        });
                    }
                }
            }
            let o = &mut self.alerts[i].outcome;
            o.checked = b.key;
            if b.at_ns >= at + SETTLE_NS {
                o.settled = true;
                let line = self.alerts[i].outcome_line(date, b.at_ns);
                self.pending.push((date, line));
            }
            if self.alerts[i].outcome != before {
                self.revision += 1;
            }
        }
    }

    /// Restores today's alerts and the record from the forward log (a restart).
    pub fn restore(&mut self, today: NaiveDate, loaded: Loaded) {
        self.days = loaded.days;
        self.priors = loaded.thresholds;
        if self.date != Some(today) {
            self.roll(today, 0);
            // A roll from "nothing" must not write a day line for no day.
            self.pending.clear();
        }
        let base = features::key_of(self.ord, 0);
        for (v, o) in loaded.today {
            let (Some(symbol), Some(at_ns)) = (
                v.get("symbol").and_then(Value::as_str),
                v.get("alert_ns").and_then(Value::as_i64),
            ) else {
                continue;
            };
            let f = |k: &str| v.get(k).and_then(Value::as_f64).unwrap_or(f64::NAN);
            let slot = ((at_ns - self.t0).div_euclid(BAR_NS) - 1).clamp(0, NSLOT - 1);
            let symbol: Arc<str> = Arc::from(symbol);
            let mut outcome = Outcome {
                checked: base + slot as u32,
                high: f64::NAN,
                low: f64::NAN,
                mark: None,
                settled: false,
                partial: false,
            };
            let price = f("price");
            if let Some(o) = o {
                let g = |k: &str| o.get(k).and_then(Value::as_f64);
                outcome.settled = true;
                outcome.partial = o.get("partial").and_then(Value::as_bool) == Some(true);
                outcome.high = g("max_gain_pct").map_or(f64::NAN, |p| price * (1.0 + p / 100.0));
                outcome.low = g("max_drawdown_pct").map_or(f64::NAN, |p| price * (1.0 + p / 100.0));
                if let (Some(s), Some(m)) = (
                    o.get("start_ns").and_then(Value::as_i64),
                    o.get("mark_ns").and_then(Value::as_i64),
                ) {
                    outcome.mark = Some(Mark {
                        start_ns: s,
                        mark_ns: m,
                        start_price: g("start_price").unwrap_or(f64::NAN),
                    });
                }
            }
            let str_of = |k: &str| {
                v.get(k)
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string()
            };
            let a = Alert {
                id: str_of("id"),
                symbol: symbol.clone(),
                at_ns,
                price,
                score: f("score"),
                threshold: f("threshold"),
                reasons: v
                    .get("reasons")
                    .and_then(Value::as_array)
                    .map(|r| {
                        r.iter()
                            .filter_map(|x| x.as_str().map(str::to_string))
                            .collect()
                    })
                    .unwrap_or_default(),
                strategy: str_of("strategy"),
                version: str_of("version"),
                outcome,
            };
            let e = self.last_alert.entry(symbol).or_insert(at_ns);
            *e = (*e).max(at_ns);
            if self.alerts.len() < MAX_ALERTS_PER_DAY {
                self.alerts.push(a);
            }
        }
        self.revision += 1;
    }

    /// The panel as a `rocket.snapshot` / `rocket.update` frame. `quote` gives a symbol's last
    /// price and previous close.
    pub fn frame(&self, kind: &str, t: Option<i64>, quote: impl Fn(&str) -> Quote) -> String {
        let body = self.body(&quote);
        let mut v = json!({ "type": kind, "t": t });
        if let (Some(m), Value::Object(b)) = (v.as_object_mut(), body) {
            m.extend(b);
        }
        v.to_string()
    }

    /// The frame without its type and clock: what decides whether an update is due.
    pub fn body(&self, quote: &dyn Fn(&str) -> Quote) -> Value {
        let today = self.date.map(|d| self.summary(d));
        let known = self
            .alerts
            .iter()
            .filter(|a| a.outcome.settled || a.hit10())
            .count();
        let rows: Vec<Value> = self
            .alerts
            .iter()
            .rev()
            .take(ROWS)
            .map(|a| {
                let (last, pc) = quote(&a.symbol);
                json!({
                    "id": a.id, "s": &*a.symbol, "at": a.at_ns / 1_000_000,
                    "p": a.price, "last": last, "pc": pc,
                    "score": (a.score * 100.0).round() / 100.0,
                    "reasons": a.reasons,
                    "outcome": {
                        "state": a.state(),
                        "gain": a.max_gain_pct().map(round2),
                        "drawdown": a.max_drawdown_pct().map(round2),
                        "partial": a.outcome.partial,
                    },
                })
            })
            .collect();
        let mode = match self.mode() {
            Threshold::Budget => "budget",
            Threshold::Fixed(_) => "fixed",
        };
        json!({
            "enabled": self.enabled,
            "strategy": {
                "id": self.scorer.id(), "version": self.scorer.version(),
                "placeholder": self.scorer.placeholder(),
            },
            "mode": mode,
            "budget": self.policy.budget,
            "threshold": finite(self.threshold).map(|x| (x * 1000.0).round() / 1000.0),
            "today": {
                "date": self.date.map(|d| d.to_string()),
                "alerts": today.as_ref().map_or(0, |s| s.alerts),
                "hits": today.as_ref().map_or(0, |s| s.hits10),
                "hits30": today.as_ref().map_or(0, |s| s.hits30),
                "known": known,
            },
            "days": self.days,
            "rows": rows,
        })
    }
}

/// The frame for a market whose board is off (`ROCKET_WATCH=0`).
pub fn disabled_frame(kind: &str) -> String {
    json!({
        "type": kind, "t": null, "enabled": false, "strategy": null, "mode": "budget",
        "budget": 0, "threshold": null,
        "today": {"date": null, "alerts": 0, "hits": 0, "hits30": 0, "known": 0},
        "days": [], "rows": [],
    })
    .to_string()
}

#[cfg(test)]
mod tests;

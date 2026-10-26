//! Buying bursts (owner, 2026-10-22: "alert me when MASSIVE BUYING hits the tape"): the
//! research detector `V3-5` of `docs/research/burst-onset/` (branch `research/burst-onset`,
//! protocol `703b19f`, frozen `d0d2156`), ported to the live server.
//!
//! # What it found, and what it is not
//!
//! On 35 unseen TEST days at 20 alerts a day, 23.4 % of its alerts went +20 % before −5 % within
//! 30 minutes (Wilson 95 % .204–.267) against 10.9 % for volatility-matched random barriers and
//! 9.9 % for a naive price-spike rule; 37 % reached +10 % first; the median 30-minute max gain was
//! +13.4 % and the median drawdown before that high −2.8 %. It fires about **5–9 minutes into a
//! move, roughly halfway**: it confirms heavy buying, it does not predict the first print.
//! The panel says exactly that and nothing more (no "buy").
//!
//! # The detector (protocol §2 granularity `5`, §4 gate and features, §5 alert policy)
//!
//! Per symbol, bounded state ([`BSym`]): the current second's accumulator (dollars, volume,
//! prints, tick-rule up/down volume, odd lots, up-tick prints ≥ $10k, prints that set a new day
//! high, a venue bitmask, first/last price), a ring of the last [`RING_S`] finished seconds, each
//! holding the day's running sums AFTER it (so a window is a difference of two running sums, the
//! research's prefix-sum arithmetic exactly, bit for bit), a ring of [`RING_B`] 15 s bars (running
//! sums at the bar's start, high, low) for the 30-minute base, and the day's tick reference and
//! direction, up-tick run, day high, first price, episode and last alert.
//!
//! - **Every counted print**: O(1) ([`BurstDetector::apply_trade`]). Seconds with no print are
//!   filled when the next print (or check) arrives; a gap longer than the rings is filled in one
//!   pass over the rings, never second by second.
//! - **Every 5 s boundary `w`** (checked at `w + 1 s`, from the quotes task's once-a-second tick,
//!   never on the ingest path): only symbols that printed in the last 15 s are looked at; for
//!   each, the **gate**: last price in `$0.10–$25`, session pre/regular/after, ≥ 5 volume-counted
//!   prints and ≥ $5,000 in the 15 s before `w`, `ret15 ≥ ln 1.015`, and the 15 s dollar rate ≥ 3×
//!   the base rate (the 120 completed 15 s bars ending ≤ `w − 30 s`, from 04:00).
//! - **Only on a gate pass**: the 34 features ([`FEATS`]), scored by the 300 trees
//!   ([`model::BurstModel`]). An alert at `w + 1 s` when the score ≥ θ (default the frozen
//!   20-a-day threshold) and the symbol had no alert in the last 10 minutes.
//! - An **episode** starts at a gate pass with no gate pass in the previous 10 minutes; `age`
//!   counts from it, `ep` counts the day's earlier ones. The row's "burst start" is that pass and
//!   its price the last price before that pass's 15 s window.
//!
//! The research scored rows read back from CSV files: each feature written with five decimals
//! (integers as integers) and read as `float32`. The live detector feeds the model exactly that
//! ([`model_input`]), so a live score equals the research score for the same tape.
//!
//! # Differences from the research
//!
//! - Entry eligibility is `$0.10–$25` (owner, 2026-10-26). The frozen study used
//!   `$0.15–$25`; its historical outcomes do not evaluate the added price range.
//! - Prints are taken in arrival order; a print stamped in a second already closed (late) is
//!   counted in the open second. The research sorted by execution time.
//! - A boundary is scored at `w + 1 s` with the prints that ARRIVED by then; a print executed
//!   before `w` that arrives later only counts from the next boundary.
//! - The alert price is the last price known at the check (the research: the last print executed
//!   ≤ `w + 1 s`).
//! - ADV30 comes from the archive's raw daily bars (the research's rule, [`facts::adv30`]),
//!   loaded in the background each trading day; until it is in, `radv30` and `rvol` are missing
//!   (the model has a learned direction for missing values). The previous close is the quote
//!   board's (the research: the latest date of `daily-closes.json`).
//! - A restart loses open outcome tracking (an alert's outcome is then `partial`). The day's
//!   tape state is rebuilt, see "After a restart" below.
//!
//! # After a restart
//!
//! A detector started mid-session sees only the prints from then on. Left alone, every symbol
//! would have an empty base for 30 minutes, so the rate gate passes on any 1.5 % tick (SHFS on
//! 2026-10-22: "2118× base"), and every day-level feature counts from the restart instead of
//! 04:00: `rvol` read −3.3 for a stock trading 20× its ADV, and `gap`, `dhi`, `vwap`, `ldd` and
//! `ep` were wrong too. Scores then move either way. In a replay of that day, XLABW went from
//! 0.222 (no alert) to 0.240 (an alert) on state that had been reset.
//!
//! So the live history load (`crate::history`) rebuilds each symbol's state the way it rebuilds
//! the chart: a [`BurstDetector::rebuilder`] gets the symbol's whole recorded day and the held
//! tail, ticked at every 5 s boundary on the history's own clock (episodes are reconstructed,
//! and no feature or model is computed), then [`BurstDetector::adopt`] swaps it in under the
//! core lock. Until then the symbol's prints still go into the detector, which marks the symbol
//! unrestored ([`BurstDetector::apply_trade_unrestored`]) and never gates or scores it: an alert
//! on reset state is worse than one a minute late. The detector keeps the live symbol's last
//! alert and open outcomes ([`BurstDetector::restore_alert`]), so nothing that fired before
//! the restart fires again.
//!
//! What can still differ from an uninterrupted run is only what arrival order can change: the
//! rebuild checks each boundary on the recorded delivery clock, the live run on the wall clock,
//! so a print delivered within the same second as a boundary's check may count from the next
//! boundary in one and not the other (`tests::a_restart_rebuilds_the_day`).
//!
//! # Outcomes (protocol §3), for the forward log
//!
//! From the alert price `P_a`, over the OHLC prints in `(t_a, t_a + 30 min]` (cut at 20:00 ET),
//! print by print: CONT10 / CONT20 / CONT30 = `P_a × 1.10 / 1.20 / 1.30` reached before any print
//! ≤ `P_a × 0.95`; MFE = max / `P_a` − 1; MAE = the lowest print before the MFE print.
//!
//! # Cost
//!
//! `rocket::burst::tests` measure it: a few tens of ns per print, a 5 s check over the symbols
//! that printed in the last 15 s in well under a millisecond, the 34 features and 300 trees only
//! on the ~2,000 gate passes a day. ~10 KB of rings per symbol that traded in the last 35 minutes
//! (rings of idle symbols are freed; their small day state stays).

pub mod facts;
pub mod model;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::NaiveDate;
use market_events::session::SessionKind;
use market_events::{Trade, UnixNanos};

use super::features::DayCache;
use crate::quotes::Counted;

pub use model::BurstModel;

/// The research features, in the research's order (`crates/setup-research/src/burst.rs`).
pub const FEATS: [&str; 34] = [
    "ld5", "ld15", "ld30", "rd5", "rd15", "rd30", "rt15", "radv30", "ret5", "ret15", "ret30",
    "brk", "buy5", "buy15", "upr", "nhd15", "nsw15", "vex15", "blk15", "odd15", "asz15", "logp",
    "sess", "tod", "gain", "gap", "rvol", "dhi", "vwap", "idle30", "rng30", "ldd", "ep", "age",
];
pub const NF: usize = FEATS.len();

const SEC: i64 = 1_000_000_000;
const MIN: i64 = 60 * SEC;
/// Seconds from 04:00 to 20:00 ET.
pub const NSEC: i64 = 57_600;
/// 15 s bars from 04:00 to 20:00 ET.
const NBAR: i64 = NSEC / 15;
/// Finished seconds kept per symbol (the features need the last 31).
pub const RING_S: usize = 64;
/// 15 s bars kept per symbol (the base needs 121 bar starts).
pub const RING_B: usize = 128;
/// Seconds between checks.
pub const STEP_S: i64 = 5;
/// A boundary is checked this long after it (protocol §2, granularity `5`).
pub const SETTLE_NS: i64 = SEC;
/// A gate pass with no pass in the previous 10 minutes starts an episode.
const EPISODE_GAP_NS: i64 = 10 * MIN;
/// At most one alert per symbol per 10 minutes (protocol §5).
pub const DEDUP_NS: i64 = 10 * MIN;
/// The outcome horizon (protocol §3).
pub const HORIZON_NS: i64 = 30 * MIN;
/// Rings of symbols idle this long are freed (the base then holds nothing of before).
const IDLE_FREE_S: i64 = 35 * 60;
/// A check that falls this far behind skips to the newest boundary (a stall's catch-up would
/// alert on the past).
const MAX_LAG_S: i64 = 15;
/// Current entry band, micro-dollars (owner, 2026-10-26).
pub const BAND: (i64, i64) = (
    market_events::PriceBand::DEFAULT.min.micros(),
    market_events::PriceBand::DEFAULT.max.micros(),
);
/// Rings are kept only for symbols printing in this wider band (micro-dollars); every symbol
/// keeps its small day state, as the research kept every symbol it did not prefilter.
const WIDE_BAND: (i64, i64) = (50_000, 30_000_000);

/// Gate (protocol §4).
const G_RET: f64 = 0.015;
const G_TRADES: f64 = 5.0;
const G_DOLLARS: f64 = 5_000.0;
const G_RD: f64 = 3.0;

/// Frozen thresholds (`frozen.json`): the score giving 10 / 20 / 40 alerts a day on DEV.
pub const THETA_B10: f64 = 0.2618383515154925;
pub const THETA_B20: f64 = 0.2334402054046066;
pub const THETA_B40: f64 = 0.20275486658803638;
pub const STRATEGY: &str = "burst-V3-5";
pub const VERSION: &str = "d0d2156";

/// What the TEST scoring found at 20 a day (fixed θ, `reports/burst/test/results.json`), for the
/// panel's "typical from here" line. Descriptive, not a promise.
pub const TYPICAL_MFE30_MED_PCT: f64 = 13.4;
pub const TYPICAL_MAE_MED_PCT: f64 = -2.8;
pub const TYPICAL_CONT10_PCT: f64 = 37.0;
pub const TYPICAL_CONT20_PCT: f64 = 23.4;
pub const TYPICAL_FIRES_AFTER_MIN: (u32, u32) = (5, 9);

/// `BURST_ALERTS`, `BURST_ALERTS_PER_DAY`, `BURST_THRESHOLD`.
#[derive(Clone, Debug, PartialEq)]
pub struct BurstPolicy {
    pub enabled: bool,
    /// The frozen budget the default threshold belongs to: 10, 20 or 40.
    pub per_day: u32,
    pub threshold: f64,
}

impl Default for BurstPolicy {
    fn default() -> Self {
        BurstPolicy {
            enabled: true,
            per_day: 20,
            threshold: THETA_B20,
        }
    }
}

impl BurstPolicy {
    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Result<BurstPolicy, String> {
        let var = |k: &str| {
            get(k)
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
        };
        let mut p = BurstPolicy::default();
        if let Some(v) = var("BURST_ALERTS") {
            p.enabled = match v.as_str() {
                "1" | "true" | "on" | "yes" => true,
                "0" | "false" | "off" | "no" => false,
                _ => return Err(format!("BURST_ALERTS must be 0 or 1: {v:?}")),
            };
        }
        if let Some(v) = var("BURST_ALERTS_PER_DAY") {
            (p.per_day, p.threshold) = match v.as_str() {
                "10" => (10, THETA_B10),
                "20" => (20, THETA_B20),
                "40" => (40, THETA_B40),
                _ => {
                    return Err(format!(
                        "BURST_ALERTS_PER_DAY must be 10, 20 or 40 (the frozen budgets): {v:?}"
                    ));
                }
            };
        }
        if let Some(v) = var("BURST_THRESHOLD") {
            p.threshold = v
                .parse::<f64>()
                .ok()
                .filter(|x| x.is_finite() && *x > 0.0 && *x < 1.0)
                .ok_or_else(|| format!("BURST_THRESHOLD must be a number in (0, 1): {v:?}"))?;
        }
        Ok(p)
    }

    /// A runaway guard on alerts a day (a broken feed must not page the owner 500 times).
    pub fn cap(&self) -> u32 {
        (self.per_day * 5).clamp(50, 200)
    }
}

/// Venue code → bit (the research's `venue`: A..Z → 0..25, anything else folded into 26..31).
fn venue(x: &str) -> u8 {
    match x.as_bytes().first() {
        Some(b) if b.is_ascii_uppercase() => b - b'A',
        Some(b) => 26 + (b % 6),
        None => 31,
    }
}

/// One second's accumulator (the research's `SecAcc`).
#[derive(Clone, Copy, Default, Debug)]
struct Acc {
    dollars: f64,
    vol: f64,
    trades: f64,
    upv: f64,
    dnv: f64,
    odd: f64,
    blkup: f64,
    nh: f64,
    venues: u32,
    first: f64,
    last_v: f64,
    hi: f64,
    lo: f64,
    has_ohlc: bool,
}

impl Acc {
    fn sweep(&self) -> bool {
        self.trades >= 3.0
            && self.venues.count_ones() >= 2
            && self.last_v > self.first
            && self.upv + self.dnv > 0.0
            && self.upv >= 2.0 / 3.0 * (self.upv + self.dnv)
    }
}

/// The day's running sums (the research's prefix arrays at one second).
#[derive(Clone, Copy, Default, Debug, PartialEq)]
struct Cum {
    d: f64,
    v: f64,
    t: f64,
    up: f64,
    dn: f64,
    odd: f64,
    blk: f64,
    nh: f64,
    sw: f64,
}

/// A finished second: the running sums after it and the tape state at its end.
#[derive(Clone, Copy, Debug)]
struct SecRec {
    sec: i64,
    cum: Cum,
    vex: u8,
    last: f64,
    dayhi: f64,
    upr: u32,
}

impl Default for SecRec {
    fn default() -> Self {
        SecRec {
            sec: -1,
            cum: Cum::default(),
            vex: 0,
            last: f64::NAN,
            dayhi: f64::NAN,
            upr: 0,
        }
    }
}

/// A 15 s bar: the running dollars / prints at its start, its OHLC high / low (NaN if none).
#[derive(Clone, Copy, Debug)]
struct BarRec {
    bar: i64,
    d: f64,
    t: f64,
    hi: f64,
    lo: f64,
}

impl Default for BarRec {
    fn default() -> Self {
        BarRec {
            bar: -1,
            d: 0.0,
            t: 0.0,
            hi: f64::NAN,
            lo: f64::NAN,
        }
    }
}

struct Rings {
    s: [SecRec; RING_S],
    b: [BarRec; RING_B],
}

impl Rings {
    fn boxed() -> Box<Rings> {
        Box::new(Rings {
            s: [SecRec::default(); RING_S],
            b: [BarRec::default(); RING_B],
        })
    }
}

/// Window sums (the research's `W`).
#[derive(Clone, Copy, Default, Debug)]
struct W {
    dollars: f64,
    trades: f64,
    upv: f64,
    dnv: f64,
    odd: f64,
    blkup: f64,
    nh: f64,
    sw: f64,
}

/// One alert's outcome while it is open (protocol §3, print by print).
#[derive(Clone, Debug, PartialEq)]
pub struct Outcome {
    pub seq: u32,
    pub at_ns: i64,
    pub pa: f64,
    pub t_up: [Option<i64>; 3],
    pub t_stop: Option<i64>,
    pub mfe_p: f64,
    pub mae_at_mfe: f64,
    runmin: f64,
    pub settled: bool,
    /// Not followed for the whole 30 minutes (a restart, or 20:00 came first).
    pub partial: bool,
}

impl Outcome {
    pub fn new(seq: u32, at_ns: i64, pa: f64) -> Outcome {
        Outcome {
            seq,
            at_ns,
            pa,
            t_up: [None; 3],
            t_stop: None,
            mfe_p: pa,
            mae_at_mfe: pa,
            runmin: pa,
            settled: false,
            partial: false,
        }
    }

    fn print(&mut self, exec: i64, p: f64) {
        if exec <= self.at_ns || exec > self.at_ns + HORIZON_NS || self.settled {
            return;
        }
        let pa = self.pa;
        for (x, m) in [1.10, 1.20, 1.30].into_iter().enumerate() {
            if self.t_up[x].is_none() && p >= m * pa - 1e-12 {
                self.t_up[x] = Some(exec);
            }
        }
        if self.t_stop.is_none() && p <= 0.95 * pa + 1e-12 {
            self.t_stop = Some(exec);
        }
        if p > self.mfe_p {
            self.mfe_p = p;
            self.mae_at_mfe = self.runmin;
        }
        self.runmin = self.runmin.min(p);
    }

    /// CONT10 / CONT20 / CONT30: the level before any stop.
    pub fn cont(&self) -> [bool; 3] {
        [0, 1, 2].map(|x| self.t_up[x].is_some_and(|u| self.t_stop.is_none_or(|d| u < d)))
    }
    /// The −5 % stop came before +10 %.
    pub fn stopped(&self) -> bool {
        self.t_stop
            .is_some_and(|d| self.t_up[0].is_none_or(|u| d < u))
    }
    pub fn mfe_pct(&self) -> f64 {
        (self.mfe_p / self.pa - 1.0) * 100.0
    }
    pub fn mae_pct(&self) -> f64 {
        (self.mae_at_mfe.min(self.pa) / self.pa - 1.0) * 100.0
    }
    /// `pending` | `cont30` | `cont20` | `cont10` | `stopped` | `faded`.
    pub fn state(&self) -> &'static str {
        let c = self.cont();
        if c[2] {
            "cont30"
        } else if c[1] && self.settled {
            "cont20"
        } else if c[0] && self.settled {
            "cont10"
        } else if self.stopped() {
            "stopped"
        } else if self.settled {
            "faded"
        } else if c[1] {
            "cont20"
        } else if c[0] {
            "cont10"
        } else {
            "pending"
        }
    }
    /// Seconds from the alert to the +20 % touch (when CONT20).
    pub fn t20_s(&self) -> Option<f64> {
        self.cont()[1].then(|| (self.t_up[1].unwrap_or(self.at_ns) - self.at_ns) as f64 / 1e9)
    }
}

/// One symbol's state for its trading date.
pub(crate) struct BSym {
    pub(crate) symbol: Arc<str>,
    day: u32,
    rings: Option<Box<Rings>>,
    /// Seconds `< fin` are finished; `acc` is second `fin`.
    fin: i64,
    acc: Acc,
    cum: Cum,
    tref: Option<i64>,
    dir: i8,
    upr: u32,
    dayhi: Option<f64>,
    last: f64,
    first: f64,
    /// The newest second a print fell in (-1 = none today).
    last_print_s: i64,
    in_recent: bool,
    ep_last: Option<i64>,
    ep_start: i64,
    ep_count: f64,
    /// The price before the episode's first passing window.
    ep_ref: f64,
    last_alert: Option<i64>,
    open: Vec<Outcome>,
    /// Its prints since a restart are in, its day before them not yet: never gated or scored
    /// until the history load adopts its rebuilt day ([`BurstDetector::adopt`]).
    unrestored: bool,
}

impl BSym {
    fn new(symbol: Arc<str>) -> BSym {
        BSym {
            symbol,
            day: 0,
            rings: None,
            fin: 0,
            acc: Acc::default(),
            cum: Cum::default(),
            tref: None,
            dir: 0,
            upr: 0,
            dayhi: None,
            last: f64::NAN,
            first: f64::NAN,
            last_print_s: -1,
            in_recent: false,
            ep_last: None,
            ep_start: 0,
            ep_count: 0.0,
            ep_ref: f64::NAN,
            last_alert: None,
            open: Vec::new(),
            unrestored: false,
        }
    }

    fn reset_day(&mut self, day: u32) {
        let symbol = self.symbol.clone();
        let (in_recent, unrestored) = (self.in_recent, self.unrestored);
        *self = BSym::new(symbol);
        self.in_recent = in_recent;
        self.unrestored = unrestored;
        self.day = day;
    }

    /// Finishes second `fin` from `acc` (the research's `finalize`). Without rings (a symbol
    /// outside the wide band, or idle) only the day's running sums move.
    fn finalize_one(&mut self) {
        let k = self.fin;
        let j = k / 15;
        if k % 15 == 0
            && let Some(r) = self.rings.as_mut()
        {
            r.b[j as usize % RING_B] = BarRec {
                bar: j,
                d: self.cum.d,
                t: self.cum.t,
                hi: f64::NAN,
                lo: f64::NAN,
            };
        }
        let a = &self.acc;
        let c = &mut self.cum;
        c.d += a.dollars;
        c.v += a.vol;
        c.t += a.trades;
        c.up += a.upv;
        c.dn += a.dnv;
        c.odd += a.odd;
        c.blk += a.blkup;
        c.nh += a.nh;
        c.sw += f64::from(u8::from(a.sweep()));
        if let Some(r) = self.rings.as_mut() {
            if a.has_ohlc {
                let b = &mut r.b[j as usize % RING_B];
                if b.bar == j {
                    b.hi = if b.hi.is_nan() { a.hi } else { b.hi.max(a.hi) };
                    b.lo = if b.lo.is_nan() { a.lo } else { b.lo.min(a.lo) };
                }
            }
            r.s[k as usize % RING_S] = SecRec {
                sec: k,
                cum: *c,
                vex: a.venues.count_ones() as u8,
                last: self.last,
                dayhi: self.dayhi.unwrap_or(f64::NAN),
                upr: self.upr,
            };
        }
        self.acc = Acc::default();
        self.fin += 1;
    }

    /// Finishes every second before `k`. A long gap of empty seconds is written straight into
    /// the rings (their running sums do not change), never second by second.
    fn advance_to(&mut self, k: i64) {
        if self.fin >= k {
            return;
        }
        self.finalize_one();
        if k - self.fin <= 20 {
            while self.fin < k {
                self.finalize_one();
            }
            return;
        }
        let (from, c) = (self.fin, self.cum);
        self.fin = k;
        let Some(r) = self.rings.as_mut() else {
            return;
        };
        for s in from.max(k - RING_S as i64)..k {
            r.s[s as usize % RING_S] = SecRec {
                sec: s,
                cum: c,
                vex: 0,
                last: self.last,
                dayhi: self.dayhi.unwrap_or(f64::NAN),
                upr: self.upr,
            };
        }
        let (j0, j1) = ((from + 14) / 15, (k - 1) / 15);
        for j in j0.max(j1 - RING_B as i64 + 1)..=j1 {
            r.b[j as usize % RING_B] = BarRec {
                bar: j,
                d: c.d,
                t: c.t,
                hi: f64::NAN,
                lo: f64::NAN,
            };
        }
    }

    /// One counted print in second `k` (the research's `apply`).
    fn apply(&mut self, k: i64, price: i64, size: u64, c: Counted, ex: u8) {
        // A late print is counted in the open second (module docs, "Differences").
        let k = k.max(self.fin);
        if self.rings.is_none() && in_wide_band(price) {
            // The first print in the wide band today, or the first after the rings were freed
            // while idle. The open second may hold prints: finish it before the rings start, so
            // a ring never holds a second it did not see whole. The empty seconds after it are
            // filled by `advance_to` below. A ring built mid-day holds no bar from before: for a
            // freed one no base window reaches those any more; a symbol entering the band waits
            // (the base lookups fail) until its base is in the ring.
            if self.fin < k && (self.acc.trades > 0.0 || self.acc.has_ohlc) {
                self.finalize_one();
            }
            self.rings = Some(Rings::boxed());
        }
        self.advance_to(k);
        let p = price as f64 / 1e6;
        let acc = &mut self.acc;
        if c.ohlc {
            if let Some(h) = self.dayhi
                && p > h
            {
                acc.nh += 1.0;
            }
            self.dayhi = Some(self.dayhi.map_or(p, |h| h.max(p)));
            if acc.has_ohlc {
                acc.hi = acc.hi.max(p);
                acc.lo = acc.lo.min(p);
            } else {
                acc.hi = p;
                acc.lo = p;
                acc.has_ohlc = true;
            }
        }
        if c.volume {
            let v = size as f64;
            acc.vol += v;
            acc.dollars += v * p;
            acc.trades += 1.0;
            if size < 100 {
                acc.odd += 1.0;
            }
            match self.tref {
                Some(r) if price > r => {
                    self.dir = 1;
                    self.upr += 1;
                }
                Some(r) if price < r => {
                    self.dir = -1;
                    self.upr = 0;
                }
                _ => {}
            }
            self.tref = Some(price);
            if self.dir > 0 {
                acc.upv += v;
                if v * p >= 10_000.0 {
                    acc.blkup += v * p;
                }
            } else if self.dir < 0 {
                acc.dnv += v;
            }
            if acc.trades == 1.0 {
                acc.first = p;
            }
            acc.last_v = p;
            acc.venues |= 1 << ex;
        }
        if c.ohlc {
            self.last = p;
            if self.first.is_nan() {
                self.first = p;
            }
        }
        self.last_print_s = self.last_print_s.max(k);
    }

    fn sec(&self, s: i64) -> Option<&SecRec> {
        let r = &self.rings.as_ref()?.s[s.rem_euclid(RING_S as i64) as usize];
        (s >= 0 && r.sec == s).then_some(r)
    }

    /// The running sums after `x` seconds (`pd[x]` in the research).
    fn pd(&self, x: i64) -> Option<Cum> {
        if x <= 0 {
            return Some(Cum::default());
        }
        self.sec(x - 1).map(|r| r.cum)
    }

    fn bar(&self, j: i64) -> Option<&BarRec> {
        let r = &self.rings.as_ref()?.b[j.rem_euclid(RING_B as i64) as usize];
        (j >= 0 && r.bar == j).then_some(r)
    }

    /// Sums over full seconds `[a, b)` (the research's `Secs::win` with no partial second).
    fn win(&self, a: i64, b: i64) -> Option<W> {
        let (pa, pb) = (self.pd(a)?, self.pd(b)?);
        let s = |x: f64, y: f64| if b <= a { 0.0 } else { y - x };
        Some(W {
            dollars: s(pa.d, pb.d),
            trades: s(pa.t, pb.t),
            upv: s(pa.up, pb.up),
            dnv: s(pa.dn, pb.dn),
            odd: s(pa.odd, pb.odd),
            blkup: s(pa.blk, pb.blk),
            nh: s(pa.nh, pb.nh),
            sw: s(pa.sw, pb.sw),
        })
    }
}

fn sess_code(t: i64) -> Option<f64> {
    match market_events::session::classify(UnixNanos(t)).map(|s| s.kind) {
        Some(SessionKind::Pre) => Some(0.0),
        Some(SessionKind::Regular) => Some(1.0),
        Some(SessionKind::After) => Some(2.0),
        _ => None,
    }
}

/// A price (micro-dollars) in the band rings are kept for.
pub fn in_wide_band(price: i64) -> bool {
    (WIDE_BAND.0..=WIDE_BAND.1).contains(&price)
}

fn in_band(last: f64) -> bool {
    let m = (last * 1e6).round() as i64;
    (BAND.0..=BAND.1).contains(&m)
}

/// The base window (protocol §4) at `t`: seconds `[a, b)` and its length in bars.
fn base(t0: i64, t: i64) -> (i64, i64, i64) {
    let be = (t - 30 * SEC - t0).div_euclid(15 * SEC).clamp(0, NBAR);
    let ba = (be - 120).max(0);
    (ba, be, be - ba)
}

/// Outside facts for one symbol (the research's `Ctx`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Ctx {
    pub prev_close: Option<f64>,
    pub adv30: Option<f64>,
}

/// A gate pass, with what the row and the reasons need.
#[derive(Clone, Debug)]
pub struct Pass {
    pub symbol: Arc<str>,
    /// The boundary `w` (epoch ns) and its second from 04:00.
    pub t_ns: i64,
    pub hi: i64,
    pub last: f64,
    /// The features as computed and as the model saw them.
    pub x: [f64; NF],
    pub input: [f64; NF],
    pub score: f64,
    pub ep_start_ns: i64,
    pub ep_ref: f64,
    /// `(w$/15 + 1) / (base$/s + 1)`: the dollar rate as a multiple of the base.
    pub rate_x: f64,
    pub new_highs: f64,
    pub sweeps: f64,
    pub buy15: f64,
    pub blk15: f64,
}

/// What the model sees for a feature: written with five decimals (integers as integers, NaN as
/// empty) and read back as `float32`, as the research's CSV round trip did.
pub fn model_input(x: f64) -> f64 {
    if x.is_nan() {
        return f64::NAN;
    }
    let v = if x == x.trunc() && x.abs() < 1e9 {
        (x as i64) as f64
    } else {
        format!("{x:.5}").parse::<f64>().unwrap_or(x)
    };
    f64::from(v as f32)
}

impl BSym {
    /// The gate at boundary `hi` (seconds from 04:00): `Some((ret15, reference price, rate))`.
    fn gate(&self, t0: i64, hi: i64) -> Option<(f64, f64, f64)> {
        let t = t0 + hi * SEC;
        let last = self.sec(hi - 1)?.last;
        if last.is_nan() || last <= 0.0 || !in_band(last) {
            return None;
        }
        let ws = (hi - 15).max(0);
        let w = self.win(ws, hi)?;
        if w.trades < G_TRADES || w.dollars < G_DOLLARS || ws == 0 {
            return None;
        }
        let r = self.sec(ws - 1)?.last;
        if r.is_nan() {
            return None;
        }
        let ret = (last / r).ln();
        if ret < (1.0 + G_RET).ln() {
            return None;
        }
        let (ba, be, _) = base(t0, t);
        let (a, b) = (ba * 15, be * 15);
        let base = if b > a {
            let (x, y) = (self.bar(ba)?, self.bar(be)?);
            (y.d - x.d) / ((b - a).max(1) as f64)
        } else {
            0.0
        };
        let rate = (w.dollars / 15.0 + 1.0) / (base + 1.0);
        if rate < G_RD {
            return None;
        }
        sess_code(t)?;
        Some((ret, r, rate))
    }

    /// The 34 features at boundary `hi` (the research's `Eval::features` with no partial
    /// second). `None` only if the rings lack a record they should hold (never in practice).
    fn features(&self, t0: i64, hi: i64, c: &Ctx, ep: f64, age: f64) -> Option<([f64; NF], W)> {
        let t = t0 + hi * SEC;
        let last = self.sec(hi - 1)?.last;
        let mut f = [f64::NAN; NF];
        let wn = |n: i64| -> Option<(i64, W)> {
            let ws = (hi - n).max(0);
            Some((ws, self.win(ws, hi)?))
        };
        let (ws5, w5) = wn(5)?;
        let (ws15, w15) = wn(15)?;
        let (ws30, w30) = wn(30)?;
        let (ba, be, nb) = base(t0, t);
        let (a, b) = (ba * 15, be * 15);
        let bsec = (b - a) as f64;
        let (bdl, btr) = if b > a {
            let (x, y) = (self.bar(ba)?, self.bar(be)?);
            (y.d - x.d, y.t - x.t)
        } else {
            (0.0, 0.0)
        };
        let bd = if b > a { bdl / bsec } else { 0.0 };
        let bt = if b > a { btr / bsec * 60.0 } else { 0.0 };
        let rd = |w: &W, n: f64| ((w.dollars / n + 1.0) / (bd + 1.0)).ln();
        let ret = |ws: i64| -> f64 {
            if ws == 0 {
                return f64::NAN;
            }
            match self.sec(ws - 1) {
                Some(r) if !r.last.is_nan() => (last / r.last).ln(),
                _ => f64::NAN,
            }
        };
        let buy = |w: &W| {
            if w.upv + w.dnv > 0.0 {
                w.upv / (w.upv + w.dnv)
            } else {
                0.5
            }
        };
        f[0] = w5.dollars.ln_1p();
        f[1] = w15.dollars.ln_1p();
        f[2] = w30.dollars.ln_1p();
        f[3] = rd(&w5, 5.0);
        f[4] = rd(&w15, 15.0);
        f[5] = rd(&w30, 30.0);
        f[6] = ((w15.trades * 4.0 + 1.0) / (bt + 1.0)).ln();
        if let (Some(adv), Some(pc)) = (c.adv30, c.prev_close) {
            f[7] = ((w30.dollars * 2.0 + 1.0) / (adv * pc / 390.0 + 1.0)).ln();
        }
        f[8] = ret(ws5);
        f[9] = ret(ws15);
        f[10] = ret(ws30);
        let (mut bh, mut bl, mut traded) = (f64::NAN, f64::NAN, 0i64);
        for k in ba..be {
            let r = self.bar(k)?;
            if !r.hi.is_nan() {
                bh = if bh.is_nan() { r.hi } else { bh.max(r.hi) };
                bl = if bl.is_nan() { r.lo } else { bl.min(r.lo) };
                traded += 1;
            }
        }
        f[11] = if bh.is_nan() {
            f64::NAN
        } else {
            (last / bh).ln()
        };
        f[12] = buy(&w5);
        f[13] = buy(&w15);
        f[14] = f64::from(self.sec(hi - 1)?.upr.min(50));
        f[15] = w15.nh.ln_1p();
        f[16] = w15.sw;
        let mut vx = 0u8;
        for k in ws15..hi {
            vx = vx.max(self.sec(k)?.vex);
        }
        f[17] = f64::from(vx);
        f[18] = if w15.dollars > 0.0 {
            w15.blkup / w15.dollars
        } else {
            0.0
        };
        f[19] = if w15.trades > 0.0 {
            w15.odd / w15.trades
        } else {
            0.0
        };
        f[20] = if w15.trades > 0.0 {
            let bavg = if btr > 0.0 { bdl / btr } else { 0.0 };
            ((w15.dollars / w15.trades + 1.0) / (bavg + 1.0)).ln()
        } else {
            f64::NAN
        };
        f[21] = last.ln();
        f[22] = sess_code(t).unwrap_or(f64::NAN);
        f[23] = (t - t0) as f64 / (NSEC as f64 * 1e9);
        f[24] = c.prev_close.map_or(f64::NAN, |pc| (last / pc).ln());
        f[25] = match (self.first.is_nan(), c.prev_close) {
            (false, Some(pc)) => (self.first / pc).ln(),
            _ => f64::NAN,
        };
        let day = self.pd(hi)?;
        let (dayv, dayd) = (day.v, day.d);
        f[26] = c
            .adv30
            .map_or(f64::NAN, |adv| ((dayv + 1.0) / (adv + 1.0)).ln());
        f[27] = if ws15 > 0 {
            let h = self.sec(ws15 - 1)?.dayhi;
            if h.is_nan() {
                f64::NAN
            } else {
                (last / h).ln()
            }
        } else {
            f64::NAN
        };
        f[28] = if dayv > 0.0 {
            (last / (dayd / dayv)).ln()
        } else {
            f64::NAN
        };
        f[29] = if nb > 0 {
            1.0 - traded as f64 / nb as f64
        } else {
            f64::NAN
        };
        f[30] = if !bl.is_nan() && bl > 0.0 {
            (bh - bl) / bl
        } else {
            f64::NAN
        };
        f[31] = dayd.ln_1p();
        f[32] = ep.ln_1p();
        f[33] = age;
        let _ = ws30;
        Some((f, w15))
    }

    /// Records a gate pass at `t` (the research's `Episodes::pass`): (earlier episodes, age s).
    fn episode(&mut self, t: i64, reference: f64) -> (f64, f64) {
        if self.ep_last.is_none_or(|l| t - l > EPISODE_GAP_NS) {
            if self.ep_last.is_some() {
                self.ep_count += 1.0;
            }
            self.ep_start = t;
            self.ep_ref = reference;
        }
        self.ep_last = Some(t);
        (self.ep_count, (t - self.ep_start) as f64 / 1e9)
    }
}

/// A new burst alert.
#[derive(Clone, Debug)]
pub struct BurstAlert {
    pub pass: Pass,
    /// Alert time `w + 1 s` and the alert price.
    pub at_ns: i64,
    pub price: f64,
    pub threshold: f64,
    /// The outcome tracker's key ([`BurstDetector::outcome`]).
    pub seq: u32,
}

impl BurstAlert {
    /// % from the price before the burst started to the alert price.
    pub fn gain_pct(&self) -> Option<f64> {
        let r = self.pass.ep_ref;
        (r > 0.0 && r.is_finite()).then(|| (self.price / r - 1.0) * 100.0)
    }
    /// % over the 30 s before the boundary (`ret30`).
    pub fn move_pct(&self) -> Option<f64> {
        let x = self.pass.x[10];
        x.is_finite().then(|| (x.exp() - 1.0) * 100.0)
    }
    /// The reasons shown on the row and the push, strongest signal first.
    pub fn reasons(&self) -> Vec<String> {
        reasons(&self.pass)
    }
}

pub fn reasons(p: &Pass) -> Vec<String> {
    let mut v = Vec::new();
    let r = p.rate_x;
    v.push(if r >= 10.0 {
        format!("$ rate {r:.0}× base")
    } else {
        format!("$ rate {r:.1}× base")
    });
    let n = p.new_highs.round() as i64;
    if n >= 1 {
        v.push(format!(
            "{n} new high{} in 15 s",
            if n == 1 { "" } else { "s" }
        ));
    }
    let s = p.sweeps.round() as i64;
    if s >= 1 {
        v.push(if s == 1 {
            "1 sweep".into()
        } else {
            format!("{s} sweeps")
        });
    }
    if p.buy15 >= 0.6 {
        v.push(format!("{:.0}% up-tick volume", p.buy15 * 100.0));
    }
    if p.blk15 >= 0.25 {
        v.push(format!("{:.0}% in $10k+ up-tick prints", p.blk15 * 100.0));
    }
    v
}

/// What one 5 s check cost, for the logs and the cost test.
#[derive(Clone, Copy, Debug, Default)]
pub struct CheckCost {
    pub elapsed: Duration,
    pub looked: usize,
    pub passes: usize,
}

/// A settled outcome.
#[derive(Clone, Debug)]
pub struct Settled {
    pub symbol: Arc<str>,
    pub outcome: Outcome,
    pub settled_ns: i64,
}

pub struct BurstDetector {
    policy: BurstPolicy,
    model: &'static BurstModel,
    index: HashMap<Arc<str>, usize>,
    syms: Vec<BSym>,
    days: DayCache,
    date: Option<NaiveDate>,
    t0: i64,
    ord: u32,
    /// Symbols with a print since their last check (each once).
    recent: Vec<usize>,
    /// The last boundary checked (seconds from 04:00; -1 = none).
    checked: i64,
    adv: HashMap<String, f64>,
    adv_date: Option<NaiveDate>,
    adv_wanted: Option<NaiveDate>,
    alerts_today: u32,
    seq: u32,
    last_cost: Option<CheckCost>,
    last_free_s: i64,
    /// Tests and the parity replay: every gate pass, scored.
    capture: Option<Vec<Pass>>,
    settled: Vec<Settled>,
    /// A restart's rebuild ([`BurstDetector::rebuilder`]): every boundary is checked however
    /// far behind, and a gate pass only records its episode (no features, model or alert).
    rebuild: bool,
}

impl BurstDetector {
    pub fn new(policy: BurstPolicy) -> BurstDetector {
        BurstDetector {
            policy,
            model: BurstModel::frozen(),
            index: HashMap::new(),
            syms: Vec::new(),
            days: DayCache::default(),
            date: None,
            t0: 0,
            ord: 0,
            recent: Vec::new(),
            checked: -1,
            adv: HashMap::new(),
            adv_date: None,
            adv_wanted: None,
            alerts_today: 0,
            seq: 0,
            last_cost: None,
            last_free_s: 0,
            capture: None,
            settled: Vec::new(),
            rebuild: false,
        }
    }

    /// The detector a restart's history load feeds one symbol's recorded day into (module docs,
    /// "After a restart"), adopted afterwards with [`BurstDetector::adopt`]. It is ticked on
    /// the history's clock, which jumps from one delivery to the next: a gap never skips
    /// boundaries, as the live tick's stall guard would. A gap holds at most the boundaries the
    /// symbol had prints for, and those go before the next print is applied, so the rings
    /// still hold every second they need.
    pub fn rebuilder() -> BurstDetector {
        let mut d = BurstDetector::new(BurstPolicy::default());
        d.rebuild = true;
        d
    }

    pub fn policy(&self) -> &BurstPolicy {
        &self.policy
    }

    pub fn alerts_today(&self) -> u32 {
        self.alerts_today
    }

    pub fn last_cost(&self) -> Option<CheckCost> {
        self.last_cost
    }

    /// Keeps every scored gate pass (tests, the parity replay).
    pub fn capture(&mut self) {
        self.capture = Some(Vec::new());
    }

    pub fn take_captured(&mut self) -> Vec<Pass> {
        self.capture
            .as_mut()
            .map(std::mem::take)
            .unwrap_or_default()
    }

    /// The trading date whose ADV30 the detector wants loaded, once (the market loads it in the
    /// background: [`facts::adv30`]).
    pub fn take_adv_request(&mut self) -> Option<NaiveDate> {
        self.adv_wanted.take()
    }

    /// Installs a trading date's ADV30 per symbol.
    pub fn set_adv(&mut self, date: NaiveDate, adv: HashMap<String, f64>) {
        if self.date.is_none_or(|d| d == date) {
            self.adv = adv;
            self.adv_date = Some(date);
        }
    }

    pub fn adv_loaded(&self) -> Option<NaiveDate> {
        self.adv_date
    }

    fn sym_index(&mut self, symbol: &str) -> usize {
        if let Some(&i) = self.index.get(symbol) {
            return i;
        }
        let s: Arc<str> = Arc::from(symbol);
        let i = self.syms.len();
        self.syms.push(BSym::new(s.clone()));
        self.index.insert(s, i);
        i
    }

    /// One print with the chart engine's verdict on it. O(1).
    pub fn apply_trade(&mut self, t: &Trade, counted: Counted) {
        self.apply(t, counted);
    }

    /// A print of a symbol whose day the history load has not rebuilt yet (module docs, "After
    /// a restart"): counted like any print, and the symbol is not gated until it is adopted.
    pub fn apply_trade_unrestored(&mut self, t: &Trade, counted: Counted) {
        if let Some(i) = self.apply(t, counted) {
            self.syms[i].unrestored = true;
        }
    }

    fn apply(&mut self, t: &Trade, counted: Counted) -> Option<usize> {
        if !counted.ohlc && !counted.volume {
            return None;
        }
        let session = t.session?;
        if session.kind == SessionKind::Overnight {
            return None;
        }
        let (t0, ord) = self.days.get(session.date);
        let exec = t.event_ts.0;
        if exec < t0 || exec >= t0 + NSEC * SEC {
            return None;
        }
        let price = t.price.micros();
        let i = self.sym_index(&t.symbol);
        let s = &mut self.syms[i];
        if s.day != ord {
            s.reset_day(ord);
        }
        let k = (exec - t0).div_euclid(SEC);
        s.apply(k, price, t.size, counted, venue(&t.exchange));
        if counted.ohlc && !s.open.is_empty() {
            let p = price as f64 / 1e6;
            for o in &mut s.open {
                o.print(exec, p);
            }
        }
        if !s.in_recent {
            s.in_recent = true;
            self.recent.push(i);
        }
        Some(i)
    }

    /// Swaps in `symbol`'s day as `from` (a [`BurstDetector::rebuilder`] fed the symbol's
    /// recorded day and held tail) rebuilt it, after ticking `from` up to the market clock `now`.
    /// Keeps what only this detector knows: the symbol's last alert (the 10-minute dedup, also
    /// restored from the forward log after a restart) and its open outcomes. Clears the
    /// unrestored mark. O(rings) once per symbol load.
    pub fn adopt(&mut self, symbol: &str, mut from: BurstDetector, now: i64) {
        from.tick(now, &|_| None);
        let date = market_events::session::trading_date(UnixNanos(now));
        if self.date != Some(date) {
            self.roll(date);
        }
        let ord = self.ord;
        let i = self.sym_index(symbol);
        let mut day = match from.index.get(symbol) {
            Some(&j) if from.syms[j].day == ord => {
                std::mem::replace(&mut from.syms[j], BSym::new(Arc::from(symbol)))
            }
            // Nothing recorded today: the day is empty, whatever arrived before the load did
            // went into the rebuild too.
            _ => {
                let mut b = BSym::new(Arc::from(symbol));
                b.day = ord;
                b
            }
        };
        let s = &mut self.syms[i];
        day.symbol = s.symbol.clone();
        day.last_alert = s.last_alert;
        day.open = std::mem::take(&mut s.open);
        day.in_recent = s.in_recent;
        day.unrestored = false;
        *s = day;
        if !s.in_recent && s.last_print_s >= 0 {
            s.in_recent = true;
            self.recent.push(i);
        }
    }

    /// Symbols marked unrestored (tests).
    pub fn unrestored(&self) -> usize {
        self.syms.iter().filter(|s| s.unrestored).count()
    }

    fn roll(&mut self, date: NaiveDate) {
        let (t0, ord) = self.days.get(date);
        self.date = Some(date);
        self.t0 = t0;
        self.ord = ord;
        self.checked = -1;
        self.alerts_today = 0;
        if self.adv_date != Some(date) {
            self.adv.clear();
            self.adv_date = None;
            self.adv_wanted = Some(date);
        }
    }

    /// The checks due at the market clock `now` (epoch ns): every 5 s boundary `w` with
    /// `w + 1 s ≤ now` not checked yet (only the newest after a stall). Returns the new alerts.
    pub fn tick(&mut self, now: i64, prev_close: &dyn Fn(&str) -> Option<f64>) -> Vec<BurstAlert> {
        let date = market_events::session::trading_date(UnixNanos(now));
        if self.date != Some(date) {
            self.roll(date);
        }
        self.settle(now);
        let due = (now - SETTLE_NS - self.t0).div_euclid(SEC);
        if due < STEP_S {
            return Vec::new();
        }
        let due = (due.div_euclid(STEP_S) * STEP_S).min(NSEC);
        if due <= self.checked {
            return Vec::new();
        }
        let stalled = !self.rebuild && due - self.checked > MAX_LAG_S;
        let first = if stalled || self.checked < 0 {
            due
        } else {
            self.checked + STEP_S
        };
        let mut out = Vec::new();
        let mut hi = first;
        while hi <= due {
            out.extend(self.check(hi, prev_close));
            hi += STEP_S;
        }
        self.checked = due;
        if due - self.last_free_s >= 60 {
            self.last_free_s = due;
            self.free_idle(due);
        }
        out
    }

    /// One boundary (seconds from 04:00): the gate for the symbols that printed in the 15 s
    /// before it, features and model on a pass, alerts.
    pub fn check(&mut self, hi: i64, prev_close: &dyn Fn(&str) -> Option<f64>) -> Vec<BurstAlert> {
        let start = Instant::now();
        let (t0, ord) = (self.t0, self.ord);
        let t = t0 + hi * SEC;
        let mut looked = 0usize;
        let mut passes: Vec<Pass> = Vec::new();
        let recent = std::mem::take(&mut self.recent);
        let mut keep = Vec::with_capacity(recent.len());
        for i in recent {
            let s = &mut self.syms[i];
            if s.day != ord || s.last_print_s < hi - 15 {
                s.in_recent = false;
                continue;
            }
            keep.push(i);
            if s.unrestored {
                continue;
            }
            looked += 1;
            s.advance_to(hi);
            let Some((_, reference, rate)) = s.gate(t0, hi) else {
                continue;
            };
            let (ep, age) = s.episode(t, reference);
            if self.rebuild {
                continue;
            }
            let c = Ctx {
                prev_close: prev_close(&s.symbol),
                adv30: self.adv.get(&*s.symbol).copied(),
            };
            let Some((x, w15)) = s.features(t0, hi, &c, ep, age) else {
                continue;
            };
            let input = x.map(model_input);
            let score = self.model.predict(&input);
            passes.push(Pass {
                symbol: s.symbol.clone(),
                t_ns: t,
                hi,
                last: x[21].exp(),
                x,
                input,
                score,
                ep_start_ns: s.ep_start,
                ep_ref: s.ep_ref,
                rate_x: rate,
                new_highs: w15.nh,
                sweeps: w15.sw,
                buy15: x[13],
                blk15: x[18],
            });
        }
        self.recent = keep;
        let n = passes.len();
        let mut alerts = Vec::new();
        let mut order: Vec<usize> = (0..n).collect();
        order.sort_by(|&a, &b| passes[b].score.total_cmp(&passes[a].score));
        let at = t + SETTLE_NS;
        for k in order {
            let p = &passes[k];
            if p.score < self.policy.threshold || self.alerts_today >= self.policy.cap() {
                continue;
            }
            let i = self.index[&p.symbol];
            let s = &mut self.syms[i];
            if s.last_alert.is_some_and(|l| at - l < DEDUP_NS) {
                continue;
            }
            s.last_alert = Some(at);
            self.alerts_today += 1;
            self.seq += 1;
            let price = if s.last.is_finite() { s.last } else { p.last };
            s.open.push(Outcome::new(self.seq, at, price));
            alerts.push(BurstAlert {
                pass: p.clone(),
                at_ns: at,
                price,
                threshold: self.policy.threshold,
                seq: self.seq,
            });
        }
        if let Some(c) = self.capture.as_mut() {
            c.extend(passes);
        }
        self.last_cost = Some(CheckCost {
            elapsed: start.elapsed(),
            looked,
            passes: n,
        });
        alerts.sort_by_key(|a| a.at_ns);
        alerts
    }

    /// Frees the rings of symbols idle for [`IDLE_FREE_S`].
    fn free_idle(&mut self, now_s: i64) {
        for s in &mut self.syms {
            if s.rings.is_some()
                && !s.in_recent
                && (s.day != self.ord || s.last_print_s < now_s - IDLE_FREE_S)
            {
                s.rings = None;
            }
        }
    }

    /// Settles the outcomes whose 30 minutes are up (or whose day ended).
    fn settle(&mut self, now: i64) {
        let end = self.t0 + NSEC * SEC;
        for s in &mut self.syms {
            if s.open.is_empty() {
                continue;
            }
            let mut i = 0;
            while i < s.open.len() {
                let o = &s.open[i];
                let due = o.at_ns + HORIZON_NS;
                let day_over = now >= end && o.at_ns < end;
                if now >= due || day_over || o.at_ns < self.t0 {
                    let mut o = s.open.swap_remove(i);
                    o.settled = true;
                    o.partial = o.partial || now < due;
                    self.settled.push(Settled {
                        symbol: s.symbol.clone(),
                        outcome: o,
                        settled_ns: now,
                    });
                } else {
                    i += 1;
                }
            }
        }
    }

    /// The outcomes settled since the last take.
    pub fn take_settled(&mut self) -> Vec<Settled> {
        std::mem::take(&mut self.settled)
    }

    /// An open outcome, by symbol and key.
    pub fn outcome(&self, symbol: &str, seq: u32) -> Option<&Outcome> {
        let i = *self.index.get(symbol)?;
        self.syms[i].open.iter().find(|o| o.seq == seq)
    }

    /// After a restart: the symbol's last alert (the 10-minute dedup) and, while its 30 minutes
    /// are not up, a partial outcome followed from now. Returns the outcome key.
    pub fn restore_alert(&mut self, symbol: &str, at_ns: i64, price: f64, now: i64) -> Option<u32> {
        let i = self.sym_index(symbol);
        let date = market_events::session::trading_date(UnixNanos(now));
        if self.date != Some(date) {
            self.roll(date);
        }
        let ord = self.ord;
        let s = &mut self.syms[i];
        if s.day != ord {
            s.reset_day(ord);
        }
        s.last_alert = Some(s.last_alert.map_or(at_ns, |l| l.max(at_ns)));
        self.alerts_today += 1;
        if now >= at_ns + HORIZON_NS || price.is_nan() || price <= 0.0 {
            return None;
        }
        self.seq += 1;
        let mut o = Outcome::new(self.seq, at_ns, price);
        o.partial = true;
        self.syms[i].open.push(o);
        Some(self.seq)
    }

    /// Symbols with state (tests).
    pub fn symbols(&self) -> usize {
        self.syms.len()
    }

    /// Symbols holding rings now (tests, memory).
    pub fn with_rings(&self) -> usize {
        self.syms.iter().filter(|s| s.rings.is_some()).count()
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
pub(crate) mod replay_parity;

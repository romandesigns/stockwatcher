//! Volume shelves (owner, 2026-10-22): the one liquidity zone that beat a random line.
//!
//! # What won, and what it is not
//!
//! The zones bake-off (`C:\research\stockwatcher\ZONES-CONTRACT.txt`, judge
//! `reports/zones/judge/results.json`) tested six zone types on unseen TEST days (Aug–Oct)
//! against a random-line placebo. Only the volume-profile **variant C** shelf beat it: price
//! coming down into one HELD (+10 % before −5 %) 31.9 % of the time against the placebo's 23.3 %
//! (+8.6 pp, Holm p = 0.003), in every price bucket and every month, and it replicated DEV. It
//! still LOSES about half the time (≈ 52 %). The panels say exactly that base rate ("about 1 in 3
//! hold, 1 in 2 fail") and never predict.
//!
//! # The frozen definition (`reports/zones/candidates/volume-profile/frozen.json`)
//!
//! Ported from `scripts/research/zones_volume_profile.py` (branch `research/zones-volume-profile`,
//! commit `178c4ab`) and the run labels of `crates/setup-research/src/rocket.rs`
//! (`rockets(d, 20)`, branch `research/rocket-precursors`):
//!
//! - **Grid**: 15 s slots from 04:00 to 20:00 ET of the print's trading date; a print executed at
//!   `e` is in the slot ending `floor((e − 04:00) / 15 s) + 1` slots later. OHLC from the prints
//!   the chart engine counted for OHLC, volume / trades / dollars from the ones it counted for
//!   volume ([`Counted`], the "updates OHLC" / "updates volume" verdicts, as the candles).
//! - **Run (the causal trigger)**, the live equivalent of the `events-g20` label: a traded slot
//!   `j` whose close is ≥ 1.2 × `L`, `L` = the min close of the traded slots `i < j` ending within
//!   the 15 minutes before `j`'s end; start = the LATEST such slot at `L`; `L` in `$0.15–$25`; at
//!   least 25 volume-counted trades and $25,000 from the start slot through `j`; no mark slot
//!   ending within 30 minutes of the previous mark counts. Confirmed at `j`'s END (`plus20_ns`),
//!   which is when the slot is complete: nothing later is read.
//! - **Profile**: volume-counted prints from the first run's start-slot OPEN (start − 15 s) on,
//!   binned at `tick × max(1, ceil(0.01 · P0 / tick))` ([`profile::bin_width`]), `P0` = the first
//!   run's start price.
//! - **Shelves** at each traded slot end `B` from the first `plus20_ns`, with the prints executed
//!   before `B` ([`profile::Profile::zones`]): smoothed [1,2,1]/4, a node is a local max ≥ 1.5 ×
//!   the mean of the occupied bins, extended to its 0.5 × contour, spans ≤ 1 empty bin apart
//!   merged, kept when they hold ≥ 5 % of the volume. A shelf keeps its identity (formation high,
//!   used flag) while some new shelf overlaps it.
//! - **Launch-base gate**: `zone_hi ≤ rs + 0.5 · (peak − rs)`, `rs` = start price of the latest
//!   confirmed run, `peak` = the session high (OHLC prints from 04:00) through `B`.
//! - **Armed** once the session high exceeds the high at the shelf's formation.
//! - **Event**: previous traded close > `zone_hi` and `zone_lo ≤ close ≤ zone_hi`. A crossing
//!   consumes the shelf (one entry per shelf; a bar falling straight through consumes it without
//!   an event); at most 3 events a symbol-day, ≥ 10 minutes apart.
//! - **Outcome** (the contract's, resolved after the fact from the OHLC prints executed in
//!   `[B, B + 30 min]`, in execution order): `held` = a print ≥ 1.10 × entry before any print ≤
//!   0.95 × entry and before any 1-minute close ≤ 0.98 × `zone_lo` (stamped at that minute's end);
//!   `lost` = one of those first; `expired` = neither.
//!
//! # Scope and cost
//!
//! Every symbol printing in `$0.10–$40` gets a small state (~200 B). While it trades, it also
//! keeps what a run confirmed at the next slot would need: the prints of the slots not processed
//! yet, a monotone deque of closes and the trades/dollars of the last 61 slots, and its volume
//! **aggregated per (slot, price)** for the last 61 slots (a run's start slot is at most 60 slots
//! back, and the profile needs its prints). That buffer is freed after 16 idle minutes and once
//! the symbol is confirmed. Only confirmed symbols (a few dozen a day) keep a profile: ~1 % of the
//! start price per bin, typically a few hundred bins (~2–4 KB). `zones::tests::cost` measures it.
//!
//! Per print: one hash lookup and a push. Per slot (checked once per wall second at the market
//! clock, from the quotes task, never on the ingest path, `SETTLE_NS` after the slot ends): only
//! the symbols that printed are touched; the shelves are recomputed only for confirmed symbols
//! on their traded slots (O(bins)).
//!
//! # Differences from the research (live only; a recorded day replayed in order has none)
//!
//! - A slot is decided [`SETTLE_NS`] after it ends with the prints that ARRIVED by then (the
//!   research: every print executed before its end). Within the slot they are sorted by
//!   execution time, so its close is the research's unless a print arrives later than that.
//! - A print arriving after its slot was decided (late) still counts in the session high, the
//!   profile and an open outcome, but never changes a decided close, run or event.
//! - The research run labels were computed over the whole day; here a run exists only from the
//!   end of its mark slot. Both use only what was known then (`plus20_ns` = the mark slot's end),
//!   the bake-off's knowability rule; nothing is backdated.
//!
//! # After a restart
//!
//! A restarted server rebuilds each symbol's day from the archive in the history `Builder`
//! (`crate::history`), like the burst detector: a [`ZoneBoard::rebuilder`] gets every recorded and
//! held print and is ticked at each clock advance, then is adopted under the core lock
//! ([`ZoneBoard::adopt`]). Runs, profiles, shelves, events and outcomes come back exactly as the
//! uninterrupted run had them (the restart gap itself has no prints until the nightly import).
//! The forward log is restart-safe: today's file is read at start, and a line is written only if
//! its key (`run`/`entry`/`outcome` + symbol + time) is not in it yet, so an adopted day logs only
//! what the previous process had not logged. Until a symbol is adopted its prints are held by the
//! market and do not reach the board (no shelf is shown for it).
//!
//! # Forward log
//!
//! `ARCHIVE_ROOT/zones/<trading date>.jsonl` (live only), one JSON object per line, `"v": 1`:
//! `run` (`symbol, start_ns, start_price, plus20_ns`), `entry` (the research's event fields:
//! `symbol, t_ns, entry_price, zone_lo, zone_hi, peak_price, run_start_ns, score`, score = the
//! run's retracement at entry) and `outcome` (`symbol, t_ns, outcome, resolved_ns`), so live
//! performance can be scored exactly like the bake-off's files.

pub mod profile;
#[cfg(test)]
mod tests;

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;

use chrono::NaiveDate;
use market_events::Trade;
use market_events::session::SessionKind;
use serde_json::{Value, json};

use crate::quotes::Counted;
use crate::rocket::features::DayCache;
use profile::Profile;

pub const BAR_NS: i64 = 15_000_000_000;
const SEC_NS: i64 = 1_000_000_000;
const MIN_NS: i64 = 60 * SEC_NS;
/// 15 s slots from 04:00 to 20:00 ET.
pub const NSLOT: i64 = 3840;
/// The run gain (`events-g20`).
pub const GAIN_PCT: f64 = 20.0;
/// A leg is at most 15 minutes: the start slot ends within 60 slots of the mark's end.
const LEG_SLOTS: i64 = 60;
/// After a mark at T no mark slot ending before T + 30 min counts.
const REFRACTORY_NS: i64 = 30 * MIN_NS;
/// The research band on the run's start price (micro-dollars), `in_band` of the labels.
pub const BAND_MICROS: (i64, i64) = (150_000, 25_000_000);
/// Only symbols printing in this wider band get a state (micro-dollars).
const WIDE_MICROS: (i64, i64) = (100_000, 40_000_000);
const MIN_TRADES: f64 = 25.0;
const MIN_DOLLARS: f64 = 25_000.0;
/// A slot is decided this long after it ends (the Runners board's 15 s barriers' rule).
pub const SETTLE_NS: i64 = SEC_NS;
/// Variant C's launch-base gate: the shelf top in the lower half of the run.
const RETR_MIN: f64 = 0.5;
const MAX_EVENTS: usize = 3;
const EVENT_GAP_NS: i64 = 10 * MIN_NS;
pub const HORIZON_NS: i64 = 30 * MIN_NS;
/// A resolved event stays on screen this long.
pub const SHOW_RESOLVED_NS: i64 = 10 * MIN_NS;
/// An unconfirmed symbol's buffers are freed after this long without a print.
const IDLE_FREE_NS: i64 = 16 * MIN_NS;
pub const STRATEGY: &str = "volume-shelf-C";
pub const VERSION: &str = "178c4ab";

/// One print, as the slot decision needs it.
#[derive(Clone, Copy, Debug)]
struct Print {
    exec: i64,
    price: i64,
    size: f64,
    ohlc: bool,
    vol: bool,
}

/// A confirmed +20 % run.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Run {
    /// The start slot's END (the labels' `start_ns`).
    pub start_ns: i64,
    pub start_price: f64,
    /// The mark slot's END: when the run became known.
    pub plus20_ns: i64,
}

#[derive(Clone, Copy, Debug)]
struct Tracked {
    lo: f64,
    hi: f64,
    formed_hi: f64,
    formed_ns: i64,
    used: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    InShelf,
    Held,
    Lost,
    Expired,
}

impl State {
    pub fn as_str(self) -> &'static str {
        match self {
            State::InShelf => "in_shelf",
            State::Held => "held",
            State::Lost => "lost",
            State::Expired => "expired",
        }
    }
}

/// Price came down into a shelf: the research's event and its live outcome.
#[derive(Clone, Copy, Debug)]
pub struct Event {
    /// The slot end it fired at (`t_ns`).
    pub t_ns: i64,
    pub entry: f64,
    pub lo: f64,
    pub hi: f64,
    pub peak: f64,
    pub run_start_ns: i64,
    /// The run's retracement at entry, `(peak − entry) / (peak − rs)`: variant C's score.
    pub retr: f64,
    pub state: State,
    pub resolved_ns: Option<i64>,
    /// The open 1-minute bar (its end, 0 = none) and its last price, for the zone-loss rule.
    min_end: i64,
    min_close: f64,
}

impl Event {
    fn end(&self) -> i64 {
        self.t_ns + HORIZON_NS
    }

    fn resolve(&mut self, s: State, at: i64) {
        self.state = s;
        self.resolved_ns = Some(at);
    }

    /// The 1-minute bar ending `min_end` is complete: the zone-loss rule.
    fn close_minute(&mut self) {
        if self.min_end != 0
            && self.min_end > self.t_ns
            && self.min_end <= self.end()
            && self.min_close <= self.lo * 0.98
        {
            let at = self.min_end;
            self.resolve(State::Lost, at);
        }
        self.min_end = 0;
    }

    /// One OHLC print executed at `exec` (in execution order while it is open).
    fn print(&mut self, exec: i64, p: f64) {
        if self.state != State::InShelf || exec < self.t_ns || exec > self.end() {
            return;
        }
        let me = (exec.div_euclid(MIN_NS) + 1) * MIN_NS;
        if self.min_end != 0 && me > self.min_end {
            self.close_minute();
            if self.state != State::InShelf {
                return;
            }
        }
        self.min_end = me;
        self.min_close = p;
        if p >= self.entry * 1.10 {
            self.resolve(State::Held, exec);
        } else if p <= self.entry * 0.95 {
            self.resolve(State::Lost, exec);
        }
    }
}

/// A shelf as published: qualifying (launch base) at the last decided slot.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Band {
    pub lo: f64,
    pub hi: f64,
    pub formed_ns: i64,
    pub armed: bool,
}

/// A confirmed symbol's day: the profile since its first run and everything decided on it.
#[derive(Clone, Debug)]
pub struct Shelf {
    pub profile: Profile,
    /// The first run's start slot (the profile's anchor is its open).
    anchor_k: i64,
    pub runs: Vec<Run>,
    tracked: Vec<Tracked>,
    pub events: Vec<Event>,
    last_ev: Option<i64>,
    pub bands: Vec<Band>,
    /// Prints dropped past [`profile::MAX_BINS`].
    pub dropped: u64,
}

impl Shelf {
    fn add(&mut self, price: f64, size: f64) {
        if !self.profile.add(price, size) {
            self.dropped += 1;
        }
    }

    /// The research's per-bar step at the end `b` of a traded slot.
    fn decide(&mut self, b: i64, close: f64, prev_close: Option<f64>, peak: f64) -> bool {
        let zones = self.profile.zones();
        let mut next: Vec<Tracked> = Vec::with_capacity(zones.len());
        for (zl, zh, _share) in zones {
            let m = self
                .tracked
                .iter()
                .find(|t| t.lo < zh && zl < t.hi)
                .copied();
            next.push(match m {
                Some(t) => Tracked {
                    lo: zl,
                    hi: zh,
                    ..t
                },
                None => Tracked {
                    lo: zl,
                    hi: zh,
                    formed_hi: peak,
                    formed_ns: b,
                    used: false,
                },
            });
        }
        self.tracked = next;
        let cur = *self.runs.last().expect("a confirmed symbol has a run");
        let rs = cur.start_price;
        let retr = if peak > rs {
            (peak - close) / (peak - rs)
        } else {
            0.0
        };
        let gate = rs + (1.0 - RETR_MIN) * (peak - rs);
        let mut fired = false;
        if let Some(pc) = prev_close {
            for t in &mut self.tracked {
                // Armed: the session high went past the high at formation (NaN never arms).
                let armed = peak > t.formed_hi;
                if t.used || !armed {
                    continue;
                }
                if t.hi > gate {
                    continue; // not a launch-base shelf (upper half of the run)
                }
                if pc > t.hi && close <= t.hi {
                    t.used = true;
                    if close < t.lo {
                        continue; // fell straight through within one bar: not an entry
                    }
                    if self.events.len() >= MAX_EVENTS
                        || self.last_ev.is_some_and(|l| b - l < EVENT_GAP_NS)
                    {
                        continue;
                    }
                    // The 1-minute bar the slot ends in, when it ends inside one: its close so
                    // far is this slot's (the research's minute bars, `m0 = searchsorted(mt, t0,
                    // "right")`).
                    let (min_end, min_close) = if b.rem_euclid(MIN_NS) != 0 {
                        ((b.div_euclid(MIN_NS) + 1) * MIN_NS, close)
                    } else {
                        (0, f64::NAN)
                    };
                    self.events.push(Event {
                        t_ns: b,
                        entry: close,
                        lo: t.lo,
                        hi: t.hi,
                        peak,
                        run_start_ns: cur.start_ns,
                        retr,
                        state: State::InShelf,
                        resolved_ns: None,
                        min_end,
                        min_close,
                    });
                    self.last_ev = Some(b);
                    fired = true;
                }
            }
        }
        let bands: Vec<Band> = self
            .tracked
            .iter()
            .filter(|t| t.hi <= gate)
            .map(|t| Band {
                lo: t.lo,
                hi: t.hi,
                formed_ns: t.formed_ns,
                armed: peak > t.formed_hi,
            })
            .collect();
        let changed = fired || bands != self.bands;
        self.bands = bands;
        changed
    }

    fn print(&mut self, exec: i64, p: f64) -> bool {
        let mut changed = false;
        for e in &mut self.events {
            if e.state == State::InShelf {
                e.print(exec, p);
                changed |= e.state != State::InShelf;
            }
        }
        changed
    }

    /// Every print up to `through` (exclusive) is in: completed minutes and windows resolve.
    fn settle(&mut self, through: i64, day_over: bool) -> bool {
        let mut changed = false;
        for e in &mut self.events {
            if e.state != State::InShelf {
                continue;
            }
            if e.min_end != 0 && e.min_end <= through {
                e.close_minute();
            }
            if e.state == State::InShelf && (through > e.end() || day_over) {
                let at = e.end();
                e.resolve(State::Expired, at);
            }
            changed |= e.state != State::InShelf;
        }
        changed
    }
}

/// One symbol's state for its trading date.
#[derive(Clone, Debug)]
struct Sym {
    symbol: Arc<str>,
    /// Prints of slots not decided yet (arrival order).
    tail: Vec<Print>,
    dirty: bool,
    /// The last slot decided (−1 = none).
    done: i64,
    /// Monotone deque of (slot, close) over the traded slots of the last 15 minutes: the front is
    /// the min close, the latest at it.
    mins: VecDeque<(i64, f64)>,
    /// (slot, trades, dollars) of the decided slots of the last 61.
    liq: VecDeque<(i64, f64, f64)>,
    refr: Option<i64>,
    day_high: f64,
    prev_close: Option<f64>,
    /// Unconfirmed: volume per (slot, price micros, shares) over the last 61 slots; 12 bytes an
    /// entry (prices here are under $40, i.e. < 2^31 micros; a (slot, price) holding more than
    /// 4.29 billion shares saturates).
    buf: VecDeque<(i32, i32, u32)>,
    shelf: Option<Box<Shelf>>,
    last_ns: i64,
}

impl Sym {
    fn new(symbol: Arc<str>) -> Self {
        Sym {
            symbol,
            tail: Vec::new(),
            dirty: false,
            done: -1,
            mins: VecDeque::new(),
            liq: VecDeque::new(),
            refr: None,
            day_high: f64::NAN,
            prev_close: None,
            buf: VecDeque::new(),
            shelf: None,
            last_ns: 0,
        }
    }

    fn bytes(&self) -> usize {
        std::mem::size_of::<Sym>()
            + self.symbol.len()
            + self.tail.capacity() * std::mem::size_of::<Print>()
            + self.mins.capacity() * 16
            + self.liq.capacity() * 24
            + self.buf.capacity() * std::mem::size_of::<(i32, i32, u32)>()
            + self.shelf.as_ref().map_or(0, |s| {
                std::mem::size_of::<Shelf>()
                    + s.profile.bytes()
                    + s.runs.capacity() * std::mem::size_of::<Run>()
                    + s.tracked.capacity() * std::mem::size_of::<Tracked>()
                    + s.events.capacity() * std::mem::size_of::<Event>()
                    + s.bands.capacity() * std::mem::size_of::<Band>()
            })
    }

    fn buffer(&mut self, k: i64, price: i64, size: f64) {
        let Ok(price) = i32::try_from(price) else {
            return; // over $2,147: outside any band a run can start in
        };
        let size = size.min(f64::from(u32::MAX)) as u32;
        // Aggregated per (slot, price): look back over this slot's entries only.
        for e in self.buf.iter_mut().rev() {
            if e.0 as i64 != k {
                break;
            }
            if e.1 == price {
                e.2 = e.2.saturating_add(size);
                return;
            }
        }
        self.buf.push_back((k as i32, price, size));
    }

    /// A print for a slot already decided: it is in the past, so it may still count in the
    /// session high, the profile and an open outcome, never in a decided close.
    fn late(&mut self, k: i64, p: Print) -> bool {
        let px = p.price as f64 / 1e6;
        let mut changed = false;
        if p.ohlc {
            self.day_high = if self.day_high.is_nan() {
                px
            } else {
                self.day_high.max(px)
            };
        }
        match self.shelf.as_mut() {
            Some(sh) => {
                if p.vol && k >= sh.anchor_k {
                    sh.add(px, p.size);
                }
                if p.ohlc {
                    changed = sh.print(p.exec, px);
                }
            }
            None => {
                if p.vol && k + LEG_SLOTS + 1 >= self.done {
                    self.buffer(k, p.price, p.size);
                }
            }
        }
        changed
    }

    /// Decides every slot up to `due` that has prints. Returns (changed, runs confirmed).
    fn process(&mut self, due: i64, t0: i64, out: &mut Vec<Run>) -> bool {
        let mut ready: Vec<Print> = Vec::new();
        let mut keep: Vec<Print> = Vec::new();
        for p in self.tail.drain(..) {
            if (p.exec - t0).div_euclid(BAR_NS) <= due {
                ready.push(p);
            } else {
                keep.push(p);
            }
        }
        self.tail = keep;
        ready.sort_by_key(|p| p.exec);
        let mut changed = false;
        let mut i = 0;
        while i < ready.len() {
            let k = (ready[i].exec - t0).div_euclid(BAR_NS);
            let mut j = i;
            while j < ready.len() && (ready[j].exec - t0).div_euclid(BAR_NS) == k {
                j += 1;
            }
            changed |= self.slot(k, &ready[i..j], t0, out);
            i = j;
        }
        self.done = self.done.max(due);
        // Unconfirmed: only the last 61 slots can hold a future run's start.
        while self
            .buf
            .front()
            .is_some_and(|e| (e.0 as i64) + LEG_SLOTS + 1 < self.done)
        {
            self.buf.pop_front();
        }
        self.dirty = !self.tail.is_empty();
        changed
    }

    fn slot(&mut self, k: i64, ps: &[Print], t0: i64, out: &mut Vec<Run>) -> bool {
        let end = t0 + (k + 1) * BAR_NS;
        let mut changed = false;
        let mut close = None;
        let mut high = f64::NAN;
        let (mut trades, mut dollars) = (0.0, 0.0);
        for p in ps {
            let px = p.price as f64 / 1e6;
            if p.ohlc {
                close = Some(px);
                high = if high.is_nan() { px } else { high.max(px) };
                if let Some(sh) = self.shelf.as_mut() {
                    changed |= sh.print(p.exec, px);
                }
            }
            if p.vol {
                trades += 1.0;
                dollars += p.size * px;
                match self.shelf.as_mut() {
                    Some(sh) => sh.add(px, p.size),
                    None => self.buffer(k, p.price, p.size),
                }
            }
        }
        if trades > 0.0 {
            self.liq.push_back((k, trades, dollars));
        }
        while self.liq.front().is_some_and(|e| e.0 + LEG_SLOTS < k) {
            self.liq.pop_front();
        }
        if !high.is_nan() {
            self.day_high = if self.day_high.is_nan() {
                high
            } else {
                self.day_high.max(high)
            };
        }
        let Some(close) = close else {
            return changed;
        };
        // The run trigger (`rockets(d, 20)`), on the traded slots.
        while self.mins.front().is_some_and(|&(i, _)| i <= k - LEG_SLOTS) {
            self.mins.pop_front();
        }
        let in_refr = self.refr.is_some_and(|t| end < t + REFRACTORY_NS);
        if !in_refr && let Some(&(ki, l)) = self.mins.front() {
            let lm = (l * 1e6).round() as i64;
            let factor = 1.0 + GAIN_PCT / 100.0;
            if close >= factor * l - 1e-12 && (BAND_MICROS.0..=BAND_MICROS.1).contains(&lm) {
                let (mut tr, mut dl) = (0.0, 0.0);
                for &(kk, t, d) in &self.liq {
                    if kk >= ki && kk <= k {
                        tr += t;
                        dl += d;
                    }
                }
                if tr >= MIN_TRADES && dl >= MIN_DOLLARS {
                    let run = Run {
                        start_ns: t0 + (ki + 1) * BAR_NS,
                        start_price: l,
                        plus20_ns: end,
                    };
                    self.confirm(run, ki);
                    self.refr = Some(end);
                    out.push(run);
                    changed = true;
                }
            }
        }
        while self.mins.back().is_some_and(|&(_, c)| c >= close) {
            self.mins.pop_back();
        }
        self.mins.push_back((k, close));
        if let Some(sh) = self.shelf.as_mut() {
            changed |= sh.decide(end, close, self.prev_close, self.day_high);
        }
        self.prev_close = Some(close);
        changed
    }

    fn confirm(&mut self, run: Run, start_k: i64) {
        if self.shelf.is_none() {
            let mut sh = Shelf {
                profile: Profile::new(profile::bin_width(run.start_price)),
                anchor_k: start_k,
                runs: Vec::new(),
                tracked: Vec::new(),
                events: Vec::new(),
                last_ev: None,
                bands: Vec::new(),
                dropped: 0,
            };
            // The profile from the start slot's open: the buffered prints since then.
            for &(kk, price, size) in &self.buf {
                if kk as i64 >= start_k {
                    sh.add(price as f64 / 1e6, f64::from(size));
                }
            }
            self.buf = VecDeque::new();
            self.shelf = Some(Box::new(sh));
        }
        if let Some(sh) = self.shelf.as_mut() {
            sh.runs.push(run);
        }
    }

    fn free_idle(&mut self) {
        self.mins = VecDeque::new();
        self.liq = VecDeque::new();
        self.buf = VecDeque::new();
        if self.tail.is_empty() {
            self.tail = Vec::new();
        }
    }
}

/// What was written to (or read back from) the forward log, by key.
fn key(kind: &str, symbol: &str, t: i64) -> String {
    format!("{kind}|{symbol}|{t}")
}

/// Every symbol's volume shelves for the current trading date.
#[derive(Clone, Debug, Default)]
pub struct ZoneBoard {
    enabled: bool,
    /// Live: lines for the forward log ([`ZoneBoard::take_log`]); a replay or a rebuild keeps none.
    log_on: bool,
    index: HashMap<Arc<str>, usize>,
    syms: Vec<Sym>,
    dirty: Vec<usize>,
    /// Confirmed symbols (indexes), for the outcome clock.
    confirmed: Vec<usize>,
    days: DayCache,
    date: Option<NaiveDate>,
    t0: i64,
    ord: u32,
    /// The last slot decided board-wide (−1 = none).
    due: i64,
    last_free: i64,
    revision: u64,
    log: Vec<(NaiveDate, String)>,
    logged: HashSet<String>,
}

fn round4(x: f64) -> f64 {
    (x * 10_000.0).round() / 10_000.0
}

impl ZoneBoard {
    pub fn new(enabled: bool) -> Self {
        ZoneBoard {
            enabled,
            due: -1,
            ..Default::default()
        }
    }

    pub fn disabled() -> Self {
        Self::new(false)
    }

    /// A board for one symbol's history (`crate::history`): no log, adopted afterwards.
    pub fn rebuilder() -> Self {
        Self::new(true)
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// Live: write the forward log, skipping the keys already in today's file.
    pub fn enable_log(&mut self, already: HashSet<String>) {
        self.log_on = true;
        self.logged.extend(already);
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn date(&self) -> Option<NaiveDate> {
        self.date
    }

    /// Whether a print of a symbol not loaded yet needs its history here: a symbol in the wide
    /// band has no state until its day is adopted.
    pub fn wants_history(&self, t: &Trade) -> bool {
        self.enabled && (WIDE_MICROS.0..=WIDE_MICROS.1).contains(&t.price.micros())
    }

    fn roll(&mut self, date: NaiveDate, t0: i64, ord: u32) {
        // The previous day's open events end with it.
        let through = self.t0 + NSLOT * BAR_NS + HORIZON_NS + BAR_NS;
        if self.date.is_some() {
            for &i in &self.confirmed.clone() {
                if let Some(sh) = self.syms[i].shelf.as_mut()
                    && sh.settle(through, true)
                {
                    self.log_outcomes(i);
                }
            }
        }
        self.index.clear();
        self.syms.clear();
        self.dirty.clear();
        self.confirmed.clear();
        self.date = Some(date);
        self.t0 = t0;
        self.ord = ord;
        self.due = -1;
        self.revision += 1;
    }

    fn sym_index(&mut self, symbol: &str) -> usize {
        if let Some(&i) = self.index.get(symbol) {
            return i;
        }
        let s: Arc<str> = Arc::from(symbol);
        let i = self.syms.len();
        self.syms.push(Sym::new(s.clone()));
        self.index.insert(s, i);
        i
    }

    /// One print with the chart engine's verdict on it. O(1) (a push; decided at the tick).
    pub fn apply_trade(&mut self, t: &Trade, counted: Counted) {
        if !self.enabled || (!counted.ohlc && !counted.volume) {
            return;
        }
        let Some(session) = t.session else { return };
        if session.kind == SessionKind::Overnight {
            return;
        }
        let (t0, ord) = self.days.get(session.date);
        if self.date != Some(session.date) {
            if self.date.is_some_and(|d| session.date < d) {
                return; // a straggler from a day already rolled
            }
            self.roll(session.date, t0, ord);
        }
        let exec = t.event_ts.0;
        let k = (exec - t0).div_euclid(BAR_NS);
        if !(0..NSLOT).contains(&k) {
            return;
        }
        let i = match self.index.get(t.symbol.as_str()) {
            Some(&i) => i,
            None => {
                if !(WIDE_MICROS.0..=WIDE_MICROS.1).contains(&t.price.micros()) {
                    return;
                }
                self.sym_index(&t.symbol)
            }
        };
        let p = Print {
            exec,
            price: t.price.micros(),
            size: t.size as f64,
            ohlc: counted.ohlc,
            vol: counted.volume,
        };
        let s = &mut self.syms[i];
        s.last_ns = s.last_ns.max(exec);
        if k <= s.done {
            if s.late(k, p) {
                self.revision += 1;
                self.log_outcomes(i);
            }
            return;
        }
        s.tail.push(p);
        if !s.dirty {
            s.dirty = true;
            self.dirty.push(i);
        }
    }

    /// Decides every slot that ended at least [`SETTLE_NS`] before the market clock `now`, then
    /// resolves what the elapsed time decides. Returns true when anything shown changed.
    pub fn tick(&mut self, now: i64) -> bool {
        if !self.enabled || self.date.is_none() {
            return false;
        }
        let due = ((now - SETTLE_NS - self.t0).div_euclid(BAR_NS) - 1).min(NSLOT - 1);
        if due <= self.due {
            return false;
        }
        self.due = due;
        let before = self.revision;
        let t0 = self.t0;
        let mut runs = Vec::new();
        let dirty = std::mem::take(&mut self.dirty);
        for i in dirty {
            let was = self.syms[i].shelf.is_some();
            runs.clear();
            let changed = self.syms[i].process(due, t0, &mut runs);
            if self.syms[i].dirty {
                self.dirty.push(i);
            }
            if !was && self.syms[i].shelf.is_some() {
                self.confirmed.push(i);
            }
            if changed {
                self.revision += 1;
            }
            if self.log_on {
                for r in &runs {
                    self.log_run(i, r);
                }
                self.log_entries(i);
                self.log_outcomes(i);
            }
        }
        // Time decides too: a completed minute, a window that ran out.
        let through = t0 + (due + 1) * BAR_NS;
        let day_over = due >= NSLOT - 1;
        for &i in &self.confirmed.clone() {
            let Some(sh) = self.syms[i].shelf.as_mut() else {
                continue;
            };
            if sh.settle(through, day_over) {
                self.revision += 1;
                self.log_outcomes(i);
            }
        }
        if now - self.last_free >= MIN_NS {
            self.last_free = now;
            for s in &mut self.syms {
                if s.shelf.is_none() && !s.dirty && now - s.last_ns > IDLE_FREE_NS {
                    s.free_idle();
                }
            }
        }
        self.revision != before
    }

    fn log_line(&mut self, k: String, line: Value) {
        if !self.log_on || self.logged.contains(&k) {
            return;
        }
        self.logged.insert(k);
        if let Some(d) = self.date {
            self.log.push((d, line.to_string()));
        }
    }

    fn log_run(&mut self, i: usize, r: &Run) {
        let sym = self.syms[i].symbol.clone();
        let line = json!({
            "kind": "run", "v": 1, "date": self.date.map(|d| d.to_string()),
            "symbol": &*sym, "start_ns": r.start_ns, "start_price": r.start_price,
            "plus20_ns": r.plus20_ns,
        });
        self.log_line(key("run", &sym, r.plus20_ns), line);
    }

    fn log_entries(&mut self, i: usize) {
        if !self.log_on {
            return;
        }
        let sym = self.syms[i].symbol.clone();
        let Some(sh) = self.syms[i].shelf.as_ref() else {
            return;
        };
        let lines: Vec<(String, Value)> = sh
            .events
            .iter()
            .map(|e| {
                (
                    key("entry", &sym, e.t_ns),
                    json!({
                        "kind": "entry", "v": 1, "date": self.date.map(|d| d.to_string()),
                        "symbol": &*sym, "t_ns": e.t_ns, "entry_price": e.entry,
                        "zone_lo": e.lo, "zone_hi": e.hi, "peak_price": e.peak,
                        "run_start_ns": e.run_start_ns, "kind_zone": "volume_shelf",
                        "score": round4(e.retr), "strategy": STRATEGY, "version": VERSION,
                    }),
                )
            })
            .collect();
        for (k, l) in lines {
            self.log_line(k, l);
        }
    }

    fn log_outcomes(&mut self, i: usize) {
        if !self.log_on {
            return;
        }
        let sym = self.syms[i].symbol.clone();
        let Some(sh) = self.syms[i].shelf.as_ref() else {
            return;
        };
        let lines: Vec<(String, Value)> = sh
            .events
            .iter()
            .filter(|e| e.state != State::InShelf)
            .map(|e| {
                (
                    key("outcome", &sym, e.t_ns),
                    json!({
                        "kind": "outcome", "v": 1, "date": self.date.map(|d| d.to_string()),
                        "symbol": &*sym, "t_ns": e.t_ns, "outcome": e.state.as_str(),
                        "resolved_ns": e.resolved_ns, "entry_price": e.entry,
                        "zone_lo": e.lo, "zone_hi": e.hi,
                    }),
                )
            })
            .collect();
        for (k, l) in lines {
            self.log_line(k, l);
        }
    }

    /// The forward-log lines produced since the last call (live only).
    pub fn take_log(&mut self) -> Vec<(NaiveDate, String)> {
        std::mem::take(&mut self.log)
    }

    /// Adopts `symbol`'s day rebuilt from history (`crate::history`), brought up to `now` first.
    /// Lines the previous process did not log (by key) are logged now.
    pub fn adopt(&mut self, symbol: &str, mut rebuilt: ZoneBoard, now: i64) {
        if !self.enabled {
            return;
        }
        rebuilt.tick(now);
        let Some(&ri) = rebuilt.index.get(symbol) else {
            return;
        };
        let Some(date) = rebuilt.date else { return };
        if self.date.is_none_or(|d| d < date) {
            let (t0, ord) = (rebuilt.t0, rebuilt.ord);
            self.days.get(date);
            self.roll(date, t0, ord);
        } else if self.date != Some(date) {
            return; // the live board is on a later day
        }
        let s = rebuilt.syms.swap_remove(ri);
        let i = self.sym_index(symbol);
        let had = self.syms[i].shelf.is_some();
        // A print the live board already holds for this symbol (none: an unloaded symbol's
        // prints are held by the market) would be lost here; keep its tail just in case.
        let mut s = s;
        let live_tail = std::mem::take(&mut self.syms[i].tail);
        s.tail.extend(live_tail);
        s.dirty = !s.tail.is_empty();
        self.syms[i] = s;
        if self.syms[i].dirty && !self.dirty.contains(&i) {
            self.dirty.push(i);
        }
        if !had && self.syms[i].shelf.is_some() && !self.confirmed.contains(&i) {
            self.confirmed.push(i);
        }
        self.revision += 1;
        if self.log_on {
            let runs = self.syms[i]
                .shelf
                .as_ref()
                .map(|sh| sh.runs.clone())
                .unwrap_or_default();
            for r in &runs {
                self.log_run(i, r);
            }
            self.log_entries(i);
            self.log_outcomes(i);
        }
    }

    pub fn shelf(&self, symbol: &str) -> Option<&Shelf> {
        self.index
            .get(symbol)
            .and_then(|&i| self.syms[i].shelf.as_deref())
    }

    /// Symbols with state, confirmed symbols, and the bytes they hold (estimate).
    pub fn cost(&self) -> (usize, usize, usize) {
        (
            self.syms.len(),
            self.confirmed.len(),
            self.syms.iter().map(Sym::bytes).sum::<usize>()
                + self.index.capacity() * 24
                + self.syms.capacity().saturating_sub(self.syms.len()) * std::mem::size_of::<Sym>(),
        )
    }

    /// The `zones` object of the Runners frame at the market clock `now` (epoch ns): per symbol
    /// with something to show, its qualifying shelves and its latest event (a resolved one for
    /// [`SHOW_RESOLVED_NS`]).
    pub fn json(&self, now: i64) -> Value {
        let mut m = serde_json::Map::new();
        for &i in &self.confirmed {
            let s = &self.syms[i];
            let Some(sh) = s.shelf.as_ref() else { continue };
            let event = sh.events.last().filter(|e| match e.resolved_ns {
                None => true,
                Some(at) => e.state != State::Expired && now - at < SHOW_RESOLVED_NS,
            });
            if sh.bands.is_empty() && event.is_none() {
                continue;
            }
            let r = |x: f64| (x * 1e6).round() / 1e6;
            m.insert(
                s.symbol.to_string(),
                json!({
                    "bands": sh.bands.iter().map(|b| json!({
                        "lo": r(b.lo), "hi": r(b.hi), "formed": b.formed_ns / 1_000_000,
                        "armed": b.armed,
                    })).collect::<Vec<_>>(),
                    "event": event.map(|e| json!({
                        "state": e.state.as_str(), "lo": r(e.lo), "hi": r(e.hi),
                        "entry": e.entry, "at": e.t_ns / 1_000_000,
                        "resolved": e.resolved_ns.map(|t| t / 1_000_000),
                    })),
                }),
            );
        }
        Value::Object(m)
    }
}

/// Today's logged keys from `dir/<today>.jsonl` (missing file = none).
pub fn logged_keys(dir: &std::path::Path, today: NaiveDate) -> HashSet<String> {
    let mut out = HashSet::new();
    let path = dir.join(format!("{today}.jsonl"));
    for line in std::fs::read_to_string(path).unwrap_or_default().lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let (Some(kind), Some(sym)) = (
            v.get("kind").and_then(Value::as_str),
            v.get("symbol").and_then(Value::as_str),
        ) else {
            continue;
        };
        let t = match kind {
            "run" => v.get("plus20_ns"),
            _ => v.get("t_ns"),
        }
        .and_then(Value::as_i64);
        if let Some(t) = t {
            out.insert(key(kind, sym, t));
        }
    }
    out
}

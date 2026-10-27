//! Per-symbol rolling state and the causal features a [`super::RocketScorer`] reads.
//!
//! # The grid
//!
//! The research bake-off's grid exactly (`docs/research/rocket-precursors/protocol.md` §2, on
//! branch `research/rocket-precursors`, `crates/setup-research/src/rocket.rs`): 15 s slots from
//! 04:00 to 20:00 ET of the print's trading date (3,840 slots); a print executed at `e` belongs to
//! slot `floor((e - 04:00) / 15 s)`, whose barrier is the slot's END. OHLC comes from the prints
//! the chart engine counted for OHLC, volume / trades / dollars from the ones it counted for
//! volume ([`Counted`], the same verdict the candles and the quote board take). Overnight prints
//! (20:00–04:00) take no part, as in the research.
//!
//! # Cost
//!
//! Per trade: O(1). One hash lookup, a fold into the slot's bar in a 256-slot ring (64 minutes,
//! enough for the longest research window, `range60` = 240 slots), and, when the print starts a
//! new slot, the previous slot's fold into the day's running totals ("finalizing", below).
//!
//! Per barrier: one pass over each in-band symbol's ring (≤ 256 slots) for the symbols that
//! traded in the last 30 minutes (the research's "unit" barriers), nothing for the rest. Rings of
//! symbols idle for over 64 minutes are freed, so memory follows the active set (~11 KB each).
//! `rocket::tests::cost_per_trade_and_per_barrier` measures both on ~7k symbols.
//!
//! # Causality
//!
//! A slot's bar only becomes part of the day's running totals (VWAP, day high, the run of higher
//! closes) once it is complete: when a print for a later slot arrives, or when the barrier that
//! ends it is processed. Window sums at barrier `k` only read slots `<= k`, so the slot in
//! progress at the tick (its prints may already be in the ring) is never seen. A print that
//! arrives after its slot was finalized (a late print) is folded into its own slot and the running
//! totals as soon as it arrives: it is in the past by then, so a later barrier may see it, an
//! earlier one never did. `rocket::tests::features_are_causal` pins this.
//!
//! # Differences from the research
//!
//! - Live scoring admits `$0.10–$25` (owner, 2026-10-26). The frozen research used
//!   `$0.15–$25`; its outcomes do not validate the added price range.
//! - The tick rule (`upt3`) runs in arrival order, the research's in execution order.
//! - `pdh`, `rvol` and the four news counts (`n15`…`nday`) are `NaN`: the previous day's high, the
//!   30-day average volume and the symbol's news times are not wired into this module yet. A
//!   scorer that needs them must treat `NaN` as missing (the research does: "Missing = NaN").
//! - `ret*` look back at most 255 slots (the ring); the research looks back to 04:00. Only
//!   `ret15` (60 slots) is used by the research and it is unaffected.

use std::sync::Arc;

use chrono::{Datelike, NaiveDate};
use market_events::session::SessionKind;
use market_events::{Trade, UnixNanos};

use crate::quotes::Counted;

pub const BAR_NS: i64 = 15_000_000_000;
/// Slots from 04:00 to 20:00 ET.
pub const NSLOT: i64 = 3840;
/// Slots kept per symbol: 64 minutes.
pub const RING: usize = 256;
/// The research band (rocket start price), micro-dollars: `$0.15–$25`.
pub const BAND_MICROS: (i64, i64) = (150_000, 25_000_000);
/// Live precursor eligibility (owner, 2026-10-26): `$0.10–$25`. The historical
/// outcome definition above stays separate from the prices admitted for scoring.
pub const SCORING_BAND_MICROS: (i64, i64) = (
    market_events::PriceBand::DEFAULT.min.micros(),
    market_events::PriceBand::DEFAULT.max.micros(),
);
/// A symbol whose prints are all outside this (dollars) never gets a ring.
const WIDE_BAND: (f64, f64) = (0.05, 30.0);

/// The research feature names, in the research's order (`setup-research/src/rocket.rs`
/// `FEATS`). A research model's weights index these.
pub const FEATS: [&str; 31] = [
    "logp", "range30", "range60", "range5", "idle30", "vr1", "vr3", "vr5", "tr3", "dv5", "grind",
    "upc3", "ret1", "ret3", "ret5", "ret15", "upt3", "odd3", "blk3", "dhi", "pdh", "gain", "gap",
    "rvol", "vwap", "tod", "sess", "n15", "n60", "n240", "nday",
];
pub const NF: usize = FEATS.len();

/// Indexes into [`Features::x`], named as in the research.
#[allow(dead_code)]
pub mod ix {
    pub const LOGP: usize = 0;
    pub const RANGE30: usize = 1;
    pub const RANGE60: usize = 2;
    pub const RANGE5: usize = 3;
    pub const IDLE30: usize = 4;
    pub const VR1: usize = 5;
    pub const VR3: usize = 6;
    pub const VR5: usize = 7;
    pub const TR3: usize = 8;
    pub const DV5: usize = 9;
    pub const GRIND: usize = 10;
    pub const UPC3: usize = 11;
    pub const RET1: usize = 12;
    pub const RET3: usize = 13;
    pub const RET5: usize = 14;
    pub const RET15: usize = 15;
    pub const UPT3: usize = 16;
    pub const ODD3: usize = 17;
    pub const BLK3: usize = 18;
    pub const DHI: usize = 19;
    pub const PDH: usize = 20;
    pub const GAIN: usize = 21;
    pub const GAP: usize = 22;
    pub const RVOL: usize = 23;
    pub const VWAP: usize = 24;
    pub const TOD: usize = 25;
    pub const SESS: usize = 26;
    pub const N15: usize = 27;
    pub const N60: usize = 28;
    pub const N240: usize = 29;
    pub const NDAY: usize = 30;
}

/// What a scorer sees for one symbol at one completed 15 s barrier. Everything in it was known
/// at `at_ns` (module docs, "Causality").
#[derive(Clone, Debug)]
pub struct Features {
    pub symbol: Arc<str>,
    /// The barrier: the END of slot `slot`, epoch ns.
    pub at_ns: i64,
    /// The slot index from 04:00 ET (0..3840).
    pub slot: i64,
    /// The last OHLC-counted close at or before the barrier (dollars).
    pub price: f64,
    pub session: Option<SessionKind>,
    /// The previous trading day's official close, when the closes file has one.
    pub prev_close: Option<f64>,
    /// The research features ([`FEATS`], [`ix`]); `NaN` = missing.
    pub x: [f64; NF],
    /// `ln(price / close 30 minutes ago)` (120 slots); not a research feature, the placeholder's
    /// gate. `NaN` when nothing traded that long ago today.
    pub ret30: f64,
    /// The barrier slot's own bar (zero when it did not trade).
    pub bar_volume: f64,
    pub bar_trades: f64,
    /// Volume over the last 3 minutes, and the prior 30 minutes' average per 3 minutes.
    pub vol3: f64,
    pub vol3_prior: f64,
    /// Highest high since 04:00 (dollars), and the day's VWAP (`NaN` before any volume).
    pub day_high: f64,
    pub vwap: f64,
    /// Minutes since the start of the barrier's session (04:00, 09:30 or 16:00 ET).
    pub minutes_in_session: f64,
}

impl Features {
    /// A feature by its research name; `NaN` for an unknown one.
    pub fn get(&self, name: &str) -> f64 {
        FEATS
            .iter()
            .position(|f| *f == name)
            .map_or(f64::NAN, |i| self.x[i])
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct Bar {
    /// `day ordinal * 4096 + slot`; 0 = empty.
    key: u32,
    has_ohlc: bool,
    upc: bool,
    run: u16,
    open: f32,
    high: f32,
    low: f32,
    close: f32,
    // Detector-only originals. Never used by frozen features, scores, or outcomes.
    exact_close: i64,
    exact_high: i64,
    vol: f32,
    dollars: f32,
    trades: f32,
    upv: f32,
    downv: f32,
    odd: f32,
    blk: f32,
}

/// One symbol's state for its current trading date.
#[derive(Clone, Debug)]
pub(crate) struct Sym {
    pub(crate) symbol: Arc<str>,
    pub(crate) latest_ohlc_exec_ns: i64,
    /// The live prefix has not yet been atomically adopted from history.
    pub(crate) unrestored: bool,
    /// This date's rebuilt prefix was adopted, but was structurally incomplete. Runners only.
    pub(crate) unavailable: bool,
    unavailable_day: u32,
    ring: Option<Box<[Bar; RING]>>,
    day: u32,
    /// The newest slot key folded, and the last one finalized into the day's totals.
    newest: u32,
    fin: u32,
    tref: Option<i64>,
    dir: i8,
    cum_vol: f64,
    cum_dollars: f64,
    day_high: f64,
    first: f64,
    last_close: f64,
    run: u32,
    /// Key of the last finalized slot with an OHLC print (0 = none today).
    last_traded: u32,
    /// The session (`session_of`) of the finalized bars, its highest high, and that high before
    /// the last finalized OHLC bar (the session-high variant the "running" calibration compared, [`super::running`]).
    sess: u8,
    sess_high: f64,
    sess_high_prev: f64,
}

/// 0 pre-market (04:00), 1 regular (09:30), 2 after-hours (16:00), by slot.
pub(crate) fn session_of(slot: i64) -> u8 {
    match slot {
        ..330 => 0,
        330..720 => 1,
        _ => 2,
    }
}

impl Sym {
    fn new(symbol: Arc<str>) -> Self {
        Sym {
            symbol,
            latest_ohlc_exec_ns: 0,
            unrestored: false,
            unavailable: false,
            unavailable_day: 0,
            ring: None,
            day: 0,
            newest: 0,
            fin: 0,
            tref: None,
            dir: 0,
            cum_vol: 0.0,
            cum_dollars: 0.0,
            day_high: f64::NAN,
            first: f64::NAN,
            last_close: f64::NAN,
            run: 0,
            last_traded: 0,
            sess: 0,
            sess_high: f64::NAN,
            sess_high_prev: f64::NAN,
        }
    }

    fn reset_day(&mut self, day: u32) {
        let ring = self.ring.take();
        let symbol = self.symbol.clone();
        let unrestored = self.unrestored;
        let unavailable = self.unavailable && self.unavailable_day == day;
        let unavailable_day = self.unavailable_day;
        *self = Sym::new(symbol);
        // Pending adoption can cross dates; a previous date's structural loss cannot make
        // today's live prefix unavailable once that symbol actually starts a new date.
        self.unrestored = unrestored;
        self.unavailable = unavailable;
        self.unavailable_day = unavailable_day;
        self.ring = ring;
        self.day = day;
    }

    fn bar(&self, key: u32) -> Option<&Bar> {
        let b = &self.ring.as_ref()?[key as usize % RING];
        (b.key == key).then_some(b)
    }

    /// Folds the newest slot into the day's totals once it is complete.
    fn finalize_newest(&mut self) {
        if self.newest == 0 || self.newest <= self.fin {
            return;
        }
        let key = self.newest;
        self.fin = key;
        let Some(ring) = self.ring.as_mut() else {
            return;
        };
        let b = &mut ring[key as usize % RING];
        if b.key != key {
            return;
        }
        self.cum_vol += f64::from(b.vol);
        self.cum_dollars += f64::from(b.dollars);
        if b.has_ohlc {
            let (h, c) = (f64::from(b.high), f64::from(b.close));
            self.day_high = if self.day_high.is_nan() {
                h
            } else {
                self.day_high.max(h)
            };
            if self.first.is_nan() {
                self.first = f64::from(b.open);
            }
            if !self.last_close.is_nan() && c > self.last_close {
                self.run += 1;
                b.upc = true;
            } else {
                self.run = 0;
            }
            b.run = self.run.min(u32::from(u16::MAX)) as u16;
            self.last_close = c;
            self.last_traded = key;
            let sess = session_of(i64::from(key % 4096));
            if sess != self.sess {
                self.sess = sess;
                self.sess_high = f64::NAN;
            }
            self.sess_high_prev = self.sess_high;
            self.sess_high = if self.sess_high.is_nan() {
                h
            } else {
                self.sess_high.max(h)
            };
        }
    }
}

/// The trading date's 04:00 ET and its ordinal, cached (one tz conversion per date, not per
/// print).
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct DayCache {
    date: Option<NaiveDate>,
    t0: i64,
    ord: u32,
}

impl DayCache {
    pub(crate) fn get(&mut self, date: NaiveDate) -> (i64, u32) {
        if self.date != Some(date) {
            let t0 = market_events::session::et(date, 4, 0, 0, 0)
                .map(|t| t.0)
                .unwrap_or(0);
            // Days since 2000-01-01: ~9.6k in 2026, so `ord * 4096 + slot` fits a u32 for
            // centuries.
            let ord = (date.num_days_from_ce() - 730_120).max(1) as u32;
            *self = DayCache {
                date: Some(date),
                t0,
                ord,
            };
        }
        (self.t0, self.ord)
    }
}

pub(crate) fn key_of(ord: u32, slot: i64) -> u32 {
    ord * 4096 + slot as u32
}

/// Every symbol's [`Sym`].
#[derive(Clone, Debug, Default)]
pub(crate) struct Tape {
    pub(crate) index: std::collections::HashMap<Arc<str>, usize>,
    pub(crate) syms: Vec<Sym>,
    days: DayCache,
    /// The latest execution time folded (epoch ns): the tape can run a little ahead of the
    /// market clock the quotes task reads (`RocketBoard::tick`).
    pub(crate) latest_ts: i64,
}

impl Tape {
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

    /// One print with the chart engine's verdict on it. O(1).
    pub(crate) fn apply_trade(&mut self, t: &Trade, counted: Counted) {
        if !counted.ohlc && !counted.volume {
            return;
        }
        let Some(session) = t.session else { return };
        if session.kind == SessionKind::Overnight {
            return;
        }
        let price = t.price.to_f64();
        let known = self.index.contains_key(t.symbol.as_str());
        if !known && !(WIDE_BAND.0..=WIDE_BAND.1).contains(&price) {
            return;
        }
        let (t0, ord) = self.days.get(session.date);
        let slot = (t.event_ts.0 - t0).div_euclid(BAR_NS);
        if !(0..NSLOT).contains(&slot) {
            return;
        }
        self.latest_ts = self.latest_ts.max(t.event_ts.0);
        let i = self.sym_index(&t.symbol);
        let s = &mut self.syms[i];
        if s.ring.is_none() && !(WIDE_BAND.0..=WIDE_BAND.1).contains(&price) {
            return;
        }
        if s.day != ord {
            s.reset_day(ord);
        }
        let key = key_of(ord, slot);
        let late = key < s.newest;
        if key > s.newest {
            s.finalize_newest();
            s.newest = key;
        }
        let ring = s
            .ring
            .get_or_insert_with(|| Box::new([Bar::default(); RING]));
        let b = &mut ring[key as usize % RING];
        if b.key > key {
            return; // the ring moved past this slot: too old to matter
        }
        if b.key < key {
            *b = Bar {
                key,
                ..Bar::default()
            };
        }
        let p = price as f32;
        if counted.ohlc {
            s.latest_ohlc_exec_ns = s.latest_ohlc_exec_ns.max(t.event_ts.0);
            b.exact_close = t.price.micros();
            b.exact_high = b.exact_high.max(t.price.micros());
            if b.has_ohlc {
                b.high = b.high.max(p);
                b.low = b.low.min(p);
            } else {
                b.has_ohlc = true;
                b.open = p;
                b.high = p;
                b.low = p;
            }
            b.close = p;
        }
        let mut late_vol = (0.0, 0.0);
        if counted.volume {
            let v = t.size as f64;
            b.vol += v as f32;
            b.dollars += (v * price) as f32;
            b.trades += 1.0;
            if t.size < 100 {
                b.odd += 1.0;
            }
            if v * price >= 20_000.0 {
                b.blk += v as f32;
            }
            let m = t.price.micros();
            match s.tref {
                Some(r) if m > r => s.dir = 1,
                Some(r) if m < r => s.dir = -1,
                _ => {}
            }
            s.tref = Some(m);
            if s.dir > 0 {
                b.upv += v as f32;
            } else if s.dir < 0 {
                b.downv += v as f32;
            }
            late_vol = (v, v * price);
        }
        if late {
            // In the past already: into the running totals now (module docs, "Causality").
            s.cum_vol += late_vol.0;
            s.cum_dollars += late_vol.1;
            if counted.ohlc && !s.day_high.is_nan() {
                s.day_high = s.day_high.max(price);
            }
        }
    }

    pub(crate) fn apply_unrestored_trade(&mut self, t: &Trade, counted: Counted) {
        self.apply_trade(t, counted);
        if let Some(&i) = self.index.get(t.symbol.as_str()) {
            self.syms[i].unrestored = true;
        }
    }

    /// An empty authoritative rebuild replaces provisional facts too. Merely clearing the
    /// mark would otherwise let the old live-only bars become admissible again.
    pub(crate) fn adopt(&mut self, symbol: &str, other: &Tape, unavailable: Option<NaiveDate>) {
        let i = self.sym_index(symbol);
        let sym = self.syms[i].symbol.clone();
        self.syms[i] = other
            .index
            .get(symbol)
            .map_or_else(|| Sym::new(sym.clone()), |&j| other.syms[j].clone());
        self.syms[i].symbol = sym;
        self.syms[i].unrestored = false;
        self.syms[i].unavailable = unavailable.is_some();
        self.syms[i].unavailable_day = unavailable.map_or(0, |date| self.days.get(date).1);
        self.latest_ts = self.latest_ts.max(other.latest_ts);
    }

    /// The trading date's 04:00 and ordinal (cached).
    pub(crate) fn day(&mut self, date: NaiveDate) -> (i64, u32) {
        self.days.get(date)
    }
}

/// Barrier-wide context, computed once per barrier.
pub(crate) struct Barrier {
    pub(crate) ord: u32,
    pub(crate) slot: i64,
    pub(crate) key: u32,
    pub(crate) at_ns: i64,
    pub(crate) session: Option<SessionKind>,
    pub(crate) sess_code: f64,
    pub(crate) minutes_in_session: f64,
}

impl Barrier {
    pub(crate) fn new(t0: i64, ord: u32, slot: i64) -> Barrier {
        let at_ns = t0 + (slot + 1) * BAR_NS;
        let session = market_events::session::classify(UnixNanos(at_ns - 1)).map(|s| s.kind);
        let sess_code = match session {
            Some(SessionKind::Pre) => 0.0,
            Some(SessionKind::Regular) => 1.0,
            Some(SessionKind::After) => 2.0,
            _ => f64::NAN,
        };
        let start = match session {
            Some(SessionKind::Regular) => 330,
            Some(SessionKind::After) => 720,
            _ => 0,
        };
        let minutes_in_session = (slot + 1) as f64 / 4.0 - f64::from(start);
        Barrier {
            ord,
            slot,
            key: key_of(ord, slot),
            at_ns,
            session,
            sess_code,
            minutes_in_session,
        }
    }
}

/// Finalizes the symbol through the barrier and frees an idle ring; true when it still has one.
pub(crate) fn settle(s: &mut Sym, b: &Barrier) -> bool {
    if s.day != b.ord {
        // Nothing today: yesterday's ring is stale.
        s.ring = None;
        return false;
    }
    if s.newest <= b.key {
        s.finalize_newest();
    }
    if s.newest + RING as u32 <= b.key {
        s.ring = None;
    }
    s.ring.is_some()
}

fn ln_ratio(a: f64, b: f64) -> f64 {
    if a > 0.0 && b > 0.0 {
        (a / b).ln()
    } else {
        f64::NAN
    }
}

/// The features at barrier `b`, or `None` when it is not a unit barrier for the symbol (the
/// research's §5 freshness rule: last OHLC print within 30 minutes), with live
/// price eligibility widened to `$0.10–$25` (owner, 2026-10-26).
/// Call [`settle`] first.
pub(crate) fn features(
    s: &Sym,
    b: &Barrier,
    prev_close: impl FnOnce() -> Option<f64>,
) -> Option<Features> {
    let ring = s.ring.as_ref()?;
    if s.last_traded == 0 || s.last_traded > b.key || b.key - s.last_traded >= 120 {
        return None;
    }
    let price = f64::from(s.bar(s.last_traded)?.close);
    let pm = (price * 1e6).round() as i64;
    if !(SCORING_BAND_MICROS.0..=SCORING_BAND_MICROS.1).contains(&pm) {
        return None;
    }
    let prev_close = prev_close();
    let base = key_of(b.ord, 0);
    let k = b.slot;
    // Window sums by offset d = key - bar key (module docs).
    let (mut v4, mut v12, mut v20) = (0.0, 0.0, 0.0);
    let (mut pv4, mut pv12, mut pv20) = (0.0, 0.0, 0.0);
    let (mut t12, mut pt12) = (0.0, 0.0);
    let mut dl20 = 0.0;
    let (mut td120, mut td40, mut td12, mut upc12) = (0.0, 0.0, 0.0, 0.0);
    let (mut up12, mut dn12, mut odd12, mut blk12) = (0.0, 0.0, 0.0, 0.0);
    let (mut hi5, mut lo5, mut hi30, mut lo30, mut hi60, mut lo60) =
        (f64::NAN, f64::NAN, f64::NAN, f64::NAN, f64::NAN, f64::NAN);
    // close_at(k - m) for m in [4, 12, 20, 60, 120]: the traded slot with the smallest d >= m.
    const MS: [u32; 5] = [4, 12, 20, 60, 120];
    let mut best: [(u32, f64); 5] = [(u32::MAX, f64::NAN); 5];
    let mut bar_volume = 0.0;
    let mut bar_trades = 0.0;
    let upd = |hi: &mut f64, lo: &mut f64, h: f64, l: f64| {
        *hi = if hi.is_nan() { h } else { hi.max(h) };
        *lo = if lo.is_nan() { l } else { lo.min(l) };
    };
    for bar in ring.iter() {
        if bar.key == 0 || bar.key > b.key || bar.key < base {
            continue;
        }
        let d = b.key - bar.key;
        if d as usize >= RING {
            continue;
        }
        let vol = f64::from(bar.vol);
        let tr = f64::from(bar.trades);
        if d == 0 {
            bar_volume = vol;
            bar_trades = tr;
        }
        if d < 4 {
            v4 += vol;
        }
        if d < 12 {
            v12 += vol;
            t12 += tr;
            up12 += f64::from(bar.upv);
            dn12 += f64::from(bar.downv);
            odd12 += f64::from(bar.odd);
            blk12 += f64::from(bar.blk);
        }
        if d < 20 {
            v20 += vol;
            dl20 += f64::from(bar.dollars);
        }
        if (4..=123).contains(&d) {
            pv4 += vol;
        }
        if (12..=131).contains(&d) {
            pv12 += vol;
            pt12 += tr;
        }
        if (20..=139).contains(&d) {
            pv20 += vol;
        }
        if bar.has_ohlc {
            let (h, l, c) = (
                f64::from(bar.high),
                f64::from(bar.low),
                f64::from(bar.close),
            );
            if d < 12 {
                td12 += 1.0;
                if bar.upc {
                    upc12 += 1.0;
                }
            }
            if d < 40 {
                td40 += 1.0;
            }
            if d < 120 {
                td120 += 1.0;
                upd(&mut hi30, &mut lo30, h, l);
            }
            if d < 20 {
                upd(&mut hi5, &mut lo5, h, l);
            }
            if d < 240 {
                upd(&mut hi60, &mut lo60, h, l);
            }
            for (i, m) in MS.iter().enumerate() {
                if d >= *m && d < best[i].0 {
                    best[i] = (d, c);
                }
            }
        }
    }
    let range = |h: f64, l: f64| {
        if l > 0.0 && !h.is_nan() {
            (h - l) / l
        } else {
            f64::NAN
        }
    };
    let rate = |rec_sum: f64, prior_sum: f64, nrec: i64| {
        let rec = rec_sum / (nrec as f64 / 4.0);
        let a = k - nrec - 119;
        let bb = k - nrec;
        let span = (bb - a.max(0) + 1).max(0) as f64 / 4.0;
        let prior = if span > 0.0 { prior_sum / span } else { 0.0 };
        ((rec + 1.0) / (prior + 1.0)).ln()
    };
    let mut x = [f64::NAN; NF];
    x[ix::LOGP] = price.ln();
    x[ix::RANGE30] = range(hi30, lo30);
    x[ix::RANGE60] = range(hi60, lo60);
    x[ix::RANGE5] = range(hi5, lo5);
    let avail = (k + 1).min(120) as f64;
    x[ix::IDLE30] = 1.0 - td120 / avail;
    x[ix::VR1] = rate(v4, pv4, 4);
    x[ix::VR3] = rate(v12, pv12, 12);
    x[ix::VR5] = rate(v20, pv20, 20);
    x[ix::TR3] = rate(t12, pt12, 12);
    x[ix::DV5] = dl20.ln_1p();
    x[ix::GRIND] = if b.key - s.last_traded < 40 {
        let run = s.bar(s.last_traded).map_or(0.0, |x| f64::from(x.run));
        run.min(td40)
    } else {
        0.0
    };
    x[ix::UPC3] = if td12 > 0.0 { upc12 / td12 } else { 0.5 };
    x[ix::RET1] = ln_ratio(price, best[0].1);
    x[ix::RET3] = ln_ratio(price, best[1].1);
    x[ix::RET5] = ln_ratio(price, best[2].1);
    x[ix::RET15] = ln_ratio(price, best[3].1);
    x[ix::UPT3] = if up12 + dn12 > 0.0 {
        up12 / (up12 + dn12)
    } else {
        0.5
    };
    x[ix::ODD3] = if t12 > 0.0 { odd12 / t12 } else { 0.0 };
    x[ix::BLK3] = if v12 > 0.0 { blk12 / v12 } else { 0.0 };
    x[ix::DHI] = ln_ratio(price, s.day_high);
    x[ix::GAIN] = prev_close.map_or(f64::NAN, |pc| ln_ratio(price, pc));
    x[ix::GAP] = prev_close.map_or(f64::NAN, |pc| ln_ratio(s.first, pc));
    let vwap = if s.cum_vol > 0.0 {
        s.cum_dollars / s.cum_vol
    } else {
        f64::NAN
    };
    x[ix::VWAP] = ln_ratio(price, vwap);
    x[ix::TOD] = (k + 1) as f64 / NSLOT as f64;
    x[ix::SESS] = b.sess_code;
    // Prior 30 min (120 slots before the recent 12) per 3 minutes, for the reasons.
    let prior_span = ((k - 12) - (k - 131).max(0) + 1).max(0) as f64 / 12.0;
    Some(Features {
        symbol: s.symbol.clone(),
        at_ns: b.at_ns,
        slot: k,
        price,
        session: b.session,
        prev_close,
        x,
        ret30: ln_ratio(price, best[4].1),
        bar_volume,
        bar_trades,
        vol3: v12,
        vol3_prior: if prior_span > 0.0 {
            pv12 / prior_span
        } else {
            0.0
        },
        day_high: s.day_high,
        vwap,
        minutes_in_session: b.minutes_in_session,
    })
}

/// What the "running" entry ([`super::running`]) reads at a barrier: one pass over the ring.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct RunInputs {
    /// The barrier bar's close (it traded: no entry on a bar that did not).
    pub(crate) price: f64,
    /// The barrier bar's high made a new high for the session (pre-market, regular or
    /// after-hours) on 15 s bars; the session's first bar counts.
    pub(crate) fresh_high: bool,
    /// The barrier bar's high is above every bar of the 60 minutes before it (a breakout out of
    /// the last hour's range, even under an earlier session high).
    pub(crate) fresh_high60: bool,
    /// The lowest low of the last 60 minutes (240 slots), the barrier bar included.
    pub(crate) base_low: f64,
    /// Dollars over the last 5 minutes (20 slots) and volume over the last 30 (120 slots).
    pub(crate) dollars5: f64,
    pub(crate) vol30: f64,
}

/// The [`RunInputs`] at barrier `b` (call [`settle`] first), or `None` when the barrier's own bar
/// did not trade.
pub(crate) fn run_inputs(s: &Sym, b: &Barrier) -> Option<RunInputs> {
    let ring = s.ring.as_ref()?;
    if s.last_traded != b.key {
        return None;
    }
    let bar = s.bar(b.key)?;
    let price = f64::from(bar.close);
    let high = f64::from(bar.high);
    let fresh_high = s.sess_high_prev.is_nan() || high > s.sess_high_prev;
    let base = key_of(b.ord, 0);
    let (mut lo, mut d5, mut v30, mut hi_before) = (f64::NAN, 0.0, 0.0, f64::NAN);
    for x in ring.iter() {
        if x.key == 0 || x.key > b.key || x.key < base {
            continue;
        }
        let d = b.key - x.key;
        if d < 20 {
            d5 += f64::from(x.dollars);
        }
        if d < 120 {
            v30 += f64::from(x.vol);
        }
        if d < 240 && x.has_ohlc {
            let l = f64::from(x.low);
            lo = if lo.is_nan() { l } else { lo.min(l) };
            if d > 0 {
                let h = f64::from(x.high);
                hi_before = if hi_before.is_nan() {
                    h
                } else {
                    hi_before.max(h)
                };
            }
        }
    }
    Some(RunInputs {
        price,
        fresh_high,
        fresh_high60: hi_before.is_nan() || high > hi_before,
        base_low: lo,
        dollars5: d5,
        vol30: v30,
    })
}

/// What the Runners momentum state ([`super::track`]) reads at a barrier, from the same ring and
/// running totals as the features, so it is causal the same way (slots `<= through` only).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Window {
    /// The last OHLC close at or before the barrier.
    pub(crate) price: f64,
    /// The day's VWAP from 04:00 (`NaN` before any volume).
    pub(crate) vwap: f64,
    /// The barrier slot's own bar: traded, its high and volume.
    pub(crate) bar_traded: bool,
    pub(crate) bar_high: f64,
    pub(crate) bar_vol: f64,
    /// Volume over the last 12 slots (3 minutes), and the average per slot over the 120 slots
    /// before them (the prior 30 minutes; 0 when nothing traded then).
    pub(crate) vol12: f64,
    pub(crate) pace_slot: f64,
    /// The lowest low of the last three rolling minutes: slots `d` in 0..4, 4..8 and 8..12 back
    /// (`NaN` for a minute without an OHLC print).
    pub(crate) min_lows: [f64; 3],
    /// The close a minute (4 slots) or more back: the traded slot nearest to 4 back.
    pub(crate) close_1m: f64,
}

/// The [`Window`] of `s` at slot key `through` (call [`settle`] first), or `None` when the symbol
/// has no ring or has not traded today by then.
pub(crate) fn window(s: &Sym, through: u32, base: u32) -> Option<Window> {
    let ring = s.ring.as_ref()?;
    if s.last_traded == 0 || s.last_traded > through {
        return None;
    }
    let price = f64::from(s.bar(s.last_traded)?.close);
    let vwap = if s.cum_vol > 0.0 {
        s.cum_dollars / s.cum_vol
    } else {
        f64::NAN
    };
    let mut w = Window {
        price,
        vwap,
        bar_traded: false,
        bar_high: f64::NAN,
        bar_vol: 0.0,
        vol12: 0.0,
        pace_slot: 0.0,
        min_lows: [f64::NAN; 3],
        close_1m: f64::NAN,
    };
    let mut prior = 0.0;
    let mut best_d = u32::MAX;
    for bar in ring.iter() {
        if bar.key == 0 || bar.key > through || bar.key < base {
            continue;
        }
        let d = through - bar.key;
        if d as usize >= RING {
            continue;
        }
        let vol = f64::from(bar.vol);
        if d == 0 {
            w.bar_vol = vol;
        }
        if d < 12 {
            w.vol12 += vol;
        } else if d <= 131 {
            prior += vol;
        }
        if bar.has_ohlc {
            if d == 0 {
                w.bar_traded = true;
                w.bar_high = f64::from(bar.high);
            }
            if d < 12 {
                let m = &mut w.min_lows[(d / 4) as usize];
                let l = f64::from(bar.low);
                *m = if m.is_nan() { l } else { m.min(l) };
            }
            if d >= 4 && d < best_d {
                best_d = d;
                w.close_1m = f64::from(bar.close);
            }
        }
    }
    w.pace_slot = prior / 120.0;
    Some(w)
}

/// The 15 s bars of `s` with keys in `(after, through]` that had an OHLC print, oldest first, as
/// `(key, close, high, low)`.
pub(crate) fn bars_between(s: &Sym, after: u32, through: u32) -> Vec<(u32, f64, f64, f64)> {
    let mut out = Vec::new();
    if let Some(ring) = s.ring.as_ref() {
        for bar in ring.iter() {
            if bar.key > after && bar.key <= through && bar.has_ohlc {
                out.push((
                    bar.key,
                    f64::from(bar.close),
                    f64::from(bar.high),
                    f64::from(bar.low),
                ));
            }
        }
    }
    out.sort_by_key(|b| b.0);
    out
}

/// Original source prices alongside the frozen model tape. No reverse conversion from f32.
pub(crate) fn exact_close(s: &Sym, through: u32) -> Option<i64> {
    let b = s
        .ring
        .as_ref()?
        .iter()
        .filter(|b| b.has_ohlc && b.key <= through)
        .max_by_key(|b| b.key)?;
    (b.exact_close > 0).then_some(b.exact_close)
}

#[cfg(test)]
pub(crate) fn exact_bar_close(s: &Sym, key: u32) -> Option<i64> {
    s.bar(key)
        .filter(|b| b.has_ohlc)
        .map(|b| b.exact_close)
        .filter(|p| *p > 0)
}

pub(crate) fn exact_highs(s: &Sym, after: u32, through: u32) -> Vec<(u32, i64)> {
    let mut bars: Vec<_> = s
        .ring
        .iter()
        .flat_map(|r| r.iter())
        .filter(|b| b.has_ohlc && b.key > after && b.key <= through && b.exact_high > 0)
        .map(|b| (b.key, b.exact_high))
        .collect();
    bars.sort_by_key(|b| b.0);
    bars
}

/// The actual source head, rather than the (possibly clock-capped) evaluation barrier,
/// determines which gap bars could already have been overwritten in the ring.
pub(crate) fn retains_exact_after(s: &Sym, after: u32) -> bool {
    s.newest.saturating_sub(after) < RING as u32
}

/// The canonical ROCKET mark test at bar `j` (`reports/rockets/definition.json`): `L` = the min
/// close over traded bars `i < j` ending within 15 minutes before `j`'s end, the start the LATEST
/// bar at `L`; a mark when `close_j >= 1.30 L`, `L` in the band and the start..=mark bars carry
/// at least 25 trades and $25,000. Returns `(start key, L)`.
pub(crate) fn mark_at(s: &Sym, j: u32) -> Option<(u32, f64)> {
    let ring = s.ring.as_ref()?;
    let bj = s.bar(j)?;
    if !bj.has_ohlc {
        return None;
    }
    let mut low: Option<(u32, f64)> = None;
    for bar in ring.iter() {
        if bar.has_ohlc && bar.key < j && bar.key + 60 > j && bar.key != 0 {
            let c = f64::from(bar.close);
            low = match low {
                Some((k, l)) if c > l || (c == l && bar.key < k) => Some((k, l)),
                _ => Some((bar.key, c)),
            };
        }
    }
    let (start, l) = low?;
    let lm = (l * 1e6).round() as i64;
    if f64::from(bj.close) < 1.30 * l - 1e-6 || !(BAND_MICROS.0..=BAND_MICROS.1).contains(&lm) {
        return None;
    }
    let (mut trades, mut dollars) = (0.0, 0.0);
    for bar in ring.iter() {
        if bar.key >= start && bar.key <= j && bar.key != 0 {
            trades += f64::from(bar.trades);
            dollars += f64::from(bar.dollars);
        }
    }
    (trades >= 25.0 && dollars >= 25_000.0).then_some((start, l))
}

impl Tape {
    pub(crate) fn sym(&self, symbol: &str) -> Option<&Sym> {
        self.index.get(symbol).map(|&i| &self.syms[i])
    }
}

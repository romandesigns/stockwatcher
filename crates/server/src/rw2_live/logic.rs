//! rw2-vwapr-1 and rw2-level-1, the detectors themselves: std only, no server types, so the
//! parity harness (`C:/wavystack/outputs/stockwatcher-research-v2/50-ship/rw2/`) compiles THIS
//! FILE unchanged (`#[path]`) and feeds it the research's own bars, baselines and facts.
//! Everything that decides whether either fires lives here; `super` only builds the bars from
//! live prints, loads the day's facts and baselines, schedules the decisions on the clock and
//! turns a [`Fire`] into a notice.
//!
//! Both are rework-2 finalists of the v2 research (`FINALISTS-v2c.md`, confirmed on Train), each
//! a NEW stream added to production (a union), never a replacement. The research code is quoted
//! per step so a reader can check it line by line.
//!
//! # rw2-vwapr-1 · VWAP flush and reclaim, held 30 s, in play (S3-hold30-G0.1)
//!
//! `research/v2-rw2-vwapr` 736a8bee: `research/v2/rw2vwapr/rw2core.py` (`candidates`,
//! `hold_row`, `session_part`), `score_rw2.py` (`select`) and x-vwapr's `vwcore.py`
//! (`seg_arrays`, `_reclaims`, `in_play`), config `{F 0.02, hold 2 slots, R 2, G 0.10, noAH,
//! per seg}` (finalist file sha256 `07ec164a…60f5`, config sha256 `b14f788f…7d2f87`). Train: 18.5
//! a day at 21.95 % GOOD, union Δ precision +0.21 pp [LB +0.12], Δ recall +0.85 pp.
//!
//! The day has two VWAP segments: **pre** (04:00–09:30, slot < 1320) and **regular + after**
//! (re-anchored at 09:30). Per segment, over the bars in slot order:
//!
//! - **VWAP** = the running `f64` sum of the segment's bar dollars / the running `f64` sum of
//!   its shares × 1e6 (micro-dollars; NaN before the segment's first share);
//! - **above** = the bar has an OHLC print and its close > VWAP;
//! - **flush** = the bar has an OHLC print, its low ≤ VWAP × (1 − 0.02), and some EARLIER bar of
//!   the segment closed above VWAP within the last 40 slots (10 minutes, `ABOVE_LOOKBACK`);
//! - **reclaim** j = the first above bar after a flush; each reclaim takes the LATEST flush i
//!   before it (`_reclaims`), and needs slot_j − slot_i ≤ 60 (15 minutes, `4 × W`);
//! - the reclaim counts when its 5-minute dollar **RVOL** ≥ 2 (trail / time-of-day baseline, as
//!   x-gap-1's: NaN never passes, a zero baseline always does) and the trailing 5-minute dollars
//!   are **rising** (trail at j > trail at i);
//! - **hold 30 s**: every printed bar in (slot_j, slot_j + 2] of the same segment closes above
//!   VWAP and at least one prints; k = the last printed one (`hold_row`);
//! - at k: close in **$0.10–$25**, dollars since 04:00 ≥ **$250,000**, and the segment's high so
//!   far ≥ C1 × **1.10** (`in_play`, C1 as `c1 * 1e6` in `f64`, NOT rounded);
//! - decided at the end of slot_j + 2 + 50 ms, at k's close, only **pre-market or regular**
//!   (`session_part` of that instant; after-hours never fires and never uses up the segment);
//! - **once per segment**: the first that passes.
//!
//! # rw2-level-1 · 20-session-high break by a +10 % gainer, RVOL 20×, held 60 s (C08)
//!
//! `research/v2-rw2-level` ac14836a: `research/v2/rw2level/rw2core.py` (`fire`) and x-level's
//! `xlcore.py` (`day_arrays`, `cond_base`) and `xl_daily.py` (`levels`), config `{gain 0.10, rvol
//! 20, hold 60 s, HI20, $100k, all sessions}` (finalist file sha256 `d48ad288…3b5e`, config
//! sha256 `bd0fdadd…5773c1`). Train: 8.8 a day at 21.4 % GOOD, union Δ precision +0.074 pp [LB
//! +0.014], Δ recall +0.61 pp. Once per symbol-day, the first bar of 04:00–20:00 such that:
//!
//! 1. it has an OHLC print, its close is in $0.10–$25, dollars since 04:00 ≥ **$100,000**, the
//!    5-minute dollar RVOL ≥ **20** and close ≥ 1.10 × C1 (`c * 1.0 >= (1 + 0.10) * c1_u`, C1
//!    as integer micro-dollars rounded half to even);
//! 2. it **crosses** HI20: close > HI20 × 1e6 and the previous printed close of the day ≤ it (0
//!    when there is none);
//! 3. the **hold**: every printed close in (slot, slot + 4] stays > HI20, and slot + 4 itself has
//!    an OHLC print in band (and is ≤ 19:59:45). A failed hold lets a later cross try again.
//!
//! It fires at the end of slot + 4 + 50 ms, at that bar's close.
//!
//! **HI20** ([`hi20`]) is the highest of the prior-session highs that the 20 latest daily
//! columns hold, each the high of the session before that column's day (`h1`) put on today's
//! share basis; a column counts only when its symbol was in the archive that day, and at least
//! 18 of the 20 must count (`WINS["HI20"] = (20, 18)`).
//!
//! # Time-of-day baseline and the trailing window
//!
//! Both read the same baseline as x-gap-1 (`crate::gap_ctx::logic::baselines`: `super` hands a
//! per-bucket value, NaN = none) and the same trailing sum: dollars of the bars with slot in
//! (s − 20, s], as the DIFFERENCE of the running sums (`cum[k] − cs[j]`).

pub const SEC: i64 = 1_000_000_000;
pub const MIN: i64 = 60 * SEC;
/// The 15 s bar grid from 04:00 ET.
pub const SLOT: i64 = 15 * SEC;
/// Slots from 04:00 to 20:00 ET.
pub const NSLOT: i64 = 3840;
/// 09:30 ET: the VWAP re-anchor (`vwcore.OPEN_SLOT`).
pub const OPEN_SLOT: i64 = 1320;
/// Decided this long after the bar ends (`DECIDE_NS`).
pub const DECIDE_NS: i64 = 50_000_000;
pub const BAND_LO_U: i64 = 100_000;
pub const BAND_HI_U: i64 = 25_000_000;
/// The trailing dollars window, in slots: (s − 20, s].
pub const TRAIL_SLOTS: i64 = 20;
/// 5-minute buckets of the time-of-day baseline.
pub const BUCKET_SLOTS: i64 = 20;
pub const NBUCKETS: usize = 192;

/// rw2-vwapr-1 (`S3-hold30-G0.1`).
pub const VWAPR_F: f64 = 0.02;
pub const VWAPR_HOLD_SLOTS: i64 = 2;
pub const VWAPR_RVOL: f64 = 2.0;
pub const VWAPR_GAIN: f64 = 0.10;
/// `vwcore.D_MIN`: dollars since 04:00 at the decision bar.
pub const VWAPR_DOLLARS: f64 = 250_000.0;
/// `vwcore.ABOVE_LOOKBACK`: a close above VWAP within this many slots before the flush bar.
pub const VWAPR_ABOVE_LOOKBACK: i64 = 40;
/// `rw2core.W_MIN` × 4: the flush is at most this many slots before its reclaim.
pub const VWAPR_MAX_PAIR_SLOTS: i64 = 60;

/// rw2-level-1 (`C08-g10R20H60-all`).
pub const LEVEL_GAIN: f64 = 0.10;
pub const LEVEL_RVOL: f64 = 20.0;
pub const LEVEL_HOLD_S: i64 = 60;
pub const LEVEL_DOLLARS: f64 = 100_000.0;
/// `xlcore.LAST_SLOT`: the hold's last slot must be at most this.
pub const LEVEL_LAST_SLOT: i64 = 3839;
/// HI20: 20 daily columns, at least 18 present.
pub const HI20_COLUMNS: usize = 20;
pub const HI20_NEED: usize = 18;
/// The research archive's membership band on a day's 15 s closes (`build_cache.py`: "at least
/// one 15 s bar close in $0.10-25"), as `super` approximates it from a daily bar.
pub const MEMBER_LO: f64 = 0.10;
pub const MEMBER_HI: f64 = 25.0;

/// The finalists' ids and research configurations, carried on every fire and ledger line.
pub const VWAPR_ID: &str = "rw2-vwapr-1";
pub const LEVEL_ID: &str = "rw2-level-1";
pub const VWAPR_CONFIG: &str = "rw2vwapr-S3-hold30-G0.1";
pub const LEVEL_CONFIG: &str = "rw2level-C08-g10R20H60-all";
/// `detector_version` in the ledger: the research code commit and the finalist file's sha256
/// prefix (`45-rework2/finalists/<id>.json`).
pub const VWAPR_VERSION: &str = "736a8bee/07ec164a";
pub const LEVEL_VERSION: &str = "ac14836a/d48ad288";

/// `int(round(x * 1e6))`: Python rounds half to even.
pub fn micros_half_even(x: f64) -> i64 {
    (x * 1e6).round_ties_even() as i64
}

pub fn in_band(c: i64) -> bool {
    (BAND_LO_U..=BAND_HI_U).contains(&c)
}

/// A bar's dollars as the research cache stores and the loader rebuilds them: the VWAP kept as
/// an offset from the close in $0.0001 (half to even), dollars back as `(c + 100 × dvw) / 1e6 ×
/// vol`. The same function as `crate::gap_ctx::logic::cache_dollars` (a test checks they agree),
/// repeated so this file compiles alone.
pub fn cache_dollars(c: i64, vol: u64, dollars: f32) -> f64 {
    if vol == 0 {
        return 0.0;
    }
    let vw = f64::from(dollars) / vol as f64 * 1e6;
    let dvw = ((vw - c as f64) / 100.0).round_ties_even() as i32;
    let vw_u = c + 100 * i64::from(dvw);
    vw_u as f64 / 1e6 * vol as f64
}

/// `session_part` / `good_label.session_of` of a decision instant `t`: 0 pre (< 09:30),
/// 1 regular (< 16:00), 2 after.
pub fn session_part(t: i64, t0: i64) -> u8 {
    let r = t - t0;
    if r < 330 * MIN {
        0
    } else if r < 720 * MIN {
        1
    } else {
        2
    }
}

/// One 15 s bar as the detectors read it (the cache's `bars15` row).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bar {
    /// 15 s slot since 04:00 ET.
    pub slot: i64,
    /// Close, high and low in micro-dollars; all 0 = no OHLC print in the slot.
    pub c: i64,
    pub h: i64,
    pub l: i64,
    /// Volume-counted shares.
    pub vol: u64,
    /// The bar's dollars ([`cache_dollars`]).
    pub dollars: f64,
}

/// One print with the chart engine's counting verdict.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Print {
    /// Execution time, epoch ns.
    pub ns: i64,
    /// Micro-dollars.
    pub px: i64,
    pub size: u64,
    pub ohlc: bool,
    pub vol: bool,
}

/// One 15 s bar being built from prints the way the research store builds it (the same rules as
/// `crate::gap_ctx::logic::BarAcc`, plus the low): high, low and close from OHLC-counted prints
/// in integer micro-dollars (the close is the print with the latest execution time, the later
/// arrival on a tie); shares and dollars from volume-counted prints, the dollars summed in `f32`
/// as `px / 1e6 × size` per print in arrival order.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct BarAcc {
    pub slot: i64,
    pub h: i64,
    pub l: i64,
    pub c: i64,
    /// Execution and arrival time of the close print (epoch ns; arrival 0 = unknown).
    pub c_ns: i64,
    pub c_arrival: i64,
    pub vol: u64,
    pub dollars: f32,
}

impl BarAcc {
    pub fn new(slot: i64) -> Self {
        BarAcc {
            slot,
            c_ns: i64::MIN,
            ..Default::default()
        }
    }

    pub fn add(&mut self, p: &Print, arrival: i64) {
        if p.ohlc {
            // `store::px32`: a price is at least one micro-dollar.
            let x = p.px.clamp(1, i64::from(u32::MAX));
            self.h = self.h.max(x);
            self.l = if self.l == 0 { x } else { self.l.min(x) };
            if p.ns >= self.c_ns {
                self.c = x;
                self.c_ns = p.ns;
                self.c_arrival = arrival;
            }
        }
        if p.vol {
            self.vol += p.size;
            self.dollars += (p.px as f64 / 1e6 * p.size as f64) as f32;
        }
    }

    /// The bar as the detectors read it (dollars rounded as the cache stores them).
    pub fn bar(&self) -> Bar {
        Bar {
            slot: self.slot,
            c: self.c,
            h: self.h,
            l: self.l,
            vol: self.vol,
            dollars: cache_dollars(self.c, self.vol, self.dollars),
        }
    }
}

/// HI20 of one symbol-day from its 20 daily columns, oldest first, the last one being today's
/// (`xl_daily.levels`): each `Some(h1 on today's basis)` when the column counts, `None` when the
/// symbol was not in the archive that day or had no earlier daily bar. The highest, when at
/// least [`HI20_NEED`] count; a shorter history than 20 columns has the missing ones absent.
pub fn hi20(columns: &[Option<f64>]) -> Option<f64> {
    let w = &columns[columns.len().saturating_sub(HI20_COLUMNS)..];
    let present: Vec<f64> = w
        .iter()
        .flatten()
        .copied()
        .filter(|x| x.is_finite())
        .collect();
    if present.len() < HI20_NEED {
        return None;
    }
    present.into_iter().reduce(f64::max)
}

/// One raw daily bar (as traded, not split-adjusted): h, l and c, as `refdata::DailyBars::load`
/// requires them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Daily {
    /// The session, as a day number (any monotone day number; `super` uses days from CE).
    pub day: i32,
    pub h: f64,
    pub l: f64,
    pub c: f64,
}

/// The split factor bringing a price of day `from` to day `to`'s basis: the product, in date
/// order, of every split `(effective day, old / new)` with its day in `(from, to]`
/// (`DailyBars::factor`, as `crate::gap_ctx::logic::factor`).
pub fn factor(splits: &[(i32, f64)], from: i32, to: i32) -> f64 {
    splits
        .iter()
        .filter(|(d, _)| *d > from && *d <= to)
        .map(|(_, f)| *f)
        .product()
}

/// Column `j`'s value on day `to`'s basis: `h1` of day j (the high of the last daily bar before
/// j, oldest first in `bars`) restated to `to`. The research restates it to j's basis and then
/// carries it session by session with its own basis factors (`xl_daily.levels`: the ratio of
/// the next h1 to the day's 15 s high, snapped to 1 within ±3 %); live applies the split ledger
/// once. Without a split in the window both are the same high.
pub fn h1_on(bars: &[Daily], splits: &[(i32, f64)], j: i32, to: i32) -> Option<f64> {
    let k = bars.partition_point(|b| b.day < j);
    let last = bars[..k].last()?;
    Some(last.h * factor(splits, last.day, to))
}

/// Whether the symbol was in the research archive on day `j`, from its daily bar: the archive
/// kept a symbol on a day with at least one 15 s close in $0.10–$25 (`build_cache.py`), which a
/// daily range [l, h] meeting that band approximates (the parity report counts the misses).
pub fn member(bars: &[Daily], j: i32) -> bool {
    let k = bars.partition_point(|b| b.day < j);
    bars.get(k)
        .is_some_and(|b| b.day == j && b.l <= MEMBER_HI && b.h >= MEMBER_LO)
}

/// HI20 of day `today` ([`hi20`]): the 19 sessions before it (`prior`, oldest first, the
/// trading days; fewer at the start of a history) plus today's own column, which always counts
/// (a symbol that can fire is in today's archive by its own band). `present(j)` says whether a
/// prior column counts ([`member`] live).
pub fn hi20_of(
    bars: &[Daily],
    splits: &[(i32, f64)],
    prior: &[i32],
    today: i32,
    present: impl Fn(i32) -> bool,
) -> Option<f64> {
    let p = &prior[prior.len().saturating_sub(HI20_COLUMNS - 1)..];
    let mut cols: Vec<Option<f64>> = p
        .iter()
        .map(|&j| {
            if present(j) {
                h1_on(bars, splits, j, today)
            } else {
                None
            }
        })
        .collect();
    cols.push(h1_on(bars, splits, today, today));
    hi20(&cols)
}

/// What a fire carries (the notice's facts, the ledger's and the parity harness's fields).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Fire {
    /// The decision slot (vwapr: reclaim + 2; level: the hold's last slot).
    pub slot: i64,
    /// Decision instant: 04:00 + (slot + 1) × 15 s + 50 ms, epoch ns.
    pub t: i64,
    /// The close it fires at (vwapr: the hold's last printed bar; level: the decision bar).
    pub close_u: i64,
    /// The slot whose close is `close_u`.
    pub close_slot: i64,
    /// vwapr: the segment high / C1 − 1. level: close / C1 − 1.
    pub gain: f64,
    pub cum: f64,
    /// The trailing 5-minute dollars and their RVOL at the trigger bar (vwapr: the reclaim;
    /// level: the cross).
    pub trail: f64,
    pub rvol: f64,
    /// vwapr: the segment VWAP at the reclaim (micro-dollars). level: HI20 (micro-dollars).
    pub level: f64,
    /// The trigger bar's slot (vwapr: the reclaim; level: the cross).
    pub trigger_slot: i64,
    /// vwapr only: the flush bar's slot and how deep its low went under VWAP (fraction).
    pub flush_slot: i64,
    pub flush_depth: f64,
}

/// The day's inputs of one symbol, fixed at 04:00.
#[derive(Clone, Debug)]
pub struct SymDay {
    /// C1 in dollars (vwapr reads `c1 * 1e6` unrounded), when positive and finite.
    pub c1: Option<f64>,
    /// HI20 × 1e6 (`hi20 * 1e6`, unrounded), when the symbol has one: level eligible.
    pub hi20_u: Option<f64>,
    /// The time-of-day baseline per 5-minute bucket (NaN = none).
    pub base: Vec<f64>,
}

impl Default for SymDay {
    fn default() -> Self {
        SymDay {
            c1: None,
            hi20_u: None,
            base: vec![f64::NAN; NBUCKETS],
        }
    }
}

impl SymDay {
    fn base_of(&self, slot: i64) -> f64 {
        usize::try_from(slot / BUCKET_SLOTS)
            .ok()
            .and_then(|b| self.base.get(b).copied())
            .unwrap_or(f64::NAN)
    }
}

/// `rv`: trail / base, NaN without a baseline, +inf with a zero one.
fn rvol(trail: f64, base: f64) -> f64 {
    if base.is_nan() {
        f64::NAN
    } else if base > 0.0 {
        trail / base
    } else {
        f64::INFINITY
    }
}

/// A reclaim waiting for its 30 s hold.
#[derive(Clone, Copy, Debug)]
struct Hold {
    seg: u8,
    slot_j: i64,
    /// The hold's last slot (slot_j + 2).
    end: i64,
    rv: f64,
    trail: f64,
    vwap: f64,
    flush_slot: i64,
    flush_depth: f64,
    /// The last printed bar in (j, end] so far: slot, close, dollars since 04:00, segment high.
    k: Option<(i64, i64, f64, i64)>,
}

/// One VWAP segment's running state.
#[derive(Clone, Copy, Debug, Default)]
struct Seg {
    cd: f64,
    cv: f64,
    shigh: i64,
    /// The last bar closing above VWAP (slot).
    last_above: Option<i64>,
    /// The latest flush since the last above bar: (slot, trail there, depth under VWAP).
    flush: Option<(i64, f64, f64)>,
    fired: bool,
}

/// One symbol's running state over the day's bars, fed in slot order ([`Sym::push`]) with the
/// clock's progress in between ([`Sym::advance`]).
#[derive(Clone, Debug, Default)]
pub struct Sym {
    cum: f64,
    window: std::collections::VecDeque<(i64, f64)>,
    before: f64,
    last_slot: i64,
    segs: [Seg; 2],
    holds: Vec<Hold>,
    /// level: the previous printed close (0: none), the pending cross (its slot, the hold's
    /// last slot, trail, rvol), and whether it fired today.
    prevc: i64,
    cross: Option<(i64, i64, f64, f64)>,
    pub level_fired: bool,
}

impl Sym {
    pub fn new() -> Self {
        Sym {
            last_slot: -1,
            ..Default::default()
        }
    }

    /// The next decision instant this symbol waits for without a bar: the earliest pending
    /// vwapr hold's end (as a slot; decided at its end + 50 ms), if any.
    pub fn next_due_slot(&self) -> Option<i64> {
        self.holds.iter().map(|h| h.end).min()
    }

    /// The clock passed the end of slot `done` (+ 50 ms): every vwapr hold ending at or before
    /// it is decided. Call before pushing a bar of a later slot and whenever the clock moves.
    pub fn advance(&mut self, done: i64, day: &SymDay, t0: i64) -> Vec<Fire> {
        let mut out = Vec::new();
        let mut i = 0;
        while i < self.holds.len() {
            if self.holds[i].end > done {
                i += 1;
                continue;
            }
            let h = self.holds.remove(i);
            if self.segs[usize::from(h.seg)].fired {
                continue;
            }
            let Some((ks, kc, kcum, kshigh)) = h.k else {
                continue;
            };
            let Some(c1) = day.c1 else { continue };
            let c1_u = c1 * 1e6;
            let t = t0 + (h.end + 1) * SLOT + DECIDE_NS;
            if in_band(kc)
                && kcum >= VWAPR_DOLLARS
                && kshigh as f64 >= c1_u * (1.0 + VWAPR_GAIN)
                && session_part(t, t0) <= 1
            {
                self.segs[usize::from(h.seg)].fired = true;
                out.push(Fire {
                    slot: h.end,
                    t,
                    close_u: kc,
                    close_slot: ks,
                    gain: kshigh as f64 / c1_u - 1.0,
                    cum: kcum,
                    trail: h.trail,
                    rvol: h.rv,
                    level: h.vwap,
                    trigger_slot: h.slot_j,
                    flush_slot: h.flush_slot,
                    flush_depth: h.flush_depth,
                });
            }
        }
        // A fired segment needs none of its other holds.
        let segs = self.segs;
        self.holds.retain(|h| !segs[usize::from(h.seg)].fired);
        out
    }

    /// The next bar (slot strictly after the last one): the vwapr fires decided up to it (holds
    /// that ended before this slot) and the level fire, if any. A bar at or past 20:00 is
    /// ignored.
    pub fn push(&mut self, b: &Bar, day: &SymDay, t0: i64) -> (Vec<Fire>, Option<Fire>) {
        if b.slot >= NSLOT || b.slot <= self.last_slot {
            return (Vec::new(), None);
        }
        let mut vw_fires = self.advance(b.slot - 1, day, t0);
        self.last_slot = b.slot;
        // Day-wide running dollars and the trailing window (both detectors).
        self.cum += b.dollars;
        self.window.push_back((b.slot, self.cum));
        while let Some(&(s, c)) = self.window.front() {
            if s >= b.slot - (TRAIL_SLOTS - 1) {
                break;
            }
            self.before = c;
            self.window.pop_front();
        }
        let trail = self.cum - self.before;
        let rv = rvol(trail, day.base_of(b.slot));
        let has = b.c > 0;
        self.vwapr_bar(b, has, trail, rv);
        // Holds that end exactly here are decided now (their last slot has been seen).
        vw_fires.extend(self.advance(b.slot, day, t0));
        let lv = self.level_bar(b, has, trail, rv, day, t0);
        (vw_fires, lv)
    }

    fn vwapr_bar(&mut self, b: &Bar, has: bool, trail: f64, rv: f64) {
        let si = usize::from(b.slot >= OPEN_SLOT);
        let seg = &mut self.segs[si];
        seg.cd += b.dollars;
        seg.cv += b.vol as f64;
        let vw = if seg.cv > 0.0 {
            seg.cd / seg.cv * 1e6
        } else {
            f64::NAN
        };
        if has {
            seg.shigh = seg.shigh.max(b.h);
        }
        let above = has && !vw.is_nan() && b.c as f64 > vw;
        // The holds of this segment see the bar (a printed bar not above fails them).
        let (cum, shigh) = (self.cum, seg.shigh);
        self.holds.retain_mut(|h| {
            if usize::from(h.seg) != si || b.slot > h.end || !has {
                return true;
            }
            if !above {
                return false;
            }
            h.k = Some((b.slot, b.c, cum, shigh));
            true
        });
        // A reclaim of the latest flush since the last above bar. The flush is consumed by the
        // reclaim whether or not it counts (`take` runs once `above` holds): flushes before an
        // above bar map to it and to no later one (`_reclaims`).
        if above
            && let Some((fs, ftrail, fdepth)) = seg.flush.take()
            && b.slot - fs <= VWAPR_MAX_PAIR_SLOTS
            && !rv.is_nan()
            && rv >= VWAPR_RVOL
            && trail > ftrail
            && !seg.fired
        {
            self.holds.push(Hold {
                seg: si as u8,
                slot_j: b.slot,
                end: b.slot + VWAPR_HOLD_SLOTS,
                rv,
                trail,
                vwap: vw,
                flush_slot: fs,
                flush_depth: fdepth,
                k: None,
            });
        }
        // This bar as a flush (against the last above bar BEFORE it).
        if has
            && !vw.is_nan()
            && (b.l as f64) <= vw * (1.0 - VWAPR_F)
            && seg
                .last_above
                .is_some_and(|a| b.slot - a <= VWAPR_ABOVE_LOOKBACK)
        {
            seg.flush = Some((b.slot, trail, 1.0 - b.l as f64 / vw));
        }
        if above {
            seg.last_above = Some(b.slot);
        }
        // A pre-market hold reaching past 09:30 reads only its own segment's bars (`same seg` in
        // `hold_row`: the bars from 09:30 on are skipped above) but is still decided at the end
        // of slot_j + 2, as `dslot` is.
    }

    fn level_bar(
        &mut self,
        b: &Bar,
        has: bool,
        trail: f64,
        rv: f64,
        day: &SymDay,
        t0: i64,
    ) -> Option<Fire> {
        let mut fire = None;
        if !self.level_fired
            && let Some(l) = day.hi20_u
            && let Some((xs, end, xtrail, xrv)) = self.cross
        {
            if b.slot > end || (has && b.c as f64 <= l) {
                self.cross = None;
            } else if b.slot == end {
                self.cross = None;
                if has && in_band(b.c) {
                    self.level_fired = true;
                    let c1_u = day.c1.map_or(0, micros_half_even);
                    fire = Some(Fire {
                        slot: end,
                        t: t0 + (end + 1) * SLOT + DECIDE_NS,
                        close_u: b.c,
                        close_slot: end,
                        gain: b.c as f64 / c1_u.max(1) as f64 - 1.0,
                        cum: self.cum,
                        trail: xtrail,
                        rvol: xrv,
                        level: l,
                        trigger_slot: xs,
                        flush_slot: -1,
                        flush_depth: f64::NAN,
                    });
                }
            }
        }
        if !self.level_fired
            && self.cross.is_none()
            && let (Some(l), Some(c1)) = (day.hi20_u, day.c1)
            && l > 0.0
            && has
            && in_band(b.c)
            && self.cum >= LEVEL_DOLLARS
            && !rv.is_nan()
            && rv >= LEVEL_RVOL
            && b.c as f64 >= (1.0 + LEVEL_GAIN) * micros_half_even(c1) as f64
            && b.c as f64 > l
            && self.prevc as f64 <= l
        {
            let end = b.slot + LEVEL_HOLD_S / 15;
            if end <= LEVEL_LAST_SLOT {
                self.cross = Some((b.slot, end, trail, rv));
            }
        }
        if has {
            self.prevc = b.c;
        }
        fire
    }

    /// A late print's dollars and shares for an already pushed bar at `slot`: added to the
    /// running sums (and to the trailing window when the bar is still in it), so later bars see
    /// what the research bar had. Its prices stay out (the bar's close / high / low were
    /// decided). The order of the additions differs from the research's, in the last bits only.
    pub fn add_late(&mut self, slot: i64, dollars: f64, vol: u64) {
        self.cum += dollars;
        if self.window.front().is_none_or(|(s, _)| *s > slot) {
            self.before += dollars;
        }
        for (s, c) in self.window.iter_mut() {
            if *s >= slot {
                *c += dollars;
            }
        }
        let seg = &mut self.segs[usize::from(slot >= OPEN_SLOT)];
        seg.cd += dollars;
        seg.cv += vol as f64;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const T0: i64 = 1_718_092_800 * SEC;

    fn day(c1: f64, hi20: Option<f64>, base: f64) -> SymDay {
        SymDay {
            c1: Some(c1),
            hi20_u: hi20.map(|h| h * 1e6),
            base: vec![base; NBUCKETS],
        }
    }

    fn bar(slot: i64, c: i64, l: i64, vol: u64) -> Bar {
        Bar {
            slot,
            c,
            h: c.max(l),
            l,
            vol,
            dollars: c as f64 / 1e6 * vol as f64,
        }
    }

    /// Runs a symbol's bars and the clock to 20:00; every fire.
    fn run(d: &SymDay, bars: &[Bar]) -> (Vec<Fire>, Vec<Fire>) {
        let mut s = Sym::new();
        let (mut v, mut l) = (Vec::new(), Vec::new());
        for b in bars {
            let (a, x) = s.push(b, d, T0);
            v.extend(a);
            l.extend(x);
        }
        v.extend(s.advance(NSLOT, d, T0));
        (v, l)
    }

    /// A regular-session flush and reclaim at $2 on a C1 of $1.50: above, flush 3 % under
    /// VWAP, reclaim with rising volume, two held bars.
    fn reclaim_day(hold_close: i64) -> Vec<Bar> {
        let mut v = vec![bar(1400, 2_000_000, 2_000_000, 150_000)];
        v.push(bar(1401, 2_010_000, 2_000_000, 1_000));
        v.push(bar(1402, 1_990_000, 1_930_000, 2_000));
        v.push(bar(1403, 2_050_000, 2_000_000, 20_000));
        v.push(bar(1404, hold_close, hold_close, 1_000));
        v.push(bar(1405, 2_060_000, 2_060_000, 1_000));
        v
    }

    #[test]
    fn vwapr_fires_at_the_end_of_the_30_s_hold_once_per_segment() {
        let d = day(1.5, None, 1_000.0);
        let (v, l) = run(&d, &reclaim_day(2_055_000));
        assert!(l.is_empty());
        assert_eq!(v.len(), 1, "{v:?}");
        let f = v[0];
        assert_eq!((f.trigger_slot, f.slot, f.close_slot), (1403, 1405, 1405));
        assert_eq!(f.t, T0 + 1406 * SLOT + DECIDE_NS);
        assert_eq!(f.close_u, 2_060_000);
        assert_eq!(f.flush_slot, 1402);
    }

    #[test]
    fn vwapr_hold_fails_on_a_close_at_or_under_vwap() {
        let d = day(1.5, None, 1_000.0);
        let (v, _) = run(&d, &reclaim_day(1_900_000));
        assert!(v.is_empty(), "{v:?}");
    }

    #[test]
    fn vwapr_decides_on_the_clock_without_a_bar_at_the_hold_end() {
        let d = day(1.5, None, 1_000.0);
        let mut bars = reclaim_day(2_055_000);
        bars.pop();
        let mut s = Sym::new();
        let mut v = Vec::new();
        for b in &bars {
            v.extend(s.push(b, &d, T0).0);
        }
        assert!(v.is_empty());
        assert_eq!(s.next_due_slot(), Some(1405));
        assert!(s.advance(1404, &d, T0).is_empty());
        let f = s.advance(1405, &d, T0);
        assert_eq!(f.len(), 1);
        assert_eq!(
            (f[0].slot, f[0].close_slot, f[0].close_u),
            (1405, 1404, 2_055_000)
        );
    }

    #[test]
    fn vwapr_needs_in_play_band_dollars_and_no_after_hours() {
        // C1 $1.90: the high $2.06 is only +8 %.
        assert!(
            run(&day(1.9, None, 1_000.0), &reclaim_day(2_055_000))
                .0
                .is_empty()
        );
        // No baseline: RVOL NaN never passes.
        assert!(
            run(&day(1.5, None, f64::NAN), &reclaim_day(2_055_000))
                .0
                .is_empty()
        );
        // The same bars after 16:00.
        let late: Vec<Bar> = reclaim_day(2_055_000)
            .into_iter()
            .map(|b| Bar {
                slot: b.slot + 1600,
                ..b
            })
            .collect();
        assert!(run(&day(1.5, None, 1_000.0), &late).0.is_empty());
    }

    #[test]
    fn level_fires_after_a_60_s_hold_above_hi20() {
        // C1 $1.00, HI20 $1.20; a cross at $1.25 with $150k, held 4 slots.
        let d = day(1.0, Some(1.2), 1_000.0);
        let mut bars = vec![bar(2000, 1_150_000, 1_150_000, 1_000)];
        bars.push(bar(2001, 1_250_000, 1_250_000, 120_000));
        for s in 2002..=2005 {
            bars.push(bar(s, 1_260_000, 1_260_000, 100));
        }
        let (_, l) = run(&d, &bars);
        assert_eq!(l.len(), 1);
        assert_eq!((l[0].trigger_slot, l[0].slot), (2001, 2005));
        assert_eq!(l[0].t, T0 + 2006 * SLOT + DECIDE_NS);
        // A dip to the level fails the hold, and the next cross may fire.
        let mut b2 = bars.clone();
        b2[3] = bar(2003, 1_200_000, 1_200_000, 100);
        b2.truncate(4);
        b2.push(bar(2010, 1_300_000, 1_300_000, 10_000));
        for s in 2011..=2014 {
            b2.push(bar(s, 1_310_000, 1_310_000, 100));
        }
        let (_, l) = run(&d, &b2);
        assert_eq!(l.len(), 1);
        assert_eq!(l[0].trigger_slot, 2010);
        // No print at the hold's last slot: no fire.
        let mut b3 = bars.clone();
        b3.pop();
        assert!(run(&d, &b3).1.is_empty());
        // A close not above the previous printed close's side: equal to the level is no cross.
        let d2 = day(1.0, Some(1.25), 1_000.0);
        assert!(run(&d2, &bars).1.is_empty());
    }

    #[test]
    fn hi20_needs_18_of_the_20_columns() {
        let mut c: Vec<Option<f64>> = (0..20).map(|i| Some(1.0 + f64::from(i) / 100.0)).collect();
        assert_eq!(hi20(&c), Some(1.19));
        c[0] = None;
        c[5] = None;
        assert_eq!(hi20(&c), Some(1.19));
        c[19] = None;
        assert_eq!(hi20(&c), None);
        // Only the last 20 count.
        let mut d = vec![Some(9.0)];
        d.extend((0..20).map(|_| Some(1.0)));
        assert_eq!(hi20(&d), Some(1.0));
    }

    #[test]
    fn bar_acc_tracks_the_low_and_rounds_dollars_like_the_cache() {
        let mut a = BarAcc::new(3);
        for (ns, px) in [(1, 2_000_000), (3, 1_950_000), (2, 2_100_000)] {
            a.add(
                &Print {
                    ns,
                    px,
                    size: 100,
                    ohlc: true,
                    vol: true,
                },
                0,
            );
        }
        let b = a.bar();
        assert_eq!(
            (b.c, b.h, b.l, b.vol),
            (1_950_000, 2_100_000, 1_950_000, 300)
        );
        assert_eq!(cache_dollars(2_000_000, 1_000, 2_000.04), 2_000.0);
        assert_eq!(session_part(T0 + 330 * MIN, T0), 1);
        assert_eq!(session_part(T0 + 330 * MIN - 1, T0), 0);
    }
}

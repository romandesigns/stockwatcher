//! x-gap-1 and x-ctx-1, the detectors themselves: std only, no server types, so the parity
//! harness (`C:/wavystack/outputs/stockwatcher-research-v2/50-ship/gap-ctx/`) compiles THIS FILE
//! unchanged (`#[path]`) and feeds it the research's own bars, baselines and facts. Everything
//! that decides whether either fires lives here; `super` only builds the bars from live prints,
//! loads the day's facts and baselines, and turns a [`Fire`] into a notice.
//!
//! Both are wave-2 finalists of the v2 research (owner-approved ship set of 2026-11-22,
//! `DEPLOY-PLAN.md` last addendum), each a NEW stream added to production (a union), never a
//! replacement. The research code is quoted per step so a reader can check it line by line.
//!
//! # x-gap-1 · pre-market new session high on volume
//!
//! `research/v2-x-gap` a27407ea, `research/v2/xgap/gapcore.py` (`pm_arrays`, `h3_trigger`) and
//! `build_events.py` (config `G0.1:D100000:R3`, finalist file sha256 `45735cfa…68c8`). Train:
//! 12.0 a day at 21.7 % GOOD; Validation Δ recall +1.9 pp, Δ precision +0.20 pp (22 a day).
//! Once per symbol-day, the FIRST pre-market 15 s bar (slot < 09:30 ET) such that:
//!
//! 1. the bar has an OHLC print and its close is in **$0.10–$25** (`in_band`, inclusive);
//! 2. **gap** = close / C1 − 1 ≥ **0.10**, with C1 the prior session's close split-restated to
//!    the day, as integer micro-dollars rounded half to even (`int(round(c1 * 1e6))`), and the
//!    division `close_u / max(c1_u, 1)` in `f64`;
//! 3. **cum** = the running `f64` sum of the bars' dollars from 04:00 through this bar ≥
//!    **$100,000** (`np.cumsum`, sequential, so the same order here);
//! 4. **rvol5** = trail / base ≥ **3**, where trail is the dollars of the bars whose slot is in
//!    (s − 20, s] computed as the DIFFERENCE of the running sums (`cum[i] − cs[j]`, not a fresh
//!    sum: the two differ in the last bits) and base is the time-of-day baseline of the bar's
//!    5-minute bucket ([`Baseline::of_bucket`]): NaN (no baseline) never passes, 0 always
//!    passes (rvol = +inf);
//! 5. a **new session high**: close > the highest high of every earlier bar of the day (bars
//!    without an OHLC print count as 0), strictly.
//!
//! # x-ctx-1 · day 2: yesterday's 30 %+ runner breaks yesterday's high
//!
//! `research/v2-x-ctx` 7429f7c4 (HEAD b19b0955 only adds post-hoc checks),
//! `research/v2/xctx/ctxcore.py` (`sess_arrays`, `trigger`), `build_events.py`, `ctx_daily.py`
//! and `score.py` (`src == "h2"`, `R == 3`, `prev_ran`, `cr >= 0`; finalist file sha256
//! `58b653a1…1dd3`). Train: 4.5 a day at 23.2 % GOOD; Validation Δ recall +1.2 pp, Δ precision
//! +0.12 pp (8 a day). Only on a symbol-day whose facts say **prev_ran** (the prior session's
//! daily high was ≥ 1.3 × its own prior close: `runs365(day) > runs365(prior session)`, see
//! [`facts_of`]) with **h1** > 0 and a **c1** (cr = c1 / h1 ≥ 0 only drops a missing c1). Once
//! per symbol-day, the FIRST bar of 04:00–20:00 ET such that:
//!
//! 1. the bar has an OHLC print and its close is in $0.10–$25;
//! 2. close > **h1** strictly, h1 as integer micro-dollars rounded half to even;
//! 3. cum ≥ $100,000 and rvol5 ≥ 3, exactly as x-gap's 3 and 4 but over every bar of the day.
//!
//! # Decision instant and price
//!
//! Both decide at the bar's end + **50 ms** (`DECIDE_NS`, "production's settle"), at the bar's
//! close. The research bar holds every print by execution time; live decides when the clock
//! passes that instant, so a print reaching the server later is not in the decision (counted by
//! `super`, never silently different).
//!
//! # What the research did that is NOT here (each one implied by the trigger, so no row changes)
//!
//! - x-gap's candidate prefilter (`build_events.py:118-125`): the day's pre-market dollars
//!   (pairwise `np.sum`) ≥ $100k and the day's max gap on the UNROUNDED `c1 * 1e6` ≥ 0.10. A fire
//!   already has cum ≥ $100k and gap ≥ 0.10 at its own bar; the two can only disagree by the
//!   last bits of a sum or the rounding of C1 (the parity report counts it: 0 on the Train days).
//! - x-ctx's prefilter (`build_events.py:93-96`): day dollars ≥ $100k and some close > h1, both
//!   implied by a fire the same way.
//!
//! # Time-of-day baseline (`cache_loader.Cache.tod_baseline` + `good_label.baseline_for`)
//!
//! Per (symbol, 5-minute bucket of 04:00–20:00, 0..191) and k ∈ {5, 20}: the `f64` sum of the
//! bucket's dollars over the k sessions strictly before the day (a session the symbol did not
//! trade the bucket in counts 0), divided by **k** (not by the sessions present), stored as
//! `f32`; `present_k` = sessions among those k where the symbol was in the archive. The
//! choice: present20 ≥ 20 → the 20-session mean; else present5 ≥ 1 → the 5-session mean
//! × 5 / present5; else none. [`baselines`] reproduces it, including one quirk of the research
//! join, on purpose (the stream was validated with it): `present5` is read from the symbol's
//! HIGHEST bucket row of the joined table, and a bucket the symbol traded in the 20-session
//! window but not in the last 5 has present5 filled with 0 there. So a symbol with present20 <
//! 20 whose latest bucket of the day went untraded in the last 5 sessions has no baseline at
//! all (never fires that day).

use std::collections::{HashMap, VecDeque};

pub const SEC: i64 = 1_000_000_000;
pub const MIN: i64 = 60 * SEC;
/// The 15 s bar grid from 04:00 ET (the research cache's and the Runners' slots).
pub const SLOT: i64 = 15 * SEC;
/// Slots from 04:00 to 20:00 ET (`ctxcore.END_SLOT`).
pub const NSLOT: i64 = 3840;
/// 09:30 ET (`gapcore.OPEN_SLOT`): x-gap reads bars with slot < this only.
pub const OPEN_SLOT: i64 = 1320;
/// Decided this long after the bar ends (`DECIDE_NS`).
pub const DECIDE_NS: i64 = 50_000_000;
/// `universe-v1`'s band on the bar's close (micro-dollars, inclusive).
pub const BAND_LO_U: i64 = 100_000;
pub const BAND_HI_U: i64 = 25_000_000;
/// The trailing volume window, in slots: (s − 20, s] (`searchsorted(s, s - 19)`).
pub const TRAIL_SLOTS: i64 = 20;
/// 5-minute buckets of the time-of-day baseline (`TOD_BUCKET_SLOTS`, `N_TOD_BUCKETS`).
pub const BUCKET_SLOTS: i64 = 20;
pub const NBUCKETS: usize = 192;
/// x-gap-1: `G0.1:D100000:R3`.
pub const GAP_MIN: f64 = 0.10;
/// Both: cumulative dollars since 04:00 and the 5-minute dollar RVOL.
pub const DOLLARS_MIN: f64 = 100_000.0;
pub const RVOL_MIN: f64 = 3.0;
/// A daily-high run: high ≥ 1.3 × the previous bar's close restated (`refdata.rs` facts).
pub const RUN_MULT: f64 = 1.3;
/// Daily bars a runs365 count reads: the last 366 bars, 365 consecutive pairs.
pub const RUNS_BARS: usize = 366;

/// The finalists' ids and their research configurations, carried on every fire and ledger line.
pub const XGAP_ID: &str = "x-gap-1";
pub const XCTX_ID: &str = "x-ctx-1";
pub const XGAP_CONFIG: &str = "xgap-H3-G0.1-D100000-R3";
pub const XCTX_CONFIG: &str = "xctx-H2-prev_ran-m0-R3";
/// `detector_version` in the ledger: the research code commit and the finalist file's sha256
/// prefix (`30-w2/finalists/<id>.json`), so a ledger line names exactly what was validated.
pub const XGAP_VERSION: &str = "a27407ea/45735cfa";
pub const XCTX_VERSION: &str = "7429f7c4/58b653a1";

/// `int(round(x * 1e6))`: Python rounds half to even.
pub fn micros_half_even(x: f64) -> i64 {
    (x * 1e6).round_ties_even() as i64
}

pub fn in_band(c: i64) -> bool {
    (BAND_LO_U..=BAND_HI_U).contains(&c)
}

/// A bar's dollars as the research cache stores and the loader rebuilds them
/// (`build_cache.bar_frame` + `cache_loader.decode_bars`): the VWAP is kept as an offset from the
/// close in units of $0.0001 (`dvw = round((dollars / vol × 1e6 − c) / 100)`, half to even), and
/// dollars come back as `(c + 100 × dvw) / 1e6 × vol`. `dollars` is the store's `f32` sum of
/// `px / 1e6 × size` over the bar's volume-counted prints; `c` is 0 without an OHLC print (the
/// offset is then the absolute VWAP). Live applies the same rounding so its bars are the cache's
/// wherever the store's bars are (a symbol the store rebuilt from 1 s rows differs in the last
/// cents: the parity report measures it).
pub fn cache_dollars(c: i64, vol: u64, dollars: f32) -> f64 {
    if vol == 0 {
        return 0.0;
    }
    let vw = f64::from(dollars) / vol as f64 * 1e6;
    let dvw = ((vw - c as f64) / 100.0).round_ties_even() as i32;
    let vw_u = c + 100 * i64::from(dvw);
    vw_u as f64 / 1e6 * vol as f64
}

/// One 15 s bar as the detectors read it (the cache's `bars15` row).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bar {
    /// 15 s slot since 04:00 ET.
    pub slot: i64,
    /// Close and high in micro-dollars; 0 = no OHLC print in the slot.
    pub c: i64,
    pub h: i64,
    /// The bar's dollars ([`cache_dollars`]).
    pub dollars: f64,
}

/// One print with the chart engine's counting verdict (`n1_live::logic::Print`, the store's
/// `tape::Print`).
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

/// One 15 s bar being built from prints the way the research store builds it
/// (`store.rs::build_symbol`, the same rules `n1_live::logic` documents): high and close from
/// OHLC-counted prints in integer micro-dollars (the close is the print with the latest execution
/// time, the later arrival on a tie); shares and dollars from volume-counted prints, the dollars
/// summed in `f32` as `px / 1e6 × size` per print in arrival order.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct BarAcc {
    pub slot: i64,
    pub h: i64,
    pub c: i64,
    /// Execution and arrival time of the close print (epoch ns; arrival 0 = unknown).
    pub c_ns: i64,
    pub c_arrival: i64,
    pub vol: u64,
    pub n: u32,
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
            if p.ns >= self.c_ns {
                self.c = x;
                self.c_ns = p.ns;
                self.c_arrival = arrival;
            }
        }
        if p.vol {
            self.vol += p.size;
            self.n += 1;
            self.dollars += (p.px as f64 / 1e6 * p.size as f64) as f32;
        }
    }

    /// The bar as the detectors read it (dollars rounded as the cache stores them).
    pub fn bar(&self) -> Bar {
        Bar {
            slot: self.slot,
            c: self.c,
            h: self.h,
            dollars: cache_dollars(self.c, self.vol, self.dollars),
        }
    }
}

/// One symbol's time-of-day baseline for the day ([`baselines`]).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Baseline {
    pub present20: u32,
    pub present5: u32,
    /// `(b20_dol, b5_dol)` by bucket; a bucket absent here is `(0, 0)`.
    pub buckets: HashMap<u8, (f32, f32)>,
}

impl Baseline {
    /// `good_label.baseline_for(present20, b20, present5, b5)[0]` for `bucket`; NaN = none.
    pub fn of_bucket(&self, bucket: i64) -> f64 {
        let (v20, v5) = u8::try_from(bucket)
            .ok()
            .and_then(|b| self.buckets.get(&b).copied())
            .unwrap_or((0.0, 0.0));
        if self.present20 >= 20 {
            f64::from(v20)
        } else if self.present5 >= 1 {
            f64::from(v5) * 5.0 / f64::from(self.present5)
        } else {
            f64::NAN
        }
    }

    /// Which branch [`Baseline::of_bucket`] takes: 20, 5, or 0 = none (`base_kind`).
    pub fn kind(&self) -> u8 {
        if self.present20 >= 20 {
            20
        } else if self.present5 >= 1 {
            5
        } else {
            0
        }
    }
}

/// One session's time-of-day aggregates: per symbol, `(bucket, dollars)` of its volume-counted
/// prints (the `f64` sum of its bars' `f32` dollars, stored as `f32`, as `tod_agg.parquet`), and
/// which symbols were in the archive that day.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TodDay {
    pub agg: HashMap<String, Vec<(u8, f32)>>,
    /// Which symbols were in the archive that day. The research archive held a symbol on a day
    /// its close was in $0.05–$30 (finding F3); live writes the same membership
    /// (`super::TOD_PRESENT_LO` / `HI`, the last OHLC print of the day).
    pub present: std::collections::HashSet<String>,
}

/// The baselines of `want` for a day whose prior sessions are `prior` (oldest first; the last
/// 20 are read, as `allv[i - k:i]`). Module docs, "Time-of-day baseline", quirk included.
pub fn baselines<'a>(
    prior: &[TodDay],
    want: impl IntoIterator<Item = &'a str>,
) -> HashMap<String, Baseline> {
    let w20 = &prior[prior.len().saturating_sub(20)..];
    let w5 = &prior[prior.len().saturating_sub(5)..];
    let mut out = HashMap::new();
    for sym in want {
        // Per bucket, the f64 sum over the window's sessions in session order, / k, as f32.
        let sum = |w: &[TodDay], k: f64| -> HashMap<u8, f32> {
            let mut s: HashMap<u8, f64> = HashMap::new();
            for d in w {
                if let Some(v) = d.agg.get(sym) {
                    for &(b, dol) in v {
                        *s.entry(b).or_insert(0.0) += f64::from(dol);
                    }
                }
            }
            s.into_iter().map(|(b, x)| (b, (x / k) as f32)).collect()
        };
        let s20 = sum(w20, 20.0);
        if s20.is_empty() {
            // No row in the joined table: `pres.get` misses and the symbol has no baseline.
            continue;
        }
        let s5 = sum(w5, 5.0);
        let p20 = w20.iter().filter(|d| d.present.contains(sym)).count() as u32;
        let p5_real = w5.iter().filter(|d| d.present.contains(sym)).count() as u32;
        // The quirk: present5 as the highest bucket row reads it (0 when that bucket has no
        // 5-session row: the full join's fill_null).
        let top = *s20.keys().max().expect("non-empty");
        let present5 = if s5.contains_key(&top) { p5_real } else { 0 };
        let buckets = s20
            .iter()
            .map(|(&b, &v20)| (b, (v20, s5.get(&b).copied().unwrap_or(0.0))))
            .collect();
        out.insert(
            sym.to_owned(),
            Baseline {
                present20: p20,
                present5,
                buckets,
            },
        );
    }
    out
}

/// One daily bar as the research reference reads it (`refdata::DailyBars::load`: h, l and c
/// required; only h and c matter here).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Daily {
    /// The session, as days since 1970-01-01 (any monotone day number works).
    pub day: i32,
    pub h: f64,
    pub c: f64,
}

/// A symbol's facts for the day (`refdata::DailyBars::facts` and `ctx_daily.py`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Facts {
    /// The prior session's close and high, split-restated to the day (dollars).
    pub c1: Option<f64>,
    pub h1: Option<f64>,
    /// `runs365(day) > runs365(prior session)`: the prior session made a daily-high run.
    pub prev_ran: bool,
    /// That run, `h / restated prior close − 1` of the prior session's bar (for the text only).
    pub prev_run: Option<f64>,
}

/// The split factor bringing a price of day `from` to day `to`'s basis: the product, in date
/// order, of every split with its effective day in `(from, to]` (`DailyBars::factor`).
pub fn factor(splits: &[(i32, f64)], from: i32, to: i32) -> f64 {
    splits
        .iter()
        .filter(|(d, _)| *d > from && *d <= to)
        .map(|(_, f)| *f)
        .product()
}

/// runs365 of the bars `prior` (oldest first, all before the day asked): the last 366 bars, each
/// against the previous bar's close restated to its own basis.
pub fn runs365(prior: &[Daily], splits: &[(i32, f64)]) -> u32 {
    let lo = prior.len().saturating_sub(RUNS_BARS);
    let mut n = 0;
    for i in (lo + 1)..prior.len() {
        let (p, b) = (&prior[i - 1], &prior[i]);
        let pc = p.c * factor(splits, p.day, b.day);
        if pc > 0.0 && b.h >= RUN_MULT * pc {
            n += 1;
        }
    }
    n
}

/// [`Facts`] for `day` from a symbol's daily bars (any order of dates is NOT accepted: oldest
/// first), its splits `(effective day, old/new)` in date order, and the prior session `prev`
/// (the trading day before `day`). Only bars before `day` are read.
pub fn facts_of(bars: &[Daily], splits: &[(i32, f64)], day: i32, prev: Option<i32>) -> Facts {
    let k = bars.partition_point(|b| b.day < day);
    let prior = &bars[..k];
    let Some(last) = prior.last() else {
        return Facts::default();
    };
    let fx = factor(splits, last.day, day);
    let mut f = Facts {
        c1: Some(last.c * fx),
        h1: Some(last.h * fx),
        ..Default::default()
    };
    // `ctx_daily.py`: runs365_p is the prior SESSION's value, null (so prev_ran false) when the
    // symbol has no history then.
    if let Some(pd) = prev {
        let kp = bars.partition_point(|b| b.day < pd);
        if kp > 0 {
            f.prev_ran = runs365(prior, splits) > runs365(&bars[..kp], splits);
        }
    }
    if prior.len() >= 2 {
        let p = &prior[prior.len() - 2];
        let pc = p.c * factor(splits, p.day, last.day);
        if pc > 0.0 {
            f.prev_run = Some(last.h / pc - 1.0);
        }
    }
    f
}

/// What a fire carries (the notice's facts, the ledger's and the parity harness's fields).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Fire {
    pub slot: i64,
    /// Decision instant: 04:00 + (slot + 1) × 15 s + 50 ms, epoch ns.
    pub t: i64,
    pub close_u: i64,
    /// x-gap: close / C1 − 1. x-ctx: close / h1 − 1.
    pub gap: f64,
    pub cum: f64,
    pub trail: f64,
    /// trail / base (+inf with a zero baseline).
    pub rvol: f64,
    pub base: f64,
    /// The level crossed: x-gap the earlier session high, x-ctx h1 (micro-dollars).
    pub level_u: i64,
}

/// The day's inputs of one symbol, fixed at 04:00.
#[derive(Clone, Debug, Default)]
pub struct SymDay {
    /// C1 in micro-dollars (half to even), when the facts have a positive C1: x-gap eligible.
    pub c1_u: Option<i64>,
    /// h1 in micro-dollars, when x-ctx is eligible (prev_ran, h1 > 0, a c1).
    pub h1_u: Option<i64>,
    pub base: Baseline,
}

impl SymDay {
    /// From the facts and the baseline (the research's eligibility, module docs).
    pub fn new(f: &Facts, base: Baseline) -> Self {
        let c1_u =
            f.c1.filter(|c| c.is_finite() && *c > 0.0)
                .map(micros_half_even);
        let h1_u =
            f.h1.filter(|h| {
                f.prev_ran && h.is_finite() && *h > 0.0 && f.c1.is_some_and(|c| c / h >= 0.0)
            })
            .map(micros_half_even);
        SymDay { c1_u, h1_u, base }
    }
}

/// One symbol's running state over the day's bars, fed in slot order.
#[derive(Clone, Debug, Default)]
pub struct Sym {
    /// The running `f64` dollars through the last bar (`np.cumsum`).
    cum: f64,
    /// `(slot, cum through it)` of the bars that may still start a trailing window.
    window: VecDeque<(i64, f64)>,
    /// The running sum just before the oldest bar in `window` (`cs[j]`).
    before: f64,
    /// The highest high of the earlier pre-market bars (0 for a bar without a print).
    prevmax: i64,
    last_slot: i64,
    pub gap_fired: bool,
    pub ctx_fired: bool,
}

impl Sym {
    pub fn new() -> Self {
        Sym {
            last_slot: -1,
            ..Default::default()
        }
    }

    /// The next bar (slot strictly after the last one). Returns x-gap's and x-ctx's fire, each
    /// at most once a day. A bar at or past 20:00 is ignored (`slot < END_SLOT`).
    pub fn push(&mut self, b: &Bar, day: &SymDay, t0: i64) -> (Option<Fire>, Option<Fire>) {
        if b.slot >= NSLOT || b.slot <= self.last_slot {
            return (None, None);
        }
        self.last_slot = b.slot;
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
        let base = day.base.of_bucket(b.slot / BUCKET_SLOTS);
        let rvol = if base.is_nan() {
            f64::NAN
        } else if base > 0.0 {
            trail / base
        } else {
            f64::INFINITY
        };
        let has = b.c > 0;
        let vol_ok = !rvol.is_nan() && rvol >= RVOL_MIN && self.cum >= DOLLARS_MIN;
        let t = t0 + (b.slot + 1) * SLOT + DECIDE_NS;
        let cum = self.cum;
        let mk = |gap: f64, level_u: i64| Fire {
            slot: b.slot,
            t,
            close_u: b.c,
            gap,
            cum,
            trail,
            rvol,
            base,
            level_u,
        };
        let mut gap_fire = None;
        if b.slot < OPEN_SLOT {
            let prevmax = self.prevmax;
            self.prevmax = self.prevmax.max(if has { b.h } else { 0 });
            if !self.gap_fired
                && let Some(c1_u) = day.c1_u
                && has
                && in_band(b.c)
                && b.c > prevmax
                && vol_ok
            {
                let gap = b.c as f64 / c1_u.max(1) as f64 - 1.0;
                if gap >= GAP_MIN {
                    self.gap_fired = true;
                    gap_fire = Some(mk(gap, prevmax));
                }
            }
        }
        let mut ctx_fire = None;
        if !self.ctx_fired
            && let Some(h1_u) = day.h1_u
            && has
            && in_band(b.c)
            && b.c > h1_u
            && vol_ok
        {
            self.ctx_fired = true;
            ctx_fire = Some(mk(b.c as f64 / h1_u as f64 - 1.0, h1_u));
        }
        (gap_fire, ctx_fire)
    }

    /// A late print's dollars for an already pushed bar at `slot`: added to the running sum (and
    /// to the trailing window when the bar is still in it), so later bars see the dollars the
    /// research bar had. The order of the additions differs from the research's, in the last
    /// bits only.
    pub fn add_late(&mut self, slot: i64, dollars: f64) {
        self.cum += dollars;
        // The trailing sum is `cum - before`: it gains the dollars exactly when the late bar is
        // still inside the window.
        if self.window.front().is_none_or(|(s, _)| *s > slot) {
            self.before += dollars;
        }
        for (s, c) in self.window.iter_mut() {
            if *s >= slot {
                *c += dollars;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const T0: i64 = 1_718_092_800 * SEC;

    fn base_flat(v: f32) -> Baseline {
        Baseline {
            present20: 20,
            present5: 5,
            buckets: (0..NBUCKETS as u8).map(|b| (b, (v, v))).collect(),
        }
    }

    fn bar(slot: i64, c: i64, dollars: f64) -> Bar {
        Bar {
            slot,
            c,
            h: c,
            dollars,
        }
    }

    #[test]
    fn x_gap_fires_once_on_the_first_new_high_with_gap_dollars_and_rvol() {
        let day = SymDay {
            c1_u: Some(1_000_000),
            h1_u: None,
            base: base_flat(10_000.0),
        };
        let mut s = Sym::new();
        // 04:00: +5 % only, $60k.
        assert_eq!(s.push(&bar(0, 1_050_000, 60_000.0), &day, T0), (None, None));
        // +12 %, $120k cum, trail $120k / 10k = 12x, new high: fires.
        let (g, c) = s.push(&bar(1, 1_120_000, 60_000.0), &day, T0);
        let g = g.expect("fires");
        assert!(c.is_none());
        assert_eq!(g.t, T0 + 2 * SLOT + DECIDE_NS);
        assert!((g.gap - 0.12).abs() < 1e-12);
        assert_eq!(g.level_u, 1_050_000);
        // Once a day.
        assert_eq!(s.push(&bar(2, 1_300_000, 60_000.0), &day, T0).0, None);
    }

    #[test]
    fn no_new_high_no_baseline_or_regular_session_means_no_x_gap() {
        let mut day = SymDay {
            c1_u: Some(1_000_000),
            h1_u: None,
            base: base_flat(10_000.0),
        };
        let mut s = Sym::new();
        s.push(&bar(0, 1_200_000, 200_000.0), &day, T0);
        // The first bar was the high: equal is not a new high... it fired already. Fresh state:
        let mut s2 = Sym::new();
        let b0 = Bar {
            slot: 0,
            c: 1_150_000,
            h: 1_300_000,
            dollars: 50_000.0,
        };
        s2.push(&b0, &day, T0);
        assert!(
            s2.push(&bar(1, 1_250_000, 80_000.0), &day, T0).0.is_none(),
            "below the earlier high"
        );
        // No baseline (NaN) never passes.
        day.base = Baseline::default();
        let mut s3 = Sym::new();
        assert!(s3.push(&bar(0, 1_200_000, 200_000.0), &day, T0).0.is_none());
        // A zero baseline always passes the RVOL test.
        day.base = Baseline {
            present20: 20,
            ..Default::default()
        };
        let mut s4 = Sym::new();
        assert!(s4.push(&bar(0, 1_200_000, 200_000.0), &day, T0).0.is_some());
        // 09:30 and later is not pre-market.
        let mut s5 = Sym::new();
        day.base = base_flat(1.0);
        assert!(
            s5.push(&bar(OPEN_SLOT, 1_200_000, 200_000.0), &day, T0)
                .0
                .is_none()
        );
        let _ = s;
    }

    #[test]
    fn x_ctx_needs_a_close_strictly_above_h1() {
        let day = SymDay {
            c1_u: Some(1_000_000),
            h1_u: Some(1_500_000),
            base: base_flat(1_000.0),
        };
        let mut s = Sym::new();
        assert!(
            s.push(&bar(2000, 1_500_000, 200_000.0), &day, T0)
                .1
                .is_none(),
            "equal"
        );
        let c = s
            .push(&bar(2001, 1_510_000, 50_000.0), &day, T0)
            .1
            .expect("fires");
        assert_eq!(c.level_u, 1_500_000);
        assert!(
            s.push(&bar(2002, 1_600_000, 50_000.0), &day, T0)
                .1
                .is_none(),
            "once"
        );
    }

    #[test]
    fn the_trailing_window_is_slots_s_minus_19_to_s() {
        let day = SymDay {
            c1_u: None,
            h1_u: Some(1_000_000),
            base: base_flat(10_000.0),
        };
        let mut s = Sym::new();
        // $100k at slot 0, then quiet $1k bars: at slot 20 the first bar left the window.
        s.push(&bar(0, 900_000, 100_000.0), &day, T0);
        for k in 1..20 {
            s.push(&bar(k, 900_000, 1_000.0), &day, T0);
        }
        // slot 19's window holds slot 0: trail 119k. slot 20's does not: 20k / 10k = 2 < 3.
        assert!(s.push(&bar(20, 1_100_000, 1_000.0), &day, T0).1.is_none());
        let mut s2 = Sym::new();
        s2.push(&bar(0, 900_000, 100_000.0), &day, T0);
        assert!(s2.push(&bar(19, 1_100_000, 1_000.0), &day, T0).1.is_some());
    }

    #[test]
    fn the_baseline_rule_and_its_quirk() {
        let day = |agg: &[(&str, &[(u8, f32)])], present: &[&str]| TodDay {
            agg: agg
                .iter()
                .map(|(s, v)| (s.to_string(), v.to_vec()))
                .collect(),
            present: present.iter().map(|s| s.to_string()).collect(),
        };
        // 20 sessions, A present on all, $200 in bucket 3 each: b20 = 200.
        let mut prior: Vec<TodDay> = (0..20)
            .map(|_| day(&[("A", &[(3, 200.0)])], &["A"]))
            .collect();
        let b = &baselines(&prior, ["A"])["A"];
        assert_eq!((b.present20, b.present5, b.of_bucket(3)), (20, 5, 200.0));
        assert_eq!(b.of_bucket(4), 0.0, "an untraded bucket is 0, not none");
        // A absent from 2 of the 20: the 5-session mean x 5 / present5.
        prior[0].present.clear();
        prior[1].present.clear();
        let b = &baselines(&prior, ["A"])["A"];
        assert_eq!((b.present20, b.of_bucket(3)), (18, 200.0));
        // Its top bucket (9) was traded only 10 sessions ago: present5 reads 0, no baseline.
        prior[9].agg.get_mut("A").unwrap().push((9, 50.0));
        let b = &baselines(&prior, ["A"])["A"];
        assert_eq!(b.present5, 0);
        assert!(b.of_bucket(3).is_nan());
        // Never traded in the window: no baseline at all.
        assert!(!baselines(&prior, ["B"]).contains_key("B"));
    }

    #[test]
    fn facts_restate_splits_and_detect_yesterdays_run() {
        let d = |day, h, c| Daily { day, h, c };
        // Day 3 ran: high 1.40 vs close 1.00. A 1-for-10 reverse split effective day 5.
        let bars = [d(1, 1.0, 1.0), d(2, 1.0, 1.0), d(3, 1.4, 1.2)];
        let f = facts_of(&bars, &[(5, 10.0)], 5, Some(4));
        assert_eq!(f.c1, Some(12.0));
        assert_eq!(f.h1, Some(14.0));
        assert!(
            !f.prev_ran,
            "no bar on the prior session (4): the run was two sessions ago"
        );
        assert!(facts_of(&bars, &[(5, 10.0)], 5, Some(3)).prev_ran);
        let f = facts_of(&bars, &[], 4, Some(3));
        assert!(f.prev_ran);
        assert!((f.prev_run.unwrap() - 0.4).abs() < 1e-12);
        let f = facts_of(&bars[..2], &[], 3, Some(2));
        assert!(!f.prev_ran);
    }

    #[test]
    fn cache_dollars_round_the_vwap_to_a_hundredth_of_a_cent() {
        // 1,000 shares for $2,000.04: VWAP 2.00004 -> 2.0000 at c = 2.00.
        assert_eq!(cache_dollars(2_000_000, 1_000, 2_000.04), 2_000.0);
        assert_eq!(cache_dollars(0, 0, 0.0), 0.0);
        // Without an OHLC print the offset is the absolute VWAP.
        assert_eq!(cache_dollars(0, 100, 150.0), 150.0);
        assert_eq!(micros_half_even(0.0000025), 2);
    }
}

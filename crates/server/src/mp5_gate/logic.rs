//! rw-mp5-1's decision, std-only so a research harness can compile this very file through
//! `#[path]` (the N1 precedent, `crate::n1_live::logic`). Every rule below is the research's,
//! with its arithmetic and order, so a decision on the research's own inputs is the research's
//! decision bit for bit (parity report: `C:\wavystack\outputs\stockwatcher-research-v2\50-ship\
//! mp5\PARITY.md`).
//!
//! Research source (frozen): branch research/v2-rw-mp5 0294ddac, `research/v2/rw_mp5`:
//! `rw_apply.py` (the rows), `rw_features.py` (the features), `rw_grid.py` (gate G9, config
//! A-G9 = mode all, no confirmation, every session); finalist file
//! `40-rework/finalists/rw-mp5-1.json` (sha256 [`CONFIG_SHA256`]).
//!
//! # The rule
//!
//! An MP5 crossing (`signals_crossing`, decided at `a`) is an rw-mp5-1 alert when, in order:
//!
//! 1. **Band:** its price is in $0.10–$25 inclusive (`mp5_label.read_notices`).
//! 2. **No B0 alert within 5 s:** no default-stream alert ([`B0_KINDS`]) on the same symbol with
//!    `|b − a| <= 5 s` (`rw_apply.py:104`). Symmetric: an alert up to 5 s AFTER the crossing
//!    removes it too, so live holds each crossing 5 s before deciding ([`Dedup`]).
//! 3. **G9:** `(chg >= 0.10 and rvol_day >= 3) or runs90 >= 1` (`rw_grid.py:39`), with
//!    - `chg = price / c1 − 1` when `c1 > 0`, else NaN, and NaN counts as −1 (`fill_nan(-1)`);
//!    - `rvol_day` = the symbol's volume-counted shares in the day's COMPLETED 5-minute buckets
//!      (bucket `b` = `[t0 + 300 s·b, t0 + 300 s·(b+1))`, `t0` = 04:00 ET; completed = `b < k`,
//!      `k = clip((a − t0) div 300 s, 0, 192)`) over the time-of-day share baseline summed over
//!      the same buckets. Both sums are sequential f64 (numpy `cumsum`), the baseline's values
//!      f32 widened to f64. `k = 0` (no completed bucket) gives +inf; a zero baseline gives +inf
//!      with volume and 0 without (`rw_features.ratio`).
//!    - `runs90` = the symbol's runner days (high >= 1.3 × the previous bar's split-restated
//!      close) among its last 90 daily bars before the day (`engine_a::refdata::DailyBars::facts`).
//!
//! The baseline per (symbol, bucket) is `Cache.tod_baseline`'s: the mean over the 20 sessions
//! strictly before the day of the bucket's shares (a session where the symbol traded nothing
//! counts 0), each mean `(Σ as f64) / 20` rounded to f32; with fewer than 20 prior sessions, the
//! 5-session mean `(Σ of the last ≤ 5) / 5` instead ([`baseline`]).
//!
//! A symbol the research universe left out (universe-v1 type excluded) never passes: research had
//! no `c1`/`runs90` for it (chg NaN, runs90 0). Live checks the type explicitly
//! ([`Facts::excluded`]).

use std::collections::{HashMap, VecDeque};

pub const SEC: i64 = 1_000_000_000;
/// A time-of-day bucket: 5 minutes.
pub const BUCKET_NS: i64 = 300 * SEC;
/// 04:00–20:00 ET in 5-minute buckets.
pub const NB: usize = 192;
/// The B0 dedup window, both ways (`rw_apply.DEDUP_NS`).
pub const DEDUP_NS: i64 = 5 * SEC;
/// The band on the crossing's price (`mp5_label.read_notices`), dollars, inclusive.
pub const BAND_LO: f64 = 0.10;
pub const BAND_HI: f64 = 25.0;
/// G9's thresholds (`rw_grid.gate_expr`).
pub const CHG_MIN: f64 = 0.10;
pub const RVOL_MIN: f64 = 3.0;
pub const RUNS90_MIN: u32 = 1;
/// The baselines' session counts (`Cache.tod_baseline(ks=(5, 20))`, `rw_features` choice rule).
pub const BASE_LONG: usize = 20;
pub const BASE_SHORT: usize = 5;
/// The runner-day rule of the daily facts (`refdata::DailyBars::facts`).
pub const RUN_HIGH_X: f64 = 1.3;
pub const RUNS_WINDOW: usize = 90;
/// Daily bars the facts need at most: 90 runner pairs = 91 bars (the archive keeps
/// `DAILY_BARS_KEEP_DAYS`, 63 by default: see the module docs of `super`).
pub const FACTS_BARS: usize = RUNS_WINDOW + 1;

/// The ledger's detector id and version.
pub const DETECTOR: &str = "rw-mp5-1";
/// sha256 of `40-rework/finalists/rw-mp5-1.json` at sealing (FINALISTS-v2b.md, row 1).
pub const CONFIG_SHA256: &str = "933ca78580fded2c9e18facd24873a48b5e50ac60a1f3b7e4bbe194d1a25bc7e";
/// The research code the port follows (research/v2-rw-mp5).
pub const CODE_COMMIT: &str = "0294ddac776c39f4583a1485d317da5088264ab4";

/// The default stream B0 the research dedups against (fc9e1532's default-switch kinds,
/// `research/v2-b0/b0_streams.py:17-34`), by `NoticeKind::as_str`. Not `breakout`, not the Ross
/// or Signals kinds, not the Halt watcher, not the halt statuses, and none of the newer add-ons.
pub const B0_KINDS: [&str; 12] = [
    "new",
    "burst",
    "running",
    "run",
    "fast",
    "shelf",
    "followup",
    "pullback",
    "pullback_crossing",
    "n1",
    "f1u",
    "halt_bull",
];

pub fn is_b0_kind(kind: &str) -> bool {
    B0_KINDS.contains(&kind)
}

/// The bucket of a print at `ns` on the day starting at `t0` (04:00 ET), or `None` outside
/// 04:00–20:00 (the store's `DayClock::slot` is `None` there, so the research never counted it).
pub fn bucket_of(t0: i64, ns: i64) -> Option<usize> {
    if ns < t0 {
        return None;
    }
    let b = ((ns - t0) / BUCKET_NS) as usize;
    (b < NB).then_some(b)
}

/// Completed buckets at `a`: `clip((a − t0) // 300 s, 0, 192)` (numpy floor division).
pub fn completed(t0: i64, a: i64) -> usize {
    (a - t0).div_euclid(BUCKET_NS).clamp(0, NB as i64) as usize
}

/// One symbol's day, per bucket: volume-counted prints, shares and dollars (the cache's
/// `tod_agg` columns `n`, `vol`, `dol`). Only `vol` enters rw-mp5-1; `n` and `dol` are kept for
/// the other time-of-day users (x-gap-1, x-ctx-1 and b-good read dollars).
#[derive(Clone, Debug, PartialEq)]
pub struct SymDay {
    pub n: [u32; NB],
    pub vol: [u64; NB],
    pub dol: [f64; NB],
}

impl Default for SymDay {
    fn default() -> Self {
        SymDay {
            n: [0; NB],
            vol: [0; NB],
            dol: [0.0; NB],
        }
    }
}

impl SymDay {
    /// One volume-counted print (the chart engine's verdict, as the store's 15 s bars count it:
    /// `store.rs`, `if p.vol { vol += size; dollars += px × size; n_vol += 1 }`). `px` dollars.
    pub fn add(&mut self, t0: i64, ns: i64, size: u64, px: f64) {
        if let Some(b) = bucket_of(t0, ns) {
            self.n[b] = self.n[b].saturating_add(1);
            self.vol[b] = self.vol[b].saturating_add(size);
            self.dol[b] += px * size as f64;
        }
    }
}

/// The share baseline per bucket from the prior sessions' bucket shares, oldest first, at most
/// the last [`BASE_LONG`] (`None` = the symbol traded nothing that session: it counts 0).
/// `Cache.tod_baseline` + the `k20_sessions >= 20` choice of `rw_features.day_features`.
pub fn baseline(prior: &[Option<&[u64; NB]>]) -> [f32; NB] {
    let prior = &prior[prior.len().saturating_sub(BASE_LONG)..];
    let (rows, k) = if prior.len() >= BASE_LONG {
        (prior, BASE_LONG)
    } else {
        (&prior[prior.len().saturating_sub(BASE_SHORT)..], BASE_SHORT)
    };
    let mut out = [0f32; NB];
    for (b, o) in out.iter_mut().enumerate() {
        // Integer shares summed in f64 are exact whatever the order (all far below 2^53).
        // A fold from +0.0, not `sum()`: an empty f64 sum is -0.0 in Rust, where the research has
        // 0 (no row); harmless for the ratio, but the baseline is then bit-identical.
        let s: f64 = rows.iter().flatten().fold(0.0, |acc, v| acc + v[b] as f64);
        *o = (s / k as f64) as f32;
    }
    out
}

/// `rw_features.ratio`: `num / den` with a positive denominator, else +inf with volume, else 0.
fn ratio(num: f64, den: f64) -> f64 {
    if den > 0.0 {
        num / den
    } else if num > 0.0 {
        f64::INFINITY
    } else {
        0.0
    }
}

/// `rvol_day` at `a` (module docs): `today` = the day's bucket shares, `base` = the baseline.
pub fn rvol_day(t0: i64, a: i64, today: Option<&[u64; NB]>, base: Option<&[f32; NB]>) -> f64 {
    let k = completed(t0, a);
    if k == 0 {
        return f64::INFINITY;
    }
    let mut vol_c = 0f64;
    let mut bas_c = 0f64;
    for b in 0..k {
        vol_c += today.map_or(0.0, |v| v[b] as f64);
        bas_c += base.map_or(0.0, |v| v[b] as f64);
    }
    ratio(vol_c, bas_c)
}

/// `chg` (module docs): NaN without a positive `c1`.
pub fn chg(price: f64, c1: Option<f64>) -> f64 {
    match c1 {
        Some(c) if c > 0.0 => price / c - 1.0,
        _ => f64::NAN,
    }
}

/// A symbol's daily facts for the gate.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Facts {
    /// The previous session's close on the day's share basis.
    pub c1: Option<f64>,
    pub runs90: u32,
    /// universe-v1 excludes the symbol's type (`crate::n1_live::logic::sec_type`).
    pub excluded: bool,
}

/// One daily bar (`refdata::DailyBars::load` keeps a bar only with h, l and c). `d` = YYYYMMDD.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DailyBar {
    pub d: u32,
    pub h: f64,
    pub c: f64,
}

/// `refdata::DailyBars::factor`: the product of every split factor (old/new) with effective date
/// in `(from, day]`, in the ledger's date order. `splits` = `(YYYYMMDD, factor)`.
pub fn split_factor(splits: &[(u32, f64)], from: u32, day: u32) -> f64 {
    splits
        .iter()
        .filter(|(d, _)| *d > from && *d <= day)
        .map(|(_, f)| *f)
        .product()
}

/// `c1` and `runs90` from a symbol's daily bars strictly before `day`, oldest first
/// (`refdata::DailyBars::facts`, same loop and arithmetic; only the bars the two facts need are
/// read: the last, and the last [`FACTS_BARS`] for the runner pairs).
pub fn facts_of(prior: &[DailyBar], splits: &[(u32, f64)], day: u32) -> (Option<f64>, u32) {
    let Some(last) = prior.last() else {
        return (None, 0);
    };
    let c1 = Some(last.c * split_factor(splits, last.d, day));
    let mut runs90 = 0;
    let lo = prior.len().saturating_sub(366);
    for i in (lo + 1)..prior.len() {
        let age = prior.len() - i; // 1 = the previous session
        if age > RUNS_WINDOW {
            continue;
        }
        let (p, b) = (&prior[i - 1], &prior[i]);
        let pc = p.c * split_factor(splits, p.d, b.d);
        if pc > 0.0 && b.h >= RUN_HIGH_X * pc {
            runs90 += 1;
        }
    }
    (c1, runs90)
}

/// Why a crossing was or was not an rw-mp5-1 alert.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Why {
    /// G9 passed by `chg >= 10 % and rvol_day >= 3`.
    InPlay,
    /// G9 passed by `runs90 >= 1` (only).
    PriorRunner,
    /// Price outside $0.10–$25.
    Band,
    /// A universe-v1 excluded type.
    Excluded,
    /// G9 failed.
    Gate,
    /// The day's prints were not all seen (restart after 04:00 ET) or its facts came late: the
    /// volume is incomplete, so no decision is trusted (the N1 rule, `crate::n1_live`).
    Incomplete,
}

impl Why {
    pub fn code(self) -> &'static str {
        match self {
            Why::InPlay => "in_play",
            Why::PriorRunner => "prior_runner",
            Why::Band => "band",
            Why::Excluded => "excluded",
            Why::Gate => "gate",
            Why::Incomplete => "incomplete",
        }
    }
    pub fn pass(self) -> bool {
        matches!(self, Why::InPlay | Why::PriorRunner)
    }
}

/// The gate's facts on one crossing, kept in the ledger whatever the verdict.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Verdict {
    pub a_ns: i64,
    pub price: f64,
    pub chg: f64,
    pub rvol_day: f64,
    pub runs90: u32,
    pub completed: usize,
    pub why: Why,
}

/// G9 on one deduplicated crossing (module docs, steps 1 and 3).
pub fn decide(
    t0: i64,
    a: i64,
    price: f64,
    facts: Option<&Facts>,
    today: Option<&[u64; NB]>,
    base: Option<&[f32; NB]>,
) -> Verdict {
    let f = facts.copied().unwrap_or_default();
    let chg = chg(price, f.c1);
    let rvol = rvol_day(t0, a, today, base);
    let in_play = (if chg.is_nan() { -1.0 } else { chg }) >= CHG_MIN && rvol >= RVOL_MIN;
    let why = if !(BAND_LO..=BAND_HI).contains(&price) {
        Why::Band
    } else if f.excluded {
        Why::Excluded
    } else if in_play {
        Why::InPlay
    } else if f.runs90 >= RUNS90_MIN {
        Why::PriorRunner
    } else {
        Why::Gate
    };
    Verdict {
        a_ns: a,
        price,
        chg,
        rvol_day: rvol,
        runs90: f.runs90,
        completed: completed(t0, a),
        why,
    }
}

/// The symmetric 5 s dedup against B0 (module docs, step 2), causal: a crossing is parked and
/// released once no B0 alert can still land in its window. Generic over what is parked (the live
/// notice; the parity harness's row).
#[derive(Debug)]
pub struct Dedup<T> {
    recent: HashMap<String, VecDeque<i64>>,
    pending: VecDeque<(String, i64, T)>,
}

impl<T> Default for Dedup<T> {
    fn default() -> Self {
        Dedup {
            recent: HashMap::new(),
            pending: VecDeque::new(),
        }
    }
}

/// How long a B0 instant is remembered: past every window it could still be inside.
const RECENT_KEEP_NS: i64 = 60 * SEC;

impl<T> Dedup<T> {
    /// A B0 alert on `sym` decided at `ns`.
    pub fn note_b0(&mut self, sym: &str, ns: i64) {
        self.recent
            .entry(sym.to_string())
            .or_default()
            .push_back(ns);
    }

    /// A crossing on `sym` decided at `a`, held until [`Dedup::release`] at `a + 5 s` or later.
    pub fn park(&mut self, sym: &str, a: i64, item: T) {
        self.pending.push_back((sym.to_string(), a, item));
    }

    pub fn pending(&self) -> usize {
        self.pending.len()
    }

    /// Every parked crossing whose window has closed by `now` (`now > a + 5 s`: a B0 alert
    /// decided at exactly `a + 5 s` is in the window), with `true` when a B0 alert of its symbol
    /// lies within 5 s of it. In parking order (they are parked in time order).
    pub fn release(&mut self, now: i64) -> Vec<(T, bool)> {
        let mut out = Vec::new();
        while self
            .pending
            .front()
            .is_some_and(|(_, a, _)| now > *a + DEDUP_NS)
        {
            let (sym, a, item) = self.pending.pop_front().expect("front checked");
            let dup = self
                .recent
                .get(&sym)
                .is_some_and(|v| v.iter().any(|b| (b - a).abs() <= DEDUP_NS));
            out.push((item, dup));
        }
        // Forget B0 instants no parked or future crossing can reach.
        let floor = self.pending.front().map_or(now, |(_, a, _)| (*a).min(now)) - RECENT_KEEP_NS;
        self.recent.retain(|_, v| {
            while v.front().is_some_and(|b| *b < floor) {
                v.pop_front();
            }
            !v.is_empty()
        });
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const T0: i64 = 1_719_388_800 * SEC; // 2024-06-26 04:00 ET

    #[test]
    fn buckets_and_completed_follow_the_research_floor_and_clip() {
        assert_eq!(bucket_of(T0, T0 - 1), None);
        assert_eq!(bucket_of(T0, T0), Some(0));
        assert_eq!(bucket_of(T0, T0 + BUCKET_NS - 1), Some(0));
        assert_eq!(bucket_of(T0, T0 + 192 * BUCKET_NS), None);
        assert_eq!(completed(T0, T0 - SEC), 0);
        assert_eq!(completed(T0, T0 + BUCKET_NS), 1);
        assert_eq!(completed(T0, T0 + 500 * BUCKET_NS), NB);
    }

    #[test]
    fn rvol_day_is_inf_before_the_first_bucket_and_without_a_baseline() {
        let mut v = [0u64; NB];
        v[0] = 300;
        let base = [100f32; NB];
        assert_eq!(
            rvol_day(T0, T0 + 10 * SEC, Some(&v), Some(&base)),
            f64::INFINITY
        );
        assert_eq!(rvol_day(T0, T0 + BUCKET_NS, Some(&v), Some(&base)), 3.0);
        assert_eq!(rvol_day(T0, T0 + BUCKET_NS, Some(&v), None), f64::INFINITY);
        assert_eq!(rvol_day(T0, T0 + BUCKET_NS, None, None), 0.0);
    }

    #[test]
    fn baseline_takes_twenty_sessions_else_the_last_five_over_five() {
        let mut a = [0u64; NB];
        a[3] = 7;
        let rows20: Vec<Option<&[u64; NB]>> = (0..20).map(|_| Some(&a)).collect();
        assert_eq!(baseline(&rows20)[3], 7.0);
        // 15 sessions, the symbol traded in only the last two of them: (7 + 7) / 5.
        let mut rows15: Vec<Option<&[u64; NB]>> = vec![None; 13];
        rows15.extend([Some(&a), Some(&a)]);
        assert_eq!(baseline(&rows15)[3], (14f64 / 5.0) as f32);
        assert_eq!(baseline(&[])[3].to_bits(), 0f32.to_bits());
    }

    #[test]
    fn g9_passes_in_play_or_prior_runner_and_nan_chg_counts_as_minus_one() {
        let base = [100f32; NB];
        let mut v = [0u64; NB];
        v[0] = 400;
        let a = T0 + BUCKET_NS + SEC;
        let f = Facts {
            c1: Some(1.0),
            runs90: 0,
            excluded: false,
        };
        assert_eq!(
            decide(T0, a, 1.10, Some(&f), Some(&v), Some(&base)).why,
            Why::InPlay
        );
        assert_eq!(
            decide(T0, a, 1.09, Some(&f), Some(&v), Some(&base)).why,
            Why::Gate
        );
        let r = Facts { runs90: 1, ..f };
        assert_eq!(
            decide(T0, a, 1.09, Some(&r), Some(&v), Some(&base)).why,
            Why::PriorRunner
        );
        let none = Facts { c1: None, ..f };
        assert_eq!(
            decide(T0, a, 5.0, Some(&none), Some(&v), Some(&base)).why,
            Why::Gate
        );
        assert_eq!(
            decide(T0, a, 25.01, Some(&r), Some(&v), Some(&base)).why,
            Why::Band
        );
        let x = Facts {
            excluded: true,
            ..r
        };
        assert_eq!(
            decide(T0, a, 1.5, Some(&x), Some(&v), Some(&base)).why,
            Why::Excluded
        );
        // Unknown symbol: no facts at all is "no c1, runs90 0".
        assert_eq!(
            decide(T0, a, 1.5, None, Some(&v), Some(&base)).why,
            Why::Gate
        );
    }

    #[test]
    fn facts_restate_splits_and_count_runner_days_in_the_last_ninety_pairs() {
        let bar = |d, h, c| DailyBar { d, h, c };
        // A 1-for-10 reverse split effective 20240103: 1.0 -> 10.0 is no run on the restated basis.
        let prior = [
            bar(20240102, 1.0, 1.0),
            bar(20240103, 10.5, 10.0),
            bar(20240104, 12.5, 12.0),
        ];
        let splits = [(20240103, 10.0)];
        let (c1, runs) = facts_of(&prior, &splits, 20240105);
        assert_eq!(c1, Some(12.0));
        assert_eq!(runs, 0);
        let prior = [bar(20240102, 1.0, 1.0), bar(20240103, 1.31, 1.2)];
        assert_eq!(facts_of(&prior, &[], 20240104).1, 1);
        // A run 91 pairs back is outside the window.
        let mut v = vec![bar(20200101, 1.0, 1.0), bar(20200102, 2.0, 1.0)];
        for i in 0..90 {
            v.push(bar(20200103 + i, 1.0, 1.0));
        }
        assert_eq!(facts_of(&v, &[], 20250101).1, 0);
        v.pop();
        assert_eq!(facts_of(&v, &[], 20250101).1, 1);
    }

    #[test]
    fn dedup_is_symmetric_and_releases_only_after_the_window() {
        let mut d = Dedup::default();
        d.note_b0("A", 100 * SEC);
        d.park("A", 104 * SEC, 1);
        d.park("B", 104 * SEC, 2);
        d.park("C", 106 * SEC, 3);
        assert!(d.release(109 * SEC).is_empty());
        d.note_b0("B", 109 * SEC); // exactly a + 5 s: inside
        d.note_b0("C", 111 * SEC + 1); // just past a + 5 s
        assert_eq!(d.release(109 * SEC + 1), vec![(1, true), (2, true)]);
        assert_eq!(d.release(111 * SEC + 1), vec![(3, false)]);
        assert_eq!(d.pending(), 0);
    }
}

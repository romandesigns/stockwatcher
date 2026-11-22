//! b-good, the detector-free model features and the two decisions: std only, no server types, so
//! the parity harness compiles THIS FILE unchanged (`#[path]`,
//! `C:/wavystack/outputs/stockwatcher-research-v2/50-ship/bgood/harness/`) and feeds it the
//! research's own cache bars. Everything that decides a b-good-1 tag or a b-good-2 pick lives here;
//! `super` only feeds bars, a clock, the day's facts and quotes, and turns the results into notices.
//!
//! The research definition is lane B-good of research v2 wave 2 (branch `research/v2-b-good`,
//! finalist commit ef61aeb6, `research/v2/bgood/`): `features.py` (wave 1's 45 features, copied
//! unchanged from `research/v2-b-xgb` 3c42f6f3), `build_w2.py` (`rvb_sofar`, `stream_ctx`, the U2
//! `screen`), `common.py load_u1` (kind / tier codes, the quote group), `u2.py pick` and
//! `finalize.py` / `finalize_u2.py` (θ). Owner approval to ship: DEPLOY-PLAN.md addendum
//! 2026-11-22 ~01:30 ET.
//!
//! # The grid and its arithmetic, exactly as the research
//!
//! The research builds one dense (symbol × 15 s slot) float32 grid per day on the GPU
//! (`features.dense_day`): close / high / low in dollars (`micro / 1e6` in f64, then rounded to
//! float32; NaN for a slot without a priced bar), shares, dollars and prints (f64 → float32, 0 for
//! no bar). Every feature at slot `s` uses slots `<= s` only, and is valid for a decision at
//! `t >= t0 + (s + 1) × 15 s + 2 s` ([`slot_for_ns`]). This module keeps the same values per
//! symbol, advanced slot by slot ([`SymFeat::advance`]):
//!
//! - `px` = the close of the last priced slot (forward fill), NaN before the first;
//! - running sums (`_rsum`) are float64 cumulative sums of the float32 values, differenced, then
//!   rounded to float32, so they are kept here as float64 prefix sums (counts as integers, which
//!   is the same number);
//! - EMAs (`_ema`) start at the first price and step `a·v + (1 − a)·e` with `a` and `1 − a` rounded
//!   to float32 and every multiply and add rounded to float32 (PyTorch launches each as its own
//!   kernel, so nothing is fused; Rust never contracts to FMA unless asked);
//! - divisions, subtractions, comparisons and `max` are IEEE float32 as on the GPU;
//! - `torch.clamp(x, min)` keeps NaN ([`clamp_min`]); `torch.sign(NaN)` is 0.
//!
//! # Where bit-exactness is not promised
//!
//! `log1p` / `log` on CUDA (`log1pf`, `logf`) are within 1 ulp but not correctly rounded; here they
//! are computed in f64 and rounded once to float32 ([`ln1p32`], [`ln32`]), which is correctly
//! rounded and the same on every platform (MSVC here, glibc on the VPS). The parity report counts
//! the features that differ by an ulp and whether any decision flips because of it. The float64
//! prefix sums are sequential here and a parallel scan on the GPU; they round to the same float32
//! except in rare last-bit cases, also counted.
//!
//! # The two decisions
//!
//! - **b-good-1, the confidence tag** (T-B1): every B0-kind alert, scored at `t = a + 30 s` from the
//!   features of slot `slot_for_ns(t)` plus the alert's context ([`Ctx`]), kind and tier codes and
//!   the quote group ([`QuoteAt`]); tagged when the model's margin is at least θ's margin
//!   (`xgb.rs`). The tag can only exist 30 s after its alert.
//! - **b-good-2, model picks** (T-A2): every symbol at every minute-end slot passing the wave-1
//!   screen ([`screen`]), decided at bar end + 2 s, scored by the second model; the online rule
//!   [`Picker`] keeps at most 2 a symbol, 600 s apart, none within 30 minutes after a B0 alert of
//!   the symbol, and at most 30 a day.

pub const SEC: i64 = 1_000_000_000;
pub const SLOT_NS: i64 = 15 * SEC;
/// A bar is usable `SETTLE_NS` after it ends (`features.SETTLE_NS`).
pub const SETTLE_NS: i64 = 2 * SEC;
/// 04:00..20:00 ET in 15 s slots (`features.T`).
pub const NSLOT: i64 = 3840;
/// b-good-1 scores an alert this long after it (`good_label.DELAY_S`).
pub const TAG_DELAY_NS: i64 = 30 * SEC;
/// The U2 screen's last slot (`build_w2.do_day`: `min(3719, last_bar)`).
pub const U2_LAST_SLOT: i64 = 3719;

/// The 46 time-series and day features, in the finalists' column order (`common.GROUPS`: the
/// session code is `ses_code`, see `common.py`; `rvb_sofar` last). b-good-2 reads exactly these;
/// b-good-1 reads them followed by [`U1_EXTRA`].
pub const FEATS: [&str; 46] = [
    "r15s",
    "r30s",
    "r1m",
    "r3m",
    "r5m",
    "r15m",
    "acc1m",
    "acc30s",
    "ldol1m",
    "ldol5m",
    "ldol15m",
    "n1m",
    "n5m",
    "rv5_5d",
    "rv5_20d",
    "rdvol_day",
    "vx1m",
    "vwap_d",
    "vwap_above40",
    "ema9_d",
    "ema20_d",
    "ema9_20",
    "ema9_slope",
    "macd",
    "macd_hist",
    "macd_hist_d1m",
    "rng1m_x",
    "rng15_x",
    "body1m",
    "hi15_d",
    "hi30_d",
    "hi60_d",
    "hod_d",
    "lod_d",
    "tod_min",
    "ses_code",
    "age_s",
    "lpx",
    "gain_c1",
    "gap_h1",
    "ladv20",
    "mdr20",
    "runs90",
    "runs365",
    "prior_sessions",
    "rvb_sofar",
];
pub const NF: usize = FEATS.len();
/// b-good-1's ten columns after [`FEATS`] (`common.GROUPS` ctx + quote).
pub const U1_EXTRA: [&str; 10] = [
    "kind_code",
    "tier_code",
    "ctx_n_sym_prior",
    "ctx_min_since_sym",
    "ctx_n_day_prior",
    "spread_bp_alert",
    "spread_bp_t",
    "q_age_ms_f",
    "q_ref_kind",
    "q_flags_f",
];
pub const NF1: usize = NF + U1_EXTRA.len();

/// The B0 kinds (the default-switch stream at fc9e1532, `b0_streams.py B0_KINDS`), sorted as
/// `common.load_u1` sorts the table's `c_kind` values: the index is `kind_code`.
pub const B0_KINDS_SORTED: [&str; 12] = [
    "burst",
    "f1u",
    "fast",
    "followup",
    "halt_bull",
    "n1",
    "new",
    "pullback",
    "pullback_crossing",
    "run",
    "running",
    "shelf",
];

/// `kind_code` of a notice kind (`as_str`), `None` for a kind outside B0 (never tagged, never
/// counted in the context).
pub fn kind_code(kind: &str) -> Option<usize> {
    B0_KINDS_SORTED.iter().position(|k| *k == kind)
}

/// The price tier (`universe::price_tier`): 0 under $1, 1 under $5, else 2. `tier_code` is the
/// same number (the table's tiers "0" < "1" < "2").
pub fn price_tier(px_u: i64) -> u8 {
    if px_u < 1_000_000 {
        0
    } else if px_u < 5_000_000 {
        1
    } else {
        2
    }
}

/// `universe-v1`'s band (micro-dollars, inclusive) and staleness, which select the B0 rows that
/// were scored (`universe::check` in the research's eval).
pub const BAND_LO_U: i64 = 100_000;
pub const BAND_HI_U: i64 = 25_000_000;
pub const STALE_NS: i64 = 60 * SEC;

/// `universe::check` on the last OHLC-counted print strictly before `t` (`(ns, px_u)`): in the
/// band and at most 60 s old. The session and security-type tests are the caller's.
pub fn universe_ok(last: Option<(i64, i64)>, t: i64) -> Option<i64> {
    let (ns, px) = last?;
    (t - ns <= STALE_NS && (BAND_LO_U..=BAND_HI_U).contains(&px)).then_some(px)
}

/// The last slot whose bar ended at least 2 s before `ns` (`features.slot_for_ns`), -1 if none.
pub fn slot_for_ns(ns: i64, t0: i64) -> i64 {
    (ns - SETTLE_NS - t0).div_euclid(SLOT_NS) - 1
}

/// One 15 s bar of one symbol (the cache's decoded bar: integer micro-dollar prices, 0 = no
/// price; shares; dollars; prints).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Bar {
    pub c_u: i64,
    pub h_u: i64,
    pub l_u: i64,
    pub vol: f64,
    pub dollars: f64,
    pub n: f64,
}

/// A symbol's daily facts (`syms.parquet`; NaN = missing).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DayFacts {
    pub c1: f64,
    pub h1: f64,
    pub adv20: f64,
    pub dadv20: f64,
    pub mdr20: f64,
    pub runs90: f64,
    pub runs365: f64,
    pub prior_sessions: f64,
}

impl Default for DayFacts {
    fn default() -> Self {
        DayFacts {
            c1: f64::NAN,
            h1: f64::NAN,
            adv20: f64::NAN,
            dadv20: f64::NAN,
            mdr20: f64::NAN,
            runs90: f64::NAN,
            runs365: f64::NAN,
            prior_sessions: f64::NAN,
        }
    }
}

/// One (symbol, 5-minute bucket) row of `Cache.tod_baseline(day)` (`fill_null(0)` applied).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TodRow {
    pub b5_dol: f32,
    pub b20_dol: f32,
    pub b5_vol: f32,
    pub b20_vol: f32,
    pub present5: u32,
    pub present20: u32,
}

/// A symbol's 192 time-of-day buckets (04:00-based 5-minute buckets).
#[derive(Clone, Debug)]
pub struct TodSym {
    pub rows: Vec<Option<TodRow>>,
}

impl Default for TodSym {
    fn default() -> Self {
        TodSym {
            rows: vec![None; 192],
        }
    }
}

impl TodSym {
    /// `build_w2.rvb_sofar`'s baseline (`good_label.baseline_for`): the 20-session mean volume
    /// when present20 >= 20, else the 5-session mean over the sessions present, else none.
    pub fn vol_base(&self, bucket: usize) -> Option<f32> {
        let r = self.rows.get(bucket).copied().flatten()?;
        let bv = if r.present20 >= 20 {
            r.b20_vol as f64
        } else if r.present5 >= 1 {
            r.b5_vol as f64 * 5.0 / (r.present5.max(1) as f64)
        } else {
            return None;
        };
        bv.is_finite().then_some(bv as f32)
    }
}

/// `torch.clamp(x, min=m)`: NaN stays NaN.
#[inline]
pub fn clamp_min(x: f32, m: f32) -> f32 {
    if x.is_nan() { x } else { x.max(m) }
}

/// `log1p` rounded once to float32 (module docs, "Where bit-exactness is not promised").
#[inline]
pub fn ln1p32(x: f32) -> f32 {
    (x as f64).ln_1p() as f32
}

#[inline]
pub fn ln32(x: f32) -> f32 {
    (x as f64).ln() as f32
}

/// A fixed ring of the last `N` slots' values, indexed by absolute slot.
#[derive(Clone, Debug)]
struct Ring<T: Copy, const N: usize> {
    v: [T; N],
}

impl<T: Copy, const N: usize> Ring<T, N> {
    fn new(fill: T) -> Self {
        Ring { v: [fill; N] }
    }
    #[inline]
    fn set(&mut self, s: i64, x: T) {
        self.v[(s as usize) % N] = x;
    }
    #[inline]
    fn get(&self, s: i64) -> T {
        self.v[(s as usize) % N]
    }
}

// EMA weights `2 / (n + 1)`, as the research's Python floats; rounded to float32 where used.
const A9: f64 = 2.0 / 37.0;
const A20: f64 = 2.0 / 81.0;
const A12: f64 = 2.0 / 49.0;
const A26: f64 = 2.0 / 105.0;

#[inline]
fn ema_step(e: f32, v: f32, a: f64) -> f32 {
    if e.is_nan() {
        v
    } else {
        let a32 = a as f32;
        let b32 = (1.0 - a) as f32;
        a32 * v + b32 * e
    }
}

/// One symbol's running feature state for one day. Advance it through every slot in order
/// ([`Self::advance`]); [`Self::features`] then gives the 46 features at the current slot.
///
/// The rings hold exactly what the features look back on (the deepest: `hi60_d`, 240 + 4 slots of
/// highs; `vx1m`, 80 + 4 slots of dollar prefix sums), about 5 KB a symbol.
#[derive(Clone, Debug)]
pub struct SymFeat {
    /// The last slot advanced through, -1 before slot 0.
    s: i64,
    /// Last priced slot (`li`), and its close.
    li: i64,
    last_c: f32,
    h: Ring<f32, 256>,
    l: Ring<f32, 128>,
    px: Ring<f32, 64>,
    e9r: Ring<f32, 8>,
    histr: Ring<f32, 8>,
    /// float64 prefix sums of D (dollars), N (prints) and V (shares) at each slot.
    cd: Ring<f64, 128>,
    cn: Ring<f64, 32>,
    cv: Ring<f64, 32>,
    /// prefix counts of priced slots and of slots closing above the VWAP.
    chas: Ring<u32, 64>,
    cabove: Ring<u32, 64>,
    /// prefix sums of `H − L` (NaN as 0) and counts of priced slots, for `rng15_x`.
    cr15: Ring<f64, 32>,
    cc15: Ring<u32, 32>,
    e9: f32,
    e20: f32,
    e12: f32,
    e26: f32,
    sig: f32,
    macd: f32,
    hod: f32,
    lodn: f32,
}

impl Default for SymFeat {
    fn default() -> Self {
        Self::new()
    }
}

impl SymFeat {
    pub fn new() -> Self {
        let nan = f32::NAN;
        SymFeat {
            s: -1,
            li: -1,
            last_c: nan,
            h: Ring::new(nan),
            l: Ring::new(nan),
            px: Ring::new(nan),
            e9r: Ring::new(nan),
            histr: Ring::new(nan),
            cd: Ring::new(0.0),
            cn: Ring::new(0.0),
            cv: Ring::new(0.0),
            chas: Ring::new(0),
            cabove: Ring::new(0),
            cr15: Ring::new(0.0),
            cc15: Ring::new(0),
            e9: nan,
            e20: nan,
            e12: nan,
            e26: nan,
            sig: nan,
            macd: nan,
            hod: f32::NEG_INFINITY,
            lodn: f32::NEG_INFINITY,
        }
    }

    /// The last slot advanced through.
    pub fn slot(&self) -> i64 {
        self.s
    }

    /// The last priced slot, -1 before the first price.
    pub fn last_priced(&self) -> i64 {
        self.li
    }

    /// Steps through every slot up to `s` (empty slots in between), `bar` being slot `s`'s own.
    /// A slot at or before the current one is ignored (a bar can be decided only once).
    pub fn advance(&mut self, s: i64, bar: Option<&Bar>) {
        if s <= self.s || s >= NSLOT {
            return;
        }
        while self.s + 1 < s {
            self.step(None);
        }
        self.step(bar);
    }

    fn step(&mut self, bar: Option<&Bar>) {
        let u = self.s + 1;
        let (c, h, l, d, n, v) = match bar {
            Some(b) => {
                let priced = b.c_u > 0;
                let p = |x: i64| {
                    if priced {
                        (x as f64 / 1e6) as f32
                    } else {
                        f32::NAN
                    }
                };
                (
                    p(b.c_u),
                    p(b.h_u),
                    p(b.l_u),
                    b.dollars as f32,
                    b.n as f32,
                    b.vol as f32,
                )
            }
            None => (f32::NAN, f32::NAN, f32::NAN, 0.0, 0.0, 0.0),
        };
        let has = !c.is_nan();
        if has {
            self.li = u;
            self.last_c = c;
        }
        let px = if self.li >= 0 { self.last_c } else { f32::NAN };
        self.h.set(u, h);
        self.l.set(u, l);
        self.px.set(u, px);
        let prev = |r: &Ring<f64, 128>| if u > 0 { r.get(u - 1) } else { 0.0 };
        let cd = prev(&self.cd) + d as f64;
        self.cd.set(u, cd);
        let p32 = |r: &Ring<f64, 32>| if u > 0 { r.get(u - 1) } else { 0.0 };
        let cn = p32(&self.cn) + n as f64;
        let cv = p32(&self.cv) + v as f64;
        self.cn.set(u, cn);
        self.cv.set(u, cv);
        let vwap = vwap_of(cd, cv);
        let above = has && c > vwap;
        let pu = |r: &Ring<u32, 64>| if u > 0 { r.get(u - 1) } else { 0 };
        self.chas.set(u, pu(&self.chas) + has as u32);
        self.cabove.set(u, pu(&self.cabove) + above as u32);
        let r15 = h - l;
        let cr = if u > 0 { self.cr15.get(u - 1) } else { 0.0 };
        let cc = if u > 0 { self.cc15.get(u - 1) } else { 0 };
        self.cr15
            .set(u, cr + if r15.is_nan() { 0.0 } else { r15 as f64 });
        self.cc15.set(u, cc + (!r15.is_nan()) as u32);
        self.e9 = ema_step(self.e9, px, A9);
        self.e20 = ema_step(self.e20, px, A20);
        self.e12 = ema_step(self.e12, px, A12);
        self.e26 = ema_step(self.e26, px, A26);
        self.macd = self.e12 - self.e26;
        self.sig = ema_step(self.sig, self.macd, A9);
        self.e9r.set(u, self.e9);
        self.histr.set(u, (self.macd - self.sig) / px);
        let hn = if h.is_nan() { f32::NEG_INFINITY } else { h };
        let ln = if l.is_nan() { f32::NEG_INFINITY } else { -l };
        self.hod = self.hod.max(hn);
        self.lodn = self.lodn.max(ln);
        self.s = u;
    }

    fn px_at(&self, u: i64) -> f32 {
        if u < 0 || u > self.s || self.s - u >= 64 {
            f32::NAN
        } else {
            self.px.get(u)
        }
    }

    /// `px / shift(px, k) − 1` at slot `u`.
    fn ret_at(&self, u: i64, k: i64) -> f32 {
        if u < 0 {
            return f32::NAN;
        }
        self.px_at(u) / self.px_at(u - k) - 1.0
    }

    /// `_rsum(D, w)` at slot `u` (float64 difference of prefix sums, then float32).
    fn rsum_d(&self, u: i64, w: i64) -> f32 {
        let c = self.cd.get(u);
        if u >= w {
            (c - self.cd.get(u - w)) as f32
        } else {
            c as f32
        }
    }

    fn rsum_n(&self, u: i64, w: i64) -> f32 {
        let c = self.cn.get(u);
        if u >= w {
            (c - self.cn.get(u - w)) as f32
        } else {
            c as f32
        }
    }

    fn rsum_u(r: &Ring<u32, 64>, u: i64, w: i64) -> f32 {
        let c = r.get(u);
        if u >= w {
            (c - r.get(u - w)) as f32
        } else {
            c as f32
        }
    }

    /// `_rmax(Hn, w)` at slot `u` (−inf padding before slot 0 and for unpriced slots).
    fn hmax(&self, u: i64, w: i64) -> f32 {
        let mut m = f32::NEG_INFINITY;
        let lo = (u - w + 1).max(0);
        let mut k = u;
        while k >= lo {
            let h = self.h.get(k);
            if !h.is_nan() {
                m = m.max(h);
            }
            k -= 1;
        }
        m
    }

    fn lnmax(&self, u: i64, w: i64) -> f32 {
        let mut m = f32::NEG_INFINITY;
        let lo = (u - w + 1).max(0);
        let mut k = u;
        while k >= lo {
            let l = self.l.get(k);
            if !l.is_nan() {
                m = m.max(-l);
            }
            k -= 1;
        }
        m
    }

    /// `rng1m` at slot `u` (NaN for `u < 0`, the shift's fill).
    fn rng1m_at(&self, u: i64) -> f32 {
        if u < 0 {
            return f32::NAN;
        }
        let hi = self.hmax(u, 4);
        let lo = -self.lnmax(u, 4);
        if hi.is_finite() && lo.is_finite() {
            hi - lo
        } else {
            f32::NAN
        }
    }

    /// The 46 [`FEATS`] at the current slot. `tod` is `None` when the day has no time-of-day
    /// baseline at all (then `rv5_*` are NaN, as the research's `tod is None`), `Some(None)` when
    /// the day has one but the symbol has no rows.
    pub fn features(&self, facts: &DayFacts, tod: Option<Option<&TodSym>>) -> [f32; NF] {
        let s = self.s;
        let nan = f32::NAN;
        let mut f = [nan; NF];
        if s < 0 {
            return f;
        }
        let px = self.px_at(s);
        let r1m = |u: i64| self.ret_at(u, 4);
        let r30 = |u: i64| self.ret_at(u, 2);
        f[0] = self.ret_at(s, 1);
        f[1] = r30(s);
        f[2] = r1m(s);
        f[3] = self.ret_at(s, 12);
        f[4] = self.ret_at(s, 20);
        f[5] = self.ret_at(s, 60);
        f[6] = r1m(s) - if s >= 4 { r1m(s - 4) } else { nan };
        f[7] = r30(s) - if s >= 2 { r30(s - 2) } else { nan };
        let d1 = self.rsum_d(s, 4);
        let d5 = self.rsum_d(s, 20);
        let d15 = self.rsum_d(s, 60);
        f[8] = ln1p32(d1);
        f[9] = ln1p32(d5);
        f[10] = ln1p32(d15);
        f[11] = self.rsum_n(s, 4);
        f[12] = self.rsum_n(s, 20);
        let bucket = (s / 20) as usize;
        match tod {
            None => {}
            Some(t) => {
                let b = |k: u8| -> f32 {
                    let r = t.and_then(|t| t.rows.get(bucket).copied().flatten());
                    match (r, k) {
                        (Some(r), 5) => r.b5_dol,
                        (Some(r), _) => r.b20_dol,
                        (None, _) => 0.0,
                    }
                };
                let lb5 = |x: f32| ln1p32(if x.is_nan() { 0.0 } else { x });
                f[13] = ln1p32(d5) - lb5(b(5));
                f[14] = ln1p32(d5) - lb5(b(20));
            }
        }
        let cum_d32 = self.cd.get(s) as f32;
        let cum_v32 = self.cv.get(s) as f32;
        f[15] = cum_d32 / clamp_min(facts.dadv20 as f32, 1.0);
        // `prior20 = shift(rsum(D, 80), 4) / 20.0`: PyTorch divides a tensor by a CPU scalar as a
        // multiply by the scalar's float32 reciprocal (`div_true_kernel_cuda`), not a true
        // division; the true division here differed by an ulp on ~15 % of rows (parity run 1).
        let prior20 = if s >= 4 { self.rsum_d(s - 4, 80) } else { 0.0 } * (1.0f32 / 20.0f32);
        f[16] = d1 / clamp_min(prior20, 1.0);
        let vwap = if cum_v32 > 0.0 {
            cum_d32 / cum_v32.max(1e-9f32)
        } else {
            nan
        };
        f[17] = px / vwap - 1.0;
        let nb = Self::rsum_u(&self.chas, s, 40);
        f[18] = if nb > 0.0 {
            Self::rsum_u(&self.cabove, s, 40) / nb.max(1.0)
        } else {
            nan
        };
        f[19] = px / self.e9 - 1.0;
        f[20] = px / self.e20 - 1.0;
        f[21] = self.e9 / self.e20 - 1.0;
        f[22] = self.e9 / if s >= 4 { self.e9r.get(s - 4) } else { nan } - 1.0;
        f[23] = self.macd / px;
        let hist = self.histr.get(s);
        f[24] = hist;
        f[25] = hist - if s >= 4 { self.histr.get(s - 4) } else { nan };
        let rng1m = self.rng1m_at(s);
        let mut acc = 0.0f32;
        let mut cnt = 0.0f32;
        for j in 1..=20 {
            let r = self.rng1m_at(s - 4 * j);
            if !r.is_nan() {
                acc += r;
                cnt += 1.0;
            }
        }
        f[26] = if cnt > 0.0 {
            rng1m / clamp_min(acc / cnt.max(1.0), 1e-6)
        } else {
            nan
        };
        let r15 = self.h.get(s) - self.l.get(s);
        let cr = |u: i64| if u >= 0 { self.cr15.get(u) } else { 0.0 };
        let cc = |u: i64| if u >= 0 { self.cc15.get(u) } else { 0 };
        let (s20, c20) = if s >= 20 {
            (
                (cr(s - 1) - cr(s - 21)) as f32,
                (cc(s - 1) - cc(s - 21)) as f32,
            )
        } else {
            (cr(s - 1) as f32, cc(s - 1) as f32)
        };
        f[27] = if c20 > 0.0 {
            r15 / clamp_min(s20 / c20.max(1.0), 1e-6)
        } else {
            nan
        };
        let dpx = px - self.px_at(s - 4);
        f[28] = if dpx > 0.0 {
            1.0
        } else if dpx < 0.0 {
            -1.0
        } else {
            0.0
        };
        for (k, w) in [(29usize, 60i64), (30, 120), (31, 240)] {
            let m = if s >= 4 {
                self.hmax(s - 4, w)
            } else {
                f32::NEG_INFINITY
            };
            f[k] = if m.is_finite() { px / m - 1.0 } else { nan };
        }
        f[32] = if self.hod.is_finite() {
            px / self.hod - 1.0
        } else {
            nan
        };
        let lod = -self.lodn;
        f[33] = if lod.is_finite() { px / lod - 1.0 } else { nan };
        let tmin = (s as f32 + 1.0) * 0.25;
        f[34] = tmin;
        f[35] = if tmin <= 330.0 {
            0.0
        } else if tmin <= 720.0 {
            1.0
        } else {
            2.0
        };
        f[36] = if self.li >= 0 {
            (s - self.li) as f32 * 15.0
        } else {
            nan
        };
        f[37] = ln32(px);
        f[38] = px / (facts.c1 as f32) - 1.0;
        f[39] = px / (facts.h1 as f32) - 1.0;
        f[40] = facts.adv20.ln_1p() as f32;
        f[41] = facts.mdr20 as f32;
        f[42] = facts.runs90 as f32;
        f[43] = facts.runs365 as f32;
        f[44] = facts.prior_sessions as f32;
        // rvb_sofar: shares so far in the current 04:00-based 5-minute bucket.
        let start = (s / 20) * 20;
        let before = if start > 0 {
            self.cv.get(start - 1)
        } else {
            0.0
        };
        let sofar = (self.cv.get(s) - before) as f32;
        let base = tod.flatten().and_then(|t| t.vol_base(bucket));
        f[45] = match base {
            Some(b) => ln1p32(sofar) - ln1p32(b),
            None => nan,
        };
        f
    }

    /// The current close-forward-filled price (`px`), NaN before the first price.
    pub fn px(&self) -> f32 {
        self.px_at(self.s)
    }

    /// `_rsum(D, 20)` and `_rsum(N, 20)` at the current slot (the screen's 5-minute dollars and
    /// prints).
    pub fn d5_n5(&self) -> (f32, f32) {
        if self.s < 0 {
            return (0.0, 0.0);
        }
        (self.rsum_d(self.s, 20), self.rsum_n(self.s, 20))
    }
}

/// The running VWAP (`cumD / clamp(cumV, 1e-9)` where `cumV > 0`, in float32).
fn vwap_of(cd: f64, cv: f64) -> f32 {
    let d = cd as f32;
    let v = cv as f32;
    if v > 0.0 {
        d / v.max(1e-9f32)
    } else {
        f32::NAN
    }
}

/// The wave-1 U2 candidate screen at the current slot (`build_w2.screen`): a minute-end slot from
/// slot 19 through `min(3719, last_bar)`, price $0.10–$25 (float32 comparisons, as the GPU's), a
/// price no more than 3 slots old, at least $50,000 and 25 prints over the last 5 minutes.
pub fn screen(f: &SymFeat, last_bar: i64) -> bool {
    let s = f.slot();
    if s < 19 || s > U2_LAST_SLOT.min(last_bar) || s % 4 != 3 || f.last_priced() < 0 {
        return false;
    }
    let px = f.px();
    let (d5, n5) = f.d5_n5();
    (0.10f32..=25.0f32).contains(&px) && s - f.last_priced() <= 3 && d5 >= 5e4 && n5 >= 25.0
}

/// A U2 candidate's decision instant: the bar's end + 2 s.
pub fn cand_ns(t0: i64, s: i64) -> i64 {
    t0 + (s + 1) * SLOT_NS + SETTLE_NS
}

/// The U1 context of one alert among the day's B0 alerts (`build_w2.stream_ctx`): earlier alerts
/// of the symbol and of the day, strictly before the alert's instant, and minutes since the
/// symbol's previous one.
#[derive(Clone, Debug, Default)]
pub struct Ctx {
    /// Every B0 alert instant of the day, by symbol and all together.
    by_sym: std::collections::HashMap<String, Vec<i64>>,
    all: Vec<i64>,
}

impl Ctx {
    /// Records a B0 alert (any order; instants are kept sorted).
    pub fn record(&mut self, sym: &str, a_ns: i64) {
        let v = self.by_sym.entry(sym.to_string()).or_default();
        let i = v.partition_point(|&x| x <= a_ns);
        v.insert(i, a_ns);
        let j = self.all.partition_point(|&x| x <= a_ns);
        self.all.insert(j, a_ns);
    }

    /// `(ctx_n_sym_prior, ctx_min_since_sym, ctx_n_day_prior)` for an alert at `a_ns`, from the
    /// alerts strictly before it (the alert itself, recorded or not, never counts).
    pub fn of(&self, sym: &str, a_ns: i64) -> (f32, f32, f32) {
        let n_day = self.all.partition_point(|&x| x < a_ns) as f64;
        let (n_sym, since) = match self.by_sym.get(sym) {
            Some(v) => {
                let k = v.partition_point(|&x| x < a_ns);
                let since = if k > 0 {
                    (a_ns - v[k - 1]) as f64 / 6e10
                } else {
                    f64::NAN
                };
                (k as f64, since)
            }
            None => (0.0, f64::NAN),
        };
        (n_sym as f32, since as f32, n_day as f32)
    }

    /// The symbol's B0 alert instants (for the picker's skip rule).
    pub fn alerts_of(&self, sym: &str) -> &[i64] {
        self.by_sym.get(sym).map(Vec::as_slice).unwrap_or(&[])
    }

    pub fn len(&self) -> usize {
        self.all.len()
    }

    pub fn is_empty(&self) -> bool {
        self.all.is_empty()
    }
}

/// `cachelib.quote_at` flags.
pub const QF_NO_FILE: u8 = 1;
pub const QF_NO_QUOTE: u8 = 2;
pub const QF_STALE: u8 = 4;
pub const QF_NON_MARKET: u8 = 8;
pub const QUOTE_MAX_AGE_NS: i64 = 60 * SEC;

/// The xsim quote rule at an instant (`cachelib.quote_at`): the last VALID quote (bid > 0, ask >
/// bid) at or before it; `age_ns` −1 and [`QF_NO_QUOTE`] when there is none, [`QF_NO_FILE`] when
/// there is no quote data for the symbol at all; [`QF_STALE`] when older than 60 s;
/// [`QF_NON_MARKET`] when the spread exceeds 50 % of the mid (`4 (ask − bid) > ask + bid`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QuoteAt {
    pub bid_u: i64,
    pub ask_u: i64,
    pub age_ns: i64,
    pub flags: u8,
}

impl QuoteAt {
    pub const NO_DATA: QuoteAt = QuoteAt {
        bid_u: 0,
        ask_u: 0,
        age_ns: -1,
        flags: QF_NO_FILE,
    };

    /// From a book of `(t_ns, bid_u, ask_u)` in time order (invalid quotes included or not: they
    /// are skipped here). `None` for the book = no quote data at all.
    pub fn at(book: Option<&[(i64, i64, i64)]>, t: i64) -> QuoteAt {
        let Some(book) = book else {
            return Self::NO_DATA;
        };
        let k = book.partition_point(|q| q.0 <= t);
        let last = book[..k]
            .iter()
            .rev()
            .find(|q| q.1 > 0 && q.2 > 0 && q.2 > q.1);
        match last {
            None => QuoteAt {
                bid_u: 0,
                ask_u: 0,
                age_ns: -1,
                flags: QF_NO_QUOTE,
            },
            Some(&(qt, b, a)) => {
                let age = t - qt;
                let mut fl = 0;
                if age > QUOTE_MAX_AGE_NS {
                    fl |= QF_STALE;
                }
                if 4 * (a - b) > a + b {
                    fl |= QF_NON_MARKET;
                }
                QuoteAt {
                    bid_u: b,
                    ask_u: a,
                    age_ns: age,
                    flags: fl,
                }
            }
        }
    }

    /// `build_b0_table` `spread`: `(ask − bid) / mid × 1e4` in float64 when the flags are clear,
    /// else missing; float32 as `load_u1` casts it.
    pub fn spread_bp(&self) -> f32 {
        if self.flags != 0 {
            return f32::NAN;
        }
        let mid = (self.ask_u + self.bid_u) as f64 / 2.0;
        ((self.ask_u - self.bid_u) as f64 / mid * 1e4) as f32
    }

    /// `q_age_ms_f`: whole milliseconds, −1 without a quote.
    pub fn age_ms_f(&self) -> f32 {
        if self.age_ns >= 0 {
            (self.age_ns / 1_000_000) as f32
        } else {
            -1.0
        }
    }
}

/// b-good-1's input row: the 46 features at `slot_for_ns(a + 30 s)`, then the codes, the context
/// and the quote group (the order of [`U1_EXTRA`]).
pub fn u1_row(
    ts: &[f32; NF],
    kind_code: usize,
    tier: u8,
    ctx: (f32, f32, f32),
    q_a: &QuoteAt,
    q_t: &QuoteAt,
) -> [f32; NF1] {
    let mut x = [f32::NAN; NF1];
    x[..NF].copy_from_slice(ts);
    x[NF] = kind_code as f32;
    x[NF + 1] = tier as f32;
    x[NF + 2] = ctx.0;
    x[NF + 3] = ctx.1;
    x[NF + 4] = ctx.2;
    x[NF + 5] = q_a.spread_bp();
    x[NF + 6] = q_t.spread_bp();
    x[NF + 7] = q_t.age_ms_f();
    x[NF + 8] = if q_t.flags == 0 { 0.0 } else { 1.0 };
    x[NF + 9] = q_t.flags as f32;
    x
}

/// `u2.py pick`'s limits (`b-good-2.json` `rule`).
pub const PICK_PER_SYM: u32 = 2;
pub const PICK_SPACING_NS: i64 = 600 * SEC;
pub const PICK_B0_SKIP_NS: i64 = 1800 * SEC;
pub const PICK_K: usize = 30;

/// Why a candidate that cleared θ was not picked (counted for the day summary).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Skip {
    DayCap,
    PerSym,
    Spacing,
    RecentB0,
}

/// The online pick rule of `u2.py pick`, fed candidates in time order (`passes` = the model's
/// margin cleared θ's).
#[derive(Clone, Debug, Default)]
pub struct Picker {
    taken: usize,
    cnt: std::collections::HashMap<String, u32>,
    last: std::collections::HashMap<String, i64>,
}

impl Picker {
    /// `Ok(())` when the candidate is picked (and recorded), else why not. `b0` are the symbol's B0
    /// alert instants so far (only those in `[ns − 30 min, ns]` matter).
    pub fn offer(&mut self, sym: &str, ns: i64, b0: &[i64]) -> Result<(), Skip> {
        if self.taken >= PICK_K {
            return Err(Skip::DayCap);
        }
        if self.cnt.get(sym).copied().unwrap_or(0) >= PICK_PER_SYM {
            return Err(Skip::PerSym);
        }
        if self
            .last
            .get(sym)
            .is_some_and(|&l| ns - l < PICK_SPACING_NS)
        {
            return Err(Skip::Spacing);
        }
        if b0.iter().any(|&t| ns - t >= 0 && ns - t < PICK_B0_SKIP_NS) {
            return Err(Skip::RecentB0);
        }
        self.taken += 1;
        *self.cnt.entry(sym.to_string()).or_default() += 1;
        self.last.insert(sym.to_string(), ns);
        Ok(())
    }

    pub fn taken(&self) -> usize {
        self.taken
    }
}

/// One raw daily bar for [`day_facts`] (`refdata::DailyBars`: only bars with high, low and close;
/// a missing volume is 0 and a missing VWAP the close). `day` is any increasing day number.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DailyIn {
    pub day: i64,
    pub h: f64,
    pub l: f64,
    pub c: f64,
    pub v: f64,
    pub vw: f64,
}

/// Linear-interpolated median of the finite values (`stats::median`, numpy's default).
fn median(v: &mut [f64]) -> Option<f64> {
    let mut x: Vec<f64> = v.iter().copied().filter(|x| x.is_finite()).collect();
    if x.is_empty() {
        return None;
    }
    x.sort_by(|a, b| a.total_cmp(b));
    let pos = 0.5 * (x.len() - 1) as f64;
    let (lo, hi) = (pos.floor() as usize, pos.ceil() as usize);
    Some(x[lo] + (x[hi] - x[lo]) * (pos - lo as f64))
}

/// The research's daily facts for a day (`refdata::DailyBars::facts`, engine-a 8ad67ad0, which
/// wrote the cache's `syms.parquet`), from the symbol's bars of every session before it, oldest
/// first. `factor(from, to)` is the split factor bringing a price of day `from` onto day `to`'s
/// share basis (1 without splits).
///
/// - C1 / H1: the last bar's close and high, split-restated to the day;
/// - over the last 20 bars (at least 10): ADV20 = mean split-restated shares, DADV20 = mean
///   `v × vw` (NOT restated, as the research), MDR20 = median `(h − l) / l`;
/// - runs90 / runs365: bars whose high reached 1.3 × the previous bar's close (restated to the
///   bar's own basis) among the last 366 (365 steps), within 90 sessions for runs90;
/// - prior_sessions: how many bars there are (the archive's depth caps it, see `super`'s docs).
pub fn day_facts(prior: &[DailyIn], factor: impl Fn(i64, i64) -> f64, day: i64) -> DayFacts {
    let mut f = DayFacts {
        runs90: 0.0,
        runs365: 0.0,
        prior_sessions: prior.len().min(u16::MAX as usize) as f64,
        ..DayFacts::default()
    };
    let Some(last) = prior.last() else {
        return f;
    };
    let fx = |b: &DailyIn| factor(b.day, day);
    f.c1 = last.c * fx(last);
    f.h1 = last.h * fx(last);
    let w = &prior[prior.len().saturating_sub(20)..];
    if w.len() >= 10 {
        f.adv20 = w.iter().map(|b| b.v / fx(b)).sum::<f64>() / w.len() as f64;
        f.dadv20 = w.iter().map(|b| b.v * b.vw).sum::<f64>() / w.len() as f64;
        let mut r: Vec<f64> = w
            .iter()
            .filter(|b| b.l > 0.0)
            .map(|b| (b.h - b.l) / b.l)
            .collect();
        f.mdr20 = median(&mut r).unwrap_or(f64::NAN);
    }
    let lo = prior.len().saturating_sub(366);
    let (mut r90, mut r365) = (0u32, 0u32);
    for i in (lo + 1)..prior.len() {
        let (p, b) = (&prior[i - 1], &prior[i]);
        let pc = p.c * factor(p.day, b.day);
        if pc > 0.0 && b.h >= 1.3 * pc {
            if prior.len() - i <= 90 {
                r90 += 1;
            }
            r365 += 1;
        }
    }
    f.runs90 = r90 as f64;
    f.runs365 = r365 as f64;
    f
}

//! N1 "early leg", the detector itself: std only, no server types, so the research worktree
//! compiles THIS FILE unchanged (`#[path]`) and replays its own tape through it (the parity
//! harness, `C:/wavystack/outputs/stockwatcher-n1-shadow/`). Everything that decides whether N1
//! fires lives here; `super` only feeds it prints, a clock and the day's facts, and turns a
//! [`Fire`] into a notice.
//!
//! The research definition (branch `research/engine-a`, `crates/engine-a/src/bin/ea_parts/t2.rs`
//! `n1_candidates` / `cmd_generate_t2`, frozen candidate `research/engine-a/r2/T2/frozen/T2-c6.json`
//! blob `d2b5aecf…`, configuration `X0.06-W3-k10` of `n1-grid.json`), step by step, with the
//! research's own arithmetic:
//!
//! 1. **15 s bars** of the day's counted prints on the 04:00 ET grid (`store.rs::build_symbol`):
//!    high, low and close in integer micro-dollars from OHLC-counted prints (the close is the print
//!    with the latest execution time, the later arrival on a tie); volume-counted prints counted
//!    (`u16`, saturating) and their dollars summed in `f32` (`px / 1e6 * size` per print).
//! 2. **Decision** at `t = b + 2 s` for the bar ending at `b`, using only bars ending at or before
//!    `b`: the bar must have a price, its close must be in **$0.10–$25**, and **R20** (the first
//!    OHLC print at least 1.2 × the lowest OHLC print so far that day, itself included) must not
//!    have happened before `b`.
//! 3. **Window** `W = 3` minutes: the 12 slots ending with this bar. High and low over its priced
//!    bars; at least **20 volume-counted prints** and **$50,000** (the difference of the running
//!    `f64` dollar sums, exactly as `derive` computes `cum_dollars`); the close at least
//!    **0.98 × the window high**; the gain close / low − 1 at least **X = 6 %**; and the window's
//!    dollars per minute at least **k = 10 ×** DADV20 / 390 (a missing DADV20 passes).
//! 4. **Symbol facts** from the day's daily reference: a previous close (C1 > 0) is required, and a
//!    security type excluded by `universe-v1` never fires ([`sec_type`]).
//! 5. **Universe at `t`** (`universe::check`): the last OHLC-counted print strictly before `t` is
//!    in $0.10–$25 and at most 60 s old, and `t` is inside 04:00 → session end.
//! 6. **N1's own budget** over the whole day from 04:00 (`alerts::pre_cap` with `per_sym 2`,
//!    `refractory 20 min`, no θ, no day cap): in-universe candidates in time order; one is kept
//!    unless the symbol's last kept one is under 20 minutes earlier or two are already kept.
//!    Premarket candidates count: they can use up the day's two.
//! 7. **Time gate** 09:30–11:30 ET on `t` (`tod_gate("0930-1130")`): only a kept candidate inside
//!    it is a [`Fire`].
//!
//! What the research did after this, and live does not: the J3 twin fold (one security under two
//! tape tickers, an archive artefact) and T2-c6's union rule, which drops an N1 row at a
//! symbol-second production already used. Both are explained, counted, in the parity report.

use std::collections::VecDeque;

pub const SEC: i64 = 1_000_000_000;
pub const MIN: i64 = 60 * SEC;
/// The bar grid (the research store's and the Runners' 15 s slots).
pub const SLOT: i64 = 15 * SEC;
/// Slots from 04:00 to 20:00 ET (`tape::NSLOT`).
pub const NSLOT: i64 = 3840;
/// Decided this long after the bar ends (`N1_SETTLE`).
pub const SETTLE: i64 = 2 * SEC;
/// Window length in minutes (`W3`) and in slots.
pub const WINDOW_MIN: u32 = 3;
pub const WINDOW_SLOTS: i64 = WINDOW_MIN as i64 * (MIN / SLOT);
/// Gain from the window low (`X0.06`).
pub const GAIN_MIN: f64 = 0.06;
/// Window dollars per minute over DADV20 / 390 (`k10`).
pub const REL_MIN: f64 = 10.0;
pub const CLOSE_VS_HIGH: f64 = 0.98;
pub const PRINTS_MIN: u32 = 20;
pub const DOLLARS_MIN: f64 = 50_000.0;
pub const DADV_MINUTES: f64 = 390.0;
pub const PER_SYM: u8 = 2;
pub const REFRACTORY: i64 = 20 * MIN;
/// `universe-v1`'s price band (micro-dollars, inclusive) and staleness.
pub const BAND_LO_U: i64 = 100_000;
pub const BAND_HI_U: i64 = 25_000_000;
pub const STALE_NS: i64 = 60 * SEC;
/// The time gate, as offsets from 04:00 ET: [09:30, 11:30). No DST change happens inside a
/// trading day, so this is exactly the research's ET hour-minute test.
pub const GATE_FROM: i64 = 330 * MIN;
pub const GATE_TO: i64 = 450 * MIN;
/// The research's configuration id, carried on every fire.
pub const CONFIG: &str = "X0.06-W3-k10";
/// Daily-bar sessions in DADV20 and the minimum needed (`refdata::DailyBars::facts`).
pub const DADV_SESSIONS: usize = 20;
pub const DADV_MIN_SESSIONS: usize = 10;

/// A trading day's clock: 04:00 ET and the session end, epoch ns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Clock {
    pub t0: i64,
    pub session_end: i64,
}

impl Clock {
    /// The 15 s slot of an execution time, `None` outside 04:00 + 16 h (`DayClock::slot`).
    pub fn slot(&self, ns: i64) -> Option<i64> {
        if ns < self.t0 {
            return None;
        }
        let k = (ns - self.t0) / SLOT;
        (k < NSLOT).then_some(k)
    }
    pub fn slot_end(&self, k: i64) -> i64 {
        self.t0 + (k + 1) * SLOT
    }
    pub fn in_gate(&self, t: i64) -> bool {
        t >= self.t0 + GATE_FROM && t < self.t0 + GATE_TO
    }
}

/// One print with the chart engine's counting verdict (`tape::Print`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Print {
    pub ns: i64,
    /// Micro-dollars.
    pub px: i64,
    pub size: u64,
    pub ohlc: bool,
    pub vol: bool,
}

/// `universe-v1`'s security-type verdict (`universe::SecTypes::verdict`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SecType {
    Common,
    Excluded,
    Unknown,
}

/// Whole words of the asset name (lowercased, split on anything not a letter or digit).
const WORD_EXCLUDE: [&str; 19] = [
    "etf",
    "etn",
    "etns",
    "fund",
    "funds",
    "ishares",
    "proshares",
    "direxion",
    "warrant",
    "warrants",
    "right",
    "rights",
    "unit",
    "units",
    "preferred",
    "preference",
    "notes",
    "debenture",
    "debentures",
];
/// Phrases, matched on the space-joined words.
const PHRASE_EXCLUDE: [&str; 4] = [
    "exchange traded",
    "depositary shares representing",
    "when issued",
    "trust units",
];

fn name_excluded(name: &str) -> bool {
    if name.contains('%') {
        return true;
    }
    let lower = name.to_lowercase();
    let words: Vec<&str> = lower
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| !w.is_empty())
        .collect();
    if words.iter().any(|w| WORD_EXCLUDE.contains(w)) {
        return true;
    }
    let joined = format!(" {} ", words.join(" "));
    PHRASE_EXCLUDE
        .iter()
        .any(|p| joined.contains(&format!(" {p} ")))
}

/// The verdict for `sym` whose asset name is `name` (`None`: not in `assets.json`).
pub fn sec_type(sym: &str, name: Option<&str>) -> SecType {
    if let Some((_, suf)) = sym.split_once('.') {
        let s = suf.to_ascii_uppercase();
        if s == "U"
            || s.starts_with("WS")
            || s == "RT"
            || s == "R"
            || s.starts_with("PR")
            || s == "WI"
        {
            return SecType::Excluded;
        }
    }
    match name {
        None => SecType::Unknown,
        Some(n) if name_excluded(n) => SecType::Excluded,
        Some(_) => SecType::Common,
    }
}

/// A symbol's facts from the daily reference (sessions strictly before the day).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Facts {
    /// The previous session's close (the research restates it for splits; only C1 > 0 matters
    /// here, and a split factor is positive).
    pub c1: Option<f64>,
    /// Mean of `volume × vwap` over the symbol's last 20 daily bars, at least 10 of them.
    pub dadv20: Option<f64>,
}

/// One daily bar as the reference reads it (`refdata::DailyBars::load`: h, l and c required;
/// a missing volume is 0, a missing VWAP is the close).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DailyIn {
    pub h: Option<f64>,
    pub l: Option<f64>,
    pub c: Option<f64>,
    pub v: Option<f64>,
    pub vw: Option<f64>,
}

/// [`Facts`] from a symbol's daily bars before the day, oldest first (`refdata::DailyBars::facts`,
/// the two fields N1 reads, same arithmetic and order).
pub fn facts_of(prior: &[DailyIn]) -> Facts {
    let usable: Vec<(f64, f64)> = prior
        .iter()
        .filter_map(|b| match (b.h, b.l, b.c) {
            (Some(_), Some(_), Some(c)) => Some((b.v.unwrap_or(0.0), b.vw.unwrap_or(c))),
            _ => None,
        })
        .collect();
    let c1 = prior.iter().rev().find_map(|b| match (b.h, b.l, b.c) {
        (Some(_), Some(_), Some(c)) => Some(c),
        _ => None,
    });
    let w = &usable[usable.len().saturating_sub(DADV_SESSIONS)..];
    let dadv20 = (w.len() >= DADV_MIN_SESSIONS)
        .then(|| w.iter().map(|(v, vw)| v * vw).sum::<f64>() / w.len() as f64);
    Facts { c1, dadv20 }
}

/// Micro-dollars as the store keeps them (`store::px32`).
fn px32(u: i64) -> u32 {
    u.clamp(1, u32::MAX as i64) as u32
}

#[derive(Clone, Copy, Debug)]
struct Bar {
    slot: i64,
    has_px: bool,
    h: u32,
    l: u32,
    c: u32,
    c_ns: i64,
    n_vol: u16,
    dollars: f32,
}

/// What one decided bar was (the parity harness reads every field).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Cand {
    /// Decision time `b + 2 s`, epoch ns.
    pub t: i64,
    pub close_u: u32,
    pub low_u: u32,
    pub high_u: u32,
    pub prints: u32,
    pub dollars: f64,
    /// Window dollars per minute over DADV20 / 390; `None` when DADV20 is missing.
    pub rel: Option<f64>,
    /// Execution time of the bar's close print.
    pub close_ns: i64,
    /// `universe-v1` at `t`.
    pub in_universe: bool,
    /// Kept by N1's budget (only an in-universe candidate can be).
    pub kept: bool,
    /// Kept and inside 09:30–11:30 ET: a fire.
    pub fire: bool,
}

impl Cand {
    pub fn gain(&self) -> f64 {
        self.close_u as f64 / self.low_u as f64 - 1.0
    }
}

/// Late or skipped input, counted (never silently different from the research).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Late {
    /// A print for a slot whose window had already moved past it.
    pub prints_dropped: u64,
    /// A print that landed in an already decided bar (it still counts for later windows).
    pub prints_after_decision: u64,
}

/// One symbol's day: bars of the last window, R20, the recent OHLC prints and the budget.
#[derive(Clone, Debug, Default)]
pub struct Sym {
    /// Bars by slot, ascending; never older than the oldest window still to be decided.
    bars: VecDeque<Bar>,
    /// Running `f64` sum of the dollars of the bars already dropped from `bars`, in slot order.
    evicted_cum: f64,
    /// Slots below this were dropped.
    evict_below: i64,
    /// The last slot decided (`-1`: none).
    decided: i64,
    run_min: Option<i64>,
    r20_ns: Option<i64>,
    /// OHLC prints `(ns, px)` by execution time: the last one before the oldest pending decision
    /// and everything after it.
    px: VecDeque<(i64, i64)>,
    kept: u8,
    last_kept: Option<i64>,
    pub late: Late,
}

impl Sym {
    pub fn new() -> Self {
        Sym {
            decided: -1,
            ..Default::default()
        }
    }

    /// One print. O(1) but for a late print, which is placed by a short walk from the back.
    pub fn apply(&mut self, p: &Print, clock: &Clock) {
        if p.ohlc {
            // R20 over every OHLC print of the day, the minimum including the print itself.
            let m = self.run_min.map_or(p.px, |m| m.min(p.px));
            self.run_min = Some(m);
            if self.r20_ns.is_none() && p.px * 10 >= m * 12 {
                self.r20_ns = Some(p.ns);
            }
            let i = self.px.len() - self.px.iter().rev().take_while(|x| x.0 > p.ns).count();
            self.px.insert(i, (p.ns, p.px));
        }
        if !p.ohlc && !p.vol {
            return;
        }
        let Some(k) = clock.slot(p.ns) else { return };
        if k < self.evict_below {
            self.late.prints_dropped += 1;
            return;
        }
        if k <= self.decided {
            self.late.prints_after_decision += 1;
        }
        // Where slot k is or goes: after every bar of an earlier or equal slot.
        let i = self.bars.len() - self.bars.iter().rev().take_while(|b| b.slot > k).count();
        let j = if i > 0 && self.bars[i - 1].slot == k {
            i - 1
        } else {
            self.bars.insert(
                i,
                Bar {
                    slot: k,
                    has_px: false,
                    h: 0,
                    l: 0,
                    c: 0,
                    c_ns: i64::MIN,
                    n_vol: 0,
                    dollars: 0.0,
                },
            );
            i
        };
        let b = &mut self.bars[j];
        if p.ohlc {
            let x = px32(p.px);
            if !b.has_px {
                b.has_px = true;
                b.h = x;
                b.l = x;
            }
            b.h = b.h.max(x);
            b.l = b.l.min(x);
            if p.ns >= b.c_ns {
                b.c = x;
                b.c_ns = p.ns;
            }
        }
        if p.vol {
            b.n_vol = b.n_vol.saturating_add(1);
            let dol = p.px as f64 / 1e6 * p.size as f64;
            b.dollars += dol as f32;
        }
    }

    /// The decision time of the oldest bar not decided yet.
    pub fn next_due(&self, clock: &Clock) -> Option<i64> {
        self.bars
            .iter()
            .find(|b| b.slot > self.decided)
            .map(|b| clock.slot_end(b.slot) + SETTLE)
    }

    /// Decides, in slot order, every bar whose decision time is at or before `now`. Returns each
    /// bar that passed the trigger (steps 2–4), with its universe, budget and gate verdicts.
    /// Without `facts` nothing can pass (the research skips a symbol without C1).
    pub fn decide_due(
        &mut self,
        now: i64,
        clock: &Clock,
        facts: Option<&Facts>,
        sec: SecType,
    ) -> Vec<Cand> {
        let mut out = Vec::new();
        while let Some(idx) = self.bars.iter().position(|b| b.slot > self.decided) {
            let k = self.bars[idx].slot;
            let b_end = clock.slot_end(k);
            let t = b_end + SETTLE;
            if t > now {
                break;
            }
            self.decided = k;
            if let Some(c) = self.decide(idx, k, b_end, t, clock, facts, sec) {
                out.push(c);
            }
            // Nothing before this window can be read again: later windows start later.
            let a = k + 1 - WINDOW_SLOTS;
            while self.bars.front().is_some_and(|b| b.slot < a) {
                let b = self.bars.pop_front().expect("front");
                self.evicted_cum += b.dollars as f64;
            }
            self.evict_below = self.evict_below.max(a);
            // Keep the last OHLC print before t (the next decision's price may be it).
            while self.px.len() >= 2 && self.px[1].0 < t {
                self.px.pop_front();
            }
        }
        out
    }

    #[allow(clippy::too_many_arguments)]
    fn decide(
        &mut self,
        idx: usize,
        k: i64,
        b_end: i64,
        t: i64,
        clock: &Clock,
        facts: Option<&Facts>,
        sec: SecType,
    ) -> Option<Cand> {
        if t >= clock.session_end || sec == SecType::Excluded {
            return None;
        }
        let f = facts?;
        if f.c1.is_none_or(|c| c <= 0.0) {
            return None;
        }
        let bar = self.bars[idx];
        if !bar.has_px {
            return None;
        }
        if !(BAND_LO_U..=BAND_HI_U).contains(&(bar.c as i64)) {
            return None;
        }
        if self.r20_ns.is_some_and(|r| r < b_end) {
            return None;
        }
        if k + 1 < WINDOW_SLOTS {
            return None;
        }
        let a = k + 1 - WINDOW_SLOTS;
        let (mut hi, mut lo) = (0u32, u32::MAX);
        let mut prints = 0u32;
        // The running dollar sum in slot order, as `derive` builds `cum_dollars`.
        let mut cum = self.evicted_cum;
        let mut before: Option<f64> = None;
        for x in self.bars.iter().take(idx + 1) {
            if x.slot < a {
                cum += x.dollars as f64;
                continue;
            }
            if before.is_none() {
                before = Some(cum);
            }
            cum += x.dollars as f64;
            prints += x.n_vol as u32;
            if x.has_px {
                hi = hi.max(x.h);
                lo = lo.min(x.l);
            }
        }
        if hi == 0 || lo == u32::MAX || lo == 0 {
            return None;
        }
        let dollars = cum - before.unwrap_or(0.0);
        if prints < PRINTS_MIN
            || dollars < DOLLARS_MIN
            || (bar.c as f64) < CLOSE_VS_HIGH * hi as f64
        {
            return None;
        }
        let dadv_min = f
            .dadv20
            .filter(|d| d.is_finite() && *d > 0.0)
            .map(|d| d / DADV_MINUTES);
        let rel = dadv_min.map(|d| dollars / WINDOW_MIN as f64 / d);
        let mut c = Cand {
            t,
            close_u: bar.c,
            low_u: lo,
            high_u: hi,
            prints,
            dollars,
            rel,
            close_ns: bar.c_ns,
            in_universe: false,
            kept: false,
            fire: false,
        };
        if c.gain() < GAIN_MIN || c.rel.is_some_and(|r| r < REL_MIN) {
            return None;
        }
        c.in_universe = self.universe_at(t, clock);
        if c.in_universe {
            let refractory = self.last_kept.is_some_and(|l| t - l < REFRACTORY);
            if !refractory && self.kept < PER_SYM {
                self.kept += 1;
                self.last_kept = Some(t);
                c.kept = true;
                c.fire = clock.in_gate(t);
            }
        }
        Some(c)
    }

    /// `universe::check` at `t` (the type was checked by the caller).
    fn universe_at(&self, t: i64, clock: &Clock) -> bool {
        if t < clock.t0 || t >= clock.session_end {
            return false;
        }
        let Some(&(ns, px)) = self.px.iter().rev().find(|x| x.0 < t) else {
            return false;
        };
        t - ns <= STALE_NS && (BAND_LO_U..=BAND_HI_U).contains(&px)
    }

    /// Bars held (for the cost line).
    pub fn held(&self) -> usize {
        self.bars.len() + self.px.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const T0: i64 = 1_700_000_000 * SEC;

    fn clock() -> Clock {
        Clock {
            t0: T0,
            session_end: T0 + 16 * 60 * MIN,
        }
    }

    fn p(ns: i64, px: i64, size: u64) -> Print {
        Print {
            ns,
            px,
            size,
            ohlc: true,
            vol: true,
        }
    }

    fn facts() -> Facts {
        Facts {
            c1: Some(2.0),
            dadv20: None,
        }
    }

    /// A 3-minute climb from $2.00 to $2.14 ending in the slot that ends at `end`: 12 bars of
    /// 3 prints of 1,000 shares.
    fn climb(s: &mut Sym, c: &Clock, end: i64) {
        let k_end = (end - c.t0) / SLOT - 1;
        for (i, k) in (k_end - 11..=k_end).enumerate() {
            let px = 2_000_000 + i as i64 * 12_727;
            for j in 0..3 {
                s.apply(&p(c.t0 + k * SLOT + j * SEC, px, 1_000), c);
            }
        }
    }

    #[test]
    fn a_climb_inside_the_gate_fires_once_then_refractory_then_a_second() {
        let c = clock();
        let mut s = Sym::new();
        // R20 must not have happened: a quiet $2.00 base first.
        s.apply(&p(c.t0 + 300 * MIN, 2_000_000, 100), &c);
        let end = c.t0 + 331 * MIN; // 09:31
        climb(&mut s, &c, end);
        let got = s.decide_due(end + SETTLE, &c, Some(&facts()), SecType::Common);
        let fires: Vec<_> = got.iter().filter(|x| x.fire).collect();
        assert_eq!(fires.len(), 1, "{got:?}");
        // The 11th bar is already +6.4 % off the low: the first bar that passes fires.
        assert_eq!(fires[0].t, end - SLOT + SETTLE);
        assert!(fires[0].gain() >= GAIN_MIN);
        // A later climb inside 20 minutes is refractory.
        let mut got2 = Vec::new();
        for m in 1..30 {
            let e = end + m * MIN;
            climb(&mut s, &c, e);
            got2.extend(s.decide_due(e + SETTLE, &c, Some(&facts()), SecType::Common));
        }
        assert!(got2.iter().filter(|x| x.fire).count() <= 1);
    }

    #[test]
    fn nothing_without_c1_or_for_an_excluded_type_or_before_the_gate() {
        let c = clock();
        for (f, sec, end) in [
            (None, SecType::Common, c.t0 + 331 * MIN),
            (
                Some(Facts {
                    c1: None,
                    dadv20: None,
                }),
                SecType::Common,
                c.t0 + 331 * MIN,
            ),
            (Some(facts()), SecType::Excluded, c.t0 + 331 * MIN),
        ] {
            let mut s = Sym::new();
            climb(&mut s, &c, end);
            let got = s.decide_due(end + SETTLE, &c, f.as_ref(), sec);
            assert!(got.is_empty(), "{got:?}");
        }
        // Before 09:30 it is kept (it uses the budget) but is not a fire.
        let mut s = Sym::new();
        let end = c.t0 + 320 * MIN;
        climb(&mut s, &c, end);
        let got = s.decide_due(end + SETTLE, &c, Some(&facts()), SecType::Common);
        assert!(got.iter().any(|x| x.kept && !x.fire), "{got:?}");
    }

    #[test]
    fn r20_before_the_bar_end_blocks_and_a_missing_dadv20_passes() {
        let c = clock();
        let mut s = Sym::new();
        s.apply(&p(c.t0 + MIN, 1_000_000, 100), &c);
        s.apply(&p(c.t0 + 2 * MIN, 1_300_000, 100), &c); // +30 %: R20 at 04:02
        let end = c.t0 + 331 * MIN;
        climb(&mut s, &c, end);
        assert!(
            s.decide_due(end + SETTLE, &c, Some(&facts()), SecType::Common)
                .is_empty()
        );
        // A huge DADV20 blocks the relative test; none passes it.
        let mut s = Sym::new();
        climb(&mut s, &c, end);
        let big = Facts {
            c1: Some(2.0),
            dadv20: Some(1e12),
        };
        assert!(
            s.decide_due(end + SETTLE, &c, Some(&big), SecType::Common)
                .is_empty()
        );
    }

    #[test]
    fn facts_follow_the_reference_rule() {
        let b = |c: f64, v: f64| DailyIn {
            h: Some(c),
            l: Some(c),
            c: Some(c),
            v: Some(v),
            vw: None,
        };
        let nine: Vec<_> = (0..9).map(|_| b(2.0, 1000.0)).collect();
        assert_eq!(
            facts_of(&nine),
            Facts {
                c1: Some(2.0),
                dadv20: None
            }
        );
        let mut many: Vec<_> = (0..25).map(|i| b(1.0 + i as f64, 100.0)).collect();
        many.push(DailyIn {
            h: None,
            ..b(9.0, 1.0)
        });
        let f = facts_of(&many);
        assert_eq!(
            f.c1,
            Some(25.0),
            "the last bar without a high does not count"
        );
        let want = (6..26).map(|i| 100.0 * i as f64).sum::<f64>() / 20.0;
        assert_eq!(f.dadv20, Some(want));
    }

    #[test]
    fn sec_types_match_universe_v1() {
        assert_eq!(
            sec_type("SPY", Some("SPDR S&P 500 ETF Trust")),
            SecType::Excluded
        );
        assert_eq!(
            sec_type("ARE", Some("Alexandria Real Estate Equities Trust")),
            SecType::Common
        );
        assert_eq!(sec_type("AAC.U", None), SecType::Excluded);
        assert_eq!(sec_type("BRK.B", None), SecType::Unknown);
        assert_eq!(
            sec_type("X", Some("United States Steel Corporation")),
            SecType::Common
        );
        assert_eq!(
            sec_type("XYZP", Some("XYZ 6.5% Notes due 2028")),
            SecType::Excluded
        );
    }

    #[test]
    fn a_late_print_lands_in_its_bar_and_the_close_is_the_latest_execution() {
        let c = clock();
        let mut s = Sym::new();
        s.apply(&p(c.t0 + 5 * SEC, 2_000_000, 100), &c);
        s.apply(&p(c.t0 + 3 * SEC, 1_900_000, 100), &c);
        assert_eq!(s.bars.len(), 1);
        assert_eq!(
            (s.bars[0].c, s.bars[0].l, s.bars[0].n_vol),
            (2_000_000, 1_900_000, 2)
        );
        assert_eq!(
            s.px.iter().map(|x| x.0).collect::<Vec<_>>(),
            vec![c.t0 + 3 * SEC, c.t0 + 5 * SEC]
        );
    }
}

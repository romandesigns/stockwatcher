//! F1u, "first pullback after ignition", the detector itself: std only, no server types, so the
//! research worktree compiles THIS FILE unchanged (`#[path]`) and replays its own tape through it
//! (the parity harness, `C:/wavystack/outputs/stockwatcher-c4-live/`). Everything that decides
//! whether F1u fires lives here; `super` only feeds it prints, halts, a clock and the day's
//! facts, and turns a [`Cand`] into a notice.
//!
//! The research definition (branch `research/engine-a-forward`, frozen candidate
//! `research/engine-a/r2/T2/frozen/T2-c4.json`, component `F1u` = finalist
//! `research/engine-a/finalists/F1.json` with the day cap removed), step by step with the
//! research's own arithmetic:
//!
//! 1. **Entrant E4** `ign=0.1;depth=0.382;bar=60;win=10;trig=bar_high` (`e4_pullback.rs`
//!    `scan_symbol`, frozen `research/engine-a/frozen/E4.json`). Per symbol, the 15 s bars of the
//!    day's prints on the 04:00 ET grid (`store.rs::build_symbol`: any print opens its slot; o/h/l/c
//!    in integer micro-dollars from OHLC-counted prints, the close the latest execution, the open
//!    the earliest; volume-counted prints counted as a saturating `u16` and their dollars summed in
//!    `f32`), grouped into 60 s bars by clock minute (only minutes with a priced 15 s bar). A 60 s
//!    bar ending at `e` is decided at `e + 2 s`:
//!    - **ignition** `close / low60 − 1 ≥ 10 %` with ≥ 25 volume-counted prints and ≥ $25k from
//!      the 15 s bar holding that `low60` through the bar (the difference of the running `f64`
//!      dollar sums, as `derive`'s prefix sums); a new ignition needs the condition to have been
//!      false first;
//!    - **impulse and retreat** over the last 20 bars: `H0` the latest highest high (ending at or
//!      after the ignition bar, not first or last), `L0` the latest lowest low before it, impulse
//!      ≥ 5 % and ≥ 3 increments, coverage ≥ 75 %, no halt overlapping, formation ≤ 8 bars; the
//!      pullback low `Lp` above `L0`, depth `(H0 − Lp)/(H0 − L0)` ≤ 0.382;
//!    - **arming** after ≥ 2 retreat bars, within 10 minutes of the ignition bar: the trigger is
//!      the latest retreat bar's high, armed for 8 bars; a print below `Lp` ends it;
//!    - **trigger** on 1 s bars when the symbol's activity gate (day gain from C1 ≥ 5 % or the
//!      20-slot range r5 ≥ 5 % at a 15 s bar end, in band, or a halt) had opened by the arming,
//!      else on 15 s bars: two distinct prints above the level in the bar, or the close above and
//!      held (1 s: no print at or below in the next 2 s, decided 1 s after; 15 s: the next slot
//!      untraded or above, decided 2 s after);
//!    - only the first pullback after each ignition; score `(H0/L0 − 1) / max(depth, 0.1)`; the
//!      price at the fire in $0.10–$25.
//! 2. **Universe at `t`** (`universe::check`): the last OHLC print strictly before `t` in
//!    $0.10–$25 and at most 60 s old, `t` inside 04:00 → session end (the type and C1 are the
//!    caller's: an excluded type or a missing C1 never gets a [`Sym`] fed).
//! 3. **Filter 10** `ext_base ≤ 0.20` (`filters.rs::features`): the last priced 15 s close over
//!    the lowest 15 s low so far today, bars ended at or before `t − 2 s`; unknown is kept.
//! 4. **E4's frozen budget, day cap removed** (`alerts::pre_cap`): score ≥ θ20
//!    (0.34985581794092424), one per symbol-second, refractory 30 min, 2 a stock a day, over the
//!    whole day from 04:00 (premarket candidates use the budget).
//! 5. **T2-c4's gate** 09:30–11:30 ET on `t`: only a kept candidate inside it is a fire.
//!
//! What the research did after this, and live does not: the J3 twin fold (an archive artefact)
//! and T2-c4's union rule (an add-on row at a symbol-second production already used is dropped).
//! One known causal difference, measured in the parity report: the research store opened a
//! symbol's 1 s gate at its FIRST HALT OF THE DAY whenever it had one, even after an earlier price
//! trigger (`build_symbol` breaks out of the price scan when a halt exists); live cannot know a
//! later halt, so it opens at the earlier of the two.

use std::collections::VecDeque;

pub const SEC: i64 = 1_000_000_000;
pub const MIN: i64 = 60 * SEC;
/// The store's 15 s grid and its slot count from 04:00 (`tape::NSLOT`).
pub const SLOT: i64 = 15 * SEC;
pub const NSLOT: i64 = 3840;
/// E4's `bar=60`: four 15 s slots by clock minute.
pub const BAR: i64 = 60 * SEC;
const PER: i64 = 4;
pub const G15_SETTLE: i64 = 2 * SEC;
pub const G1_SETTLE: i64 = SEC;
/// E4's frozen parameters (`ign=0.1;depth=0.382;bar=60;win=10;trig=bar_high`).
pub const IGN: f64 = 0.1;
pub const DEPTH_MAX: f64 = 0.382;
pub const WIN: i64 = 10 * MIN;
/// E4's fixed rules (MP5 detection-v2).
pub const LOOKBACK_BARS: i64 = 20;
pub const IMPULSE_MIN: f64 = 0.05;
pub const IMPULSE_MIN_INCREMENTS: i64 = 3;
pub const COVERAGE_MIN: f64 = 0.75;
pub const MIN_PULLBACK_BARS: usize = 2;
pub const FORMATION_MAX_BARS: i64 = 8;
pub const ARMED_MAX_BARS: i64 = 8;
pub const FRESH: i64 = 60 * SEC;
pub const IGN_TRADES: u64 = 25;
pub const IGN_DOLLARS: f64 = 25_000.0;
/// low60's window: 60 minutes of 15 s slots.
const SLOTS_60M: i64 = 240;
/// The store's 1 s activity gate.
pub const GATE_GAIN: f64 = 0.05;
pub const GATE_R5: f64 = 0.05;
const R5_SLOTS: i64 = 20;
/// Filter 10.
pub const EXT_BASE_MAX: f64 = 0.20;
const EXT_SETTLE: i64 = 2 * SEC;
/// E4's frozen Train θ20 and budget (`research/engine-a/frozen/theta20-E4.json`, `E4.json`).
pub const THETA: f64 = 0.349_855_817_940_924_24;
pub const PER_SYM: u8 = 2;
pub const REFRACTORY: i64 = 30 * MIN;
/// `universe-v1`'s band (micro-dollars, inclusive) and staleness.
pub const BAND_LO_U: i64 = 100_000;
pub const BAND_HI_U: i64 = 25_000_000;
pub const STALE_NS: i64 = 60 * SEC;
/// T2-c4's add-on gate `0930-1130`, as offsets from 04:00 ET: [09:30, 11:30).
pub const GATE_FROM: i64 = 330 * MIN;
pub const GATE_TO: i64 = 450 * MIN;
/// What the fires are, carried on every notice.
pub const CONFIG: &str =
    "F1u: E4 ign=0.1;depth=0.382;bar=60;win=10;trig=bar_high + ext_base<=0.2, uncapped";
/// 15 s bars kept behind the newest built minute: low60 and the ignition search read 240.
const KEEP_SLOTS: i64 = SLOTS_60M + 12;
/// 60 s bars kept: the 20-bar lookback reads 19 minutes back.
const KEEP_BARS_NS: i64 = 30 * MIN;
/// 1 s bars kept while not armed (arming reads only seconds from its own instant).
const KEEP_SECS_NS: i64 = 30 * SEC;
/// OHLC prints kept for the universe price (the last one before `t`, at most 60 s old).
const KEEP_PX_NS: i64 = 120 * SEC;

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

/// One print with the chart engine's counting verdict (`tape::Print`). `entry` is the research's
/// ruling-R4 flag (OHLC-counted, or volume-only for the odd-lot flag alone); it only places a
/// halt's reopening print.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Print {
    pub ns: i64,
    /// Micro-dollars.
    pub px: i64,
    pub size: u64,
    pub ohlc: bool,
    pub vol: bool,
    pub entry: bool,
}

/// Micro-dollars as the store keeps them (`store::px32`).
fn px32(u: i64) -> u32 {
    u.clamp(1, u32::MAX as i64) as u32
}

/// One price increment: $0.01 at $1 and above, $0.0001 below (`e4_pullback::increment`).
fn increment(px_u: u32) -> i64 {
    if px_u >= 1_000_000 { 10_000 } else { 100 }
}

/// Two distinct counted prints above `lvl` inside one bar (`e4_pullback::two_above`).
fn two_above(o: u32, h: u32, c: u32, n_ohlc: Option<u16>, lvl: u32) -> bool {
    (h > lvl && c > lvl && h > c)
        || (o > lvl && h > lvl && h > o)
        || (n_ohlc.is_some_and(|n| n >= 2) && o > lvl && c > lvl)
}

#[derive(Clone, Copy, Debug)]
struct B15 {
    slot: i64,
    has_px: bool,
    o: u32,
    o_ns: i64,
    h: u32,
    l: u32,
    c: u32,
    c_ns: i64,
    n_ohlc: u16,
    n_vol: u16,
    dollars: f32,
}

/// One priced 60 s bar (`e4_pullback::B`), with its ignition verdict (a pure function of the
/// bars up to its end, so computed once when the minute is built).
#[derive(Clone, Copy, Debug)]
struct B60 {
    end: i64,
    h: u32,
    l: u32,
    ign: bool,
}

/// One 1 s bar of OHLC-counted prints (`store::Sec1`; only seconds with a price are rows).
#[derive(Clone, Copy, Debug)]
struct S1 {
    sec: i64,
    o: u32,
    o_ns: i64,
    h: u32,
    l: u32,
    c: u32,
    c_ns: i64,
}

#[derive(Clone, Copy, Debug)]
enum St {
    Idle {
        reset: bool,
    },
    Ignited {
        ign_end: i64,
    },
    Forming {
        ign_end: i64,
        h0: u32,
        h0_end: i64,
        l0: u32,
        lp: u32,
        n_after: usize,
    },
}

#[derive(Clone, Copy, Debug)]
struct Armed {
    /// End of the arming bar.
    x_end: i64,
    lvl: u32,
    lp: u32,
    h0: u32,
    l0: u32,
    ign_end: i64,
    armed_at: i64,
    deadline: i64,
    g1: bool,
}

enum Outcome {
    Fired { t: i64, px: u32, g1: bool },
    Ended(i64),
}

/// A trading halt as the detector knows it at a given moment (`store::HaltRow`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Halt {
    pub halt_ns: i64,
    /// The resumption status (listed), once known.
    pub resume_ns: Option<i64>,
    /// The first entry print after the halt and the resumption.
    pub reopen_ns: Option<i64>,
}

/// One E4 fire and its verdicts (the parity harness reads every field).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Cand {
    /// Decision time, epoch ns.
    pub t: i64,
    pub px_u: u32,
    pub score: f64,
    pub h0: u32,
    pub l0: u32,
    pub lp: u32,
    pub trig_u: u32,
    pub ign_end: i64,
    pub armed_at: i64,
    /// Triggered on 1 s bars (else 15 s).
    pub g1: bool,
    /// `universe-v1` at `t`.
    pub in_universe: bool,
    /// Filter 10's input (`None`: no priced bar yet, kept).
    pub ext_base: Option<f64>,
    pub ext_ok: bool,
    /// Kept by the budget (only an in-universe, filter-kept candidate can be).
    pub kept: bool,
    /// Kept and inside 09:30–11:30 ET: a fire.
    pub fire: bool,
}

/// Late or skipped input, counted (never silently different from the research).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Late {
    /// A print for a 15 s slot already dropped from the window.
    pub prints_dropped: u64,
    /// A print that landed in a minute already built (it still counts for later reads).
    pub prints_after_decision: u64,
}

/// One symbol's day (module docs).
#[derive(Clone, Debug)]
pub struct Sym {
    b15: VecDeque<B15>,
    /// Running sums of the bars dropped from `b15`, in slot order (`derive`'s prefix sums).
    ev_n: u64,
    ev_d: f64,
    /// Lowest low and last close of the dropped priced bars (filter 10's "so far today").
    ev_low: Option<u32>,
    ev_close: Option<u32>,
    ev_below: i64,
    m60: Vec<B60>,
    /// Minutes (from 04:00) below this are built.
    built_min: i64,
    st: St,
    resume_after: i64,
    armed: Option<Armed>,
    /// Past the session end: nothing more is decided.
    stopped: bool,
    secs: VecDeque<S1>,
    px: VecDeque<(i64, i64)>,
    /// The activity gate from the price rule: the first bar end that met it.
    gate_px: Option<i64>,
    /// Slots below this were checked for it; the last close seen in that scan.
    gate_slot: i64,
    gate_close: Option<u32>,
    halts: Vec<Halt>,
    kept: u8,
    last_kept: Option<(i64, i64)>,
    pub late: Late,
}

impl Default for Sym {
    fn default() -> Self {
        Sym::new()
    }
}

impl Sym {
    pub fn new() -> Self {
        Sym {
            b15: VecDeque::new(),
            ev_n: 0,
            ev_d: 0.0,
            ev_low: None,
            ev_close: None,
            ev_below: 0,
            m60: Vec::new(),
            built_min: 0,
            st: St::Idle { reset: false },
            resume_after: i64::MIN,
            armed: None,
            stopped: false,
            secs: VecDeque::new(),
            px: VecDeque::new(),
            gate_px: None,
            gate_slot: 0,
            gate_close: None,
            halts: Vec::new(),
            kept: 0,
            last_kept: None,
            late: Late::default(),
        }
    }

    /// A halt began at `halt_ns` (the trading-status feed's clock).
    pub fn halt_start(&mut self, halt_ns: i64) {
        if !self.halts.iter().any(|h| h.halt_ns == halt_ns) {
            self.halts.push(Halt {
                halt_ns,
                resume_ns: None,
                reopen_ns: None,
            });
        }
    }

    /// The halt that began at `halt_ns` resumed at `resume_ns`.
    pub fn halt_resume(&mut self, halt_ns: i64, resume_ns: i64) {
        self.halt_start(halt_ns);
        if let Some(h) = self.halts.iter_mut().find(|h| h.halt_ns == halt_ns) {
            h.resume_ns = Some(resume_ns);
        }
    }

    pub fn halts(&self) -> &[Halt] {
        &self.halts
    }

    /// One print. O(1) but for a late print, placed by a short walk from the back.
    pub fn apply(&mut self, p: &Print, clock: &Clock) {
        if p.entry {
            for h in self.halts.iter_mut() {
                if let Some(r) = h.resume_ns
                    && h.reopen_ns.is_none()
                    && p.ns > h.halt_ns
                    && p.ns >= h.halt_ns.max(r)
                {
                    h.reopen_ns = Some(p.ns);
                }
            }
        }
        if p.ohlc {
            let i = self.px.len() - self.px.iter().rev().take_while(|x| x.0 > p.ns).count();
            self.px.insert(i, (p.ns, p.px));
            if p.ns < clock.session_end {
                self.apply_sec(p);
            }
        }
        let Some(k) = clock.slot(p.ns) else { return };
        if k < self.ev_below {
            self.late.prints_dropped += 1;
            return;
        }
        if k / PER < self.built_min {
            self.late.prints_after_decision += 1;
        }
        let i = self.b15.len() - self.b15.iter().rev().take_while(|b| b.slot > k).count();
        let j = if i > 0 && self.b15[i - 1].slot == k {
            i - 1
        } else {
            self.b15.insert(
                i,
                B15 {
                    slot: k,
                    has_px: false,
                    o: 0,
                    o_ns: i64::MAX,
                    h: 0,
                    l: 0,
                    c: 0,
                    c_ns: i64::MIN,
                    n_ohlc: 0,
                    n_vol: 0,
                    dollars: 0.0,
                },
            );
            i
        };
        let b = &mut self.b15[j];
        if p.ohlc {
            let x = px32(p.px);
            if !b.has_px {
                b.has_px = true;
                b.h = x;
                b.l = x;
            }
            b.h = b.h.max(x);
            b.l = b.l.min(x);
            if p.ns < b.o_ns {
                b.o = x;
                b.o_ns = p.ns;
            }
            if p.ns >= b.c_ns {
                b.c = x;
                b.c_ns = p.ns;
            }
            b.n_ohlc = b.n_ohlc.saturating_add(1);
        }
        if p.vol {
            b.n_vol = b.n_vol.saturating_add(1);
            let dol = p.px as f64 / 1e6 * p.size as f64;
            b.dollars += dol as f32;
        }
    }

    fn apply_sec(&mut self, p: &Print) {
        let s = p.ns.div_euclid(SEC);
        let x = px32(p.px);
        let i = self.secs.len() - self.secs.iter().rev().take_while(|r| r.sec > s).count();
        if i > 0 && self.secs[i - 1].sec == s {
            let r = &mut self.secs[i - 1];
            r.h = r.h.max(x);
            r.l = r.l.min(x);
            if p.ns < r.o_ns {
                r.o = x;
                r.o_ns = p.ns;
            }
            if p.ns >= r.c_ns {
                r.c = x;
                r.c_ns = p.ns;
            }
        } else {
            self.secs.insert(
                i,
                S1 {
                    sec: s,
                    o: x,
                    o_ns: p.ns,
                    h: x,
                    l: x,
                    c: x,
                    c_ns: p.ns,
                },
            );
        }
    }

    /// When something can next be decided: the armed watch each second, else the next minute
    /// holding a bar (2 s after it ends).
    pub fn next_due(&self, clock: &Clock, now: i64) -> Option<i64> {
        if self.stopped {
            return None;
        }
        if self.armed.is_some() {
            return Some(now - now.rem_euclid(SEC) + SEC);
        }
        let first = self.b15.iter().find(|b| b.slot / PER >= self.built_min)?;
        Some(clock.t0 + (first.slot / PER + 1) * BAR + G15_SETTLE)
    }

    /// Decides everything decidable at `now`: the armed watch, then each minute whose bar is due.
    /// `c1` is the previous close (the gate's day gain).
    pub fn advance(&mut self, now: i64, clock: &Clock, c1: Option<f64>) -> Vec<Cand> {
        let mut out = Vec::new();
        loop {
            if self.stopped {
                break;
            }
            if let Some(a) = self.armed {
                let Some(o) = self.resolve(&a, now, clock) else {
                    break;
                };
                self.armed = None;
                self.st = St::Idle { reset: true };
                match o {
                    Outcome::Fired { t, px, g1 } => {
                        if t < clock.session_end && (BAND_LO_U..=BAND_HI_U).contains(&(px as i64)) {
                            let depth = (a.h0 - a.lp) as f64 / (a.h0 - a.l0) as f64;
                            let c = self.verdicts(
                                Cand {
                                    t,
                                    px_u: px,
                                    score: (a.h0 as f64 / a.l0 as f64 - 1.0) / depth.max(0.1),
                                    h0: a.h0,
                                    l0: a.l0,
                                    lp: a.lp,
                                    trig_u: a.lvl,
                                    ign_end: a.ign_end,
                                    armed_at: a.armed_at,
                                    g1,
                                    in_universe: false,
                                    ext_base: None,
                                    ext_ok: false,
                                    kept: false,
                                    fire: false,
                                },
                                clock,
                            );
                            out.push(c);
                        }
                        self.resume_after = t - G15_SETTLE.max(G1_SETTLE);
                    }
                    Outcome::Ended(te) => self.resume_after = te,
                }
                continue;
            }
            // The next minute holding a 15 s bar, once its bar is due.
            let Some(g) = self
                .b15
                .iter()
                .find(|b| b.slot / PER >= self.built_min)
                .map(|b| b.slot / PER)
            else {
                break;
            };
            let end = clock.t0 + (g + 1) * BAR;
            if end + G15_SETTLE > now {
                break;
            }
            self.gate_through(end, clock, c1);
            let built = self.build(g, end);
            self.built_min = g + 1;
            if built {
                self.step(clock);
            }
            self.evict(clock);
        }
        out
    }

    /// The price rule of the activity gate over the 15 s bars ending at or before `through`
    /// (`build_symbol`, "the activity gate at each bar end").
    fn gate_through(&mut self, through: i64, clock: &Clock, c1: Option<f64>) {
        if self.gate_px.is_some() {
            return;
        }
        let idx: Vec<usize> = (0..self.b15.len())
            .filter(|&i| {
                let b = &self.b15[i];
                b.slot >= self.gate_slot && clock.slot_end(b.slot) <= through
            })
            .collect();
        for i in idx {
            let b = self.b15[i];
            if b.has_px {
                self.gate_close = Some(b.c);
            }
            self.gate_slot = b.slot + 1;
            let end = clock.slot_end(b.slot);
            let px = self.gate_close.unwrap_or(0) as i64;
            let in_band = (BAND_LO_U..=BAND_HI_U).contains(&px);
            let g = c1.filter(|c| *c > 0.0).map(|c| px as f64 / 1e6 / c - 1.0);
            if in_band && (g.is_some_and(|g| g >= GATE_GAIN) || self.r5(i) as f64 >= GATE_R5) {
                self.gate_px = Some(end);
                return;
            }
        }
    }

    /// `derive`'s r5 of the bar at `i`: high/low range of the priced bars of the 20 slots ending
    /// at it (its own high even unpriced, which is 0).
    fn r5(&self, i: usize) -> f32 {
        let b = &self.b15[i];
        let k = b.slot;
        let (mut hi, mut lo) = (b.h, if b.has_px { b.l } else { u32::MAX });
        for x in self
            .b15
            .iter()
            .take(i)
            .rev()
            .take_while(|x| x.slot + R5_SLOTS > k)
        {
            if x.has_px {
                hi = hi.max(x.h);
                lo = lo.min(x.l);
            }
        }
        if hi > 0 && lo < u32::MAX {
            (hi as f64 / lo as f64 - 1.0) as f32
        } else {
            0.0
        }
    }

    /// The gate open at `t`: the price rule's bar end or the first halt from 04:00, whichever
    /// came first (module docs: the research took the halt whenever the day had one).
    fn gate_open(&self, t: i64, clock: &Clock) -> bool {
        let halt = self
            .halts
            .iter()
            .map(|h| h.halt_ns)
            .filter(|&x| x >= clock.t0)
            .min();
        let g = match (self.gate_px, halt) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        g.is_some_and(|g| g <= t)
    }

    /// `e3_vwap::halted_within`.
    fn halted(&self, from: i64, t: i64) -> bool {
        self.halts
            .iter()
            .any(|h| h.halt_ns <= t && h.reopen_ns.is_none_or(|r| r > t || r >= from))
    }

    /// Builds minute `g`'s 60 s bar (`bars_of_size`) with its ignition verdict. False when the
    /// minute has no priced 15 s bar (no 60 s bar then).
    fn build(&mut self, g: i64, end: i64) -> bool {
        let Some(first) = self.b15.iter().position(|b| b.slot / PER == g) else {
            return false;
        };
        let last = first
            + self
                .b15
                .iter()
                .skip(first)
                .take_while(|b| b.slot / PER == g)
                .count()
            - 1;
        let (mut h, mut l, mut c, mut any) = (0u32, u32::MAX, 0u32, false);
        for b in self.b15.range(first..=last) {
            if b.has_px {
                h = h.max(b.h);
                l = l.min(b.l);
                c = b.c;
                any = true;
            }
        }
        if !any {
            return false;
        }
        // low60 of the minute's last 15 s bar: the lowest priced low of the slots
        // (slot − 240, slot].
        let k_slot = self.b15[last].slot;
        let low60 = self
            .b15
            .range(..=last)
            .rev()
            .take_while(|b| b.slot + SLOTS_60M > k_slot)
            .filter(|b| b.has_px)
            .map(|b| b.l)
            .min()
            .unwrap_or(0);
        let ign = low60 > 0 && c as f64 / low60 as f64 - 1.0 >= IGN && {
            let from = (0..=last)
                .rev()
                .take_while(|&j| self.b15[j].slot + SLOTS_60M > k_slot)
                .find(|&j| self.b15[j].has_px && self.b15[j].l == low60);
            from.is_some_and(|j| {
                // pn / pd as prefix sums over every 15 s bar of the day, in slot order.
                let (mut n, mut d) = (self.ev_n, self.ev_d);
                let (mut n_j, mut d_j) = (0u64, 0f64);
                for (i, b) in self.b15.range(..=last).enumerate() {
                    if i == j {
                        n_j = n;
                        d_j = d;
                    }
                    n += b.n_vol as u64;
                    d += b.dollars as f64;
                }
                n - n_j >= IGN_TRADES && d - d_j >= IGN_DOLLARS
            })
        };
        self.m60.push(B60 { end, h, l, ign });
        true
    }

    /// The research loop's body for the newest 60 s bar (`e4_pullback::scan_symbol`).
    fn step(&mut self, clock: &Clock) {
        let i = self.m60.len() - 1;
        let x = self.m60[i];
        if x.end <= self.resume_after {
            return;
        }
        let tau = x.end + G15_SETTLE;
        if tau >= clock.session_end {
            self.stopped = true;
            return;
        }
        let ign_now = x.ign;
        match self.st {
            St::Idle { reset } => {
                if reset {
                    if !ign_now {
                        self.st = St::Idle { reset: false };
                    }
                } else if ign_now {
                    self.st = St::Ignited { ign_end: x.end };
                }
                return;
            }
            St::Forming {
                ign_end,
                h0,
                h0_end,
                l0,
                lp,
                n_after,
            } => {
                if x.h > h0 {
                    self.st = St::Ignited { ign_end };
                } else {
                    let lp = lp.min(x.l);
                    let depth = (h0 - lp) as f64 / (h0 - l0) as f64;
                    if lp <= l0 || depth > DEPTH_MAX {
                        self.st = St::Idle { reset: true };
                        return;
                    }
                    self.st = St::Forming {
                        ign_end,
                        h0,
                        h0_end,
                        l0,
                        lp,
                        n_after: n_after + 1,
                    };
                }
            }
            St::Ignited { .. } => {}
        }
        if let St::Ignited { ign_end } = self.st {
            if x.end - ign_end > WIN {
                self.st = St::Idle { reset: true };
                return;
            }
            let from = x.end - (LOOKBACK_BARS - 1) * BAR;
            let lo = self.m60[..=i].partition_point(|b| b.end < from);
            let w = &self.m60[lo..=i];
            if w.len() >= 3 {
                let j = (0..w.len()).fold(0, |a, k| if w[k].h >= w[a].h { k } else { a });
                let (h0, h0_end) = (w[j].h, w[j].end);
                if j != w.len() - 1
                    && j != 0
                    && h0_end >= ign_end
                    && (x.end - h0_end) / BAR <= FORMATION_MAX_BARS
                {
                    let a = (0..j).fold(0, |a, k| if w[k].l <= w[a].l { k } else { a });
                    let (l0, l0_end) = (w[a].l, w[a].end);
                    let u = h0 as i64 - l0 as i64;
                    let impulse_slots = (h0_end - l0_end) / BAR + 1;
                    let traded = w
                        .iter()
                        .filter(|b| b.end >= l0_end && b.end <= h0_end)
                        .count() as i64;
                    let after = &w[j + 1..];
                    let (lp, _) = after.iter().fold((after[0].l, 0), |acc, b| {
                        if b.l <= acc.0 { (b.l, 0) } else { acc }
                    });
                    let depth = if u > 0 {
                        (h0 as i64 - lp as i64) as f64 / u as f64
                    } else {
                        f64::INFINITY
                    };
                    let n_after = after.len();
                    if l0 > 0
                        && u > 0
                        && u as f64 >= l0 as f64 * IMPULSE_MIN
                        && u >= IMPULSE_MIN_INCREMENTS * increment(l0)
                        && traded as f64 >= COVERAGE_MIN * impulse_slots as f64
                        && !self.halted(l0_end - BAR, x.end)
                        && lp > l0
                        && depth <= DEPTH_MAX
                    {
                        self.st = St::Forming {
                            ign_end,
                            h0,
                            h0_end,
                            l0,
                            lp,
                            n_after,
                        };
                    }
                }
            }
        }
        if let St::Forming {
            ign_end,
            h0,
            h0_end,
            l0,
            lp,
            n_after,
        } = self.st
        {
            if (x.end - h0_end) / BAR > FORMATION_MAX_BARS || tau - ign_end > WIN {
                self.st = St::Idle { reset: true };
                return;
            }
            if n_after < MIN_PULLBACK_BARS {
                return;
            }
            if self.halted(x.end, x.end) || tau - x.end > FRESH {
                return;
            }
            // `trig=bar_high`: the latest retreat bar's high, fixed at arming.
            self.armed = Some(Armed {
                x_end: x.end,
                lvl: x.h,
                lp,
                h0,
                l0,
                ign_end,
                armed_at: tau,
                deadline: x.end + ARMED_MAX_BARS * BAR,
                g1: self.gate_open(tau, clock),
            });
        }
    }

    /// The armed setup's outcome once the data up to `now` decides it (`watch_g1` /
    /// `watch_g15`), else `None`.
    fn resolve(&self, a: &Armed, now: i64, clock: &Clock) -> Option<Outcome> {
        if a.g1 {
            let start = |s: &S1| s.sec * SEC;
            let rows: Vec<&S1> = self
                .secs
                .iter()
                .filter(|s| start(s) >= a.armed_at)
                .collect();
            for (i, s) in rows.iter().enumerate() {
                let st = start(s);
                // A second is read once it ended and settled.
                if st + SEC + G1_SETTLE > now {
                    return None;
                }
                if st >= a.deadline {
                    return Some(Outcome::Ended(a.deadline));
                }
                if s.l < a.lp {
                    return Some(Outcome::Ended(st + SEC));
                }
                if s.h <= a.lvl {
                    continue;
                }
                let end = st + SEC;
                if two_above(s.o, s.h, s.c, None, a.lvl) {
                    return Some(Outcome::Fired {
                        t: end + G1_SETTLE,
                        px: s.c,
                        g1: true,
                    });
                }
                if s.c > a.lvl {
                    if end + 2 * SEC + G1_SETTLE > now {
                        return None;
                    }
                    let held = rows[i + 1..]
                        .iter()
                        .take_while(|x| start(x) < end + 2 * SEC)
                        .all(|x| x.l > a.lvl);
                    if held {
                        return Some(Outcome::Fired {
                            t: end + 2 * SEC + G1_SETTLE,
                            px: s.c,
                            g1: true,
                        });
                    }
                }
            }
            (now >= a.deadline + G1_SETTLE).then_some(Outcome::Ended(a.deadline))
        } else {
            let rows: Vec<&B15> = self
                .b15
                .iter()
                .filter(|b| clock.slot_end(b.slot) > a.x_end && b.has_px)
                .collect();
            for (i, x) in rows.iter().enumerate() {
                let end = clock.slot_end(x.slot);
                if end + G15_SETTLE > now {
                    return None;
                }
                if end - SLOT >= a.deadline {
                    return Some(Outcome::Ended(a.deadline));
                }
                if x.l < a.lp {
                    return Some(Outcome::Ended(end));
                }
                if x.h <= a.lvl {
                    continue;
                }
                if two_above(x.o, x.h, x.c, Some(x.n_ohlc), a.lvl) {
                    return Some(Outcome::Fired {
                        t: end + G15_SETTLE,
                        px: x.c,
                        g1: false,
                    });
                }
                if x.c > a.lvl {
                    if end + SLOT + G15_SETTLE > now {
                        return None;
                    }
                    let next = rows.get(i + 1).filter(|n| n.slot == x.slot + 1);
                    if next.is_none_or(|n| n.l > a.lvl) {
                        return Some(Outcome::Fired {
                            t: end + SLOT + G15_SETTLE,
                            px: x.c,
                            g1: false,
                        });
                    }
                }
            }
            (now >= a.deadline + G15_SETTLE).then_some(Outcome::Ended(a.deadline))
        }
    }

    /// Steps 2–5 for one E4 fire.
    fn verdicts(&mut self, mut c: Cand, clock: &Clock) -> Cand {
        let t = c.t;
        c.in_universe = t >= clock.t0
            && t < clock.session_end
            && self
                .px
                .iter()
                .rev()
                .find(|x| x.0 < t)
                .is_some_and(|&(ns, px)| {
                    t - ns <= STALE_NS && (BAND_LO_U..=BAND_HI_U).contains(&px)
                });
        c.ext_base = self.ext_base(t, clock);
        c.ext_ok = c.ext_base.is_none_or(|x| x <= EXT_BASE_MAX);
        if c.in_universe && c.ext_ok && c.score.is_finite() && c.score >= THETA {
            let sec = t.div_euclid(SEC);
            match self.last_kept {
                // One per symbol-second: folded into the kept one, not a second alert.
                Some((ls, _)) if ls == sec => {}
                Some((_, lns)) if t - lns < REFRACTORY => {}
                _ if self.kept >= PER_SYM => {}
                _ => {
                    self.kept += 1;
                    self.last_kept = Some((sec, t));
                    c.kept = true;
                    c.fire = clock.in_gate(t);
                }
            }
        }
        c
    }

    /// Filter 10's `ext_base` at `t` (`filters.rs::features`): bars ended at or before `t − 2 s`.
    fn ext_base(&self, t: i64, clock: &Clock) -> Option<f64> {
        let cut = t - EXT_SETTLE;
        let mut last = self.ev_close;
        let mut low = self.ev_low;
        for b in self.b15.iter().filter(|b| clock.slot_end(b.slot) <= cut) {
            if b.has_px {
                last = Some(b.c);
                if b.l > 0 {
                    low = Some(low.map_or(b.l, |x| x.min(b.l)));
                }
            }
        }
        let px = last? as f64 / 1e6;
        let lo = low?;
        Some(px / (lo as f64 / 1e6) - 1.0)
    }

    /// Drops what no later decision reads (module constants).
    fn evict(&mut self, clock: &Clock) {
        let keep_from = self.built_min * PER - KEEP_SLOTS;
        // The g15 watch reads bars after its arming bar: nothing at or after it goes.
        let keep_from = match self.armed {
            Some(a) => keep_from.min((a.x_end - clock.t0) / SLOT),
            None => keep_from,
        };
        while self.b15.front().is_some_and(|b| b.slot < keep_from) {
            let b = self.b15.pop_front().expect("front");
            self.ev_n += b.n_vol as u64;
            self.ev_d += b.dollars as f64;
            if b.has_px {
                self.ev_close = Some(b.c);
                if b.l > 0 {
                    self.ev_low = Some(self.ev_low.map_or(b.l, |x| x.min(b.l)));
                }
            }
            self.ev_below = self.ev_below.max(b.slot + 1);
            self.gate_slot = self.gate_slot.max(b.slot + 1);
        }
        let newest = clock.t0 + self.built_min * BAR;
        let cut = newest - KEEP_BARS_NS;
        let n = self.m60.iter().take_while(|b| b.end < cut).count();
        if n > 64 {
            self.m60.drain(..n);
        }
        let sec_from = match self.armed {
            Some(a) => a.armed_at.min(newest - KEEP_SECS_NS),
            None => newest - KEEP_SECS_NS,
        };
        while self.secs.front().is_some_and(|s| s.sec * SEC < sec_from) {
            self.secs.pop_front();
        }
        let px_from = newest - KEEP_PX_NS;
        while self.px.len() >= 2 && self.px[1].0 < px_from {
            self.px.pop_front();
        }
    }

    /// Whether a setup is armed (the cost line).
    pub fn is_armed(&self) -> bool {
        self.armed.is_some()
    }

    /// Bars and prints held (the cost line).
    pub fn held(&self) -> usize {
        self.b15.len() + self.m60.len() + self.secs.len() + self.px.len()
    }
}

/// T2-c4's HALT add-on test for a bull Halt watcher gauge appearing at `t` (`super::halt_addon`):
/// `universe-v1` at `t` (not an excluded type; the last OHLC print strictly before `t`,
/// `last` = `(exec ns, micro-dollars)`, in $0.10–$25 and at most 60 s old; `t` inside 04:00 →
/// session end) and the gate 09:30–11:30 ET. Here, std only, so the parity harness runs it on
/// the replay's own gauges.
pub fn halt_addon_keeps(t: i64, last: Option<(i64, i64)>, excluded: bool, clock: &Clock) -> bool {
    if excluded || t < clock.t0 || t >= clock.session_end || !clock.in_gate(t) {
        return false;
    }
    last.is_some_and(|(ns, px)| {
        ns < t && t - ns <= STALE_NS && (BAND_LO_U..=BAND_HI_U).contains(&px)
    })
}

impl Sym {
    /// The activity gate as live knows it at `t` (the parity harness reads it).
    pub fn gate_at(&self, t: i64, clock: &Clock) -> bool {
        self.gate_open(t, clock)
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
            entry: true,
        }
    }

    /// Feeds `prints` in time order, deciding before each one as the live clock does.
    fn run(s: &mut Sym, c: &Clock, prints: &[Print], c1: Option<f64>, until: i64) -> Vec<Cand> {
        let mut out = Vec::new();
        for x in prints {
            out.extend(s.advance(x.ns, c, c1));
            s.apply(x, c);
        }
        out.extend(s.advance(until, c, c1));
        out
    }

    /// A $2.00 base from 09:00, a leg to about $2.32 over 09:35–09:39 on heavy prints (the 09:38
    /// minute closes +12.8 % over the base: the ignition), a pullback to $2.22 for two minutes
    /// and a cross of the last retreat bar's high ($2.26) at 09:42:10.
    fn setup(c: &Clock) -> Vec<Print> {
        let mut v = Vec::new();
        let at = |m: i64, s: i64| c.t0 + m * MIN + s * SEC;
        for m in 300..335 {
            for s in [5, 35] {
                v.push(p(at(m, s), 2_000_000, 500));
            }
        }
        // The leg: one minute a step, 10 prints of 1,000 shares each minute.
        for (i, px) in [2_060_000i64, 2_120_000, 2_180_000, 2_240_000, 2_300_000]
            .iter()
            .enumerate()
        {
            for s in 0..10 {
                v.push(p(
                    at(335 + i as i64, s * 5 + 2),
                    *px - 20_000 + s * 4_000,
                    1_000,
                ));
            }
        }
        // Retreat: 09:40 and 09:41, highs $2.27 / $2.26, lows $2.23 / $2.22.
        for (m, hi, lo) in [
            (340, 2_270_000i64, 2_230_000i64),
            (341, 2_260_000, 2_220_000),
        ] {
            v.push(p(at(m, 5), hi, 800));
            v.push(p(at(m, 30), lo, 800));
            v.push(p(at(m, 50), (hi + lo) / 2, 800));
        }
        // The cross: two prints above $2.26 in one second at 09:42:10.
        v.push(p(at(342, 10) + 100_000_000, 2_270_000, 500));
        v.push(p(at(342, 10) + 600_000_000, 2_290_000, 500));
        v.push(p(at(342, 20), 2_290_000, 500));
        v
    }

    #[test]
    fn a_first_pullback_after_ignition_fires_inside_the_gate() {
        let c = clock();
        let mut s = Sym::new();
        let got = run(&mut s, &c, &setup(&c), Some(2.0), c.t0 + 400 * MIN);
        assert_eq!(got.len(), 1, "{got:?}");
        let f = got[0];
        // The day gain opened the 1 s gate long before: the trigger is the 1 s bar of 09:42:10,
        // decided 1 s after it ends.
        assert!(f.g1);
        assert_eq!(f.t, c.t0 + 342 * MIN + 12 * SEC);
        assert_eq!((f.trig_u, f.lp, f.l0), (2_260_000, 2_220_000, 2_000_000));
        assert!(f.ext_base.is_some_and(|x| x < EXT_BASE_MAX), "{f:?}");
        assert!(f.in_universe && f.kept && f.fire, "{f:?}");
        assert!(f.score >= THETA);
    }

    #[test]
    fn filter_10_drops_an_extended_stock_and_the_budget_keeps_two() {
        let c = clock();
        // The same day after a $1.50 low at 07:20: +49 % over the day's low, so ext_base > 0.2.
        let low: Vec<Print> = (0..4)
            .map(|i| p(c.t0 + 200 * MIN + i * MIN, 1_500_000, 100))
            .collect();
        let mut v = low;
        v.extend(setup(&c));
        let mut s = Sym::new();
        let got = run(&mut s, &c, &v, Some(2.0), c.t0 + 400 * MIN);
        assert_eq!(got.len(), 1);
        assert!(
            !got[0].ext_ok && !got[0].kept && !got[0].fire,
            "{:?}",
            got[0]
        );
        // The budget: two kept a day, 30 minutes apart, one per second.
        let mut s = Sym::new();
        let mk = |t: i64| Cand {
            t,
            px_u: 2_000_000,
            score: 1.0,
            h0: 0,
            l0: 0,
            lp: 0,
            trig_u: 0,
            ign_end: 0,
            armed_at: 0,
            g1: true,
            in_universe: false,
            ext_base: None,
            ext_ok: false,
            kept: false,
            fire: false,
        };
        for m in [340, 345, 371, 400] {
            s.px.push_back((c.t0 + m * MIN - SEC, 2_000_000));
        }
        let k: Vec<bool> = [340, 345, 371, 400]
            .iter()
            .map(|m| s.verdicts(mk(c.t0 + m * MIN), &c).kept)
            .collect();
        assert_eq!(k, vec![true, false, true, false]);
    }

    #[test]
    fn r5_opens_the_gate_without_c1_and_a_halt_blocks_the_search() {
        let c = clock();
        // No C1: r5 (+5 % within 20 slots) opens the 1 s gate on the leg itself.
        let mut s = Sym::new();
        let got = run(&mut s, &c, &setup(&c), None, c.t0 + 400 * MIN);
        assert_eq!(got.len(), 1);
        assert!(got[0].g1);
        // A halt over the impulse: no setup at all.
        let mut s = Sym::new();
        s.halt_start(c.t0 + 337 * MIN);
        s.halt_resume(c.t0 + 337 * MIN, c.t0 + 337 * MIN + 30 * SEC);
        let got = run(&mut s, &c, &setup(&c), Some(2.0), c.t0 + 400 * MIN);
        assert!(got.is_empty(), "{got:?}");
    }

    #[test]
    fn before_the_gate_a_fire_uses_the_budget_but_does_not_fire() {
        let c = clock();
        let shift = -120 * MIN; // 07:42
        let v: Vec<Print> = setup(&c)
            .into_iter()
            .map(|x| Print {
                ns: x.ns + shift,
                ..x
            })
            .collect();
        let mut s = Sym::new();
        let got = run(&mut s, &c, &v, Some(2.0), c.t0 + 400 * MIN);
        assert_eq!(got.len(), 1);
        assert!(got[0].kept && !got[0].fire, "{:?}", got[0]);
    }

    #[test]
    fn two_above_needs_two_prints_or_a_hold() {
        assert!(two_above(101, 105, 103, None, 100));
        assert!(!two_above(99, 101, 101, None, 100));
        assert!(two_above(101, 101, 101, Some(2), 100));
    }
}

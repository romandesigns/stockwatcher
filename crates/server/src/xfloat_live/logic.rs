//! x-float-1 "rotation ignition", the detector itself: std only, no server types, so the parity
//! harness (`C:/wavystack/outputs/stockwatcher-research-v2/50-ship/float/`) compiles THIS FILE
//! unchanged (`#[path]`) and feeds it the research's own bars. Everything that decides whether
//! x-float-1 fires lives here; `super` only feeds it prints, a clock, the float and the previous
//! close, and turns a [`Cand`] into a notice.
//!
//! The research definition (branch `research/v2-x-float`, finalist commit d4bb58c0, registry seq
//! 852 `xfloat-H2-F50M-k0.25-g0.1`, config sha256 `dbaad445…a655b`; code
//! `research/v2/xfloat/floatcore.py` `qualified` / `day_arrays` / `h2_trigger` and `build.py`),
//! step by step, with the research's own arithmetic:
//!
//! 1. **15 s bars** of the day's counted prints on the 04:00 ET grid (the v2 cache's `bars15`,
//!    the engine-a store's bars plus the ones rebuilt from 1 s rows): the close and high in
//!    integer micro-dollars from OHLC-counted prints (the close is the print with the latest
//!    execution time, the later arrival on a tie; `c == 0` = the bar has no OHLC print) and the
//!    volume in shares of the volume-counted prints. A bar exists only for a slot with a print.
//! 2. **Float qualified** ([`qualified`]): finite, at least 10,000 shares and no float flags;
//!    and at most **50 M** shares ([`FLOAT_MAX`]).
//! 3. **Previous close** C1 > 0, as integer micro-dollars rounded half to even (`int(round(c1 *
//!    1e6))`, [`c1_micros`]).
//! 4. **Per bar, through the bar itself** (`day_arrays`): `cum` = the running sum of every bar's
//!    shares since 04:00 (an `f64` cumsum of integers, exact below 2^53, so a `u64` sum compared
//!    as `f64` is the same number); `gain` = close / C1 − 1 in `f64` (`c / max(c1_u, 1) - 1.0`,
//!    numpy true division of two int64s); `newhi` = the bar has a price and its close is strictly
//!    above the highest high of every EARLIER bar with a price (0 before the first).
//! 5. **Trigger** (`h2_trigger`, `k = 0.25`, `g = 0.10`): the FIRST bar of the day with `newhi`,
//!    the close in **$0.10–$25** inclusive (integer micro-dollars), `cum >= 0.25 × float` and
//!    `gain >= 0.10`. Once per symbol-day. The universe's "last print at most 15 s old" holds by
//!    construction (the close is a print of the bar itself).
//! 6. **Decision** at `a = bar end + 50 ms` (`DECIDE_NS`, production's settle).
//!
//! What the research had and live does not (all in the parity report and `super`'s docs): the
//! research float is the SEC-derived `float_ex_insiders` as of the day with its qualification
//! flags; live has only the FMP float, with no flags. The research C1 is the engine-a reference's
//! split-restated daily close; live takes the quote board's previous close, put on the day's share
//! basis by the corporate-actions ledger. The research bar holds every print by execution time
//! (hindsight); live decides at the bar's end + 50 ms, so a print reaching the server later is
//! counted for the later bars but not in that bar's decision ([`Late`]).

use std::collections::VecDeque;

pub const SEC: i64 = 1_000_000_000;
pub const MIN: i64 = 60 * SEC;
/// The bar grid (the research cache's and the Runners' 15 s slots).
pub const SLOT: i64 = 15 * SEC;
/// Slots from 04:00 to 20:00 ET.
pub const NSLOT: i64 = 3840;
/// Decided this long after the bar ends (`floatcore.DECIDE_NS`).
pub const DECIDE_NS: i64 = 50_000_000;
/// The float qualification's floor (`floatcore.MIN_FLOAT`) and the finalist's ceiling (`F50M`).
pub const MIN_FLOAT: f64 = 10_000.0;
pub const FLOAT_MAX: f64 = 50_000_000.0;
/// Shares traded since 04:00 over the float (`k0.25`).
pub const ROTATION_MIN: f64 = 0.25;
/// Close over the previous close, minus one (`g0.1`).
pub const GAIN_MIN: f64 = 0.10;
/// `universe-v1`'s price band (micro-dollars, inclusive).
pub const BAND_LO_U: i64 = 100_000;
pub const BAND_HI_U: i64 = 25_000_000;
/// The finalist's id (`30-w2/finalists/x-float-1.json`), the registry config id and the first
/// 12 hex digits of its config sha256: the ledger's `detector` / `detector_version`.
pub const DETECTOR: &str = "x-float-1";
pub const CONFIG: &str = "xfloat-H2-F50M-k0.25-g0.1";
pub const VERSION: &str = "dbaad44579768ee9";

/// `floatcore.qualified`: a finite float of at least 10,000 shares with no flags. Live floats
/// carry no flags (`flags_empty` is always true there).
pub fn qualified(float: Option<f64>, flags_empty: bool) -> bool {
    float.is_some_and(|f| f.is_finite() && f >= MIN_FLOAT) && flags_empty
}

/// [`qualified`] and at most [`FLOAT_MAX`] shares: a float x-float-1 can fire on.
pub fn eligible_float(float: Option<f64>, flags_empty: bool) -> Option<f64> {
    float.filter(|f| qualified(Some(*f), flags_empty) && *f <= FLOAT_MAX)
}

/// The previous close in integer micro-dollars as `build.py` makes it (`int(round(c1 * 1e6))`:
/// Python's `round` is half to even), or `None` when it cannot gain (`build.py` skips a missing,
/// NaN or non-positive C1, and `day_arrays` gives NaN gains for a C1 that rounds to 0).
pub fn c1_micros(c1: Option<f64>) -> Option<i64> {
    let c = c1.filter(|c| c.is_finite() && *c > 0.0)?;
    let u = (c * 1e6).round_ties_even() as i64;
    (u > 0).then_some(u)
}

/// A trading day's clock: 04:00 ET, epoch ns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Clock {
    pub t0: i64,
}

impl Clock {
    /// The 15 s slot of an execution time, `None` outside 04:00 + 16 h.
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
    /// The decision instant of slot `k`'s bar.
    pub fn decision(&self, k: i64) -> i64 {
        self.slot_end(k) + DECIDE_NS
    }
}

/// One complete 15 s bar, as the research cache stores it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Bar {
    pub slot: i64,
    /// Close and high in micro-dollars; 0 = no OHLC-counted print in the bar.
    pub c: i64,
    pub h: i64,
    /// Shares of the volume-counted prints.
    pub vol: u64,
}

/// What a bar is decided against: the symbol's eligible float ([`eligible_float`]) and previous
/// close ([`c1_micros`]). Either `None`: the bar cannot trigger (it still counts for later bars).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Inputs {
    pub float: Option<f64>,
    pub c1_u: Option<i64>,
}

/// A bar that met every condition (step 5): the symbol-day's one trigger.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Cand {
    pub slot: i64,
    /// The decision instant (bar end + 50 ms), epoch ns: the research's `a_ns`.
    pub t: i64,
    pub close_u: i64,
    /// The highest earlier high the close broke (0: the day's first priced bar).
    pub prior_high_u: i64,
    /// Shares since 04:00 through this bar, and the float and C1 it was judged against.
    pub cum: u64,
    pub float: f64,
    pub c1_u: i64,
    /// Execution and arrival time of the bar's close print (live only; 0 from bars).
    pub close_ns: i64,
    pub close_recv_ns: i64,
}

impl Cand {
    pub fn gain(&self) -> f64 {
        self.close_u as f64 / self.c1_u as f64 - 1.0
    }
    pub fn rotation(&self) -> f64 {
        self.cum as f64 / self.float
    }
}

/// One symbol's running day (step 4): the shares so far, the highest high so far and whether it
/// already triggered. [`Day::step`] is the research's per-bar loop; [`Sym`] builds the bars.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Day {
    cum: u64,
    /// Highest high of the priced bars stepped so far (`np.maximum.accumulate(hm)`, 0 before).
    prev_max: i64,
    fired: bool,
}

impl Day {
    /// One bar, in slot order: whether it triggers (only the day's first trigger is returned).
    pub fn step(&mut self, b: &Bar, inp: Inputs) -> Option<Cand> {
        self.cum = self.cum.saturating_add(b.vol);
        let has = b.c > 0;
        let prior = self.prev_max;
        if has {
            self.prev_max = self.prev_max.max(b.h);
        }
        if self.fired || !has {
            return None;
        }
        let (Some(float), Some(c1_u)) = (inp.float, inp.c1_u) else {
            return None;
        };
        let newhi = b.c > prior;
        let in_band = (BAND_LO_U..=BAND_HI_U).contains(&b.c);
        // `cum >= k * flt` in f64 (an exact integer below 2^53) and `c / max(c1_u, 1) - 1.0 >= g`.
        let rotated = self.cum as f64 >= ROTATION_MIN * float;
        let gain = b.c as f64 / c1_u.max(1) as f64 - 1.0;
        if !(newhi && in_band && rotated && gain >= GAIN_MIN) {
            return None;
        }
        self.fired = true;
        Some(Cand {
            slot: b.slot,
            t: 0,
            close_u: b.c,
            prior_high_u: prior,
            cum: self.cum,
            float,
            c1_u,
            close_ns: 0,
            close_recv_ns: 0,
        })
    }

    /// A print counted after its bar was decided: it is in every later bar's running sums, as
    /// it is in the research's (the bar's own decision is already made).
    fn late(&mut self, p: &Print) {
        if p.vol {
            self.cum = self.cum.saturating_add(p.size);
        }
        if p.ohlc {
            self.prev_max = self.prev_max.max(p.px);
        }
    }

    pub fn fired(&self) -> bool {
        self.fired
    }
    pub fn cum(&self) -> u64 {
        self.cum
    }
}

/// One print with the chart engine's counting verdict.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Print {
    /// Execution time and arrival time, epoch ns.
    pub ns: i64,
    pub recv_ns: i64,
    /// Micro-dollars.
    pub px: i64,
    pub size: u64,
    pub ohlc: bool,
    pub vol: bool,
}

/// A bar still being built (not decided yet).
#[derive(Clone, Copy, Debug, Default)]
struct Open {
    bar: Bar,
    /// The close print's execution and arrival time (the close is the latest execution, the
    /// later arrival on a tie).
    c_ns: i64,
    c_recv: i64,
}

/// Late input, counted (never silently different from the research).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Late {
    /// A print whose bar had already been decided: counted for the later bars only.
    pub prints_after_decision: u64,
}

/// One symbol's day, fed print by print (live): the bars not decided yet and the [`Day`].
#[derive(Clone, Debug)]
pub struct Sym {
    open: VecDeque<Open>,
    /// The last slot decided (`-1`: none).
    decided: i64,
    day: Day,
    pub late: Late,
}

impl Default for Sym {
    fn default() -> Self {
        Sym {
            open: VecDeque::new(),
            decided: -1,
            day: Day::default(),
            late: Late::default(),
        }
    }
}

impl Sym {
    pub fn new() -> Self {
        Self::default()
    }

    /// One counted print. O(1) but for a print of an older open bar (a short walk from the back).
    pub fn apply(&mut self, p: &Print, clock: &Clock) {
        if !p.ohlc && !p.vol {
            return;
        }
        let Some(k) = clock.slot(p.ns) else { return };
        if k <= self.decided {
            self.late.prints_after_decision += 1;
            self.day.late(p);
            return;
        }
        // Open bars are in slot order; `i` = the first one past slot k.
        let i = self.open.len()
            - self
                .open
                .iter()
                .rev()
                .take_while(|o| o.bar.slot > k)
                .count();
        let j = if i > 0 && self.open[i - 1].bar.slot == k {
            i - 1
        } else {
            self.open.insert(
                i,
                Open {
                    bar: Bar {
                        slot: k,
                        ..Bar::default()
                    },
                    c_ns: i64::MIN,
                    c_recv: 0,
                },
            );
            i
        };
        let o = &mut self.open[j];
        if p.vol {
            o.bar.vol = o.bar.vol.saturating_add(p.size);
        }
        if p.ohlc {
            o.bar.h = o.bar.h.max(p.px);
            // Arrival order breaks a tie: this print arrived after the held close.
            if p.ns >= o.c_ns {
                o.bar.c = p.px;
                o.c_ns = p.ns;
                o.c_recv = p.recv_ns;
            }
        }
    }

    /// When the oldest open bar is due, epoch ns.
    pub fn next_due(&self, clock: &Clock) -> Option<i64> {
        self.open.front().map(|o| clock.decision(o.bar.slot))
    }

    /// Decides every open bar due by `now`, oldest first; `inputs` is read once per call.
    pub fn decide_due(&mut self, now: i64, clock: &Clock, inputs: Inputs) -> Option<Cand> {
        let mut out = None;
        while let Some(o) = self.open.front() {
            if clock.decision(o.bar.slot) > now {
                break;
            }
            let o = self.open.pop_front().expect("peeked");
            self.decided = o.bar.slot;
            if let Some(mut c) = self.day.step(&o.bar, inputs) {
                c.t = clock.decision(o.bar.slot);
                c.close_ns = o.c_ns;
                c.close_recv_ns = o.c_recv;
                out = Some(c);
            }
        }
        out
    }

    /// Bars still open (the cost line).
    pub fn held(&self) -> usize {
        self.open.len()
    }

    pub fn fired(&self) -> bool {
        self.day.fired()
    }

    /// Whether this symbol can never fire today (it fired already): its prints need no bars.
    pub fn done(&self) -> bool {
        self.day.fired()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const T0: i64 = 1_718_092_800 * SEC; // 2024-06-11 04:00 ET
    fn clock() -> Clock {
        Clock { t0: T0 }
    }
    fn inp() -> Inputs {
        Inputs {
            float: Some(1_000_000.0),
            c1_u: Some(1_000_000),
        }
    }
    fn bar(slot: i64, c: i64, h: i64, vol: u64) -> Bar {
        Bar { slot, c, h, vol }
    }

    #[test]
    fn qualification_and_c1_follow_the_research() {
        assert!(qualified(Some(10_000.0), true));
        assert!(!qualified(Some(9_999.0), true));
        assert!(!qualified(Some(f64::NAN), true));
        assert!(!qualified(Some(1e6), false));
        assert!(!qualified(None, true));
        assert_eq!(eligible_float(Some(50e6), true), Some(50e6));
        assert_eq!(eligible_float(Some(50e6 + 1.0), true), None);
        assert_eq!(c1_micros(Some(1.2345675)), Some(1_234_568));
        // Half to even, as Python's round.
        assert_eq!(c1_micros(Some(0.0000025)), Some(2));
        assert_eq!(c1_micros(Some(0.0000001)), None);
        assert_eq!(c1_micros(Some(-1.0)), None);
        assert_eq!(c1_micros(None), None);
    }

    #[test]
    fn first_bar_meeting_every_condition_fires_once() {
        let mut d = Day::default();
        // Up 10 % but only 0.1 of the float traded.
        assert!(
            d.step(&bar(0, 1_100_000, 1_100_000, 100_000), inp())
                .is_none()
        );
        // Rotation reached, gain reached, but not above the earlier high 1.10.
        assert!(
            d.step(&bar(1, 1_100_000, 1_100_000, 200_000), inp())
                .is_none()
        );
        // A volume-only bar counts shares, no price.
        assert!(d.step(&bar(2, 0, 0, 1), inp()).is_none());
        let c = d.step(&bar(3, 1_120_000, 1_150_000, 1), inp()).unwrap();
        assert_eq!(
            (c.slot, c.close_u, c.prior_high_u, c.cum),
            (3, 1_120_000, 1_100_000, 300_002)
        );
        assert!(
            d.step(&bar(4, 1_300_000, 1_300_000, 1), inp()).is_none(),
            "once a day"
        );
    }

    #[test]
    fn band_and_gain_edges_are_inclusive() {
        let i = Inputs {
            float: Some(4.0),
            c1_u: Some(90_000),
        };
        let mut d = Day::default();
        // 0.099 → below the band.
        assert!(d.step(&bar(0, 99_999, 99_999, 1), i).is_none());
        let c = d.step(&bar(1, 100_000, 100_000, 0), i);
        assert!(
            c.is_some(),
            "$0.10 is in the band; gain 11 %; cum 1 >= 0.25 * 4"
        );
        let mut d = Day::default();
        let i = Inputs {
            float: Some(4.0),
            c1_u: Some(1_000_000),
        };
        assert!(
            d.step(&bar(0, 1_100_000, 1_100_000, 1), i).is_some(),
            "gain exactly 10 %"
        );
    }

    #[test]
    fn prints_build_the_same_bars_and_late_prints_count_later() {
        let ck = clock();
        let mut s = Sym::new();
        let p = |off: i64, px: i64, size: u64, ohlc: bool, vol: bool| Print {
            ns: T0 + off,
            recv_ns: T0 + off + 1,
            px,
            size,
            ohlc,
            vol,
        };
        s.apply(&p(SEC, 1_000_000, 200_000, true, true), &ck);
        s.apply(&p(2 * SEC, 1_050_000, 100_000, true, true), &ck);
        assert_eq!(s.next_due(&ck), Some(ck.decision(0)));
        assert!(s.decide_due(ck.decision(0) - 1, &ck, inp()).is_none());
        assert!(
            s.decide_due(ck.decision(0), &ck, inp()).is_none(),
            "gain 5 %"
        );
        // A late print in slot 0: counted for later bars (it raises the high to 1.20).
        s.apply(&p(3 * SEC, 1_200_000, 1, true, true), &ck);
        assert_eq!(s.late.prints_after_decision, 1);
        s.apply(&p(SLOT + SEC, 1_150_000, 1, true, true), &ck);
        assert!(
            s.decide_due(ck.decision(1), &ck, inp()).is_none(),
            "1.15 is below 1.20"
        );
        // Two prints at one execution time: the later arrival is the close.
        s.apply(&p(2 * SLOT + SEC, 1_300_000, 1, true, true), &ck);
        s.apply(&p(2 * SLOT + SEC, 1_250_000, 1, true, true), &ck);
        let c = s.decide_due(ck.decision(2), &ck, inp()).unwrap();
        assert_eq!((c.slot, c.close_u, c.t), (2, 1_250_000, ck.decision(2)));
        assert_eq!(c.cum, 300_004);
        assert_eq!(c.close_recv_ns, T0 + 2 * SLOT + SEC + 1);
    }

    #[test]
    fn an_out_of_order_print_lands_in_its_own_open_bar() {
        let ck = clock();
        let mut s = Sym::new();
        let p = |off: i64, px: i64| Print {
            ns: T0 + off,
            recv_ns: 0,
            px,
            size: 1,
            ohlc: true,
            vol: true,
        };
        s.apply(&p(SLOT + 1, 2_000_000), &ck);
        s.apply(&p(1, 1_000_000), &ck);
        s.apply(&p(2, 1_100_000), &ck);
        assert_eq!(s.held(), 2);
        assert_eq!(s.next_due(&ck), Some(ck.decision(0)));
    }
}

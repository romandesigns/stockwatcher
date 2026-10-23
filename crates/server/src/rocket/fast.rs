//! Fast move: a stock in `$0.10–$25` that jumps inside a minute, with real
//! trading behind it. The owner, 2026-10-22: "many of these events happen within 15 seconds,
//! 30 seconds and 1 min". The other entry paths look at 15 s bars (the +20 %-in-15-min leg, the
//! running entry, R2) or at bursts; this one is evaluated every second.
//!
//! # The rule
//!
//! At the end of every second ([`FastTape::inputs`], then [`decide`]), a symbol fires when, for
//! one of the [`ARMS`]:
//!
//! 1. **Held move.** `held / low − 1 ≥ gain`. `low` is the lowest OHLC-counted price in the last
//!    `secs` seconds, including the price standing when the window opened. `held` is the price
//!    the move has kept: the higher of the lowest of the last [`HOLD_N`] OHLC prints and the
//!    lowest price over the last [`HOLD_S`] seconds. So a single print (or two) at a far price
//!    cannot fire it, and only prints that update OHLC (`Trade::effect`, the chart engine's
//!    verdict) move any price here.
//! 2. **Real trading inside the same window:** at least `trades` volume-counted prints and
//!    `dollars` traded.
//! 3. **In band** at the held price.
//!
//! The arms: **+8 % in 60 s with ≥ 40 trades and ≥ $50k**, or **+12 % in 15 s with ≥ 10 trades
//! and ≥ $12.5k** (the explosive start that has not yet had the minute's trades). The caller adds
//! a refractory of [`REFRACTORY_NS`] per symbol; whether a symbol that already has a row gets a
//! second one is the board's choice.
//!
//! Cost: O(1) per print, O(64) per symbol per second (only for symbols that printed in the last
//! 2 s: after that nothing can newly fire), a 64-second ring per symbol.
//!
//! # In Runners
//!
//! The fifth way into the Runners panel ([`super::RocketBoard`], `AlertKind::Fast`, chip "Fast
//! move"): the board feeds every print the chart engine counted into a per-symbol tape (the
//! live path and the provisional one for a symbol whose history is still loading, exactly where
//! the 15 s tape and the burst detector are fed) and reads the rule at the once-a-second rocket
//! tick for the second that just ended ([`FastTape::inputs_settled`]: a print of the next second
//! may already be in). One row per symbol a day, none for a symbol that already has a row, at
//! most [`MAX_PER_DAY`]; logged as a `fast` line, restored after a restart, pushed per phone
//! (on by default). **After a restart the tapes start empty**: the rule reads 64 s of history,
//! so a symbol can fire again about a minute after the process is back, and nothing older is
//! worth rebuilding.
//!
//! # Calibration
//!
//! `fast::calib` (ignored, run by hand) replays archive days through the production chart engine
//! into this tape, second by second, and writes every second where a loose version holds (any
//! window ≥ +4 % held, ≥ 5 trades, ≥ $5k) with the forward outcome, plus each symbol's first
//! running entry ([`super::running::decide`]); the constants were swept offline over those rows.
//! Fires carry a 30-minute refractory. Metrics: fires a day; **opportunity** = the share of fires
//! followed within 30 minutes by +10 % before −5 % from the fire price; **leg recall** = the share
//! of `reports/rockets/events-g20` legs with a fire between the leg's start bar and its peak, and
//! the **lead** over that leg's +20 % mark; **extra** = fired symbols with neither a running entry
//! nor a +20 % leg that day (bursts not counted, so this overstates the extra a little).
//!
//! Tuned on 2026-09-22..10-02 (9 days); chosen as the best "≥ 30 s ahead of the existing paths"
//! leg share with about 30 extra symbols a day. Findings that shaped it:
//!
//! - **Volatility scaling did not help.** `req = clamp(k × median daily range, floor, cap)` with
//!   k = 0.5 matched a flat threshold and k = 1 lost recall at the same fires: inside a minute the
//!   move needed is about the same for a $0.50 and a $10 stock once money is required.
//! - **With equal liquidity floors the 60 s window subsumes 15 s and 30 s** (a +8 % move in 15 s
//!   is also one in 60 s); in the sweep a 30 s arm next to the 60 s one changed nothing. The 15 s arm is only useful with
//!   thinner floors and a higher bar, and buys ~2 points of recall for ~4 extra symbols a day.
//! - These are **opportunities about a quarter of the time**, twice the base rate of a loose
//!   +4 % trigger (11–14 %), and a fast fire lands near the middle of a leg (median 48 % of the
//!   way from the leg's start to its peak), so it is an "it is moving now" signal, not a bottom.
//! - The lead over the 15 s-bar +20 % mark is short (median ~20 s): most +20 % legs are already
//!   visible at that mark. The value is the ~15 % of legs fired ≥ 30 s before every existing
//!   path, and the movers no other path surfaces in time.
//!
//! | days | fires/day med (p90) | extra med (p90) | opportunity | leg recall | lead vs +20 % mark |
//! |---|---|---|---|---|---|
//! | 09-22..10-02 (tune) | 139 (174) | 28 (33) | 23.2 % | 73.8 % | +23 s |
//! | 10-05..10-20 | 142 (174) | 35 (39) | 21.7 % | 67.6 % | +22 s |
//! | 06-01..06-12 (busier) | 229 (421) | 45 (54) | 26.0 % | 72.7 % | +18 s |
//!
//! Without the 15 s arm (60 s only): 24 (30) / 30 (36) / 40 (48) extra a day, 71.9 / 66.2 /
//! 71.1 % recall. Final check, 2026-10-22, never used to choose: 119 fires on 65 symbols (37
//! extra), opportunity 19 %; of the day's 60 missed-movers (+20 % in 30 min or +30 % on the day),
//! **37 fired by a leg's peak** (34 inside a leg) against 24 for the replayed R2 + burst + running
//! paths; together 38/60, the fast trigger alone adding AUID, BEZ, CHOW, ESLA, GPUS, IBX, KFIIR,
//! KPTI, OPTU, SBFM, SDEV, SPHL, WORX and YHC.

/// The Runners row's strategy id and version (`fast` log lines).
pub const STRATEGY: &str = "fast-move";
pub const VERSION: &str = "v1";
/// A runaway guard: at most this many fast rows a day (calibration: ~140 fires a day, many on
/// symbols that already have a row).
pub const MAX_PER_DAY: usize = 200;

/// Band, dollars (the running entry's).
pub const BAND: (f64, f64) = (0.10, 25.0);
/// Window lengths, seconds.
pub const WINDOWS: [usize; 3] = [15, 30, 60];
/// Prints a move must hold for (or [`HOLD_S`] seconds).
pub const HOLD_N: usize = 3;
pub const HOLD_S: i64 = 2;
/// Seconds the ring keeps (the longest window, the standing price before it, slack).
const RING: usize = 64;

/// One second of OHLC-counted and volume-counted prints.
#[derive(Clone, Copy, Debug)]
struct Sec {
    sec: i64,
    lo: f64,
    hi: f64,
    last: f64,
    trades: u32,
    dollars: f64,
}

/// Per-symbol rolling state on a 1 s grid. O(1) per print; [`FastTape::inputs`] is O(64).
#[derive(Clone, Debug)]
pub struct FastTape {
    ring: Vec<Option<Sec>>,
    /// The newest second applied (prints must arrive in time order).
    head: i64,
    /// Last OHLC-counted price, carried into seconds without prints.
    last: f64,
    /// The last [`HOLD_N`] OHLC-counted prices, oldest first.
    recent: [f64; HOLD_N],
    nrecent: usize,
    /// `last`, `recent` and `nrecent` as they stood at the end of `prev.0`, the second before
    /// the first print of `head` (O(1), taken in [`FastTape::apply`]): what
    /// [`FastTape::inputs_settled`] reads for a second that ended while the next one already
    /// has prints.
    prev: (i64, f64, [f64; HOLD_N], usize),
    /// The second of the newest print applied.
    printed: i64,
}

impl Default for FastTape {
    fn default() -> Self {
        Self {
            ring: vec![None; RING],
            head: i64::MIN,
            last: f64::NAN,
            recent: [f64::NAN; HOLD_N],
            nrecent: 0,
            prev: (i64::MIN, f64::NAN, [f64::NAN; HOLD_N], 0),
            printed: i64::MIN,
        }
    }
}

/// One window's reading.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Win {
    /// Lowest OHLC price in the window, including the price standing when it opened.
    pub low: f64,
    /// Volume-counted prints and their dollars inside the window.
    pub trades: u32,
    pub dollars: f64,
}

/// What the rule reads at the end of a second.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FastInputs {
    /// Last OHLC-counted price.
    pub price: f64,
    /// The price the move has held: the higher of the lowest of the last [`HOLD_N`] OHLC prints
    /// and the lowest price over the last [`HOLD_S`] seconds.
    pub held: f64,
    /// Per [`WINDOWS`] entry.
    pub win: [Win; 3],
}

impl FastTape {
    /// The ring's heap size, bytes (the struct itself is `size_of::<FastTape>()`).
    pub fn heap_bytes() -> usize {
        RING * std::mem::size_of::<Option<Sec>>()
    }

    fn slot(&mut self, sec: i64) -> &mut Option<Sec> {
        &mut self.ring[sec.rem_euclid(RING as i64) as usize]
    }

    /// Fills seconds up to `sec` (exclusive of nothing: `sec` itself is created).
    fn advance(&mut self, sec: i64) {
        if self.head == i64::MIN {
            self.head = sec - 1;
        } else if sec - self.head > RING as i64 {
            // A long gap: only the standing price matters, carried by the fill below.
            self.head = sec - RING as i64;
        }
        while self.head < sec {
            self.head += 1;
            let last = self.last;
            let h = self.head;
            *self.slot(h) = Some(Sec {
                sec: h,
                lo: last,
                hi: last,
                last,
                trades: 0,
                dollars: 0.0,
            });
        }
    }

    /// One print at `ts_ns` (UNIX nanoseconds). `ohlc`/`volume`: the chart engine's verdict.
    pub fn apply(&mut self, ts_ns: i64, price: f64, size: u64, ohlc: bool, volume: bool) {
        let sec = ts_ns.div_euclid(1_000_000_000);
        if sec < self.head {
            return; // out of order: the second is already read
        }
        if sec > self.head && self.head != i64::MIN {
            self.prev = (self.head, self.last, self.recent, self.nrecent);
        }
        self.printed = sec;
        self.advance(sec);
        let i = sec.rem_euclid(RING as i64) as usize;
        let Some(s) = self.ring[i].as_mut() else {
            return;
        };
        if ohlc {
            s.lo = s.lo.min(price);
            s.hi = s.hi.max(price);
            if s.lo.is_nan() {
                s.lo = price;
                s.hi = price;
            }
            s.last = price;
        }
        if volume {
            s.trades += 1;
            s.dollars += price * size as f64;
        }
        if ohlc {
            self.last = price;
            self.recent.rotate_left(1);
            self.recent[HOLD_N - 1] = price;
            self.nrecent = (self.nrecent + 1).min(HOLD_N);
        }
    }

    /// The readings at the end of second `sec` (all prints with `ts < (sec + 1) s` applied).
    /// `None` before the first OHLC print.
    pub fn inputs(&mut self, sec: i64) -> Option<FastInputs> {
        if self.last.is_nan() || self.head == i64::MIN || sec < self.head {
            return None;
        }
        self.advance(sec);
        self.read(sec, self.last, self.recent, self.nrecent)
    }

    /// The readings at the end of second `sec` even when a print of a later second already
    /// arrived (the live tick reads a second just after it ended). Equal to what
    /// [`FastTape::inputs`] gave at the end of `sec`, as long as `sec` is at most 3 s behind the
    /// newest print (the 60 s window and its standing price fit the ring beside it); `None`
    /// further back.
    pub fn inputs_settled(&mut self, sec: i64) -> Option<FastInputs> {
        if self.head == i64::MIN || sec >= self.head {
            return self.inputs(sec);
        }
        if self.head - sec > (RING - WINDOWS[2] - 1) as i64 {
            return None;
        }
        if sec >= self.printed {
            // No print after `sec`: the price state is today's.
            return self.read(sec, self.last, self.recent, self.nrecent);
        }
        let (at, last, recent, n) = self.prev;
        if sec < at || last.is_nan() {
            return None;
        }
        self.read(sec, last, recent, n)
    }

    /// The readings at the end of `sec` with the price state `last`/`recent`/`nrecent` as it
    /// stood then; every second up to `sec` is in the ring.
    fn read(
        &self,
        sec: i64,
        last: f64,
        recent: [f64; HOLD_N],
        nrecent: usize,
    ) -> Option<FastInputs> {
        let get = |r: &[Option<Sec>], s: i64| {
            r[s.rem_euclid(RING as i64) as usize].filter(|x| x.sec == s)
        };
        let mut win = [Win::default(); 3];
        for (i, &w) in WINDOWS.iter().enumerate() {
            let w = w as i64;
            let mut low = get(&self.ring, sec - w).map_or(f64::NAN, |x| x.last);
            let (mut n, mut d) = (0u32, 0.0);
            for s in sec - w + 1..=sec {
                if let Some(x) = get(&self.ring, s) {
                    if !x.lo.is_nan() && (low.is_nan() || x.lo < low) {
                        low = x.lo;
                    }
                    n += x.trades;
                    d += x.dollars;
                }
            }
            win[i] = Win {
                low,
                trades: n,
                dollars: d,
            };
        }
        let mut held_s = get(&self.ring, sec - HOLD_S).map_or(f64::NAN, |x| x.last);
        for s in sec - HOLD_S + 1..=sec {
            if let Some(x) = get(&self.ring, s)
                && !x.lo.is_nan()
                && (held_s.is_nan() || x.lo < held_s)
            {
                held_s = x.lo;
            }
        }
        let held_n = if nrecent == HOLD_N {
            recent.iter().copied().fold(f64::INFINITY, f64::min)
        } else {
            f64::NAN
        };
        let held = match (held_s.is_nan(), held_n.is_nan()) {
            (false, false) => held_s.max(held_n),
            (false, true) => held_s,
            (true, false) => held_n,
            (true, true) => return None,
        };
        Some(FastInputs {
            price: last,
            held,
            win,
        })
    }
}

/// One arm of the rule: a move of at least `gain` (held price over the window's low) inside the
/// last `secs` seconds, with at least `trades` volume-counted prints and `dollars` traded in it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Arm {
    pub secs: usize,
    pub gain: f64,
    pub trades: u32,
    pub dollars: f64,
}

/// The rule's arms, checked in order (calibration: module docs). Either one fires.
pub const ARMS: [Arm; 2] = [
    Arm {
        secs: 60,
        gain: 0.08,
        trades: 40,
        dollars: 50_000.0,
    },
    Arm {
        secs: 15,
        gain: 0.12,
        trades: 10,
        dollars: 12_500.0,
    },
];

/// The caller's refractory: at most one fast-move fire per symbol per 30 minutes.
pub const REFRACTORY_NS: i64 = 30 * 60 * 1_000_000_000;

/// Why a symbol fired.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FastEntry {
    /// The arm's window, seconds.
    pub secs: usize,
    pub price: f64,
    pub held: f64,
    pub low: f64,
    /// `held / low − 1`.
    pub gain: f64,
    pub trades: u32,
    pub dollars: f64,
}

impl FastEntry {
    pub fn reasons(&self) -> Vec<String> {
        let d = if self.dollars >= 1e6 {
            format!("{:.1}M", self.dollars / 1e6)
        } else {
            format!("{:.0}k", self.dollars / 1e3)
        };
        vec![
            format!("+{:.0}% in {} s (held)", self.gain * 100.0, self.secs),
            format!("{} trades, ${d} in {} s", self.trades, self.secs),
        ]
    }
}

/// The rule (module docs) on one second's readings. `None`: no fire. Pure; the refractory
/// ([`REFRACTORY_NS`]) and "has a row already" are the caller's.
pub fn decide(x: &FastInputs) -> Option<FastEntry> {
    if !(BAND.0..=BAND.1).contains(&x.held) {
        return None;
    }
    for a in ARMS {
        let Some(i) = WINDOWS.iter().position(|w| *w == a.secs) else {
            continue;
        };
        let w = x.win[i];
        if w.low.is_nan() || w.low <= 0.0 {
            continue;
        }
        let gain = x.held / w.low - 1.0;
        if gain >= a.gain && w.trades >= a.trades && w.dollars >= a.dollars {
            return Some(FastEntry {
                secs: a.secs,
                price: x.price,
                held: x.held,
                low: w.low,
                gain,
                trades: w.trades,
                dollars: w.dollars,
            });
        }
    }
    None
}

#[cfg(test)]
mod calib;

#[cfg(test)]
mod tests {
    use super::*;

    const S: i64 = 1_000_000_000;
    const T0: i64 = 1_790_000_000 * S;

    /// `n` prints of `size` at `price` spread over second `sec` (relative to T0).
    fn prints(t: &mut FastTape, sec: i64, price: f64, n: usize, size: u64) {
        for k in 0..n {
            t.apply(
                T0 + sec * S + k as i64 * (S / (n as i64 + 1)),
                price,
                size,
                true,
                true,
            );
        }
    }

    #[test]
    fn window_low_includes_the_standing_price() {
        let mut t = FastTape::default();
        prints(&mut t, 0, 1.00, 1, 100);
        // Nothing for 70 s, then 1.10: the 15 s low is the standing 1.00, not the 1.10 print.
        prints(&mut t, 70, 1.10, 3, 100);
        let x = t.inputs(T0 / S + 70).unwrap();
        assert_eq!(x.win[0].low, 1.00);
        assert_eq!(x.win[2].low, 1.00);
        assert_eq!(x.win[0].trades, 3);
        assert_eq!(x.price, 1.10);
    }

    #[test]
    fn windows_forget_old_prints() {
        let mut t = FastTape::default();
        prints(&mut t, 0, 1.00, 5, 100);
        prints(&mut t, 40, 1.05, 5, 100);
        let x = t.inputs(T0 / S + 40).unwrap();
        assert_eq!(x.win[0].trades, 5);
        assert_eq!(x.win[1].trades, 5);
        assert_eq!(x.win[2].trades, 10);
        assert_eq!(x.win[0].low, 1.00);
    }

    #[test]
    fn a_strong_minute_fires_the_60s_arm() {
        let mut t = FastTape::default();
        prints(&mut t, 0, 2.00, 2, 100);
        // 2.00 → 2.20 over 50 s, one 1,000-share print a second.
        for s in 1..=50 {
            prints(&mut t, s, 2.00 + 0.004 * s as f64, 1, 1000);
        }
        let x = t.inputs(T0 / S + 50).unwrap();
        let e = decide(&x).expect("fires");
        assert_eq!(e.secs, 60);
        assert!(e.gain >= 0.08);
        assert_eq!(e.reasons().len(), 2);
    }

    #[test]
    fn thin_money_does_not_fire() {
        let mut t = FastTape::default();
        prints(&mut t, 0, 2.00, 2, 10);
        for s in 1..=50 {
            prints(&mut t, s, 2.00 + 0.004 * s as f64, 1, 10);
        }
        let x = t.inputs(T0 / S + 50).unwrap();
        assert!(decide(&x).is_none());
    }

    #[test]
    fn an_explosive_burst_fires_the_15s_arm_before_the_minute_has_the_trades() {
        let mut t = FastTape::default();
        prints(&mut t, 0, 0.50, 2, 1000);
        // +19 % in 6 s on 12 prints of 3,000 shares (~$20k): too few trades for the 60 s arm.
        for s in 10..=15 {
            prints(&mut t, s, 0.50 + 0.016 * (s - 9) as f64, 2, 3000);
        }
        let x = t.inputs(T0 / S + 15).unwrap();
        let e = decide(&x).expect("fires");
        assert_eq!(e.secs, 15);
    }

    #[test]
    fn a_single_bad_print_does_not_fire() {
        let mut t = FastTape::default();
        for s in 0..30 {
            prints(&mut t, s, 1.00, 2, 2000);
        }
        // One print at 1.30 with nothing after it: not held for 3 prints or 2 s.
        t.apply(T0 + 31 * S, 1.30, 5000, true, true);
        let x = t.inputs(T0 / S + 31).unwrap();
        assert!(x.held < 1.01);
        assert!(decide(&x).is_none());
        // And once it is back at 1.00.
        prints(&mut t, 31, 1.00, 2, 2000);
        let x = t.inputs(T0 / S + 31).unwrap();
        assert!(decide(&x).is_none());
    }

    #[test]
    fn prints_that_do_not_update_ohlc_cannot_make_a_move() {
        let mut t = FastTape::default();
        for s in 0..30 {
            prints(&mut t, s, 1.00, 2, 2000);
        }
        for k in 0..20 {
            t.apply(T0 + 31 * S + k * 1000, 1.40, 5000, false, true);
        }
        let x = t.inputs(T0 / S + 33).unwrap();
        assert_eq!(x.price, 1.00);
        assert!(decide(&x).is_none());
    }

    /// Reading a second after a print of the next one arrived gives what reading it at its end
    /// gave (the live tick's case), and nothing further back than the ring allows.
    #[test]
    fn a_settled_read_matches_the_read_at_the_end_of_the_second() {
        let feed = |t: &mut FastTape, upto: i64| {
            prints(t, 0, 2.00, 2, 100);
            for s in 1..=upto {
                if s % 7 != 3 {
                    prints(t, s, 2.00 + 0.004 * s as f64, 2, 1000);
                }
            }
        };
        for sec in [20, 41, 45, 50] {
            let mut a = FastTape::default();
            feed(&mut a, sec);
            let want = a.inputs(T0 / S + sec).unwrap();
            // The same tape with the next seconds' first prints already in.
            for ahead in 1..=3 {
                let mut b = FastTape::default();
                feed(&mut b, sec);
                prints(&mut b, sec + ahead, 9.99, 1, 1);
                assert_eq!(b.inputs_settled(T0 / S + sec), Some(want), "{sec} +{ahead}");
            }
            let mut c = FastTape::default();
            feed(&mut c, sec);
            prints(&mut c, sec + 4, 9.99, 1, 1);
            assert_eq!(c.inputs_settled(T0 / S + sec), None, "too far back");
        }
        // A second with no print after the newest print reads the current state.
        let mut d = FastTape::default();
        feed(&mut d, 30);
        let _ = d.inputs(T0 / S + 32);
        let mut e = FastTape::default();
        feed(&mut e, 30);
        assert_eq!(e.inputs_settled(T0 / S + 31), d.inputs_settled(T0 / S + 31));
        assert!(d.inputs_settled(T0 / S + 31).is_some());
    }

    #[test]
    fn out_of_band_does_not_fire() {
        let mut t = FastTape::default();
        prints(&mut t, 0, 24.00, 2, 1000);
        for s in 1..=50 {
            prints(&mut t, s, 24.00 + 0.05 * s as f64, 1, 1000);
        }
        let x = t.inputs(T0 / S + 50).unwrap();
        assert!(x.held > 25.0);
        assert!(decide(&x).is_none());
    }
}

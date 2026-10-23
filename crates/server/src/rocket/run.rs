//! The "Run +20%" entry into Runners (owner, 2026-10-22): the research labeller's +20 % leg, the
//! last way into the panel beside R2 flags, buying bursts and the running entry ([`super::running`]).
//!
//! # The rule
//!
//! It is the volume-shelf module's run trigger (`crate::zones`, "Run"), the causal port of
//! `rockets(d, 20)` from `reports/rockets/definition.json`, computed once there and handed to
//! this board ([`crate::zones::ZoneBoard::take_entries`] → [`super::RocketBoard::add_runs`]):
//!
//! - the 15 s close of a traded slot is at least 1.2 × the min close of the traded slots of the 15
//!   minutes before it (the start: the latest slot at that min);
//! - at least 25 volume-counted trades and $25,000 from the start slot through the mark slot;
//! - 30-minute refractory per symbol;
//! - the start price in `$0.10–$25` ([`crate::zones::RUN_BAND_MICROS`]; the shelves keep the
//!   labels' `$0.15` floor, with their own refractory).
//!
//! Live, the row does not wait for the mark slot's close (owner, 2026-10-22: "many of these
//! events happen within 15 seconds, 30 seconds and 1 min"): the same trigger is read every
//! second on the slot in progress (`crate::zones`, "Early fire"), and the row appears the second
//! it holds, at the last price, guarded against one bad print (2 prints over the mark, or 2 s
//! held). The 15 s-close version still runs as the reference: when it confirms first (the guard
//! held the early fire back), the row appears at the end of the mark slot (decided 1 s after
//! it), at the mark close. The `run` log line says which (`early`) and carries the triggering
//! print's `exec_ns`. One row per symbol a day, and none when the symbol already has a row (an
//! R2 flag, a burst or a running entry): later legs show as its momentum states. Its phone push
//! (`crate::alerts::runners`) is on by default per phone.
//!
//! # Calibration
//!
//! From the 2026-10-22 missed-movers sweep
//! (`reports/missed-movers/2026-10-22/rule-comparison.json`): the running entry v2 plus this path
//! gives about 73 rows a day median, p90 77 on the tune days (2026-09-22 to 10-02), 86 / 139 on
//! the busier June days, and catches 43 of 2026-10-22's 60 movers. Each day's count of run rows
//! is in the `day` line of the forward log (`rows.run`), so the budget can be checked after the
//! first sessions.
//!
//! # Without volume shelves
//!
//! The trigger lives in the shelves' board, so `VOLUME_SHELVES=0` (or `ROCKET_WATCH=0`) turns
//! this entry off too.

use crate::zones::{GAIN_PCT, Run};

pub const STRATEGY: &str = "run-g20";
pub const VERSION: &str = "v1";
/// A runaway guard: at most this many run rows a day.
pub const MAX_PER_DAY: usize = 200;

/// The row's score: the leg's gain over the +20 % it needed (≥ 1).
pub fn score(r: &Run) -> f64 {
    gain(r) / (GAIN_PCT / 100.0)
}

/// The leg's gain, start to mark close (≥ 0.2).
pub fn gain(r: &Run) -> f64 {
    r.close / r.start_price - 1.0
}

/// The row's reasons.
pub fn reasons(r: &Run) -> Vec<String> {
    let min = ((r.plus20_ns - r.start_ns) as f64 / 60e9).max(0.25);
    let mins = if min < 1.0 {
        format!("{:.0} s", min * 60.0)
    } else {
        format!("{min:.0} min")
    };
    vec![
        format!(
            "+{:.0}% in {mins} off {}",
            gain(r) * 100.0,
            dollars(r.start_price)
        ),
        "25+ trades and $25k+ in the leg".to_string(),
    ]
}

fn dollars(p: f64) -> String {
    if p >= 1.0 {
        format!("${p:.2}")
    } else {
        format!("${p:.4}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reasons_and_score() {
        let r = Run {
            start_ns: 0,
            start_price: 1.0,
            plus20_ns: 6 * 60_000_000_000,
            close: 1.25,
            exec_ns: 0,
            early: false,
        };
        assert!((score(&r) - 1.25).abs() < 1e-9);
        assert_eq!(reasons(&r)[0], "+25% in 6 min off $1.00");
        let fast = Run {
            plus20_ns: 15_000_000_000,
            start_price: 0.12,
            close: 0.15,
            ..r
        };
        assert_eq!(reasons(&fast)[0], "+25% in 15 s off $0.1200");
    }
}

//! Crossing-time context (owner, 2026-10-26, after WFF): the first Runners entry of a stock that
//! has just risen through the live entry floor ($0.10) says what the tape saw below it.
//!
//! WFF climbed from $0.0755 for more than an hour below the floor; the moment it crossed $0.10
//! four paths fired within 46 s, and none of them could say where the move came from. The 15 s
//! tape already keeps sub-dime prints ([`super::features`]'s wide band, $0.05 and up), so the
//! facts exist; this module only reads them (docs/research/wff-followup-2026-10-26.md).
//!
//! # What the line says, and what it never says
//!
//! `Crossed $0.10 2 min ago · 60-min low $0.0755 at 12:41 PM (+35.0%) · $412k traded below
//! $0.10`: when the stock last came up through the floor, the lowest low of the last hour and
//! when (ET, 12-hour), how far the alert price is above it, and the dollars traded in the bars
//! that stayed wholly below the floor. All causal: bars up to the entry only. No "base" or
//! "consolidation": no causal recognizer for those exists, and a hindsight label would read as
//! evidence it is not.
//!
//! # When
//!
//! Only on the stock's first Runners entry of the day (a row's own entry notice; later paths are
//! follow-ups and never get it), only when a bar of the last hour traded wholly below the floor,
//! the stock came back above it at most [`RECENT_MIN`] minutes before the entry, and the alert
//! price is at or above the floor. A stock still waiting for its history (restart, late
//! adoption) gets nothing: a short prefix would invent a low. One adopted with a structurally
//! incomplete prefix gets the line marked "partial history".

use market_events::PriceBand;

use super::features::{self, BAR_NS, Sym};

/// How far back the line looks: the hour a trader reads (the tape keeps 64 minutes).
pub const SPAN_SLOTS: u32 = 240;
/// A crossing older than this is not "just crossed": the entry is about what happened since.
pub const RECENT_MIN: i64 = 15;

/// The live entry floor in dollars (every Runners path admits from it, `PriceBand::DEFAULT`).
pub fn floor() -> f64 {
    PriceBand::DEFAULT.min.to_f64()
}

/// The crossing line for an entry of `sym` at `at_ns` and `price`, or `None` (module docs,
/// "When"). `t0` / `ord` are the trading date's 04:00 and ordinal ([`features::Tape::day`]).
pub(crate) fn line(sym: &Sym, at_ns: i64, price: f64, t0: i64, ord: u32) -> Option<String> {
    let floor = floor();
    if sym.unrestored || !(price.is_finite() && price >= floor) {
        return None;
    }
    // The entry's own slot: the bar its decision closed (at_ns is a barrier end or a print time).
    let slot = (at_ns - 1 - t0).div_euclid(BAR_NS);
    if slot < 0 {
        return None;
    }
    let through = features::key_of(ord, slot);
    let b = features::below_floor(sym, through, SPAN_SLOTS, floor)?;
    let start_ns = |key: u32| t0 + i64::from(key - ord * 4096) * BAR_NS;
    let crossed_ns = start_ns(b.crossed_key);
    let ago_min = (at_ns - crossed_ns).div_euclid(60_000_000_000);
    if ago_min > RECENT_MIN || b.low.is_nan() || b.low <= 0.0 {
        return None;
    }
    let ago = if ago_min < 1 {
        "under 1 min ago".to_string()
    } else {
        format!("{ago_min} min ago")
    };
    let pct = (price / b.low - 1.0) * 100.0;
    let mut text = format!(
        "Crossed {} {ago} · 60-min low {} at {} ({pct:+.1}%) · {} traded below {}",
        price_text(floor),
        price_text(b.low),
        et_clock(start_ns(b.low_key)),
        money(b.dollars_below),
        price_text(floor),
    );
    if sym.unavailable {
        text.push_str(" · partial history");
    }
    Some(text)
}

/// Sub-dollar prices at up to four decimals ($0.0755, the floor as $0.10), others at two ($1.23).
fn price_text(p: f64) -> String {
    if p >= 1.0 {
        return format!("${p:.2}");
    }
    let mut t = format!("{p:.4}");
    while t.ends_with('0') && t.len() > "0.00".len() {
        t.pop();
    }
    format!("${t}")
}

/// $950, $41k, $1.2M.
fn money(d: f64) -> String {
    if d >= 1_000_000.0 {
        format!("${:.1}M", d / 1_000_000.0)
    } else if d >= 1_000.0 {
        format!("${:.0}k", d / 1_000.0)
    } else {
        format!("${d:.0}")
    }
}

/// "12:41 PM" in ET, the 12-hour clock the owner reads (as the chart's crosshair tag).
fn et_clock(ns: i64) -> String {
    use chrono::TimeZone;
    let t = chrono_tz::America::New_York.timestamp_nanos(ns);
    t.format("%-I:%M %p").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::quotes::Counted;
    use chrono::NaiveDate;
    use market_events::session::et;
    use market_events::{Price, Trade, UnixNanos};

    const BOTH: Counted = Counted {
        ohlc: true,
        volume: true,
    };

    fn date() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 10, 26).unwrap()
    }

    /// Prints as (seconds after 04:00 ET, price, size).
    fn tape_with(prints: &[(i64, f64, u64)]) -> (features::Tape, i64, u32) {
        let mut tape = features::Tape::default();
        let (t0, ord) = tape.day(date());
        assert_eq!(t0, et(date(), 4, 0, 0, 0).unwrap().0);
        for (i, &(sec, price, size)) in prints.iter().enumerate() {
            let t = Trade::new(
                "WFF",
                UnixNanos(t0 + sec * 1_000_000_000),
                Price::from_micros((price * 1e6).round() as i64),
                size,
                i as u64 + 1,
            );
            tape.apply_trade(&t, BOTH);
        }
        (tape, t0, ord)
    }

    /// 13:00 ET is slot 9 h after 04:00.
    const H13: i64 = 9 * 3600;

    #[test]
    fn a_stock_that_rose_through_the_floor_gets_the_facts_from_below_it() {
        // An hour below the floor from $0.0755, then above it 2 minutes before the entry.
        let mut prints: Vec<(i64, f64, u64)> = (0..56)
            .map(|m| (H13 + m * 60, 0.0755 + m as f64 * 0.0004, 50_000))
            .collect();
        prints.push((H13 + 58 * 60, 0.1005, 80_000));
        prints.push((H13 + 59 * 60, 0.1019, 90_000));
        let (tape, t0, ord) = tape_with(&prints);
        let at = t0 + (H13 + 60 * 60) * 1_000_000_000;
        let s = tape.sym("WFF").unwrap();
        let text = line(s, at, 0.1019, t0, ord).expect("crossing facts");
        assert_eq!(
            text,
            "Crossed $0.10 2 min ago · 60-min low $0.0755 at 1:00 PM (+35.0%) · $242k traded below $0.10"
        );
    }

    #[test]
    fn no_line_without_time_below_the_floor_or_with_a_stale_crossing() {
        // Always above the floor.
        let prints: Vec<(i64, f64, u64)> = (0..30).map(|m| (H13 + m * 60, 0.12, 10_000)).collect();
        let (tape, t0, ord) = tape_with(&prints);
        let at = t0 + (H13 + 30 * 60) * 1_000_000_000;
        assert_eq!(line(tape.sym("WFF").unwrap(), at, 0.12, t0, ord), None);
        // Crossed 40 minutes before the entry: not "just crossed".
        let mut prints: Vec<(i64, f64, u64)> =
            (0..10).map(|m| (H13 + m * 60, 0.08, 10_000)).collect();
        prints.extend((10..50).map(|m| (H13 + m * 60, 0.11, 10_000)));
        let (tape, t0, ord) = tape_with(&prints);
        let at = t0 + (H13 + 50 * 60) * 1_000_000_000;
        assert_eq!(line(tape.sym("WFF").unwrap(), at, 0.11, t0, ord), None);
    }

    #[test]
    fn exactly_at_the_floor_counts_as_above_and_the_latest_crossing_is_the_one_reported() {
        // Below, at exactly $0.10, back below, then up again: the line dates the last crossing.
        let prints = [
            (H13, 0.09, 10_000),
            (H13 + 60, 0.10, 10_000),
            (H13 + 120, 0.095, 10_000),
            (H13 + 300, 0.10, 10_000),
        ];
        let (tape, t0, ord) = tape_with(&prints);
        let at = t0 + (H13 + 360) * 1_000_000_000;
        let text = line(tape.sym("WFF").unwrap(), at, 0.10, t0, ord).unwrap();
        assert!(
            text.starts_with("Crossed $0.10 1 min ago · 60-min low $0.09 at 1:00 PM"),
            "{text}"
        );
        // An entry price below the floor never gets it.
        assert_eq!(line(tape.sym("WFF").unwrap(), at, 0.0999, t0, ord), None);
    }

    #[test]
    fn history_pending_is_silent_and_an_incomplete_prefix_is_marked() {
        let prints = [(H13, 0.08, 10_000), (H13 + 600, 0.105, 10_000)];
        let (mut tape, t0, ord) = tape_with(&prints);
        let at = t0 + (H13 + 660) * 1_000_000_000;
        let i = tape.index["WFF"];
        tape.syms[i].unrestored = true;
        assert_eq!(line(&tape.syms[i], at, 0.105, t0, ord), None);
        tape.syms[i].unrestored = false;
        tape.syms[i].unavailable = true;
        assert!(
            line(&tape.syms[i], at, 0.105, t0, ord)
                .unwrap()
                .ends_with(" · partial history")
        );
    }

    #[test]
    fn twelve_hour_clock_and_money() {
        assert_eq!(money(950.0), "$950");
        assert_eq!(money(41_200.0), "$41k");
        assert_eq!(money(1_240_000.0), "$1.2M");
        assert_eq!(price_text(0.0755), "$0.0755");
        assert_eq!(price_text(0.10), "$0.10");
        assert_eq!(price_text(0.1019), "$0.1019");
        assert_eq!(price_text(1.234), "$1.23");
    }
}

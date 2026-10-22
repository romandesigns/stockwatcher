//! The Ross Panel row chart's day line: one close per completed interval from the date's first
//! price, quiet intervals carried, overnight and the forming interval left out.
mod common;

use chart_engine::{ChartEngine, Timeframe};
use common::*;
use market_events::{SessionKind, UnixNanos};

fn closes(e: &ChartEngine) -> Vec<String> {
    let line = e.day_line(SYM, Timeframe::S30).expect("a line");
    line.closes.iter().map(|p| p.to_string()).collect()
}

#[test]
fn day_line_is_index_aligned_and_ends_at_the_last_completed_interval() {
    let mut e = ChartEngine::default();
    apply_all(
        &mut e,
        &[
            // Overnight (20:00–04:00 belongs to the date but not to its extended day).
            ev(trade(at(3, 59, 0, 0), "2.50", 100, 1)),
            ev(trade(at(4, 0, 5, 0), "3.00", 100, 2)),
            ev(trade(at(4, 0, 40, 0), "3.10", 100, 3)),
            // 04:01:00–04:02:00: nothing traded, the close carries.
            ev(trade(at(4, 2, 10, 0), "3.20", 100, 4)),
            // Forming at the clock below: not a close yet.
            ev(trade(at(4, 3, 5, 0), "3.40", 100, 5)),
        ],
    );
    e.advance_clock(at(4, 3, 15, 0));
    let line = e.day_line(SYM, Timeframe::S30).expect("a line");
    assert_eq!(line.start, at(4, 0, 0, 0));
    assert_eq!(line.step, Timeframe::S30);
    assert_eq!(line.session.kind, SessionKind::Pre);
    assert_eq!(line.session.date, day());
    assert_eq!(closes(&e), ["3.00", "3.10", "3.10", "3.10", "3.20", "3.20"]);

    // The clock moving on completes the forming interval and carries its close to the clock.
    e.advance_clock(at(4, 4, 31, 0));
    assert_eq!(
        closes(&e),
        [
            "3.00", "3.10", "3.10", "3.10", "3.20", "3.20", "3.40", "3.40", "3.40"
        ]
    );
}

#[test]
fn day_line_stops_at_the_dates_last_session() {
    let mut e = ChartEngine::default();
    apply_all(&mut e, &[ev(trade(at(19, 59, 0, 0), "5.00", 100, 1))]);
    // Long after 20:00: the line ends with the after-hours session, it does not run on.
    e.advance_clock(UnixNanos(at(19, 59, 0, 0).0 + 3 * 3600 * 1_000_000_000));
    let line = e.day_line(SYM, Timeframe::S30).unwrap();
    assert_eq!(line.start, at(19, 59, 0, 0));
    assert_eq!(line.closes.len(), 2);
}

#[test]
fn no_line_without_a_price_or_a_symbol() {
    let e = ChartEngine::default();
    assert!(e.day_line(SYM, Timeframe::S30).is_none());
}

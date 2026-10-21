//! The admission ledger's bounds and the per-symbol coverage they leave behind: an idle symbol
//! gives its ledger back on the clock, a symbol far outside the band keeps a short one, and what
//! is known about a date's collection does not outlive the date. Synthetic, fictional symbols.

use market_events::session::et;
use market_events::{Price, Trade, UnixNanos};
use stock_context::engine::{LEDGER_MAX_OUTSIDE_BAND, LEDGER_WINDOW_SECS};
use stock_context::*;

const SEC: i64 = 1_000_000_000;
const VOL: Verdict = Verdict {
    ohlc: true,
    volume: true,
};

fn day() -> chrono::NaiveDate {
    chrono::NaiveDate::from_ymd_opt(2026, 10, 15).expect("valid")
}

fn t(h: u32, m: u32, s: u32) -> i64 {
    et(day(), h, m, s, 0).expect("valid").nanos()
}

fn engine() -> Engine {
    Engine::new(
        Policy::current(),
        RunInfo {
            run_id: "fixture".into(),
            source: "live".into(),
            fidelity: Fidelity::RecordedLive,
            numerator_coverage: Coverage::Unverified,
        },
    )
}

fn print(e: &mut Engine, sym: &str, seq: u64, exec: i64, cents: i64) {
    let t = Trade::new(sym, UnixNanos(exec), Price::from_cents(cents), 100, seq);
    e.apply(&t, VOL, UnixNanos(exec + 100_000_000));
}

/// A symbol that stops trading releases its ledger on the clock, not only on its own next print.
#[test]
fn an_idle_symbols_ledger_is_released_by_the_clock() {
    let mut e = engine();
    for i in 0..500 {
        print(
            &mut e,
            "IDLE",
            i + 1,
            t(9, 30, 0) + i as i64 * SEC / 10,
            500,
        );
    }
    e.advance_to(UnixNanos(t(9, 31, 0)));
    assert_eq!(e.ledger_footprint().0, 500);
    // Past the window with no print of its own: the next minute's sweep retires all of it.
    e.advance_to(UnixNanos(t(9, 31, 0) + (LEDGER_WINDOW_SECS + 120) * SEC));
    let (entries, bytes) = e.ledger_footprint();
    assert_eq!((entries, bytes), (0, 0), "and the allocation goes with it");
    // The day's facts are untouched: retiring identities is not un-counting volume.
    let d = e
        .current()
        .find(|d| d.symbol == "IDLE")
        .expect("a decision");
    assert_eq!(d.volume.shares, 50_000);
}

/// A symbol printing far above the band keeps a short ledger; one inside keeps the full one.
#[test]
fn a_symbol_far_outside_the_band_keeps_a_short_ledger() {
    let mut e = engine();
    let n = (LEDGER_MAX_OUTSIDE_BAND + 500) as u64;
    for i in 0..n {
        let at = t(9, 30, 0) + i as i64 * SEC / 100;
        print(&mut e, "HIGH", i + 1, at, 40_000); // $400
        print(&mut e, "BAND", i + 1, at, 500); // $5
    }
    // Fifteen hundred prints inside one bar: all still queued, none may be dropped for the
    // shorter bound (that would be volume never counted).
    assert_eq!(e.ledger_footprint().0, 2 * n as usize);
    e.advance_to(UnixNanos(t(9, 31, 0)));
    let high = e
        .current()
        .find(|d| d.symbol == "HIGH")
        .expect("a decision");
    assert_eq!(
        high.volume.shares,
        100 * n,
        "a short ledger retires identities, never uncounted volume"
    );
    assert_eq!(e.revision_telemetry().queued_evicted, 0);
    // Folded now: the next print trims the outside-band ledger to its bound.
    print(&mut e, "HIGH", n + 1, t(9, 31, 1), 40_000);
    print(&mut e, "BAND", n + 1, t(9, 31, 1), 500);
    let (entries, _) = e.ledger_footprint();
    assert_eq!(entries, LEDGER_MAX_OUTSIDE_BAND + (n + 1) as usize);
}

/// What was known about how completely one date was collected is not a statement about the next.
/// `Partial` makes Ross momentum unavailable, and it used to stay for the life of the process.
#[test]
fn a_dates_partial_coverage_does_not_outlive_the_date() {
    let mut e = engine();
    print(&mut e, "SHRT", 1, t(9, 30, 0), 500);
    e.advance_to(UnixNanos(t(9, 31, 0)));
    e.set_coverage("SHRT", Some((Coverage::Partial, "recorder_drops")));
    print(&mut e, "SHRT", 2, t(9, 32, 0), 500);
    e.advance_to(UnixNanos(t(9, 33, 0)));
    assert_eq!(
        e.coverage_of("SHRT"),
        Some((Coverage::Partial, "recorder_drops")),
        "it holds for the rest of its own date"
    );
    let next = day().succ_opt().expect("valid");
    let n9 = et(next, 9, 30, 0, 0).expect("valid").nanos();
    print(&mut e, "SHRT", 3, n9, 500);
    e.advance_to(UnixNanos(n9 + 60 * SEC));
    // (The clock retired yesterday's identities overnight, which may be named at the run's own
    // coverage; what must not survive is yesterday's Partial.)
    assert_ne!(
        e.coverage_of("SHRT").map(|(c, _)| c),
        Some(Coverage::Partial),
        "and not into the next: {:?}",
        e.coverage_of("SHRT")
    );
}

/// A late print past the retained horizon is refused; when it would have counted, coverage says
/// the count is short — as `Unverified`, not `Partial`, because an old print says nothing about
/// the recent prices momentum reads.
#[test]
fn a_quarantined_old_print_is_not_partial_coverage() {
    let mut e = engine();
    print(&mut e, "LATE", 1, t(9, 30, 0), 500);
    print(
        &mut e,
        "LATE",
        2,
        t(9, 30, 0) + (LEDGER_WINDOW_SECS + 60) * SEC,
        500,
    );
    // Executed before the retired print: it cannot be proved new, so it is refused.
    print(&mut e, "LATE", 3, t(9, 29, 0), 500);
    assert!(e.revision_telemetry().quarantined_old >= 1);
    assert_eq!(
        e.coverage_of("LATE"),
        Some((Coverage::Unverified, "quarantined_old_prints"))
    );
}

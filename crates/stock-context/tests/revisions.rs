//! One identity rule and one revision contract, for both paths a print reaches the engine by.
//!
//! The engine took whatever verdict its caller had. On the server that is the chart engine's for
//! a symbol whose history is loaded and the print's own conditions for one still loading, and the
//! second has no duplicate exclusion — so a reconnect replay, which redelivers prints by design,
//! was counted twice in the RVOL numerator during the startup window (the audit's F10). And a
//! `Correction` or `Cancel` never reached the engine at all: the server routed revisions to the
//! quote board only, so the scanner kept a cancelled print's volume, trades and day high for the
//! rest of the day while the candles and the quote row agreed it was gone (F3). An inflated
//! numerator is not a safe lower bound, so "missing prints can only lower RVOL" did not hold.
//!
//! What this pins: the engine deduplicates on its own identity whatever the caller says, a
//! revision moves the current facts by exactly the delta the resolver gives it, and nothing
//! already decided is rewritten.

use chrono::NaiveDate;
use market_events::session::et;
use market_events::{Price, Trade, UnixNanos};
use stock_context::*;

const VOL: Verdict = Verdict {
    ohlc: true,
    volume: true,
};
const SEC: i64 = 1_000_000_000;

fn day() -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 10, 16).expect("valid")
}
fn t(h: u32, m: u32, s: u32) -> i64 {
    et(day(), h, m, s, 0).expect("valid").nanos()
}

fn engine() -> Engine {
    Engine::new(
        Policy::v1(),
        RunInfo {
            run_id: "fixture".into(),
            source: "live".into(),
            fidelity: Fidelity::RecordedLive,
            numerator_coverage: Coverage::Complete,
        },
    )
}

/// A print with an explicit provider sequence, so its identity is controllable.
fn print_seq(e: &mut Engine, seq: u64, exec: i64, cents: i64, size: u64) -> Vec<Decision> {
    e.apply(
        &Trade::new("ABCD", UnixNanos(exec), Price::from_cents(cents), size, seq),
        VOL,
        UnixNanos(exec + 100_000_000),
    )
}

fn facts(e: &Engine) -> (u64, u64, u64) {
    let d = e.current().next().expect("a decision");
    (d.volume.shares, d.volume.trades, d.volume.dollars)
}

fn revised(exec: i64, cents: i64, size: u64, verdict: Verdict) -> RevisedPrint {
    RevisedPrint {
        exec,
        price: Price::from_cents(cents).micros(),
        size,
        verdict,
    }
}

/// A cancel of the print at `exec`, with the date's last price and high after it.
fn cancel(
    exec: i64,
    cents: i64,
    size: u64,
    last: Option<(i64, i64)>,
    day_high: Option<DayHigh>,
) -> PrintRevision {
    PrintRevision {
        symbol: "ABCD".into(),
        date: day(),
        removed: revised(exec, cents, size, VOL),
        added: None,
        last,
        day_high,
    }
}

fn high(cents: i64, complete: bool) -> Option<DayHigh> {
    Some(DayHigh {
        price: Price::from_cents(cents).micros(),
        complete,
    })
}

#[test]
fn a_redelivered_print_is_counted_once_whatever_the_caller_says() {
    let mut e = engine();
    print_seq(&mut e, 1, t(9, 30, 1), 500, 100);
    // The same identity again — same provider reference, same execution nanosecond — which is a
    // redelivery by definition (`market_events::PrintId`), whatever verdict the caller passes.
    print_seq(&mut e, 1, t(9, 30, 1), 500, 100);
    print_seq(&mut e, 2, t(9, 30, 2), 500, 50);
    e.advance_to(UnixNanos(t(9, 30, 20)));
    assert_eq!(facts(&e), (150, 2, 750), "100 + 50, counted twice never");
    assert_eq!(e.revision_telemetry().duplicates_dropped, 1);
}

#[test]
fn the_identity_set_is_bounded_and_a_new_date_starts_over() {
    let mut e = engine();
    print_seq(&mut e, 1, t(9, 30, 1), 500, 100);
    // Far enough past the dedup window that the identity can no longer be redelivered: it is
    // evicted, so a print repeating it is applied rather than dropped. Not a real case — a
    // replay redelivers within seconds — but it is what keeps the memory bounded.
    let later = t(9, 30, 1) + (DEDUP_WINDOW_SECS + 60) * SEC;
    print_seq(&mut e, 2, later, 500, 10);
    print_seq(&mut e, 1, t(9, 30, 1), 500, 100);
    e.advance_to(UnixNanos(later + 20 * SEC));
    assert_eq!(
        e.revision_telemetry().duplicates_dropped,
        0,
        "the identity was evicted, so nothing was called a duplicate"
    );
}

#[test]
fn a_cancel_takes_the_print_back_out_of_the_current_facts() {
    let mut e = engine();
    print_seq(&mut e, 1, t(9, 30, 1), 500, 100);
    print_seq(&mut e, 2, t(9, 30, 2), 600, 200);
    e.advance_to(UnixNanos(t(9, 30, 20)));
    assert_eq!(facts(&e), (300, 2, 1_700), "100 * $5 + 200 * $6");

    // The 200-share print at $6.00 is cancelled; the resolver recomputes the date's last price
    // and high from what it retains.
    e.apply_revision(
        &cancel(
            t(9, 30, 2),
            600,
            200,
            Some((Price::from_cents(500).micros(), t(9, 30, 1))),
            high(500, true),
        ),
        UnixNanos(t(9, 30, 25)),
    );
    e.advance_to(UnixNanos(t(9, 30, 40)));
    assert_eq!(
        facts(&e),
        (100, 1, 500),
        "exactly the day without the cancelled print"
    );
    let tel = e.revision_telemetry();
    assert_eq!(tel.applied, 1);
    assert!(tel.complete(), "{tel:?}");
}

/// Parity: the same day with a cancelled print, and without it, agree on the current facts.
#[test]
fn a_cancelled_day_equals_the_day_without_the_print() {
    let with = {
        let mut e = engine();
        print_seq(&mut e, 1, t(9, 30, 1), 500, 100);
        print_seq(&mut e, 2, t(9, 30, 2), 600, 200);
        print_seq(&mut e, 3, t(9, 30, 3), 510, 300);
        e.advance_to(UnixNanos(t(9, 30, 20)));
        e.apply_revision(
            &cancel(
                t(9, 30, 2),
                600,
                200,
                Some((Price::from_cents(510).micros(), t(9, 30, 3))),
                high(510, true),
            ),
            UnixNanos(t(9, 30, 25)),
        );
        e.advance_to(UnixNanos(t(9, 31, 0)));
        facts(&e)
    };
    let without = {
        let mut e = engine();
        print_seq(&mut e, 1, t(9, 30, 1), 500, 100);
        print_seq(&mut e, 3, t(9, 30, 3), 510, 300);
        e.advance_to(UnixNanos(t(9, 31, 0)));
        facts(&e)
    };
    assert_eq!(with, without, "the cancelled print is gone from both");
}

#[test]
fn a_correction_applies_the_delta_both_ways() {
    let mut e = engine();
    print_seq(&mut e, 1, t(9, 30, 1), 500, 100);
    e.advance_to(UnixNanos(t(9, 30, 20)));
    assert_eq!(facts(&e), (100, 1, 500));

    // 100 shares at $5.00 corrected to 40 at $5.50.
    e.apply_revision(
        &PrintRevision {
            symbol: "ABCD".into(),
            date: day(),
            removed: revised(t(9, 30, 1), 500, 100, VOL),
            added: Some(revised(t(9, 30, 1), 550, 40, VOL)),
            last: Some((Price::from_cents(550).micros(), t(9, 30, 1))),
            day_high: high(550, true),
        },
        UnixNanos(t(9, 30, 25)),
    );
    e.advance_to(UnixNanos(t(9, 30, 40)));
    assert_eq!(facts(&e), (40, 1, 220), "40 * $5.50");
    assert_eq!(e.revision_telemetry().applied, 1);
}

/// A correction into a condition that no longer counts volume removes it and adds nothing.
#[test]
fn a_correction_into_a_non_counting_condition_removes_the_volume() {
    let mut e = engine();
    print_seq(&mut e, 1, t(9, 30, 1), 500, 100);
    print_seq(&mut e, 2, t(9, 30, 2), 500, 100);
    e.advance_to(UnixNanos(t(9, 30, 20)));
    e.apply_revision(
        &PrintRevision {
            symbol: "ABCD".into(),
            date: day(),
            removed: revised(t(9, 30, 2), 500, 100, VOL),
            added: Some(revised(
                t(9, 30, 2),
                500,
                100,
                Verdict {
                    ohlc: false,
                    volume: false,
                },
            )),
            last: Some((Price::from_cents(500).micros(), t(9, 30, 1))),
            day_high: high(500, true),
        },
        UnixNanos(t(9, 30, 25)),
    );
    e.advance_to(UnixNanos(t(9, 30, 40)));
    assert_eq!(facts(&e), (100, 1, 500));
}

#[test]
fn decisions_already_emitted_are_never_rewritten() {
    let mut e = engine();
    print_seq(&mut e, 1, t(9, 30, 1), 500, 100);
    print_seq(&mut e, 2, t(9, 30, 2), 600, 200);
    let before: Vec<Decision> = e.advance_to(UnixNanos(t(9, 30, 20)));
    let frozen = before.clone();
    assert_eq!(before.last().expect("a decision").volume.shares, 300);

    e.apply_revision(
        &cancel(
            t(9, 30, 2),
            600,
            200,
            Some((Price::from_cents(500).micros(), t(9, 30, 1))),
            high(500, true),
        ),
        UnixNanos(t(9, 30, 25)),
    );
    let after = e.advance_to(UnixNanos(t(9, 30, 40)));
    assert_eq!(before, frozen, "the emitted decisions are the same objects");
    assert_eq!(
        after.last().expect("a decision").volume.shares,
        100,
        "the change is forward-effective"
    );
    assert!(
        after.last().expect("a decision").cutoff > t(9, 30, 20),
        "and lands at a later barrier"
    );
}

#[test]
fn a_day_high_the_resolver_could_not_prove_is_counted_not_guessed() {
    let mut e = engine();
    print_seq(&mut e, 1, t(9, 30, 1), 500, 100);
    print_seq(&mut e, 2, t(9, 30, 2), 900, 200);
    e.advance_to(UnixNanos(t(9, 30, 20)));

    // The $9.00 print — the day's high — is cancelled, but the resolver's retention no longer
    // reaches the date's start, so its figure is a lower bound rather than the high.
    e.apply_revision(
        &cancel(
            t(9, 30, 2),
            900,
            200,
            Some((Price::from_cents(500).micros(), t(9, 30, 1))),
            high(500, false),
        ),
        UnixNanos(t(9, 30, 25)),
    );
    e.advance_to(UnixNanos(t(9, 30, 40)));
    assert_eq!(
        facts(&e),
        (100, 1, 500),
        "the volume side is exact either way"
    );
    let tel = e.revision_telemetry();
    assert_eq!(tel.applied, 1);
    assert_eq!(tel.day_high_unproven, 1);
    assert!(
        !tel.complete(),
        "an incomplete repair is reported, not assumed away: {tel:?}"
    );
}

#[test]
fn a_revision_for_a_symbol_or_date_the_engine_does_not_hold_is_counted() {
    let mut e = engine();
    print_seq(&mut e, 1, t(9, 30, 1), 500, 100);
    e.advance_to(UnixNanos(t(9, 30, 20)));

    // A symbol that never traded here.
    let mut other = cancel(t(9, 30, 1), 500, 100, None, None);
    other.symbol = "ZZZZ".into();
    e.apply_revision(&other, UnixNanos(t(9, 30, 25)));
    assert_eq!(e.revision_telemetry().unknown_target, 1);

    // The right symbol, another trading date: the facts it speaks about are not the current ones
    // and the old decisions stand.
    let mut old = cancel(t(9, 30, 1), 500, 100, None, None);
    old.date = day().pred_opt().expect("valid");
    e.apply_revision(&old, UnixNanos(t(9, 30, 30)));
    let tel = e.revision_telemetry();
    assert_eq!(tel.too_late, 1);
    assert_eq!(tel.applied, 0, "neither was applied");
    assert_eq!(facts(&e), (100, 1, 500), "and nothing moved");
}

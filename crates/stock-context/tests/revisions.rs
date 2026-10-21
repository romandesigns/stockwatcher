//! One admission ledger: identity, revision resolution and the retained-window state.
//!
//! The engine took a print with whatever verdict its caller had. On the server that is the chart
//! engine's for a symbol whose history is loaded and the print's own conditions for one still
//! loading, and the second has no duplicate exclusion — so a reconnect replay, which redelivers
//! prints by design, was counted twice in the RVOL numerator during the startup window (the
//! audit's F10). And a `Correction` or `Cancel` never reached the engine at all: the server routed
//! revisions to the quote board only, so the scanner kept a cancelled print's volume, trades and
//! day high for the rest of the day while the candles and the quote row agreed it was gone (F3).
//! An inflated numerator is not a safe lower bound, so "missing prints can only lower RVOL" did
//! not hold.
//!
//! The queue, the duplicate index, the revision resolver and the source of the retained-window
//! state are **one** structure, because they have to agree with each other. What this file pins:
//!
//! - a redelivery is dropped on identity, whatever the caller says, and whenever it arrives;
//! - a revision is resolved against the prints this engine admitted — never applied as a delta
//!   on trust, so it cannot subtract volume from a print the engine never counted;
//! - a print still queued is revised in the queue, so a cancel that beats its barrier is not
//!   lost and the print does not count afterwards;
//! - the cumulative checkpoints, price history and setup bars the next decisions read are made
//!   exact again, so a later recent-window RVOL agrees with the same stream without the print;
//! - nothing already decided is rewritten, and every repair the engine could not make is counted.

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

fn trade(seq: u64, exec: i64, cents: i64, size: u64) -> Trade {
    Trade::new("ABCD", UnixNanos(exec), Price::from_cents(cents), size, seq)
}

fn print_seq(e: &mut Engine, seq: u64, exec: i64, cents: i64, size: u64) -> Vec<Decision> {
    e.apply(
        &trade(seq, exec, cents, size),
        VOL,
        UnixNanos(exec + 100_000_000),
    )
}

fn facts(e: &Engine) -> (u64, u64, u64) {
    let d = e.current().next().expect("a decision");
    (d.volume.shares, d.volume.trades, d.volume.dollars)
}

/// The provider's reference to the print `seq` at `exec`, as a correction or cancel carries it.
fn target(seq: u64, cents: i64, size: u64) -> RevisionTarget {
    RevisionTarget {
        id: trade(seq, 0, cents, size).id(),
        price: Price::from_cents(cents).micros(),
        size,
    }
}

fn high(cents: i64, complete: bool) -> Option<DayHigh> {
    Some(DayHigh {
        price: Price::from_cents(cents).micros(),
        complete,
    })
}

fn cancel(seq: u64, cents: i64, size: u64, day_high: Option<DayHigh>) -> PrintRevision {
    PrintRevision {
        symbol: "ABCD".into(),
        target: target(seq, cents, size),
        corrected: None,
        last: None,
        day_high,
    }
}

fn correct(
    seq: u64,
    cents: i64,
    size: u64,
    to_cents: i64,
    to_size: u64,
    verdict: Verdict,
) -> PrintRevision {
    PrintRevision {
        symbol: "ABCD".into(),
        target: target(seq, cents, size),
        corrected: Some(CorrectedPrint {
            price: Price::from_cents(to_cents).micros(),
            size: to_size,
            verdict,
        }),
        last: None,
        day_high: high(to_cents, true),
    }
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

/// A redelivery long after the first bar is still a duplicate.
///
/// The identity set used to be cleared inside `evaluate`, on the date change — **after** the
/// date's first prints had already been queued with their identities recorded. Those identities
/// were lost, so a redelivery arriving after that first barrier counted twice. The ledger is not
/// cleared on a date change at all: its entries carry their own session date.
#[test]
fn a_redelivery_after_the_first_barrier_is_still_a_duplicate() {
    let mut e = engine();
    print_seq(&mut e, 1, t(9, 30, 1), 500, 100);
    print_seq(&mut e, 2, t(9, 30, 2), 500, 200);
    // The first barrier of the date runs here, which is where the identities used to be dropped.
    e.advance_to(UnixNanos(t(9, 30, 20)));
    assert_eq!(facts(&e), (300, 2, 1_500));

    // Minutes later the stream replays both prints after a reconnect.
    print_seq(&mut e, 1, t(9, 30, 1), 500, 100);
    print_seq(&mut e, 2, t(9, 30, 2), 500, 200);
    e.advance_to(UnixNanos(t(9, 35, 0)));
    assert_eq!(
        facts(&e),
        (300, 2, 1_500),
        "the redelivery is still a redelivery"
    );
    assert_eq!(e.revision_telemetry().duplicates_dropped, 2);
}

#[test]
fn a_cancel_takes_the_print_back_out_of_the_current_facts() {
    let mut e = engine();
    print_seq(&mut e, 1, t(9, 30, 1), 500, 100);
    print_seq(&mut e, 2, t(9, 30, 2), 600, 200);
    e.advance_to(UnixNanos(t(9, 30, 20)));
    assert_eq!(facts(&e), (300, 2, 1_700), "100 * $5 + 200 * $6");

    e.apply_revision(
        &cancel(2, 600, 200, high(500, true)),
        UnixNanos(t(9, 30, 25)),
    );
    e.advance_to(UnixNanos(t(9, 30, 40)));
    assert_eq!(
        facts(&e),
        (100, 1, 500),
        "exactly the day without the cancelled print"
    );
    let tel = e.revision_telemetry();
    assert_eq!((tel.applied, tel.applied_queued), (1, 0));
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
            &cancel(2, 600, 200, high(510, true)),
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
    e.apply_revision(&correct(1, 500, 100, 550, 40, VOL), UnixNanos(t(9, 30, 25)));
    e.advance_to(UnixNanos(t(9, 30, 40)));
    assert_eq!(facts(&e), (40, 1, 220), "40 * $5.50");
    assert_eq!(e.revision_telemetry().applied, 1);
}

#[test]
fn a_correction_into_a_non_counting_condition_removes_the_volume() {
    let mut e = engine();
    print_seq(&mut e, 1, t(9, 30, 1), 500, 100);
    print_seq(&mut e, 2, t(9, 30, 2), 500, 100);
    e.advance_to(UnixNanos(t(9, 30, 20)));
    e.apply_revision(
        &correct(
            2,
            500,
            100,
            500,
            100,
            Verdict {
                ohlc: false,
                volume: false,
            },
        ),
        UnixNanos(t(9, 30, 25)),
    );
    e.advance_to(UnixNanos(t(9, 30, 40)));
    assert_eq!(facts(&e), (100, 1, 500));
}

/// A cancel that beats its print's barrier is applied in the queue, so the print never counts.
///
/// The engine used to need `sym.date`, which is only set once a barrier has run, so a cancel
/// arriving before the first barrier was discarded as too late — and the queued print was then
/// counted at that barrier anyway, exactly the volume the cancel said was not there.
#[test]
fn a_cancel_that_beats_its_barrier_is_applied_in_the_queue() {
    let mut e = engine();
    print_seq(&mut e, 1, t(9, 30, 1), 500, 100);
    print_seq(&mut e, 2, t(9, 30, 2), 500, 200);
    // No barrier has run yet: both prints are queued and the symbol has no date.
    e.apply_revision(&cancel(2, 500, 200, None), UnixNanos(t(9, 30, 3)));
    let tel = e.revision_telemetry();
    assert_eq!(
        (tel.applied_queued, tel.too_late, tel.unknown_target),
        (1, 0, 0),
        "placed in the queue, not discarded: {tel:?}"
    );
    e.advance_to(UnixNanos(t(9, 30, 20)));
    assert_eq!(
        facts(&e),
        (100, 1, 500),
        "the cancelled print never counted at all"
    );
}

/// A correction that beats the barrier counts the corrected print, once.
#[test]
fn a_correction_that_beats_its_barrier_counts_the_corrected_print() {
    let mut e = engine();
    print_seq(&mut e, 1, t(9, 30, 1), 500, 100);
    e.apply_revision(&correct(1, 500, 100, 500, 25, VOL), UnixNanos(t(9, 30, 2)));
    e.advance_to(UnixNanos(t(9, 30, 20)));
    assert_eq!(facts(&e), (25, 1, 125));
    assert_eq!(e.revision_telemetry().applied_queued, 1);
}

/// A revision naming a print this engine never admitted subtracts nothing.
///
/// It used to check only the symbol and the date before applying a delta, so a cancel of a print
/// from before this engine existed — a restart, a symbol still loading, anything past the
/// ledger's horizon — took volume off an unrelated print.
#[test]
fn a_revision_for_a_print_the_engine_never_admitted_changes_nothing() {
    let mut e = engine();
    print_seq(&mut e, 1, t(9, 30, 1), 500, 100);
    e.advance_to(UnixNanos(t(9, 30, 20)));
    assert_eq!(facts(&e), (100, 1, 500));

    // A print this engine never saw (it belongs to the history before it started).
    e.apply_revision(&cancel(99, 700, 400, None), UnixNanos(t(9, 30, 25)));
    // A symbol it does not hold at all.
    let mut elsewhere = cancel(1, 500, 100, None);
    elsewhere.symbol = "ZZZZ".into();
    e.apply_revision(&elsewhere, UnixNanos(t(9, 30, 26)));
    // The right reference, the wrong price and size: a print with that reference is held but
    // none of them fits, which is `Resolution::Ambiguous`'s own case — applying one would be a
    // guess either way, so nothing is applied.
    e.apply_revision(&cancel(1, 800, 100, None), UnixNanos(t(9, 30, 27)));

    e.advance_to(UnixNanos(t(9, 30, 40)));
    assert_eq!(facts(&e), (100, 1, 500), "nothing was subtracted");
    let tel = e.revision_telemetry();
    assert_eq!((tel.unknown_target, tel.ambiguous), (2, 1), "{tel:?}");
    assert_eq!(tel.applied, 0);
    assert!(!tel.complete());
}

/// A reference fitting more than one retained print is never guessed at.
#[test]
fn an_ambiguous_reference_is_counted_not_guessed() {
    let mut e = engine();
    // Two prints of the same venue reference, price and size: a TRF reference can do this.
    print_seq(&mut e, 7, t(9, 30, 1), 500, 100);
    print_seq(&mut e, 7, t(9, 30, 2), 500, 100);
    e.advance_to(UnixNanos(t(9, 30, 20)));
    assert_eq!(facts(&e), (200, 2, 1_000));
    e.apply_revision(&cancel(7, 500, 100, None), UnixNanos(t(9, 30, 25)));
    e.advance_to(UnixNanos(t(9, 30, 40)));
    assert_eq!(facts(&e), (200, 2, 1_000), "nothing moved");
    let tel = e.revision_telemetry();
    assert_eq!(tel.ambiguous, 1);
    assert!(!tel.complete());
}

/// The cumulative checkpoints the recent window reads are made exact again.
///
/// The deltas used to touch the day's totals only, leaving the checkpoints holding the cancelled
/// print. Once the print's execution is older than the window's start, the checkpoint at that
/// start still counted it while the running total no longer did, so `cum - checkpoint`
/// understated the window — and with the total below the checkpoint it underflowed outright.
///
/// The boundary is walked on both sides of the cancelled print's execution: once with it inside
/// the window, once with it behind the window's start.
#[test]
fn a_cancel_fixes_the_recent_window_on_both_sides_of_its_execution() {
    let mut sessions = Vec::new();
    let mut d = day().pred_opt().expect("valid");
    while sessions.len() < 10 {
        if market_events::is_trading_day(d) {
            // Seconds since 04:00: 19,800 = 09:35, 20,100 = 09:40.
            sessions.push(IntradayCurve {
                date: d,
                cum: vec![(0, 0), (19_800, 100), (20_100, 200)],
                complete: true,
            });
        }
        d = d.pred_opt().expect("valid");
    }
    let mut e = engine();
    e.add_intraday(IntradayBaseline {
        id: "intraday:ABCD".into(),
        version: 1,
        instrument: InstrumentId::symbol("ABCD"),
        sessions,
        basis: Default::default(),
        observed_at: t(4, 0, 0),
    });
    let recent = |e: &Engine| e.current().next().expect("a decision").rvol.recent_milli;

    // An early print so a cumulative checkpoint exists before the window's start: without one
    // the window is honestly `window_not_covered` and there is nothing to get wrong.
    print_seq(&mut e, 9, t(9, 29, 0), 500, 100);
    // 400 shares at 09:31 and 100 at 09:34, both inside the window at the barrier below.
    print_seq(&mut e, 1, t(9, 31, 0), 500, 400);
    print_seq(&mut e, 2, t(9, 34, 0), 500, 100);
    e.advance_to(UnixNanos(t(9, 35, 20)));
    assert_eq!(
        recent(&e),
        Some(5_000),
        "500 in the window / a 100-share prior mean, before the cancel"
    );

    // The 400-share print is cancelled. Still inside the window, and the checkpoint before it
    // was never affected, so the window is simply 100 shares now.
    e.apply_revision(
        &cancel(1, 500, 400, high(500, true)),
        UnixNanos(t(9, 35, 21)),
    );
    e.advance_to(UnixNanos(t(9, 35, 40)));
    assert_eq!(recent(&e), Some(1_000), "100 / 100 with it gone");

    // Now past 09:36:45, so the cancelled print's execution is **behind** the window's start and
    // the checkpoint at that start is one that was taken after it. With the checkpoints left
    // holding the cancelled 400 this is where the window went wrong.
    print_seq(&mut e, 3, t(9, 37, 0), 500, 100);
    e.advance_to(UnixNanos(t(9, 38, 0)));
    let d = e.current().next().expect("a decision");
    assert_eq!(
        d.volume.shares, 300,
        "100 + 100 + 100, the cancelled 400 gone"
    );
    // The checkpoint at the window's start was taken after the cancelled print. Left holding
    // its 400 shares it would exceed the day's total of 300, and `cum - checkpoint` on unsigned
    // volume would not merely understate the window, it would underflow.
    assert_eq!(
        d.rvol.recent_milli,
        Some(2_000),
        "200 shares in the window against a 100-share prior mean: {:?}",
        d.rvol
    );
    assert_eq!(d.rvol.recent_reason, "ok");
    let tel = e.revision_telemetry();
    assert_eq!(tel.applied, 1);
    assert_eq!(
        tel.windows_unproven, 0,
        "the ledger reached the window's start, so the rebuild was exact: {tel:?}"
    );
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
        &cancel(2, 600, 200, high(500, true)),
        UnixNanos(t(9, 30, 25)),
    );
    let after = e.advance_to(UnixNanos(t(9, 30, 40)));
    assert_eq!(before, frozen, "the emitted decisions are the same objects");
    assert_eq!(
        after.last().expect("a decision").volume.shares,
        100,
        "the change is forward-effective"
    );
    assert!(after.last().expect("a decision").cutoff > t(9, 30, 20));
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
        &cancel(2, 900, 200, high(500, false)),
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

/// The ledger is bounded by time as well as by entries, and a retired identity is named.
#[test]
fn the_ledger_is_bounded_and_says_what_it_retired() {
    let mut e = engine();
    print_seq(&mut e, 1, t(9, 30, 1), 500, 100);
    e.advance_to(UnixNanos(t(9, 30, 20)));
    // Far past the revision window: the first print's identity is retired, so a redelivery of it
    // can no longer be proved and a revision naming it can no longer be placed.
    let later = t(9, 30, 1) + (REVISION_WINDOW_SECS + 60) * SEC;
    print_seq(&mut e, 2, later, 500, 10);
    e.advance_to(UnixNanos(later + 20 * SEC));
    let tel = e.revision_telemetry();
    assert!(tel.identities_evicted >= 1, "{tel:?}");
    assert_eq!(tel.queued_evicted, 0, "nothing queued was lost: {tel:?}");
    e.apply_revision(&cancel(1, 500, 100, None), UnixNanos(later + 30 * SEC));
    assert_eq!(
        e.revision_telemetry().unknown_target,
        1,
        "past the horizon, so it is counted rather than applied to the wrong print"
    );
    assert_eq!(
        e.coverage_of("ABCD").map(|(_, why)| why),
        Some("identities_evicted"),
        "and the symbol says why it can no longer prove them"
    );
}

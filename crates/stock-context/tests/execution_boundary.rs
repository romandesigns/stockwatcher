//! Characterization of the admission boundary, and the counterexample that withdraws a claimed
//! sparse-window defect.
//!
//! **Nothing here changes behaviour.** Both cases came out of the audit and both are settled by
//! recording what the engine does today:
//!
//! - The exact-cutoff tie is a real inconsistency between the documented contract (a decision
//!   sees prints "admitted at or before `tau`") and the code (`Engine::apply` calls
//!   `advance_to(admit)` first, so a print admitted exactly at `tau` finalizes that barrier
//!   before it is queued, and joins the next one). Reconciling it needs an explicit clock
//!   watermark / arrival-total-order contract with its own tests, which is **deferred** to a
//!   separate execution contract — a blind call reorder would silently change which decisions
//!   admitted what. Until then these tests pin the current grouping, so the change cannot happen
//!   by accident and the contract has a before/after to compare against.
//!
//! - The sparse recent-window claim (independent audit F6: a cumulative checkpoint older than
//!   `b - w` inflates `recent_milli`) is **withdrawn as unproven**. With no accepted trade
//!   between the checkpoint and the window start, cumulative volume is constant over the gap, so
//!   the older checkpoint is exact. The counterexample below is the audit's own probe, kept as a
//!   property test rather than a fix.

use chrono::NaiveDate;
use market_events::session::et;
use market_events::{Price, Trade, UnixNanos};
use stock_context::*;

const VOL: Verdict = Verdict {
    ohlc: true,
    volume: true,
};
fn day() -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 10, 16).unwrap()
}
fn at(h: u32, m: u32, s: u32) -> i64 {
    et(day(), h, m, s, 0).unwrap().nanos()
}

fn engine() -> Engine {
    Engine::new(
        Policy::v1(),
        RunInfo {
            run_id: "fixture".into(),
            source: "replay".into(),
            fidelity: Fidelity::SyntheticFixture,
            numerator_coverage: Coverage::Unverified,
        },
    )
}

fn apply(e: &mut Engine, exec: i64, admit: i64, size: u64) -> Vec<Decision> {
    e.apply(
        &Trade::new(
            "TEST",
            UnixNanos(exec),
            Price::from_micros(6_000_000),
            size,
            1,
        ),
        VOL,
        UnixNanos(admit),
    )
}

/// Volume of the decision covering the bar ending at `bar_end`, if one was emitted.
fn shares_at(ds: &[Decision], bar_end: i64) -> Option<u64> {
    ds.iter()
        .find(|d| d.bar_end == bar_end)
        .map(|d| d.volume.shares)
}

/// The bar ending 09:30:15 is decided at `tau` = 09:30:17. A print admitted a nanosecond
/// before `tau` is in that decision; one admitted exactly at `tau`, or after, is not — it joins
/// the next bar's decision instead.
///
/// The documented contract says "at or before `tau`", so the `tau` case is the inconsistency the
/// deferred execution contract has to settle. Recorded here, not changed.
#[test]
fn a_print_admitted_exactly_at_the_cutoff_joins_the_next_bar_today() {
    let (b, tau) = (at(9, 30, 15), at(9, 30, 17));
    let observed = |admit: i64| {
        let mut e = engine();
        // An earlier print so the symbol is active and the barrier really decides.
        apply(&mut e, at(9, 30, 1), at(9, 30, 1) + 100_000_000, 100);
        let mut ds = apply(&mut e, at(9, 30, 14), admit, 200);
        ds.extend(e.advance_to(UnixNanos(at(9, 30, 32))));
        (shares_at(&ds, b), shares_at(&ds, at(9, 30, 30)))
    };
    assert_eq!(
        observed(tau - 1),
        (Some(300), None),
        "a nanosecond before the cutoff: in the decision, and nothing changes after it"
    );
    assert_eq!(
        observed(tau),
        (Some(100), Some(300)),
        "exactly at the cutoff: excluded today, counted one bar later"
    );
    assert_eq!(
        observed(tau + 1),
        (Some(100), Some(300)),
        "after the cutoff: late, as documented"
    );
}

/// The clock moving to `tau` on its own, before the print is applied, is the same case: the
/// barrier is already decided, so the print is late. This is the "clock first" tie.
#[test]
fn the_clock_reaching_the_cutoff_first_closes_the_barrier_today() {
    let (b, tau) = (at(9, 30, 15), at(9, 30, 17));
    let mut e = engine();
    apply(&mut e, at(9, 30, 1), at(9, 30, 1) + 100_000_000, 100);
    let mut ds = e.advance_to(UnixNanos(tau));
    assert_eq!(shares_at(&ds, b), Some(100));
    // The print executed inside the bar but reaches the engine after the clock did.
    ds.extend(apply(&mut e, at(9, 30, 14), tau, 200));
    ds.extend(e.advance_to(UnixNanos(at(9, 30, 32))));
    assert_eq!(
        shares_at(&ds, at(9, 30, 30)),
        Some(300),
        "it joins the next decision; the decided one is never rewritten"
    );
    let decided: Vec<_> = ds.iter().filter(|d| d.bar_end == b).collect();
    assert_eq!(decided.len(), 1, "one frozen decision per bar end");
}

/// Independent-audit F6's counterexample: a ten-minute gap with no accepted trade leaves the
/// cumulative checkpoint exact, so the recent window is not overstated.
///
/// Prior sessions average 10 shares in the five minutes to 09:40; today's five minutes to 09:40
/// hold the single 200-share print, so the ratio is 200/10 = 20.000x.
#[test]
fn a_sparse_cumulative_gap_alone_does_not_overstate_the_recent_window() {
    let mut e = engine();
    let mut sessions = Vec::new();
    let mut d = NaiveDate::from_ymd_opt(2026, 10, 15).unwrap();
    while sessions.len() < 10 {
        if market_events::is_trading_day(d) {
            // Seconds since 04:00: 20,100 = 09:35, 20,400 = 09:40.
            sessions.push(IntradayCurve {
                date: d,
                cum: vec![(0, 0), (20_100, 10), (20_400, 20)],
                complete: true,
            });
        }
        d = d.pred_opt().unwrap();
    }
    e.add_intraday(IntradayBaseline {
        id: "intraday".into(),
        version: 1,
        instrument: InstrumentId::symbol("TEST"),
        sessions,
        observed_at: at(4, 0, 0),
    });
    apply(&mut e, at(9, 30, 1), at(9, 30, 1) + 1, 100);
    e.advance_to(UnixNanos(at(9, 30, 17)));
    // Ten minutes with no print: no barrier is due, so no cumulative checkpoint is appended.
    apply(&mut e, at(9, 40, 1), at(9, 40, 1) + 1, 200);
    e.advance_to(UnixNanos(at(9, 40, 17)));
    let latest = e.current().next().expect("a decision");
    assert_eq!(
        latest.rvol.recent_milli,
        Some(20_000),
        "the checkpoint before the window start is exact over a gap with no trades"
    );
    assert_eq!(latest.rvol.recent_reason, "ok");
}

/// And the window is honest when it genuinely is not covered: before the engine has a cumulative
/// point at or before `b - w`, and with the day start inside the window, the recent window reads
/// today's whole cumulative volume — not a longer window scaled as if it were five minutes.
#[test]
fn the_recent_window_at_the_start_of_the_day_is_the_day_itself() {
    let mut e = engine();
    let w = Policy::v1().body.diagnostics.recent_window_secs;
    assert_eq!(w, 300, "the fixture assumes the v1 five-minute window");
    apply(&mut e, at(4, 0, 1), at(4, 0, 1) + 1, 100);
    e.advance_to(UnixNanos(at(4, 1, 2)));
    let d = e.current().next().expect("a decision");
    assert_eq!(d.volume.shares, 100);
    // No intraday baseline, so the diagnostic is honestly unavailable rather than invented.
    assert_eq!(d.rvol.recent_milli, None);
    assert_eq!(d.rvol.recent_reason, "no_intraday_baseline");
}

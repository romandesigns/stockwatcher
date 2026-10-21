//! The engine's two output journals are bounded, and say what they dropped.
//!
//! `panel_log` and `setup_log` are owned by the caller: the engine appends and the caller takes.
//! Nothing bounded them, and in production neither was taken reliably — `take_setup_log` had no
//! caller at all, and panels were only drained inside `take_update`, which the server skipped
//! whenever no client was subscribed. The audit probe
//! `repro_undrained_panel_log_retains_all_snapshots` retains 20 full snapshots over five
//! simulated minutes with no consumer; a market that is meant to run unattended has no bound on
//! that.
//!
//! The fix is a bound with counted drops, not silence: a caller that stops taking costs bounded
//! memory, and the telemetry stops claiming the journal it took is complete.

use chrono::NaiveDate;
use market_events::session::et;
use market_events::{Price, Trade, UnixNanos};
use stock_context::*;

const VOL: Verdict = Verdict {
    ohlc: true,
    volume: true,
};
const S: i64 = 1_000_000_000;

fn day() -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 10, 16).unwrap()
}
fn prev() -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 10, 15).unwrap()
}
fn t(h: u32, m: u32, s: u32) -> i64 {
    et(day(), h, m, s, 0).unwrap().nanos()
}
fn inst(sym: &str) -> InstrumentId {
    InstrumentId::symbol(sym)
}

fn engine() -> Engine {
    Engine::new(
        Policy::v1(),
        RunInfo {
            run_id: "fixture".into(),
            source: "replay".into(),
            fidelity: Fidelity::SyntheticFixture,
            numerator_coverage: Coverage::Complete,
        },
    )
}

/// Evidence enough for five passes on a $4 close with a 5M float.
fn evidence(e: &mut Engine, sym: &str) {
    e.add_prev_close(PrevClose {
        id: format!("close:{sym}"),
        version: 1,
        instrument: inst(sym),
        date: prev(),
        close_micros: 4_000_000,
        basis: CloseBasis::SplitAdjusted,
        actions_checked_through: Some(day()),
        actions: Default::default(),
        observed_at: t(4, 0, 0),
    });
    let mut sessions = Vec::new();
    let mut d = prev();
    while sessions.len() < 30 {
        if market_events::is_trading_day(d) {
            sessions.push(SessionVolume {
                date: d,
                volume: 1_000,
                complete: true,
                comparable: true,
            });
        }
        d = d.pred_opt().unwrap();
    }
    e.add_daily(DailyBaseline {
        id: format!("daily:{sym}"),
        version: 1,
        instrument: inst(sym),
        sessions,
        basis: Default::default(),
        observed_at: t(4, 0, 0),
    });
    e.add_float(FloatObservation {
        id: format!("float:{sym}"),
        version: 1,
        instrument: inst(sym),
        shares: 5_000_000,
        provider: "fixture".into(),
        effective_as_of: None,
        observed_at: t(4, 0, 0),
        quality: vec![],
    });
    e.add_news(NewsItem {
        id: format!("news:{sym}"),
        version: 1,
        instrument: inst(sym),
        event_type: "clinical_result".into(),
        source: "alpaca/benzinga".into(),
        credible_source: true,
        relevant: true,
        substantive: true,
        authenticity: Authenticity::ReportedUnverified,
        favorability: Favorability::Undetermined,
        published_at: Some(t(7, 0, 0)),
        observed_at: t(7, 0, 1),
        tag: Tag::Present,
        content_key: "fixture".into(),
            revision_at: Some(0),
        rules: "fixture".into(),
    });
}

/// End of the k-th 15 s bar after 09:30.
fn end(k: i64) -> i64 {
    t(9, 30, 0) + (k + 1) * 15 * S
}

fn print(e: &mut Engine, sym: &str, exec: i64, micros: i64, size: u64) {
    e.apply(
        &Trade::new(sym, UnixNanos(exec), Price::from_micros(micros), size, 1),
        VOL,
        UnixNanos(exec + 100_000_000),
    );
}

/// Prints making bar `k` exactly `[open, high, low, close]` (cents) with `vol` shares.
fn bar(e: &mut Engine, sym: &str, k: i64, ohlc: [i64; 4], vol: u64) {
    let e0 = end(k) - 15 * S;
    let [o, h, l, c] = ohlc.map(|x| x * 10_000);
    print(e, sym, e0 + S, o, vol / 4);
    print(e, sym, e0 + 3 * S, h, vol / 4);
    print(e, sym, e0 + 5 * S, l, vol / 4);
    print(e, sym, e0 + 13 * S, c, vol - 3 * (vol / 4));
}

/// One pole, flag, cross and trigger, then the hold that re-arms the family: 19 bars that walk
/// the setup through every state, so each cycle writes several transitions and level versions.
const CYCLE: [([i64; 4], u64); 19] = [
    ([500, 500, 500, 500], 400),
    ([500, 500, 500, 500], 400),
    ([500, 510, 500, 510], 2_000),
    ([510, 530, 510, 528], 2_000),
    ([525, 525, 518, 520], 500),
    ([520, 522, 517, 519], 500),
    ([519, 521, 518, 521], 400),
    ([521, 527, 520, 526], 300),
    ([526, 532, 526, 531], 2_000),
    ([531, 531, 531, 531], 400),
    ([531, 531, 531, 531], 400),
    ([531, 531, 531, 531], 400),
    ([531, 531, 531, 531], 400),
    ([531, 531, 531, 531], 400),
    ([531, 531, 531, 531], 400),
    ([531, 531, 531, 531], 400),
    ([531, 531, 531, 531], 400),
    ([500, 500, 500, 500], 400),
    ([500, 500, 500, 500], 400),
];

/// `cycles` runs of [`CYCLE`] for one member, decided through the end.
fn run_member(e: &mut Engine, sym: &str, cycles: i64) {
    evidence(e, sym);
    let mut k = 0i64;
    for _ in 0..cycles {
        for (ohlc, vol) in CYCLE {
            bar(e, sym, k, ohlc, vol);
            k += 1;
        }
    }
    e.advance_to(UnixNanos(end(k) + 3 * S));
}

#[test]
fn an_undrained_panel_log_is_bounded_and_counts_what_it_dropped() {
    let mut e = engine().with_output_bounds(4, 8);
    run_member(&mut e, "ABCD", 3);
    let tel = e.output_telemetry();
    assert!(
        tel.panels_published > 4,
        "the fixture must publish more panels than the bound, got {}",
        tel.panels_published
    );
    assert_eq!(tel.panels_held, 4, "held at the bound");
    assert!(tel.panels_dropped > 0, "the oldest were dropped");
    assert!(!tel.complete(), "a journal with drops is not complete");
    let taken = e.take_panels();
    assert_eq!(taken.len(), 4);
    let tel = e.output_telemetry();
    assert_eq!(tel.panels_taken, 4);
    assert_eq!(
        tel.panels_published,
        tel.panels_taken + tel.panels_dropped + tel.panels_held as u64,
        "every published snapshot is accounted for"
    );
    // The latest snapshot is always available, whatever was dropped: a client snapshot is never
    // a stale one because nobody drained.
    let last = e.panel().expect("a latest panel");
    assert_eq!(last.revision, taken.last().expect("rows").revision);
}

#[test]
fn an_undrained_setup_log_is_bounded_and_counts_what_it_dropped() {
    let mut e = engine().with_output_bounds(1024, 8);
    run_member(&mut e, "ABCD", 3);
    let tel = e.output_telemetry();
    assert!(
        tel.setup_entries_logged > 8,
        "the fixture must log more setup entries than the bound, got {}",
        tel.setup_entries_logged
    );
    assert_eq!(tel.setup_entries_held, 8);
    assert!(tel.setup_entries_dropped > 0);
    let taken = e.take_setup_log();
    assert_eq!(taken.len(), 8);
    let tel = e.output_telemetry();
    assert_eq!(
        tel.setup_entries_logged,
        tel.setup_entries_taken + tel.setup_entries_dropped + tel.setup_entries_held as u64
    );
}

#[test]
fn draining_every_barrier_loses_nothing() {
    let mut e = engine().with_output_bounds(4, 8);
    evidence(&mut e, "ABCD");
    let mut panels = 0usize;
    let mut setups = 0usize;
    let mut k = 0i64;
    for _ in 0..3 {
        for (ohlc, vol) in CYCLE {
            bar(&mut e, "ABCD", k, ohlc, vol);
            k += 1;
            // A caller that keeps up: drain at every barrier, viewer or no viewer.
            panels += e.take_panels().len();
            setups += e.take_setup_log().len();
        }
    }
    e.advance_to(UnixNanos(end(k) + 3 * S));
    panels += e.take_panels().len();
    setups += e.take_setup_log().len();
    let tel = e.output_telemetry();
    assert!(tel.complete(), "nothing dropped when the caller keeps up");
    assert_eq!(tel.panels_dropped, 0);
    assert_eq!(tel.setup_entries_dropped, 0);
    assert!(panels > 4 && setups > 8, "the bounds were really exceeded");
    assert_eq!(panels as u64, tel.panels_published);
    assert_eq!(setups as u64, tel.setup_entries_logged);
}

#[test]
fn the_default_bounds_are_the_published_constants() {
    let e = engine();
    let tel = e.output_telemetry();
    assert_eq!(tel.panel_log_max, PANEL_LOG_MAX);
    assert_eq!(tel.setup_log_max, SETUP_LOG_MAX);
    assert!(tel.complete(), "a fresh engine has dropped nothing");
}

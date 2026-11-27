//! The clock budget ([`ClockBudget`]): a market-wide re-evaluation (one news poll's coverage
//! record marks every symbol dirty) is spread over several clock calls, each bounded, and decides
//! exactly what the unbounded engine decides, at the same barrier. A symbol with a print of its
//! own near the band is never deferred. Synthetic, fictional symbols.

use market_events::session::et;
use market_events::{Price, Trade, UnixNanos};
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

fn engine(budget: Option<ClockBudget>) -> Engine {
    let e = Engine::new(
        Policy::current(),
        RunInfo {
            run_id: "fixture".into(),
            source: "live".into(),
            fidelity: Fidelity::RecordedLive,
            numerator_coverage: Coverage::Unverified,
        },
    );
    match budget {
        Some(b) => e.with_clock_budget(b),
        None => e,
    }
}

fn print(e: &mut Engine, sym: &str, seq: u64, exec: i64, cents: i64) -> Vec<Decision> {
    let t = Trade::new(sym, UnixNanos(exec), Price::from_cents(cents), 100, seq);
    e.apply(&t, VOL, UnixNanos(exec + 100_000_000))
}

fn coverage(e: &mut Engine, n: u32, at: i64) {
    e.add_coverage(NewsCoverage {
        id: format!("cov:{n}"),
        instrument: InstrumentId::market(),
        sources: vec!["fixture".into()],
        covered_from: at - 3 * 86_400 * SEC,
        covered_through: at,
        observed_at: at,
    });
}

/// Without the run-local sequence number, which counts in evaluation order.
fn key(mut d: Decision) -> Decision {
    d.seq = 0;
    d
}

const N: usize = 1_000;

/// `N` symbols that each printed once at 10:00, decided through the 10:01:00 barrier.
fn market(e: &mut Engine) {
    for i in 0..N {
        print(
            e,
            &format!("S{i:04}"),
            1,
            t(10, 0, 0) + i as i64 * 1_000,
            500,
        );
    }
    e.advance_to(UnixNanos(t(10, 1, 3)));
}

#[test]
fn a_market_wide_record_is_decided_over_several_bounded_calls_at_its_own_barrier() {
    let budget = ClockBudget {
        symbols: 100,
        time: None,
    };
    let (mut free, mut bounded) = (engine(None), engine(Some(budget)));
    market(&mut free);
    market(&mut bounded);
    assert_eq!(
        bounded.clock_telemetry().backlog,
        0,
        "{:?}",
        bounded.clock_telemetry()
    );

    // One news poll: every symbol is dirty, and the next barrier (10:01:15, decided once it
    // settles at 10:01:17) re-evaluates all of them.
    coverage(&mut free, 1, t(10, 1, 5));
    coverage(&mut bounded, 1, t(10, 1, 5));
    let at = t(10, 1, 17);
    let all: Vec<Decision> = free.advance_to(UnixNanos(at));
    assert_eq!(
        all.len(),
        N,
        "the unbounded engine decides the whole market in one call"
    );

    let mut got = bounded.advance_to(UnixNanos(at));
    assert_eq!(
        got.len(),
        budget.symbols,
        "the barrier's own call takes one budget"
    );
    assert_eq!(bounded.clock_telemetry().backlog, N - budget.symbols);
    // The provider's idle tick, once a wall second, before the next barrier (10:01:32).
    let mut calls = 1;
    for s in 1..14 {
        let more = bounded.advance_to(UnixNanos(at + s * SEC));
        assert!(more.len() <= budget.symbols, "{} in one call", more.len());
        calls += usize::from(!more.is_empty());
        got.extend(more);
    }
    assert_eq!(calls, N / budget.symbols);
    let tel = bounded.clock_telemetry();
    assert_eq!((tel.backlog, tel.carried_over), (0, 0), "{tel:?}");
    assert_eq!(tel.deferred, (N - budget.symbols) as u64);

    // The same decisions, at the same barrier and cutoff, citing the same coverage record.
    let mut a: Vec<Decision> = all.into_iter().map(key).collect();
    let mut b: Vec<Decision> = got.into_iter().map(key).collect();
    a.sort_by(|x, y| x.symbol.cmp(&y.symbol));
    b.sort_by(|x, y| x.symbol.cmp(&y.symbol));
    assert!(b.iter().all(|d| d.bar_end == t(10, 1, 15)));
    assert_eq!(a, b);
    let current = |e: &Engine| e.current().cloned().map(key).collect::<Vec<_>>();
    assert_eq!(current(&free), current(&bounded));
}

#[test]
fn a_print_near_the_band_is_decided_at_its_barrier_whatever_the_backlog() {
    let budget = ClockBudget {
        symbols: 10,
        time: None,
    };
    let mut e = engine(Some(budget));
    market(&mut e);
    coverage(&mut e, 1, t(10, 1, 5));
    // A fresh print in the band, and one far above it ($400).
    print(&mut e, "S0999", 2, t(10, 1, 5), 510);
    print(&mut e, "S0998", 2, t(10, 1, 5), 40_000);
    let out = e.advance_to(UnixNanos(t(10, 1, 17)));
    let named = |s: &str| out.iter().any(|d| d.symbol == s);
    assert!(named("S0999"), "its own print is never deferred");
    assert_eq!(
        out.len(),
        budget.symbols + 1,
        "one own decision plus one budget of bulk"
    );
    // The far one waits its turn (calls ten times a second here, all before the next barrier)
    // and is still decided at 10:01:15, with its print folded.
    let mut rest = Vec::new();
    for s in 1..140 {
        rest.extend(e.advance_to(UnixNanos(t(10, 1, 17) + s * SEC / 10)));
    }
    assert_eq!(e.clock_telemetry().backlog, 0);
    let far = rest
        .iter()
        .chain(&out)
        .find(|d| d.symbol == "S0998")
        .expect("decided");
    assert_eq!(far.bar_end, t(10, 1, 15));
    assert_eq!(far.volume.shares, 200);
}

#[test]
fn a_revision_settles_a_deferred_symbol_first() {
    let budget = ClockBudget {
        symbols: 10,
        time: None,
    };
    let mut e = engine(Some(budget));
    market(&mut e);
    coverage(&mut e, 1, t(10, 1, 5));
    e.advance_to(UnixNanos(t(10, 1, 17)));
    let waiting = e.clock_telemetry().backlog;
    assert_eq!(waiting, N - budget.symbols);
    // A coverage fact about the last symbol changes now: its 10:01:15 decision is made first,
    // on what the barrier would have read, and the next call returns it.
    e.set_coverage("S0999", Some((Coverage::Partial, "recorder_drops")));
    assert_eq!(e.clock_telemetry().backlog, waiting - 1);
    let next = e.advance_to(UnixNanos(t(10, 1, 18)));
    let d = next.iter().find(|d| d.symbol == "S0999").expect("settled");
    assert_eq!(d.bar_end, t(10, 1, 15));
    assert_ne!(
        d.volume.coverage,
        Coverage::Partial,
        "read before the change"
    );
}

#[test]
fn a_backlog_the_calls_cannot_finish_is_carried_into_the_next_barrier() {
    let budget = ClockBudget {
        symbols: 100,
        time: None,
    };
    let mut e = engine(Some(budget));
    market(&mut e);
    coverage(&mut e, 1, t(10, 1, 5));
    // One call per bar: each barrier's call takes one budget, and the rest stays due.
    let mut decided = std::collections::BTreeSet::new();
    let mut b = t(10, 1, 17);
    while decided.len() < N {
        for d in e.advance_to(UnixNanos(b)) {
            decided.insert(d.symbol);
        }
        b += 15 * SEC;
        assert!(
            b < t(10, 5, 0),
            "every symbol is reached: {}",
            decided.len()
        );
    }
    let tel = e.clock_telemetry();
    assert!(tel.carried_over > 0, "{tel:?}");
}

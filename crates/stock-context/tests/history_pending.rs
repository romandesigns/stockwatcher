//! The temporary history-pending guard (`Engine::set_history_pending`): while a symbol's day
//! history is still being restored, nothing history-dependent is claimed from the live tail; a
//! successful facts adoption ends it without clearing genuine doubts; a refused one does not.
//! Synthetic fixtures, fictional symbols.

use market_events::session::et;
use market_events::{Price, Trade, TradeId, UnixNanos};
use stock_context::detect::fixture::{self, FixtureStep, Script, cutoff, end};
use stock_context::*;

const S: i64 = 1_000_000_000;
const BOTH: Verdict = Verdict {
    ohlc: true,
    volume: true,
};

fn t(h: u32, m: u32, s: u32) -> i64 {
    et(fixture::day(), h, m, s, 0).unwrap().nanos()
}

fn run(run_id: &str) -> RunInfo {
    RunInfo {
        run_id: run_id.into(),
        source: "live".into(),
        fidelity: Fidelity::RecordedLive,
        numerator_coverage: Coverage::Complete,
    }
}

fn engine(run_id: &str) -> Engine {
    Engine::new(Policy::current(), run(run_id)).with_detection_policy(DetectionPolicy::current())
}

/// Evidence on which every pillar passes once 500 shares have traded at $3.50.
fn evidence(e: &mut Engine, sym: &str) {
    let prev = fixture::day().pred_opt().unwrap();
    let inst = InstrumentId::symbol(sym);
    e.add_prev_close(PrevClose {
        id: format!("close:{sym}"),
        version: 1,
        instrument: inst.clone(),
        date: prev,
        close_micros: 3_000_000,
        basis: CloseBasis::SplitAdjusted,
        actions_checked_through: Some(fixture::day()),
        actions: Default::default(),
        observed_at: t(4, 0, 0),
    });
    let mut sessions = Vec::new();
    let mut d = prev;
    while sessions.len() < 30 {
        if market_events::is_trading_day(d) {
            sessions.push(SessionVolume {
                date: d,
                volume: 100,
                complete: true,
                comparable: true,
            });
        }
        d = d.pred_opt().unwrap();
    }
    e.add_daily(DailyBaseline {
        id: format!("daily:{sym}"),
        version: 1,
        instrument: inst.clone(),
        sessions,
        basis: Default::default(),
        observed_at: t(4, 0, 0),
    });
    e.add_float(FloatObservation {
        id: format!("float:{sym}"),
        version: 1,
        instrument: inst.clone(),
        shares: 5_000_000,
        provider: "fixture".into(),
        effective_as_of: None,
        observed_at: t(4, 0, 0),
        quality: vec![],
    });
    e.add_news(NewsItem {
        id: format!("news:{sym}"),
        version: 1,
        instrument: inst,
        event_type: "other".into(),
        source: "issuer".into(),
        credible_source: true,
        relevant: true,
        substantive: true,
        authenticity: Authenticity::IssuerAnnounced,
        favorability: Favorability::Favorable,
        published_at: Some(t(7, 0, 0)),
        observed_at: t(7, 0, 1),
        tag: Tag::Present,
        content_key: "fixture".into(),
        revision_at: Some(0),
        rules: "fixture".into(),
    });
}

fn print(e: &mut Engine, sym: &str, exec: i64, size: u64, seq: u64) {
    e.apply(
        &Trade::new(
            sym,
            UnixNanos(exec),
            Price::from_micros(3_500_000),
            size,
            seq,
        ),
        BOTH,
        UnixNanos(exec + 100_000_000),
    );
}

fn rvol(e: &Engine, sym: &str) -> (Tri, &'static str, Qualification, bool) {
    let d = e.current().find(|d| d.symbol == sym).expect("a decision");
    let p = d
        .pillars
        .iter()
        .find(|p| p.pillar == PillarId::Rvol)
        .unwrap();
    (
        p.state,
        p.reason,
        d.qualification,
        d.volume.integrity.history_pending,
    )
}

/// The live tail: 1,000 shares at 10:00, enough on its own to pass a 500-share RVOL.
fn live_tail(e: &mut Engine, sym: &str) {
    for i in 0..10 {
        print(e, sym, t(10, 0, i), 100, 1000 + i as u64);
    }
    e.advance_to(UnixNanos(t(10, 0, 30)));
}

/// The restore: the morning, 2,000 shares, replayed on an engine of the same run and policies.
fn restore(run_id: &str, sym: &str) -> Engine {
    let mut r = engine(run_id);
    evidence(&mut r, sym);
    for i in 0..20 {
        print(&mut r, sym, t(9, 30, 0) + i * 60 * S, 100, 1 + i as u64);
    }
    r.advance_to(UnixNanos(t(9, 59, 30)));
    r
}

#[test]
fn a_pending_tail_claims_nothing_and_adoption_ends_the_guard() {
    let mut live = engine("live");
    evidence(&mut live, "HPND");
    // Before the symbol's first print.
    live.set_history_pending("HPND", true);
    assert!(live.history_pending("HPND"));
    live_tail(&mut live, "HPND");
    let (state, reason, q, flag) = rvol(&live, "HPND");
    assert_eq!((state, reason), (Tri::Unknown, "history_pending"));
    assert_ne!(q, Qualification::Qualified, "no 5/5 from a tail");
    assert!(flag);
    let snap = live.detection_snapshot().unwrap();
    assert!(snap.active.is_empty(), "no fake episodes");
    assert_eq!(snap.monitored.symbols_history_pending, 1);
    assert_eq!(
        snap.data_status.history_pending_symbols,
        vec!["HPND".to_string()]
    );

    let mut r = restore("live", "HPND");
    assert_eq!(
        live.adopt_facts(&mut r, "HPND"),
        Adoption::Adopted { published: true }
    );
    assert!(!live.history_pending("HPND"));
    // The next barrier decides on the day's facts.
    print(&mut live, "HPND", t(10, 1, 0), 100, 2000);
    live.advance_to(UnixNanos(t(10, 1, 30)));
    let (state, _, q, flag) = rvol(&live, "HPND");
    assert_eq!(state, Tri::Pass);
    assert_eq!(q, Qualification::Qualified);
    assert!(!flag);
    assert_eq!(
        live.detection_snapshot()
            .unwrap()
            .monitored
            .symbols_history_pending,
        0
    );
}

#[test]
fn pending_wins_over_a_missing_baseline_and_both_diagnostics_say_so() {
    // No evidence at all: no daily or intraday baseline. Without the guard the pillar would say
    // no_daily_baseline, which hides why nothing is assessed.
    let mut live = engine("live");
    live.set_history_pending("HNOB", true);
    live_tail(&mut live, "HNOB");
    let d = live.current().find(|d| d.symbol == "HNOB").unwrap();
    let p = d
        .pillars
        .iter()
        .find(|p| p.pillar == PillarId::Rvol)
        .unwrap();
    assert_eq!(
        (p.state, p.reason, p.value),
        (Tri::Unknown, "history_pending", None)
    );
    assert_eq!(d.rvol.same_time_reason, "history_pending");
    assert_eq!(d.rvol.recent_reason, "history_pending");
    // Resolved explicitly: the missing baseline is reported for what it is.
    live.set_history_pending("HNOB", false);
    print(&mut live, "HNOB", t(10, 1, 0), 100, 2000);
    live.advance_to(UnixNanos(t(10, 1, 30)));
    let d = live.current().find(|d| d.symbol == "HNOB").unwrap();
    let p = d
        .pillars
        .iter()
        .find(|p| p.pillar == PillarId::Rvol)
        .unwrap();
    assert_eq!((p.state, p.reason), (Tri::Unknown, "no_daily_baseline"));
    assert_eq!(d.rvol.same_time_reason, "no_intraday_baseline");
}

#[test]
fn adoption_ends_the_guard_but_never_a_genuine_doubt() {
    let mut live = engine("live");
    evidence(&mut live, "HDBT");
    live.set_history_pending("HDBT", true);
    live_tail(&mut live, "HDBT");
    // A cancel naming a print this engine never admitted: the count may hold volume that should
    // not be there, a genuine and lasting doubt. Its size is larger than the whole day, so the
    // count net of it proves nothing and the pillar stays UNKNOWN (a small one would pass on the
    // net lower bound: `tests/revisions.rs`).
    live.apply_revision(
        &PrintRevision {
            symbol: "HDBT".into(),
            target: RevisionTarget {
                id: TradeId {
                    source: market_events::Source::Fixture,
                    exchange: String::new(),
                    seq: 999_999,
                },
                price: 3_500_000,
                size: 1_000_000,
            },
            corrected: None,
            last: None,
            day_high: None,
        },
        UnixNanos(t(10, 0, 40)),
    );
    let mut r = restore("live", "HDBT");
    assert!(matches!(
        live.adopt_facts(&mut r, "HDBT"),
        Adoption::Adopted { .. }
    ));
    assert!(!live.history_pending("HDBT"));
    print(&mut live, "HDBT", t(10, 1, 0), 100, 2000);
    live.advance_to(UnixNanos(t(10, 1, 30)));
    let (state, reason, _, flag) = rvol(&live, "HDBT");
    assert_eq!((state, reason), (Tri::Unknown, "volume_untrusted"));
    assert!(!flag);
}

#[test]
fn a_refused_restore_leaves_the_symbol_pending() {
    let mut live = engine("live");
    evidence(&mut live, "HREF");
    live.set_history_pending("HREF", true);
    live_tail(&mut live, "HREF");
    let mut other = restore("another-run", "HREF");
    assert_eq!(
        live.adopt_facts(&mut other, "HREF"),
        Adoption::Refused("run_identity")
    );
    assert!(live.history_pending("HREF"));
    let (state, reason, _, _) = rvol(&live, "HREF");
    assert_eq!((state, reason), (Tri::Unknown, "history_pending"));
}

#[test]
fn pending_detection_keeps_its_baseline_and_detects_once_resolved() {
    let steps = fixture::script("ignition_no_precursor").unwrap();
    let split = steps
        .iter()
        .position(|s| matches!(s, FixtureStep::Print { exec, .. } if *exec >= end(40) - 15 * S))
        .unwrap();
    // Pending throughout: nothing at all.
    let mut e = fixture::engine("t");
    e.set_history_pending("IGNB", true);
    assert!(fixture::drive(&mut e, &steps).is_empty());
    // Pending until just before the ignition bar, resolved explicitly (no adoption): the return
    // baseline kept growing, so the ignition is recognized at once with all 39 returns.
    let mut e = fixture::engine("t");
    e.set_history_pending("IGNB", true);
    fixture::drive(&mut e, &steps[..split]);
    e.set_history_pending("IGNB", false);
    let ts = fixture::drive(&mut e, &steps[split..]);
    let ig = ts
        .iter()
        .find(|t| t.family == Family::Ignition)
        .expect("ignition");
    assert_eq!(ig.observed_at, cutoff(40));
    assert_eq!(ig.features.as_ref().unwrap().baseline.returns_n, 39);
}

#[test]
fn no_trade_driven_crossing_while_pending_between_barriers() {
    let mut s = Script::new();
    for k in 0..6 {
        s.bar("HCRS", k, [300, 300, 300, 300], 100);
    }
    s.bar("HCRS", 6, [300, 302, 300, 302], 200);
    s.bar("HCRS", 7, [302, 310, 302, 310], 300);
    s.bar("HCRS", 8, [310, 325, 310, 325], 300);
    s.bar("HCRS", 9, [325, 340, 325, 338], 400);
    s.bar("HCRS", 10, [338, 338, 333, 335], 150);
    s.bar("HCRS", 11, [335, 336, 332, 334], 120);
    s.advance(cutoff(11));
    let mut e = fixture::engine("t");
    let ts = fixture::drive(&mut e, &s.steps);
    assert_eq!(ts.last().unwrap().to, State::Armed);
    e.set_history_pending("HCRS", true);
    let mut after = Script::new();
    after.print("HCRS", end(11) + 4 * S, 341, 100);
    let ts = fixture::drive(&mut e, &after.steps);
    assert!(
        ts.is_empty(),
        "no crossing between barriers while pending: {ts:?}"
    );
    let ts = fixture::drive(&mut e, &[FixtureStep::Advance(cutoff(12))]);
    let last = ts.last().unwrap();
    assert_eq!(
        (last.to, last.reason),
        (State::Interrupted, "history_pending")
    );
}

//! Acceptance cases from the five-pillars brief (§12) that the context engine itself can answer.
//! Every fixture is synthetic: one trading day, 2026-10-16, previous close 2026-10-15.

use chrono::NaiveDate;
use market_events::session::et;
use market_events::{Price, Trade, UnixNanos};
use stock_context::*;

const VOL: Verdict = Verdict {
    ohlc: true,
    volume: true,
};
const H: i64 = 3_600 * 1_000_000_000;

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

fn engine(coverage: Coverage) -> Engine {
    Engine::new(
        Policy::v1(),
        RunInfo {
            run_id: "fixture".into(),
            source: "replay".into(),
            fidelity: Fidelity::SyntheticFixture,
            numerator_coverage: coverage,
        },
    )
}

/// A print executed at `exec`, admitted 100 ms later (or at `admit`).
fn print(e: &mut Engine, sym: &str, exec: i64, micros: i64, size: u64) -> Vec<Decision> {
    print_at(e, sym, exec, exec + 100_000_000, micros, size)
}
fn print_at(
    e: &mut Engine,
    sym: &str,
    exec: i64,
    admit: i64,
    micros: i64,
    size: u64,
) -> Vec<Decision> {
    e.apply(
        &Trade::new(sym, UnixNanos(exec), Price::from_micros(micros), size, 1),
        VOL,
        UnixNanos(admit),
    )
}

fn close(
    sym: &str,
    micros: i64,
    basis: CloseBasis,
    checked: Option<NaiveDate>,
    observed: i64,
) -> PrevClose {
    PrevClose {
        id: format!("close:{sym}"),
        version: 1,
        instrument: inst(sym),
        date: prev(),
        close_micros: micros,
        basis,
        actions_checked_through: checked,
        actions: Default::default(),
        observed_at: observed,
    }
}

fn daily(sym: &str, per_session: u64, n: usize, observed: i64) -> DailyBaseline {
    let mut sessions = Vec::new();
    let mut d = prev();
    while sessions.len() < n {
        if market_events::is_trading_day(d) {
            sessions.push(SessionVolume {
                date: d,
                volume: per_session,
                complete: true,
                comparable: true,
            });
        }
        d = d.pred_opt().unwrap();
    }
    DailyBaseline {
        id: format!("daily:{sym}"),
        version: 1,
        instrument: inst(sym),
        sessions,
        basis: Default::default(),
        observed_at: observed,
    }
}

fn float(sym: &str, shares: u64, observed: i64) -> FloatObservation {
    FloatObservation {
        id: format!("float:{sym}"),
        version: 1,
        instrument: inst(sym),
        shares,
        provider: "fixture".into(),
        effective_as_of: None,
        observed_at: observed,
        quality: vec![],
    }
}

fn news(
    sym: &str,
    version: u32,
    relevant: bool,
    fav: Favorability,
    published: i64,
    observed: i64,
) -> NewsItem {
    NewsItem {
        id: format!("news:{sym}:1"),
        version,
        instrument: inst(sym),
        event_type: "clinical_result".into(),
        source: "issuer press release".into(),
        credible_source: true,
        relevant,
        substantive: true,
        authenticity: Authenticity::IssuerAnnounced,
        favorability: fav,
        published_at: Some(published),
        observed_at: observed,
        tag: Tag::Present,
        content_key: "fixture".into(),
        revision_at: Some(0),
        rules: "fixture".into(),
    }
}

fn pillar(d: &Decision, p: PillarId) -> &PillarResult {
    d.pillars.iter().find(|x| x.pillar == p).unwrap()
}

fn last_for<'a>(ds: &'a [Decision], sym: &str) -> &'a Decision {
    ds.iter()
        .rev()
        .find(|d| d.symbol == sym)
        .expect("a decision")
}

/// Everything that makes ABCD pass, observed before the open.
fn all_pass_evidence(e: &mut Engine, sym: &str) {
    e.add_prev_close(close(
        sym,
        5_000_000,
        CloseBasis::SplitAdjusted,
        Some(day()),
        t(4, 0, 0),
    ));
    e.add_daily(daily(sym, 100_000, 30, t(4, 0, 0)));
    e.add_float(float(sym, 8_000_000, t(4, 0, 0)));
    e.add_news(news(
        sym,
        1,
        true,
        Favorability::Favorable,
        t(7, 0, 0),
        t(7, 0, 1),
    ));
}

#[test]
fn every_pillar_passes_on_complete_evidence_and_first_times_are_cutoffs() {
    let mut e = engine(Coverage::Complete);
    all_pass_evidence(&mut e, "ABCD");
    let mut ds = print(&mut e, "ABCD", t(9, 30, 1), 5_500_000, 500_000);
    ds.extend(e.advance_to(UnixNanos(t(9, 31, 0))));
    let d = last_for(&ds, "ABCD");
    assert_eq!(
        d.qualification,
        Qualification::Qualified,
        "{:#?}",
        d.pillars
    );
    assert_eq!(
        d.counts,
        Counts {
            pass: 5,
            fail: 0,
            unknown: 0
        }
    );
    // Known at the barrier (bar end + 2 s), never at the bar end.
    assert_eq!(d.bar_end, t(9, 30, 15));
    assert_eq!(d.timeline.first_qualified, Some(t(9, 30, 17)));
    assert!(d.policy.starts_with("pillars-v1@"));
}

#[test]
fn exact_boundaries_for_gain_rvol_and_float() {
    // +10 % exactly passes; one micro-dollar less fails although both display as 10.00 %.
    for (price, want) in [(5_500_000, Tri::Pass), (5_499_999, Tri::Fail)] {
        let mut e = engine(Coverage::Complete);
        all_pass_evidence(&mut e, "ABCD");
        let mut ds = print(&mut e, "ABCD", t(9, 30, 1), price, 500_000);
        ds.extend(e.advance_to(UnixNanos(t(9, 31, 0))));
        assert_eq!(
            pillar(last_for(&ds, "ABCD"), PillarId::Gain).state,
            want,
            "price {price}"
        );
    }
    // RVOL 5.0x exactly passes, a share less fails; float 19,999,999 passes, 20,000,000 fails.
    for (shares, fl, rv, ff) in [
        (500_000, 19_999_999, Tri::Pass, Tri::Pass),
        (499_999, 20_000_000, Tri::Fail, Tri::Fail),
    ] {
        let mut e = engine(Coverage::Complete);
        all_pass_evidence(&mut e, "ABCD");
        e.add_float(float("ABCD", fl, t(5, 0, 0)));
        let mut ds = print(&mut e, "ABCD", t(9, 30, 1), 5_500_000, shares);
        ds.extend(e.advance_to(UnixNanos(t(9, 31, 0))));
        let d = last_for(&ds, "ABCD");
        assert_eq!(pillar(d, PillarId::Rvol).state, rv);
        assert_eq!(pillar(d, PillarId::Float).state, ff);
    }
}

#[test]
fn unverified_numerator_can_pass_but_never_fail_rvol() {
    let mut e = engine(Coverage::Unverified);
    all_pass_evidence(&mut e, "ABCD");
    let mut ds = print(&mut e, "ABCD", t(9, 30, 1), 5_500_000, 400_000);
    ds.extend(e.advance_to(UnixNanos(t(9, 31, 0))));
    let r = pillar(last_for(&ds, "ABCD"), PillarId::Rvol);
    assert_eq!(
        (r.state, r.reason),
        (Tri::Unknown, "numerator_coverage_unverified")
    );
    assert_eq!(r.value, Some(4000));
    let mut ds = print(&mut e, "ABCD", t(9, 31, 1), 5_500_000, 100_000);
    ds.extend(e.advance_to(UnixNanos(t(9, 32, 0))));
    assert_eq!(
        pillar(last_for(&ds, "ABCD"), PillarId::Rvol).state,
        Tri::Pass
    );
}

#[test]
fn daily_and_same_time_rvol_are_different_numbers() {
    // 1,000,000 shares by 08:00; a normal day has 100,000 by 08:00 and 2,000,000 in all.
    let mut e = engine(Coverage::Complete);
    e.add_daily(daily("ABCD", 2_000_000, 30, t(4, 0, 0)));
    let mut sessions = Vec::new();
    let mut d = prev();
    while sessions.len() < 10 {
        if market_events::is_trading_day(d) {
            sessions.push(IntradayCurve {
                date: d,
                cum: vec![(0, 0), (4 * 3600, 100_000), (16 * 3600, 2_000_000)],
                complete: true,
            });
        }
        d = d.pred_opt().unwrap();
    }
    e.add_intraday(IntradayBaseline {
        id: "intraday:ABCD".into(),
        version: 1,
        instrument: inst("ABCD"),
        sessions,
        basis: Default::default(),
        observed_at: t(4, 0, 0),
    });
    let mut ds = print(&mut e, "ABCD", t(7, 59, 50), 5_500_000, 1_000_000);
    ds.extend(e.advance_to(UnixNanos(t(8, 0, 3))));
    let d = last_for(&ds, "ABCD");
    assert_eq!(d.bar_end, t(8, 0, 0));
    assert_eq!(
        pillar(d, PillarId::Rvol).value,
        Some(500),
        "daily denominator: 0.5x"
    );
    assert_eq!(pillar(d, PillarId::Rvol).state, Tri::Fail);
    assert_eq!(d.rvol.same_time_milli, Some(10_000), "same time: 10x");
}

#[test]
fn missing_coverage_zero_denominator_and_short_history_are_unknown() {
    // 29 covered sessions: the strict 30-session denominator is unavailable, with the count.
    let mut e = engine(Coverage::Complete);
    e.add_daily(daily("ABCD", 100_000, 29, t(4, 0, 0)));
    let mut ds = print(&mut e, "ABCD", t(9, 30, 1), 5_500_000, 600_000);
    ds.extend(e.advance_to(UnixNanos(t(9, 31, 0))));
    let d = last_for(&ds, "ABCD");
    assert_eq!(pillar(d, PillarId::Rvol).reason, "insufficient_baseline");
    assert_eq!(d.rvol.daily_sessions, 29);

    // A session lost to collection is not a zero: marking one incomplete leaves 29.
    let mut e = engine(Coverage::Complete);
    let mut b = daily("ABCD", 100_000, 30, t(4, 0, 0));
    b.sessions[3].complete = false;
    e.add_daily(b);
    let mut ds = print(&mut e, "ABCD", t(9, 30, 1), 5_500_000, 600_000);
    ds.extend(e.advance_to(UnixNanos(t(9, 31, 0))));
    assert_eq!(
        pillar(last_for(&ds, "ABCD"), PillarId::Rvol).reason,
        "insufficient_baseline"
    );

    // Thirty covered zeros: a valid zero baseline gives an unavailable ratio, not infinity.
    let mut e = engine(Coverage::Complete);
    e.add_daily(daily("ABCD", 0, 30, t(4, 0, 0)));
    let mut ds = print(&mut e, "ABCD", t(9, 30, 1), 5_500_000, 600_000);
    ds.extend(e.advance_to(UnixNanos(t(9, 31, 0))));
    let r = pillar(last_for(&ds, "ABCD"), PillarId::Rvol);
    assert_eq!(
        (r.state, r.reason, r.value),
        (Tri::Unknown, "zero_denominator", None)
    );

    // Shortened sessions are not comparable for the full-day denominator.
    let mut e = engine(Coverage::Complete);
    let mut b = daily("ABCD", 100_000, 30, t(4, 0, 0));
    b.sessions[0].comparable = false;
    e.add_daily(b);
    let mut ds = print(&mut e, "ABCD", t(9, 30, 1), 5_500_000, 600_000);
    ds.extend(e.advance_to(UnixNanos(t(9, 31, 0))));
    assert_eq!(last_for(&ds, "ABCD").rvol.daily_sessions, 29);
}

#[test]
fn reverse_split_never_fabricates_a_gain() {
    // Yesterday $0.50 as printed; a 1:10 reverse split today puts the stock at $5.00.
    let mut e = engine(Coverage::Complete);
    e.add_prev_close(close("ABCD", 500_000, CloseBasis::Raw, None, t(4, 0, 0)));
    let mut ds = print(&mut e, "ABCD", t(9, 30, 1), 5_000_000, 1_000);
    ds.extend(e.advance_to(UnixNanos(t(9, 31, 0))));
    let d = last_for(&ds, "ABCD");
    let g = pillar(d, PillarId::Gain);
    assert_eq!(
        (g.state, g.reason),
        (Tri::Unknown, "adjustment_coverage_unknown")
    );
    // The raw comparison stays visible under its own name, never as the pillar.
    assert_eq!(d.gain_vs_stored_close_bp, Some(90_000));
    // With the split applied and checked through today: no gain at all.
    let mut e = engine(Coverage::Complete);
    e.add_prev_close(close(
        "ABCD",
        5_000_000,
        CloseBasis::SplitAdjusted,
        Some(day()),
        t(4, 0, 0),
    ));
    let mut ds = print(&mut e, "ABCD", t(9, 30, 1), 5_000_000, 1_000);
    ds.extend(e.advance_to(UnixNanos(t(9, 31, 0))));
    let g = pillar(last_for(&ds, "ABCD"), PillarId::Gain).clone();
    assert_eq!((g.state, g.value), (Tri::Fail, Some(0)));
}

#[test]
fn float_retrieved_later_cannot_reach_back_and_goes_stale() {
    let mut e = engine(Coverage::Complete);
    e.add_float(float("ABCD", 8_000_000, t(10, 0, 0)));
    let mut ds = print(&mut e, "ABCD", t(9, 30, 1), 5_500_000, 1_000);
    ds.extend(e.advance_to(UnixNanos(t(10, 1, 0))));
    let before: Vec<_> = ds.iter().filter(|d| d.cutoff < t(10, 0, 0)).collect();
    assert!(!before.is_empty());
    assert!(
        before
            .iter()
            .all(|d| pillar(d, PillarId::Float).reason == "no_float_evidence")
    );
    // Visible from the first barrier after its retrieval, with no new print needed.
    assert_eq!(
        pillar(last_for(&ds, "ABCD"), PillarId::Float).state,
        Tri::Pass
    );

    // Retrieved more than 24 h before the decision: stale, so unknown.
    let mut e = engine(Coverage::Complete);
    e.add_float(float("ABCD", 8_000_000, t(9, 0, 0) - 25 * H));
    let mut ds = print(&mut e, "ABCD", t(9, 30, 1), 5_500_000, 1_000);
    ds.extend(e.advance_to(UnixNanos(t(9, 31, 0))));
    assert_eq!(
        pillar(last_for(&ds, "ABCD"), PillarId::Float).reason,
        "float_stale"
    );
}

#[test]
fn a_later_news_edit_does_not_requalify_earlier_decisions() {
    let mut e = engine(Coverage::Complete);
    // Seen at 09:00 as not relevant to the company; an edit seen at 11:00 makes it relevant.
    e.add_news(news(
        "ABCD",
        1,
        false,
        Favorability::Favorable,
        t(8, 59, 0),
        t(9, 0, 0),
    ));
    e.add_news(news(
        "ABCD",
        2,
        true,
        Favorability::Favorable,
        t(8, 59, 0),
        t(11, 0, 0),
    ));
    let mut ds = print(&mut e, "ABCD", t(9, 30, 1), 5_500_000, 1_000);
    ds.extend(e.advance_to(UnixNanos(t(11, 1, 0))));
    for d in ds.iter().filter(|d| d.cutoff < t(11, 0, 0)) {
        assert_ne!(
            pillar(d, PillarId::News).state,
            Tri::Pass,
            "cutoff {}",
            d.cutoff
        );
    }
    let n = pillar(last_for(&ds, "ABCD"), PillarId::News);
    assert_eq!(n.state, Tri::Pass);
    assert_eq!(n.evidence, vec!["news:ABCD:1@2".to_string()]);
}

#[test]
fn adverse_news_is_still_fresh_news_and_favorability_stays_separate() {
    let mut e = engine(Coverage::Complete);
    e.add_news(news(
        "ABCD",
        1,
        true,
        Favorability::Adverse,
        t(8, 0, 0),
        t(8, 0, 1),
    ));
    let mut ds = print(&mut e, "ABCD", t(9, 30, 1), 5_500_000, 1_000);
    ds.extend(e.advance_to(UnixNanos(t(9, 31, 0))));
    let d = last_for(&ds, "ABCD");
    assert_eq!(pillar(d, PillarId::News).state, Tri::Pass);
    assert_eq!(d.news.favorability, Some(Favorability::Adverse));
    assert!(d.news.primary_verified);
}

#[test]
fn news_fails_only_over_covered_sources_and_ages_out() {
    let mut e = engine(Coverage::Complete);
    let mut ds = print(&mut e, "ABCD", t(9, 30, 1), 5_500_000, 1_000);
    ds.extend(e.advance_to(UnixNanos(t(9, 31, 0))));
    assert_eq!(
        pillar(last_for(&ds, "ABCD"), PillarId::News).reason,
        "news_coverage_unknown"
    );

    e.add_coverage(NewsCoverage {
        id: "cov:ABCD".into(),
        instrument: inst("ABCD"),
        sources: vec!["benzinga".into()],
        covered_from: t(9, 0, 0) - 30 * H,
        covered_through: t(17, 0, 0),
        observed_at: t(9, 31, 10),
    });
    let ds = e.advance_to(UnixNanos(t(9, 32, 0)));
    let n = pillar(last_for(&ds, "ABCD"), PillarId::News);
    assert_eq!(
        (n.state, n.reason),
        (Tri::Fail, "no_matching_news_in_covered_sources")
    );
}

#[test]
fn a_failure_keeps_the_unknowns_visible() {
    let mut e = engine(Coverage::Complete);
    // $0.40: price fails; gain, RVOL, float and news have no evidence.
    let mut ds = print(&mut e, "PENY", t(9, 30, 1), 400_000, 1_000);
    ds.extend(e.advance_to(UnixNanos(t(9, 31, 0))));
    let d = last_for(&ds, "PENY");
    assert_eq!(d.qualification, Qualification::NotQualified);
    assert_eq!(
        d.counts,
        Counts {
            pass: 0,
            fail: 1,
            unknown: 4
        }
    );
    // Inside the discovery band ($0.15–$25) though not the reference price band.
    assert_eq!(d.in_discovery_band, Some(true));
}

#[test]
fn a_late_print_joins_the_next_decision_and_earlier_ones_stay() {
    let mut e = engine(Coverage::Complete);
    let mut ds = print(&mut e, "ABCD", t(9, 30, 1), 5_000_000, 100);
    // Executed 09:30:10 (bar ending 09:30:15) but admitted 09:30:20, after that bar's barrier.
    ds.extend(print_at(
        &mut e,
        "ABCD",
        t(9, 30, 10),
        t(9, 30, 20),
        5_100_000,
        900,
    ));
    ds.extend(e.advance_to(UnixNanos(t(9, 31, 0))));
    let first = ds.iter().find(|d| d.bar_end == t(9, 30, 15)).unwrap();
    assert_eq!(first.volume.shares, 100);
    assert_eq!(pillar(first, PillarId::Price).value, Some(5_000_000));
    let next = ds.iter().find(|d| d.bar_end == t(9, 30, 30)).unwrap();
    assert_eq!(next.volume.shares, 1_000);
    assert!(next.changed.contains(&"volume"));
}

#[test]
fn a_stale_price_turns_unknown_without_a_new_print() {
    let mut e = engine(Coverage::Complete);
    let mut ds = print(&mut e, "ABCD", t(9, 30, 1), 5_500_000, 1_000);
    ds.extend(e.advance_to(UnixNanos(t(9, 45, 0))));
    let d = last_for(&ds, "ABCD");
    let p = pillar(d, PillarId::Price);
    assert_eq!((p.state, p.reason), (Tri::Unknown, "price_stale"));
    // Ten minutes after the print, at the first bar end past it.
    assert_eq!(d.bar_end, t(9, 40, 15));
}

#[test]
fn losing_and_regaining_qualification_is_tracked_not_erased() {
    let mut e = engine(Coverage::Complete);
    all_pass_evidence(&mut e, "ABCD");
    let mut ds = print(&mut e, "ABCD", t(9, 30, 1), 5_500_000, 500_000);
    ds.extend(print(&mut e, "ABCD", t(9, 31, 1), 20_500_000, 10)); // above the band
    ds.extend(print(&mut e, "ABCD", t(9, 32, 1), 6_000_000, 10)); // back inside
    ds.extend(e.advance_to(UnixNanos(t(9, 33, 0))));
    let d = last_for(&ds, "ABCD");
    assert_eq!(d.qualification, Qualification::Qualified);
    assert_eq!(d.timeline.first_qualified, Some(t(9, 30, 17)));
    assert_eq!(d.timeline.lost_at, Some(t(9, 31, 17)));
    assert_eq!(d.timeline.requalified, 1);
    assert!(d.timeline.qualified_earlier);
}

/// The fixture used by the determinism and leakage tests: a few symbols, evidence arriving
/// through the morning, prints until `until`.
fn scenario(e: &mut Engine, until: i64) -> Vec<Decision> {
    all_pass_evidence(e, "ABCD");
    e.add_float(float("WXYZ", 50_000_000, t(10, 0, 0)));
    e.add_news(news(
        "WXYZ",
        1,
        true,
        Favorability::Mixed,
        t(10, 5, 0),
        t(10, 5, 30),
    ));
    let mut out = Vec::new();
    let mut k = 0u64;
    let mut at = t(9, 30, 0);
    while at < until {
        k += 1;
        out.extend(print(
            e,
            "ABCD",
            at,
            5_400_000 + (k as i64 % 7) * 30_000,
            1_000 + k * 37,
        ));
        out.extend(print(
            e,
            "WXYZ",
            at + 3_000_000_000,
            12_000_000 - (k as i64 % 5) * 10_000,
            500 + k,
        ));
        at += 11 * 1_000_000_000;
    }
    out.extend(e.advance_to(UnixNanos(until)));
    out
}

fn json(ds: &[Decision]) -> Vec<String> {
    ds.iter()
        .map(|d| serde_json::to_string(d).unwrap())
        .collect()
}

#[test]
fn the_same_input_gives_the_same_decisions_linear_or_rebuilt() {
    let a = scenario(&mut engine(Coverage::Complete), t(10, 30, 0));
    // A seek rebuilds from a fresh engine on the same input.
    let b = scenario(&mut engine(Coverage::Complete), t(10, 30, 0));
    assert!(a.len() > 100);
    assert_eq!(json(&a), json(&b));
}

#[test]
fn appending_the_future_never_changes_the_past() {
    let short = scenario(&mut engine(Coverage::Complete), t(10, 0, 0));
    let mut e = engine(Coverage::Complete);
    // Future evidence known to the store up front: it must stay invisible until observed.
    e.add_float(float("ABCD", 30_000_000, t(10, 20, 0)));
    e.add_news(news(
        "ABCD",
        2,
        true,
        Favorability::Adverse,
        t(10, 10, 0),
        t(10, 15, 0),
    ));
    let long = scenario(&mut e, t(10, 30, 0));
    let past: Vec<_> = long
        .iter()
        .filter(|d| d.cutoff <= t(10, 0, 0))
        .cloned()
        .collect();
    assert_eq!(json(&short), json(&past));
    assert!(long.len() > past.len());
}

#[test]
fn overnight_prints_stay_out_and_the_day_rolls_at_20_00() {
    let mut e = engine(Coverage::Complete);
    let mut ds = print(&mut e, "ABCD", t(19, 59, 0), 5_000_000, 100);
    // 20:30 ET is the next trading day's BOATS overnight: excluded from SIP volume.
    ds.extend(print(&mut e, "ABCD", t(20, 30, 0), 5_000_000, 9_999));
    ds.extend(e.advance_to(UnixNanos(t(20, 45, 0))));
    let d = last_for(&ds, "ABCD");
    assert_eq!(d.trading_date, day());
    assert_eq!(d.volume.shares, 100);
}

fn outstanding(sym: &str, shares: u64, observed: i64) -> OutstandingObservation {
    OutstandingObservation {
        id: format!("outstanding:fixture:{sym}"),
        version: 1,
        instrument: inst(sym),
        shares,
        provider: "fixture".into(),
        observed_at: observed,
        quality: vec![],
    }
}

fn engine_with(policy: Policy) -> Engine {
    Engine::new(
        policy,
        RunInfo {
            run_id: "fixture".into(),
            source: "replay".into(),
            fidelity: Fidelity::SyntheticFixture,
            numerator_coverage: Coverage::Complete,
        },
    )
}

/// The float pillar of ABCD's last decision at 09:31 under `policy`, with `setup` run first.
fn float_at(policy: Policy, setup: impl FnOnce(&mut Engine)) -> PillarResult {
    let mut e = engine_with(policy);
    setup(&mut e);
    let mut ds = print(&mut e, "ABCD", t(9, 30, 1), 5_500_000, 1_000);
    ds.extend(e.advance_to(UnixNanos(t(9, 31, 0))));
    pillar(last_for(&ds, "ABCD"), PillarId::Float).clone()
}

#[test]
fn v5_passes_the_float_on_shares_outstanding_only_where_no_usable_float_exists() {
    let at = t(8, 0, 0);
    // No float record, outstanding below the max: PASS, with its own reason and evidence.
    let p = float_at(Policy::v5(), |e| {
        e.add_outstanding(outstanding("ABCD", 8_000_000, at))
    });
    assert_eq!(
        (p.state, p.reason),
        (Tri::Pass, "below_max_via_outstanding")
    );
    assert_eq!(p.value, Some(8_000_000));
    assert_eq!(p.evidence, vec!["outstanding:fixture:ABCD@1".to_string()]);
    // v4 never reads it.
    let p = float_at(Policy::v4(), |e| {
        e.add_outstanding(outstanding("ABCD", 8_000_000, at))
    });
    assert_eq!((p.state, p.reason), (Tri::Unknown, "no_float_evidence"));
    // Outstanding at or above the max proves nothing: UNKNOWN, never FAIL.
    let p = float_at(Policy::v5(), |e| {
        e.add_outstanding(outstanding("ABCD", 20_000_000, at))
    });
    assert_eq!((p.state, p.reason), (Tri::Unknown, "no_float_evidence"));
    // A zero is never a pass.
    let p = float_at(Policy::v5(), |e| {
        e.add_outstanding(outstanding("ABCD", 0, at))
    });
    assert_eq!(p.state, Tri::Unknown);
    let p = float_at(Policy::v5(), |e| {
        e.add_float(float("ABCD", 0, at));
        e.add_outstanding(outstanding("ABCD", 0, at));
    });
    assert_eq!((p.state, p.reason), (Tri::Unknown, "nonpositive_float"));
    // A zero float with a real outstanding figure below the max: PASS via outstanding.
    let p = float_at(Policy::v5(), |e| {
        e.add_float(float("ABCD", 0, at));
        e.add_outstanding(outstanding("ABCD", 9_000_000, at));
    });
    assert_eq!(p.reason, "below_max_via_outstanding");
    // A usable float decides alone, whatever the outstanding figure says.
    let p = float_at(Policy::v5(), |e| {
        e.add_float(float("ABCD", 25_000_000, at));
        e.add_outstanding(outstanding("ABCD", 9_000_000, at));
    });
    assert_eq!((p.state, p.reason), (Tri::Fail, "at_or_above_max"));
    // A stale float falls back; a stale outstanding figure does not pass.
    let old = t(9, 0, 0) - 5 * 24 * H;
    let p = float_at(Policy::v5(), |e| {
        e.add_float(float("ABCD", 8_000_000, old));
        e.add_outstanding(outstanding("ABCD", 9_000_000, at));
    });
    assert_eq!(p.reason, "below_max_via_outstanding");
    let p = float_at(Policy::v5(), |e| {
        e.add_outstanding(outstanding("ABCD", 9_000_000, old))
    });
    assert_eq!(p.state, Tri::Unknown);
    // Causal: a figure retrieved after the cutoff is invisible.
    let p = float_at(Policy::v5(), |e| {
        e.add_outstanding(outstanding("ABCD", 9_000_000, t(10, 0, 0)))
    });
    assert_eq!(p.state, Tri::Unknown);
}

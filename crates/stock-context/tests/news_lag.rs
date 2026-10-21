//! The pillars-v2 news coverage tolerance (`news.coverage_max_lag_secs`, 120 s, owner decision
//! 2026-10-20) against pillars-v1's strict rule (coverage must reach the cutoff itself).

use chrono::NaiveDate;
use market_events::session::et;
use market_events::{Price, Trade, UnixNanos};
use stock_context::*;

const S: i64 = 1_000_000_000;

fn t(h: u32, m: u32, s: u32) -> i64 {
    et(NaiveDate::from_ymd_opt(2026, 10, 16).unwrap(), h, m, s, 0)
        .unwrap()
        .nanos()
}

/// The decision this fixture reads: bar end 09:30:15, cutoff 09:30:17.
fn cutoff() -> i64 {
    t(9, 30, 17)
}

/// The news pillar at the 09:30:15 bar, coverage ending `gap_secs` before the cutoff, plus an
/// optional qualifying item observed at `item_observed`.
fn news_at(policy: Policy, gap_secs: i64, item_observed: Option<i64>) -> PillarResult {
    let mut e = Engine::new(
        policy,
        RunInfo {
            run_id: "fixture".into(),
            source: "replay".into(),
            fidelity: Fidelity::SyntheticFixture,
            numerator_coverage: Coverage::Complete,
        },
    );
    let inst = InstrumentId::symbol("ABCD");
    e.add_coverage(NewsCoverage {
        id: "cov:ABCD".into(),
        instrument: inst.clone(),
        sources: vec!["fixture".into()],
        covered_from: t(4, 0, 0) - 30 * 3_600 * S,
        covered_through: cutoff() - gap_secs * S,
        // The search completed (and was recorded) before the decision.
        observed_at: t(9, 0, 0),
    });
    if let Some(obs) = item_observed {
        e.add_news(NewsItem {
            id: "news:ABCD".into(),
            version: 1,
            instrument: inst,
            event_type: "other".into(),
            source: "issuer".into(),
            credible_source: true,
            relevant: true,
            substantive: true,
            authenticity: Authenticity::IssuerAnnounced,
            favorability: Favorability::Favorable,
            published_at: Some(obs - S),
            observed_at: obs,
            tag: Tag::Present,
            content_key: "fixture".into(),
            revision_at: Some(0),
            rules: "fixture".into(),
        });
    }
    e.apply(
        &Trade::new(
            "ABCD",
            UnixNanos(t(9, 30, 1)),
            Price::from_micros(5_000_000),
            100,
            1,
        ),
        Verdict {
            ohlc: true,
            volume: true,
        },
        UnixNanos(t(9, 30, 1) + 100_000_000),
    );
    let ds = e.advance_to(UnixNanos(cutoff()));
    let d = ds.iter().rfind(|d| d.symbol == "ABCD").expect("a decision");
    assert_eq!(d.cutoff, cutoff());
    d.pillars
        .iter()
        .find(|p| p.pillar == PillarId::News)
        .unwrap()
        .clone()
}

#[test]
fn v2_fails_with_coverage_ending_up_to_120_s_before_the_decision() {
    for gap in [0, 119, 120] {
        let n = news_at(Policy::v2(), gap, None);
        assert_eq!(
            (n.state, n.reason),
            (Tri::Fail, "no_matching_news_in_covered_sources"),
            "gap {gap} s"
        );
    }
}

#[test]
fn v2_is_unknown_with_coverage_ending_121_s_before_the_decision() {
    let n = news_at(Policy::v2(), 121, None);
    assert_eq!((n.state, n.reason), (Tri::Unknown, "news_coverage_unknown"));
}

#[test]
fn v1_keeps_the_strict_rule_on_the_same_fixture() {
    assert_eq!(Policy::v1().body.news.coverage_max_lag_secs, 0);
    assert_eq!(news_at(Policy::v1(), 0, None).state, Tri::Fail);
    for gap in [1, 119, 120, 121] {
        let n = news_at(Policy::v1(), gap, None);
        assert_eq!(
            (n.state, n.reason),
            (Tri::Unknown, "news_coverage_unknown"),
            "gap {gap} s"
        );
    }
}

#[test]
fn an_item_observed_inside_the_lag_window_before_the_decision_still_passes() {
    // Coverage ends 120 s before the cutoff; the item was first seen 30 s before it.
    for p in [Policy::v2(), Policy::v1()] {
        let n = news_at(p, 120, Some(cutoff() - 30 * S));
        assert_eq!((n.state, n.reason), (Tri::Pass, "fresh_relevant_news"));
    }
    // Observed after the cutoff: invisible to this decision (no lookahead), so v2 FAILs.
    let n = news_at(Policy::v2(), 120, Some(cutoff() + S));
    assert_eq!(n.state, Tri::Fail);
}

//! Latest-visible news versions and tombstones.
//!
//! A news story is append-only: an edit is a new version of the same story id, a removed symbol
//! tag is a tombstone version. A decision must read **one** version per story — the latest one
//! visible at its cutoff — never every version ever observed. Reading them all lets a version
//! that a later edit disqualified keep the news pillar passing forever (reproduced by the audit
//! probe `repro_superseded_news_keeps_passing_after_disqualifying_edit`).
//!
//! Earlier decisions never change: a version only affects cutoffs at or after its `observed_at`,
//! and every version stays in the store so an older decision can be replayed exactly.

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

/// One version of the story `news:ABCD:1`.
fn version(v: u32, substantive: bool, relevant: bool, observed: i64) -> NewsItem {
    NewsItem {
        id: "news:ABCD:1".into(),
        version: v,
        instrument: inst("ABCD"),
        event_type: if substantive {
            "clinical_result".into()
        } else {
            "other".into()
        },
        source: "alpaca/benzinga".into(),
        credible_source: true,
        relevant,
        substantive,
        authenticity: Authenticity::ReportedUnverified,
        favorability: Favorability::Undetermined,
        published_at: Some(t(8, 0, 0)),
        observed_at: observed,
        tag: Tag::Present,
        rules: "news-rules-v2".into(),
    }
}

/// The version that drops ABCD's symbol tag: the story is no longer about ABCD.
fn tombstone(v: u32, observed: i64) -> NewsItem {
    NewsItem {
        tag: Tag::Removed,
        ..version(v, true, true, observed)
    }
}

fn pillar(d: &Decision, p: PillarId) -> &PillarResult {
    d.pillars.iter().find(|x| x.pillar == p).unwrap()
}

fn run(e: &mut Engine, until: (u32, u32, u32)) -> Vec<Decision> {
    let (h, m, s) = until;
    let mut ds = e.apply(
        &Trade::new(
            "ABCD",
            UnixNanos(t(9, 30, 1)),
            Price::from_micros(5_500_000),
            1_000,
            1,
        ),
        VOL,
        UnixNanos(t(9, 30, 1)),
    );
    ds.extend(e.advance_to(UnixNanos(t(h, m, s))));
    ds
}

fn news_states(ds: &[Decision]) -> Vec<(i64, Tri)> {
    ds.iter()
        .map(|d| (d.cutoff, pillar(d, PillarId::News).state))
        .collect()
}

#[test]
fn a_disqualifying_edit_stops_the_story_qualifying() {
    let mut e = engine();
    e.add_news(version(1, true, true, t(9, 0, 0)));
    // 10:00: the story is re-fetched as pure commentary, so it no longer qualifies.
    e.add_news(version(2, false, true, t(10, 0, 0)));
    let ds = run(&mut e, (10, 1, 0));
    for (cutoff, state) in news_states(&ds) {
        if cutoff < t(10, 0, 0) {
            assert_eq!(state, Tri::Pass, "v1 qualified at cutoff {cutoff}");
        } else {
            assert_ne!(
                state,
                Tri::Pass,
                "v2 superseded v1: no qualifying version at cutoff {cutoff}"
            );
        }
    }
    // The superseded version is still stored: an earlier decision stays replayable.
    let all = e.evidence_len();
    assert!(all >= 2, "both versions retained, got {all}");
}

#[test]
fn a_removed_symbol_tag_is_a_tombstone() {
    let mut e = engine();
    e.add_news(version(1, true, true, t(9, 0, 0)));
    e.add_news(tombstone(2, t(10, 0, 0)));
    let ds = run(&mut e, (10, 1, 0));
    for (cutoff, state) in news_states(&ds) {
        if cutoff < t(10, 0, 0) {
            assert_eq!(state, Tri::Pass, "tagged at cutoff {cutoff}");
        } else {
            assert_ne!(state, Tri::Pass, "untagged at cutoff {cutoff}");
        }
    }
}

#[test]
fn a_restoring_edit_qualifies_again_from_its_own_observation_on() {
    let mut e = engine();
    e.add_news(version(1, false, true, t(9, 0, 0)));
    e.add_news(tombstone(2, t(9, 40, 0)));
    e.add_news(version(3, true, true, t(10, 0, 0)));
    let ds = run(&mut e, (10, 1, 0));
    for (cutoff, state) in news_states(&ds) {
        if cutoff < t(10, 0, 0) {
            assert_ne!(state, Tri::Pass, "cutoff {cutoff}");
        } else {
            assert_eq!(state, Tri::Pass, "cutoff {cutoff}");
        }
    }
    let last = ds.last().expect("a decision");
    assert_eq!(last.news.items, vec!["news:ABCD:1@3".to_string()]);
}

#[test]
fn distinct_stories_are_never_collapsed() {
    let mut e = engine();
    let other = NewsItem {
        id: "news:ABCD:2".into(),
        ..version(1, true, true, t(9, 0, 0))
    };
    e.add_news(version(1, true, true, t(9, 0, 0)));
    e.add_news(other);
    // A disqualifying edit of the first story leaves the second one qualifying on its own.
    e.add_news(version(2, false, true, t(10, 0, 0)));
    let ds = run(&mut e, (10, 1, 0));
    let last = ds.last().expect("a decision");
    assert_eq!(pillar(last, PillarId::News).state, Tri::Pass);
    assert_eq!(last.news.items, vec!["news:ABCD:2@1".to_string()]);
}

#[test]
fn an_edit_observed_exactly_at_the_cutoff_is_visible_to_it() {
    let store_at = |observed_v2: i64, cutoff: i64| {
        let mut s = EvidenceStore::default();
        s.add_news(version(1, true, true, t(9, 0, 0)));
        s.add_news(version(2, false, true, observed_v2));
        s.news_current(&inst("ABCD"), cutoff)
            .iter()
            .map(|n| n.version)
            .collect::<Vec<_>>()
    };
    let tau = t(10, 0, 0);
    assert_eq!(store_at(tau, tau - 1), vec![1], "not observed yet");
    assert_eq!(store_at(tau, tau), vec![2], "at the cutoff: visible");
    assert_eq!(store_at(tau, tau + 1), vec![2]);
}

#[test]
fn pruning_never_revives_a_superseded_version() {
    let mut s = EvidenceStore::default();
    let fresh = 24 * 3600 * 1_000_000_000;
    s.add_news(version(1, true, true, t(9, 0, 0)));
    // The edit's own publication time moved past the freshness window of the prune point.
    let mut v2 = version(2, false, true, t(9, 30, 0));
    v2.published_at = Some(t(9, 0, 0) - fresh - 1);
    s.add_news(v2);
    s.prune(t(9, 31, 0), fresh);
    let current = s.news_current(&inst("ABCD"), t(10, 0, 0));
    assert!(
        current.iter().all(|n| !n.substantive),
        "the latest visible version must still be the non-substantive edit, got {:?}",
        current.iter().map(|n| n.version).collect::<Vec<_>>()
    );
}

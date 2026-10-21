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
        content_key: format!("fixture-v{v}"),
        revision_at: Some(v as i64),
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

// --- ambiguity only blocks what it could decide ----------------------------------------------

/// A version of another story, so one symbol can carry two stories at once.
fn other_story(id: &str, content: &str, revision: i64, observed: i64, published: i64) -> NewsItem {
    NewsItem {
        id: id.into(),
        version: 1,
        content_key: content.into(),
        revision_at: Some(revision),
        published_at: Some(published),
        observed_at: observed,
        ..version(1, true, true, observed)
    }
}

/// One story whose current version cannot be established does not block a fresh, separately
/// established catalyst.
///
/// The pillar's claim is existential: one qualifying story is enough. A first draft asked the
/// broad question "is anything about this symbol ambiguous" before looking at the qualifying
/// items at all, so a stale conflicting story — outside the freshness window, or irrelevant —
/// forced UNKNOWN beside a perfectly good company catalyst. The ambiguity has to be able to
/// change the answer before it may withhold one.
#[test]
fn an_unrelated_ambiguous_story_does_not_block_a_fresh_catalyst() {
    let mut e = engine();
    // A fresh, unambiguous catalyst.
    e.add_news(version(1, true, true, t(9, 0, 0)));
    // And a second story with two contents claiming one revision time — but published two days
    // ago, so no version of it could qualify whichever is current.
    let stale = t(8, 0, 0) - 2 * 24 * 3_600 * 1_000_000_000;
    e.add_news(other_story("news:ABCD:2", "a", 50, t(9, 0, 0), stale));
    e.add_news(other_story("news:ABCD:2", "b", 50, t(9, 0, 0), stale));

    let ds = run(&mut e, (9, 31, 0));
    let last = ds.last().expect("a decision");
    assert_eq!(
        pillar(last, PillarId::News).state,
        Tri::Pass,
        "the fresh catalyst decides it: {:?}",
        pillar(last, PillarId::News)
    );
    assert_eq!(
        last.news.items,
        vec!["news:ABCD:1@1".to_string()],
        "and only the story that was actually established is cited"
    );
}

/// An ambiguous story that *could* qualify does withhold the answer, because which version is
/// current decides whether this symbol has a catalyst at all.
#[test]
fn an_ambiguous_story_that_could_qualify_withholds_the_answer() {
    let mut e = engine();
    let published = t(8, 0, 0);
    // Two contents at one revision time, both fresh and both qualifying.
    e.add_news(other_story("news:ABCD:2", "a", 50, t(9, 0, 0), published));
    e.add_news(other_story("news:ABCD:2", "b", 50, t(9, 0, 0), published));
    // Market-wide coverage, so without the ambiguity this would be a confident FAIL.
    e.add_coverage(NewsCoverage {
        id: "cov:all".into(),
        instrument: InstrumentId::market(),
        sources: vec!["benzinga".into()],
        covered_from: t(4, 0, 0) - 48 * 3_600 * 1_000_000_000,
        covered_through: t(9, 31, 0),
        observed_at: t(9, 0, 0),
    });

    let ds = run(&mut e, (9, 31, 0));
    let last = ds.last().expect("a decision");
    let p = pillar(last, PillarId::News);
    assert_eq!(p.state, Tri::Unknown, "{p:?}");
    assert_eq!(p.reason, "news_revision_order_unknown");
    assert!(
        last.news.items.is_empty(),
        "nothing is cited from a story whose current version is a guess"
    );
}

/// With no ambiguity and covered sources, the absence of qualifying news is still a FAIL: the
/// ambiguity check does not turn every FAIL into UNKNOWN.
#[test]
fn covered_sources_with_nothing_qualifying_still_fail() {
    let mut e = engine();
    e.add_coverage(NewsCoverage {
        id: "cov:all".into(),
        instrument: InstrumentId::market(),
        sources: vec!["benzinga".into()],
        covered_from: t(4, 0, 0) - 48 * 3_600 * 1_000_000_000,
        covered_through: t(9, 31, 0),
        observed_at: t(9, 0, 0),
    });
    let ds = run(&mut e, (9, 31, 0));
    let p = pillar(ds.last().expect("a decision"), PillarId::News);
    assert_eq!(p.state, Tri::Fail, "{p:?}");
    assert_eq!(p.reason, "no_matching_news_in_covered_sources");
}

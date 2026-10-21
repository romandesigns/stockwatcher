//! `news-rules-v2`: the mixed catalyst/commentary headline, symbol-tag tombstones, and the rules
//! identity carried on every item.
//!
//! `news-rules-v1` treats any tape remark ("Shares Trading Higher") as commentary for the whole
//! headline, so a real catalyst with one appended remark is classified not substantive and the
//! news pillar rejects it. The audit probe
//! `repro_real_catalyst_is_discarded_when_headline_also_says_trading_higher` reproduces it with
//! "TEST Receives FDA Approval, Shares Trading Higher".
//!
//! v2 narrows that: a screen, roundup or analyst-note subject is still never substantive, but a
//! tape remark appended to a clause that states a company event no longer cancels the event.
//! v1 stays callable byte-for-byte, so stored stories can be re-normalized under the rules a
//! decision was made with.

use market_archive::evidence::NewsVersionRecord;
use market_archive::evidence_load::{
    NEWS_RULES_V1, NEWS_RULES_V2, NEWS_RULES_VERSION, NOISE_RULES, NewsRules,
    PRICE_COMMENTARY_RULES, SCREEN_RULES, classify_headline_with, news_items,
};
use std::collections::BTreeSet;
use stock_context::Tag;

/// One stored version of story `1`, tagging `symbols`.
fn version(v: u32, headline: &str, symbols: &[&str], observed: i64) -> NewsVersionRecord {
    NewsVersionRecord {
        v: 1,
        provider: "alpaca".into(),
        source: "benzinga".into(),
        article_id: "1".into(),
        created_at: None,
        updated_at: None,
        created_at_ns: Some(observed - 60),
        updated_at_ns: None,
        first_observed_at: observed,
        headline: headline.into(),
        headline_sha256: format!("h{v}"),
        summary_sha256: format!("s{v}"),
        author: None,
        symbols: symbols.iter().map(|s| s.to_string()).collect(),
        url: None,
        version_key: format!("k{v}"),
    }
}

fn v2(headline: &str) -> (&'static str, bool) {
    classify_headline_with(NewsRules::V2, headline)
}
fn v1(headline: &str) -> (&'static str, bool) {
    classify_headline_with(NewsRules::V1, headline)
}

#[test]
fn a_catalyst_with_an_appended_tape_remark_stays_substantive() {
    let h = "TEST Receives FDA Approval, Shares Trading Higher";
    assert_eq!(v1(h), ("regulatory_decision", true), "v1 is unchanged");
    assert_eq!(v2(h), ("regulatory_decision", false));
    // And the same headline with a dash instead of a comma.
    assert_eq!(
        v2("TEST Receives FDA Approval - Shares Trading Higher"),
        ("regulatory_decision", false)
    );
    assert_eq!(
        v2("TEST Announces Phase 3 Topline Results; Stock Trading Higher"),
        ("clinical_result", false)
    );
}

#[test]
fn an_adverse_event_is_still_an_event() {
    // Favorability is never inferred from keywords: a CRL is news, and the pillar says so.
    assert_eq!(
        v2("TEST Receives Complete Response Letter From FDA, Shares Trading Lower"),
        ("regulatory_decision", false)
    );
}

#[test]
fn screens_roundups_and_analyst_notes_are_never_substantive() {
    for h in [
        "Stocks Moving In Monday's Mid-Day Session",
        "12 Health Care Stocks Moving In Monday's Mid-Day Session: TEST Receives FDA Approval",
        "TEST Price Target Raised To $5 By Some Bank",
        "Top Analyst Ratings For October 21",
        "Unusual Options Activity In TEST",
        "Check Out These Whale Alerts In TEST",
        "What's Moving In Health Care Today",
    ] {
        assert!(v2(h).1, "must stay noise: {h}");
    }
}

#[test]
fn pure_tape_commentary_is_still_not_substantive() {
    for h in [
        "TEST Shares Are Trading Higher On Volume",
        "TEST Stock Is Trading Lower Today",
        "TEST Hits A New 52-Week High",
    ] {
        let (event, noise) = v2(h);
        assert!(
            event == "other" || noise,
            "no company event in {h:?}: got ({event}, {noise})"
        );
    }
}

#[test]
fn v2_drops_no_phrase_v1_used() {
    let v1: BTreeSet<&str> = NOISE_RULES.iter().copied().collect();
    let v2: BTreeSet<&str> = SCREEN_RULES
        .iter()
        .chain(PRICE_COMMENTARY_RULES.iter())
        .copied()
        .collect();
    assert_eq!(v1, v2, "v2 partitions v1's phrases, it does not drop any");
}

#[test]
fn a_dropped_symbol_tag_becomes_a_tombstone() {
    let records = vec![
        version(
            1,
            "AAA And BBB Sign A Supply Agreement",
            &["AAA", "BBB"],
            100,
        ),
        // The edit keeps only AAA: BBB is no longer tagged.
        version(2, "AAA Signs A Supply Agreement", &["AAA"], 200),
    ];
    let items = news_items(&records, None);
    let bbb: Vec<_> = items
        .iter()
        .filter(|n| n.instrument == stock_context::InstrumentId::symbol("BBB"))
        .collect();
    assert_eq!(bbb.len(), 2, "the tagged version and its tombstone");
    assert_eq!((bbb[0].version, bbb[0].tag), (1, Tag::Present));
    assert_eq!((bbb[1].version, bbb[1].tag), (2, Tag::Removed));
    assert_eq!(bbb[1].observed_at, 200, "visible from the edit on");
    assert!(
        !bbb[1].substantive && !bbb[1].relevant,
        "a tombstone can never qualify, whatever reads it"
    );
    let aaa: Vec<_> = items
        .iter()
        .filter(|n| n.instrument == stock_context::InstrumentId::symbol("AAA"))
        .collect();
    assert_eq!(aaa.len(), 2);
    assert!(aaa.iter().all(|n| n.tag == Tag::Present));
}

#[test]
fn a_re_added_symbol_tag_is_present_again() {
    let records = vec![
        version(1, "AAA Receives FDA Approval", &["AAA"], 100),
        version(2, "AAA Receives FDA Approval", &["ZZZ"], 200),
        version(3, "AAA Receives FDA Approval", &["AAA"], 300),
    ];
    let only: BTreeSet<String> = ["AAA".to_string()].into();
    let items = news_items(&records, Some(&only));
    assert_eq!(
        items.iter().map(|n| (n.version, n.tag)).collect::<Vec<_>>(),
        [(1, Tag::Present), (2, Tag::Removed), (3, Tag::Present)]
    );
}

#[test]
fn every_item_carries_its_rules_identity() {
    let records = vec![version(1, "AAA Receives FDA Approval", &["AAA"], 100)];
    assert_eq!(NEWS_RULES_VERSION, NEWS_RULES_V2);
    assert!(
        news_items(&records, None)
            .iter()
            .all(|n| n.rules == NEWS_RULES_V2)
    );
    assert!(
        market_archive::evidence_load::news_items_with(NewsRules::V1, &records, None)
            .iter()
            .all(|n| n.rules == NEWS_RULES_V1)
    );
}

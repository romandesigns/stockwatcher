//! Which version of a story is current, when two transports deliver it out of order — and when
//! that question has no answer.
//!
//! A REST page and a news stream deliver the same article independently, so an older revision can
//! arrive after a newer one: a page fetched before an edit and served after it, or a stream
//! reconnect replaying what the pages have already moved past. Nothing about that is unusual.
//!
//! Two properties have to hold together, and the first draft of this broke both:
//!
//! - **Current content follows the provider's revision order**, not the order we happened to see
//!   things in. Ordering by first sighting made the late arrival current, so the story read as
//!   having reverted and a decision would quote a headline already replaced.
//! - **Nothing a later observation reveals may change an earlier decision.** The draft computed
//!   each story's ambiguity over every row in the file and stamped it on all of that story's
//!   items, so a conflict first seen at 11:00 made an unambiguous 09:00 item UNKNOWN when
//!   replayed at 10:00. That is lookahead, however the field is named. Identity is immutable too:
//!   `version` is the first-sighting ordinal, so discovering an older revision late cannot
//!   renumber evidence a decision has already cited.
//!
//! Every fixture is synthetic and offline.

use chrono::{DateTime, Utc};
use market_archive::NewsVersionRecord;
use market_archive::evidence_load::news_items;
use stock_context::{EvidenceStore, InstrumentId, NewsItem, Tag};

const H: i64 = 3_600 * 1_000_000_000;

fn ns(rfc: &str) -> i64 {
    DateTime::parse_from_rfc3339(rfc)
        .expect("a timestamp")
        .with_timezone(&Utc)
        .timestamp_nanos_opt()
        .expect("in range")
}

/// A stored version of story 1. `updated` is the provider's revision time, `seen` ours.
fn version(headline: &str, updated: Option<&str>, seen: i64, symbols: &[&str]) -> NewsVersionRecord {
    let syms: Vec<String> = symbols.iter().map(|s| s.to_string()).collect();
    NewsVersionRecord {
        v: 1,
        provider: "alpaca".into(),
        source: "benzinga".into(),
        article_id: "1".into(),
        created_at: Some("2026-10-15T13:00:00Z".into()),
        updated_at: updated.map(|u| u.to_string()),
        created_at_ns: Some(ns("2026-10-15T13:00:00Z")),
        updated_at_ns: updated.map(ns),
        first_observed_at: seen,
        headline: headline.into(),
        headline_sha256: market_archive::sha256_hex(headline.as_bytes()),
        summary_sha256: market_archive::sha256_hex(b""),
        author: Some("Staff".into()),
        symbols: syms.clone(),
        url: Some("https://example.invalid/1".into()),
        // What distinguishes versions, as the collector computes it.
        version_key: market_archive::sha256_hex(
            format!("{}|{headline}|{}", updated.unwrap_or(""), syms.join(",")).as_bytes(),
        ),
    }
}

/// The same as [`version`], named for the cases where the symbol set is the point.
fn tagged(headline: &str, updated: Option<&str>, seen: i64, symbols: &[&str]) -> NewsVersionRecord {
    version(headline, updated, seen, symbols)
}

/// The projected items for ABCD in a store, as a decision at `cutoff` sees them.
fn store_of(records: &[NewsVersionRecord]) -> EvidenceStore {
    let mut store = EvidenceStore::default();
    for item in news_items(records, None) {
        store.add_news(item);
    }
    store
}

fn abcd() -> InstrumentId {
    InstrumentId::symbol("ABCD")
}

/// What a decision at `cutoff` would see: the current version's ordinal and headline-derived
/// event type, whether the story's order was established, and the evidence identity.
fn seen_at(records: &[NewsVersionRecord], cutoff: i64) -> Vec<(String, u32, bool, i64, Tag)> {
    let store = store_of(records);
    store
        .news_stories(&abcd(), cutoff)
        .into_iter()
        .map(|s| {
            (
                s.current.id.clone(),
                s.current.version,
                s.ordered,
                s.current.observed_at,
                s.current.tag,
            )
        })
        .collect()
}

/// The decisive causality check: a conflict first observed at 11:00 must not change anything a
/// decision at 10:00 sees.
///
/// Projecting the whole archive and evaluating at 10:00 has to agree with projecting only what
/// had been observed by 09:00 — identity included. From 11:00 on, the conflict may make the story
/// unanswerable, and that is the only time it may.
#[test]
fn a_later_conflict_does_not_change_an_earlier_decision() {
    let nine = ns("2026-10-15T13:00:00Z");
    let v1 = version(
        "ABCD Announces FDA Approval",
        Some("2026-10-15T13:00:00Z"),
        nine,
        &["ABCD"],
    );
    // A second content claiming the *same* revision time, first seen two hours later: the
    // provider says they are one revision and they are not.
    let conflicting = version(
        "ABCD Announces Something Else Entirely",
        Some("2026-10-15T13:00:00Z"),
        nine + 2 * H,
        &["ABCD"],
    );

    let prefix = seen_at(std::slice::from_ref(&v1), nine + H);
    let full = seen_at(&[v1.clone(), conflicting.clone()], nine + H);
    assert_eq!(
        full, prefix,
        "a decision at 10:00 sees what the 09:00 prefix alone would give it"
    );
    assert_eq!(full.len(), 1);
    let (id, version_n, ordered, observed, tag) = full[0].clone();
    assert_eq!((version_n, ordered, observed, tag), (1, true, nine, Tag::Present));
    assert_eq!(id, "news:alpaca:1");

    // From 11:00 the conflict is visible, and only then is the story unanswerable.
    let after = seen_at(&[v1.clone(), conflicting.clone()], nine + 3 * H);
    assert_eq!(after.len(), 1);
    assert!(
        !after[0].2,
        "the conflict is known now, so which version is current is not: {after:?}"
    );
    // Which of the two is nominally "current" is beside the point once the order is unknown —
    // nothing may be decided on it. What matters is that the earlier version's own identity did
    // not move: a decision that cited `news:alpaca:1@1` still means the same evidence.
    let store = store_of(&[v1, conflicting]);
    let identities: Vec<(u32, i64)> = store
        .news_stories(&abcd(), nine + 3 * H)
        .into_iter()
        .flat_map(|s| s.visible.into_iter().map(|n| (n.version, n.observed_at)))
        .collect();
    assert!(
        identities.contains(&(1, nine)),
        "version 1 is still version 1, first seen when it was: {identities:?}"
    );
    assert_eq!(identities.len(), 2, "and both are retained: {identities:?}");
}

/// A late *older* provider revision is retained as evidence and does not become current, and it
/// does not renumber what was already published.
///
/// Numbering by provider revision time would have inserted it before the versions already seen,
/// so evidence a decision had cited by `(id, version)` would afterwards mean something else.
#[test]
fn a_late_older_revision_is_kept_without_becoming_current_or_renumbering() {
    let t0 = ns("2026-10-15T13:00:00Z");
    let v_new = version(
        "ABCD Announces FDA Approval Of Lead Candidate",
        Some("2026-10-15T14:00:00Z"),
        t0,
        &["ABCD"],
    );
    let v_old_late = version(
        "ABCD Announces FDA Approval",
        Some("2026-10-15T13:00:00Z"),
        t0 + 2 * H,
        &["ABCD"],
    );

    // One-shot over the whole archive.
    let one_shot = news_items(&[v_new.clone(), v_old_late.clone()], None);
    let ids: Vec<(String, u32, i64)> = one_shot
        .iter()
        .filter(|n| n.instrument == abcd())
        .map(|n| (n.id.clone(), n.version, n.observed_at))
        .collect();
    // Incremental: the first refresh saw only the newer one.
    let incremental = news_items(std::slice::from_ref(&v_new), None);
    let first: Vec<(String, u32, i64)> = incremental
        .iter()
        .filter(|n| n.instrument == abcd())
        .map(|n| (n.id.clone(), n.version, n.observed_at))
        .collect();
    assert_eq!(
        first,
        vec![("news:alpaca:1".to_string(), 1, t0)],
        "the first refresh published version 1"
    );
    assert!(
        ids.contains(&("news:alpaca:1".to_string(), 1, t0)),
        "and it is still version 1 after the older revision arrived: {ids:?}"
    );
    assert_eq!(ids.len(), 2, "the late older revision is kept as history: {ids:?}");

    // It is not current: the provider's revision order puts it before the newer one.
    let after = seen_at(&[v_new, v_old_late], t0 + 3 * H);
    assert_eq!(after.len(), 1);
    assert_eq!(
        (after[0].1, after[0].3),
        (1, t0),
        "the newer revision, which was seen first, is still current: {after:?}"
    );
    assert!(after[0].2, "and the order was established: {after:?}");
}

/// A symbol tag removed by an edit stays removed, and a late redelivery of the pre-edit version
/// does not give the symbol its news back.
#[test]
fn a_removed_symbol_tag_is_not_restored_by_a_late_redelivery() {
    let t0 = ns("2026-10-15T13:00:00Z");
    let tagged = version(
        "ABCD And WXYZ Announce A Partnership",
        Some("2026-10-15T13:00:00Z"),
        t0,
        &["ABCD", "WXYZ"],
    );
    let removed = version(
        "WXYZ Announces A Partnership",
        Some("2026-10-15T14:00:00Z"),
        t0 + H,
        &["WXYZ"],
    );
    let tagged_again = version(
        "ABCD And WXYZ Announce A Partnership",
        Some("2026-10-15T13:00:00Z"),
        t0 + 5 * H,
        &["ABCD", "WXYZ"],
    );

    // After the edit: ABCD's current version is a tombstone.
    let after_edit = seen_at(&[tagged.clone(), removed.clone()], t0 + 2 * H);
    assert_eq!(after_edit.len(), 1);
    assert_eq!(
        after_edit[0].4,
        Tag::Removed,
        "the edit dropped ABCD: {after_edit:?}"
    );

    // The pre-edit version redelivered later is an older revision and does not become current.
    let after_replay = seen_at(&[tagged, removed, tagged_again], t0 + 6 * H);
    assert_eq!(after_replay.len(), 1);
    assert_eq!(
        after_replay[0].4,
        Tag::Removed,
        "a replay of the pre-edit version does not restore the tag: {after_replay:?}"
    );
    assert!(after_replay[0].2);
}

/// `news_current` reports the present-tagged current version per story, and nothing visible later.
#[test]
fn news_current_is_the_current_version_visible_at_the_cutoff() {
    let t0 = ns("2026-10-15T13:00:00Z");
    let v1 = version(
        "ABCD Announces FDA Approval",
        Some("2026-10-15T13:00:00Z"),
        t0,
        &["ABCD"],
    );
    let v2 = version(
        "ABCD Announces FDA Approval Of Lead Candidate",
        Some("2026-10-15T14:00:00Z"),
        t0 + H,
        &["ABCD"],
    );
    let store = store_of(&[v1, v2]);
    let early: Vec<u32> = store
        .news_current(&abcd(), t0)
        .iter()
        .map(|n| n.version)
        .collect();
    assert_eq!(early, vec![1], "only what had been observed");
    let later: Vec<u32> = store
        .news_current(&abcd(), t0 + 2 * H)
        .iter()
        .map(|n| n.version)
        .collect();
    assert_eq!(later, vec![2], "the edit is current once it is visible");
}

/// A version with no usable revision time cannot be placed against ones that have it; a story
/// whose every version lacks one has nothing to conflict with.
#[test]
fn a_version_without_a_revision_time_is_only_a_problem_beside_dated_ones() {
    let t0 = ns("2026-10-15T13:00:00Z");
    let dated = NewsVersionRecord {
        created_at_ns: Some(ns("2026-10-15T13:00:00Z")),
        ..version(
            "ABCD Announces FDA Approval",
            Some("2026-10-15T13:00:00Z"),
            t0,
            &["ABCD"],
        )
    };
    // No updated_at and no parsable created_at either.
    let undated = NewsVersionRecord {
        created_at: None,
        created_at_ns: None,
        ..version("ABCD Announces An Update", None, t0 + H, &["ABCD"])
    };
    let mixed = seen_at(&[dated, undated.clone()], t0 + 2 * H);
    assert_eq!(mixed.len(), 1);
    assert!(
        !mixed[0].2,
        "nothing places the undated version against the dated one: {mixed:?}"
    );

    let alone = seen_at(std::slice::from_ref(&undated), t0 + 2 * H);
    assert!(
        alone[0].2,
        "one undated version has nothing to conflict with: {alone:?}"
    );
}

/// The item carries the provider's revision time as a fact, and the ordinal remains observation
/// order.
#[test]
fn an_item_carries_the_revision_time_and_an_observation_ordinal() {
    let t0 = ns("2026-10-15T13:00:00Z");
    let v = version(
        "ABCD Announces FDA Approval",
        Some("2026-10-15T14:30:00Z"),
        t0,
        &["ABCD"],
    );
    let items: Vec<NewsItem> = news_items(std::slice::from_ref(&v), None);
    let mine = items.iter().find(|n| n.instrument == abcd()).expect("ABCD");
    assert_eq!(mine.revision_at, Some(ns("2026-10-15T14:30:00Z")));
    assert_eq!(mine.observed_at, t0, "and our own first sighting is separate");
    assert_eq!(mine.version, 1);
}

/// A symbol revealed only by a late-arriving *older* revision does not qualify on it, because the
/// newer revision — which was seen first — does not mention that symbol.
///
/// Revision 10 (content B) is seen at 09:00 and tags only WXYZ. Revision 08 (content A) arrives at
/// 11:00 and tags ABCD. The projection is per instrument, so ABCD's only version used to be A:
/// the one that tags it. It never learned that the story's current content dropped it, and
/// qualified on a mention the provider had already superseded.
#[test]
fn a_symbol_revealed_by_a_late_older_revision_does_not_qualify_on_it() {
    let nine = ns("2026-10-15T13:00:00Z");
    let newer_b = tagged(
        "WXYZ Announces FDA Approval",
        Some("2026-10-15T14:10:00Z"),
        nine,
        &["WXYZ"],
    );
    let older_a = tagged(
        "ABCD And WXYZ Announce FDA Approval",
        Some("2026-10-15T14:08:00Z"),
        nine + 2 * H,
        &["ABCD", "WXYZ"],
    );
    let records = [newer_b.clone(), older_a.clone()];

    // At 10:00 ABCD has nothing: its relation to the story was not known yet, and a tombstone
    // stamped at the newer revision's own sighting would have inserted a record at a time
    // nothing knew of it.
    let early = seen_at(&records, nine + H);
    assert!(
        early.is_empty(),
        "nothing about ABCD was known at 10:00: {early:?}"
    );
    assert_eq!(
        early,
        seen_at(std::slice::from_ref(&newer_b), nine + H),
        "and the full archive agrees with the 09:00 prefix"
    );

    // At 11:00 ABCD's relation is known — and so is the fact that the newer revision drops it.
    let after = seen_at(&records, nine + 3 * H);
    assert_eq!(after.len(), 1, "{after:?}");
    assert_eq!(
        after[0].4,
        Tag::Removed,
        "the current content is the newer revision, which does not mention ABCD: {after:?}"
    );
    assert!(after[0].2, "and the order is established: {after:?}");
    let store = store_of(&records);
    assert!(
        store.news_current(&abcd(), nine + 3 * H).is_empty(),
        "so ABCD has no current news from this story"
    );

    // WXYZ, which the newer revision does tag, qualifies throughout.
    let wxyz = InstrumentId::symbol("WXYZ");
    assert_eq!(
        store.news_current(&wxyz, nine + H).len(),
        1,
        "WXYZ is in the newer revision and was seen at 09:00"
    );
    assert_eq!(
        store.news_current(&wxyz, nine + 3 * H).len(),
        1,
        "and the older revision arriving later does not take it away"
    );
}

/// A unique newer revision settles a story whose older revisions had tied.
///
/// Two contents arrived claiming revision 09 — unanswerable. Then a unique revision 11 arrives.
/// What the story says *now* is no longer in doubt, and the obsolete tie must stop forcing
/// UNKNOWN: an ambiguity in a story's history is not an ambiguity about its current content.
#[test]
fn a_unique_newer_revision_settles_an_obsolete_tie() {
    let t0 = ns("2026-10-15T13:00:00Z");
    let a = tagged(
        "ABCD Announces FDA Approval",
        Some("2026-10-15T14:09:00Z"),
        t0,
        &["ABCD"],
    );
    let b = tagged(
        "ABCD Announces Something Else Entirely",
        Some("2026-10-15T14:09:00Z"),
        t0 + H,
        &["ABCD"],
    );
    // While only the tie is visible, the story decides nothing.
    let tied = seen_at(&[a.clone(), b.clone()], t0 + 2 * H);
    assert_eq!(tied.len(), 1);
    assert!(!tied[0].2, "two contents at one revision time: {tied:?}");

    // A later, unique revision resolves what the story currently says.
    let c = tagged(
        "ABCD Announces FDA Approval Of Lead Candidate",
        Some("2026-10-15T14:11:00Z"),
        t0 + 3 * H,
        &["ABCD"],
    );
    let settled = seen_at(&[a.clone(), b.clone(), c.clone()], t0 + 4 * H);
    assert_eq!(settled.len(), 1);
    assert!(
        settled[0].2,
        "the newest revision is unique, so the current content is settled: {settled:?}"
    );
    assert_eq!(
        settled[0].3,
        t0 + 3 * H,
        "and it is the one that resolved it: {settled:?}"
    );
    // Earlier cutoffs are untouched: the resolution is known from its own observation on.
    let still_tied = seen_at(&[a, b, c], t0 + 2 * H);
    assert!(
        !still_tied[0].2,
        "at 15:00 the resolving revision had not arrived: {still_tied:?}"
    );
}

/// The same revision of the same text, written by two transports with different timestamp
/// formats, is one version — not a conflict.
///
/// A REST page sends `2026-10-15T14:00:00Z` where a stream sends
/// `2026-10-15T14:00:00.000000+00:00`. Those are the same instant and the same article. Hashing
/// the raw string made them two contents at one revision time, which is exactly the shape of a
/// genuine conflict, so a formatting difference reported the story as unanswerable.
#[test]
fn equivalent_timestamps_from_two_transports_are_one_version() {
    let t0 = ns("2026-10-15T13:00:00Z");
    let headline = "ABCD Announces FDA Approval";
    let rest = tagged(headline, Some("2026-10-15T14:00:00Z"), t0, &["ABCD"]);
    let stream = tagged(
        headline,
        Some("2026-10-15T14:00:00.000000+00:00"),
        t0 + H,
        &["ABCD"],
    );
    assert_ne!(
        rest.version_key, stream.version_key,
        "the raw keys do differ, which is why this needed fixing"
    );
    assert_eq!(
        rest.content_key(),
        stream.content_key(),
        "but the content is the same revision of the same text"
    );
    assert_eq!(rest.updated_at_ns, stream.updated_at_ns);

    let got = seen_at(&[rest.clone(), stream.clone()], t0 + 2 * H);
    assert_eq!(got.len(), 1);
    assert!(
        got[0].2,
        "one version, so nothing is in doubt: {got:?}"
    );
    assert_eq!(
        (got[0].1, got[0].3),
        (1, t0),
        "and it keeps the first sighting and the first ordinal: {got:?}"
    );
    let items = news_items(&[rest, stream], None);
    assert_eq!(
        items.iter().filter(|n| n.instrument == abcd()).count(),
        1,
        "the second delivery is not a second version: {items:?}"
    );
}

/// A redelivery separated from its original by a late older revision is still one version, at its
/// earliest observation.
///
/// This is what a forgotten cache entry produces: the collector no longer recognises a version it
/// stored long ago, so it appends it again. Collapsing only *adjacent* duplicates left two copies
/// of revision 11 — one at 09:00 and one at 12:00, with revision 08 between them — and the later
/// copy won the revision-order tiebreak. The story's current version then reported the 12:00
/// sighting, moving the observation time and the evidence identity of something already
/// published.
#[test]
fn a_redelivery_across_a_late_older_revision_keeps_its_first_observation() {
    let nine = ns("2026-10-15T13:00:00Z");
    let rev11 = tagged(
        "ABCD Announces FDA Approval Of Lead Candidate",
        Some("2026-10-15T14:11:00Z"),
        nine,
        &["ABCD"],
    );
    let rev08 = tagged(
        "ABCD Announces FDA Approval",
        Some("2026-10-15T14:08:00Z"),
        nine + 2 * H,
        &["ABCD"],
    );
    // The collector forgot rev11 and appended it again at 12:00.
    let rev11_again = NewsVersionRecord {
        first_observed_at: nine + 3 * H,
        ..rev11.clone()
    };
    let records = [rev11.clone(), rev08.clone(), rev11_again];

    let items: Vec<(u32, i64)> = news_items(&records, None)
        .into_iter()
        .filter(|n| n.instrument == abcd() && n.tag == Tag::Present)
        .map(|n| (n.version, n.observed_at))
        .collect();
    assert_eq!(
        items.len(),
        2,
        "two revisions, not three: the redelivery is the same version: {items:?}"
    );
    assert!(
        items.contains(&(1, nine)),
        "and revision 11 keeps its 09:00 sighting and its ordinal: {items:?}"
    );

    let after = seen_at(&records, nine + 4 * H);
    assert_eq!(after.len(), 1);
    assert_eq!(
        (after[0].1, after[0].3),
        (1, nine),
        "the current version is revision 11 as first observed, not its later copy: {after:?}"
    );
    assert!(after[0].2, "and nothing is ambiguous about it: {after:?}");

    // A genuine edit back to the earlier text is a new provider revision, so it stays distinct.
    let edited_back = tagged(
        "ABCD Announces FDA Approval",
        Some("2026-10-15T14:20:00Z"),
        nine + 5 * H,
        &["ABCD"],
    );
    let with_edit = [rev11, rev08, edited_back];
    let latest = seen_at(&with_edit, nine + 6 * H);
    assert_eq!(
        latest[0].3,
        nine + 5 * H,
        "an edit back is current from its own observation: {latest:?}"
    );
}

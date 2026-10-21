//! Float and news evidence end to end on recorded probe responses (research report 02,
//! `tests/fixtures/evidence/`): provider client → append-only archive records → loader → context
//! engine. No network and no credentials: the keys below are dummies.

use chrono::{DateTime, NaiveDate, TimeZone, Utc};
use market_archive::evidence_load::{float_observations, news_coverage, news_items};
use market_archive::{
    Archive, ArchiveConfig, CoverageScope, LiveQuote, LoadedEvidence, NewsVersionRecord, sha256_hex,
};
use market_events::session::et;
use market_events::{Price, Trade, UnixNanos};
use market_providers::import::alpaca::AlpacaCredentials;
use market_providers::import::alpaca_news::{
    NewsFetch, NewsQuery, NewsVersions, RawStory, coverage_record, fetch_news,
};
use market_providers::import::evidence_collect::{
    Collector, EvidenceConfig, FloatMode, news_repair_window, news_window, watch_universe,
};
use market_providers::import::fmp::{FmpError, FmpKey};
use market_providers::import::fmp_float::{fetch_float, fetch_float_all};
use market_providers::import::http::{Http, HttpResponse, RetryPolicy};
use serde_json::json;
use std::cell::Cell;
use std::collections::{BTreeSet, VecDeque};
use std::time::Duration;
use stock_context::{
    Coverage, Decision, Engine, Fidelity, PILLARS_V1, PillarId, PillarResult, Policy, RunInfo, Tri,
    Verdict,
};

const SEC: i64 = 1_000_000_000;
const H: i64 = 3_600 * SEC;
const FMP: &str = "https://fmp.test";
const DATA: &str = "https://data.test";

// --- scripted HTTP ----------------------------------------------------------------------------

#[derive(Default)]
struct Script {
    replies: VecDeque<Result<HttpResponse, String>>,
    urls: Vec<String>,
    headers: Vec<Vec<(String, String)>>,
}

impl Script {
    fn ok(mut self, body: Vec<u8>) -> Self {
        self.replies.push_back(Ok(HttpResponse {
            status: 200,
            body,
            retry_after: None,
        }));
        self
    }
    fn json(self, v: serde_json::Value) -> Self {
        self.ok(serde_json::to_vec(&v).unwrap())
    }
    fn status(mut self, status: u16, body: &[u8]) -> Self {
        self.replies.push_back(Ok(HttpResponse {
            status,
            body: body.to_vec(),
            retry_after: None,
        }));
        self
    }
}

impl Http for Script {
    fn get(&mut self, url: &str, headers: &[(&str, &str)]) -> Result<HttpResponse, String> {
        self.urls.push(url.to_string());
        self.headers.push(
            headers
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        );
        self.replies
            .pop_front()
            .expect("the script ran out of replies")
    }
}

fn fixture(name: &str) -> Vec<u8> {
    let path = format!(
        "{}/tests/fixtures/evidence/{name}",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

fn key() -> FmpKey {
    FmpKey("dummy-fmp-key-0001".into())
}

fn creds() -> AlpacaCredentials {
    AlpacaCredentials {
        key_id: "dummy-key-id-0001".into(),
        secret: "dummy-secret-0001".into(),
    }
}

fn archive() -> (tempfile::TempDir, Archive) {
    let dir = tempfile::tempdir().unwrap();
    let a = Archive::new(ArchiveConfig {
        root: dir.path().join("archive"),
        retention_days: 0,
        min_free_bytes: 0,
    });
    (dir, a)
}

fn d(s: &str) -> NaiveDate {
    NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
}

fn t(day: &str, h: u32, m: u32, s: u32) -> i64 {
    et(d(day), h, m, s, 0).unwrap().nanos()
}

fn utc(ns: i64) -> DateTime<Utc> {
    Utc.timestamp_nanos(ns)
}

// --- the context engine ---------------------------------------------------------------------

const VOL: Verdict = Verdict {
    ohlc: true,
    volume: true,
};

fn engine(policy: Policy) -> Engine {
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

/// v1 with a coverage lag: the shape a policy that lets polled coverage FAIL the pillar needs.
fn lag_policy(secs: i64) -> Policy {
    let text = PILLARS_V1.replacen(
        "\"float\": {",
        &format!("\"news\": {{ \"coverage_max_lag_secs\": {secs} }},\n  \"float\": {{"),
        1,
    );
    let p = Policy::parse(&text).unwrap();
    assert_eq!(p.body.news.coverage_max_lag_secs, secs);
    p
}

/// A print at `exec`, then the engine's clock 20 s on; the symbol's current decision.
fn decide(e: &mut Engine, sym: &str, exec: i64) -> Decision {
    e.apply(
        &Trade::new(
            sym,
            UnixNanos(exec),
            Price::from_micros(5_000_000),
            1_000,
            1,
        ),
        VOL,
        UnixNanos(exec + SEC / 10),
    );
    e.advance_to(UnixNanos(exec + 20 * SEC));
    e.current()
        .find(|x| x.symbol == sym)
        .expect("a decision")
        .clone()
}

fn pillar(dec: &Decision, p: PillarId) -> PillarResult {
    dec.pillars.iter().find(|x| x.pillar == p).unwrap().clone()
}

// --- float ----------------------------------------------------------------------------------

#[test]
fn a_float_retrieval_round_trips_from_the_recorded_probe() {
    let (_d, a) = archive();
    let body = fixture("fmp-shares-float-AAPL.json");
    let mut http = Script::default().ok(body.clone());
    let at = t("2026-10-19", 7, 0, 0);
    let rec = fetch_float(
        &mut http,
        FMP,
        &key(),
        "AAPL",
        RetryPolicy::default(),
        &mut || at,
        &mut |_| {},
        &mut |_| {},
    )
    .unwrap();
    assert_eq!(rec.float_shares, Some(14_569_078_010));
    assert_eq!(rec.outstanding_shares, Some(14_594_180_000));
    assert_eq!(rec.free_float_pct, Some(99.828));
    assert!(
        rec.source_url
            .as_deref()
            .unwrap()
            .starts_with("https://www.sec.gov/")
    );
    // FMP's `date` is its refresh time (F1): kept as delivered, never made an as-of date.
    assert_eq!(rec.provider_date.as_deref(), Some("2026-10-18 15:42:25"));
    assert_eq!(rec.effective_as_of, None);
    assert!(rec.quality.contains(&"no_effective_date".to_string()));
    assert_eq!(rec.retrieved_at, at);
    assert_eq!(rec.response_sha256, sha256_hex(&body));
    // The key goes out in the request and nowhere else.
    assert_eq!(rec.endpoint, "/stable/shares-float?symbol=AAPL");
    assert!(http.urls[0].contains("apikey=dummy-fmp-key-0001"));
    a.append_float(std::slice::from_ref(&rec)).unwrap();
    let back = a.read_float().unwrap();
    assert_eq!(back, vec![rec.clone()]);
    assert!(
        !serde_json::to_string(&back)
            .unwrap()
            .contains("dummy-fmp-key")
    );

    let obs = float_observations(&back);
    assert_eq!(obs.len(), 1);
    assert_eq!(obs[0].id, "float:fmp:AAPL");
    assert_eq!(obs[0].version, 1);
    assert_eq!(obs[0].shares, 14_569_078_010);
    assert_eq!(
        obs[0].observed_at, at,
        "freshness counts from our retrieval"
    );
}

#[test]
fn shares_outstanding_is_never_used_as_float() {
    // SGRX: floatShares == outstandingShares (799,914), freeFloat null.
    let mut http = Script::default().ok(fixture("fmp-shares-float-SGRX.json"));
    let at = t("2026-10-19", 7, 0, 0);
    let rec = fetch_float(
        &mut http,
        FMP,
        &key(),
        "SGRX",
        RetryPolicy::default(),
        &mut || at,
        &mut |_| {},
        &mut |_| {},
    )
    .unwrap();
    assert!(
        rec.quality
            .contains(&"float_equals_outstanding".to_string())
    );
    assert!(rec.quality.contains(&"free_float_missing".to_string()));
    assert!(float_observations(std::slice::from_ref(&rec)).is_empty());

    // The engine therefore has no float for SGRX, rather than 799,914 "float" shares.
    let mut e = engine(Policy::v1());
    for o in float_observations(&[rec]) {
        e.add_float(o);
    }
    let dec = decide(&mut e, "SGRX", t("2026-10-19", 9, 31, 0));
    let f = pillar(&dec, PillarId::Float);
    assert_eq!((f.state, f.reason), (Tri::Unknown, "no_float_evidence"));

    // A provider row with no outstanding count at all is not "float = outstanding" either way;
    // a zero float is kept (the engine reports it as nonpositive) and never replaced.
    let mut http = Script::default()
        .ok(fixture("fmp-shares-float-all-p0.json"))
        .json(serde_json::json!([]));
    let want = |s: &str| matches!(s, "A" | "AA" | "AAA");
    let (rows, requests) = fetch_float_all(
        &mut http,
        FMP,
        &key(),
        &want,
        RetryPolicy::default(),
        &mut || at,
        &mut |_| {},
        &mut |_| {},
    )
    .unwrap();
    assert_eq!(requests, 2, "stops at the first empty page");
    let syms: Vec<_> = rows.iter().map(|r| r.symbol.as_str()).collect();
    assert_eq!(syms, ["A", "AA", "AAA"]);
    let obs = float_observations(&rows);
    let aaa = obs.iter().find(|o| o.id == "float:fmp:AAA").unwrap();
    assert_eq!(aaa.shares, 0, "1,800,263 outstanding is not substituted");
}

#[test]
fn float_is_forward_only_and_goes_stale() {
    let at = t("2026-10-15", 7, 0, 0);
    let mut http = Script::default().json(serde_json::json!([{
        "symbol": "ABCD", "date": "2026-10-15 10:58:00", "freeFloat": 80.0,
        "floatShares": 8_000_000, "outstandingShares": 10_000_000,
        "source": "https://www.sec.gov/Archives/edgar/data/1/abcd-10q.htm"
    }]));
    let rec = fetch_float(
        &mut http,
        FMP,
        &key(),
        "ABCD",
        RetryPolicy::default(),
        &mut || at,
        &mut |_| {},
        &mut |_| {},
    )
    .unwrap();
    let mut e = engine(Policy::v1());
    for o in float_observations(&[rec]) {
        e.add_float(o);
    }
    // The day before the retrieval: today's float never reaches an older episode.
    let f = pillar(
        &decide(&mut e, "ABCD", t("2026-10-14", 9, 31, 0)),
        PillarId::Float,
    );
    assert_eq!((f.state, f.reason), (Tri::Unknown, "no_float_evidence"));
    let f = pillar(
        &decide(&mut e, "ABCD", t("2026-10-15", 9, 31, 0)),
        PillarId::Float,
    );
    assert_eq!((f.state, f.reason), (Tri::Pass, "below_max"));
    assert_eq!(f.as_of, Some(at));
    // 24 h after the retrieval with no newer one: stale, not still true.
    let f = pillar(
        &decide(&mut e, "ABCD", t("2026-10-16", 9, 31, 0)),
        PillarId::Float,
    );
    assert_eq!((f.state, f.reason), (Tri::Unknown, "float_stale"));
}

#[test]
fn a_plan_refusal_is_reported_as_not_entitled() {
    let mut http = Script::default().status(402, &fixture("fmp-402-premium-query.txt"));
    let err = fetch_float(
        &mut http,
        FMP,
        &key(),
        "AAPL",
        RetryPolicy::default(),
        &mut || 0,
        &mut |_| {},
        &mut |_| {},
    )
    .unwrap_err();
    assert!(matches!(err, FmpError::NotEntitled { status: 402, .. }));
    // The legacy historical-float route answers 200-style JSON with an "Error Message".
    let mut http = Script::default().ok(fixture("fmp-v4-historical-shares-float-SGRX.json"));
    let err = fetch_float(
        &mut http,
        FMP,
        &key(),
        "SGRX",
        RetryPolicy::default(),
        &mut || 0,
        &mut |_| {},
        &mut |_| {},
    )
    .unwrap_err();
    assert!(matches!(err, FmpError::NotEntitled { .. }));
}

// --- news -----------------------------------------------------------------------------------

fn page_2023() -> serde_json::Value {
    serde_json::from_slice(&fixture("alp-news-AAPL-2023.json")).unwrap()
}

fn empty_page() -> serde_json::Value {
    serde_json::json!({"news": [], "next_page_token": null})
}

fn poll(
    http: &mut Script,
    q: &NewsQuery,
    at: i64,
) -> market_providers::import::alpaca_news::NewsFetch {
    fetch_news(
        http,
        DATA,
        &creds(),
        q,
        RetryPolicy::default(),
        &mut || at,
        &mut |_| {},
        &mut |_| {},
    )
}

fn query(start: i64, end: i64) -> NewsQuery {
    NewsQuery {
        start: utc(start),
        end: utc(end),
        symbols: None,
    }
}

#[test]
fn a_later_edit_is_a_new_version_that_an_earlier_cutoff_never_sees() {
    let (_d, a) = archive();
    let day = "2023-10-18";
    let q = query(t(day, 0, 0, 0), t(day, 9, 24, 30));
    let mut versions = NewsVersions::default();

    // Poll 1 at 09:25 ET: ten stories on page one, an empty second page (cursor followed).
    let t1 = t(day, 9, 25, 0);
    let mut http = Script::default().json(page_2023()).json(empty_page());
    let f = poll(&mut http, &q, t1);
    assert!(f.complete);
    assert_eq!(f.requests, 2);
    assert!(http.urls[1].contains("page_token=MTY5NzYzNTIxNDAwMDAwMDAwMHwzNTMwODY0MQ%3D%3D"));
    assert!(http.urls[0].contains("include_content=false"));
    assert!(
        !http.urls[0].contains("symbols="),
        "one query for every symbol"
    );
    let first = versions.propose(&f.stories);
    assert_eq!(first.len(), 10);
    a.append_news(&first).unwrap();
    versions.commit(&first);

    // Poll 2 at 09:26: the same stories again add nothing.
    let mut http = Script::default().json(page_2023()).json(empty_page());
    let f = poll(&mut http, &q, t(day, 9, 26, 0));
    assert!(versions.propose(&f.stories).is_empty());

    // Poll 3 at 11:00: story 35303347 was edited (new headline, later updated_at).
    let mut edited = page_2023();
    let story = edited["news"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|s| s["id"] == 35303347)
        .unwrap();
    story["headline"] = "Apple Supplier TSMC Signs Supply Agreement With Apple".into();
    story["updated_at"] = "2023-10-18T14:55:00Z".into();
    let t3 = t(day, 11, 0, 0);
    let mut http = Script::default().json(edited).json(empty_page());
    let f = poll(&mut http, &q, t3);
    let edit = versions.propose(&f.stories);
    assert_eq!(edit.len(), 1);
    assert_eq!(edit[0].article_id, "35303347");
    assert_eq!(edit[0].first_observed_at, t3);
    a.append_news(&edit).unwrap();
    versions.commit(&edit);

    let stored = a.read_news(None).unwrap();
    assert_eq!(stored.len(), 11);
    // No body or summary text is stored: hashes only.
    let text = serde_json::to_string(&stored).unwrap();
    assert!(
        !text.contains("protest by residents"),
        "summary text must not be stored"
    );
    assert!(!text.contains("dummy-"), "credentials never reach a record");

    let items = news_items(&stored, None);
    let tsmc: Vec<_> = items
        .iter()
        .filter(|n| n.id == "news:alpaca:35303347")
        .collect();
    assert_eq!(tsmc.len(), 2);
    let (v1, v2) = (tsmc[0], tsmc[1]);
    assert_eq!((v1.version, v1.observed_at), (1, t1));
    assert_eq!((v2.version, v2.observed_at), (2, t3));
    // Publication time is the story's created_at for both; visibility is first sighting.
    let created = DateTime::parse_from_rfc3339("2023-10-18T09:13:42Z")
        .unwrap()
        .timestamp_nanos_opt();
    assert_eq!(v1.published_at, created);
    assert_eq!(v2.published_at, created);
    assert_eq!((v1.event_type.as_str(), v1.substantive), ("other", false));
    assert_eq!(
        (v2.event_type.as_str(), v2.substantive),
        ("commercial_agreement", true)
    );
    assert_eq!(
        v2.authenticity,
        stock_context::Authenticity::ReportedUnverified
    );
    assert_eq!(v2.favorability, stock_context::Favorability::Undetermined);

    let mut e = engine(Policy::v1());
    for n in items {
        e.add_news(n);
    }
    let before = pillar(&decide(&mut e, "AAPL", t(day, 10, 0, 0)), PillarId::News);
    assert_ne!(before.state, Tri::Pass, "the edit is not visible at 10:00");
    let after = pillar(&decide(&mut e, "AAPL", t(day, 11, 5, 0)), PillarId::News);
    assert_eq!(
        (after.state, after.reason),
        (Tri::Pass, "fresh_relevant_news")
    );
    assert_eq!(after.evidence, vec!["news:alpaca:35303347@2".to_string()]);
}

#[test]
fn coverage_fails_the_pillar_only_inside_the_searched_window() {
    let (_d, a) = archive();
    let day = "2025-11-25";
    let now = t(day, 14, 30, 0);
    let cfg = EvidenceConfig::default();
    let (start, end) = news_window(&cfg, None, now);
    assert_eq!(end, now - 30 * SEC);
    assert_eq!(
        start,
        end - 25 * H,
        "the first search covers the whole 24 h window"
    );
    let q = query(start, end);
    let mut http = Script::default().ok(fixture("alp-news-AAPL-2025-11-25.json"));
    let f = poll(&mut http, &q, now);
    assert!(f.complete);
    let cov = coverage_record(&q, &f).unwrap();
    assert_eq!(cov.scope, CoverageScope::All);
    assert_eq!(
        (cov.covered_from, cov.covered_through, cov.observed_at),
        (start, end, now)
    );
    a.append_news(&NewsVersions::default().propose(&f.stories))
        .unwrap();
    a.append_news_coverage(std::slice::from_ref(&cov)).unwrap();

    // A search that failed on its second page: its stories are kept, its window is not claimed.
    let mut http = Script::default()
        .json(serde_json::json!({"news": [], "next_page_token": "abc"}))
        .status(400, b"bad request");
    let partial = poll(&mut http, &query(end, end + 60 * SEC), now + 60 * SEC);
    assert!(!partial.complete);
    assert!(coverage_record(&query(end, end + 60 * SEC), &partial).is_none());

    let syms: BTreeSet<String> = ["ZZZZ".to_string(), "AAPL".to_string()].into();
    let loaded = LoadedEvidence::load(&a, &syms, None, 0).unwrap();
    assert_eq!(loaded.coverage.len(), 2, "one per symbol");

    // pillars-v1 (no lag): an honest, already-past window never reaches the cutoff, so a
    // forward run stays UNKNOWN rather than claiming a search it did not make.
    let mut e = engine(Policy::v1());
    loaded.clone().apply(&mut e);
    let n = pillar(&decide(&mut e, "ZZZZ", now + 5 * SEC), PillarId::News);
    assert_eq!((n.state, n.reason), (Tri::Unknown, "news_coverage_unknown"));

    // With a 120 s lag: FAIL inside the searched window (plus lag), UNKNOWN before the search
    // completed and after its end has aged past the lag with no newer search.
    let mut e = engine(lag_policy(120));
    loaded.apply(&mut e);
    let n = pillar(&decide(&mut e, "ZZZZ", now - 60 * SEC), PillarId::News);
    assert_eq!((n.state, n.reason), (Tri::Unknown, "news_coverage_unknown"));
    let n = pillar(&decide(&mut e, "ZZZZ", now + 5 * SEC), PillarId::News);
    assert_eq!(
        (n.state, n.reason),
        (Tri::Fail, "no_matching_news_in_covered_sources")
    );
    assert_eq!(n.evidence, vec![cov.id.clone()]);
    let n = pillar(&decide(&mut e, "ZZZZ", now + 300 * SEC), PillarId::News);
    assert_eq!((n.state, n.reason), (Tri::Unknown, "news_coverage_unknown"));

    // A symbol-scoped search covers only its symbols.
    let scoped = market_archive::NewsCoverageRecord {
        scope: CoverageScope::Symbols(vec!["AAPL".into()]),
        ..cov.clone()
    };
    let c = news_coverage(&[scoped], &["ZZZZ".into(), "AAPL".into()], 0);
    assert_eq!(c.len(), 1);
    assert_eq!(c[0].instrument, stock_context::InstrumentId::symbol("AAPL"));
}

#[test]
fn coverage_runs_join_overlapping_polls_and_thinning_only_shortens_them() {
    let cfg = EvidenceConfig::default();
    let base = t("2026-10-15", 9, 0, 0);
    let mut recs = Vec::new();
    let mut last = None;
    for i in 0..5 {
        let now = base + i * 60 * SEC;
        let (s, e) = news_window(&cfg, last, now);
        last = Some(e);
        recs.push(market_archive::NewsCoverageRecord {
            v: 1,
            id: format!("c{i}"),
            provider: "alpaca".into(),
            sources: vec!["benzinga".into()],
            scope: CoverageScope::All,
            covered_from: s,
            covered_through: e,
            observed_at: now + SEC,
            requests: 1,
            stories: 0,
        });
    }
    let all = news_coverage(&recs, &["X".into()], 0);
    assert_eq!(all.len(), 5);
    assert!(
        all.iter().all(|c| c.covered_from == recs[0].covered_from),
        "one run"
    );
    assert_eq!(all[4].covered_through, recs[4].covered_through);
    let thin = news_coverage(&recs, &["X".into()], 150);
    assert_eq!(
        thin.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(),
        ["c0", "c3"]
    );
    // A gap longer than the catch-up limit starts a new run instead of bridging it.
    let (s, _) = news_window(&cfg, last, base + 4 * 86_400 * SEC);
    assert!(s > last.unwrap());
    // Market-wide: the same runs once, for every instrument; a named-symbol search is not in them.
    let mut with_scoped = recs.clone();
    with_scoped.push(market_archive::NewsCoverageRecord {
        id: "named".into(),
        scope: CoverageScope::Symbols(vec!["AAPL".into()]),
        ..recs[4].clone()
    });
    let market = market_archive::evidence_load::market_news_coverage(&with_scoped, 0);
    assert_eq!(market.len(), 5);
    for (m, x) in market.iter().zip(&all) {
        assert_eq!(m.instrument, stock_context::InstrumentId::market());
        assert_eq!(
            (&m.id, m.covered_from, m.covered_through, m.observed_at),
            (&x.id, x.covered_from, x.covered_through, x.observed_at)
        );
    }
    assert_eq!(
        market_archive::evidence_load::scoped_symbols(&with_scoped),
        BTreeSet::from(["AAPL".to_string()])
    );
}

#[test]
fn duplicates_and_recycled_stories_are_kept_as_history() {
    let story = |id: u64, headline: &str, updated: &str| -> RawStory {
        serde_json::from_value(serde_json::json!({
            "id": id, "headline": headline, "author": "x", "created_at": "2026-10-15T11:00:00Z",
            "updated_at": updated, "summary": "s", "symbols": ["ABCD"], "source": "benzinga",
            "url": format!("https://example.test/{id}")
        }))
        .unwrap()
    };
    let h = "ABCD Announces FDA Approval Of Lead Candidate";
    let t0 = t("2026-10-15", 7, 0, 0);
    let mut v = NewsVersions::default();
    let mut stored: Vec<NewsVersionRecord> =
        v.propose(&[(story(1, h, "2026-10-15T11:00:00Z"), t0)]);
    v.commit(&stored);
    // The identical story again: not a new version.
    assert!(
        v.propose(&[(story(1, h, "2026-10-15T11:00:00Z"), t0 + H)])
            .is_empty()
    );
    // The same headline under a new id (a recycled story): its own item, kept.
    let recycled = v.propose(&[(story(2, h, "2026-10-15T12:00:00Z"), t0 + 2 * H)]);
    v.commit(&recycled);
    stored.extend(recycled);
    assert_eq!(stored.len(), 2);
    // A record written twice (a restart that lost the in-memory index) collapses in the loader,
    // keeping the first sighting.
    let mut dup = stored[0].clone();
    dup.first_observed_at = t0 + 3 * H;
    stored.push(dup);
    let items = news_items(&stored, None);
    let ids: Vec<_> = items
        .iter()
        .map(|n| (n.id.as_str(), n.version, n.observed_at))
        .collect();
    assert_eq!(
        ids,
        [("news:alpaca:1", 1, t0), ("news:alpaca:2", 1, t0 + 2 * H)]
    );
    assert!(
        items
            .iter()
            .all(|n| n.event_type == "regulatory_decision" && n.substantive)
    );
}

#[test]
fn roundups_are_not_relevant_and_screens_are_not_substantive() {
    let page = page_2023();
    let stories: Vec<RawStory> = serde_json::from_value(page["news"].clone()).unwrap();
    let recs = NewsVersions::default().propose(
        &stories
            .into_iter()
            .map(|s| (s, t("2023-10-18", 9, 25, 0)))
            .collect::<Vec<_>>(),
    );
    let only: BTreeSet<String> = ["AAPL".to_string()].into();
    let items = news_items(&recs, Some(&only));
    let wsb = items
        .iter()
        .find(|n| n.id == "news:alpaca:35308592")
        .unwrap();
    assert!(
        !wsb.relevant,
        "ten symbols and AAPL not in the headline: a roundup"
    );
    let watch = items
        .iter()
        .find(|n| n.id == "news:alpaca:35302515")
        .unwrap();
    assert!(watch.relevant && watch.credible_source);
}

// --- the collector ----------------------------------------------------------------------------

fn test_cfg() -> EvidenceConfig {
    EvidenceConfig {
        data_base: DATA.into(),
        fmp_base: FMP.into(),
        // The scripted replies are per-symbol `shares-float` answers; bulk (the default) has its
        // own fixtures above.
        float_mode: FloatMode::PerSymbol,
        ..EvidenceConfig::default()
    }
}

#[test]
fn the_collector_is_bounded_per_day_and_resumes_from_its_records() {
    let (_d, a) = archive();
    let day = d("2026-10-15");
    let mut closes = market_archive::DailyCloses::new();
    closes.insert(
        d("2026-10-14"),
        [
            ("ABCD".to_string(), 5.0),
            ("BIGG".to_string(), 50.0),
            ("PENY".to_string(), 0.05),
        ]
        .into(),
    );
    a.write_daily_closes(&closes).unwrap();
    assert_eq!(
        watch_universe(&closes, market_events::PriceBand::DEFAULT, day, 5),
        BTreeSet::from(["ABCD".to_string()])
    );
    std::fs::create_dir_all(a.live_dir(day)).unwrap();
    let quote = |s: &str, px: i64| LiveQuote {
        symbol: s.into(),
        day,
        last: Some(Price::from_micros(px)),
        last_ts: 0,
        first: None,
        open: None,
        volume: 1,
        trades: 1,
    };
    assert!(
        a.write_live_quotes(day, &[quote("ABCD", 5_000_000), quote("NEWW", 3_000_000)])
            .unwrap()
    );

    let float_row = |s: &str| {
        serde_json::json!([{"symbol": s, "floatShares": 1_000_000,
        "outstandingShares": 2_000_000, "freeFloat": 50.0, "source": "https://x.test", "date": "d"}])
    };
    let clock = Cell::new(utc(t("2026-10-15", 7, 5, 0)));
    let tick = |c: &mut Collector, http: &mut Script| {
        c.tick(
            &a,
            http,
            Some(&key()),
            Some(&creds()),
            &mut || clock.get(),
            &mut |_: Duration| {},
            &mut |_: &str| {},
        )
    };

    // Tick 1: the entrant first, then the universe; one forward news search.
    let mut c = Collector::open(test_cfg(), &a, clock.get()).unwrap();
    let mut http = Script::default()
        .json(float_row("NEWW"))
        .json(float_row("ABCD"))
        .json(empty_page());
    let r = tick(&mut c, &mut http);
    assert_eq!(r.errors, Vec::<String>::new());
    assert_eq!((r.float_requests, r.float_records, r.warmups), (2, 2, 1));
    assert_eq!((r.news_requests, r.news_complete), (1, 1));
    assert!(http.urls[0].contains("symbol=NEWW"));
    assert!(http.urls[1].contains("symbol=ABCD"));
    assert!(http.urls[2].starts_with("https://data.test/v1beta1/news?"));
    assert!(http.headers[2].iter().any(|(k, _)| k == "APCA-API-KEY-ID"));

    // Tick 2, 15 s later: nothing is due.
    clock.set(clock.get() + chrono::Duration::seconds(15));
    let mut http = Script::default();
    let r = tick(&mut c, &mut http);
    assert_eq!((r.float_requests, r.news_requests), (0, 0));

    // A restart: today's floats are not fetched again; news resumes from the stored coverage.
    // Three searches, each one answering a different question: the forward window for what is
    // new, the trailing edit window for recent changes, and the repair window for a change to
    // something older that neither of those would ask about.
    clock.set(clock.get() + chrono::Duration::seconds(60));
    let mut c = Collector::open(test_cfg(), &a, clock.get()).unwrap();
    let mut http = Script::default()
        .json(empty_page())
        .json(empty_page())
        .json(empty_page());
    let r = tick(&mut c, &mut http);
    assert_eq!(r.float_requests, 0);
    assert_eq!((r.news_requests, r.news_complete), (3, 3));
    assert_eq!((r.news_repairs, r.news_repair_incomplete), (1, 0));
    let cov = a.read_news_coverage(None).unwrap();
    assert_eq!(cov.len(), 4);
    assert!(
        cov[1].covered_from <= cov[0].covered_through,
        "the forward search overlaps"
    );
    // The repair search really read the whole horizon, not a window near now.
    let widest = cov
        .iter()
        .max_by_key(|c| c.covered_through - c.covered_from)
        .expect("a coverage record");
    assert!(
        widest.covered_through - widest.covered_from >= 24 * H,
        "the repair covers the freshness horizon: {}h",
        (widest.covered_through - widest.covered_from) / H
    );
    assert_eq!(a.read_float().unwrap().len(), 2);
}

#[test]
fn a_float_plan_refusal_pauses_float_for_the_day() {
    let (_d, a) = archive();
    let day = d("2026-10-15");
    let mut closes = market_archive::DailyCloses::new();
    closes.insert(
        d("2026-10-14"),
        [("AAAA".to_string(), 5.0), ("BBBB".to_string(), 6.0)].into(),
    );
    a.write_daily_closes(&closes).unwrap();
    let clock = Cell::new(utc(t("2026-10-15", 7, 5, 0)));
    let mut c = Collector::open(test_cfg(), &a, clock.get()).unwrap();
    let mut http = Script::default().status(402, &fixture("fmp-402-premium-query.txt"));
    let r = c.tick(
        &a,
        &mut http,
        Some(&key()),
        None,
        &mut || clock.get(),
        &mut |_| {},
        &mut |_| {},
    );
    assert_eq!(
        r.float_requests, 1,
        "BBBB is not asked after AAAA was refused"
    );
    assert_eq!(r.float_queued, 0);
    assert!(r.errors[0].contains("not entitled"));
    assert!(!r.errors[0].contains("dummy-fmp-key"));
    let _ = day;
}

// --- float completion and transient retry -----------------------------------------------------

/// `AAAA` in the watch universe, nothing else.
fn one_symbol_universe(a: &Archive, day: NaiveDate) {
    let mut closes = market_archive::DailyCloses::new();
    closes.insert(day.pred_opt().unwrap(), [("AAAA".to_string(), 5.0)].into());
    a.write_daily_closes(&closes).unwrap();
}

fn float_body(sym: &str) -> serde_json::Value {
    serde_json::json!([{"symbol": sym, "floatShares": 1_000_000,
    "outstandingShares": 2_000_000, "freeFloat": 50.0, "source": "https://x.test", "date": "d"}])
}

/// Five transient 500s: enough for `RetryPolicy::default()` to exhaust its attempts.
fn five_transient() -> Script {
    let mut s = Script::default();
    for _ in 0..5 {
        s = s.status(500, b"upstream hiccup");
    }
    s
}

/// A collector whose transient retries are quick and few, so a test can walk them.
fn retry_cfg(attempts: u32) -> EvidenceConfig {
    EvidenceConfig {
        float_retry_attempts: attempts,
        float_retry_base_secs: 10,
        float_retry_max_secs: 60,
        ..test_cfg()
    }
}

/// One `Collector::tick` with no Alpaca credentials (float only).
fn float_tick(
    c: &mut Collector,
    a: &Archive,
    http: &mut Script,
    clock: &Cell<DateTime<Utc>>,
) -> market_providers::import::evidence_collect::TickReport {
    c.tick(
        a,
        http,
        Some(&key()),
        None,
        &mut || clock.get(),
        &mut |_| {},
        &mut |_| {},
    )
}

/// A transient provider failure must not end the symbol's day: it is scheduled for a bounded
/// retry and, crucially, never recorded as done. Inserting an exhausted retry into `float_done`
/// (and marking rows done before `append_float`) was the audit's F12 — one hiccup hid a
/// qualifying symbol's float until the next ET date or a collector restart.
#[test]
fn a_transient_float_failure_is_retried_rather_than_marked_done() {
    let (_d, a) = archive();
    let day = d("2026-10-15");
    one_symbol_universe(&a, day);
    let clock = Cell::new(utc(t("2026-10-15", 7, 5, 0)));
    let mut c = Collector::open(retry_cfg(4), &a, clock.get()).unwrap();

    let r = float_tick(&mut c, &a, &mut five_transient(), &clock);
    assert_eq!(r.float_records, 0, "nothing was stored");
    assert_eq!(r.float_retries_scheduled, 1, "{r:?}");
    assert_eq!(r.float_retry_pending, 1);
    assert_eq!(r.float_retry_exhausted, 0);
    assert!(r.errors[0].contains("AAAA"), "{:?}", r.errors);
    assert!(a.read_float().unwrap().is_empty());

    // Too early: the backoff has not elapsed, so no request is made at all.
    clock.set(clock.get() + chrono::Duration::seconds(1));
    let r = float_tick(&mut c, &a, &mut Script::default(), &clock);
    assert_eq!((r.float_requests, r.float_retries_due), (0, 0), "{r:?}");
    assert_eq!(r.float_retry_pending, 1);

    // Past the backoff (base 10 s plus deterministic jitter under a quarter of it): the retry
    // runs and succeeds, and only then is the symbol done.
    clock.set(clock.get() + chrono::Duration::seconds(60));
    let r = float_tick(
        &mut c,
        &a,
        &mut Script::default().json(float_body("AAAA")),
        &clock,
    );
    assert_eq!((r.float_requests, r.float_records), (1, 1), "{r:?}");
    assert_eq!(r.float_retry_pending, 0);
    assert_eq!(a.read_float().unwrap().len(), 1);

    // And it is not asked a second time that day.
    clock.set(clock.get() + chrono::Duration::seconds(60));
    let r = float_tick(&mut c, &a, &mut Script::default(), &clock);
    assert_eq!(r.float_requests, 0, "{r:?}");
}

/// A fetch that succeeded but could not be stored is not complete either: completion is the
/// durable append, not the HTTP 200. The old code marked the row done before `append_float`, so
/// a disk error lost the observation silently.
#[test]
fn a_float_is_complete_only_once_the_append_succeeded() {
    let (_d, a) = archive();
    let day = d("2026-10-15");
    one_symbol_universe(&a, day);
    let clock = Cell::new(utc(t("2026-10-15", 7, 5, 0)));
    let mut c = Collector::open(retry_cfg(4), &a, clock.get()).unwrap();
    // Block the append: a directory where the month's file has to be opened.
    let month = a
        .evidence_dir()
        .join(market_archive::evidence::FLOAT_DIR)
        .join("2026-10.jsonl");
    std::fs::create_dir_all(&month).unwrap();
    let r = float_tick(
        &mut c,
        &a,
        &mut Script::default().json(float_body("AAAA")),
        &clock,
    );
    assert_eq!((r.float_requests, r.float_records), (1, 0), "{r:?}");
    assert!(
        r.errors.iter().any(|e| e.contains("float store")),
        "{:?}",
        r.errors
    );
    assert_eq!(r.float_retries_scheduled, 1, "the row is not durable yet");

    // The disk recovers; the retry re-fetches and stores it, with no duplicate and no loss.
    std::fs::remove_dir_all(&month).unwrap();
    clock.set(clock.get() + chrono::Duration::seconds(60));
    let r = float_tick(
        &mut c,
        &a,
        &mut Script::default().json(float_body("AAAA")),
        &clock,
    );
    assert_eq!((r.float_requests, r.float_records), (1, 1), "{r:?}");
    let rows = a.read_float().unwrap();
    assert_eq!(rows.len(), 1, "exactly one observation: {rows:?}");
    assert_eq!(rows[0].symbol, "AAAA");
}

/// Retries are bounded: a symbol the provider keeps failing costs a fixed number of attempts a
/// day, never a loop, and the exhaustion is counted rather than hidden.
#[test]
fn transient_float_retries_are_capped_per_symbol_per_day() {
    let (_d, a) = archive();
    let day = d("2026-10-15");
    one_symbol_universe(&a, day);
    let clock = Cell::new(utc(t("2026-10-15", 7, 5, 0)));
    let mut c = Collector::open(retry_cfg(2), &a, clock.get()).unwrap();
    let mut exhausted = 0;
    let mut requests = 0;
    for _ in 0..5 {
        let r = float_tick(&mut c, &a, &mut five_transient(), &clock);
        requests += r.float_requests;
        exhausted += r.float_retry_exhausted;
        clock.set(clock.get() + chrono::Duration::seconds(300));
    }
    assert_eq!(
        requests, 3,
        "the first attempt plus two capped retries, then nothing"
    );
    assert_eq!(exhausted, 1);
    assert!(a.read_float().unwrap().is_empty());
}

/// A plan refusal is not transient: the day's float work pauses, and the pending retries go with
/// it rather than hammering a provider that has already said no.
#[test]
fn a_plan_refusal_clears_pending_float_retries() {
    let (_d, a) = archive();
    let day = d("2026-10-15");
    let mut closes = market_archive::DailyCloses::new();
    closes.insert(
        day.pred_opt().unwrap(),
        [("AAAA".to_string(), 5.0), ("BBBB".to_string(), 6.0)].into(),
    );
    a.write_daily_closes(&closes).unwrap();
    let clock = Cell::new(utc(t("2026-10-15", 7, 5, 0)));
    let mut c = Collector::open(retry_cfg(4), &a, clock.get()).unwrap();

    // AAAA fails transiently (scheduled for retry), then BBBB is refused by the plan.
    let mut http = five_transient().status(402, &fixture("fmp-402-premium-query.txt"));
    let r = float_tick(&mut c, &a, &mut http, &clock);
    assert_eq!(r.float_retries_scheduled, 1, "{r:?}");
    assert!(r.errors.iter().any(|e| e.contains("not entitled")));
    assert_eq!(r.float_retry_pending, 0, "the refusal cleared them: {r:?}");
    assert_eq!(r.float_queued, 0);

    clock.set(clock.get() + chrono::Duration::seconds(600));
    let r = float_tick(&mut c, &a, &mut Script::default(), &clock);
    assert_eq!(r.float_requests, 0, "paused until tomorrow: {r:?}");
}

/// A news append that fails leaves nothing believed stored, and no coverage is claimed for the
/// window either.
///
/// This was the defect: the version index was updated as the records were handed to the caller,
/// before the append. A failed append left the version marked in memory and absent from disk, so
/// the next search suppressed the story as already known — and then wrote a coverage record
/// saying the window had been searched. The story was permanently missing while coverage claimed
/// it had been looked for, which reads as a confident "no qualifying news" rather than as an
/// unknown. The lower-bound argument does not help here: a false FAIL is not a missing PASS.
#[test]
fn a_failed_news_append_leaves_nothing_seen_and_nothing_covered() {
    let day = "2026-10-15";
    let at = t(day, 9, 25, 0);
    let story = |id: u64, headline: &str| -> RawStory {
        serde_json::from_value(json!({
            "id": id,
            "headline": headline,
            "summary": "",
            "author": "Staff",
            "created_at": "2026-10-15T13:00:00Z",
            "updated_at": "2026-10-15T13:00:00Z",
            "symbols": ["ABCD"],
            "source": "benzinga",
            "url": "https://example.invalid/1"
        }))
        .unwrap()
    };
    let stories = vec![(story(1, "ABCD Announces FDA Approval"), at)];

    let mut v = NewsVersions::default();
    let proposed = v.propose(&stories);
    assert_eq!(proposed.len(), 1, "a new story is offered");
    assert!(
        v.is_empty(),
        "and proposing it changed nothing: nothing is durable until it is stored"
    );

    // The append fails. The next search must offer the same version again.
    let again = v.propose(&stories);
    assert_eq!(
        again, proposed,
        "the same version is offered again, with its original first sighting"
    );
    assert_eq!(again[0].first_observed_at, at);

    // Once it is really stored, it stops being offered.
    v.commit(&proposed);
    assert_eq!(v.len(), 1);
    assert!(
        v.propose(&stories).is_empty(),
        "a durable version is not written twice"
    );
}

/// A failed append and the real archive: the window's coverage is not claimed, so the news pillar
/// stays UNKNOWN rather than reading as a searched window with no news.
#[test]
fn a_store_failure_does_not_claim_the_window() {
    let (_tmp, a) = archive();

    // A news directory that cannot be written into: a file where the directory belongs.
    let dir = a.evidence_dir().join(market_archive::evidence::NEWS_DIR);
    std::fs::create_dir_all(dir.parent().unwrap()).unwrap();
    std::fs::write(&dir, b"not a directory").unwrap();

    let at = t("2026-10-15", 9, 25, 0);
    let s: RawStory = serde_json::from_value(json!({
        "id": 7,
        "headline": "ABCD Announces FDA Approval",
        "summary": "",
        "author": "Staff",
        "created_at": "2026-10-15T13:00:00Z",
        "updated_at": "2026-10-15T13:00:00Z",
        "symbols": ["ABCD"],
        "source": "benzinga",
        "url": "https://example.invalid/7"
    }))
    .unwrap();
    let v = NewsVersions::default();
    let proposed = v.propose(&[(s, at)]);
    assert_eq!(proposed.len(), 1);
    assert!(
        a.append_news(&proposed).is_err(),
        "the fixture really made the append fail"
    );
    // The collector's rule: on a failed append it does not commit and does not claim coverage.
    assert!(v.is_empty(), "nothing became durable");
    assert_eq!(
        a.read_news_coverage(None).map(|c| c.len()).unwrap_or(0),
        0,
        "and no coverage record says the window was searched"
    );
}

/// A redelivery of an *older* version through another transport is not a new revision.
///
/// Keeping only the latest key meant a story that went v1 → v2 and was then redelivered as v1 — a
/// REST page behind a stream, or the reverse — did not match, so a fresh record was written for
/// v1 with a new `first_observed_at`. The story then reads as having changed back when nothing
/// happened but a transport toggle.
#[test]
fn a_redelivered_older_version_is_not_a_new_revision() {
    let at = t("2026-10-15", 9, 0, 0);
    let story = |headline: &str, updated: &str| -> RawStory {
        serde_json::from_value(json!({
            "id": 11,
            "headline": headline,
            "summary": "",
            "author": "Staff",
            "created_at": "2026-10-15T13:00:00Z",
            "updated_at": updated,
            "symbols": ["ABCD"],
            "source": "benzinga",
            "url": "https://example.invalid/11"
        }))
        .unwrap()
    };
    let v1 = story("ABCD Announces FDA Approval", "2026-10-15T13:00:00Z");
    let v2 = story(
        "ABCD Announces FDA Approval Of Lead Candidate",
        "2026-10-15T14:00:00Z",
    );

    let mut v = NewsVersions::default();
    let first = v.propose(&[(v1.clone(), at)]);
    v.commit(&first);
    let second = v.propose(&[(v2.clone(), at + 3_600 * 1_000_000_000)]);
    v.commit(&second);
    assert_eq!((first.len(), second.len()), (1, 1), "an edit is a version");

    // Now the first version arrives again, on the other transport, later.
    let redelivered = v.propose(&[(v1, at + 7_200 * 1_000_000_000)]);
    assert!(
        redelivered.is_empty(),
        "a version already stored is not written again with a new first sighting: {redelivered:?}"
    );
    // And the newer one is not re-proposed either.
    assert!(v.propose(&[(v2, at + 7_300 * 1_000_000_000)]).is_empty());
    assert_eq!(v.forgotten(), 0);
}

/// Versions per story are bounded, and what is dropped is counted rather than changing the rule
/// in silence.
#[test]
fn versions_per_story_are_bounded_and_counted() {
    let at = t("2026-10-15", 9, 0, 0);
    let mut v = NewsVersions::default();
    let edit = |n: usize| -> RawStory {
        serde_json::from_value(json!({
            "id": 5,
            "headline": format!("ABCD Announces FDA Approval, revision {n}"),
            "summary": "",
            "author": "Staff",
            "created_at": "2026-10-15T13:00:00Z",
            "updated_at": format!("2026-10-15T13:{:02}:00Z", n % 60),
            "symbols": ["ABCD"],
            "source": "benzinga",
            "url": "https://example.invalid/5"
        }))
        .unwrap()
    };
    let first = v.propose(&[(edit(0), at)]);
    v.commit(&first);
    for n in 1..=market_providers::import::alpaca_news::VERSIONS_PER_STORY {
        let r = v.propose(&[(edit(n), at + n as i64)]);
        v.commit(&r);
    }
    assert_eq!(
        v.forgotten(),
        1,
        "one key past the bound was dropped, and said so"
    );
    // The oldest version can no longer be recognised, which is the honest cost of the bound: it
    // would be written again rather than silently treated as known.
    assert_eq!(v.propose(&[(edit(0), at + 10_000)]).len(), 1);
}

/// Two versions of a story in one search are both offered once, not twice.
#[test]
fn one_search_offering_a_story_twice_proposes_it_once() {
    let at = t("2026-10-15", 9, 0, 0);
    let s: RawStory = serde_json::from_value(json!({
        "id": 9,
        "headline": "ABCD Announces FDA Approval",
        "summary": "",
        "author": "Staff",
        "created_at": "2026-10-15T13:00:00Z",
        "updated_at": "2026-10-15T13:00:00Z",
        "symbols": ["ABCD"],
        "source": "benzinga",
        "url": "https://example.invalid/9"
    }))
    .unwrap();
    let v = NewsVersions::default();
    let out = v.propose(&[(s.clone(), at), (s, at + 1)]);
    assert_eq!(
        out.len(),
        1,
        "the same payload twice in one page is one version"
    );
    assert_eq!(out[0].first_observed_at, at, "and keeps the first sighting");
}

/// The version index is bounded in both directions: versions per story and stories in total.
///
/// `VERSIONS_PER_STORY` bounds each story's list, but the number of stories grew for the life of
/// the process — a few thousand articles a day, every day a collector stays up. Both bounds now
/// forget the least recent and count what they forget, because a forgotten version would be
/// appended again and that is worth knowing rather than discovering from the file.
#[test]
fn the_version_index_is_bounded_by_stories_as_well_as_versions() {
    use market_providers::import::alpaca_news::STORIES_TRACKED;
    let at = t("2026-10-15", 9, 0, 0);
    let story = |id: u64| -> RawStory {
        serde_json::from_value(json!({
            "id": id,
            "headline": format!("ABCD Announces FDA Approval {id}"),
            "summary": "",
            "author": "Staff",
            "created_at": "2026-10-15T13:00:00Z",
            "updated_at": "2026-10-15T13:00:00Z",
            "symbols": ["ABCD"],
            "source": "benzinga",
            "url": "https://example.invalid/x"
        }))
        .unwrap()
    };
    let mut v = NewsVersions::default();
    for id in 0..(STORIES_TRACKED as u64 + 50) {
        let r = v.propose(&[(story(id), at + id as i64)]);
        v.commit(&r);
    }
    assert_eq!(
        v.len(),
        STORIES_TRACKED,
        "the index never grows past its bound"
    );
    assert_eq!(v.stories_forgotten(), 50, "and says how many it dropped");
    assert_eq!(v.forgotten(), 50, "one version each");

    // The oldest story is no longer recognised, which is the honest cost: it would be appended
    // again rather than silently treated as stored.
    assert_eq!(v.propose(&[(story(0), at + 1_000_000)]).len(), 1);
    // A story confirmed again moves to the back, so it is not the next to go.
    let recent = STORIES_TRACKED as u64 + 49;
    assert!(v.propose(&[(story(recent), at)]).is_empty());
}

/// The repair search covers the whole freshness horizon, so an edit to an older story is found.
///
/// Alpaca's REST news is documented as *sorted* by updated date; whether `start`/`end` filter on
/// the update time or the publication time is not stated
/// (`docs.alpaca.markets/us/reference/news-3`, read 2026-10-21). If it filters on publication,
/// then an edit made now to a story published six hours ago is in neither of the windows anchored
/// near `now`: the forward window starts at the last covered point, and the edit window reaches
/// back three hours. The story is still inside the pillar's 24-hour horizon and could still decide
/// a symbol, so missing its edit means deciding on content the provider has replaced.
///
/// This is query correctness, not a threshold change: the horizon searched is the one the first
/// start already searches, and the per-minute request cap is untouched.
#[test]
fn the_repair_window_covers_the_whole_freshness_horizon() {
    let cfg = EvidenceConfig::default();
    let now = t("2026-10-15", 15, 0, 0);
    let (start, end) = news_repair_window(&cfg, now);
    assert_eq!(
        end,
        now - cfg.news_settle_secs * SEC,
        "the settle is respected, as everywhere else"
    );
    let span = end - start;
    assert!(
        span >= 24 * H,
        "the horizon a decision can read from is covered: {}h",
        span / H
    );
    assert_eq!(
        span,
        (cfg.news_initial_lookback_secs + cfg.news_overlap_secs) * SEC,
        "the first-start lookback plus the configured overlap"
    );

    // A story published six hours ago is inside the repair window and outside both others.
    let published = now - 6 * H;
    assert!(
        published >= start && published <= end,
        "the repair window reaches it"
    );
    let (_, edit_end) = (0, now - cfg.news_settle_secs * SEC);
    let edit_start = edit_end - cfg.news_edit_window_secs * SEC;
    assert!(
        published < edit_start,
        "while the edit window does not: it reaches back {}h",
        cfg.news_edit_window_secs / 3600
    );
    // And the forward window starts at the last covered point, which is later still.
    let (fwd_start, _) = news_window(&cfg, Some(now - 10 * 60 * SEC), now);
    assert!(published < fwd_start, "nor does the forward window");
}

/// An edit made while the collector was disconnected is picked up by the repair search, and the
/// coverage it claims is the window it actually finished reading.
#[test]
fn an_edit_during_a_disconnect_is_found_by_the_repair_search() {
    let (_tmp, a) = archive();
    let day = "2026-10-15";
    // A story published six hours before the collector comes back: still inside the 24-hour
    // horizon, and far outside the edit window.
    let published = "2026-10-15T09:00:00Z";
    let story = |headline: &str, updated: &str| -> serde_json::Value {
        json!({
            "id": 42,
            "headline": headline,
            "summary": "",
            "author": "Staff",
            "created_at": published,
            "updated_at": updated,
            "symbols": ["ABCD"],
            "source": "benzinga",
            "url": "https://example.invalid/42"
        })
    };

    // Before the disconnect: the original version, stored.
    let mut v = NewsVersions::default();
    let before: Vec<RawStory> =
        serde_json::from_value(json!([story("ABCD Announces FDA Approval", published)])).unwrap();
    let first = v.propose(
        &before
            .into_iter()
            .map(|s| (s, t(day, 9, 1, 0)))
            .collect::<Vec<_>>(),
    );
    a.append_news(&first).unwrap();
    v.commit(&first);
    assert_eq!(first.len(), 1);

    // During the disconnect the story is edited: a new revision of the same article.
    let edited: Vec<RawStory> = serde_json::from_value(json!([story(
        "ABCD Announces FDA Approval And Phase 3 Start",
        "2026-10-15T14:40:00Z"
    )]))
    .unwrap();
    // The repair search is what sees it; the edit window never asked about 09:00.
    let repaired = v.propose(
        &edited
            .into_iter()
            .map(|s| (s, t(day, 15, 0, 0)))
            .collect::<Vec<_>>(),
    );
    assert_eq!(
        repaired.len(),
        1,
        "the edit is a new version of the story: {repaired:?}"
    );
    a.append_news(&repaired).unwrap();
    v.commit(&repaired);

    // And from its own observation on, the edited content is what a decision reads.
    let stored = a.read_news(None).unwrap();
    let items = news_items(&stored, None);
    let before_edit: Vec<u32> = items
        .iter()
        .filter(|n| n.observed_at <= t(day, 14, 0, 0))
        .map(|n| n.version)
        .collect();
    assert_eq!(
        before_edit,
        vec![1],
        "the original, until the edit was seen"
    );
    let latest = items
        .iter()
        .filter(|n| n.observed_at <= t(day, 15, 1, 0))
        .max_by_key(|n| n.revision_at)
        .expect("a version");
    assert_eq!(
        latest.version, 2,
        "the edit is current once observed: {items:?}"
    );
}

/// A stream delivery is not a search: it never advances coverage, so it cannot stand in for the
/// repair.
///
/// Receiving one message says that one story changed. It says nothing about what else did, and
/// coverage is a claim about a window having been read in full.
#[test]
fn coverage_is_never_inferred_from_a_delivery() {
    let q = NewsQuery {
        start: utc(t("2026-10-15", 9, 0, 0)),
        end: utc(t("2026-10-15", 15, 0, 0)),
        symbols: None,
    };
    // A fetch that did not finish its pagination claims nothing, however many stories it carried.
    let partial = NewsFetch {
        stories: Vec::new(),
        requests: 1,
        complete: false,
        finished_at: t("2026-10-15", 15, 0, 1),
        error: Some("page 2: 500".into()),
    };
    assert!(
        coverage_record(&q, &partial).is_none(),
        "an unfinished search covers nothing"
    );
    let whole = NewsFetch {
        complete: true,
        error: None,
        ..partial
    };
    let c = coverage_record(&q, &whole).expect("a complete search covers its window");
    assert_eq!(
        (c.covered_from, c.covered_through),
        (
            q.start.timestamp_nanos_opt().unwrap(),
            q.end.timestamp_nanos_opt().unwrap()
        ),
        "and covers exactly the window it read"
    );
}

/// The float half's trouble does not delay the news half.
///
/// They were one tick: a float phase working through its retry backoff — five transient failures
/// and the sleeps between them — held the news poll behind it, so an outage at FMP delayed
/// catalyst detection that has nothing to do with FMP. The halves hold disjoint state and talk to
/// different providers, and production now runs them as two workers; this pins that the split is
/// real rather than cosmetic.
#[test]
fn a_float_outage_does_not_delay_the_news_poll() {
    let (_d, a) = archive();
    let day = d("2026-10-15");
    one_symbol_universe(&a, day);
    let clock = Cell::new(utc(t("2026-10-15", 9, 0, 0)));
    let slept = Cell::new(Duration::ZERO);
    let mut c = Collector::open(test_cfg(), &a, clock.get()).unwrap();

    // The float half: every attempt fails transiently, and it sleeps through its backoff.
    let mut http = five_transient();
    let fr = c.tick_float(
        &a,
        &mut http,
        Some(&key()),
        &mut || clock.get(),
        &mut |dur| slept.set(slept.get() + dur),
        &mut |_| {},
    );
    assert!(fr.float_requests > 0, "the float half really tried: {fr:?}");
    assert!(
        !fr.errors.is_empty() || fr.float_retries_scheduled > 0,
        "and really failed: {fr:?}"
    );
    assert_eq!(
        (fr.news_requests, fr.news_complete),
        (0, 0),
        "and asked nothing of the news provider: {fr:?}"
    );

    // The news half, at the same instant: unaffected, and its own search runs.
    let mut http = Script::default().json(empty_page());
    let nr = c.tick_news(
        &a,
        &mut http,
        Some(&creds()),
        &mut || clock.get(),
        &mut |_| {},
        &mut |_| {},
    );
    assert_eq!(
        (nr.news_requests, nr.news_complete),
        (1, 1),
        "the news poll was not waiting behind the float retries: {nr:?}"
    );
    assert_eq!(
        nr.float_requests, 0,
        "and it asked nothing of the float provider: {nr:?}"
    );
    // Nothing the float half did moved the news schedule, and nothing the news half did moved
    // the float plan.
    assert!(
        nr.news_through.is_some(),
        "and advanced its own coverage: {nr:?}"
    );
    assert_eq!(nr.float_retry_pending, 0, "{nr:?}");
}

/// A stream delivery reaches the archive by the same durable path a search uses, and claims no
/// coverage.
///
/// The two transports must not own separate dedup state: a version stored by one is stored, and
/// the other must not write it again. And a delivery is not a search — it says one story changed
/// and nothing about what else did — so it never advances the covered-through point. Only the
/// bounded repair search can claim a window was read.
#[test]
fn a_stream_delivery_shares_the_durable_path_and_claims_no_coverage() {
    let (_tmp, a) = archive();
    let day = "2026-10-15";
    let at = t(day, 9, 0, 0);
    let story: RawStory = serde_json::from_value(json!({
        "id": 77,
        "headline": "ABCD Announces FDA Approval",
        "summary": "",
        "author": "Staff",
        "created_at": "2026-10-15T13:00:00Z",
        "updated_at": "2026-10-15T13:00:00Z",
        "symbols": ["ABCD"],
        "source": "benzinga",
        "url": "https://example.invalid/77"
    }))
    .unwrap();
    let mut c = Collector::open(test_cfg(), &a, utc(at)).unwrap();

    let mut report = market_providers::import::evidence_collect::TickReport::default();
    c.ingest_stories(&a, &[(story.clone(), at)], &mut report);
    assert_eq!(
        (report.news_versions, report.news_from_stream),
        (1, 1),
        "the story was stored: {report:?}"
    );
    assert_eq!(a.read_news(None).unwrap().len(), 1);
    assert_eq!(
        a.read_news_coverage(None).map(|c| c.len()).unwrap_or(0),
        0,
        "and no window was claimed as searched"
    );
    assert_eq!(report.news_requests, 0, "no search happened");

    // The same story again, from either transport: already durable, so nothing is written twice.
    let mut again = market_providers::import::evidence_collect::TickReport::default();
    c.ingest_stories(&a, &[(story.clone(), at + 60 * SEC)], &mut again);
    assert_eq!(
        (again.news_versions, again.news_from_stream),
        (0, 0),
        "one owner of version state, not two: {again:?}"
    );
    assert_eq!(a.read_news(None).unwrap().len(), 1);

    // And a search that now finds it adds nothing either.
    let mut http = Script::default().json(json!({
        "news": [{
            "id": 77,
            "headline": "ABCD Announces FDA Approval",
            "summary": "",
            "author": "Staff",
            "created_at": "2026-10-15T13:00:00Z",
            "updated_at": "2026-10-15T13:00:00Z",
            "symbols": ["ABCD"],
            "source": "benzinga",
            "url": "https://example.invalid/77"
        }],
        "next_page_token": null
    }));
    let clock = Cell::new(utc(at + 300 * SEC));
    let r = c.tick_news(
        &a,
        &mut http,
        Some(&creds()),
        &mut || clock.get(),
        &mut |_| {},
        &mut |_| {},
    );
    assert_eq!(
        r.news_versions, 0,
        "the search found what the stream had already stored: {r:?}"
    );
    assert!(
        r.news_complete > 0,
        "and it did claim its own window: {r:?}"
    );
    assert_eq!(a.read_news(None).unwrap().len(), 1);
}

// --- the optional stream through the collector's own path -------------------------------------

/// A transport that replays a script, as the news worker's socket would.
struct ScriptedSocket {
    frames: Vec<market_providers::news_stream::Recv>,
}

impl market_providers::news_stream::Transport for ScriptedSocket {
    fn send(&mut self, _text: &str, _timeout: Duration) -> Result<(), String> {
        Ok(())
    }

    fn recv(&mut self, _timeout: Duration) -> market_providers::news_stream::Recv {
        if self.frames.is_empty() {
            market_providers::news_stream::Recv::Idle
        } else {
            self.frames.remove(0)
        }
    }
}

struct ScriptedDial {
    sockets: Vec<Vec<market_providers::news_stream::Recv>>,
}

impl market_providers::news_stream::Dial for ScriptedDial {
    fn dial(
        &mut self,
        _url: &str,
        _timeout: Duration,
    ) -> Result<Box<dyn market_providers::news_stream::Transport>, String> {
        if self.sockets.is_empty() {
            return Err("no socket".into());
        }
        Ok(Box::new(ScriptedSocket {
            frames: self.sockets.remove(0),
        }))
    }
}

fn frame(text: &str) -> market_providers::news_stream::Recv {
    market_providers::news_stream::Recv::Frame(text.to_string())
}

/// The whole optional path, as the news worker runs it: an injected socket, the runner's pump,
/// and the collector's own durable append — with coverage untouched.
///
/// The enable flag used to change nothing in production, because nothing read it. This exercises
/// the seam the worker actually uses: `Runner::pump` into `Collector::ingest_stories`, one owner
/// of version state shared with the REST searches, and no claim of coverage from a delivery.
#[test]
fn the_runner_feeds_the_collectors_durable_path() {
    use market_providers::news_stream::{NewsStreamConfig, Runner};
    let (_tmp, a) = archive();
    let at = t("2026-10-15", 9, 0, 0);
    let mut c = Collector::open(test_cfg(), &a, utc(at)).unwrap();

    let cfg = NewsStreamConfig {
        enabled: true,
        ..NewsStreamConfig::default()
    };
    let mut runner = Runner::new(cfg, creds()).expect("enabled");
    let mut dial = ScriptedDial {
        sockets: vec![vec![
            frame(r#"[{"T":"success","msg":"authenticated"}]"#),
            frame(r#"[{"T":"subscription","news":["*"]}]"#),
            frame(
                r#"[{"T":"n","id":501,"headline":"ABCD Announces FDA Approval","summary":"",
                "author":"Staff","created_at":"2026-10-15T13:00:00Z",
                "updated_at":"2026-10-15T13:00:00Z","symbols":["ABCD"],"source":"benzinga",
                "url":"https://example.invalid/501"}]"#,
            ),
        ]],
    };

    let stories = runner.pump(
        &mut dial,
        &mut || at,
        Duration::from_millis(500),
        &mut |_| {},
    );
    assert_eq!(stories.len(), 1, "the pump yielded the delivery");
    let mut report = market_providers::import::evidence_collect::TickReport::default();
    c.ingest_stories(&a, &stories, &mut report);
    assert_eq!(
        (report.news_versions, report.news_from_stream),
        (1, 1),
        "and the collector stored it durably: {report:?}"
    );
    assert_eq!(a.read_news(None).unwrap().len(), 1);
    assert_eq!(
        a.read_news_coverage(None).map(|c| c.len()).unwrap_or(0),
        0,
        "a delivery claims no window"
    );

    // A second delivery of the same version adds nothing: one owner of version state.
    let mut again = ScriptedDial {
        sockets: vec![vec![
            frame(r#"[{"T":"success","msg":"authenticated"}]"#),
            frame(
                r#"[{"T":"n","id":501,"headline":"ABCD Announces FDA Approval","summary":"",
                "author":"Staff","created_at":"2026-10-15T13:00:00Z",
                "updated_at":"2026-10-15T13:00:00Z","symbols":["ABCD"],"source":"benzinga",
                "url":"https://example.invalid/501"}]"#,
            ),
        ]],
    };
    let mut r2 = Runner::new(
        NewsStreamConfig {
            enabled: true,
            ..NewsStreamConfig::default()
        },
        creds(),
    )
    .expect("enabled");
    let repeat = r2.pump(
        &mut again,
        &mut || at + 60 * SEC,
        Duration::from_millis(500),
        &mut |_| {},
    );
    assert_eq!(repeat.len(), 1, "the socket delivered it again");
    let mut report2 = market_providers::import::evidence_collect::TickReport::default();
    c.ingest_stories(&a, &repeat, &mut report2);
    assert_eq!(
        report2.news_versions, 0,
        "already durable, so nothing is written twice: {report2:?}"
    );
    assert_eq!(a.read_news(None).unwrap().len(), 1);

    // An edit does reach the archive, and still claims no coverage.
    let mut edited = ScriptedDial {
        sockets: vec![vec![
            frame(r#"[{"T":"success","msg":"authenticated"}]"#),
            frame(
                r#"[{"T":"n","id":501,"headline":"ABCD Announces FDA Approval Of Lead Candidate",
                "summary":"","author":"Staff","created_at":"2026-10-15T13:00:00Z",
                "updated_at":"2026-10-15T14:00:00Z","symbols":["ABCD"],"source":"benzinga",
                "url":"https://example.invalid/501"}]"#,
            ),
        ]],
    };
    let mut r3 = Runner::new(
        NewsStreamConfig {
            enabled: true,
            ..NewsStreamConfig::default()
        },
        creds(),
    )
    .expect("enabled");
    let edit = r3.pump(
        &mut edited,
        &mut || at + 3_600 * SEC,
        Duration::from_millis(500),
        &mut |_| {},
    );
    let mut report3 = market_providers::import::evidence_collect::TickReport::default();
    c.ingest_stories(&a, &edit, &mut report3);
    assert_eq!(report3.news_versions, 1, "the edit is a new version");
    assert_eq!(a.read_news(None).unwrap().len(), 2);
    assert_eq!(
        a.read_news_coverage(None).map(|c| c.len()).unwrap_or(0),
        0,
        "and still no window is claimed from deliveries alone"
    );
}

/// With the stream disabled — the default — there is no runner to pump, so the enable flag is the
/// only thing that turns it on.
#[test]
fn the_default_configuration_has_no_runner_to_pump() {
    use market_providers::news_stream::{NewsStreamConfig, Runner};
    assert!(Runner::new(NewsStreamConfig::default(), creds()).is_none());
    assert!(
        !NewsStreamConfig::from_lookup(|_| None).enabled,
        "and nothing in a bare environment enables it"
    );
}

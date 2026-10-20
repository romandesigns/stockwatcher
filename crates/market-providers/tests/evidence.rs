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
    NewsQuery, NewsVersions, RawStory, coverage_record, fetch_news,
};
use market_providers::import::evidence_collect::{
    Collector, EvidenceConfig, FloatMode, news_window, watch_universe,
};
use market_providers::import::fmp::{FmpError, FmpKey};
use market_providers::import::fmp_float::{fetch_float, fetch_float_all};
use market_providers::import::http::{Http, HttpResponse, RetryPolicy};
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
    let first = versions.observe(&f.stories);
    assert_eq!(first.len(), 10);
    a.append_news(&first).unwrap();

    // Poll 2 at 09:26: the same stories again add nothing.
    let mut http = Script::default().json(page_2023()).json(empty_page());
    let f = poll(&mut http, &q, t(day, 9, 26, 0));
    assert!(versions.observe(&f.stories).is_empty());

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
    let edit = versions.observe(&f.stories);
    assert_eq!(edit.len(), 1);
    assert_eq!(edit[0].article_id, "35303347");
    assert_eq!(edit[0].first_observed_at, t3);
    a.append_news(&edit).unwrap();

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
    a.append_news(&NewsVersions::default().observe(&f.stories))
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
        v.observe(&[(story(1, h, "2026-10-15T11:00:00Z"), t0)]);
    // The identical story again: not a new version.
    assert!(
        v.observe(&[(story(1, h, "2026-10-15T11:00:00Z"), t0 + H)])
            .is_empty()
    );
    // The same headline under a new id (a recycled story): its own item, kept.
    stored.extend(v.observe(&[(story(2, h, "2026-10-15T12:00:00Z"), t0 + 2 * H)]));
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
    let recs = NewsVersions::default().observe(
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

    // A restart: today's floats are not fetched again; news resumes from the stored coverage
    // (forward search plus the first trailing edit search).
    clock.set(clock.get() + chrono::Duration::seconds(60));
    let mut c = Collector::open(test_cfg(), &a, clock.get()).unwrap();
    let mut http = Script::default().json(empty_page()).json(empty_page());
    let r = tick(&mut c, &mut http);
    assert_eq!(r.float_requests, 0);
    assert_eq!((r.news_requests, r.news_complete), (2, 2));
    let cov = a.read_news_coverage(None).unwrap();
    assert_eq!(cov.len(), 3);
    assert!(
        cov[1].covered_from <= cov[0].covered_through,
        "the forward search overlaps"
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

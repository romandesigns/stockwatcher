//! Corporate-action announcements against the research probes' recorded Alpaca responses
//! (`tests/fixtures/alpaca-corporate-actions-*.json`, recorded 2026-10-19). No network, no keys.

use chrono::{DateTime, NaiveDate, Utc};
use market_archive::baseline::{ActionCoverage, SplitRatio, coverage_for};
use market_archive::{ActionKind, Archive, ArchiveConfig, CoveragePolicy};
use market_providers::import::alpaca::AlpacaCredentials;
use market_providers::import::corporate::{fetch_announcements, normalize};
use market_providers::import::http::{Http, HttpResponse};

const BTOG: &str = include_str!("fixtures/alpaca-corporate-actions-BTOG.json");
const SGRX: &str = include_str!("fixtures/alpaca-corporate-actions-SGRX.json");

fn d(s: &str) -> NaiveDate {
    NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
}

fn at(s: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(s).unwrap().to_utc()
}

/// Chunk 1 (BTOG + 99 fillers): BTOG's recorded page with a next page, then SGRX's body as page
/// 2. The follow-up chunk (PLIN, SGRX: the rename counterparts) fails with 403.
struct Recorded {
    urls: Vec<String>,
}

impl Http for Recorded {
    fn get(&mut self, url: &str, _h: &[(&str, &str)]) -> Result<HttpResponse, String> {
        self.urls.push(url.to_string());
        assert!(url.contains("/v1/corporate-actions?"), "{url}");
        if url.contains("symbols=PLIN") {
            return Ok(HttpResponse {
                status: 403,
                body: b"{\"message\":\"forbidden\"}".to_vec(),
                retry_after: None,
            });
        }
        let body = if url.contains("page_token=p2") {
            SGRX.to_string()
        } else {
            BTOG.replace("\"next_page_token\":null", "\"next_page_token\":\"p2\"")
        };
        Ok(HttpResponse {
            status: 200,
            body: body.into_bytes(),
            retry_after: None,
        })
    }
}

#[test]
fn recorded_responses_follow_the_rename_chain_and_record_per_chunk_completeness() {
    let policy = CoveragePolicy::v1();
    let mut symbols = vec!["BTOG".to_string()];
    symbols.extend((0..99).map(|i| format!("S{i:03}")));
    let mut http = Recorded { urls: Vec::new() };
    let creds = AlpacaCredentials {
        key_id: "K".into(),
        secret: "S".into(),
    };
    let mut r = fetch_announcements(
        &mut http,
        "https://data.example",
        &creds,
        &policy,
        &symbols,
        d("2026-08-21"),
        // The retrieval's own stamp, injected: provenance a test can assert on.
        chrono::DateTime::parse_from_rfc3339("2026-08-21T08:30:00Z")
            .expect("valid")
            .to_utc(),
        60_000,
        "test",
        &mut |_| {},
    );
    // Process-date window around D, every record (data_quality=all), full pages.
    assert!(
        http.urls[0].contains(
            "types=forward_split%2Creverse_split%2Cunit_split%2Cname_change&start=2026-07-22&end=2026-08-31&data_quality=all&limit=1000"
        ),
        "{}",
        http.urls[0]
    );
    assert!(http.urls[1].contains("page_token=p2"));
    assert!(
        http.urls[2].contains("symbols=PLIN%2CSGRX"),
        "rename follow-ups"
    );
    assert_eq!(r.chunks.len(), 2);
    assert!(r.chunks[0].complete && r.chunks[0].pages == 2 && !r.chunks[0].followup);
    assert!(!r.chunks[1].complete && r.chunks[1].followup);
    assert!(!r.success);
    assert_eq!(r.symbols_covered.len(), 100);
    assert!(r.covers("BTOG") && !r.covers("SGRX") && !r.covers("PLIN"));
    assert_eq!(r.requests, 2);
    // Three reverse splits and two name changes, deduplicated by id, raw kept.
    let splits: Vec<_> = r
        .actions
        .iter()
        .filter(|a| a.kind == ActionKind::ReverseSplit)
        .collect();
    assert_eq!(splits.len(), 3);
    assert!(
        splits
            .iter()
            .all(|a| a.complete && a.symbol.as_deref() == Some("BTOG"))
    );
    assert_eq!(
        r.actions
            .iter()
            .filter(|a| a.kind == ActionKind::NameChange)
            .count(),
        2
    );
    let aug = splits
        .iter()
        .find(|a| a.effective_date == Some(d("2026-08-21")))
        .unwrap();
    assert_eq!((aug.old_rate, aug.new_rate), (Some(5.0), Some(1.0)));
    assert_eq!(aug.raw["old_cusip"], "G21621134");

    // Stored append-only and read back.
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_string_lossy().to_string();
    let archive = Archive::new(
        ArchiveConfig::from_lookup(|k| match k {
            "ARCHIVE_ROOT" => Some(root.clone()),
            "ARCHIVE_MIN_FREE_MB" => Some("0".into()),
            _ => None,
        })
        .unwrap(),
    );
    // As if fetched at 03:30 ET on the ex date (the fetch stamps the real clock).
    r.retrieved_at = at("2026-08-21T07:30:00Z");
    archive.write_action_retrieval(&r).unwrap();
    assert!(archive.write_action_retrieval(&r).is_err(), "append-only");
    let back = archive.read_action_retrievals(d("2026-08-21")).unwrap();
    assert_eq!(back, [r.clone()]);

    // The loader's reading: BTOG's 1-for-5 reverse split on 08-21, price × 5.
    match coverage_for(
        &back,
        &policy,
        "BTOG",
        d("2026-08-20"),
        d("2026-08-21"),
        at("2026-08-21T08:00:00Z"),
    ) {
        ActionCoverage::Confirmed {
            splits,
            price_multiplier,
            ..
        } => {
            assert_eq!(
                splits[0].0,
                SplitRatio {
                    old_rate: 5,
                    new_rate: 1
                }
            );
            assert!((price_multiplier - 5.0).abs() < 1e-9);
        }
        other => panic!("{other:?}"),
    }
    // A symbol whose chunk failed is unavailable, and that does not touch BTOG's result.
    assert_eq!(
        coverage_for(
            &back,
            &policy,
            "SGRX",
            d("2026-08-20"),
            d("2026-08-21"),
            at("2026-08-21T08:00:00Z")
        )
        .reason(),
        "no_usable_retrieval"
    );

    // The rename day (BTOG → SGRX, 2026-09-04), from a check made for that session.
    let mut rename_day = r.clone();
    rename_day.session_date = d("2026-09-04");
    (rename_day.process_start, rename_day.process_end) = policy.process_window(d("2026-09-04"));
    rename_day.retrieved_at = at("2026-09-04T07:30:00Z");
    rename_day.symbols_covered = vec!["BTOG".into(), "SGRX".into()];
    match coverage_for(
        &[rename_day.clone()],
        &policy,
        "SGRX",
        d("2026-09-03"),
        d("2026-09-04"),
        at("2026-09-04T08:00:00Z"),
    ) {
        ActionCoverage::Confirmed {
            splits,
            history_symbol,
            ..
        } => {
            assert!(splits.is_empty(), "a rename alone keeps the share basis");
            assert_eq!(history_symbol.as_deref(), Some("BTOG"));
        }
        other => panic!("{other:?}"),
    }
    rename_day.symbols_covered = vec!["SGRX".into()];
    assert_eq!(
        coverage_for(
            &[rename_day],
            &policy,
            "SGRX",
            d("2026-09-03"),
            d("2026-09-04"),
            at("2026-09-04T08:00:00Z")
        )
        .reason(),
        "rename_chain_not_covered"
    );
}

#[test]
fn incomplete_records_are_kept_and_marked() {
    let v = serde_json::json!({"id": "x", "symbol": "ABC", "old_rate": 10, "new_rate": 1,
                               "process_date": "2026-10-20"});
    let a = normalize("reverse_splits", &v).unwrap();
    assert!(!a.complete, "no ex_date");
    assert_eq!(a.raw, v);
    let v = serde_json::json!({"id": "y", "symbol": "ABC", "ex_date": "2026-10-21",
                               "old_rate": 10, "new_rate": 1, "data_quality": "incomplete"});
    let a = normalize("reverse_splits", &v).unwrap();
    assert!(!a.complete, "flagged by the provider");
    assert_eq!(a.quality.as_deref(), Some("incomplete"));
    let v = serde_json::json!({"id": "z", "symbol": "ABC", "new_symbol": "ABCD",
                               "ex_date": "2026-10-21", "old_rate": 10, "new_rate": 1});
    let a = normalize("reverse_splits", &v).unwrap();
    assert!(a.complete);
    assert_eq!(a.new_symbol.as_deref(), Some("ABCD"));
    assert!(
        normalize("cash_dividends", &v).is_none(),
        "not requested, not kept"
    );
}

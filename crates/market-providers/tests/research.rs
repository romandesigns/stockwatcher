//! Research acquisition (quotes, raw reference data, the pilot driver and its manifest) against
//! scripted HTTP responses: no network, no credentials.

use chrono::{NaiveDate, TimeZone, Utc};
use market_archive::{Archive, ArchiveConfig, SymbolReader};
use market_events::MarketEvent;
use market_providers::import::alpaca::{AlpacaCredentials, Feed, ImportError, ImportOptions};
use market_providers::import::http::{FetchError, Http, HttpResponse, RetryPolicy};
use market_providers::import::pilot::{
    BUDGET_STATUS, Budget, Kind, Metered, PilotConfig, Status, SymbolDay, all_checkpoints,
    append_run, read_checkpoint, run_pilot, write_manifest,
};
use market_providers::import::quotes::{QuotesQuery, import_quotes};
use market_providers::import::reference::{
    corporate_actions_url, daily_bars_url, et_date_range, fetch_pages, sessions_before,
    summarize_corporate_actions,
};
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::path::Path;
use std::time::Duration;

#[derive(Default)]
struct Script {
    replies: VecDeque<HttpResponse>,
    urls: Vec<String>,
}

impl Script {
    fn reply(mut self, status: u16, body: Value) -> Self {
        self.replies.push_back(HttpResponse {
            status,
            body: serde_json::to_vec(&body).unwrap(),
            retry_after: None,
        });
        self
    }
}

impl Http for Script {
    fn get(&mut self, url: &str, _headers: &[(&str, &str)]) -> Result<HttpResponse, String> {
        self.urls.push(url.to_string());
        Ok(self
            .replies
            .pop_front()
            .unwrap_or_else(|| panic!("unexpected request {url}")))
    }
}

fn creds() -> AlpacaCredentials {
    AlpacaCredentials {
        key_id: "KEYID1234".into(),
        secret: "SECRET5678".into(),
    }
}

/// Wednesday 2026-10-14 (EDT).
fn day() -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 10, 14).unwrap()
}

fn archive_at(root: &Path) -> Archive {
    let root = root.to_string_lossy().to_string();
    Archive::new(
        ArchiveConfig::from_lookup(|k| match k {
            "ARCHIVE_ROOT" => Some(root.clone()),
            "ARCHIVE_MIN_FREE_MB" => Some("0".into()),
            _ => None,
        })
        .unwrap(),
    )
}

fn quote(t: &str, bp: Value, bs: u64, ap: Value, as_: u64) -> Value {
    json!({"t": t, "bx": "Q", "bp": bp, "bs": bs, "ax": "P", "ap": ap, "as": as_, "c": ["R"], "z": "C"})
}

fn quotes_query() -> QuotesQuery {
    QuotesQuery {
        symbol: "ABC".into(),
        day: day(),
        start: Utc.with_ymd_and_hms(2026, 10, 14, 8, 0, 0).unwrap(),
        end: Utc.with_ymd_and_hms(2026, 10, 15, 0, 0, 0).unwrap(),
        feed: Feed::Sip,
        limit: 10_000,
    }
}

#[test]
fn quotes_import_pages_quarantines_and_tallies() {
    let dir = tempfile::tempdir().unwrap();
    let archive = archive_at(dir.path());
    let q1 = quote(
        "2026-10-14T13:30:00.000000001Z",
        json!(3.1),
        300,
        json!(3.12),
        200,
    );
    let mut http = Script::default()
        .reply(
            200,
            json!({"symbol": "ABC", "next_page_token": "tok/1+", "quotes": [
                q1,
                quote("2026-10-14T13:30:01Z", json!(0), 0, json!(3.12), 5),       // no bid
                quote("2026-10-14T13:30:02Z", json!(3.13), 1, json!(3.12), 1),    // crossed
                {"t": "2026-10-14T13:30:03Z", "bp": "x"},                          // malformed
                quote("2026-10-14T13:30:04Z", json!(-1), 1, json!(3.12), 1),      // negative
            ]}),
        )
        .reply(
            200,
            json!({"symbol": "ABC", "next_page_token": null, "quotes": [
                quote("2026-10-14T13:30:05Z", json!(3.12), 1, json!(3.12), 1),    // locked
                q1,                                                                  // duplicate, and earlier
            ]}),
        );
    let mut quarantined = Vec::new();
    let r = import_quotes(
        &mut http,
        "https://data.example",
        &creds(),
        &quotes_query(),
        &archive,
        &ImportOptions::default(),
        &mut |_| {},
        &mut |_| {},
        &mut |raw: &str, why: &str| quarantined.push((raw.to_string(), why.to_string())),
    )
    .unwrap();
    assert_eq!(r.pages, 2);
    assert_eq!(r.events, 5);
    assert_eq!(r.quarantined, 2);
    assert_eq!(quarantined.len(), 2);
    assert!(quarantined[1].1.contains("bad bid"));
    assert_eq!(r.zero_bid, 1);
    assert_eq!(r.crossed, 1);
    assert_eq!(r.locked, 1);
    assert_eq!(r.exact_duplicates, 1);
    assert_eq!(r.out_of_order, 1);
    assert_eq!(r.size_multiple_of_100, 2, "q1 and its duplicate");
    assert_eq!(r.conditions.get("R"), Some(&5));
    assert!(http.urls[0].contains("/v2/stocks/ABC/quotes?"));
    assert!(http.urls[0].contains("feed=sip&sort=asc"));
    assert!(
        http.urls[1].ends_with("&page_token=tok%2F1%2B"),
        "{}",
        http.urls[1]
    );
    for u in &http.urls {
        assert!(!u.contains("KEYID1234") && !u.contains("SECRET5678"));
    }

    // Read back: quotes only, zero bid kept, receive_ts absent.
    let entry = r.entry.clone().unwrap();
    assert_eq!(entry.quotes, 5);
    assert_eq!(entry.trades, 0);
    let path = archive.day_dir(day()).join(&entry.file);
    let events: Vec<_> = SymbolReader::open(&path)
        .unwrap()
        .map(|e| e.unwrap().event)
        .collect();
    let MarketEvent::Quote(q) = &events[1] else {
        panic!("not a quote")
    };
    assert_eq!(q.bid_price.micros(), 0);
    assert_eq!(q.ask_size, 5);
    assert!(q.receive_ts.is_none());
    assert_eq!(
        archive.manifest(day()).unwrap().unwrap().symbols["ABC"]
            .import
            .as_ref()
            .unwrap()
            .dataset,
        "quotes"
    );

    // Identical request again: nothing fetched.
    let mut none = Script::default();
    let again = import_quotes(
        &mut none,
        "https://data.example",
        &creds(),
        &quotes_query(),
        &archive,
        &ImportOptions::default(),
        &mut |_| {},
        &mut |_| {},
        &mut |_, _| {},
    )
    .unwrap();
    assert!(again.skipped);
    assert!(none.urls.is_empty());
}

#[test]
fn quotes_import_refused_leaves_no_manifest_entry() {
    let dir = tempfile::tempdir().unwrap();
    let archive = archive_at(dir.path());
    let mut http = Script::default().reply(403, json!({"message": "forbidden"}));
    let err = import_quotes(
        &mut http,
        "https://data.example",
        &creds(),
        &quotes_query(),
        &archive,
        &ImportOptions::default(),
        &mut |_| {},
        &mut |_| {},
        &mut |_, _| {},
    )
    .unwrap_err();
    assert!(matches!(
        err,
        ImportError::Fetch {
            error: FetchError::Permanent { status: 403, .. },
            ..
        }
    ));
    assert!(archive.manifest(day()).unwrap().is_none());
}

#[test]
fn fetch_pages_follows_tokens_and_takes_bare_arrays() {
    let mut http = Script::default()
        .reply(
            200,
            json!({"bars": {"ABC": [{"t": "x"}]}, "next_page_token": "p2"}),
        )
        .reply(
            200,
            json!({"bars": {"ABC": [{"t": "y"}]}, "next_page_token": null}),
        );
    let pages = fetch_pages(
        &mut http,
        &creds(),
        &|t| format!("https://d/x?{}", t.unwrap_or("first")),
        &mut |_| {},
        &mut |_| {},
    )
    .unwrap();
    assert_eq!(pages.len(), 2);
    assert_eq!(http.urls, vec!["https://d/x?first", "https://d/x?p2"]);

    let mut http = Script::default().reply(200, json!([{"symbol": "A"}, {"symbol": "B"}]));
    let pages = fetch_pages(
        &mut http,
        &creds(),
        &|_| "u".into(),
        &mut |_| {},
        &mut |_| {},
    )
    .unwrap();
    assert_eq!(pages.len(), 1);

    let mut http = Script::default()
        .reply(200, json!({"next_page_token": "same"}))
        .reply(200, json!({"next_page_token": "same"}));
    let err = fetch_pages(
        &mut http,
        &creds(),
        &|_| "u".into(),
        &mut |_| {},
        &mut |_| {},
    )
    .unwrap_err();
    assert!(err.contains("repeated"));
}

#[test]
fn reference_urls_and_calendar() {
    // 30 sessions before Wed 2026-10-14 skip weekends; the last is Tue 10-13.
    let s = sessions_before(day(), 30);
    assert_eq!(s.len(), 30);
    assert_eq!(
        *s.last().unwrap(),
        NaiveDate::from_ymd_opt(2026, 10, 13).unwrap()
    );
    assert!(s.windows(2).all(|w| w[0] < w[1]));
    assert!(s.iter().all(|d| market_events::is_trading_day(*d)));

    // The range ends one second before midnight ET of the day after: D's own bar is excluded.
    let (a, b) = et_date_range(s[0], *s.last().unwrap()).unwrap();
    assert_eq!(b, Utc.with_ymd_and_hms(2026, 10, 14, 3, 59, 59).unwrap());
    let u = daily_bars_url("https://d/", &["BRK.B".into(), "ABC".into()], a, b, None);
    assert!(u.starts_with("https://d/v2/stocks/bars?symbols=BRK.B%2CABC&timeframe=1Day"));
    assert!(u.contains("adjustment=raw") && u.contains("feed=sip"));
    assert!(u.contains("end=2026-10-14T03:59:59Z"), "{u}");

    let u = corporate_actions_url(
        "https://d",
        Some(&["ABC".into()]),
        "forward_split,reverse_split",
        s[0],
        day(),
        Some("t"),
    );
    assert!(u.contains("/v1/corporate-actions?start="));
    assert!(u.contains("types=forward_split%2Creverse_split"));
    assert!(u.contains("&symbols=ABC&page_token=t"));
}

#[test]
fn ticker_filter_and_refusal_parsing() {
    use market_providers::import::reference::{invalid_symbol_in, plausible_ticker};
    for ok in ["A", "BRK.B", "BF-B", "ABCDW"] {
        assert!(plausible_ticker(ok), "{ok}");
    }
    for bad in ["", "23248B208", "F017956", "CIC_DELISTED", "abc"] {
        assert!(!plausible_ticker(bad), "{bad}");
    }
    assert_eq!(
        invalid_symbol_in(r#"HTTP 400: {"message":"invalid symbol: 0029900E0"}"#).as_deref(),
        Some("0029900E0")
    );
    assert_eq!(invalid_symbol_in("HTTP 500: oops"), None);
}

#[test]
fn corporate_actions_summary() {
    let pages = vec![
        json!({"corporate_actions": {
            "reverse_splits": [{"symbol": "ABC", "new_rate": 1, "old_rate": 10, "ex_date": "2026-10-01"}],
            "name_changes": [{"old_symbol": "OLD", "new_symbol": "NEW"}],
        }, "next_page_token": "x"}),
        json!({"corporate_actions": {"reverse_splits": [{"symbol": "XYZ"}]}}),
    ];
    let s = summarize_corporate_actions(&pages);
    assert_eq!(s.counts["reverse_splits"], 2);
    assert_eq!(s.counts["name_changes"], 1);
    assert_eq!(s.symbols(), vec!["ABC", "NEW", "OLD", "XYZ"]);
}

#[test]
fn metered_refuses_after_the_deadline() {
    let mut inner = Script::default().reply(200, json!({}));
    let mut m = Metered::new(&mut inner, Some(std::time::Instant::now()));
    m.set_kind("k");
    let r = m.get("u", &[]).unwrap();
    assert_eq!(r.status, BUDGET_STATUS);
    assert!(m.budget_hit);
    assert_eq!(m.stats["k"].budget_refused, 1);
    assert_eq!(m.stats["k"].requests, 0);
}

fn pilot_cfg<'a>(out: &'a Path, creds: &'a AlpacaCredentials, minutes: u64) -> PilotConfig<'a> {
    PilotConfig {
        out,
        data_base: "https://data.example",
        creds,
        budget: Budget {
            active: Duration::from_secs(minutes * 60),
            bytes: 1_000_000_000,
        },
        used_active: Duration::ZERO,
        baseline_bytes: 0,
        warmup_sessions: 3,
        kinds: Kind::ALL.to_vec(),
        retry: RetryPolicy {
            max_attempts: 2,
            base_delay: Duration::ZERO,
            max_delay: Duration::ZERO,
        },
    }
}

#[test]
fn pilot_runs_every_kind_checkpoints_and_resumes() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path();
    std::fs::write(out.join("ignore-me"), b"").unwrap();
    let creds = creds();
    let cfg = pilot_cfg(out, &creds, 30);
    let sd = SymbolDay {
        day: day(),
        symbol: "ABC".into(),
    };
    let mut http = Script::default()
        // daily bars: 3 sessions requested (10-09, 10-12, 10-13), 10-12 missing
        .reply(
            200,
            json!({"bars": {"ABC": [
                {"t": "2026-10-09T04:00:00Z", "o": 1, "h": 1, "l": 1, "c": 1, "v": 10, "n": 2, "vw": 1},
                {"t": "2026-10-13T04:00:00Z", "o": 1, "h": 1, "l": 1, "c": 1, "v": 10, "n": 2, "vw": 1},
            ]}, "next_page_token": null}),
        )
        // corporate actions
        .reply(
            200,
            json!({"corporate_actions": {"forward_splits": [{"symbol": "ABC"}]}, "next_page_token": null}),
        )
        // SIP trades: one canceled print
        .reply(
            200,
            json!({"symbol": "ABC", "next_page_token": null, "trades": [
                {"t": "2026-10-14T13:30:00Z", "x": "V", "p": 3.18, "s": 100, "c": ["@"], "i": 1, "z": "C"},
                {"t": "2026-10-14T13:30:01Z", "x": "V", "p": 3.19, "s": 50, "c": ["@"], "i": 2, "z": "C", "u": "canceled"},
            ]}),
        )
        // BOATS: not entitled
        .reply(403, json!({"message": "subscription does not permit querying boats"}))
        // SIP quotes
        .reply(
            200,
            json!({"symbol": "ABC", "next_page_token": null, "quotes": [
                quote("2026-10-14T13:30:00Z", json!(3.17), 1, json!(3.19), 2),
            ]}),
        );
    let run = run_pilot(
        &mut http,
        &cfg,
        std::slice::from_ref(&sd),
        &mut |_| {},
        &mut |_| {},
    )
    .unwrap();
    assert_eq!(run.ran, 5);
    assert_eq!(run.skipped, 0);
    assert!(run.stopped_by.is_none());
    assert!(http.urls[3].contains("feed=boats"));
    // Overnight window: 20:00 ET 10-13 -> 04:00 ET 10-14.
    assert!(http.urls[3].contains("start=2026-10-14T00:00:00Z&end=2026-10-14T08:00:00Z"));

    let bars = read_checkpoint(out, &sd, Kind::DailyBars).unwrap();
    assert_eq!(bars.status, Status::Complete);
    assert_eq!(bars.records, 2);
    assert_eq!(bars.details["missing_sessions"], json!(["2026-10-12"]));
    assert!(out.join("raw/daily-bars/2026-10-14/ABC.json").exists());

    let ca = read_checkpoint(out, &sd, Kind::CorporateActions).unwrap();
    assert_eq!(ca.records, 1);

    let t = read_checkpoint(out, &sd, Kind::TradesSip).unwrap();
    assert_eq!(t.status, Status::Complete);
    assert_eq!(t.records, 2);
    assert_eq!(t.details["verify"]["update_flags_u"]["canceled"], 1);
    assert_eq!(t.details["verify"]["duplicate_ids"], 0);
    assert_eq!(t.files, vec!["archive/2026-10-14/ABC.swa"]);
    assert_eq!(t.stats.requests, 1);

    let b = read_checkpoint(out, &sd, Kind::TradesBoats).unwrap();
    assert_eq!(b.status, Status::Inaccessible);
    assert_eq!(b.http_status, Some(403));
    assert!(
        !out.join("archive-boats/2026-10-14/ABC.swa.partial")
            .exists()
    );

    let q = read_checkpoint(out, &sd, Kind::QuotesSip).unwrap();
    assert_eq!(q.status, Status::Complete);
    assert_eq!(q.files, vec!["archive-quotes/2026-10-14/ABC.swa"]);

    // Re-run: everything checkpointed, nothing fetched.
    let mut none = Script::default();
    let again = run_pilot(
        &mut none,
        &cfg,
        std::slice::from_ref(&sd),
        &mut |_| {},
        &mut |_| {},
    )
    .unwrap();
    assert_eq!(again.skipped, 5);
    assert!(none.urls.is_empty());

    // Manifest: every file hashed, revision increments, partitions and totals reported.
    append_run(out, &json!({"command": "test", "active_ms": 1})).unwrap();
    let m = write_manifest(out, json!({"code": {"commit": "abc"}})).unwrap();
    assert_eq!(m["revision"], 1);
    assert_eq!(m["code"]["commit"], "abc");
    let files = m["files"].as_array().unwrap();
    assert!(
        files
            .iter()
            .any(|f| f["path"] == "archive/2026-10-14/ABC.swa"
                && f["sha256"].as_str().unwrap().len() == 64)
    );
    assert_eq!(m["missing_partitions"].as_array().unwrap().len(), 1);
    assert_eq!(m["totals"]["trade_update_flags_u"]["canceled"], 1);
    assert_eq!(
        m["symbol_days"]["2026-10-14 ABC"]
            .as_object()
            .unwrap()
            .len(),
        5
    );
    assert_eq!(m["runs"].as_array().unwrap().len(), 1);
    let m2 = write_manifest(out, json!({})).unwrap();
    assert_eq!(m2["revision"], 2);
    assert_eq!(all_checkpoints(out).len(), 5);
}

#[test]
fn pilot_stops_at_the_time_budget_without_fetching() {
    let dir = tempfile::tempdir().unwrap();
    let creds = creds();
    let cfg = pilot_cfg(dir.path(), &creds, 0);
    let sd = SymbolDay {
        day: day(),
        symbol: "ABC".into(),
    };
    let mut none = Script::default();
    let run = run_pilot(&mut none, &cfg, &[sd], &mut |_| {}, &mut |_| {}).unwrap();
    assert_eq!(run.stopped_by.as_deref(), Some("active-time budget"));
    assert_eq!(run.ran, 0);
    assert!(none.urls.is_empty());
}

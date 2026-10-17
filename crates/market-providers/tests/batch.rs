//! Batch planning and parallel import against a URL-routed fake server: no network, no keys.

use chrono::NaiveDate;
use market_archive::{Archive, ArchiveConfig};
use market_events::{Price, PriceBand};
use market_providers::import::alpaca::AlpacaCredentials;
use market_providers::import::batch::{Plan, Task, plan_universe, run_batch};
use market_providers::import::http::{Http, HttpResponse};
use serde_json::json;
use std::sync::Mutex;
use std::time::Duration;

fn creds() -> AlpacaCredentials {
    AlpacaCredentials {
        key_id: "K".into(),
        secret: "S".into(),
    }
}

fn ok(v: serde_json::Value) -> Result<HttpResponse, String> {
    Ok(HttpResponse {
        status: 200,
        body: serde_json::to_vec(&v).unwrap(),
        retry_after: None,
    })
}

/// Answers by URL: the assets list, daily bars, and a one-page trade list per symbol.
#[derive(Clone)]
struct Router;

impl Http for Router {
    fn get(&mut self, url: &str, _h: &[(&str, &str)]) -> Result<HttpResponse, String> {
        if url.contains("/v2/assets") {
            return ok(json!([
                {"symbol": "AAA", "tradable": true},
                {"symbol": "BBB", "tradable": true},
                {"symbol": "CCC", "tradable": true},
                {"symbol": "DDD", "tradable": false},
                {"symbol": "EEE/WS", "tradable": true},
                {"symbol": "FAIL", "tradable": true}
            ]));
        }
        if url.contains("/v2/stocks/bars") {
            // Midnight ET as UTC, as Alpaca stamps daily bars.
            return ok(json!({"bars": {
                "AAA":    [{"t": "2026-10-14T04:00:00Z", "c": 0.45, "n": 30000},
                           {"t": "2026-10-15T04:00:00Z", "c": 0.09, "n": 5}],
                "BBB":    [{"t": "2026-10-14T04:00:00Z", "c": 12.0, "n": 10},
                           {"t": "2026-10-15T04:00:00Z", "c": 25.0, "n": 20}],
                "CCC":    [{"t": "2026-10-14T04:00:00Z", "c": 30.0, "n": 99999}],
                "EEE/WS": [{"t": "2026-10-14T04:00:00Z", "c": 0.2, "n": 7}],
                "FAIL":   [{"t": "2026-10-15T04:00:00Z", "c": 1.0, "n": 1}]
            }, "next_page_token": null}));
        }
        if let Some(rest) = url.split("/v2/stocks/").nth(1)
            && let Some(sym) = rest.split('/').next()
        {
            if sym == "FAIL" {
                return Ok(HttpResponse {
                    status: 403,
                    body: b"{\"message\":\"forbidden\"}".to_vec(),
                    retry_after: None,
                });
            }
            let day = if url.contains("start=2026-10-14") {
                "2026-10-14"
            } else {
                "2026-10-15"
            };
            return ok(json!({"trades": [
                {"t": format!("{day}T13:30:00Z"), "x": "V", "p": 1.0, "s": 100, "c": ["@"], "i": 1, "z": "C"},
                {"t": format!("{day}T13:31:00Z"), "x": "V", "p": 1.01, "s": 200, "c": ["@"], "i": 2, "z": "C"}
            ], "symbol": sym, "next_page_token": null}));
        }
        Err(format!("unexpected url {url}"))
    }
}

fn d(s: &str) -> NaiveDate {
    NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
}

fn archive() -> (tempfile::TempDir, Archive) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_string_lossy().to_string();
    let cfg = ArchiveConfig::from_lookup(|k| match k {
        "ARCHIVE_ROOT" => Some(root.clone()),
        "ARCHIVE_MIN_FREE_MB" => Some("0".into()),
        _ => None,
    })
    .unwrap();
    (dir, Archive::new(cfg))
}

fn plan() -> Plan {
    plan_universe(
        &mut Router,
        "https://trading.example",
        "https://data.example",
        &creds(),
        d("2026-10-14"),
        d("2026-10-15"),
        PriceBand::DEFAULT,
        &mut |_| {},
    )
    .unwrap()
}

#[test]
fn the_plan_keeps_in_band_symbol_days_busiest_first() {
    let p = plan();
    assert_eq!(p.assets, 5, "the untradable asset is not considered");
    let tasks: Vec<(String, String, u64)> = p
        .tasks
        .iter()
        .map(|t| (t.day.to_string(), t.symbol.clone(), t.trades))
        .collect();
    assert_eq!(
        tasks,
        vec![
            ("2026-10-14".into(), "AAA".into(), 30000),
            ("2026-10-15".into(), "BBB".into(), 20),
            ("2026-10-14".into(), "BBB".into(), 10),
            ("2026-10-15".into(), "FAIL".into(), 1),
        ],
        "AAA under $0.10 and CCC over $25 are out; $25.00 itself is in"
    );
    assert_eq!(p.skipped_symbols, vec!["EEE/WS".to_string()]);
    assert_eq!(p.trades(), 30031);
    assert_eq!(p.requests(10_000), 3 + 1 + 1 + 1);
    let _ = Price::from_cents(1);
}

#[test]
fn workers_import_in_parallel_collect_failures_and_a_rerun_skips_what_is_done() {
    let (_dir, archive) = archive();
    let p = plan();
    let make = || Ok::<_, String>(Router);
    let logs = Mutex::new(Vec::new());
    let run = || {
        run_batch(
            &p,
            &make,
            "https://data.example",
            &creds(),
            &archive,
            3,
            600_000,
            Duration::from_secs(3600),
            &|_| {},
            &|l| logs.lock().unwrap().push(l.to_string()),
        )
    };
    let r = run();
    assert_eq!((r.tasks, r.imported, r.skipped), (4, 3, 0));
    assert_eq!(r.failed.len(), 1);
    assert_eq!(r.failed[0].symbol, "FAIL");
    assert!(
        r.failed[0].error.contains("HTTP 403"),
        "{}",
        r.failed[0].error
    );
    assert_eq!(r.events, 6);
    // Three workers wrote two days' manifests concurrently: no entry was lost.
    let m14 = archive.manifest(d("2026-10-14")).unwrap().unwrap();
    let m15 = archive.manifest(d("2026-10-15")).unwrap().unwrap();
    assert_eq!(
        m14.symbols.keys().cloned().collect::<Vec<_>>(),
        vec!["AAA", "BBB"]
    );
    assert_eq!(m15.symbols.keys().cloned().collect::<Vec<_>>(), vec!["BBB"]);
    assert!(archive.verify(d("2026-10-14")).unwrap().ok());
    assert!(
        logs.lock()
            .unwrap()
            .iter()
            .any(|l| l.contains("FAILED 2026-10-15 FAIL"))
    );

    // Re-running retries only what is missing.
    let r = run();
    assert_eq!((r.imported, r.skipped, r.failed.len()), (0, 3, 1));
    assert_eq!(r.requests, 1, "only the failed symbol-day is fetched again");
}

#[test]
fn a_task_on_a_non_trading_day_fails_cleanly() {
    let (_dir, archive) = archive();
    let p = Plan {
        tasks: vec![Task {
            day: d("2026-10-17"),
            symbol: "AAA".into(),
            close: 1.0,
            trades: 1,
        }],
        ..Plan::default()
    };
    let r = run_batch(
        &p,
        &|| Ok::<_, String>(Router),
        "https://data.example",
        &creds(),
        &archive,
        1,
        600_000,
        Duration::from_secs(3600),
        &|_| {},
        &|_| {},
    );
    assert_eq!(r.failed.len(), 1);
    assert!(r.failed[0].error.contains("no SIP sessions"));
}

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

/// Records the request and answers with an assets list carrying names and exchanges.
struct AssetsServer {
    urls: Vec<String>,
    headers: Vec<(String, String)>,
}

impl Http for AssetsServer {
    fn get(&mut self, url: &str, h: &[(&str, &str)]) -> Result<HttpResponse, String> {
        self.urls.push(url.to_string());
        self.headers = h
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        ok(json!([
            {"symbol": "AAL", "tradable": true, "name": "American Airlines Group Inc.", "exchange": "NASDAQ"},
            {"symbol": "F", "tradable": true, "name": " Ford Motor Company ", "exchange": "NYSE"},
            {"symbol": "OLDCO", "tradable": false, "name": "Old Co", "exchange": "OTC"},
            {"symbol": "BLANK", "tradable": true, "name": "", "exchange": ""},
            {"symbol": "NONAME", "tradable": true, "exchange": "ARCA"}
        ]))
    }
}

#[test]
fn asset_metadata_keeps_the_providers_names_and_exchanges_and_invents_none() {
    use market_archive::AssetMeta;
    use market_providers::import::batch::asset_metadata;
    let mut http = AssetsServer {
        urls: Vec::new(),
        headers: Vec::new(),
    };
    let m = asset_metadata(&mut http, "https://trading.example/", &creds(), &mut |_| {}).unwrap();
    assert_eq!(
        http.urls,
        ["https://trading.example/v2/assets?status=active&asset_class=us_equity"],
        "one request, credentials never in the URL"
    );
    assert!(
        http.headers
            .contains(&("APCA-API-KEY-ID".into(), "K".into()))
    );
    let meta = |name: &str, exchange: &str| AssetMeta {
        name: name.into(),
        exchange: exchange.into(),
    };
    assert_eq!(
        m.get("AAL"),
        Some(&meta("American Airlines Group Inc.", "NASDAQ"))
    );
    assert_eq!(
        m.get("F"),
        Some(&meta("Ford Motor Company", "NYSE")),
        "trimmed"
    );
    assert_eq!(
        m.get("OLDCO"),
        Some(&meta("Old Co", "OTC")),
        "untradable today may still be in an archived day"
    );
    assert_eq!(
        m.get("NONAME"),
        Some(&meta("", "ARCA")),
        "no name is not made up"
    );
    assert!(!m.contains_key("BLANK"), "nothing to show: left out");
    assert_eq!(m.len(), 4);

    // Written atomically to the archive root, where the server reads it.
    let (_dir, archive) = archive();
    archive.write_assets(&m).unwrap();
    assert_eq!(archive.read_assets().unwrap(), Some(m));
}

/// Daily bars by URL with two pages for the first chunk, recording every request, so the closes
/// fetch is checked for its query (raw, SIP, 1Day), chunking and paging.
struct ClosesServer {
    urls: Vec<String>,
}

impl Http for ClosesServer {
    fn get(&mut self, url: &str, _h: &[(&str, &str)]) -> Result<HttpResponse, String> {
        self.urls.push(url.to_string());
        if url.contains("/v2/assets") {
            // 201 assets: two chunks of at most 200 symbols. Untradable ones are included.
            let mut a: Vec<serde_json::Value> = (0..199)
                .map(|i| json!({"symbol": format!("S{i:03}"), "tradable": true}))
                .collect();
            a.push(json!({"symbol": "AAA", "tradable": true}));
            a.push(json!({"symbol": "ZOLD", "tradable": false}));
            return ok(json!(a));
        }
        if url.contains("/v2/stocks/bars") {
            if url.contains("ZOLD") {
                return ok(json!({"bars": {
                    "ZOLD": [{"t": "2026-10-14T04:00:00Z", "c": 0.5, "n": 3}]
                }, "next_page_token": null}));
            }
            if !url.contains("page_token=") {
                return ok(json!({"bars": {
                    "AAA": [{"t": "2026-10-14T04:00:00Z", "c": 1.25, "n": 10},
                            // Outside the requested range: ignored.
                            {"t": "2026-10-16T04:00:00Z", "c": 9.0, "n": 1}],
                    "S000": [{"t": "2026-10-14T04:00:00Z", "c": 0.0, "n": 1}]
                }, "next_page_token": "p2"}));
            }
            return ok(json!({"bars": {
                "AAA": [{"t": "2026-10-15T04:00:00Z", "c": 1.5, "n": 10}]
            }, "next_page_token": null}));
        }
        Err(format!("unexpected url {url}"))
    }
}

#[test]
fn daily_closes_fetch_raw_sip_closes_for_every_active_asset_and_merge_into_the_file() {
    use market_archive::{DailyCloses, PrevCloses, merge_daily_closes};
    use market_providers::import::batch::daily_closes;
    let mut http = ClosesServer { urls: Vec::new() };
    let m = daily_closes(
        &mut http,
        "https://trading.example",
        "https://data.example",
        &creds(),
        d("2026-10-14"),
        d("2026-10-15"),
        &mut |_| {},
    )
    .unwrap();
    let bars: Vec<&String> = http
        .urls
        .iter()
        .filter(|u| u.contains("/v2/stocks/bars"))
        .collect();
    assert_eq!(
        bars.len(),
        3,
        "two pages for the first chunk, one for the second"
    );
    for u in &bars {
        assert!(
            u.contains("timeframe=1Day") && u.contains("feed=sip") && u.contains("adjustment=raw"),
            "{u}"
        );
        assert!(u.contains("start=2026-10-14&end=2026-10-15"), "{u}");
    }
    assert!(bars[1].contains("page_token=p2"));
    assert!(bars[2].contains("ZOLD"), "untradable assets get closes too");

    let mut want = DailyCloses::new();
    want.entry(d("2026-10-14"))
        .or_default()
        .insert("AAA".into(), 1.25);
    want.entry(d("2026-10-14"))
        .or_default()
        .insert("ZOLD".into(), 0.5);
    want.entry(d("2026-10-15"))
        .or_default()
        .insert("AAA".into(), 1.5);
    assert_eq!(m, want, "a zero close and an out-of-range bar are left out");

    // Merged with what the file already has, written atomically, and read back as the server's
    // reference: the latest close strictly before the replay day.
    let (_dir, archive) = archive();
    let mut existing = DailyCloses::new();
    existing
        .entry(d("2026-10-13"))
        .or_default()
        .insert("AAA".into(), 1.0);
    existing
        .entry(d("2026-10-14"))
        .or_default()
        .insert("AAA".into(), 1.2);
    archive.write_daily_closes(&existing).unwrap();
    let mut all = archive.read_daily_closes().unwrap().unwrap();
    assert_eq!(merge_daily_closes(&mut all, m), 3);
    archive.write_daily_closes(&all).unwrap();
    let back = archive.read_daily_closes().unwrap().unwrap();
    assert_eq!(back.len(), 3, "the 13th is kept");
    let (idx, _) = PrevCloses::new(&back);
    assert_eq!(
        idx.before("AAA", d("2026-10-15")),
        Some((d("2026-10-14"), Price::from_cents(125))),
        "the re-fetched close replaced the old one"
    );
}

/// A hand-written page in Alpaca's daily-bars shape (all fields, as the API returns them).
const RAW_PAGE: &str = r#"{
  "bars": {
    "AAA": [
      {"t":"2026-10-14T04:00:00Z","o":1.20,"h":1.31,"l":1.18,"c":1.25,"v":120500,"n":812,"vw":1.2471},
      {"t":"2026-10-15T04:00:00Z","o":1.26,"h":1.55,"l":1.25,"c":1.50,"v":980100,"n":4410,"vw":1.4012}
    ],
    "BBB": [
      {"t":"2026-10-15T04:00:00Z","o":7.00,"h":7.10,"l":6.90,"c":7.05,"v":5500,"n":61,"vw":7.01}
    ]
  },
  "next_page_token": null
}"#;

/// Serves [`RAW_PAGE`] for raw requests and a split-adjusted page for split ones, recording URLs.
struct BarsFixture {
    urls: Vec<String>,
}

impl Http for BarsFixture {
    fn get(&mut self, url: &str, _h: &[(&str, &str)]) -> Result<HttpResponse, String> {
        self.urls.push(url.to_string());
        if url.contains("/v2/assets") {
            return ok(json!([
                {"symbol": "AAA", "tradable": true},
                {"symbol": "BBB", "tradable": true},
                {"symbol": "CCC", "tradable": false}
            ]));
        }
        if url.contains("adjustment=split") {
            // AAA after a 1:10 reverse split: prices x10, volumes /10. ZZZ was not asked for.
            return ok(json!({"bars": {
                "AAA": [{"t":"2026-10-14T04:00:00Z","o":12.0,"h":13.1,"l":11.8,"c":12.5,"v":12050,"n":812,"vw":12.471}],
                "ZZZ": [{"t":"2026-10-14T04:00:00Z","c":1.0,"v":1,"n":1}]
            }, "next_page_token": null}));
        }
        if url.contains("/v2/stocks/bars") {
            return Ok(HttpResponse {
                status: 200,
                body: RAW_PAGE.as_bytes().to_vec(),
                retry_after: None,
            });
        }
        Err(format!("unexpected url {url}"))
    }
}

#[test]
fn raw_daily_bars_are_kept_from_the_same_responses_as_the_closes() {
    use market_archive::{Adjustment, DailyBar};
    use market_providers::import::batch::daily_closes_and_bars;
    let mut http = BarsFixture { urls: Vec::new() };
    let (closes, raw) = daily_closes_and_bars(
        &mut http,
        "https://trading.example",
        "https://data.example",
        &creds(),
        d("2026-10-14"),
        d("2026-10-15"),
        "test",
        &mut |_| {},
    )
    .unwrap();
    let bar_requests = http
        .urls
        .iter()
        .filter(|u| u.contains("/v2/stocks/bars"))
        .count();
    assert_eq!(bar_requests, 1, "no extra request for the raw bars");
    assert_eq!(closes[&d("2026-10-15")]["AAA"], 1.5, "closes unchanged");

    assert_eq!(raw.len(), 2);
    let d14 = &raw[0];
    assert_eq!(d14.date, d("2026-10-14"));
    assert_eq!(d14.adjustment, Adjustment::Raw);
    assert_eq!(
        d14.bars["AAA"],
        DailyBar {
            o: Some(1.20),
            h: Some(1.31),
            l: Some(1.18),
            c: 1.25,
            v: Some(120_500),
            n: Some(812),
            vw: Some(1.2471),
        }
    );
    // Requested but absent is listed as covered-without-a-bar, never a zero-volume bar.
    assert_eq!(
        d14.no_bar.iter().map(String::as_str).collect::<Vec<_>>(),
        ["BBB", "CCC"]
    );
    let f = &d14.fetches[0];
    assert_eq!(
        (f.feed.as_str(), f.timeframe.as_str(), f.adjustment, f.asof),
        ("sip", "1Day", Adjustment::Raw, None)
    );
    assert_eq!((f.symbols, f.requests), (3, 1));
    assert_eq!(raw[1].bars["BBB"].v, Some(5500));

    // Kept beside the closes, merged per date; the identical refetch writes nothing.
    let (_dir, archive) = archive();
    let rep = archive.merge_raw_bars(raw.clone()).unwrap();
    assert_eq!(rep.dates_written, [d("2026-10-14"), d("2026-10-15")]);
    assert_eq!(
        archive.read_raw_bars(d("2026-10-14")).unwrap().as_ref(),
        Some(d14)
    );
    assert!(
        archive
            .merge_raw_bars(raw)
            .unwrap()
            .dates_written
            .is_empty()
    );
}

#[test]
fn a_split_snapshot_is_opt_in_bounded_and_keeps_only_what_was_asked() {
    use market_archive::Adjustment;
    use market_providers::import::batch::{MAX_SPLIT_SYMBOLS, split_snapshot};
    let mut http = BarsFixture { urls: Vec::new() };
    let snap = split_snapshot(
        &mut http,
        "https://data.example",
        &creds(),
        &["aaa".to_string(), "BBB".to_string(), "AAA".to_string()],
        d("2026-10-14"),
        d("2026-10-15"),
        60_000,
        None,
        "test",
        &mut |_| {},
    )
    .unwrap();
    assert_eq!(http.urls.len(), 1);
    assert!(
        http.urls[0].contains("adjustment=split") && http.urls[0].contains("feed=sip"),
        "{}",
        http.urls[0]
    );
    assert!(!http.urls[0].contains("asof"), "asof is never sent");
    assert_eq!(snap.symbols, ["AAA", "BBB"]);
    assert_eq!(snap.fetch.adjustment, Adjustment::Split);
    assert!(!snap.bars.contains_key("ZZZ"), "not requested, not kept");
    assert_eq!(snap.bars["AAA"][&d("2026-10-14")].v, Some(12_050));
    assert_eq!(snap.coverage("BBB", d("2026-10-14")), Some(None));

    let (_dir, archive) = archive();
    archive.write_split_snapshot(&snap).unwrap();
    assert_eq!(archive.read_split_snapshots().unwrap(), [snap]);

    // Bounds: refused before any request.
    let mut http = BarsFixture { urls: Vec::new() };
    let many: Vec<String> = (0..=MAX_SPLIT_SYMBOLS).map(|i| format!("S{i}")).collect();
    assert!(
        split_snapshot(
            &mut http,
            "https://data.example",
            &creds(),
            &many,
            d("2026-10-14"),
            d("2026-10-15"),
            60_000,
            None,
            "test",
            &mut |_| {},
        )
        .is_err()
    );
    assert!(
        split_snapshot(
            &mut http,
            "https://data.example",
            &creds(),
            &["AAA".to_string()],
            d("2026-01-02"),
            d("2026-10-15"),
            60_000,
            None,
            "test",
            &mut |_| {},
        )
        .is_err()
    );
    assert!(http.urls.is_empty());
}

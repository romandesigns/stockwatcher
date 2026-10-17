//! The importers against scripted HTTP responses: no network, no credentials.

use chart_engine::Timeframe;
use chrono::{NaiveDate, TimeZone, Utc};
use market_archive::{Archive, ArchiveConfig, BarEntry};
use market_events::{MarketEvent, Price, Tape};
use market_providers::import::alpaca::{
    AlpacaCredentials, Feed, ImportError, ImportOptions, RawTrade, TradesQuery, import_trades,
    to_trade,
};
use market_providers::import::check::{compare_minutes, replay_day};
use market_providers::import::fmp::{self, FmpError, FmpKey, fetch_minute_bars};
use market_providers::import::http::{
    FetchError, Http, HttpResponse, RetryPolicy, redact_url, scrub,
};
use serde_json::json;
use std::collections::VecDeque;
use std::time::Duration;

// --- a scripted HTTP server -------------------------------------------------------------------

#[derive(Default)]
struct Script {
    replies: VecDeque<Result<HttpResponse, String>>,
    urls: Vec<String>,
    headers: Vec<Vec<(String, String)>>,
}

impl Script {
    fn reply(mut self, status: u16, body: serde_json::Value) -> Self {
        self.replies.push_back(Ok(HttpResponse {
            status,
            body: serde_json::to_vec(&body).unwrap(),
            retry_after: None,
        }));
        self
    }
    fn reply_after(mut self, status: u16, secs: u64) -> Self {
        self.replies.push_back(Ok(HttpResponse {
            status,
            body: b"slow down".to_vec(),
            retry_after: Some(Duration::from_secs(secs)),
        }));
        self
    }
    fn network_error(mut self) -> Self {
        self.replies.push_back(Err("connection reset".into()));
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

// --- fixtures ----------------------------------------------------------------------------------

/// Wednesday 2026-10-14, a trading day (EDT: 09:30 ET = 13:30Z).
fn day() -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 10, 14).unwrap()
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

fn creds() -> AlpacaCredentials {
    AlpacaCredentials {
        key_id: "TESTKEYID0001".into(),
        secret: "TESTSECRET0001".into(),
    }
}

fn query() -> TradesQuery {
    TradesQuery {
        symbol: "ABCD".into(),
        day: day(),
        start: Utc.with_ymd_and_hms(2026, 10, 14, 8, 0, 0).unwrap(),
        end: Utc.with_ymd_and_hms(2026, 10, 15, 0, 0, 0).unwrap(),
        feed: Feed::Sip,
        limit: 2,
    }
}

fn trade(t: &str, p: serde_json::Value, s: u64, i: u64) -> serde_json::Value {
    json!({"t": t, "x": "V", "p": p, "s": s, "c": ["@"], "i": i, "z": "C"})
}

fn page(trades: Vec<serde_json::Value>, next: Option<&str>) -> serde_json::Value {
    json!({"trades": trades, "symbol": "ABCD", "next_page_token": next})
}

fn no_retry_wait() -> RetryPolicy {
    RetryPolicy {
        max_attempts: 4,
        base_delay: Duration::from_millis(100),
        max_delay: Duration::from_secs(5),
    }
}

fn run(
    http: &mut Script,
    archive: &Archive,
    q: &TradesQuery,
    force: bool,
) -> (
    Result<market_providers::import::alpaca::ImportReport, ImportError>,
    Vec<Duration>,
    Vec<String>,
) {
    let mut sleeps = Vec::new();
    let mut logs = Vec::new();
    let opts = ImportOptions {
        retry: no_retry_wait(),
        force,
    };
    let r = import_trades(
        http,
        "https://data.example",
        &creds(),
        q,
        archive,
        &opts,
        &mut |d| sleeps.push(d),
        &mut |l| logs.push(l.to_string()),
    );
    (r, sleeps, logs)
}

fn three_pages() -> Script {
    Script::default()
        // A full page (limit 2) with a token that needs encoding.
        .reply(
            200,
            page(
                vec![
                    trade("2026-10-14T13:30:00.123456789Z", json!(0.4512), 100, 11),
                    trade("2026-10-14T13:30:00.123456789Z", json!(0.4513), 37, 12),
                ],
                Some("tok/1+a=="),
            ),
        )
        // A partial page that still carries a token: it is not the end.
        .reply(
            200,
            page(
                vec![trade("2026-10-14T13:31:05Z", json!(0.46), 500, 13)],
                Some("tok2"),
            ),
        )
        // An empty final page.
        .reply(200, page(vec![], None))
}

// --- pagination ---------------------------------------------------------------------------------

#[test]
fn follows_every_page_until_the_token_runs_out() {
    let (_d, archive) = archive();
    let mut http = three_pages();
    let (r, _, _) = run(&mut http, &archive, &query(), false);
    let r = r.unwrap();
    assert_eq!((r.pages, r.empty_pages, r.events), (3, 1, 3));
    assert_eq!(http.urls.len(), 3);
    let u0 = &http.urls[0];
    assert!(
        u0.starts_with("https://data.example/v2/stocks/ABCD/trades?"),
        "{u0}"
    );
    for part in [
        "start=2026-10-14T08:00:00Z",
        "end=2026-10-15T00:00:00Z",
        "limit=2",
        "feed=sip",
        "sort=asc",
    ] {
        assert!(u0.contains(part), "{u0} lacks {part}");
    }
    assert!(!u0.contains("page_token"));
    assert!(
        http.urls[1].ends_with("&page_token=tok%2F1%2Ba%3D%3D"),
        "{}",
        http.urls[1]
    );
    assert!(http.urls[2].ends_with("&page_token=tok2"));
    // Credentials travel only in headers.
    assert!(
        http.headers[0]
            .iter()
            .any(|(k, v)| k == "APCA-API-KEY-ID" && v == "TESTKEYID0001")
    );
    assert!(
        http.urls
            .iter()
            .all(|u| !u.contains("TESTKEYID0001") && !u.contains("TESTSECRET0001"))
    );
}

#[test]
fn a_full_page_without_a_token_is_the_last_page() {
    let (_d, archive) = archive();
    let mut http = Script::default().reply(
        200,
        page(
            vec![
                trade("2026-10-14T13:30:00Z", json!(1.5), 100, 1),
                trade("2026-10-14T13:30:01Z", json!(1.51), 100, 2),
            ],
            None,
        ),
    );
    let (r, _, _) = run(&mut http, &archive, &query(), false);
    assert_eq!(r.unwrap().pages, 1);
}

#[test]
fn a_session_with_no_trades_is_complete_and_empty() {
    let (_d, archive) = archive();
    let mut http = Script::default().reply(
        200,
        json!({"trades": null, "symbol": "ABCD", "next_page_token": null}),
    );
    let (r, _, _) = run(&mut http, &archive, &query(), false);
    let r = r.unwrap();
    assert_eq!((r.pages, r.empty_pages, r.events), (1, 1, 0));
    let m = archive.manifest(day()).unwrap().unwrap();
    assert!(m.symbols["ABCD"].import.as_ref().unwrap().complete);
}

#[test]
fn a_repeated_token_is_refused_rather_than_looped() {
    let (_d, archive) = archive();
    let mut http = Script::default()
        .reply(
            200,
            page(
                vec![trade("2026-10-14T13:30:00Z", json!(1.5), 100, 1)],
                Some("same"),
            ),
        )
        .reply(
            200,
            page(
                vec![trade("2026-10-14T13:30:01Z", json!(1.5), 100, 2)],
                Some("same"),
            ),
        );
    let (r, _, _) = run(&mut http, &archive, &query(), false);
    assert!(matches!(r, Err(ImportError::Pagination(_))));
    assert!(
        archive.manifest(day()).unwrap().is_none(),
        "never labelled complete"
    );
}

// --- retries -------------------------------------------------------------------------------------

#[test]
fn rate_limits_and_server_errors_are_retried_with_backoff() {
    let (_d, archive) = archive();
    let mut http = Script::default()
        .reply(503, json!({"message": "unavailable"}))
        .reply_after(429, 2)
        .network_error()
        .reply(
            200,
            page(
                vec![trade("2026-10-14T13:30:00Z", json!(1.5), 100, 1)],
                None,
            ),
        );
    let (r, sleeps, logs) = run(&mut http, &archive, &query(), false);
    assert_eq!(r.unwrap().events, 1);
    // Exponential from 100 ms, Retry-After honoured (2 s), then 400 ms.
    assert_eq!(
        sleeps,
        vec![
            Duration::from_millis(100),
            Duration::from_secs(2),
            Duration::from_millis(400)
        ]
    );
    assert_eq!(logs.iter().filter(|l| l.contains("retry")).count(), 3);
}

#[test]
fn authentication_failures_are_not_retried() {
    let (_d, archive) = archive();
    let mut http = Script::default().reply(
        401,
        json!({"code": 40110000, "message": "request is not authorized"}),
    );
    let (r, sleeps, _) = run(&mut http, &archive, &query(), false);
    match r {
        Err(ImportError::Fetch {
            page: 1,
            error: FetchError::Permanent { status: 401, .. },
        }) => {}
        other => panic!("{other:?}"),
    }
    assert!(sleeps.is_empty());
    assert_eq!(http.urls.len(), 1);
    assert!(archive.manifest(day()).unwrap().is_none());
}

#[test]
fn transient_failures_give_up_after_the_attempt_limit() {
    let (_d, archive) = archive();
    let mut http = Script::default()
        .reply(503, json!({}))
        .reply(502, json!({}))
        .reply(504, json!({}))
        .reply(500, json!({}));
    let (r, sleeps, _) = run(&mut http, &archive, &query(), false);
    assert!(matches!(
        r,
        Err(ImportError::Fetch {
            error: FetchError::Exhausted { attempts: 4, .. },
            ..
        })
    ));
    assert_eq!(sleeps.len(), 3);
}

#[test]
fn a_malformed_page_fails_the_import() {
    let (_d, archive) = archive();
    let mut http = Script::default().reply(
        200,
        json!({"trades": [{"t": "yesterday", "x": "V", "p": 1.0, "s": 1, "i": 1}]}),
    );
    let (r, _, _) = run(&mut http, &archive, &query(), false);
    assert!(matches!(r, Err(ImportError::Malformed { page: 1, .. })));
}

// --- resume and idempotency ------------------------------------------------------------------------

#[test]
fn an_interrupted_import_is_not_labelled_and_a_rerun_completes_then_is_skipped() {
    let (_d, archive) = archive();
    // Page 2 fails permanently: nothing is committed, the partial file is left as evidence.
    let mut http = Script::default()
        .reply(
            200,
            page(
                vec![trade("2026-10-14T13:30:00Z", json!(1.5), 100, 1)],
                Some("p2"),
            ),
        )
        .reply(403, json!({"message": "forbidden"}));
    let (r, _, _) = run(&mut http, &archive, &query(), false);
    assert!(matches!(r, Err(ImportError::Fetch { page: 2, .. })));
    assert!(archive.manifest(day()).unwrap().is_none());
    assert!(archive.day_dir(day()).join("ABCD.swa.partial").is_file());

    // The re-run starts over, replaces the partial and completes.
    let mut http = three_pages();
    let (r, _, _) = run(&mut http, &archive, &query(), false);
    assert_eq!(r.unwrap().events, 3);
    assert!(!archive.day_dir(day()).join("ABCD.swa.partial").exists());
    let first = archive.manifest(day()).unwrap().unwrap().symbols["ABCD"].clone();

    // The same request again fetches nothing.
    let mut http = Script::default();
    let (r, _, _) = run(&mut http, &archive, &query(), false);
    let r = r.unwrap();
    assert!(r.skipped);
    assert_eq!(r.events, 3);
    assert!(http.urls.is_empty());
    assert_eq!(
        archive.manifest(day()).unwrap().unwrap().symbols["ABCD"],
        first
    );
}

#[test]
fn a_different_request_needs_force_to_replace_an_import() {
    let (_d, archive) = archive();
    run(&mut three_pages(), &archive, &query(), false)
        .0
        .unwrap();
    let mut other = query();
    other.start = Utc.with_ymd_and_hms(2026, 10, 14, 13, 30, 0).unwrap();
    let (r, _, _) = run(&mut Script::default(), &archive, &other, false);
    assert!(matches!(r, Err(ImportError::Occupied(_))));
    let mut http = Script::default().reply(
        200,
        page(
            vec![trade("2026-10-14T13:30:00Z", json!(1.5), 100, 1)],
            None,
        ),
    );
    let (r, _, _) = run(&mut http, &archive, &other, true);
    assert_eq!(r.unwrap().events, 1);
    let p = archive.manifest(day()).unwrap().unwrap().symbols["ABCD"]
        .import
        .clone()
        .unwrap();
    assert_eq!(p.start, "2026-10-14T13:30:00Z");
}

// --- field preservation, ordering, updates ---------------------------------------------------------

#[test]
fn every_source_field_is_preserved_and_no_receive_time_is_invented() {
    let raw: RawTrade = serde_json::from_value(json!({
        "t": "2026-10-14T13:30:00.123456789Z", "x": "D", "p": 0.4512, "s": 37,
        "c": ["@", "I"], "i": 52983525029461u64, "z": "C", "u": "corrected"
    }))
    .unwrap();
    let t = to_trade(&raw, "ABCD", market_events::Source::AlpacaSip).unwrap();
    assert_eq!(
        t.event_ts.0,
        Utc.with_ymd_and_hms(2026, 10, 14, 13, 30, 0)
            .unwrap()
            .timestamp_nanos_opt()
            .unwrap()
            + 123_456_789
    );
    assert_eq!(t.price, "0.4512".parse::<Price>().unwrap());
    assert_eq!(
        (t.size, t.source_seq, t.exchange.as_str(), t.tape),
        (37, 52983525029461, "D", Tape::C)
    );
    assert_eq!(t.conditions, vec!["@", "I"]);
    assert_eq!(t.update.as_deref(), Some("corrected"));
    assert_eq!(t.receive_ts, None);
    assert_eq!(t.session.unwrap().to_string(), "2026-10-14/regular");
}

#[test]
fn time_going_backwards_is_counted_and_kept_in_arrival_order() {
    let (_d, archive) = archive();
    let mut http = Script::default().reply(
        200,
        page(
            vec![
                trade("2026-10-14T13:30:02Z", json!(1.5), 100, 2),
                trade("2026-10-14T13:30:01Z", json!(1.5), 100, 1),
            ],
            None,
        ),
    );
    let r = run(&mut http, &archive, &query(), false).0.unwrap();
    assert_eq!(r.out_of_order, 1);
    let seqs: Vec<u64> = archive
        .reader(day(), "ABCD")
        .unwrap()
        .map(|e| match e.unwrap().event {
            MarketEvent::Trade(t) => t.source_seq,
            _ => 0,
        })
        .collect();
    assert_eq!(seqs, vec![2, 1], "the archive never re-sorts");
}

#[test]
fn canceled_and_incorrect_prints_are_kept_raw_and_never_counted() {
    let (_d, archive) = archive();
    let mut cancelled = trade("2026-10-14T13:30:01Z", json!(9.99), 5000, 2);
    cancelled["u"] = json!("canceled");
    let mut wrong = trade("2026-10-14T13:30:02Z", json!(0.01), 5000, 3);
    wrong["u"] = json!("incorrect");
    let mut fixed = trade("2026-10-14T13:30:03Z", json!(1.52), 200, 4);
    fixed["u"] = json!("corrected");
    let mut http = Script::default().reply(
        200,
        page(
            vec![
                trade("2026-10-14T13:30:00Z", json!(1.5), 100, 1),
                cancelled,
                wrong,
                fixed,
            ],
            None,
        ),
    );
    let r = run(&mut http, &archive, &query(), false).0.unwrap();
    assert_eq!(r.updates.get("canceled"), Some(&1));
    assert_eq!(r.updates.get("incorrect"), Some(&1));
    assert_eq!(r.updates.get("corrected"), Some(&1));
    let p = archive.manifest(day()).unwrap().unwrap().symbols["ABCD"]
        .import
        .clone()
        .unwrap();
    assert_eq!(p.updates.len(), 3);

    let (s, snaps) = replay_day(&archive, day(), "ABCD").unwrap();
    let m1 = snaps.iter().find(|s| s.timeframe == Timeframe::M1).unwrap();
    assert_eq!(m1.candles.len(), 1);
    let c = &m1.candles[0];
    // Only the regular and the corrected print count: no 9.99 high, no 0.01 low, no 10,000 shares.
    assert_eq!((c.o, c.h, c.l, c.c, c.v), (1.5, 1.52, 1.5, 1.52, 300));
    assert_eq!(s.condition_excluded, 2);
}

// --- archive integrity and replay -------------------------------------------------------------------

#[test]
fn the_imported_file_verifies_and_reads_back_with_its_provenance() {
    let (_d, archive) = archive();
    run(&mut three_pages(), &archive, &query(), false)
        .0
        .unwrap();
    let rep = archive.verify(day()).unwrap();
    assert!(rep.ok(), "{rep:?}");
    assert!(rep.unlisted.is_empty());
    let m = archive.manifest(day()).unwrap().unwrap();
    assert!(!m.fixture);
    assert_eq!(m.sources, vec![market_events::Source::AlpacaSip]);
    let e = &m.symbols["ABCD"];
    assert_eq!((e.events, e.trades), (3, 3));
    let p = e.import.as_ref().unwrap();
    assert_eq!(
        (
            p.provider.as_str(),
            p.dataset.as_str(),
            p.feed.as_str(),
            p.pages,
            p.complete
        ),
        ("alpaca", "trades", "sip", 3, true)
    );
    assert_eq!(p.endpoint, "/v2/stocks/ABCD/trades");
    let back: Vec<MarketEvent> = archive
        .reader(day(), "ABCD")
        .unwrap()
        .map(|r| r.unwrap().event)
        .collect();
    assert_eq!(back.len(), 3);
}

#[test]
fn the_imported_log_replays_deterministically_at_every_timeframe() {
    let (_d, archive) = archive();
    // A few minutes of regular-session prints, plus one in premarket.
    let mut trades = vec![trade("2026-10-14T12:00:00Z", json!(0.44), 300, 1)];
    let mut seq = 10u64;
    for minute in 0..5 {
        for sec in [0u32, 14, 29, 44, 59] {
            seq += 1;
            let p = 0.45 + 0.001 * f64::from(minute) + 0.0001 * f64::from(sec % 3);
            trades.push(trade(
                &format!("2026-10-14T13:3{minute}:{sec:02}Z"),
                json!((p * 10_000.0).round() / 10_000.0),
                100 + seq,
                seq,
            ));
        }
    }
    let mut http = Script::default().reply(200, page(trades, None));
    run(&mut http, &archive, &query(), false).0.unwrap();
    let (s, snaps) = replay_day(&archive, day(), "ABCD").unwrap();
    assert!(s.deterministic);
    assert_eq!(snaps.len(), 4, "15s, 30s, 1m and 5m");
    let vol = |tf| s.timeframes.iter().find(|t| t.tf == tf).unwrap().volume;
    // Every timeframe is built from the same prints.
    assert_eq!(vol(Timeframe::S15), vol(Timeframe::M5));
    assert_eq!(vol(Timeframe::S30), vol(Timeframe::M1));
    assert_eq!(
        s.timeframes
            .iter()
            .find(|t| t.tf == Timeframe::M1)
            .unwrap()
            .candles,
        6,
        "five regular minutes and the premarket one"
    );
    assert_eq!(
        s.timeframes
            .iter()
            .find(|t| t.tf == Timeframe::S15)
            .unwrap()
            .candles,
        21,
        "twenty regular quarter-minutes and the premarket one"
    );
    // The profile is the regular session's: the premarket print is not in it.
    assert_eq!(s.profile_anchor.as_deref(), Some("2026-10-14/regular"));
    assert_eq!(s.profile_total, vol(Timeframe::M5) - 300);
    // Historical trades carry no statuses: no halts are derived.
    assert_eq!((s.statuses, s.halts), (0, 0));
}

// --- FMP minute bars ----------------------------------------------------------------------------------

fn fmp_rows() -> serde_json::Value {
    // FMP lists newest first, in exchange (ET) time.
    json!([
        {"date": "2026-10-14 09:31:00", "open": 0.46, "low": 0.455, "high": 0.47, "close": 0.4651, "volume": 1200},
        {"date": "2026-10-14 09:30:00", "open": 0.45, "low": 0.449, "high": 0.4513, "close": 0.4513, "volume": 137},
        {"date": "2026-10-13 15:59:00", "open": 0.5, "low": 0.5, "high": 0.5, "close": 0.5, "volume": 10}
    ])
}

fn fetch(http: &mut Script) -> (Result<market_archive::BarDataset, FmpError>, Vec<String>) {
    let mut logs = Vec::new();
    let r = fetch_minute_bars(
        http,
        "https://fmp.example",
        &FmpKey("FMPSECRETKEY42".into()),
        "ABCD",
        day(),
        no_retry_wait(),
        &mut |_| {},
        &mut |l| logs.push(l.to_string()),
    );
    (r, logs)
}

#[test]
fn fmp_bars_are_stored_as_their_own_dataset_never_as_events() {
    let (_d, archive) = archive();
    let mut http = Script::default().reply(200, fmp_rows());
    let ds = fetch(&mut http).0.unwrap();
    assert_eq!(
        http.urls[0],
        "https://fmp.example/stable/historical-chart/1min?symbol=ABCD&from=2026-10-14&to=2026-10-14&apikey=FMPSECRETKEY42"
    );
    assert_eq!(
        (ds.provider.as_str(), ds.granularity.as_str()),
        ("fmp", "1min")
    );
    assert!(!ds.query.contains("FMPSECRETKEY42") && !ds.query.contains("apikey"));
    // Only the requested date, ascending, 09:30 ET = 13:30Z.
    assert_eq!(ds.bars.len(), 2);
    assert_eq!(
        ds.bars[0].t,
        Utc.with_ymd_and_hms(2026, 10, 14, 13, 30, 0)
            .unwrap()
            .timestamp_nanos_opt()
            .unwrap()
    );
    assert_eq!(ds.bars[0].h, 451_300);

    let e = archive.write_bars(day(), &ds).unwrap();
    assert_eq!(e.file, "bars/fmp-1min/ABCD.json");
    let m = archive.manifest(day()).unwrap().unwrap();
    assert!(m.symbols.is_empty(), "bars never become an event file");
    assert!(m.bars.contains_key(&BarEntry::key("fmp", "1min", "ABCD")));
    assert!(archive.verify(day()).unwrap().ok());
    assert!(archive.verify(day()).unwrap().unlisted.is_empty());
    assert!(
        replay_day(&archive, day(), "ABCD").is_err(),
        "nothing to replay from bars alone"
    );
    assert_eq!(
        archive
            .read_bars(day(), "fmp", "1min", "ABCD")
            .unwrap()
            .unwrap(),
        ds
    );
}

#[test]
fn fmp_plan_limits_are_reported_and_the_key_never_appears() {
    for (status, body) in [
        (
            402,
            json!({"Error Message": "Premium endpoint. Upgrade your plan."}),
        ),
        (
            403,
            json!({"Error Message": "Forbidden for key FMPSECRETKEY42"}),
        ),
        (
            200,
            json!({"Error Message": "Exclusive endpoint for FMPSECRETKEY42"}),
        ),
    ] {
        let mut http = Script::default().reply(status, body);
        let (r, logs) = fetch(&mut http);
        let err = r.unwrap_err();
        let text = err.to_string();
        assert!(
            matches!(err, FmpError::NotEntitled { .. } | FmpError::Fetch(_)),
            "{text}"
        );
        assert!(!text.contains("FMPSECRETKEY42"), "{text}");
        assert!(logs.iter().all(|l| !l.contains("FMPSECRETKEY42")));
    }
}

#[test]
fn fmp_retry_logs_show_a_redacted_url() {
    let mut http = Script::default()
        .reply(503, json!({}))
        .reply(200, fmp_rows());
    let (r, logs) = fetch(&mut http);
    assert!(r.is_ok());
    assert!(logs[0].contains("apikey=REDACTED"), "{}", logs[0]);
    assert!(logs.iter().all(|l| !l.contains("FMPSECRETKEY42")));
}

#[test]
fn redaction_helpers() {
    assert_eq!(
        redact_url("https://x/y?symbol=A&apikey=SECRET&page_token=abc&token=T"),
        "https://x/y?symbol=A&apikey=REDACTED&page_token=abc&token=REDACTED"
    );
    assert_eq!(
        scrub("key SECRET1 and SECRET1", &["SECRET1"]),
        "key REDACTED and REDACTED"
    );
    assert_eq!(
        format!("{:?}", creds()),
        "AlpacaCredentials { key_id: REDACTED, secret: REDACTED }"
    );
    assert_eq!(format!("{:?}", FmpKey("abc".into())), "FmpKey(REDACTED)");
}

#[test]
fn minute_comparison_is_diagnostic_not_forced_equal() {
    let (_d, archive) = archive();
    let mut http = Script::default().reply(
        200,
        page(
            vec![
                trade("2026-10-14T13:30:00.1Z", json!(0.45), 100, 1),
                trade("2026-10-14T13:30:30Z", json!(0.4513), 37, 2),
                trade("2026-10-14T13:32:00Z", json!(0.47), 100, 3),
            ],
            None,
        ),
    );
    run(&mut http, &archive, &query(), false).0.unwrap();
    let (_, snaps) = replay_day(&archive, day(), "ABCD").unwrap();
    let m1 = snaps.iter().find(|s| s.timeframe == Timeframe::M1).unwrap();
    let mut http = Script::default().reply(200, fmp_rows());
    let ds = fetch(&mut http).0.unwrap();
    let c = compare_minutes(m1, &ds.bars);
    assert_eq!(
        (
            c.alpaca_minutes,
            c.fmp_minutes,
            c.both,
            c.only_alpaca,
            c.only_fmp
        ),
        (2, 2, 1, 1, 1)
    );
    // 09:30: both close at 0.4513; FMP's low (0.449) and volume (137) differ from the trades.
    assert_eq!((c.close_equal, c.ohlc_equal), (1, 0));
    assert_eq!((c.alpaca_volume, c.fmp_volume), (137, 137));
    assert_eq!(c.examples.len(), 1);
}

#[test]
fn feed_selection_maps_to_distinct_sources() {
    use market_events::Source;
    assert_eq!(Feed::parse("SIP").unwrap().source(), Source::AlpacaSip);
    assert_eq!(Feed::parse("iex").unwrap().source(), Source::AlpacaIex);
    assert_eq!(Feed::parse("boats").unwrap().source(), Source::AlpacaBoats);
    assert!(Feed::parse("delayed_sip").is_err());
    let _ = fmp::GRANULARITY;
}

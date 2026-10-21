//! `swimport maintain` cycles against a URL-routed fake Alpaca and a fake server: no network, no
//! keys. The window, schedule and prune rules themselves are unit-tested in `import::maintain`.

use chrono::{DateTime, NaiveDate, Utc};
use market_archive::fixture::{DEFAULT_SEED, write_fixture_day};
use market_archive::{Archive, ArchiveConfig};
use market_providers::import::alpaca::AlpacaCredentials;
use market_providers::import::http::{Http, HttpResponse};
use market_providers::import::maintain::{
    CycleReport, MaintainConfig, Maintainer, missing_days, read_status, status_path, window,
    write_status,
};
use serde_json::json;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

fn d(s: &str) -> NaiveDate {
    s.parse().unwrap()
}

fn utc(s: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
}

fn ok(v: serde_json::Value) -> Result<HttpResponse, String> {
    Ok(HttpResponse {
        status: 200,
        body: serde_json::to_vec(&v).unwrap(),
        retry_after: None,
    })
}

/// What the fakes answer, shared by every client a cycle builds.
#[derive(Default)]
struct World {
    /// FAIL's trades answer 403 while set.
    fail: AtomicBool,
    /// Daily-bars requests answer 503 while set (a transient provider failure).
    bars_fail: AtomicBool,
    /// The day the server replays; `None` = the server is down.
    replaying: Mutex<Option<Option<&'static str>>>,
    /// Every trades request, by symbol and day.
    trade_requests: Mutex<Vec<String>>,
    probes: AtomicUsize,
    /// Every daily-bars and corporate-actions request.
    data_requests: Mutex<Vec<String>>,
}

#[derive(Clone)]
struct Router(Arc<World>);

impl Http for Router {
    fn get(&mut self, url: &str, headers: &[(&str, &str)]) -> Result<HttpResponse, String> {
        let w = &self.0;
        if let Some(path) = url.strip_prefix("http://server:8790") {
            assert_eq!(path, "/api/replay/state");
            assert!(headers.contains(&("Authorization", "Bearer test-token")));
            w.probes.fetch_add(1, Ordering::Relaxed);
            return match *w.replaying.lock().unwrap() {
                None => Err("connection refused".into()),
                Some(day) => ok(json!({"mode": "replay", "day": day})),
            };
        }
        if url.contains("/v2/assets") {
            return ok(json!([
                {"symbol": "AAA", "tradable": true, "name": "Aaa Inc", "exchange": "NASDAQ"},
                {"symbol": "BBB", "tradable": true, "name": "Bbb Corp", "exchange": "NYSE"},
                {"symbol": "FAIL", "tradable": true, "name": "Fail Ltd", "exchange": "NYSE"}
            ]));
        }
        if url.contains("/v1/corporate-actions") {
            w.data_requests.lock().unwrap().push(url.to_string());
            return Ok(HttpResponse {
                status: 200,
                body: include_bytes!("fixtures/alpaca-corporate-actions-BTOG.json").to_vec(),
                retry_after: None,
            });
        }
        if url.contains("/v2/stocks/bars") {
            w.data_requests.lock().unwrap().push(url.to_string());
            if w.bars_fail.load(Ordering::Relaxed) {
                return Ok(HttpResponse {
                    status: 503,
                    body: b"{\"message\":\"upstream\"}".to_vec(),
                    retry_after: None,
                });
            }
            return ok(json!({"bars": {
                "AAA":  [{"t": "2026-10-19T04:00:00Z", "c": 1.0, "n": 2},
                         {"t": "2026-10-20T04:00:00Z", "c": 1.1, "n": 2}],
                "BBB":  [{"t": "2026-10-20T04:00:00Z", "c": 2.0, "n": 2}],
                "FAIL": [{"t": "2026-10-20T04:00:00Z", "c": 3.0, "n": 1}]
            }, "next_page_token": null}));
        }
        if let Some(rest) = url.split("/v2/stocks/").nth(1)
            && let Some(sym) = rest.split('/').next()
        {
            let day = if url.contains("start=2026-10-19") {
                "2026-10-19"
            } else {
                "2026-10-20"
            };
            w.trade_requests
                .lock()
                .unwrap()
                .push(format!("{sym} {day}"));
            if sym == "FAIL" && w.fail.load(Ordering::Relaxed) {
                return Ok(HttpResponse {
                    status: 403,
                    body: b"{\"message\":\"forbidden\"}".to_vec(),
                    retry_after: None,
                });
            }
            return ok(json!({"trades": [
                {"t": format!("{day}T13:30:00Z"), "x": "V", "p": 1.0, "s": 100, "c": ["@"], "i": 1, "z": "C"}
            ], "symbol": sym, "next_page_token": null}));
        }
        Err(format!("unexpected url {url}"))
    }
}

fn archive(dir: &std::path::Path, min_free_mb: &str) -> Archive {
    let root = dir.join("archive").to_string_lossy().into_owned();
    let min = min_free_mb.to_string();
    Archive::new(
        ArchiveConfig::from_lookup(|k| match k {
            "ARCHIVE_ROOT" => Some(root.clone()),
            "ARCHIVE_MIN_FREE_MB" => Some(min.clone()),
            _ => None,
        })
        .unwrap(),
    )
}

fn config() -> MaintainConfig {
    MaintainConfig {
        server_url: "http://server:8790".into(),
        api_token: Some("test-token".into()),
        data_base: "https://data.example".into(),
        trading_base: "https://trading.example".into(),
        retries: 2,
        jobs: 2,
        // 22, not the default 21: these scenarios were written around 2026-09-21 ..= 10-20 (22
        // trading days). They test the cycle (import, retries, prune safety), not the length,
        // which the unit tests pin to 21.
        window_days: 22,
        ..MaintainConfig::default()
    }
}

fn creds() -> AlpacaCredentials {
    AlpacaCredentials {
        key_id: "K".into(),
        secret: "S".into(),
    }
}

/// Two old fixture days outside the window that starts 2026-09-21, and an old close.
fn seed(a: &Archive) {
    for day in ["2026-09-17", "2026-09-18"] {
        write_fixture_day(a, d(day), &["SMPL".to_string()], DEFAULT_SEED).unwrap();
    }
    let mut closes = market_archive::DailyCloses::new();
    closes
        .entry(d("2026-09-10"))
        .or_default()
        .insert("AAA".into(), 0.9);
    a.write_daily_closes(&closes).unwrap();
}

/// Tuesday 2026-10-20, after the run time: L = 10-20, window 2026-09-21 ..= 2026-10-20.
const NOW: &str = "2026-10-20T22:00:00-04:00";

struct Run {
    report: CycleReport,
    waits: Vec<Duration>,
}

fn cycle(world: &Arc<World>, a: &Archive, creds: Option<&AlpacaCredentials>, stop: bool) -> Run {
    let cfg = config();
    let make = {
        let w = world.clone();
        move || Ok::<_, String>(Router(w.clone()))
    };
    let stop = AtomicBool::new(stop);
    let waits = Mutex::new(Vec::new());
    let wait = |t: Duration| {
        waits.lock().unwrap().push(t);
        true
    };
    let m = Maintainer {
        cfg: &cfg,
        creds,
        archive: a,
        make_http: &make,
        stop: &stop,
        wait: &wait,
        log: &|_l: &str| {},
    };
    let report = m.run_cycle(utc(NOW));
    Run {
        report,
        waits: waits.into_inner().unwrap(),
    }
}

#[test]
fn a_cycle_fills_the_window_holds_a_failing_day_back_and_only_then_prunes() {
    let tmp = tempfile::tempdir().unwrap();
    let a = archive(tmp.path(), "0");
    seed(&a);
    let world = Arc::new(World::default());
    world.fail.store(true, Ordering::Relaxed);
    *world.replaying.lock().unwrap() = Some(Some("2026-09-18"));
    let w = window(d("2026-10-20"), 22);
    assert_eq!((w.first, w.days.len()), (d("2026-09-21"), 22));
    assert_eq!(missing_days(&w, &a).len(), 22, "nothing of the window yet");
    // The live market recorded 10-20 during the session (provisional).
    let mut rec = market_archive::LiveWriter::open(&a, d("2026-10-20"), 0, "test").unwrap();
    rec.append(&market_events::MarketEvent::Trade(
        market_events::Trade::new(
            "AAA",
            market_events::session::et(d("2026-10-20"), 10, 0, 0, 0).unwrap(),
            market_events::Price::from_cents(500),
            100,
            1,
        ),
    ))
    .unwrap();
    rec.finish().unwrap();
    assert_eq!(
        a.day_data(d("2026-10-20")),
        Some(market_archive::DayData::Provisional { sealed: false })
    );

    // 1. FAIL is refused: one of 10-20's three symbol-days fails on every pass.
    let r = cycle(&world, &a, Some(&creds()), false);
    let rep = &r.report;
    assert_eq!(rep.outcome, "partial");
    assert_eq!(rep.latest_completed_day, Some(d("2026-10-20")));
    assert_eq!(rep.imported_symbol_days, 3);
    assert_eq!(
        rep.imported_by_day
            .iter()
            .map(|(d, n)| (d.to_string(), *n))
            .collect::<Vec<_>>(),
        [("2026-10-19".to_string(), 1), ("2026-10-20".to_string(), 2)]
    );
    assert_eq!(rep.failed, 1);
    assert_eq!(rep.failure_sample[0].symbol, "FAIL");
    assert_eq!(
        r.waits,
        [Duration::from_secs(120), Duration::from_secs(240)],
        "two retry passes, the delay doubling"
    );
    let fail_requests = |w: &World| {
        w.trade_requests
            .lock()
            .unwrap()
            .iter()
            .filter(|s| s.starts_with("FAIL"))
            .count()
    };
    assert_eq!(fail_requests(&world), 3, "the first pass and two retries");
    // 10-19 is complete and visible; 10-20 (a third failed, far above 1%) stays hidden.
    assert_eq!(rep.still_importing, [d("2026-10-20")]);
    assert!(a.is_importing(d("2026-10-20")));
    // Half imported: readers still take the recording, and the recorder may not add to it.
    assert_eq!(
        a.day_data(d("2026-10-20")),
        Some(market_archive::DayData::Provisional { sealed: false })
    );
    assert!(!a.accepts_live(d("2026-10-20")));
    assert_eq!(
        a.ready_days().unwrap(),
        [d("2026-09-17"), d("2026-09-18"), d("2026-10-19")]
    );
    // Not caught up to 10-20: the old days are kept, and the server was not even asked.
    assert!(rep.pruned.is_empty());
    assert!(
        rep.prune_skipped
            .as_deref()
            .unwrap()
            .contains("not caught up")
    );
    assert_eq!(world.probes.load(Ordering::Relaxed), 0);
    // Metadata: assets written, closes merged and the 09-10 close trimmed (the earliest archived
    // day is 09-17, so closes from 09-16 on are kept).
    assert_eq!(rep.assets, Some(3));
    assert_eq!(a.read_assets().unwrap().unwrap()["AAA"].name, "Aaa Inc");
    assert_eq!(rep.closes_trimmed, 1);
    let closes = a.read_daily_closes().unwrap().unwrap();
    assert_eq!(
        closes.keys().copied().collect::<Vec<_>>(),
        [d("2026-10-19"), d("2026-10-20")]
    );
    assert_eq!(closes[&d("2026-10-20")]["BBB"], 2.0);
    // The same responses' whole bars are kept per date, with who was covered but had no bar.
    assert_eq!(rep.bars_dates_written, 2);
    assert_eq!(
        a.raw_bar_dates().unwrap(),
        [d("2026-10-19"), d("2026-10-20")]
    );
    let raw19 = a.read_raw_bars(d("2026-10-19")).unwrap().unwrap();
    assert_eq!(raw19.bars["AAA"].c, 1.0);
    assert_eq!(raw19.bars["AAA"].n, Some(2));
    assert_eq!(
        raw19.no_bar.iter().map(String::as_str).collect::<Vec<_>>(),
        ["BBB", "FAIL"]
    );
    assert_eq!(raw19.fetches[0].command, "maintain");
    // Backfill: the retention window's dates older than the refresh (it starts 09-18) were asked
    // for once, raw; then the watch list's split-adjusted snapshot over 64 dates, as one set.
    let reqs = world.data_requests.lock().unwrap().clone();
    let keep_first = window(d("2026-10-20"), 63).first;
    assert!(
        reqs.iter().any(|u| u.contains("adjustment=raw")
            && u.contains(&format!("start={keep_first}&end=2026-09-17"))),
        "{reqs:#?}"
    );
    let split_first = window(d("2026-10-20"), 64).first;
    let split: Vec<&String> = reqs
        .iter()
        .filter(|u| u.contains("adjustment=split"))
        .collect();
    assert_eq!(split.len(), 1);
    assert!(split[0].contains(&format!("start={split_first}&end=2026-10-20")));
    assert!(
        split[0].contains("symbols=AAA%2CBBB%2CFAIL"),
        "watch list = in-band raw closes"
    );
    assert_eq!((rep.split_snapshots, rep.split_requests), (1, 1));
    let snaps = a.read_split_snapshots().unwrap();
    assert_eq!(snaps.len(), 1);
    assert!(snaps[0].set.is_some());
    world.data_requests.lock().unwrap().clear();
    let failures: serde_json::Value = serde_json::from_slice(
        &std::fs::read(a.root().join("plans/maintainer-failures.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(failures[0]["symbol"], "FAIL");

    // 2. The next cycle: FAIL works now. Only it is fetched; the day is released; caught up, so
    //    the old days go, except the one the server is replaying.
    world.fail.store(false, Ordering::Relaxed);
    world.trade_requests.lock().unwrap().clear();
    let assets_mtime = std::fs::metadata(a.assets_path())
        .unwrap()
        .modified()
        .unwrap();
    let closes_mtime = std::fs::metadata(a.daily_closes_path())
        .unwrap()
        .modified()
        .unwrap();
    let r = cycle(&world, &a, Some(&creds()), false);
    let rep = &r.report;
    assert_eq!(rep.outcome, "ok", "{:?}", rep.errors);
    assert_eq!(
        *world.trade_requests.lock().unwrap(),
        ["FAIL 2026-10-20"],
        "completed symbol-days are never re-fetched"
    );
    assert_eq!((rep.imported_symbol_days, rep.already_complete), (1, 3));
    assert!(rep.still_importing.is_empty());
    // Released: the import replaced the recording in one step.
    assert_eq!(
        a.day_data(d("2026-10-20")),
        Some(market_archive::DayData::Final)
    );
    assert!(!a.live_dir(d("2026-10-20")).exists());
    assert!(r.waits.is_empty());
    assert_eq!(rep.pruned, [d("2026-09-17")]);
    assert_eq!(rep.protected_day, Some(d("2026-09-18")));
    assert_eq!(
        a.ready_days().unwrap(),
        [d("2026-09-18"), d("2026-10-19"), d("2026-10-20")]
    );
    assert!(!a.root().join(".removing-2026-09-17").exists());
    // Nothing new in either file: neither is rewritten, so the server does not reload them.
    assert_eq!((rep.closes_changed, rep.closes_trimmed), (0, 0));
    assert_eq!(
        (rep.bars_dates_written, rep.bars_pruned),
        (0, 0),
        "an identical refetch rewrites no daily-bar file"
    );
    assert_eq!(
        std::fs::metadata(a.assets_path())
            .unwrap()
            .modified()
            .unwrap(),
        assets_mtime
    );
    assert_eq!(
        std::fs::metadata(a.daily_closes_path())
            .unwrap()
            .modified()
            .unwrap(),
        closes_mtime
    );

    // 3. The replay moved on: the protected day goes on the next cycle.
    *world.replaying.lock().unwrap() = Some(Some("2026-10-20"));
    let rep = cycle(&world, &a, Some(&creds()), false).report;
    assert_eq!(rep.pruned, [d("2026-09-18")]);
    assert_eq!(rep.protected_day, None);
    assert!(
        world.trade_requests.lock().unwrap().len() == 1,
        "still nothing re-fetched"
    );
}

#[test]
fn an_unreachable_server_means_nothing_is_deleted() {
    let tmp = tempfile::tempdir().unwrap();
    let a = archive(tmp.path(), "0");
    seed(&a);
    let world = Arc::new(World::default());
    *world.replaying.lock().unwrap() = None;
    let rep = cycle(&world, &a, Some(&creds()), false).report;
    assert_eq!(rep.imported_symbol_days, 4, "the import itself goes on");
    assert!(rep.pruned.is_empty());
    let why = rep.prune_skipped.unwrap();
    assert!(
        why.contains("server unreachable") && why.contains("2 old day(s) kept"),
        "{why}"
    );
    assert!(!why.contains("test-token"));
    assert_eq!(rep.outcome, "ok", "a held-back prune is not a failure");
    assert_eq!(a.days().unwrap().len(), 4);
}

#[test]
fn without_keys_nothing_is_imported_and_the_window_does_not_shrink() {
    let tmp = tempfile::tempdir().unwrap();
    let a = archive(tmp.path(), "0");
    seed(&a);
    let world = Arc::new(World::default());
    *world.replaying.lock().unwrap() = Some(None);
    let rep = cycle(&world, &a, None, false).report;
    assert_eq!(rep.outcome, "partial");
    assert!(rep.errors[0].contains("ALPACA_API_KEY"));
    assert_eq!(rep.imported_symbol_days, 0);
    assert!(
        rep.pruned.is_empty(),
        "old days stay while nothing new arrives"
    );
    assert_eq!(a.days().unwrap(), [d("2026-09-17"), d("2026-09-18")]);
    assert_eq!(world.trade_requests.lock().unwrap().len(), 0);
}

#[test]
fn below_the_free_space_floor_old_days_are_pruned_and_nothing_is_imported() {
    let tmp = tempfile::tempdir().unwrap();
    let seeded = archive(tmp.path(), "0");
    seed(&seeded);
    // A floor no disk satisfies.
    let a = archive(tmp.path(), "999999999999");
    let world = Arc::new(World::default());
    *world.replaying.lock().unwrap() = Some(Some("2026-09-18"));
    let rep = cycle(&world, &a, Some(&creds()), false).report;
    assert_eq!(
        rep.pruned,
        [d("2026-09-17")],
        "space first, replayed day spared"
    );
    assert_eq!(rep.imported_symbol_days, 0);
    assert!(rep.errors.iter().any(|e| e.contains("import refused")));
    assert_eq!(world.trade_requests.lock().unwrap().len(), 0);
}

#[test]
fn a_stop_request_ends_the_cycle_without_importing_or_deleting() {
    let tmp = tempfile::tempdir().unwrap();
    let a = archive(tmp.path(), "0");
    seed(&a);
    let world = Arc::new(World::default());
    *world.replaying.lock().unwrap() = Some(None);
    let rep = cycle(&world, &a, Some(&creds()), true).report;
    assert_eq!(rep.outcome, "stopped");
    assert_eq!(rep.imported_symbol_days, 0);
    assert!(rep.pruned.is_empty());
}

#[test]
fn the_pre_market_announcement_fetch_is_stored_append_only_for_the_watch_list() {
    let tmp = tempfile::tempdir().unwrap();
    let a = archive(tmp.path(), "0");
    let world = Arc::new(World::default());
    // A cycle the evening before gives the raw bars the watch list is read from.
    cycle(&world, &a, Some(&creds()), false);
    world.data_requests.lock().unwrap().clear();
    let cfg = config();
    let make = {
        let w = world.clone();
        move || Ok::<_, String>(Router(w.clone()))
    };
    let stop = AtomicBool::new(false);
    let m = Maintainer {
        cfg: &cfg,
        creds: Some(&creds()),
        archive: &a,
        make_http: &make,
        stop: &stop,
        wait: &|_| true,
        log: &|_l: &str| {},
    };
    // An intraday band entrant from the day before asked for a warmup (the server's job later).
    assert!(
        a.request_warmup(&market_archive::WarmupRequest {
            symbol: "NEWB".into(),
            session_date: d("2026-10-21"),
            requested_at: utc("2026-10-21T02:00:00-04:00"),
            reason: "test".into(),
            outcome: None,
            done_at: None,
            attempts: 0,
            last_error: None,
        })
        .unwrap()
    );
    // 03:30 ET Wednesday 10-21, before the SIP start: P = 10-20.
    let rep = m.run_actions(utc("2026-10-21T03:30:00-04:00"));
    assert_eq!(rep.date, d("2026-10-21"));
    assert!(rep.success, "{:?}", rep.errors);
    assert_eq!((rep.symbols, rep.covered), (4, 4), "watch list + warmup");
    let reqs = world.data_requests.lock().unwrap().clone();
    assert!(
        reqs[0].contains("symbols=AAA%2CBBB%2CFAIL%2CNEWB")
            && reqs[0].contains("start=2026-09-21&end=2026-10-31&data_quality=all"),
        "{}",
        reqs[0]
    );
    assert!(
        reqs[1].contains("symbols=BTOG%2CPLIN%2CSGRX"),
        "the recorded page's rename counterparts are followed up: {}",
        reqs[1]
    );
    // Validation: every symbol checked, nothing relevant (BTOG's actions are someone else's).
    assert_eq!(
        rep.states.get("no_known_action_provider_checked"),
        Some(&4),
        "{:?}",
        rep.states
    );
    // BTOG's recorded actions all took effect before they were first seen here: late, recorded.
    assert_eq!(rep.late, 5);
    assert_eq!(a.read_late_actions().unwrap().len(), 5);
    assert_eq!(a.read_action_ledger().unwrap().len(), 5);
    // The warmup ran: raw bars, a snapshot and its own announcement check; the request is closed.
    assert_eq!(rep.warmed, 1);
    assert!(a.pending_warmups(d("2026-10-21"), 10).unwrap().is_empty());
    let stored = a.read_action_retrievals(d("2026-10-21")).unwrap();
    assert_eq!(stored.len(), 2);
    // Both fetches ran at the same test instant, so their order is the file-name tie-break.
    let mut commands: Vec<&str> = stored.iter().map(|r| r.command.as_str()).collect();
    commands.sort();
    assert_eq!(commands, ["maintain", "warmup"]);
    assert!(
        a.read_split_snapshots()
            .unwrap()
            .iter()
            .any(|s| s.fetch.command == "warmup")
    );
    // On a Saturday the fetch is for Monday's session.
    let rep = m.run_actions(utc("2026-10-24T07:00:00-04:00"));
    assert_eq!(rep.date, d("2026-10-26"));
}

#[test]
fn the_status_file_round_trips_and_is_replaced_atomically() {
    let tmp = tempfile::tempdir().unwrap();
    let a = archive(tmp.path(), "0");
    seed(&a);
    let world = Arc::new(World::default());
    *world.replaying.lock().unwrap() = Some(None);
    let mut rep = cycle(&world, &a, Some(&creds()), false).report;
    // run_cycle leaves a "running" status while it works; the loop writes the final one.
    rep.next_run = Some("2026-10-21T21:15:00-04:00".into());
    let path = write_status(&a, &rep).unwrap();
    assert_eq!(path, status_path(&a));
    assert_eq!(path, a.root().join("plans").join("maintainer-status.json"));
    assert!(!path.with_extension("json.tmp").exists());
    let back = read_status(&a).unwrap();
    assert_eq!(back, rep);
    let v: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(v["outcome"], "ok");
    assert_eq!(v["window"]["first"], "2026-09-21");
    assert_eq!(v["pruned"], json!(["2026-09-17", "2026-09-18"]));
    assert_eq!(v["next_run"], "2026-10-21T21:15:00-04:00");
    let line = rep.summary();
    assert!(
        line.starts_with("maintain ok: window 2026-09-21..2026-10-20 (22 days)"),
        "{line}"
    );
}

/// A warmup whose fetch fails is tried again on the next run, and only closed once its attempts
/// are spent.
///
/// `run_warmup` used to close every request with its outcome, success or not, so one transient
/// provider failure cost an intraday band entrant its baseline, its previous close and its
/// announcement coverage until the next session date — the audit's point about `.done`-ing a
/// temporary failure.
#[test]
fn a_failed_warmup_is_retried_on_the_next_run_and_then_closed() {
    let tmp = tempfile::tempdir().unwrap();
    let a = archive(tmp.path(), "0");
    let world = Arc::new(World::default());
    cycle(&world, &a, Some(&creds()), false);
    let cfg = MaintainConfig {
        warmup_max_attempts: 2,
        ..config()
    };
    let make = {
        let w = world.clone();
        move || Ok::<_, String>(Router(w.clone()))
    };
    let stop = AtomicBool::new(false);
    let m = Maintainer {
        cfg: &cfg,
        creds: Some(&creds()),
        archive: &a,
        make_http: &make,
        stop: &stop,
        wait: &|_| true,
        log: &|_l: &str| {},
    };
    let day = d("2026-10-21");
    assert!(
        a.request_warmup(&market_archive::WarmupRequest {
            symbol: "NEWB".into(),
            session_date: day,
            requested_at: utc("2026-10-21T02:00:00-04:00"),
            reason: "server:no_daily_baseline".into(),
            outcome: None,
            done_at: None,
            attempts: 0,
            last_error: None,
        })
        .unwrap()
    );

    // The provider fails the warmup's daily bars. The request stays open, with the attempt and
    // the error on it, so the next run tries again.
    world.bars_fail.store(true, Ordering::Relaxed);
    let rep = m.run_actions(utc("2026-10-21T03:30:00-04:00"));
    assert_eq!(rep.warmed, 1);
    let open = a.pending_warmups(day, 10).unwrap();
    assert_eq!(open.len(), 1, "still open: {:?}", rep.errors);
    assert_eq!(open[0].attempts, 1);
    assert!(
        open[0]
            .last_error
            .as_deref()
            .is_some_and(|e| e.contains("raw bars"))
    );
    assert!(
        rep.errors.iter().any(|e| e.contains("warmup")),
        "{:?}",
        rep.errors
    );

    // The second attempt spends the allowance: closed with the error, never retried again.
    let rep = m.run_actions(utc("2026-10-21T04:00:00-04:00"));
    assert!(
        a.pending_warmups(day, 10).unwrap().is_empty(),
        "attempts spent: {:?}",
        rep.errors
    );
    assert!(
        rep.errors.iter().any(|e| e.contains("attempts spent")),
        "and it says so: {:?}",
        rep.errors
    );

    // The provider recovers, but a request already closed is not re-run: a fresh ask is needed.
    world.bars_fail.store(false, Ordering::Relaxed);
    let rep = m.run_actions(utc("2026-10-21T04:30:00-04:00"));
    assert_eq!(rep.warmed, 0);
}

/// And a warmup that works still closes on its first run.
#[test]
fn a_successful_warmup_closes_on_its_first_run() {
    let tmp = tempfile::tempdir().unwrap();
    let a = archive(tmp.path(), "0");
    let world = Arc::new(World::default());
    cycle(&world, &a, Some(&creds()), false);
    let cfg = config();
    let make = {
        let w = world.clone();
        move || Ok::<_, String>(Router(w.clone()))
    };
    let stop = AtomicBool::new(false);
    let m = Maintainer {
        cfg: &cfg,
        creds: Some(&creds()),
        archive: &a,
        make_http: &make,
        stop: &stop,
        wait: &|_| true,
        log: &|_l: &str| {},
    };
    let day = d("2026-10-21");
    a.request_warmup(&market_archive::WarmupRequest {
        symbol: "NEWB".into(),
        session_date: day,
        requested_at: utc("2026-10-21T02:00:00-04:00"),
        reason: "server:no_daily_baseline".into(),
        outcome: None,
        done_at: None,
        attempts: 0,
        last_error: None,
    })
    .unwrap();
    let rep = m.run_actions(utc("2026-10-21T03:30:00-04:00"));
    assert_eq!(rep.warmed, 1);
    assert!(a.pending_warmups(day, 10).unwrap().is_empty());
    assert!(
        !rep.errors.iter().any(|e| e.contains("warmup")),
        "{:?}",
        rep.errors
    );
}

/// The nightly cycle derives the intraday diagnostic curves from the days already on disk,
/// bounded per run and resumable, with no provider call at all.
///
/// `stock_context::IntradayBaseline` had no producer anywhere, so the same-time and
/// recent-window RVOL measures were permanently `None` (the audit's F5/F10). These curves are
/// the diagnostic denominator only: the pillar keeps the owner's 30-session daily denominator.
#[test]
fn intraday_curves_are_derived_from_disk_bounded_and_resumable() {
    let tmp = tempfile::tempdir().unwrap();
    let a = archive(tmp.path(), "0");
    // Two fixture days with several symbols each, already on disk.
    let days = [d("2026-10-19"), d("2026-10-20")];
    let symbols: Vec<String> = ["AAAA", "BBBB", "CCCC"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    for day in days {
        write_fixture_day(&a, day, &symbols, DEFAULT_SEED).unwrap();
    }
    let world = Arc::new(World::default());
    let make = {
        let w = world.clone();
        move || Ok::<_, String>(Router(w.clone()))
    };
    let stop = AtomicBool::new(false);
    // Two curves a run, so the bound and the resume are both visible.
    let cfg = MaintainConfig {
        intraday_max_per_run: 2,
        ..config()
    };
    let m = Maintainer {
        cfg: &cfg,
        creds: None,
        archive: &a,
        make_http: &make,
        stop: &stop,
        wait: &|_| true,
        log: &|_l: &str| {},
    };
    let w = window(d("2026-10-20"), 22);

    let mut r = CycleReport::default();
    m.run_intraday_curves(&w, utc(NOW), &mut r);
    assert_eq!(r.curves_derived, 2, "the bound: {r:?}");
    assert_eq!(r.curves_pending, 4, "and what is still waiting: {r:?}");
    assert!(r.errors.is_empty(), "{:?}", r.errors);
    assert!(
        world.data_requests.lock().unwrap().is_empty()
            && world.trade_requests.lock().unwrap().is_empty(),
        "derived from disk: no provider call"
    );

    // The newest day is served first: the sessions a diagnostic reaches for.
    let newest: Vec<&String> = symbols
        .iter()
        .filter(|s| a.read_intraday_curve(days[1], s).unwrap().is_some())
        .collect();
    assert_eq!(newest.len(), 2, "the newest day first: {newest:?}");

    // Later runs pick up the rest, and nothing is derived twice.
    for _ in 0..3 {
        let mut r = CycleReport::default();
        m.run_intraday_curves(&w, utc(NOW), &mut r);
        assert!(r.errors.is_empty(), "{:?}", r.errors);
    }
    let mut r = CycleReport::default();
    m.run_intraday_curves(&w, utc(NOW), &mut r);
    assert_eq!(
        (r.curves_derived, r.curves_pending),
        (0, 0),
        "every symbol-day has one and none is re-derived: {r:?}"
    );
    for day in days {
        for s in &symbols {
            let c = a.read_intraday_curve(day, s).unwrap().expect("a curve");
            assert_eq!(c.symbol, *s);
            assert_eq!(c.date, day);
            assert_eq!(c.rules, market_archive::CURVE_RULES_VERSION);
            // A fixture day is never market evidence, and the record says so rather than
            // quietly posing as a prior session.
            assert_eq!(
                c.source.fidelity,
                market_archive::CurveFidelity::SyntheticFixture
            );
            assert!(!c.collection_complete && !c.usable_as_history());
            assert_eq!(c.unusable_reason(), Some("synthetic_fixture"));
        }
    }
}

/// With the step switched off, nothing is derived.
#[test]
fn intraday_curves_can_be_switched_off() {
    let tmp = tempfile::tempdir().unwrap();
    let a = archive(tmp.path(), "0");
    let day = d("2026-10-20");
    write_fixture_day(&a, day, &["AAAA".to_string()], DEFAULT_SEED).unwrap();
    let world = Arc::new(World::default());
    *world.replaying.lock().unwrap() = Some(None);
    let cfg = MaintainConfig {
        intraday_curves: false,
        ..config()
    };
    let make = {
        let w = world.clone();
        move || Ok::<_, String>(Router(w.clone()))
    };
    let stop = AtomicBool::new(false);
    let m = Maintainer {
        cfg: &cfg,
        creds: None,
        archive: &a,
        make_http: &make,
        stop: &stop,
        wait: &|_| true,
        log: &|_l: &str| {},
    };
    let r = m.run_cycle(utc(NOW));
    assert_eq!((r.curves_derived, r.curves_pending), (0, 0), "{r:?}");
    assert!(a.read_intraday_curve(day, "AAAA").unwrap().is_none());
}

/// A retrieval is stamped at the run's own clock, so a cycle can always read what it just wrote.
///
/// `fetch_announcements` used to stamp `retrieved_at` from `Utc::now()` inside itself, and the
/// maintainer then took `max(now)`. With a fixed `now` in the past the real clock won, so the
/// retrieval was stamped *after* the as_of the same cycle validated against and
/// `coverage_for` reported `no_usable_retrieval` — a test that passed only while the wall clock
/// was before 03:30 ET on one particular date, and started failing at 03:30 that morning.
#[test]
fn an_announcement_retrieval_is_stamped_at_the_runs_own_clock() {
    let tmp = tempfile::tempdir().unwrap();
    let a = archive(tmp.path(), "0");
    let world = Arc::new(World::default());
    cycle(&world, &a, Some(&creds()), false);
    let cfg = config();
    let make = {
        let w = world.clone();
        move || Ok::<_, String>(Router(w.clone()))
    };
    let stop = AtomicBool::new(false);
    let m = Maintainer {
        cfg: &cfg,
        creds: Some(&creds()),
        archive: &a,
        make_http: &make,
        stop: &stop,
        wait: &|_| true,
        log: &|_l: &str| {},
    };
    // An as_of far in the past: whatever the wall clock says, the stamp is this.
    let asked = utc("2026-10-21T03:30:00-04:00");
    let rep = m.run_actions(asked);
    assert!(rep.success, "{:?}", rep.errors);
    let stored = a.read_action_retrievals(d("2026-10-21")).unwrap();
    assert!(!stored.is_empty());
    assert!(
        stored.iter().all(|r| r.retrieved_at == asked),
        "stamped at the run's clock, not the wall clock: {:?}",
        stored.iter().map(|r| r.retrieved_at).collect::<Vec<_>>()
    );
    // And the same cycle can therefore use it: a decision at that instant has usable coverage.
    assert_eq!(
        rep.states.get("no_usable_retrieval"),
        None,
        "{:?}",
        rep.states
    );
}

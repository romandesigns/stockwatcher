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
    /// The day the server replays; `None` = the server is down.
    replaying: Mutex<Option<Option<&'static str>>>,
    /// Every trades request, by symbol and day.
    trade_requests: Mutex<Vec<String>>,
    probes: AtomicUsize,
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
        if url.contains("/v2/stocks/bars") {
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
    let w = window(d("2026-10-20"), 1);
    assert_eq!((w.first, w.days.len()), (d("2026-09-21"), 22));
    assert_eq!(missing_days(&w, &a).len(), 22, "nothing of the window yet");

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

//! Server integration: provider → engine → /ws and /api, driven by a fixture replay.

use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use futures_util::{SinkExt, StreamExt};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use stockwatcher_server::{AppState, Market, MarketConfig, Markets, Settings, router};
use tokio_tungstenite::tungstenite::Message;
use tower::ServiceExt;

type Ws =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// 2026-10-15 10:40:00 ET (EDT) in epoch ms.
const T1040: i64 = 1_792_075_200_000;

struct Stack {
    _tmp: tempfile::TempDir,
    app: axum::Router,
    url: String,
    /// The archive root (for tests that change it under a running server).
    root: std::path::PathBuf,
}

async fn stack(extra: &[(&str, &str)]) -> Stack {
    stack_with_assets(extra, None).await
}

/// [`stack`] with an `assets.json` written to the archive root before the market starts.
async fn stack_with_assets(extra: &[(&str, &str)], assets: Option<Value>) -> Stack {
    let files: Vec<(&str, Value)> = assets.into_iter().map(|a| ("assets.json", a)).collect();
    stack_with_files(extra, &files).await
}

/// [`stack`] with files (`assets.json`, `daily-closes.json`) written to the archive root before
/// the market starts.
async fn stack_with_files(extra: &[(&str, &str)], files: &[(&str, Value)]) -> Stack {
    stack_prepared(extra, |root| write_files(root, files)).await
}

fn write_files(root: &std::path::Path, files: &[(&str, Value)]) {
    for (name, v) in files {
        std::fs::create_dir_all(root).unwrap();
        std::fs::write(root.join(name), v.to_string()).unwrap();
    }
}

/// [`stack`] on an archive that already holds these fixture days (so SIM_AUTOFIXTURE, which only
/// fills an empty archive, writes nothing), plus `files` at its root.
async fn stack_with_days(
    extra: &[(&str, &str)],
    days: &[(&str, &[&str])],
    files: &[(&str, Value)],
) -> Stack {
    let days: Vec<(chrono::NaiveDate, Vec<String>)> = days
        .iter()
        .map(|(d, syms)| {
            (
                d.parse().unwrap(),
                syms.iter().map(|s| s.to_string()).collect(),
            )
        })
        .collect();
    stack_prepared(extra, move |root| {
        write_files(root, files);
        let archive = market_archive::Archive::new(market_archive::ArchiveConfig {
            root: root.to_path_buf(),
            retention_days: 0,
            min_free_bytes: 0,
        });
        for (day, syms) in &days {
            market_archive::fixture::write_fixture_day(
                &archive,
                *day,
                syms,
                market_archive::fixture::DEFAULT_SEED,
            )
            .unwrap();
        }
    })
    .await
}

/// [`stack`] after `prepare` ran on the (not yet created) archive root.
async fn stack_prepared(extra: &[(&str, &str)], prepare: impl FnOnce(&std::path::Path)) -> Stack {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("archive").to_string_lossy().into_owned();
    let root_path = root.clone();
    prepare(std::path::Path::new(&root));
    let mut vars: Vec<(String, String)> = vec![
        ("ARCHIVE_ROOT".into(), root),
        ("ARCHIVE_MIN_FREE_MB".into(), "0".into()),
        ("SIM_AUTOFIXTURE".into(), "1".into()),
        ("SIM_FIXTURE_SYMBOLS".into(), "SMPL,FXLO".into()),
        ("REPLAY_START".into(), "10:40".into()),
        ("REPLAY_SPEED".into(), "60".into()),
    ];
    vars.extend(extra.iter().map(|(k, v)| (k.to_string(), v.to_string())));
    let cfg = MarketConfig::from_lookup(|k| {
        vars.iter()
            .rev()
            .find(|(n, _)| n == k)
            .map(|(_, v)| v.clone())
    })
    .unwrap();
    // Both markets, as in production (main.rs): replay on the archive, live not configured.
    let (markets, _sim) = Markets::start(&cfg);
    let dir = tmp.path().join("static");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("index.html"), "<!doctype html>").unwrap();
    let app = router(
        AppState::with_markets(None, markets, Settings::in_memory()),
        &dir,
        &tmp.path().join("data"),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let served = app.clone();
    tokio::spawn(async move { axum::serve(listener, served).await.unwrap() });
    Stack {
        _tmp: tmp,
        app,
        url: format!("ws://{addr}/ws"),
        root: std::path::PathBuf::from(root_path),
    }
}

async fn connect(url: &str) -> Ws {
    let (mut ws, _) = tokio_tungstenite::connect_async(url).await.unwrap();
    send(&mut ws, json!({"type": "hello", "client": "test"})).await;
    let w = next(&mut ws, |m| m["type"] == "welcome").await;
    assert_eq!(w["type"], "welcome");
    ws
}

async fn send(ws: &mut Ws, v: Value) {
    ws.send(Message::Text(v.to_string().into())).await.unwrap();
}

/// The next frame matching `pred` (others are skipped), within 20 s.
async fn next(ws: &mut Ws, pred: impl Fn(&Value) -> bool) -> Value {
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            match ws.next().await.expect("stream ended").unwrap() {
                Message::Text(t) => {
                    let v: Value = serde_json::from_str(&t).unwrap();
                    if pred(&v) {
                        return v;
                    }
                }
                Message::Close(c) => panic!("closed: {c:?}"),
                _ => {}
            }
        }
    })
    .await
    .expect("timed out waiting for a matching frame")
}

fn is(t: &'static str) -> impl Fn(&Value) -> bool {
    move |m| m["type"] == t
}

async fn get(app: &axum::Router, uri: &str) -> (StatusCode, Value) {
    let res = app
        .clone()
        .oneshot(Request::get(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let s = res.status();
    let b = res.into_body().collect().await.unwrap().to_bytes();
    (s, serde_json::from_slice(&b).unwrap_or(Value::Null))
}

/// Total volume a snapshot accounts for: candles plus volume-only gaps.
fn total_volume(snap: &Value) -> u64 {
    let c: u64 = snap["candles"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["v"].as_u64().unwrap())
        .sum();
    let g: u64 = snap["gaps"]
        .as_array()
        .unwrap()
        .iter()
        .map(|g| g["v"].as_u64().unwrap())
        .sum();
    c + g
}

async fn wait_for_replay(ws: &mut Ws) {
    // Started (catch-up to 10:40 finished) once a clock at or past 10:40 arrives.
    next(ws, |m| {
        m["type"] == "replay.clock" && m["clock"]["now"].as_i64().unwrap_or(0) >= T1040
    })
    .await;
}

#[tokio::test]
async fn subscribe_gets_snapshot_then_live_updates_while_playing() {
    let s = stack(&[]).await;
    let mut ws = connect(&s.url).await;
    wait_for_replay(&mut ws).await;
    send(
        &mut ws,
        json!({"type":"chart.subscribe","symbol":"SMPL","timeframe":"1m","profileBin":0.01}),
    )
    .await;
    let snap = next(&mut ws, is("chart.snapshot")).await;
    assert_eq!(snap["symbol"], "SMPL");
    assert_eq!(snap["timeframe"], "1m");
    assert!(snap["candles"].as_array().unwrap().len() > 100);
    assert_eq!(snap["clock"]["mode"], "replay");
    assert_eq!(snap["profile"]["anchor"], "2026-10-15/regular");
    // The engine's own session schedule (sessions.v1): the replay day's four sessions and the
    // next trading day's, so a client filters and shades by the server's calendar.
    let ids: Vec<&str> = snap["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["id"].as_str().unwrap())
        .collect();
    assert!(ids.contains(&"2026-10-15/regular"), "{ids:?}");
    assert!(ids.contains(&"2026-10-16/overnight"), "{ids:?}");
    // Recent tape follows the snapshot, then live increments for this symbol and timeframe.
    let tape = next(&mut ws, is("tape.trade")).await;
    assert_eq!(tape["symbol"], "SMPL");
    let up = next(&mut ws, is("chart.update")).await;
    assert_eq!(
        (up["symbol"].as_str(), up["timeframe"].as_str()),
        (Some("SMPL"), Some("1m"))
    );
    assert!(up["candle"]["t"].as_i64().unwrap() >= T1040 - 60_000);
    // Nothing for other symbols or timeframes leaks into this subscription.
    for _ in 0..200 {
        let m = next(&mut ws, |m| m["type"] != "replay.clock").await;
        assert_eq!(m["symbol"], "SMPL");
        if let Some(tf) = m.get("timeframe") {
            assert_eq!(tf, "1m");
        }
    }
}

#[tokio::test]
async fn timeframe_switch_mid_replay_keeps_the_same_totals() {
    let s = stack(&[]).await;
    let mut ws = connect(&s.url).await;
    wait_for_replay(&mut ws).await;
    send(&mut ws, json!({"type":"replay.control","action":"pause"})).await;
    let paused = next(&mut ws, |m| {
        m["type"] == "replay.clock" && m["clock"]["playing"] == false
    })
    .await;
    let mut snaps = Vec::new();
    for tf in ["1m", "10s", "15s", "30s", "5m", "1h"] {
        send(
            &mut ws,
            json!({"type":"chart.subscribe","symbol":"SMPL","timeframe":tf,"profileBin":0.01}),
        )
        .await;
        let snap = next(&mut ws, is("chart.snapshot")).await;
        assert_eq!(snap["timeframe"], tf);
        assert_eq!(snap["clock"]["now"], paused["clock"]["now"], "still paused");
        snaps.push(snap);
    }
    // 1h candles sit on the ET hour, or on the 09:30 open where the hour is clipped.
    let hourly = snaps[5]["candles"].as_array().unwrap();
    assert!(!hourly.is_empty());
    assert!(
        hourly
            .iter()
            .all(|c| c["t"].as_i64().unwrap() % 1_800_000 == 0),
        "{hourly:?}"
    );
    let tens = snaps[1]["candles"].as_array().unwrap();
    assert!(tens.iter().all(|c| c["t"].as_i64().unwrap() % 10_000 == 0));
    assert!(tens.len() >= snaps[2]["candles"].as_array().unwrap().len());
    let totals: Vec<u64> = snaps.iter().map(total_volume).collect();
    assert!(totals[0] > 0);
    assert!(totals.iter().all(|t| *t == totals[0]), "{totals:?}");
    for s in &snaps[1..] {
        assert_eq!(
            s["profile"], snaps[0]["profile"],
            "profile independent of timeframe"
        );
        assert_eq!(s["quality"], snaps[0]["quality"]);
    }
    // Switching back mid-replay (playing again) still starts from a full snapshot.
    send(&mut ws, json!({"type":"replay.control","action":"play"})).await;
    send(
        &mut ws,
        json!({"type":"chart.subscribe","symbol":"SMPL","timeframe":"1m","profileBin":0.01}),
    )
    .await;
    let again = next(&mut ws, is("chart.snapshot")).await;
    assert!(total_volume(&again) >= totals[0]);
}

#[tokio::test]
async fn replay_control_round_trips() {
    let s = stack(&[]).await;
    let mut ws = connect(&s.url).await;
    wait_for_replay(&mut ws).await;
    let clock = |m: &Value, k: &str| m["clock"][k].clone();

    send(&mut ws, json!({"type":"replay.control","action":"pause"})).await;
    let c = next(&mut ws, |m| {
        m["type"] == "replay.clock" && m["clock"]["playing"] == false
    })
    .await;
    let t0 = clock(&c, "now").as_i64().unwrap();

    send(
        &mut ws,
        json!({"type":"replay.control","action":"speed","value":2}),
    )
    .await;
    let c = next(&mut ws, |m| {
        m["type"] == "replay.clock" && m["clock"]["speed"] == 2.0
    })
    .await;
    assert_eq!(clock(&c, "playing"), false);

    send(
        &mut ws,
        json!({"type":"replay.control","action":"speed","value":0}),
    )
    .await;
    next(&mut ws, |m| {
        m["type"] == "replay.clock" && m["clock"]["speed"] == 0.0
    })
    .await;

    send(&mut ws, json!({"type":"replay.control","action":"step"})).await;
    let c = next(&mut ws, |m| {
        m["type"] == "replay.clock" && m["clock"]["now"].as_i64() != Some(t0)
    })
    .await;
    let t1 = clock(&c, "now").as_i64().unwrap();
    assert!(t1 > t0);

    send(
        &mut ws,
        json!({"type":"replay.control","action":"stepInterval","value":"5m"}),
    )
    .await;
    let c = next(&mut ws, |m| {
        m["type"] == "replay.clock" && m["clock"]["now"].as_i64().is_some_and(|n| n > t1)
    })
    .await;
    assert_eq!(clock(&c, "now").as_i64().unwrap() % 300_000, 0);

    // Subscribe, then seek: subscribers get a fresh snapshot of the rebuilt state.
    send(
        &mut ws,
        json!({"type":"chart.subscribe","symbol":"SMPL","timeframe":"5m","profileBin":0.01}),
    )
    .await;
    next(&mut ws, is("chart.snapshot")).await;
    let target = T1040 + 20 * 60_000; // 11:00 ET
    send(
        &mut ws,
        json!({"type":"replay.control","action":"seek","value":target}),
    )
    .await;
    let snap = next(&mut ws, is("chart.snapshot")).await;
    assert_eq!(snap["clock"]["now"], target);
    let last = snap["candles"].as_array().unwrap().last().unwrap()["t"]
        .as_i64()
        .unwrap();
    assert!(
        last < target && last >= target - 300_000,
        "rebuilt up to the seek target"
    );

    send(&mut ws, json!({"type":"replay.control","action":"play"})).await;
    next(&mut ws, |m| {
        m["type"] == "replay.clock" && m["clock"]["playing"] == true
    })
    .await;

    send(
        &mut ws,
        json!({"type":"replay.control","action":"speed","value":1.5}),
    )
    .await;
    let e = next(&mut ws, is("error")).await;
    assert_eq!(e["code"], "bad_request");
}

#[tokio::test]
async fn other_profile_bins_are_rebinned() {
    let s = stack(&[]).await;
    let mut ws = connect(&s.url).await;
    wait_for_replay(&mut ws).await;
    send(
        &mut ws,
        json!({"type":"chart.subscribe","symbol":"SMPL","timeframe":"15s","profileBin":0.05}),
    )
    .await;
    let snap = next(&mut ws, is("chart.snapshot")).await;
    assert_eq!(snap["profile"]["bin"], 0.05);
    for r in snap["profile"]["rows"].as_array().unwrap() {
        let cents = (r["p"].as_f64().unwrap() * 100.0).round() as i64;
        assert_eq!(cents % 5, 0);
    }
    let up = next(&mut ws, |m| {
        m["type"] == "chart.update" && !m["profile"].as_array().unwrap().is_empty()
    })
    .await;
    assert_eq!(up["profileBin"], 0.05);
    let cents = (up["profile"][0]["p"].as_f64().unwrap() * 100.0).round() as i64;
    assert_eq!(cents % 5, 0);
}

/// Folds an increment's touched rows into `rows` (bin key = cents / bin cents), as a client does.
fn fold_rows(rows: &mut std::collections::BTreeMap<i64, u64>, frame: &Value) {
    for r in frame["profile"].as_array().unwrap() {
        let k = (r["p"].as_f64().unwrap() * 100.0).round() as i64;
        match r["v"].as_u64().unwrap() {
            0 => rows.remove(&k),
            v => rows.insert(k, v),
        };
    }
}

fn snapshot_rows(snap: &Value) -> std::collections::BTreeMap<i64, u64> {
    snap["profile"]["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            (
                (r["p"].as_f64().unwrap() * 100.0).round() as i64,
                r["v"].as_u64().unwrap(),
            )
        })
        .collect()
}

/// Increments that change profile rows carry the whole profile's `total`, `poc` and `valueArea`
/// at the frame's bin (crate::profile_frames), at the default bin as at any other, and a client
/// that folds the rows and takes the figures as sent ends exactly where a fresh snapshot is.
#[tokio::test]
async fn increments_carry_the_profiles_figures_at_every_bin() {
    let s = stack(&[]).await;
    let mut ws = connect(&s.url).await;
    wait_for_replay(&mut ws).await;
    let at = |id: &str, bin: f64| json!({"type":"chart.subscribe","id":id,"symbol":"SMPL","timeframe":"1m","profileBin":bin});
    send(&mut ws, at("a", 0.01)).await;
    let snap_a = next(&mut ws, is("chart.snapshot")).await;
    send(&mut ws, at("b", 0.05)).await;
    let snap_b = next(&mut ws, is("chart.snapshot")).await;
    assert_eq!(snap_b["profile"]["bin"], 0.05);
    let mut rows = [snapshot_rows(&snap_a), snapshot_rows(&snap_b)];
    let mut last: [Option<Value>; 2] = [None, None];
    let mut seen = [0, 0];
    // Play on until both bins have had a good number of row-changing frames, then pause.
    while seen[0] < 40 || seen[1] < 40 {
        let m = next(&mut ws, |m| m["type"] == "chart.update").await;
        let i = if m["profileBin"] == 0.01 { 0 } else { 1 };
        assert_eq!(
            m.get("total").is_some(),
            !m["profile"].as_array().unwrap().is_empty()
        );
        if m["profile"].as_array().unwrap().is_empty() {
            continue;
        }
        let poc = m["poc"].as_f64().unwrap();
        let (lo, hi) = (
            m["valueArea"]["low"].as_f64().unwrap(),
            m["valueArea"]["high"].as_f64().unwrap(),
        );
        assert!(lo <= poc && poc <= hi, "{m}");
        let bin_cents = if i == 0 { 1 } else { 5 };
        assert_eq!(((poc * 100.0).round() as i64) % bin_cents, 0);
        fold_rows(&mut rows[i], &m);
        last[i] = Some(m);
        seen[i] += 1;
    }
    send(&mut ws, json!({"type":"replay.control","action":"pause"})).await;
    // Every increment before the pause is on the socket ahead of the paused clock; keep folding.
    loop {
        let m = next(&mut ws, |m| {
            m["type"] == "chart.update"
                || m["type"] == "chart.remove"
                || m["type"] == "chart.gap"
                || (m["type"] == "replay.clock" && m["clock"]["playing"] == false)
        })
        .await;
        if m["type"] == "replay.clock" {
            break;
        }
        if m["profile"].as_array().unwrap().is_empty() {
            continue;
        }
        let i = if m["profileBin"] == 0.01 { 0 } else { 1 };
        fold_rows(&mut rows[i], &m);
        last[i] = Some(m);
    }
    // Fresh snapshots of the paused engine: the folded rows and the last figures sent match.
    for (i, (id, bin)) in [("c", 0.01), ("d", 0.05)].into_iter().enumerate() {
        send(&mut ws, at(id, bin)).await;
        let snap = next(&mut ws, is("chart.snapshot")).await;
        assert_eq!(rows[i], snapshot_rows(&snap), "bin {bin}");
        let l = last[i].as_ref().unwrap();
        assert_eq!(l["total"], snap["profile"]["total"], "bin {bin}");
        assert_eq!(l["poc"], snap["profile"]["poc"], "bin {bin}");
        assert_eq!(l["valueArea"], snap["profile"]["valueArea"], "bin {bin}");
    }
}

#[tokio::test]
async fn rest_routes() {
    let s = stack(&[("REPLAY_AUTOPLAY", "0")]).await;
    // Wait until the catch-up finished.
    let mut ws = connect(&s.url).await;
    wait_for_replay(&mut ws).await;

    let (st, v) = get(&s.app, "/api/symbols").await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(v["symbols"], json!(["FXLO", "SMPL"]));
    assert_eq!(v["fixture"], true);
    assert_eq!(v["mode"], "replay");
    assert_eq!(v["day"], "2026-10-15");

    let (st, v) = get(&s.app, "/api/replay/state").await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(v["status"]["state"], "running");
    assert_eq!(v["controllable"], true);
    assert_eq!(v["clock"]["now"], T1040);
    assert_eq!(v["clock"]["playing"], false);

    let (st, v) = get(&s.app, "/api/chart/smpl/snapshot?timeframe=5m&bin=0.02").await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(v["type"], "chart.snapshot");
    assert_eq!(v["timeframe"], "5m");
    assert_eq!(v["profile"]["bin"], 0.02);
    assert!(
        v["candles"]
            .as_array()
            .unwrap()
            .iter()
            .all(|c| c["t"].as_i64().unwrap() < T1040)
    );

    assert_eq!(
        get(&s.app, "/api/chart/NOPE/snapshot").await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        get(&s.app, "/api/chart/SMPL/snapshot?timeframe=2m").await.0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        get(&s.app, "/api/chart/SMPL/snapshot?bin=-1").await.0,
        StatusCode::BAD_REQUEST
    );
}

#[tokio::test]
async fn replay_alerts_serve_the_days_logged_entries() {
    let s = stack_prepared(&[("REPLAY_AUTOPLAY", "0")], |root| {
        let dir = root.join("rocket-watch");
        std::fs::create_dir_all(&dir).unwrap();
        let lines = [
            json!({"kind":"fast","v":1,"symbol":"SMPL","alert_ns":1_792_075_500_000_000_000_i64,"price":4.2,"reasons":["+12% in 60 s"]}),
            json!({"kind":"state","v":1,"symbol":"SMPL","alert_ns":1_792_075_500_000_000_000_i64}),
            json!({"kind":"alert","v":1,"symbol":"FXLO","alert_ns":1_792_075_200_000_000_000_i64,"price":1.1}),
        ];
        let text: String = lines.iter().map(|l| format!("{l}\n")).collect();
        std::fs::write(dir.join("2026-10-15.jsonl"), text).unwrap();
    })
    .await;
    let mut ws = connect(&s.url).await;
    wait_for_replay(&mut ws).await;

    let (st, v) = get(&s.app, "/api/replay/alerts?day=2026-10-15").await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(v["day"], "2026-10-15");
    assert_eq!(
        v["alerts"],
        json!([
            {"t": 1_792_075_200_000_i64, "kind": "r2", "symbol": "FXLO", "price": 1.1},
            {"t": 1_792_075_500_000_i64, "kind": "fast", "symbol": "SMPL", "price": 4.2, "reason": "+12% in 60 s"},
        ])
    );
    let (_, v) = get(&s.app, "/api/replay/alerts?day=2026-10-15&symbol=smpl").await;
    assert_eq!(v["alerts"].as_array().unwrap().len(), 1);
    // A day nothing was logged on is an empty list; a bad day is refused before it names a file.
    let (st, v) = get(&s.app, "/api/replay/alerts?day=2026-10-14").await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(v["alerts"], json!([]));
    for bad in [
        "/api/replay/alerts",
        "/api/replay/alerts?day=../../etc",
        "/api/replay/alerts?day=2026-13-01",
    ] {
        assert_eq!(get(&s.app, bad).await.0, StatusCode::BAD_REQUEST, "{bad}");
    }
}

#[tokio::test]
async fn live_mode_is_not_configured_and_refuses_replay_controls() {
    let s = stack(&[("MARKET_DATA_MODE", "live")]).await;
    let (_, v) = get(&s.app, "/api/replay/state").await;
    assert_eq!(v["mode"], "live");
    assert_eq!(v["status"]["state"], "not_configured");
    assert_eq!(v["controllable"], false);
    let (_, v) = get(&s.app, "/api/symbols").await;
    assert_eq!(v["symbols"], json!([]));
    let mut ws = connect(&s.url).await;
    send(&mut ws, json!({"type":"replay.control","action":"pause"})).await;
    let e = next(&mut ws, is("error")).await;
    assert_eq!(e["code"], "not_replay");
    send(
        &mut ws,
        json!({"type":"chart.subscribe","symbol":"SMPL","timeframe":"1m","profileBin":0.01}),
    )
    .await;
    let e = next(&mut ws, is("error")).await;
    assert_eq!(e["code"], "unknown_symbol");
}

#[tokio::test]
async fn each_connection_and_route_picks_its_market() {
    // Default replay; the live market runs beside it (not configured here: no credentials).
    let s = stack(&[]).await;
    assert_eq!(get(&s.app, "/api/replay/state").await.1["mode"], "replay");
    assert_eq!(
        get(&s.app, "/api/replay/state?source=replay").await.1["mode"],
        "replay"
    );
    let (_, v) = get(&s.app, "/api/replay/state?source=live").await;
    assert_eq!(v["mode"], "live");
    assert_eq!(v["status"]["state"], "not_configured");
    assert_eq!(
        get(&s.app, "/api/symbols?source=live").await.1["symbols"],
        json!([])
    );
    assert_eq!(
        get(&s.app, "/api/chart/SMPL/snapshot?source=live").await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        get(&s.app, "/api/symbols?source=paper").await.0,
        StatusCode::BAD_REQUEST
    );

    // A live socket: no replay controls, no archive symbols.
    let (mut live, _) = tokio_tungstenite::connect_async(&s.url).await.unwrap();
    send(
        &mut live,
        json!({"type": "hello", "client": "test", "source": "live"}),
    )
    .await;
    next(&mut live, is("welcome")).await;
    send(&mut live, json!({"type":"replay.control","action":"pause"})).await;
    assert_eq!(next(&mut live, is("error")).await["code"], "not_replay");
    send(
        &mut live,
        json!({"type":"chart.subscribe","symbol":"SMPL","timeframe":"1m","profileBin":0.01}),
    )
    .await;
    assert_eq!(next(&mut live, is("error")).await["code"], "unknown_symbol");

    // A replay socket on the same server charts the archive day.
    let (mut replay, _) = tokio_tungstenite::connect_async(&s.url).await.unwrap();
    send(
        &mut replay,
        json!({"type": "hello", "client": "test", "source": "replay"}),
    )
    .await;
    next(&mut replay, is("welcome")).await;
    wait_for_replay(&mut replay).await;
    send(
        &mut replay,
        json!({"type":"chart.subscribe","symbol":"SMPL","timeframe":"1m","profileBin":0.01}),
    )
    .await;
    assert_eq!(
        next(&mut replay, is("chart.snapshot")).await["symbol"],
        "SMPL"
    );
    assert_eq!(
        get(&s.app, "/api/chart/SMPL/snapshot?source=replay")
            .await
            .0,
        StatusCode::OK
    );
}

#[tokio::test]
async fn empty_archive_without_autofixture_reports_no_data() {
    let s = stack(&[("SIM_AUTOFIXTURE", "0")]).await;
    let (_, v) = get(&s.app, "/api/symbols").await;
    assert_eq!(v["status"], "no_data");
    assert_eq!(v["symbols"], json!([]));
    // Health is unaffected.
    assert_eq!(get(&s.app, "/api/health").await.0, StatusCode::OK);
}

#[tokio::test]
async fn only_price_band_candidates_are_let_through() {
    // FXHI trades above $25 in the fixture day; SMPL and FXLO are inside $0.10–$25.
    let s = stack(&[
        ("REPLAY_AUTOPLAY", "0"),
        ("SIM_FIXTURE_SYMBOLS", "SMPL,FXLO,FXHI"),
    ])
    .await;
    let mut ws = connect(&s.url).await;
    wait_for_replay(&mut ws).await;

    let (st, v) = get(&s.app, "/api/symbols").await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(v["symbols"], json!(["FXLO", "SMPL"]));
    assert_eq!(v["screened"], json!({"total": 3, "candidates": 2}));
    let band = |k: &str| v["priceBand"][k].as_str().unwrap().parse::<f64>().unwrap();
    assert_eq!((band("min"), band("max")), (0.10, 25.0));

    let (st, body) = get(&s.app, "/api/chart/FXHI/snapshot").await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    assert_eq!(body["error"], "outside_price_band");

    send(
        &mut ws,
        json!({"type": "chart.subscribe", "symbol": "FXHI", "timeframe": "1m", "profileBin": 0.01}),
    )
    .await;
    let err = next(&mut ws, is("error")).await;
    assert_eq!(err["code"], "outside_price_band");
    assert_eq!(err["symbol"], "FXHI", "a chart error names its symbol");
    assert!(err.get("id").is_none(), "the default slot has no id");
}

// --- Several charts on one connection (the dashboard's multi-chart panel) -------------------------

fn sub(id: &str, symbol: &str, tf: &str) -> Value {
    json!({"type":"chart.subscribe","id":id,"symbol":symbol,"timeframe":tf,"profileBin":0.01})
}

#[tokio::test]
async fn one_connection_streams_several_charts() {
    let s = stack(&[]).await;
    let mut ws = connect(&s.url).await;
    wait_for_replay(&mut ws).await;
    send(&mut ws, sub("a", "SMPL", "1m")).await;
    let first = next(&mut ws, is("chart.snapshot")).await;
    send(&mut ws, sub("b", "FXLO", "5m")).await;
    let second = next(&mut ws, |m| {
        m["type"] == "chart.snapshot" && m["symbol"] == "FXLO"
    })
    .await;
    assert_eq!(
        (first["symbol"].as_str(), first["timeframe"].as_str()),
        (Some("SMPL"), Some("1m"))
    );
    assert_eq!(second["timeframe"], "5m");

    // Both charts' increments arrive on the same socket, each for its own timeframe, and nothing
    // for a symbol no slot shows.
    let (mut smpl, mut fxlo) = (0, 0);
    for _ in 0..400 {
        let m = next(&mut ws, |m| m["type"] == "chart.update").await;
        match (m["symbol"].as_str(), m["timeframe"].as_str()) {
            (Some("SMPL"), Some("1m")) => smpl += 1,
            (Some("FXLO"), Some("5m")) => fxlo += 1,
            other => panic!("unexpected chart frame {other:?}"),
        }
        if smpl > 0 && fxlo > 0 {
            break;
        }
    }
    assert!(smpl > 0 && fxlo > 0, "SMPL {smpl}, FXLO {fxlo}");

    // Unsubscribing one slot stops its stream and leaves the other.
    send(&mut ws, json!({"type":"chart.unsubscribe","id":"b"})).await;
    // Frames sent before the server handled the unsubscribe may still be in flight. The session
    // handles messages in order, so everything after the pong to this ping postdates it.
    send(&mut ws, json!({"type":"ping","ts":7})).await;
    next(&mut ws, |m| m["type"] == "pong" && m["ts"] == 7).await;
    for _ in 0..150 {
        let m = next(&mut ws, |m| m["type"] == "chart.update").await;
        assert_eq!(m["symbol"], "SMPL", "FXLO stopped after its slot was freed");
    }
}

#[tokio::test]
async fn two_slots_on_the_same_chart_share_one_stream() {
    let s = stack(&[]).await;
    let mut ws = connect(&s.url).await;
    wait_for_replay(&mut ws).await;
    send(&mut ws, sub("a", "SMPL", "1m")).await;
    next(&mut ws, is("chart.snapshot")).await;
    send(&mut ws, sub("b", "SMPL", "1m")).await;
    next(&mut ws, is("chart.snapshot")).await;
    // Every candle and tape frame goes out once: a duplicated stream would show each frame twice
    // in a row (the engine never emits the same frame twice in a row on its own).
    let mut last = String::new();
    let mut seen = 0;
    while seen < 300 {
        let m = next(&mut ws, |m| {
            m["type"] == "chart.update" || m["type"] == "tape.trade"
        })
        .await;
        let text = m.to_string();
        assert_ne!(text, last, "a frame was sent twice");
        last = text;
        seen += 1;
    }
}

#[tokio::test]
async fn at_most_four_charts_per_connection() {
    let s = stack(&[("REPLAY_AUTOPLAY", "0")]).await;
    let mut ws = connect(&s.url).await;
    wait_for_replay(&mut ws).await;
    for id in ["1", "2", "3", "4"] {
        send(&mut ws, sub(id, "SMPL", "1m")).await;
        next(&mut ws, is("chart.snapshot")).await;
    }
    send(&mut ws, sub("5", "FXLO", "1m")).await;
    assert_eq!(next(&mut ws, is("error")).await["code"], "too_many_charts");
    // Replacing a held slot is not a new chart.
    send(&mut ws, sub("4", "FXLO", "1m")).await;
    assert_eq!(next(&mut ws, is("chart.snapshot")).await["symbol"], "FXLO");
    // Freeing one makes room.
    send(&mut ws, json!({"type":"chart.unsubscribe","id":"1"})).await;
    send(&mut ws, sub("5", "FXLO", "5m")).await;
    assert_eq!(next(&mut ws, is("chart.snapshot")).await["timeframe"], "5m");
}

#[tokio::test]
async fn a_refused_chart_names_its_slot() {
    let s = stack(&[("REPLAY_AUTOPLAY", "0")]).await;
    let mut ws = connect(&s.url).await;
    wait_for_replay(&mut ws).await;
    send(&mut ws, sub("left", "NOPE", "1m")).await;
    let err = next(&mut ws, is("error")).await;
    assert_eq!(
        (
            err["code"].as_str(),
            err["symbol"].as_str(),
            err["id"].as_str()
        ),
        (Some("unknown_symbol"), Some("NOPE"), Some("left"))
    );
    let long = "x".repeat(33);
    send(&mut ws, sub(&long, "SMPL", "1m")).await;
    assert_eq!(next(&mut ws, is("error")).await["code"], "bad_request");
}

#[tokio::test]
async fn the_price_band_is_configurable() {
    // A $0.10–$1.00 band keeps only the sub-dollar fixture symbol.
    let s = stack(&[
        ("REPLAY_AUTOPLAY", "0"),
        ("SIM_FIXTURE_SYMBOLS", "SMPL,FXLO,FXHI"),
        ("PRICE_BAND_MAX", "1.00"),
    ])
    .await;
    let mut ws = connect(&s.url).await;
    wait_for_replay(&mut ws).await;
    let (_, v) = get(&s.app, "/api/symbols").await;
    assert_eq!(v["symbols"], json!(["FXLO"]));

    let bad = MarketConfig::from_lookup(|k| match k {
        "PRICE_BAND_MIN" => Some("5".into()),
        "PRICE_BAND_MAX" => Some("1".into()),
        _ => None,
    });
    assert!(bad.is_err(), "an inverted band must be refused at startup");
}

// --- simulated chart trading (docs/architecture/chart-trading-sim.md) -----------------------

/// The next `paper.state` matching `pred`.
async fn paper_state(ws: &mut Ws, pred: impl Fn(&Value) -> bool) -> Value {
    next(ws, |m| m["type"] == "paper.state" && pred(m)).await
}

fn order<'a>(state: &'a Value, id: &str) -> Option<&'a Value> {
    state["orders"].as_array()?.iter().find(|o| o["id"] == id)
}

#[tokio::test]
async fn a_limit_order_fills_during_replay_and_every_connection_hears_it() {
    let s = stack(&[]).await;
    let mut ws = connect(&s.url).await;
    // paper.state comes right after welcome: a flat, empty, clearly simulated account.
    let first = next(&mut ws, |m| m["type"] != "replay.clock").await;
    assert_eq!(first["type"], "paper.state");
    assert_eq!(first["simulated"], true);
    assert_eq!(first["account"]["cash"], 100_000.0);
    assert_eq!(first["orders"], json!([]));
    assert_eq!(first["positions"], json!([]));
    let mut other = connect(&s.url).await;
    wait_for_replay(&mut ws).await;

    // A price-qualifying print tells us where SMPL trades; a buy limit 10 cents above it is
    // traded through by the next print near that level.
    send(
        &mut ws,
        json!({"type":"chart.subscribe","symbol":"SMPL","timeframe":"1m","profileBin":0.01}),
    )
    .await;
    let print = next(&mut ws, |m| {
        m["type"] == "tape.trade" && m["counted"] == "ohlc_and_volume"
    })
    .await;
    let p = print["p"].as_f64().unwrap();
    let limit = ((p + 0.10) * 100.0).ceil() / 100.0;
    send(
        &mut ws,
        json!({"type":"order.place","clientOrderId":"c-1","symbol":"smpl","side":"buy",
               "orderType":"limit","qty":100,"limitPrice":limit}),
    )
    .await;
    let placed = paper_state(&mut ws, |m| !m["orders"].as_array().unwrap().is_empty()).await;
    let o = &placed["orders"][0];
    assert_eq!(o["clientOrderId"], "c-1");
    assert_eq!(o["symbol"], "SMPL");
    assert_eq!(o["limitPrice"], limit);
    assert!(
        o["createdAt"].as_i64().unwrap() >= T1040,
        "virtual clock, not wall clock"
    );
    let id = o["id"].as_str().unwrap().to_string();

    // The state that contains a fill goes out first, then the fill event itself.
    let filled = paper_state(&mut ws, |m| {
        order(m, &id).is_some_and(|o| o["status"] == "filled")
    })
    .await;
    let fill = next(&mut ws, is("paper.fill")).await;
    assert_eq!(fill["simulated"], true);
    assert_eq!(fill["orderId"], id.as_str());
    assert_eq!(
        (fill["side"].as_str(), fill["qty"].as_i64()),
        (Some("buy"), Some(100))
    );
    assert_eq!(fill["price"], limit, "a limit fills at its limit");
    assert_eq!(order(&filled, &id).unwrap()["avgFillPrice"], limit);
    assert_eq!(filled["positions"][0]["symbol"], "SMPL");
    assert_eq!(filled["positions"][0]["qty"], 100);
    let cash = filled["account"]["cash"].as_f64().unwrap();
    assert!((cash - (100_000.0 - 100.0 * limit)).abs() < 1e-6, "{cash}");

    // The other connection (no chart subscription) got the same broadcast. Matched on the state
    // rather than the fill: this socket is not read while the replay plays, so it may lag, and a
    // lagged client is resynchronized with a fresh paper.state, not the missed fill.
    paper_state(&mut other, |m| {
        order(m, &id).is_some_and(|o| o["status"] == "filled")
    })
    .await;

    // Time travel resets the account for everyone.
    send(
        &mut ws,
        json!({"type":"replay.control","action":"seek","value":T1040 + 5 * 60_000}),
    )
    .await;
    for c in [&mut ws, &mut other] {
        let reset = paper_state(c, |m| m["orders"].as_array().unwrap().is_empty()).await;
        assert_eq!(reset["positions"], json!([]));
        assert_eq!(reset["account"]["cash"], 100_000.0);
        assert_eq!(reset["account"]["realizedPnl"], 0.0);
    }
}

#[tokio::test]
async fn rejections_show_up_in_state_and_bad_commands_get_errors() {
    let s = stack(&[("REPLAY_AUTOPLAY", "0")]).await;
    let mut ws = connect(&s.url).await;
    wait_for_replay(&mut ws).await;

    send(
        &mut ws,
        json!({"type":"order.place","symbol":"SMPL","side":"buy","orderType":"limit",
               "qty":10,"limitPrice":3.385}),
    )
    .await;
    let st = paper_state(&mut ws, |m| !m["orders"].as_array().unwrap().is_empty()).await;
    assert_eq!(st["orders"][0]["status"], "rejected");
    assert_eq!(st["orders"][0]["rejectReason"], "sub_penny");

    // Unknown means not in the market's symbol list at all.
    send(
        &mut ws,
        json!({"type":"order.place","symbol":"NOPE","side":"sell","orderType":"market","qty":1}),
    )
    .await;
    let st = paper_state(&mut ws, |m| m["orders"].as_array().unwrap().len() == 2).await;
    assert_eq!(st["orders"][1]["rejectReason"], "unknown_symbol");

    // Malformed (bad side) and unknown commands: error bad_order, no state change.
    send(
        &mut ws,
        json!({"type":"order.place","symbol":"SMPL","side":"hold","orderType":"market","qty":1}),
    )
    .await;
    assert_eq!(next(&mut ws, is("error")).await["code"], "bad_order");
    send(&mut ws, json!({"type":"order.replace","id":"o-1"})).await;
    assert_eq!(next(&mut ws, is("error")).await["code"], "bad_order");

    // Modifying or cancelling a finished order: not_open.
    send(&mut ws, json!({"type":"order.modify","id":"o-1","qty":5})).await;
    assert_eq!(next(&mut ws, is("error")).await["code"], "not_open");
    send(&mut ws, json!({"type":"order.cancel","id":"o-1"})).await;
    assert_eq!(next(&mut ws, is("error")).await["code"], "not_open");

    // A working order can be modified and cancelled; each change is broadcast.
    send(
        &mut ws,
        json!({"type":"order.place","symbol":"SMPL","side":"buy","orderType":"stop",
               "qty":10,"stopPrice":24.0}),
    )
    .await;
    paper_state(&mut ws, |m| {
        order(m, "o-3").is_some_and(|o| o["status"] == "open")
    })
    .await;
    send(
        &mut ws,
        json!({"type":"order.modify","id":"o-3","stopPrice":24.5}),
    )
    .await;
    paper_state(&mut ws, |m| {
        order(m, "o-3").is_some_and(|o| o["stopPrice"] == 24.5)
    })
    .await;
    send(&mut ws, json!({"type":"order.cancel","id":"o-3"})).await;
    paper_state(&mut ws, |m| {
        order(m, "o-3").is_some_and(|o| o["status"] == "cancelled")
    })
    .await;

    send(&mut ws, json!({"type":"paper.reset"})).await;
    paper_state(&mut ws, |m| m["orders"].as_array().unwrap().is_empty()).await;
}

#[tokio::test]
async fn a_bracket_fills_its_entry_then_one_exit_and_cancels_the_other() {
    let s = stack(&[]).await;
    let mut ws = connect(&s.url).await;
    wait_for_replay(&mut ws).await;
    send(
        &mut ws,
        json!({"type":"chart.subscribe","symbol":"SMPL","timeframe":"1m","profileBin":0.01}),
    )
    .await;
    let print = next(&mut ws, |m| {
        m["type"] == "tape.trade" && m["counted"] == "ohlc_and_volume"
    })
    .await;
    let p = print["p"].as_f64().unwrap();
    // A market entry fills on the next print near p. Its take-profit, a sell limit 10 cents
    // under that, is traded through by the very same print but must wait for the one after;
    // the stop-loss 30 cents under is out of reach, so it is the one the OCO cancels.
    let cents = |x: f64| (x * 100.0).round() / 100.0;
    let (tp, sl) = (cents(p - 0.10), cents(p - 0.30));
    send(
        &mut ws,
        json!({"type":"order.place","clientOrderId":"c-b","symbol":"SMPL","side":"buy",
               "orderType":"market","qty":100,"bracket":{"takeProfit":tp,"stopLoss":sl}}),
    )
    .await;
    let placed = paper_state(&mut ws, |m| m["orders"].as_array().unwrap().len() == 3).await;
    let ids: Vec<String> = placed["orders"]
        .as_array()
        .unwrap()
        .iter()
        .map(|o| o["id"].as_str().unwrap().to_string())
        .collect();
    let [entry, tp_id, sl_id] = [&ids[0], &ids[1], &ids[2]];
    let group = placed["orders"][0]["ocoGroup"].clone();
    assert!(group.as_str().unwrap().starts_with("b-"));
    for (o, role, status) in [
        (entry, "entry", "open"),
        (tp_id, "take_profit", "pending"),
        (sl_id, "stop_loss", "pending"),
    ] {
        let o = order(&placed, o).unwrap();
        assert_eq!((&o["role"], &o["status"]), (&json!(role), &json!(status)));
        assert_eq!(o["ocoGroup"], group);
    }
    assert_eq!(order(&placed, tp_id).unwrap()["parentId"], entry.as_str());
    assert_eq!(order(&placed, tp_id).unwrap()["limitPrice"], tp);
    assert_eq!(order(&placed, sl_id).unwrap()["stopPrice"], sl);

    // The entry's fill opens both exits, and fills neither of them on that print.
    let opened = paper_state(&mut ws, |m| {
        order(m, entry).is_some_and(|o| o["status"] == "filled")
    })
    .await;
    assert_eq!(order(&opened, tp_id).unwrap()["status"], "open");
    assert_eq!(order(&opened, sl_id).unwrap()["status"], "open");
    assert_eq!(opened["positions"][0]["qty"], 100);

    // The next print fills the take-profit at its limit and cancels the stop-loss.
    let closed = paper_state(&mut ws, |m| {
        order(m, tp_id).is_some_and(|o| o["status"] == "filled")
    })
    .await;
    let sib = order(&closed, sl_id).unwrap();
    assert_eq!(
        (&sib["status"], &sib["rejectReason"]),
        (&json!("cancelled"), &Value::Null)
    );
    assert_eq!(closed["positions"], json!([]), "flat");
    let fill = next(&mut ws, is("paper.fill")).await;
    assert_eq!(fill["orderId"], tp_id.as_str());
    assert_eq!(
        (fill["side"].as_str(), fill["price"].as_f64()),
        (Some("sell"), Some(tp))
    );

    // A bracket out of order is refused when modified, and the order stays as it was.
    send(&mut ws, json!({"type":"paper.reset"})).await;
    paper_state(&mut ws, |m| m["orders"].as_array().unwrap().is_empty()).await;
    send(
        &mut ws,
        json!({"type":"order.place","symbol":"SMPL","side":"buy","orderType":"limit",
               "qty":1,"limitPrice":1.00,"bracket":{"takeProfit":1.50,"stopLoss":0.50}}),
    )
    .await;
    let st = paper_state(&mut ws, |m| m["orders"].as_array().unwrap().len() == 3).await;
    let tp_id = st["orders"][1]["id"].as_str().unwrap().to_string();
    send(
        &mut ws,
        json!({"type":"order.modify","id":tp_id,"limitPrice":0.40}),
    )
    .await;
    assert_eq!(next(&mut ws, is("error")).await["code"], "bad_bracket");
}

/// docs/architecture/halts-luld-fills.md: the fixture day halts every symbol 10:05–10:06:30.
/// Replayed through the server, a subscriber sees `chart.halt` start and end and LULD points,
/// whatever its timeframe, and a later snapshot carries both.
#[tokio::test]
async fn fixture_halt_and_luld_bands_reach_subscribers_and_snapshots() {
    const T1004: i64 = T1040 - 36 * 60_000;
    const T1005: i64 = T1040 - 35 * 60_000;
    const T100630: i64 = T1040 - 33 * 60_000 - 30_000;
    let s = stack(&[
        ("REPLAY_START", "10:04"),
        ("REPLAY_AUTOPLAY", "0"),
        ("REPLAY_SPEED", "60"),
    ])
    .await;
    let mut ws = connect(&s.url).await;
    next(&mut ws, |m| {
        m["type"] == "replay.clock" && m["clock"]["now"].as_i64().unwrap_or(0) >= T1004
    })
    .await;
    send(
        &mut ws,
        json!({"type":"chart.subscribe","symbol":"SMPL","timeframe":"1m","profileBin":0.01}),
    )
    .await;
    let snap = next(&mut ws, is("chart.snapshot")).await;
    assert_eq!(snap["halts"], json!([]));
    let bands = snap["luld"].as_array().unwrap();
    assert!(!bands.is_empty(), "bands since 09:30");
    assert!(bands.last().unwrap()["up"].is_number());
    assert!(
        bands
            .windows(2)
            .all(|w| w[0]["t"].as_i64() < w[1]["t"].as_i64())
    );

    send(&mut ws, json!({"type":"replay.control","action":"play"})).await;
    let mut halt_frames = Vec::new();
    let mut luld_frames = Vec::new();
    // Until the first bands after the resume.
    loop {
        let m = next(&mut ws, |m| {
            m["type"] == "chart.halt" || m["type"] == "chart.luld"
        })
        .await;
        assert_eq!(m["symbol"], "SMPL");
        assert!(m.get("timeframe").is_none());
        if m["type"] == "chart.halt" {
            halt_frames.push(m["halt"].clone());
        } else {
            let p = m["point"].clone();
            let after_resume = p["t"].as_i64().unwrap() >= T100630 && p["up"].is_number();
            luld_frames.push(p);
            if after_resume {
                break;
            }
        }
    }
    // Each ms time with its exchange-ns twin (`times.ns.v1`); the fixture's times are whole ms.
    let ns = |ms: i64| ms * 1_000_000;
    let start = json!({"start": T1005, "startNs": ns(T1005), "end": null, "endNs": null, "reason": "LUDP", "code": "H"});
    let end = json!({"start": T1005, "startNs": ns(T1005), "end": T100630, "endNs": ns(T100630), "reason": "LUDP", "code": "H"});
    assert_eq!(halt_frames, vec![start, end.clone()]);
    assert!(
        luld_frames
            .iter()
            .any(|p| *p
                == json!({"t": T1005, "tNs": ns(T1005), "ref": null, "up": null, "down": null})),
        "{luld_frames:?}"
    );
    let first_after = luld_frames.last().unwrap().clone();

    send(&mut ws, json!({"type":"replay.control","action":"pause"})).await;
    next(&mut ws, |m| {
        m["type"] == "replay.clock" && m["clock"]["playing"] == false
    })
    .await;
    // Another timeframe: the same halts and bands.
    send(
        &mut ws,
        json!({"type":"chart.subscribe","symbol":"SMPL","timeframe":"5m","profileBin":0.01}),
    )
    .await;
    let snap = next(&mut ws, is("chart.snapshot")).await;
    assert_eq!(snap["halts"], json!([end]));
    let bands = snap["luld"].as_array().unwrap();
    assert!(
        bands.contains(
            &json!({"t": T1005, "tNs": ns(T1005), "ref": null, "up": null, "down": null})
        )
    );
    assert!(bands.contains(&first_after), "{bands:?}");
    // No bands are published while halted.
    assert!(
        !bands
            .iter()
            .any(|p| (T1005 + 1..T100630).contains(&p["t"].as_i64().unwrap()))
    );
}

// --- quote board (quotes.subscribe / quotes.snapshot / quotes.update) ------------------------

fn quote<'a>(frame: &'a Value, symbol: &str) -> Option<&'a Value> {
    frame["quotes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|q| q["s"] == symbol)
}

fn quote_symbols(frame: &Value) -> Vec<String> {
    frame["quotes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|q| q["s"].as_str().unwrap().to_string())
        .collect()
}

/// Reads frames for `wait` and returns the types seen.
async fn types_for(ws: &mut Ws, wait: Duration) -> Vec<String> {
    let mut seen = Vec::new();
    let _ = tokio::time::timeout(wait, async {
        while let Some(Ok(m)) = ws.next().await {
            if let Message::Text(t) = m {
                let v: Value = serde_json::from_str(&t).unwrap();
                seen.push(v["type"].as_str().unwrap_or("").to_string());
            }
        }
    })
    .await;
    seen
}

#[tokio::test]
async fn quotes_subscribe_gets_a_snapshot_then_changed_rows_about_once_a_second() {
    let s = stack(&[("SIM_FIXTURE_SYMBOLS", "SMPL,FXLO,FXHI")]).await;
    let mut ws = connect(&s.url).await;
    let mut bystander = connect(&s.url).await;
    wait_for_replay(&mut ws).await;

    send(&mut ws, json!({"type": "quotes.subscribe"})).await;
    let snap = next(&mut ws, is("quotes.snapshot")).await;
    assert_eq!(
        quote_symbols(&snap),
        ["FXLO", "SMPL"],
        "candidates only (FXHI is above $25), sorted"
    );
    assert!(snap["t"].as_i64().unwrap() >= T1040);
    // The market clock's exchange-ns twin (`times.ns.v1`), floored to the ms `t`.
    assert_eq!(
        snap["tNs"].as_i64().unwrap().div_euclid(1_000_000),
        snap["t"].as_i64().unwrap()
    );
    let smpl = quote(&snap, "SMPL").unwrap().clone();
    assert!(smpl["p"].as_f64().unwrap() > 0.0);
    assert!(smpl["o"].as_f64().unwrap() > 0.0);
    assert!(smpl["v"].as_u64().unwrap() > 0);
    assert!(smpl["n"].as_u64().unwrap() > 0);
    assert!(
        smpl.get("pre").is_none(),
        "past 09:30 the reference is the regular open"
    );
    assert!(
        smpl.get("name").is_none(),
        "no assets.json: no invented name"
    );

    // Updates: only changed rows, monotone volume, spaced by wall time, and the reference
    // (the regular open) does not move once set.
    let first = next(&mut ws, is("quotes.update")).await;
    let t_first = std::time::Instant::now();
    let second = next(&mut ws, is("quotes.update")).await;
    let gap = t_first.elapsed();
    assert!(
        gap >= Duration::from_millis(800),
        "updates at most ~1/s of wall time, got {gap:?}"
    );
    for u in [&first, &second] {
        assert!(!u["quotes"].as_array().unwrap().is_empty());
        // `>=`, not `>`: `t` is the display clock, which the replay publishes ~4 times a second
        // of wall time, while events keep arriving in between. The first update right after the
        // snapshot can carry rows changed by events applied since, before the clock ticked again
        // (seen as `t == snap.t` at 10:40:00, flaky under load).
        assert!(u["t"].as_i64().unwrap() >= snap["t"].as_i64().unwrap());
        assert!(quote(u, "FXHI").is_none());
        for q in u["quotes"].as_array().unwrap() {
            assert!(
                q.get("name").is_none() && q.get("x").is_none(),
                "names only in snapshots"
            );
        }
    }
    if let Some(q) = quote(&second, "SMPL") {
        assert!(q["v"].as_u64() >= smpl["v"].as_u64());
        assert_eq!(q["o"], smpl["o"]);
    }

    // A connection that never subscribed never sees a quotes frame.
    let seen = types_for(&mut bystander, Duration::from_millis(2500)).await;
    assert!(seen.iter().any(|t| t == "replay.clock"));
    assert!(!seen.iter().any(|t| t.starts_with("quotes.")), "{seen:?}");

    // Unsubscribe: after the server has processed it (the pong proves it), nothing more.
    send(&mut ws, json!({"type": "quotes.unsubscribe"})).await;
    send(&mut ws, json!({"type": "ping", "ts": 42})).await;
    next(&mut ws, |m| m["type"] == "pong" && m["ts"] == 42).await;
    let seen = types_for(&mut ws, Duration::from_millis(2500)).await;
    assert!(!seen.iter().any(|t| t.starts_with("quotes.")), "{seen:?}");
}

/// `quotes.search.v1`: the server ranks the board for a search and pushes the top N (agreeing
/// with the whole board's rows), pins get rows whatever their rank, updates come at most once a
/// second, a seek resends the snapshot, bad chips are refused and the directory names the
/// candidates.
#[tokio::test]
async fn quotes_search_ranks_the_board_on_the_server_and_pushes_the_top_n() {
    let s = stack_with_assets(
        &[("SIM_FIXTURE_SYMBOLS", "SMPL,FXLO,FXHI")],
        Some(json!({"SMPL": {"name": "Sample Inc", "exchange": "NASDAQ"}})),
    )
    .await;
    let (mut ws, _) = tokio_tungstenite::connect_async(&s.url).await.unwrap();
    send(&mut ws, json!({"type": "hello"})).await;
    let welcome = next(&mut ws, is("welcome")).await;
    assert!(
        welcome["features"]
            .as_array()
            .unwrap()
            .contains(&json!("quotes.search.v1")),
        "{welcome}"
    );
    assert!(
        welcome["features"]
            .as_array()
            .unwrap()
            .contains(&json!("times.ns.v1")),
        "{welcome}"
    );
    // The session schedule on snapshots, the Runners notices a phone may ring from, and the
    // MACD styles.
    for f in ["sessions.v1", "runners.notice.v1", "indicators.macd.v2"] {
        assert!(
            welcome["features"].as_array().unwrap().contains(&json!(f)),
            "{f}: {welcome}"
        );
    }
    wait_for_replay(&mut ws).await;
    // Paused, so the whole board and the search see the same prints.
    send(
        &mut ws,
        json!({"type": "replay.control", "action": "pause"}),
    )
    .await;
    next(&mut ws, |m| {
        m["type"] == "replay.clock" && m["clock"]["playing"] == false
    })
    .await;

    // The whole board, for comparison: the search must rank exactly these rows.
    send(&mut ws, json!({"type": "quotes.subscribe"})).await;
    let board = next(&mut ws, is("quotes.snapshot")).await;
    send(&mut ws, json!({"type": "quotes.unsubscribe"})).await;
    let mut by_volume: Vec<(u64, String)> = board["quotes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|q| {
            (
                q["v"].as_u64().unwrap(),
                q["s"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    by_volume.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));

    send(
        &mut ws,
        json!({"type": "quotes.search", "id": "a", "limit": 1, "pins": ["FXLO", "SMPL"]}),
    )
    .await;
    let snap = next(&mut ws, is("quotes.search.snapshot")).await;
    assert_eq!(snap["id"], "a");
    assert_eq!(snap["total"], 2, "FXHI is above $25: not a candidate");
    assert_eq!(snap["rank"], json!([by_volume[0].1]), "{snap}");
    let rows = snap["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 2, "the top 1 plus the other pin: {snap}");
    let smpl = rows.iter().find(|r| r["s"] == "SMPL").unwrap();
    assert_eq!(smpl["name"], "Sample Inc");
    assert_eq!(smpl["x"], "NASDAQ");
    assert!(smpl["p"].as_f64().unwrap() > 0.0 && smpl["chg"].is_number());
    let board_smpl = quote(&board, "SMPL").unwrap();
    assert_eq!(smpl["v"], board_smpl["v"], "the same row the board has");

    // Playing again: updates carry only what changed, spaced by wall time.
    send(&mut ws, json!({"type": "replay.control", "action": "play"})).await;
    let first = next(&mut ws, is("quotes.search.update")).await;
    let t0 = std::time::Instant::now();
    let _second = next(&mut ws, is("quotes.search.update")).await;
    assert!(
        t0.elapsed() >= Duration::from_millis(800),
        "{:?}",
        t0.elapsed()
    );
    for r in first["rows"].as_array().unwrap() {
        assert!(r.get("name").is_none(), "names only for new rows: {first}");
    }

    // A filter the server knows narrows; one it does not is refused with the search's id.
    send(
        &mut ws,
        json!({"type": "quotes.search", "id": "a", "filters": ["nasdaq"]}),
    )
    .await;
    let snap = next(&mut ws, is("quotes.search.snapshot")).await;
    assert_eq!(snap["rank"], json!(["SMPL"]), "{snap}");
    send(
        &mut ws,
        json!({"type": "quotes.search", "id": "b", "filters": ["bogus"]}),
    )
    .await;
    let err = next(&mut ws, is("error")).await;
    assert_eq!(
        (err["code"].as_str(), err["id"].as_str()),
        (Some("bad_request"), Some("b"))
    );

    // A seek resends the snapshot.
    send(
        &mut ws,
        json!({"type": "replay.control", "action": "seek", "value": T1040 + 60_000}),
    )
    .await;
    next(&mut ws, is("quotes.search.snapshot")).await;

    // The directory names every candidate it knows.
    send(&mut ws, json!({"type": "quotes.directory"})).await;
    let dir = next(&mut ws, is("quotes.directory")).await;
    let rows = dir["rows"].as_array().unwrap();
    assert!(
        rows.contains(&json!(["SMPL", "Sample Inc", "NASDAQ"])),
        "{dir}"
    );
    assert!(rows.contains(&json!(["FXLO"])), "{dir}");

    // Stopped: no more search frames once the server has processed it.
    send(&mut ws, json!({"type": "quotes.search.stop", "id": "a"})).await;
    send(&mut ws, json!({"type": "ping", "ts": 7})).await;
    next(&mut ws, |m| m["type"] == "pong" && m["ts"] == 7).await;
    let seen = types_for(&mut ws, Duration::from_millis(2500)).await;
    assert!(
        !seen.iter().any(|t| t.starts_with("quotes.search")),
        "{seen:?}"
    );
}

#[tokio::test]
async fn quotes_volume_and_last_match_the_candles_through_seeks_corrections_and_cancels() {
    let s = stack(&[("REPLAY_AUTOPLAY", "0")]).await;
    let mut ws = connect(&s.url).await;
    wait_for_replay(&mut ws).await;

    send(&mut ws, json!({"type": "quotes.subscribe"})).await;
    let snap = next(&mut ws, is("quotes.snapshot")).await;
    assert_eq!(snap["t"], T1040);
    // The same volume rule as the candles: the board's day volume is the chart's total.
    send(
        &mut ws,
        json!({"type":"chart.subscribe","symbol":"SMPL","timeframe":"1m","profileBin":0.01}),
    )
    .await;
    let chart = next(&mut ws, is("chart.snapshot")).await;
    let v_1040 = quote(&snap, "SMPL").unwrap()["v"].as_u64().unwrap();
    assert_eq!(v_1040, total_volume(&chart));

    // Paused: nothing changes, so no update goes out.
    let seen = types_for(&mut ws, Duration::from_millis(2200)).await;
    assert!(!seen.iter().any(|t| t == "quotes.update"), "{seen:?}");

    // Seek back: the board is rebuilt with the core and a fresh snapshot replaces the old one.
    let target = T1040 - 30 * 60_000; // 10:10 ET
    send(
        &mut ws,
        json!({"type":"replay.control","action":"seek","value":target}),
    )
    .await;
    let again = next(&mut ws, is("quotes.snapshot")).await;
    assert_eq!(again["t"], target);
    let v_1010 = quote(&again, "SMPL").unwrap()["v"].as_u64().unwrap();
    assert!(v_1010 < v_1040, "{v_1010} < {v_1040}");
    let chart = next(&mut ws, is("chart.snapshot")).await;
    assert_eq!(v_1010, total_volume(&chart));
    // The rebuild's rows are in that snapshot, not replayed as a burst of updates.
    let seen = types_for(&mut ws, Duration::from_millis(2200)).await;
    assert!(!seen.iter().any(|t| t == "quotes.update"), "{seen:?}");

    // Corrections and cancels, applied live (playing through them, not rebuilt): the fixture
    // corrects SMPL's first regular print at/after 11:00 (+30 s, price +$0.05) and cancels its
    // first one at/after 12:00 (+20 s). The board follows the engine's own revision, so its volume
    // and last still equal the chart's totals and latest close.
    let chart = play_through(&mut ws, T1040 + 18 * 60_000, T1040 + 23 * 60_000).await; // 10:58 → 11:03
    assert_eq!(
        chart["quality"]["correctionsApplied"], 1,
        "the correction happened"
    );
    assert_board_matches_chart(&s, &chart).await;
    let chart = play_through(&mut ws, T1040 + 78 * 60_000, T1040 + 83 * 60_000).await; // 11:58 → 12:03
    assert_eq!(chart["quality"]["cancelsApplied"], 1, "the cancel happened");
    assert_board_matches_chart(&s, &chart).await;
    // And after a seek past both: the rebuild applies them the same way.
    let target = T1040 + 90 * 60_000; // 12:10 ET
    send(
        &mut ws,
        json!({"type":"replay.control","action":"seek","value":target}),
    )
    .await;
    let chart = next(&mut ws, |m| {
        m["type"] == "chart.snapshot" && m["clock"]["now"] == target
    })
    .await;
    assert_eq!(chart["quality"]["correctionsApplied"], 1);
    assert_eq!(chart["quality"]["cancelsApplied"], 1);
    assert_board_matches_chart(&s, &chart).await;
}

/// Seeks to `from`, plays until the clock passes `until`, pauses, and returns a fresh SMPL 1m
/// `chart.snapshot` taken while paused (matched by its clock, so a snapshot the seek's resync
/// sent earlier is never mistaken for it).
async fn play_through(ws: &mut Ws, from: i64, until: i64) -> Value {
    send(
        ws,
        json!({"type":"replay.control","action":"seek","value":from}),
    )
    .await;
    next(ws, |m| m["type"] == "quotes.snapshot" && m["t"] == from).await;
    send(ws, json!({"type":"replay.control","action":"play"})).await;
    next(ws, |m| {
        m["type"] == "replay.clock" && m["clock"]["now"].as_i64().unwrap_or(0) >= until
    })
    .await;
    send(ws, json!({"type":"replay.control","action":"pause"})).await;
    let paused = next(ws, |m| {
        m["type"] == "replay.clock" && m["clock"]["playing"] == false
    })
    .await;
    let now = paused["clock"]["now"].clone();
    send(
        ws,
        json!({"type":"chart.subscribe","symbol":"SMPL","timeframe":"1m","profileBin":0.01}),
    )
    .await;
    next(ws, |m| {
        m["type"] == "chart.snapshot" && m["clock"]["now"] == now
    })
    .await
}

/// The board's SMPL row (`/api/quotes`, taken at the same paused clock as `chart`) has the chart's
/// day volume and its latest close as the last price.
async fn assert_board_matches_chart(s: &Stack, chart: &Value) {
    let (_, q) = get(&s.app, "/api/quotes").await;
    assert_eq!(
        q["t"], chart["clock"]["now"],
        "both taken at the same clock"
    );
    let row = quote(&q, "SMPL").unwrap();
    assert_eq!(row["v"].as_u64().unwrap(), total_volume(chart), "volume");
    let last = chart["candles"].as_array().unwrap().last().unwrap()["c"].clone();
    assert_eq!(row["p"], last, "last price");
}

#[tokio::test]
async fn quotes_and_the_chart_measure_change_against_the_previous_close_when_known() {
    // The fixture day is Thursday 2026-10-15. SMPL has closes for the 13th and the 14th (the
    // 14th is the reference; the 15th's own close is never used); FXLO has none and falls back.
    let s = stack_with_files(
        &[("REPLAY_AUTOPLAY", "0")],
        &[(
            "daily-closes.json",
            json!({
                "2026-10-13": {"SMPL": 3.5},
                "2026-10-14": {"SMPL": 4.25},
                "2026-10-15": {"SMPL": 99.0}
            }),
        )],
    )
    .await;
    let mut ws = connect(&s.url).await;
    wait_for_replay(&mut ws).await;
    send(&mut ws, json!({"type": "quotes.subscribe"})).await;
    let snap = next(&mut ws, is("quotes.snapshot")).await;
    let smpl = quote(&snap, "SMPL").unwrap();
    assert_eq!(smpl["pc"], 4.25);
    assert!(
        smpl["o"].as_f64().unwrap() > 0.0,
        "the open fallback is still sent"
    );
    assert!(
        quote(&snap, "FXLO").unwrap().get("pc").is_none(),
        "no close known: no invented reference"
    );
    send(
        &mut ws,
        json!({"type":"chart.subscribe","symbol":"SMPL","timeframe":"1m","profileBin":0.01}),
    )
    .await;
    assert_eq!(next(&mut ws, is("chart.snapshot")).await["prevClose"], 4.25);
    send(
        &mut ws,
        json!({"type":"chart.subscribe","symbol":"FXLO","timeframe":"1m","profileBin":0.01}),
    )
    .await;
    let fxlo = next(&mut ws, is("chart.snapshot")).await;
    assert!(
        fxlo.get("prevClose").is_none(),
        "{:?}",
        fxlo.get("prevClose")
    );

    // Static for the day: updates leave it out (the client keeps the snapshot's).
    send(&mut ws, json!({"type":"replay.control","action":"play"})).await;
    let u = next(&mut ws, |m| {
        m["type"] == "quotes.update" && quote(m, "SMPL").is_some()
    })
    .await;
    assert!(quote(&u, "SMPL").unwrap().get("pc").is_none());
}

#[tokio::test]
async fn quotes_carry_names_and_exchanges_from_assets_json_and_the_rest_route_serves_them() {
    let s = stack_with_assets(
        &[("REPLAY_AUTOPLAY", "0")],
        Some(json!({
            "SMPL": {"name": "Sample Labs Inc.", "exchange": "NASDAQ"},
            "ZZZZ": {"name": "Not In The Archive", "exchange": "NYSE"}
        })),
    )
    .await;
    let mut ws = connect(&s.url).await;
    wait_for_replay(&mut ws).await;

    let (st, v) = get(&s.app, "/api/quotes").await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(v["type"], "quotes.snapshot");
    assert_eq!(v["t"], T1040);
    assert_eq!(quote_symbols(&v), ["FXLO", "SMPL"]);
    let smpl = quote(&v, "SMPL").unwrap();
    assert_eq!(smpl["name"], "Sample Labs Inc.");
    assert_eq!(smpl["x"], "NASDAQ");
    let fxlo = quote(&v, "FXLO").unwrap();
    assert!(
        fxlo.get("name").is_none() && fxlo.get("x").is_none(),
        "a symbol missing from assets.json gets no invented name"
    );

    send(&mut ws, json!({"type": "quotes.subscribe"})).await;
    let snap = next(&mut ws, is("quotes.snapshot")).await;
    assert_eq!(quote(&snap, "SMPL").unwrap()["name"], "Sample Labs Inc.");
}

/// 10:40 ET on 2026-10-14 (EDT) in epoch ms: the configured start time on the day switched to.
const T1040_OCT14: i64 = T1040 - 86_400_000;
/// 20:00 ET on 2026-10-13: where the 2026-10-14 trading day (its overnight session) begins.
const OPEN_OCT14: i64 = T1040_OCT14 - (14 * 60 + 40) * 60_000;

/// Fixture days for the day-switch tests: three weekdays, different symbol sets.
const DAYS: &[(&str, &[&str])] = &[
    ("2026-10-13", &["SMPL", "FXLO", "FXMD"]),
    ("2026-10-14", &["SMPL", "FXHI"]),
    ("2026-10-15", &["SMPL", "FXLO"]),
];

/// Folds `symbols.snapshot` / `symbols.update` frames as a client does, checking `rev` continuity.
fn fold_symbols(list: &mut Vec<String>, rev: &mut u64, m: &Value) {
    let strs = |v: &Value| -> Vec<String> {
        v.as_array()
            .unwrap()
            .iter()
            .map(|s| s.as_str().unwrap().to_string())
            .collect()
    };
    if m["type"] == "symbols.snapshot" {
        *list = strs(&m["symbols"]);
    } else {
        assert_eq!(
            m["rev"].as_u64().unwrap(),
            *rev + 1,
            "an update applies to rev - 1"
        );
        list.retain(|s| !strs(&m["removed"]).contains(s));
        list.extend(strs(&m["added"]));
        list.sort();
    }
    *rev = m["rev"].as_u64().unwrap();
}

/// The candidate list over the socket (`symbols.v1`, crate::symbols): a snapshot with the same
/// fields as `/api/symbols`, and after a day switch a fresh snapshot (then any updates) until the
/// new day's list is in. Replaces the clients' 10 s `/api/symbols` polling. Updates for stocks
/// crossing the band: the unit test in market.rs (the fixture's stocks never leave it).
#[tokio::test]
async fn the_candidate_list_is_pushed_and_follows_a_day_switch() {
    let s = stack_with_days(&[], DAYS, &[]).await;
    let mut ws = connect(&s.url).await;
    send(&mut ws, json!({"type": "symbols.subscribe"})).await;
    let snap = next(&mut ws, is("symbols.snapshot")).await;
    for k in ["mode", "status", "day", "fixture", "priceBand", "screened"] {
        assert!(snap.get(k).is_some(), "snapshot has {k}");
    }
    let (mut list, mut rev) = (Vec::new(), 0);
    fold_symbols(&mut list, &mut rev, &snap);
    while list != ["FXLO", "SMPL"] {
        let m = next(&mut ws, is("symbols.update")).await;
        fold_symbols(&mut list, &mut rev, &m);
    }
    let (_, http) = get(&s.app, "/api/symbols").await;
    assert_eq!(http["symbols"], json!(list));

    send(
        &mut ws,
        json!({"type":"replay.control","action":"day","value":"2026-10-14"}),
    )
    .await;
    // The rebuild's resync sends a fresh snapshot, then updates follow until the new day's list
    // and meta are in (the snapshot may be taken before all of its candidates have traded).
    let mut kinds = Vec::new();
    loop {
        let m = next(&mut ws, |m| {
            m["type"] == "symbols.update" || m["type"] == "symbols.snapshot"
        })
        .await;
        kinds.push(m["type"].as_str().unwrap().to_string());
        fold_symbols(&mut list, &mut rev, &m);
        // FXHI trades above $25 (only_price_band_candidates_are_let_through): never a candidate.
        if list == ["SMPL"] && m["day"] == "2026-10-14" {
            break;
        }
    }
    assert!(kinds.contains(&"symbols.snapshot".to_string()), "{kinds:?}");

    // Unsubscribed: nothing more, even across another switch.
    send(&mut ws, json!({"type": "symbols.unsubscribe"})).await;
    // Frames sent before the server handled it may still be in flight (a day switch resyncs
    // more than once); everything after the pong to this ping postdates it.
    send(&mut ws, json!({"type":"ping","ts":9})).await;
    next(&mut ws, |m| m["type"] == "pong" && m["ts"] == 9).await;
    send(
        &mut ws,
        json!({"type":"replay.control","action":"day","value":"2026-10-13"}),
    )
    .await;
    let t = types_for(&mut ws, Duration::from_secs(3)).await;
    assert!(!t.iter().any(|t| t.starts_with("symbols.")), "{t:?}");
}

#[tokio::test]
async fn replay_state_lists_the_archive_days() {
    let s = stack_with_days(&[("REPLAY_AUTOPLAY", "0")], DAYS, &[]).await;
    let (_, v) = get(&s.app, "/api/replay/state").await;
    // The latest day plays by default; every day is offered, ascending, with its manifest facts.
    assert_eq!(v["day"], "2026-10-15");
    assert_eq!(
        v["days"],
        json!([
            {"day": "2026-10-13", "fixture": true, "symbols": 3},
            {"day": "2026-10-14", "fixture": true, "symbols": 2},
            {"day": "2026-10-15", "fixture": true, "symbols": 2},
        ])
    );
}

#[tokio::test]
async fn switching_the_day_rebuilds_everything_like_a_seek_and_resyncs_every_client() {
    let s = stack_with_days(&[("REPLAY_AUTOPLAY", "0")], DAYS, &[]).await;
    let mut ws = connect(&s.url).await;
    let mut other = connect(&s.url).await;
    wait_for_replay(&mut ws).await;
    send(
        &mut ws,
        json!({"type":"chart.subscribe","symbol":"SMPL","timeframe":"1m","profileBin":0.01}),
    )
    .await;
    let before = next(&mut ws, is("chart.snapshot")).await;
    assert_eq!(before["clock"]["now"], T1040);
    send(&mut ws, json!({"type": "quotes.subscribe"})).await;
    next(&mut ws, is("quotes.snapshot")).await;
    send(
        &mut other,
        json!({"type":"chart.subscribe","symbol":"SMPL","timeframe":"5m","profileBin":0.01}),
    )
    .await;
    next(&mut other, is("chart.snapshot")).await;
    // A resting order far below the market: it can only go away by a reset.
    send(
        &mut ws,
        json!({"type":"order.place","clientOrderId":"rest","symbol":"SMPL","side":"buy",
               "orderType":"limit","qty":10,"limitPrice":0.11}),
    )
    .await;
    paper_state(&mut ws, |m| !m["orders"].as_array().unwrap().is_empty()).await;

    send(
        &mut ws,
        json!({"type":"replay.control","action":"day","value":"2026-10-14"}),
    )
    .await;
    // Time travel resets the account (docs/architecture/chart-trading-sim.md).
    let flat = paper_state(&mut ws, |m| m["orders"].as_array().unwrap().is_empty()).await;
    assert_eq!(flat["account"]["cash"], 100_000.0);
    // The new day starts paused at the configured 10:40 ET, and both subscribers get a fresh
    // snapshot of it (quotes first, then the chart: the Resync order).
    let clock = next(&mut ws, |m| {
        m["type"] == "replay.clock" && m["clock"]["now"] == T1040_OCT14
    })
    .await;
    assert_eq!(clock["clock"]["playing"], false);
    let quotes = next(&mut ws, is("quotes.snapshot")).await;
    assert_eq!(quotes["t"], T1040_OCT14);
    let snap = next(&mut ws, is("chart.snapshot")).await;
    assert_eq!(snap["clock"]["now"], T1040_OCT14);
    let candles = snap["candles"].as_array().unwrap();
    assert!(!candles.is_empty());
    // Nothing of the old day survived: every candle is inside Oct 14's trading day, before 10:40.
    assert!(
        candles.iter().all(|c| {
            let t = c["t"].as_i64().unwrap();
            (OPEN_OCT14..T1040_OCT14).contains(&t)
        }),
        "only Oct 14 candles"
    );
    assert_eq!(
        quote(&quotes, "SMPL").unwrap()["v"].as_u64().unwrap(),
        total_volume(&snap)
    );
    let snap5 = next(&mut other, |m| {
        m["type"] == "chart.snapshot" && m["clock"]["now"] == T1040_OCT14
    })
    .await;
    assert_eq!(snap5["timeframe"], "5m");

    // The provider description follows the switch.
    let (_, st) = get(&s.app, "/api/replay/state").await;
    assert_eq!(st["day"], "2026-10-14");
    assert!(
        st["source"].as_str().unwrap().contains("2026-10-14"),
        "{st}"
    );
    let (_, sy) = get(&s.app, "/api/symbols").await;
    assert_eq!(sy["day"], "2026-10-14");
    assert_eq!(
        sy["screened"]["total"], 2,
        "the new day's symbols: SMPL, FXHI"
    );

    // The rebuilt state is exactly what a server started on that day shows.
    let fresh = stack_with_days(
        &[("REPLAY_AUTOPLAY", "0"), ("REPLAY_DAY", "2026-10-14")],
        DAYS,
        &[],
    )
    .await;
    let mut f = connect(&fresh.url).await;
    send(
        &mut f,
        json!({"type":"chart.subscribe","symbol":"SMPL","timeframe":"1m","profileBin":0.01}),
    )
    .await;
    let want = next(&mut f, |m| {
        m["type"] == "chart.snapshot" && m["clock"]["now"] == T1040_OCT14
    })
    .await;
    assert_eq!(snap["candles"], want["candles"]);
    assert_eq!(snap["profile"], want["profile"]);

    // Switching back works the same way.
    send(
        &mut ws,
        json!({"type":"replay.control","action":"day","value":"2026-10-15"}),
    )
    .await;
    let back = next(&mut ws, |m| {
        m["type"] == "chart.snapshot" && m["clock"]["now"] == T1040
    })
    .await;
    assert_eq!(back["candles"], before["candles"]);
}

#[tokio::test]
async fn a_symbol_missing_on_the_new_day_is_reported_on_the_resync() {
    let s = stack_with_days(&[("REPLAY_AUTOPLAY", "0")], DAYS, &[]).await;
    let mut ws = connect(&s.url).await;
    wait_for_replay(&mut ws).await;
    send(
        &mut ws,
        json!({"type":"chart.subscribe","symbol":"FXLO","timeframe":"1m","profileBin":0.01}),
    )
    .await;
    next(&mut ws, is("chart.snapshot")).await;
    send(
        &mut ws,
        json!({"type":"replay.control","action":"day","value":"2026-10-14"}),
    )
    .await;
    let e = next(&mut ws, is("error")).await;
    assert_eq!(e["code"], "unknown_symbol");
}

#[tokio::test]
async fn an_unknown_or_malformed_day_is_refused_and_nothing_changes() {
    let s = stack_with_days(&[("REPLAY_AUTOPLAY", "0")], DAYS, &[]).await;
    let mut ws = connect(&s.url).await;
    wait_for_replay(&mut ws).await;
    let (_, before) = get(&s.app, "/api/replay/state").await;
    // Not in the archive, and a weekend (never replayable).
    for day in ["2026-10-09", "2026-10-17"] {
        send(
            &mut ws,
            json!({"type":"replay.control","action":"day","value":day}),
        )
        .await;
        let e = next(&mut ws, is("error")).await;
        assert_eq!(e["code"], "unknown_day", "{day}");
    }
    for bad in [json!("Oct 14"), json!(20261014), Value::Null] {
        send(
            &mut ws,
            json!({"type":"replay.control","action":"day","value":bad}),
        )
        .await;
        let e = next(&mut ws, is("error")).await;
        assert_eq!(e["code"], "bad_request", "{bad}");
    }
    let (_, after) = get(&s.app, "/api/replay/state").await;
    assert_eq!(after["day"], "2026-10-15");
    assert_eq!(after["clock"], before["clock"]);
    assert_eq!(after["events"], before["events"]);
}

#[tokio::test]
async fn live_mode_offers_no_days_and_refuses_a_day_switch() {
    let s = stack_with_days(&[("MARKET_DATA_MODE", "live")], DAYS, &[]).await;
    let (_, v) = get(&s.app, "/api/replay/state").await;
    assert_eq!(v["days"], json!([]));
    let mut ws = connect(&s.url).await;
    send(
        &mut ws,
        json!({"type":"replay.control","action":"day","value":"2026-10-14"}),
    )
    .await;
    let e = next(&mut ws, is("error")).await;
    assert_eq!(e["code"], "not_replay");
}

#[tokio::test]
async fn after_a_day_switch_the_previous_close_is_the_new_days_previous_close() {
    // A distinct close per date, so a reference taken from the wrong day cannot pass.
    let closes = json!({
        "2026-10-12": {"SMPL": 2.12},
        "2026-10-13": {"SMPL": 2.13},
        "2026-10-14": {"SMPL": 2.14},
        "2026-10-15": {"SMPL": 2.15},
    });
    let s = stack_with_days(
        &[("REPLAY_AUTOPLAY", "0")],
        DAYS,
        &[("daily-closes.json", closes)],
    )
    .await;
    let mut ws = connect(&s.url).await;
    wait_for_replay(&mut ws).await;
    send(&mut ws, json!({"type": "quotes.subscribe"})).await;
    let q = next(&mut ws, is("quotes.snapshot")).await;
    assert_eq!(
        quote(&q, "SMPL").unwrap()["pc"],
        2.14,
        "Oct 15 → Oct 14's close"
    );
    send(
        &mut ws,
        json!({"type":"chart.subscribe","symbol":"SMPL","timeframe":"1m","profileBin":0.01}),
    )
    .await;
    let snap = next(&mut ws, is("chart.snapshot")).await;
    assert_eq!(snap["prevClose"], 2.14);

    for (day, now, want) in [
        ("2026-10-13", T1040 - 2 * 86_400_000, 2.12),
        ("2026-10-14", T1040_OCT14, 2.13),
    ] {
        send(
            &mut ws,
            json!({"type":"replay.control","action":"day","value":day}),
        )
        .await;
        // The resync's snapshots, both taken on the rebuilt state of the new day.
        let q = next(&mut ws, |m| m["type"] == "quotes.snapshot" && m["t"] == now).await;
        assert_eq!(quote(&q, "SMPL").unwrap()["pc"], want, "{day}: quotes pc");
        let snap = next(&mut ws, |m| {
            m["type"] == "chart.snapshot" && m["clock"]["now"] == now
        })
        .await;
        assert_eq!(snap["prevClose"], want, "{day}: chart prevClose");
        // A fresh subscribe (not only the resync) agrees.
        send(
            &mut ws,
            json!({"type":"chart.subscribe","symbol":"SMPL","timeframe":"5m","profileBin":0.01}),
        )
        .await;
        let again = next(&mut ws, |m| {
            m["type"] == "chart.snapshot" && m["timeframe"] == "5m"
        })
        .await;
        assert_eq!(again["prevClose"], want, "{day}: resubscribe prevClose");
        send(
            &mut ws,
            json!({"type":"chart.subscribe","symbol":"SMPL","timeframe":"1m","profileBin":0.01}),
        )
        .await;
        next(&mut ws, |m| {
            m["type"] == "chart.snapshot" && m["timeframe"] == "1m"
        })
        .await;
    }
}

/// A running live provider (not the idle "not configured" one): its link state is the server's
/// status, and there are no replay days to offer.
#[tokio::test]
async fn a_running_live_feed_reports_its_link_and_offers_no_days() {
    use market_events::MarketEvent;
    use market_providers::{
        Link, LinkState, LiveFeed, LiveOptions, LiveProvider, MarketDataProvider, ProviderError,
    };
    use tokio::sync::{mpsc, watch};

    struct Feed {
        link: watch::Receiver<Link>,
        rx: mpsc::Receiver<MarketEvent>,
    }
    impl LiveFeed for Feed {
        fn label(&self) -> String {
            "test link".into()
        }
        fn symbols(&self) -> Vec<String> {
            Vec::new()
        }
        fn link(&mut self) -> Option<watch::Receiver<Link>> {
            Some(self.link.clone())
        }
        fn connect(self: Box<Self>) -> Result<mpsc::Receiver<MarketEvent>, ProviderError> {
            Ok(self.rx)
        }
    }

    let (link_tx, link) = watch::channel(Link {
        state: LinkState::Connecting,
        detail: Some("connecting".into()),
    });
    let (_tx, rx) = mpsc::channel(16);
    let session = Box::new(LiveProvider::with_feed(
        Box::new(Feed { link, rx }),
        LiveOptions {
            idle_tick: None,
            ..LiveOptions::default()
        },
    ))
    .start()
    .unwrap();
    let market = Market::from_session(session, None);
    let state = |want: &'static str| {
        let market = market.clone();
        async move {
            for _ in 0..500 {
                let v = market.state_json();
                if v["status"]["state"] == want {
                    return v;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            panic!("status never became {want}: {}", market.state_json())
        }
    };
    let v = state("starting").await;
    assert_eq!(v["mode"], "live");
    assert_eq!(v["days"], json!([]));
    assert_eq!(v["controllable"], false);

    link_tx.send_replace(Link {
        state: LinkState::Up,
        detail: Some("subscribed".into()),
    });
    state("running").await;
    link_tx.send_replace(Link {
        state: LinkState::Down,
        detail: Some("connection lost; reconnecting in 1.0 s".into()),
    });
    let v = state("reconnecting").await;
    assert_eq!(
        v["status"]["detail"],
        "connection lost; reconnecting in 1.0 s"
    );
    assert_eq!(v["days"], json!([]));
}

fn fixture_archive(root: &std::path::Path) -> market_archive::Archive {
    market_archive::Archive::new(market_archive::ArchiveConfig {
        root: root.to_path_buf(),
        retention_days: 0,
        min_free_bytes: 0,
    })
}

/// Polls `GET uri` until `pred` holds (the meta task runs every second in these tests).
async fn eventually(app: &axum::Router, uri: &str, pred: impl Fn(&Value) -> bool) -> Value {
    for _ in 0..100 {
        let (_, v) = get(app, uri).await;
        if pred(&v) {
            return v;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let (_, v) = get(app, uri).await;
    panic!("{uri} never matched; last: {v}");
}

#[tokio::test]
async fn assets_and_closes_rewritten_by_the_maintainer_are_picked_up_without_a_restart() {
    let s = stack_with_files(
        &[("REPLAY_AUTOPLAY", "0"), ("ARCHIVE_META_CHECK_SECS", "1")],
        &[
            (
                "assets.json",
                json!({"SMPL": {"name": "Old Name", "exchange": "NYSE"}}),
            ),
            ("daily-closes.json", json!({"2026-10-14": {"SMPL": 4.25}})),
        ],
    )
    .await;
    let mut ws = connect(&s.url).await;
    wait_for_replay(&mut ws).await;
    let (_, q) = get(&s.app, "/api/quotes").await;
    assert_eq!(quote(&q, "SMPL").unwrap()["name"], "Old Name");
    assert_eq!(quote(&q, "SMPL").unwrap()["pc"], 4.25);
    assert!(quote(&q, "FXLO").unwrap().get("pc").is_none());
    send(&mut ws, json!({"type": "quotes.subscribe"})).await;
    next(&mut ws, is("quotes.snapshot")).await;

    // The maintainer's atomic rewrites (temp file + rename), as market_archive writes them.
    let archive = fixture_archive(&s.root);
    let mut assets = market_archive::AssetMap::new();
    assets.insert(
        "SMPL".into(),
        market_archive::AssetMeta {
            name: "Sample Holdings Corp".into(),
            exchange: "NASDAQ".into(),
        },
    );
    archive.write_assets(&assets).unwrap();
    let mut closes = market_archive::DailyCloses::new();
    let d14: chrono::NaiveDate = "2026-10-14".parse().unwrap();
    closes.entry(d14).or_default().insert("SMPL".into(), 4.25);
    closes.entry(d14).or_default().insert("FXLO".into(), 0.55);
    archive.write_daily_closes(&closes).unwrap();

    let q = eventually(&s.app, "/api/quotes", |q| {
        quote(q, "SMPL").is_some_and(|r| r["name"] == "Sample Holdings Corp")
            && quote(q, "FXLO").is_some_and(|r| r["pc"] == 0.55)
    })
    .await;
    assert_eq!(quote(&q, "SMPL").unwrap()["x"], "NASDAQ");
    assert_eq!(quote(&q, "SMPL").unwrap()["pc"], 4.25, "unchanged close");
    // The running board resends only the reference that changed, and the chart agrees.
    let u = next(&mut ws, |m| {
        m["type"] == "quotes.update" && quote(m, "FXLO").is_some()
    })
    .await;
    assert_eq!(quote(&u, "FXLO").unwrap()["pc"], 0.55);
    send(
        &mut ws,
        json!({"type":"chart.subscribe","symbol":"FXLO","timeframe":"1m","profileBin":0.01}),
    )
    .await;
    assert_eq!(next(&mut ws, is("chart.snapshot")).await["prevClose"], 0.55);
    // And a rebuild (seek) starts from the new file too.
    send(
        &mut ws,
        json!({"type":"replay.control","action":"seek","value":T1040 + 60_000}),
    )
    .await;
    let snap = next(&mut ws, |m| {
        m["type"] == "quotes.snapshot" && m["t"] == T1040 + 60_000
    })
    .await;
    assert_eq!(quote(&snap, "FXLO").unwrap()["pc"], 0.55);
    assert_eq!(
        quote(&snap, "SMPL").unwrap()["name"],
        "Sample Holdings Corp"
    );
}

fn day_list(v: &Value) -> Vec<String> {
    v["days"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["day"].as_str().unwrap().to_string())
        .collect()
}

#[tokio::test]
async fn the_day_list_follows_imports_and_removals_and_the_replay_stays_put_by_default() {
    let s = stack_with_days(
        &[("REPLAY_AUTOPLAY", "0"), ("ARCHIVE_META_CHECK_SECS", "1")],
        DAYS,
        &[],
    )
    .await;
    let archive = fixture_archive(&s.root);
    // A day being imported is not offered (nor can it be switched to) until its marker goes.
    let d16: chrono::NaiveDate = "2026-10-16".parse().unwrap();
    archive.mark_importing(d16).unwrap();
    market_archive::fixture::write_fixture_day(
        &archive,
        d16,
        &["SMPL".to_string()],
        market_archive::fixture::DEFAULT_SEED,
    )
    .unwrap();
    let (_, v) = get(&s.app, "/api/replay/state").await;
    assert_eq!(day_list(&v), ["2026-10-13", "2026-10-14", "2026-10-15"]);
    let mut ws = connect(&s.url).await;
    wait_for_replay(&mut ws).await;
    send(
        &mut ws,
        json!({"type":"replay.control","action":"day","value":"2026-10-16"}),
    )
    .await;
    assert_eq!(next(&mut ws, is("error")).await["code"], "unknown_day");
    archive.clear_importing(d16).unwrap();
    archive.remove_day("2026-10-13".parse().unwrap()).unwrap();
    let (_, v) = get(&s.app, "/api/replay/state").await;
    assert_eq!(day_list(&v), ["2026-10-14", "2026-10-15", "2026-10-16"]);
    // Without REPLAY_FOLLOW_LATEST the replay does not move on its own.
    tokio::time::sleep(Duration::from_millis(2500)).await;
    let (_, v) = get(&s.app, "/api/replay/state").await;
    assert_eq!(v["day"], "2026-10-15");
}

#[tokio::test]
async fn follow_latest_switches_to_a_newly_imported_day_once() {
    let s = stack_with_days(
        &[
            ("REPLAY_AUTOPLAY", "0"),
            ("ARCHIVE_META_CHECK_SECS", "1"),
            ("REPLAY_FOLLOW_LATEST", "1"),
        ],
        DAYS,
        &[],
    )
    .await;
    let archive = fixture_archive(&s.root);
    let mut ws = connect(&s.url).await;
    wait_for_replay(&mut ws).await;
    // Nothing new: it stays on the latest day it started on.
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert_eq!(
        get(&s.app, "/api/replay/state").await.1["day"],
        "2026-10-15"
    );
    // A day still importing is not followed...
    let d16: chrono::NaiveDate = "2026-10-16".parse().unwrap();
    archive.mark_importing(d16).unwrap();
    market_archive::fixture::write_fixture_day(
        &archive,
        d16,
        &["SMPL".to_string()],
        market_archive::fixture::DEFAULT_SEED,
    )
    .unwrap();
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert_eq!(
        get(&s.app, "/api/replay/state").await.1["day"],
        "2026-10-15"
    );
    // ...until the maintainer releases it.
    archive.clear_importing(d16).unwrap();
    eventually(&s.app, "/api/replay/state", |v| v["day"] == "2026-10-16").await;
    // Switching back by hand sticks: the day was followed once.
    send(
        &mut ws,
        json!({"type":"replay.control","action":"day","value":"2026-10-14"}),
    )
    .await;
    eventually(&s.app, "/api/replay/state", |v| v["day"] == "2026-10-14").await;
    tokio::time::sleep(Duration::from_millis(2500)).await;
    assert_eq!(
        get(&s.app, "/api/replay/state").await.1["day"],
        "2026-10-14"
    );
}

#[tokio::test]
async fn settings_sync_ignores_replay_seeks_and_works_the_same_in_live_mode() {
    // Replay: a seek rebuilds the market (paper account, tape, quotes), never the settings.
    let s = stack(&[]).await;
    let mut a = connect(&s.url).await;
    wait_for_replay(&mut a).await;
    send(
        &mut a,
        json!({"type":"settings.set","ns":"layers","value":{"halts":false},"opId":"a-1"}),
    )
    .await;
    next(&mut a, |m| {
        m["type"] == "settings.changed" && m["opId"] == "a-1"
    })
    .await;
    send(
        &mut a,
        json!({"type":"replay.control","action":"seek","value": T1040 - 600_000}),
    )
    .await;
    next(&mut a, |m| {
        m["type"] == "replay.clock" && m["clock"]["now"].as_i64().is_some_and(|n| n < T1040)
    })
    .await;
    let mut b = connect(&s.url).await;
    let snap = next(&mut b, is("settings.snapshot")).await;
    assert_eq!(
        snap["docs"],
        json!([{"ns":"layers","rev":1,"updatedAt":snap["docs"][0]["updatedAt"],"value":{"halts":false}}])
    );

    // Live mode (no provider configured): the same protocol.
    let s = stack(&[("MARKET_DATA_MODE", "live")]).await;
    let mut ws = connect(&s.url).await;
    next(&mut ws, is("settings.snapshot")).await;
    send(
        &mut ws,
        json!({"type":"settings.set","ns":"indicators","value":[],"opId":"l-1"}),
    )
    .await;
    let m = next(&mut ws, is("settings.changed")).await;
    assert_eq!(
        (m["rev"].clone(), m["opId"].clone()),
        (json!(1), json!("l-1"))
    );
}

// --- Halt watcher (pressure.subscribe / pressure.snapshot / pressure.update) -----------------

/// The fixture day halts every symbol 10:05–10:06:30 (halts-luld-fills.md), out of a scheduled
/// silence rather than a run at the band, and none of its symbols builds up toward a band all day
/// (the calibration bench, crates/server/tests/halt_bench.rs, ranks zero cards on it). Replayed
/// from 10:04 through the halt and the resume, a subscriber gets a snapshot with an empty board
/// and no card ever: a halt nobody was building toward is not a card, and an empty board is the
/// normal state (owner, 2026-10-20). A connection that did not subscribe never sees a pressure
/// frame. The card's wire shape is tested in market.rs (`a_halt_watcher_card_on_the_wire`) and
/// the board's behaviour with cards in pressure.rs.
#[tokio::test]
async fn pressure_board_stays_empty_without_a_build_up_even_through_a_halt() {
    const T1004: i64 = T1040 - 36 * 60_000;
    const T1007: i64 = T1040 - 33 * 60_000;
    let s = stack(&[
        ("SIM_FIXTURE_SYMBOLS", "SMPL,FXLO,FXHI"),
        ("REPLAY_START", "10:04"),
        ("REPLAY_SPEED", "30"),
    ])
    .await;
    let mut ws = connect(&s.url).await;
    let mut bystander = connect(&s.url).await;
    next(&mut ws, |m| {
        m["type"] == "replay.clock" && m["clock"]["now"].as_i64().unwrap_or(0) >= T1004
    })
    .await;
    send(&mut ws, json!({"type": "pressure.subscribe"})).await;
    let snap = next(&mut ws, is("pressure.snapshot")).await;
    assert_eq!(snap["board"], json!([]), "{snap}");

    // Through the halt (10:05) and the resume (10:06:30): any pressure frame would carry cards.
    loop {
        let m = next(&mut ws, |m| {
            m["type"] == "replay.clock"
                || m["type"]
                    .as_str()
                    .is_some_and(|t| t.starts_with("pressure."))
        })
        .await;
        assert_eq!(m["type"], "replay.clock", "no build-up, no card: {m}");
        if m["clock"]["now"].as_i64().unwrap_or(0) >= T1007 {
            break;
        }
    }

    let seen = types_for(&mut bystander, Duration::from_millis(1500)).await;
    assert!(!seen.iter().any(|t| t.starts_with("pressure.")), "{seen:?}");

    send(&mut ws, json!({"type": "pressure.unsubscribe"})).await;
    send(&mut ws, json!({"type": "ping", "ts": 7})).await;
    next(&mut ws, |m| m["type"] == "pong" && m["ts"] == 7).await;
    let seen = types_for(&mut ws, Duration::from_millis(2500)).await;
    assert!(!seen.iter().any(|t| t.starts_with("pressure.")), "{seen:?}");
}

/// Stock context off (the default): `ross.subscribe` answers `enabled: false` (the client shows
/// "not enabled", never an empty panel that would read as "nothing qualifies"), and
/// `context.get` the same; no `ross.update` ever comes.
#[tokio::test]
async fn ross_panel_says_disabled_when_stock_context_is_off() {
    let s = stack(&[
        ("SIM_FIXTURE_SYMBOLS", "SMPL,FXLO"),
        ("REPLAY_START", "10:04"),
    ])
    .await;
    let mut ws = connect(&s.url).await;
    send(&mut ws, json!({"type": "ross.subscribe"})).await;
    let snap = next(&mut ws, is("ross.snapshot")).await;
    assert_eq!(snap["enabled"], false);
    assert!(snap["panel"].is_null());
    send(&mut ws, json!({"type": "context.get", "symbol": "smpl"})).await;
    let d = next(&mut ws, is("context.decision")).await;
    assert_eq!(
        (d["symbol"].as_str(), d["enabled"].as_bool()),
        (Some("SMPL"), Some(false))
    );
    assert!(d["decision"].is_null());
}

/// Stock context on, replaying the fixture day from an archive with no stored evidence (no daily
/// bars, corporate-action checks, floats or news): every traded symbol gets decisions with the
/// five pillars under the current policy, the gain pillar UNKNOWN (no checked previous close), and the
/// Ross Panel enabled but empty (nothing can pass five pillars without that evidence; the server
/// test with it is `evidence::tests`). A bystander that did not subscribe sees no `ross.*` frame.
#[tokio::test]
async fn stock_context_decides_every_traded_symbol_and_the_panel_stays_honestly_empty() {
    let s = stack(&[
        ("SIM_FIXTURE_SYMBOLS", "SMPL,FXLO"),
        ("REPLAY_START", "10:04"),
        ("REPLAY_SPEED", "30"),
        ("STOCK_CONTEXT", "1"),
    ])
    .await;
    let mut ws = connect(&s.url).await;
    let mut bystander = connect(&s.url).await;
    send(&mut ws, json!({"type": "ross.subscribe"})).await;
    let snap = next(&mut ws, is("ross.snapshot")).await;
    assert_eq!(snap["enabled"], true, "{snap}");
    assert!(
        snap["panelPolicy"]
            .as_str()
            .unwrap()
            .starts_with("ross-panel-v1@")
    );
    assert!(
        snap["pillarPolicy"]
            .as_str()
            .unwrap()
            .starts_with("pillars-v6@")
    );
    // Let the replay run a few decision barriers.
    next(&mut ws, |m| {
        m["type"] == "replay.clock"
            && m["clock"]["now"].as_i64().unwrap_or(0) >= T1040 - 34 * 60_000
    })
    .await;
    send(&mut ws, json!({"type": "context.get", "symbol": "SMPL"})).await;
    let d = next(&mut ws, is("context.decision")).await;
    assert_eq!(d["enabled"], true);
    let dec = &d["decision"];
    assert_eq!(dec["symbol"], "SMPL", "{d}");
    assert_eq!(dec["source"], "replay");
    assert_eq!(dec["fidelity"], "reconstructed_history");
    let pillars = dec["pillars"].as_array().unwrap();
    assert_eq!(pillars.len(), 5);
    let gain = pillars.iter().find(|p| p["pillar"] == "gain").unwrap();
    assert_eq!(gain["state"], "UNKNOWN", "{gain}");
    let rvol = pillars.iter().find(|p| p["pillar"] == "rvol").unwrap();
    assert_eq!(
        (rvol["state"].as_str(), rvol["reason"].as_str()),
        (Some("UNKNOWN"), Some("no_daily_baseline"))
    );
    assert_ne!(dec["qualification"], "QUALIFIED");
    // No member: the panel's rows stay empty however long it runs.
    let seen = types_for(&mut ws, Duration::from_millis(2500)).await;
    assert!(
        !seen.iter().any(|t| t == "ross.update"),
        "an empty panel never changes: {seen:?}"
    );
    let seen = types_for(&mut bystander, Duration::from_millis(500)).await;
    assert!(!seen.iter().any(|t| t.starts_with("ross.")), "{seen:?}");
}

// --- Server-computed indicators (`indicators.v1`, crate::indicators) -----------------------------

/// A client's model of its indicator values: per (spec key, line), value by bar time (warm-up
/// nulls are not held). Folded exactly as a client folds the frames.
#[derive(Debug, Default, PartialEq)]
struct IndicatorModel(
    std::collections::BTreeMap<(String, usize), std::collections::BTreeMap<i64, Value>>,
);

impl IndicatorModel {
    fn from_snapshot(f: &Value) -> Self {
        let mut m = IndicatorModel::default();
        m.apply_snapshot(f);
        m
    }

    /// `chart.indicators` replaces the keys it carries.
    fn apply_snapshot(&mut self, f: &Value) {
        assert_eq!(f["type"], "chart.indicators");
        let t: Vec<i64> = f["t"]
            .as_array()
            .unwrap()
            .iter()
            .map(|x| x.as_i64().unwrap())
            .collect();
        for st in f["studies"].as_array().unwrap() {
            let key = st["key"].as_str().unwrap().to_string();
            for (j, line) in st["lines"].as_array().unwrap().iter().enumerate() {
                let from = line["from"].as_u64().unwrap() as usize;
                let col = self.0.entry((key.clone(), j)).or_default();
                col.clear();
                for (i, v) in line["values"].as_array().unwrap().iter().enumerate() {
                    if !v.is_null() {
                        col.insert(t[from + i], v.clone());
                    }
                }
            }
        }
    }

    /// `chart.indicator`: removals first, then each point's values per key (only keys held).
    fn apply_increment(&mut self, f: &Value) {
        assert_eq!(f["type"], "chart.indicator");
        for t in f["removed"].as_array().into_iter().flatten() {
            for col in self.0.values_mut() {
                col.remove(&t.as_i64().unwrap());
            }
        }
        for p in f["points"].as_array().unwrap() {
            let t = p["t"].as_i64().unwrap();
            for (key, vals) in p["v"].as_object().unwrap() {
                for (j, v) in vals.as_array().unwrap().iter().enumerate() {
                    let Some(col) = self.0.get_mut(&(key.clone(), j)) else {
                        continue;
                    };
                    if v.is_null() {
                        col.remove(&t);
                    } else {
                        col.insert(t, v.clone());
                    }
                }
            }
        }
    }

    fn apply(&mut self, f: &Value) {
        if f["type"] == "chart.indicators" {
            self.apply_snapshot(f);
        } else {
            self.apply_increment(f);
        }
    }
}

fn studies_list() -> Value {
    json!([
        {"id": "vwap", "kind": "vwap"},
        {"id": "ema-9", "kind": "ema", "period": 9},
        {"id": "bb-20", "kind": "bb", "period": 20, "mult": 2},
        {"id": "rsi-14", "kind": "rsi", "period": 14},
        {"id": "macd", "kind": "macd", "fast": 12, "slow": 26, "signal": 9}
    ])
}

fn is_indicator(m: &Value) -> bool {
    m["type"] == "chart.indicator" || m["type"] == "chart.indicators"
}

/// The welcome advertises the feature; a slot that sends its list gets `chart.indicators` right
/// after its `chart.snapshot`, then increments that, folded on a client model while the replay
/// plays, give exactly the values a fresh snapshot of the same chart holds once it is paused.
/// On 15 s candles, through the fixture day's 11:00 correction (published 30 s late, so it
/// revises a bar two back): the increments carry older bars too, not only the forming one.
#[tokio::test]
async fn indicator_increments_fold_to_the_snapshot() {
    let s = stack(&[("REPLAY_SPEED", "300")]).await;
    let (mut ws, _) = tokio_tungstenite::connect_async(&s.url).await.unwrap();
    send(&mut ws, json!({"type": "hello", "client": "test"})).await;
    let w = next(&mut ws, is("welcome")).await;
    assert!(
        w["features"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f == "indicators.v1"),
        "{w}"
    );
    wait_for_replay(&mut ws).await;
    send(
        &mut ws,
        json!({"type": "chart.subscribe", "id": "a", "symbol": "SMPL", "timeframe": "15s",
               "profileBin": 0.01, "indicators": studies_list(), "rth": false}),
    )
    .await;
    next(&mut ws, is("chart.snapshot")).await;
    // The very next frame: same core lock, same seq.
    let first = next(&mut ws, |m| m["type"] != "replay.clock").await;
    assert_eq!(first["type"], "chart.indicators", "{first}");
    assert_eq!(
        (
            first["symbol"].as_str(),
            first["timeframe"].as_str(),
            first["rth"].as_bool()
        ),
        (Some("SMPL"), Some("15s"), Some(false))
    );
    let keys: Vec<&str> = first["studies"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["key"].as_str().unwrap())
        .collect();
    assert_eq!(keys, ["vwap", "ema:9", "bb:20:2", "rsi:14", "macd:12:26:9"]);
    assert!(first["t"].as_array().unwrap().len() > 100, "the day so far");
    let macd = first["studies"][4]["lines"].as_array().unwrap();
    let names: Vec<&str> = macd.iter().map(|l| l["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["Hist", "MACD", "Signal"]);
    assert_eq!(macd[0]["style"], "histogram");
    let mut model = IndicatorModel::from_snapshot(&first);
    let vwap_len = |m: &IndicatorModel| m.0.get(&("vwap".to_string(), 0)).map_or(0, |c| c.len());
    let start_len = vwap_len(&model);

    // Fold increments while the replay plays (300x), until past 11:01:30.
    let (mut increments, mut older) = (0, 0);
    loop {
        let m = next(&mut ws, |m| is_indicator(m) || m["type"] == "replay.clock").await;
        if m["type"] == "replay.clock" {
            if m["clock"]["now"].as_i64().unwrap() >= T1040 + 21 * 60_000 + 30_000 {
                break;
            }
            continue;
        }
        assert_eq!(
            (m["symbol"].as_str(), m["timeframe"].as_str()),
            (Some("SMPL"), Some("15s"))
        );
        let newest = model
            .0
            .get(&("vwap".to_string(), 0))
            .and_then(|c| c.keys().next_back().copied())
            .unwrap_or(0);
        if m["type"] == "chart.indicator"
            && m["points"]
                .as_array()
                .unwrap()
                .iter()
                .any(|p| p["t"].as_i64().unwrap() < newest)
        {
            older += 1;
        }
        model.apply(&m);
        increments += 1;
    }
    assert!(increments > 100, "increments: {increments}");
    assert!(
        vwap_len(&model) > start_len,
        "the replay ran across new bars"
    );
    assert!(older > 0, "a revision of an older bar came as an increment");
    send(
        &mut ws,
        json!({"type": "replay.control", "action": "pause"}),
    )
    .await;
    // Paused: fold the tail until the indicator stream has been quiet for a while.
    while let Ok(m) =
        tokio::time::timeout(Duration::from_millis(1500), next(&mut ws, is_indicator)).await
    {
        model.apply(&m);
    }
    // A second connection's snapshot of the same chart key: the shared series, now.
    let mut other = connect(&s.url).await;
    send(
        &mut other,
        json!({"type": "chart.subscribe", "symbol": "SMPL", "timeframe": "15s", "profileBin": 0.01,
               "indicators": studies_list()}),
    )
    .await;
    let fresh = IndicatorModel::from_snapshot(&next(&mut other, is("chart.indicators")).await);
    assert_eq!(fresh.0.len(), 1 + 1 + 3 + 1 + 3);
    for (k, col) in &fresh.0 {
        assert_eq!(
            model.0.get(k),
            Some(col),
            "{k:?}: folded increments != snapshot"
        );
    }
    assert_eq!(model, fresh);
}

/// Old phone builds send no list: they get exactly the frames they got before and no indicator
/// frame. A list that is not a list is refused for its slot; `rth` keeps the regular session;
/// `chart.indicators.set` changes a slot's studies and `[]` turns them off.
#[tokio::test]
async fn indicators_are_opt_in_per_slot() {
    let s = stack(&[]).await;
    let mut old = connect(&s.url).await;
    let mut new = connect(&s.url).await;
    wait_for_replay(&mut old).await;
    wait_for_replay(&mut new).await;
    send(&mut old, sub("", "SMPL", "1m")).await;
    next(&mut old, is("chart.snapshot")).await;
    let seen = types_for(&mut old, Duration::from_millis(2500)).await;
    assert!(seen.iter().any(|t| t == "chart.update"), "{seen:?}");
    assert!(
        !seen.iter().any(|t| t.starts_with("chart.indicator")),
        "{seen:?}"
    );

    send(
        &mut new,
        json!({"type": "chart.subscribe", "id": "x", "symbol": "SMPL", "timeframe": "1m",
               "indicators": {"id": "e"}}),
    )
    .await;
    let e = next(&mut new, is("error")).await;
    assert_eq!(
        (e["code"].as_str(), e["id"].as_str(), e["symbol"].as_str()),
        (Some("bad_request"), Some("x"), Some("SMPL"))
    );

    send(
        &mut new,
        json!({"type": "chart.subscribe", "id": "x", "symbol": "SMPL", "timeframe": "5m",
               "indicators": [{"id": "e", "kind": "ema", "period": 9}], "rth": true}),
    )
    .await;
    let snap = next(&mut new, is("chart.snapshot")).await;
    let ind = next(&mut new, is("chart.indicators")).await;
    assert_eq!(ind["rth"], true);
    // 2026-10-15 09:30 ET.
    let regular_open = 1_792_071_000_000;
    let t: Vec<i64> = ind["t"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x.as_i64().unwrap())
        .collect();
    assert!(
        !t.is_empty() && t.iter().all(|&x| x >= regular_open),
        "regular session only"
    );
    assert!(
        snap["candles"].as_array().unwrap().len() > t.len(),
        "the chart itself still shows every session"
    );
    let inc = next(&mut new, is("chart.indicator")).await;
    assert_eq!(
        (inc["timeframe"].as_str(), inc["rth"].as_bool()),
        (Some("5m"), Some(true))
    );

    // Other studies, without a new candle snapshot.
    send(
        &mut new,
        json!({"type": "chart.indicators.set", "id": "x", "indicators": [{"id": "r", "kind": "rsi"}], "rth": true}),
    )
    .await;
    let set = next(&mut new, is("chart.indicators")).await;
    assert_eq!(set["studies"][0]["key"], "rsi:14");
    // All off: an empty frame, then no more increments.
    send(
        &mut new,
        json!({"type": "chart.indicators.set", "id": "x", "indicators": []}),
    )
    .await;
    let off = next(&mut new, is("chart.indicators")).await;
    assert!(off["studies"].as_array().unwrap().is_empty());
    send(&mut new, json!({"type": "ping", "ts": 9})).await;
    next(&mut new, |m| m["type"] == "pong" && m["ts"] == 9).await;
    let seen = types_for(&mut new, Duration::from_millis(2000)).await;
    assert!(seen.iter().any(|t| t == "chart.update"), "{seen:?}");
    assert!(!seen.iter().any(|t| t == "chart.indicator"), "{seen:?}");
    // An id no slot holds.
    send(
        &mut new,
        json!({"type": "chart.indicators.set", "id": "nope", "indicators": []}),
    )
    .await;
    let e = next(&mut new, is("error")).await;
    assert_eq!(
        (e["code"].as_str(), e["id"].as_str()),
        (Some("bad_request"), Some("nope"))
    );
}

/// `limit` (`indicators.limit.v1`): the slot's `chart.indicators` carries only the newest bars,
/// says so with `truncated`, and holds exactly the values the unlimited frame has at those bars;
/// `chart.indicators.set` without it sends every bar again.
#[tokio::test]
async fn a_limit_shortens_the_indicator_snapshot_only() {
    let s = stack(&[]).await;
    let (mut ws, _) = tokio_tungstenite::connect_async(&s.url).await.unwrap();
    send(&mut ws, json!({"type": "hello", "client": "test"})).await;
    let w = next(&mut ws, is("welcome")).await;
    assert!(
        w["features"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f == "indicators.limit.v1"),
        "{w}"
    );
    wait_for_replay(&mut ws).await;
    // Paused, so both frames see the same bars.
    send(
        &mut ws,
        json!({"type": "replay.control", "action": "pause"}),
    )
    .await;
    send(&mut ws, json!({"type": "ping", "ts": 1})).await;
    next(&mut ws, |m| m["type"] == "pong" && m["ts"] == 1).await;
    send(
        &mut ws,
        json!({"type": "chart.subscribe", "id": "a", "symbol": "SMPL", "timeframe": "15s",
               "profileBin": 0.01, "indicators": studies_list(), "rth": false, "limit": 120}),
    )
    .await;
    next(&mut ws, is("chart.snapshot")).await;
    let lim = next(&mut ws, is("chart.indicators")).await;
    send(
        &mut ws,
        json!({"type": "chart.indicators.set", "id": "a", "indicators": studies_list(), "rth": false}),
    )
    .await;
    let full = next(&mut ws, is("chart.indicators")).await;
    let t = |v: &Value| -> Vec<i64> {
        v["t"]
            .as_array()
            .unwrap()
            .iter()
            .map(|x| x.as_i64().unwrap())
            .collect()
    };
    let (lt, ft) = (t(&lim), t(&full));
    assert!(ft.len() > 120, "the day so far: {} bars", ft.len());
    assert_eq!(lt.len(), 120);
    assert_eq!(lt[..], ft[ft.len() - 120..]);
    assert_eq!(lim["truncated"], true);
    assert!(full.get("truncated").is_none(), "{}", full["truncated"]);
    let (lm, fm) = (
        IndicatorModel::from_snapshot(&lim),
        IndicatorModel::from_snapshot(&full),
    );
    for (k, col) in &lm.0 {
        let whole = &fm.0[k];
        for (bar, v) in col {
            assert_eq!(whole.get(bar), Some(v), "{k:?} at {bar}");
        }
        // Nothing of the full frame's tail is missing from the limited one.
        let tail = whole.range(lt[0]..).count();
        assert_eq!(col.len(), tail, "{k:?}");
    }
}

/// A seek rebuilds the engine: a slot with studies gets a fresh `chart.indicators` over the
/// rebuilt chart with its resync snapshot.
#[tokio::test]
async fn a_seek_resends_the_indicators() {
    let s = stack(&[]).await;
    let mut ws = connect(&s.url).await;
    wait_for_replay(&mut ws).await;
    send(
        &mut ws,
        json!({"type": "chart.subscribe", "symbol": "SMPL", "timeframe": "1m",
               "indicators": studies_list()}),
    )
    .await;
    let before = next(&mut ws, is("chart.indicators")).await;
    let n_before = before["t"].as_array().unwrap().len();
    // Back to 10:00: about 40 fewer one-minute bars.
    send(
        &mut ws,
        json!({"type": "replay.control", "action": "seek", "value": T1040 - 40 * 60_000}),
    )
    .await;
    let after = loop {
        let m = next(&mut ws, is("chart.indicators")).await;
        if m["t"].as_array().unwrap().len() < n_before {
            break m;
        }
    };
    let last = after["t"]
        .as_array()
        .unwrap()
        .last()
        .unwrap()
        .as_i64()
        .unwrap();
    assert!(
        last < T1040 - 30 * 60_000,
        "the rebuilt chart ends near the seek target: {last}"
    );
}

/// Two slots of one socket on one chart key with different studies (here on two profile bins,
/// so two candle charts): each `chart.indicators` carries the studies of BOTH slots, and both
/// slots are current to it. Frames carry no seq; with one snapshot per slot, a frame queued
/// between the two snapshots went out for the older slot and overwrote the newer slot's keys
/// with older values. Folding everything on one client model must still end equal to a fresh
/// snapshot.
#[tokio::test]
async fn slots_on_one_chart_key_share_one_indicator_snapshot() {
    let s = stack(&[("REPLAY_SPEED", "300")]).await;
    let mut ws = connect(&s.url).await;
    wait_for_replay(&mut ws).await;
    let keys = |m: &Value| -> Vec<String> {
        m["studies"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s["key"].as_str().unwrap().to_string())
            .collect()
    };
    send(
        &mut ws,
        json!({"type": "chart.subscribe", "id": "a", "symbol": "SMPL", "timeframe": "15s",
               "profileBin": 0.01, "indicators": [{"id": "e", "kind": "ema", "period": 9}]}),
    )
    .await;
    let first = next(&mut ws, is("chart.indicators")).await;
    assert_eq!(keys(&first), ["ema:9"]);
    let mut model = IndicatorModel::from_snapshot(&first);
    send(
        &mut ws,
        json!({"type": "chart.subscribe", "id": "b", "symbol": "SMPL", "timeframe": "15s",
               "profileBin": 0.05, "indicators": [{"id": "r", "kind": "rsi"}]}),
    )
    .await;
    // Increments of slot a's key may come first; then the union snapshot.
    let union = loop {
        let m = next(&mut ws, is_indicator).await;
        if m["type"] == "chart.indicators" {
            break m;
        }
        model.apply(&m);
    };
    assert_eq!(keys(&union), ["ema:9", "rsi:14"]);
    model.apply(&union);
    // Slot a changes its studies: the answer still carries slot b's.
    send(
        &mut ws,
        json!({"type": "chart.indicators.set", "id": "a",
               "indicators": [{"id": "v", "kind": "vwap"}]}),
    )
    .await;
    let set = loop {
        let m = next(&mut ws, is_indicator).await;
        if m["type"] == "chart.indicators" {
            break m;
        }
        model.apply(&m);
    };
    assert_eq!(keys(&set), ["vwap", "rsi:14"]);
    model.apply(&set);
    for _ in 0..200 {
        let m = next(&mut ws, is_indicator).await;
        model.apply(&m);
    }
    send(
        &mut ws,
        json!({"type": "replay.control", "action": "pause"}),
    )
    .await;
    while let Ok(m) =
        tokio::time::timeout(Duration::from_millis(1500), next(&mut ws, is_indicator)).await
    {
        model.apply(&m);
    }
    let mut other = connect(&s.url).await;
    send(
        &mut other,
        json!({"type": "chart.subscribe", "symbol": "SMPL", "timeframe": "15s", "profileBin": 0.01,
               "indicators": [{"id": "v", "kind": "vwap"}, {"id": "r", "kind": "rsi"}]}),
    )
    .await;
    let fresh = IndicatorModel::from_snapshot(&next(&mut other, is("chart.indicators")).await);
    for (k, col) in &fresh.0 {
        assert_eq!(model.0.get(k), Some(col), "{k:?}: folded != snapshot");
    }
}

// --- Server-computed levels and zones (`levels.v1`, `zones.v1`, crate::chart_context) -----------

fn is_context(m: &Value) -> bool {
    m["type"] == "chart.levels" || m["type"] == "chart.zones"
}

/// The welcome advertises both features; a slot asking for them gets `chart.levels` and
/// `chart.zones` after its snapshot, then whole-state frames when they change. On 15 s candles,
/// where the fixture day forms and breaks zones in its first hour. Folded (the latest
/// of each wins), they equal what a fresh snapshot of the same chart says once the replay is
/// paused and the forming bar's throttled zones have been flushed.
#[tokio::test]
async fn levels_and_zones_follow_the_replay_and_match_a_fresh_snapshot() {
    let s = stack(&[("REPLAY_SPEED", "300")]).await;
    let (mut ws, _) = tokio_tungstenite::connect_async(&s.url).await.unwrap();
    send(&mut ws, json!({"type": "hello", "client": "test"})).await;
    let w = next(&mut ws, is("welcome")).await;
    let features: Vec<&str> = w["features"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert!(
        features.contains(&"levels.v1") && features.contains(&"zones.v1"),
        "{w}"
    );
    wait_for_replay(&mut ws).await;
    send(
        &mut ws,
        json!({"type": "chart.subscribe", "id": "a", "symbol": "SMPL", "timeframe": "15s",
               "profileBin": 0.01, "levels": true, "zones": true, "rth": false}),
    )
    .await;
    next(&mut ws, is("chart.snapshot")).await;
    let mut levels = next(&mut ws, is("chart.levels")).await;
    let mut zones = next(&mut ws, is("chart.zones")).await;
    assert_eq!(
        (levels["symbol"].as_str(), levels["timeframe"].as_str()),
        (Some("SMPL"), Some("15s"))
    );
    assert_eq!(levels["day"], "2026-10-15");
    for k in ["pmHigh", "pmLow", "hod", "lod", "open"] {
        assert!(levels[k].is_f64(), "{k}: {levels}");
    }
    assert!(levels["hod"].as_f64() >= levels["lod"].as_f64());
    let r = &levels["openingRanges"][0];
    // 2026-10-15 09:30 ET, final long ago, ending at 16:00.
    assert_eq!(r["t"], 1_792_071_000_000i64);
    assert_eq!(r["end"], 1_792_094_400_000i64);
    assert_eq!(r["forming"], false);
    assert_eq!(zones["rth"], false);
    assert!(zones["zones"].is_array());

    // After 10:40 the fixture day makes no new extreme; from just after the open it does, and
    // the opening range stops forming. The seek's resync resends both whole states.
    let open = 1_792_071_000_000i64;
    send(
        &mut ws,
        json!({"type": "replay.control", "action": "seek", "value": open + 5_000}),
    )
    .await;
    loop {
        let m = next(&mut ws, is("chart.levels")).await;
        if m["openingRanges"][0]["forming"] == true {
            levels = m;
            break;
        }
    }
    zones = next(&mut ws, is("chart.zones")).await;
    let (mut n_levels, mut n_zones) = (0, 0);
    loop {
        let m = next(&mut ws, |m| is_context(m) || m["type"] == "replay.clock").await;
        match m["type"].as_str() {
            Some("replay.clock") => {
                if m["clock"]["now"].as_i64().unwrap() >= open + 70 * 60_000 {
                    break;
                }
            }
            Some("chart.levels") => {
                assert_ne!(m, levels, "only a change is sent");
                levels = m;
                n_levels += 1;
            }
            _ => {
                assert_ne!(m, zones, "only a change is sent");
                zones = m;
                n_zones += 1;
            }
        }
    }
    assert!(n_levels > 0, "70 minutes after the open moved no level?");
    assert_eq!(levels["openingRanges"][0]["forming"], false);
    send(
        &mut ws,
        json!({"type": "replay.control", "action": "pause"}),
    )
    .await;
    // Paused: take the tail, including the forming bar's flush (at most a second behind).
    while let Ok(m) =
        tokio::time::timeout(Duration::from_millis(2500), next(&mut ws, is_context)).await
    {
        if m["type"] == "chart.levels" {
            levels = m;
        } else {
            zones = m;
            n_zones += 1;
        }
    }
    let mut other = connect(&s.url).await;
    send(
        &mut other,
        json!({"type": "chart.subscribe", "symbol": "SMPL", "timeframe": "15s", "profileBin": 0.01,
               "levels": true, "zones": true}),
    )
    .await;
    assert_eq!(next(&mut other, is("chart.levels")).await, levels);
    assert_eq!(next(&mut other, is("chart.zones")).await, zones);
    assert!(n_zones > 0, "the first hour forms and breaks 15 s zones");
    eprintln!("levels frames: {n_levels}, zones frames: {n_zones}");
}

/// A client from before sends no flag and gets no frame; levels and zones are asked separately;
/// `chart.indicators.set {zones}` turns the zones on (over the regular session with `rth`) and
/// off; a seek resends both with the resync snapshot.
#[tokio::test]
async fn levels_and_zones_are_opt_in_per_slot() {
    let s = stack(&[]).await;
    let mut old = connect(&s.url).await;
    let mut new = connect(&s.url).await;
    wait_for_replay(&mut old).await;
    wait_for_replay(&mut new).await;
    send(&mut old, sub("", "SMPL", "1m")).await;
    next(&mut old, is("chart.snapshot")).await;
    let seen = types_for(&mut old, Duration::from_millis(2500)).await;
    assert!(seen.iter().any(|t| t == "chart.update"), "{seen:?}");
    assert!(
        !seen
            .iter()
            .any(|t| t == "chart.levels" || t == "chart.zones"),
        "{seen:?}"
    );

    send(
        &mut new,
        json!({"type": "chart.subscribe", "id": "x", "symbol": "SMPL", "timeframe": "5m",
               "profileBin": 0.01, "levels": true}),
    )
    .await;
    next(&mut new, is("chart.snapshot")).await;
    let lv = next(&mut new, is("chart.levels")).await;
    assert_eq!(lv["timeframe"], "5m");
    // The 5m opening range is the 09:30-09:35 candle.
    assert_eq!(lv["openingRanges"][0]["t"], 1_792_071_000_000i64);
    // Zones on, regular session only.
    send(
        &mut new,
        json!({"type": "chart.indicators.set", "id": "x", "indicators": [], "rth": true, "zones": true}),
    )
    .await;
    let z = next(&mut new, is("chart.zones")).await;
    assert_eq!(
        (z["timeframe"].as_str(), z["rth"].as_bool()),
        (Some("5m"), Some(true))
    );
    // A seek: both come again with the resync.
    send(
        &mut new,
        json!({"type": "replay.control", "action": "seek", "value": T1040 - 40 * 60_000}),
    )
    .await;
    next(&mut new, is("chart.snapshot")).await;
    let after = next(&mut new, is_context).await;
    assert_eq!(after["type"], "chart.levels");
    assert_eq!(next(&mut new, is_context).await["type"], "chart.zones");
    // Zones off: no more zone frames, levels still come.
    send(
        &mut new,
        json!({"type": "chart.indicators.set", "id": "x", "indicators": [], "rth": true, "zones": false}),
    )
    .await;
    next(&mut new, is("chart.indicators")).await;
    let seen = types_for(&mut new, Duration::from_millis(2500)).await;
    assert!(!seen.iter().any(|t| t == "chart.zones"), "{seen:?}");
}

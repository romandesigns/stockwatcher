//! Server integration: provider → engine → /ws and /api, driven by a fixture replay.

use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use futures_util::{SinkExt, StreamExt};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use stockwatcher_server::{AppState, Market, MarketConfig, router};
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
}

async fn stack(extra: &[(&str, &str)]) -> Stack {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("archive").to_string_lossy().into_owned();
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
    let (market, _sim) = Market::start(&cfg);
    let dir = tmp.path().join("static");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("index.html"), "<!doctype html>").unwrap();
    let app = router(
        AppState::with_market(None, market),
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
    for tf in ["1m", "15s", "30s", "5m"] {
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

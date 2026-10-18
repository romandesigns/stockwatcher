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
    let start = json!({"start": T1005, "end": null, "reason": "LUDP", "code": "H"});
    let end = json!({"start": T1005, "end": T100630, "reason": "LUDP", "code": "H"});
    assert_eq!(halt_frames, vec![start, end.clone()]);
    assert!(
        luld_frames
            .iter()
            .any(|p| *p == json!({"t": T1005, "ref": null, "up": null, "down": null})),
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
    assert!(bands.contains(&json!({"t": T1005, "ref": null, "up": null, "down": null})));
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

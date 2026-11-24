//! Exercises the real auth/router/Market boundary, without a live provider or paid request.
use axum::body::Body;
use axum::http::{Request, StatusCode};
use futures_util::{SinkExt, StreamExt};
use http_body_util::BodyExt;
use market_events::{MarketEvent, Price, Trade, UnixNanos};
use market_providers::FeedItem;
use serde_json::Value;
use serde_json::json;
use stockwatcher_server::Market;
use stockwatcher_server::{AppState, MarketConfig, router};
use tokio_tungstenite::tungstenite::Message;

fn day() -> chrono::NaiveDate {
    "2026-10-16".parse().unwrap()
}
fn start() -> i64 {
    market_events::session::et(day(), 9, 40, 0, 0).unwrap().0
}
fn frame(market: &Market) -> Value {
    serde_json::from_str(&market.detection_snapshot().2).unwrap()
}

/// Prints start three seconds into each slot, after the preceding 2s settle. Thus every event
/// and explicit clock remains in monotone admission order; no fixture-only backdating trick.
fn bar(market: &Market, slot: usize, prices: [f64; 4]) {
    for (j, price) in prices.into_iter().enumerate() {
        let at = start() + (slot as i64 * 15 + 3 + j as i64 * 3) * 1_000_000_000;
        let mut trade = Trade::new(
            "MPXA",
            UnixNanos(at),
            Price::from_f64_rounded(price).unwrap(),
            100,
            (slot * 4 + j + 1) as u64,
        );
        trade.receive_ts = Some(UnixNanos(at + 100_000_000));
        market
            .apply_detection_fixture(FeedItem::Event(MarketEvent::Trade(trade)))
            .unwrap();
    }
    market
        .apply_detection_fixture(FeedItem::Advance(UnixNanos(
            start() + ((slot as i64 + 1) * 15 + 2) * 1_000_000_000,
        )))
        .unwrap();
}

fn micro_fixture(market: &Market) {
    for slot in 0..8 {
        bar(market, slot, [3.01, 3.02, 3.00, 3.01]);
    }
    for (offset, prices) in [
        [3.02, 3.10, 3.01, 3.10],
        [3.11, 3.20, 3.10, 3.20],
        [3.21, 3.30, 3.20, 3.30],
        [3.31, 3.40, 3.30, 3.40],
    ]
    .into_iter()
    .enumerate()
    {
        bar(market, 8 + offset, prices);
    }
    bar(market, 12, [3.37, 3.38, 3.33, 3.35]);
    bar(market, 13, [3.35, 3.36, 3.32, 3.34]);
    bar(market, 14, [3.41, 3.42, 3.39, 3.41]);
    bar(market, 15, [3.42, 3.44, 3.39, 3.43]);
    market.publish_detection();
}
use tower::ServiceExt;

#[tokio::test]
async fn detection_default_off_and_explicit_source_routes_are_bounded_replacements() {
    assert!(!MarketConfig::default().detection_panel);
    let on = MarketConfig::from_lookup(|k| (k == "DETECTION_PANEL").then(|| "1".into())).unwrap();
    assert!(on.detection_panel);
    assert!(!on.stock_context);
    let dir = tempfile::tempdir().unwrap();
    let app = router(AppState::new(None), dir.path(), dir.path());
    for uri in [
        "/api/detection?source=live",
        "/api/detection?source=replay",
        "/api/detection/export?source=replay",
    ] {
        let response = app
            .clone()
            .oneshot(Request::get(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let frame: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(frame["type"], "detection.snapshot");
        assert_eq!(frame["enabled"], false);
        assert_eq!(frame["alertsEnabled"], false);
        assert_eq!(frame["replacement"], true);
        assert_eq!(frame["transitions"], serde_json::json!([]));
    }
}

#[tokio::test]
async fn socket_opt_in_returns_current_disabled_state_on_every_reconnect_without_alerts() {
    let dir = tempfile::tempdir().unwrap();
    let app = router(AppState::new(None), dir.path(), dir.path());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    for _ in 0..2 {
        let (mut socket, _) = tokio_tungstenite::connect_async(format!("ws://{address}/ws"))
            .await
            .unwrap();
        socket
            .send(Message::Text(
                json!({"type":"hello","client":"test","source":"replay"})
                    .to_string()
                    .into(),
            ))
            .await
            .unwrap();
        socket
            .send(Message::Text(
                json!({"type":"detection.subscribe"}).to_string().into(),
            ))
            .await
            .unwrap();
        let frame = tokio::time::timeout(std::time::Duration::from_secs(3), async {
            loop {
                let Message::Text(text) = socket.next().await.unwrap().unwrap() else {
                    continue;
                };
                let frame: Value = serde_json::from_str(&text).unwrap();
                if frame["type"] == "detection.snapshot" {
                    break frame;
                }
            }
        })
        .await
        .unwrap();
        assert_eq!(frame["enabled"], false);
        assert_eq!(frame["alertsEnabled"], false);
        assert_eq!(frame["cursor"], 0);
        assert_eq!(frame["transitions"], json!([]));
        socket
            .send(Message::Text(
                json!({"type":"detection.unsubscribe"}).to_string().into(),
            ))
            .await
            .unwrap();
        socket.close(None).await.unwrap();
    }
    server.abort();
}

#[tokio::test]
async fn actual_market_discovers_nonmember_without_a_chart_and_keeps_live_isolated() {
    let market = Market::detection_fixture(day(), None);
    micro_fixture(&market);
    let detected = frame(&market);
    assert_eq!(detected["enabled"], true);
    assert_eq!(detected["source"], "fixture");
    assert_eq!(detected["alertsEnabled"], false);
    let transitions = detected["transitions"].as_array().unwrap();
    assert!(
        transitions
            .iter()
            .any(|t| t["transition"]["family"] == "micro_pullback"),
        "{}",
        detected
    );
    assert!(transitions.iter().all(|t| t["restored"] == true));
    assert_eq!(detected["context"]["MPXA"]["ross_member"], Value::Null);
    // No daily-closes file in a fixture market: no previous close, so no gain is sent (never a
    // guessed one), and the new keys stay absent rather than null.
    assert!(detected["context"]["MPXA"].get("gain_bp").is_none());
    assert!(
        detected["context"]["MPXA"]
            .get("prev_close_micros")
            .is_none()
    );
    // Every episode record carries the additive `summary` list; a micro-pullback's is empty
    // (its row note is its levels).
    let episodes: Vec<&Value> = ["active", "recent"]
        .iter()
        .flat_map(|k| detected["snapshot"][k].as_array().unwrap())
        .collect();
    assert!(!episodes.is_empty());
    for e in episodes {
        assert!(e["summary"].is_array(), "{e}");
        if e["family"] == "micro_pullback" {
            assert_eq!(e["summary"], json!([]));
        }
    }
    assert_eq!(
        serde_json::from_str::<Value>(&market.ross_snapshot().1).unwrap()["enabled"],
        false
    );
    assert!(
        !detected["snapshot"]["symbols"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let dir = tempfile::tempdir().unwrap();
    let state = AppState::with_market(None, market.clone());
    assert!(
        state
            .market_for(Some(stockwatcher_server::Source::Live))
            .apply_detection_fixture(FeedItem::Advance(UnixNanos(start())))
            .is_err()
    );
    let app = router(state, dir.path(), dir.path());
    for (source, enabled) in [("live", false), ("replay", true)] {
        let response = app
            .clone()
            .oneshot(
                Request::get(format!("/api/detection?source={source}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let value: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["enabled"], enabled);
        if enabled {
            assert_eq!(value["snapshot"], detected["snapshot"]);
        }
    }
}

#[tokio::test]
async fn zero_active_link_gap_is_reported_and_persisted_without_fabricated_detection() {
    let dir = tempfile::tempdir().unwrap();
    let market = Market::detection_fixture(day(), Some(dir.path()));
    market
        .apply_detection_fixture(FeedItem::Advance(UnixNanos(start())))
        .unwrap();
    market
        .apply_detection_fixture(FeedItem::Link(market_providers::Link {
            state: market_providers::LinkState::Down,
            detail: Some("documented fixture disconnect".into()),
        }))
        .unwrap();
    market.publish_detection();
    let value = frame(&market);
    assert!(value["snapshot"]["active"].as_array().unwrap().is_empty());
    assert_eq!(
        value["snapshot"]["data_status"]["global_interruption"]["reason"],
        "link_down"
    );
    let path = std::fs::read_dir(dir.path())
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let read = tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            let read = stockwatcher_server::detection::read_journal(&path).unwrap();
            if read
                .status
                .iter()
                .any(|s| s.data_status["global_interruption"]["reason"] == "link_down")
            {
                break read;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(read.header.as_ref().unwrap().source, "replay");
    assert_eq!(
        read.header.as_ref().unwrap().detector_policy,
        value["snapshot"]["policy"].as_str().unwrap()
    );
    assert!(read.status.iter().all(|s| s.source == "fixture"));
    assert!(
        read.status
            .iter()
            .all(|s| s.detector_policy == read.header.as_ref().unwrap().detector_policy)
    );
    assert_eq!(read.tail, "unverified");
}

fn armed_fixture() -> Market {
    let market = Market::detection_fixture(day(), None);
    for step in stock_context::detect::fixture::script("micro_retracted").unwrap() {
        market.apply_detection_fixture_step(&step).unwrap();
        market.publish_detection();
        if frame(&market)["snapshot"]["active"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["state"] == "armed")
        {
            return market;
        }
    }
    panic!("fixture never armed");
}

#[test]
fn recoverable_error_retains_gap_and_cannot_trigger_pre_gap_armed_setup() {
    let market = armed_fixture();
    let before = frame(&market);
    let at = before["snapshot"]["as_of"].as_i64().unwrap();
    let old_episode = before["snapshot"]["active"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["state"] == "armed")
        .unwrap()["episode_id"]
        .clone();
    market
        .apply_detection_fixture(FeedItem::Error("synthetic recoverable read error".into()))
        .unwrap();
    market.publish_detection();
    let interrupted = frame(&market);
    assert_eq!(
        interrupted["snapshot"]["data_status"]["global_interruption"]["reason"],
        "provider_error"
    );
    let gap_from = interrupted["snapshot"]["data_status"]["global_interruption"]["from"].clone();
    assert!(gap_from.as_i64().unwrap() > 0);
    assert!(
        interrupted["transitions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["transition"]["episode_id"] == old_episode
                && e["transition"]["to"] == "interrupted")
    );
    assert!(
        interrupted["snapshot"]["active"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    // This would cross the old armed level. New receipt closes the documentary gap, but
    // continuity and the old setup are not restored; the core requires post-gap qualification.
    let resumed_at = at + 3_000_000_000;
    let mut trade = Trade::new(
        "MPXC",
        UnixNanos(resumed_at),
        Price::from_f64_rounded(3.50).unwrap(),
        100,
        900_001,
    );
    trade.receive_ts = Some(UnixNanos(resumed_at));
    market
        .apply_detection_fixture(FeedItem::Event(MarketEvent::Trade(trade)))
        .unwrap();
    market.publish_detection();
    let resumed = frame(&market);
    assert!(resumed["snapshot"]["data_status"]["global_interruption"].is_null());
    assert!(
        resumed["snapshot"]["data_status"]["recent_gaps"]
            .as_array()
            .unwrap()
            .iter()
            .any(|g| g["reason"] == "provider_error"
                && g["from"] == gap_from
                && g["to"] == resumed_at)
    );
    assert!(resumed["snapshot"]["active"].as_array().unwrap().is_empty());
    assert!(
        resumed["transitions"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|e| e["sequence"].as_u64().unwrap()
                > interrupted["history"]["last_sequence"].as_u64().unwrap()
                && e["transition"]["episode_id"] == old_episode)
            .all(|e| !["crossing_provisional", "triggered"]
                .iter()
                .any(|s| e["transition"]["to"] == *s))
    );
    assert_eq!(resumed["alertsEnabled"], false);
}

#[test]
fn explicit_link_down_requires_up_even_after_error_and_buffered_print() {
    let market = armed_fixture();
    let at = frame(&market)["snapshot"]["as_of"].as_i64().unwrap();
    market
        .apply_detection_fixture(FeedItem::Link(market_providers::Link {
            state: market_providers::LinkState::Down,
            detail: Some("synthetic disconnect".into()),
        }))
        .unwrap();
    market
        .apply_detection_fixture(FeedItem::Error("read failed while disconnected".into()))
        .unwrap();
    let mut trade = Trade::new(
        "MPXC",
        UnixNanos(at + 3_000_000_000),
        Price::from_f64_rounded(3.50).unwrap(),
        100,
        900_002,
    );
    trade.receive_ts = Some(UnixNanos(at + 3_000_000_000));
    market
        .apply_detection_fixture(FeedItem::Event(MarketEvent::Trade(trade)))
        .unwrap();
    market.publish_detection();
    assert_eq!(
        frame(&market)["snapshot"]["data_status"]["global_interruption"]["reason"],
        "link_down"
    );
    market
        .apply_detection_fixture(FeedItem::Link(market_providers::Link {
            state: market_providers::LinkState::Up,
            detail: None,
        }))
        .unwrap();
    market.publish_detection();
    let recovered = frame(&market);
    assert!(recovered["snapshot"]["data_status"]["global_interruption"].is_null());
    assert!(
        recovered["snapshot"]["data_status"]["recent_gaps"]
            .as_array()
            .unwrap()
            .iter()
            .any(|g| g["reason"] == "link_down")
    );
    assert!(
        recovered["snapshot"]["active"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn unavailable_replay_provider_uses_configured_instant_without_epoch_zero_gap() {
    let archive = tempfile::tempdir().unwrap();
    let mut cfg = MarketConfig {
        detection_panel: true,
        ..MarketConfig::default()
    };
    cfg.provider.archive = market_archive::ArchiveConfig {
        root: archive.path().to_path_buf(),
        retention_days: 0,
        min_free_bytes: 0,
    };
    cfg.provider.day = Some(day());
    cfg.provider.start_et = chrono::NaiveTime::from_hms_opt(9, 40, 0);
    let (market, _) = Market::start(&cfg);
    let snapshot = frame(&market);
    assert_eq!(snapshot["enabled"], true);
    assert_eq!(
        snapshot["snapshot"]["data_status"]["global_interruption"]["reason"],
        "provider_unavailable"
    );
    assert_eq!(
        snapshot["snapshot"]["data_status"]["global_interruption"]["from"],
        start()
    );
}

#[tokio::test]
async fn significant_burst_captures_without_per_transition_universe_snapshots() {
    fn feed(market: &Market) {
        let prices: Vec<[f64; 4]> = (0..8)
            .map(|_| [3.01, 3.02, 3.00, 3.01])
            .chain([
                [3.02, 3.10, 3.01, 3.10],
                [3.11, 3.20, 3.10, 3.20],
                [3.21, 3.30, 3.20, 3.30],
                [3.31, 3.40, 3.30, 3.40],
                [3.37, 3.38, 3.33, 3.35],
                [3.35, 3.36, 3.32, 3.34],
                [3.41, 3.42, 3.39, 3.41],
                [3.42, 3.44, 3.39, 3.43],
            ])
            .collect();
        let mut sequence = 0;
        for (slot, prices) in prices.into_iter().enumerate() {
            for (j, price) in prices.into_iter().enumerate() {
                let at = start() + (slot as i64 * 15 + 3 + j as i64 * 3) * 1_000_000_000;
                for symbol in 0..32 {
                    sequence += 1;
                    let mut trade = Trade::new(
                        format!("FX{symbol:03}"),
                        UnixNanos(at),
                        Price::from_f64_rounded(price).unwrap(),
                        100,
                        sequence,
                    );
                    trade.receive_ts = Some(UnixNanos(at + 100_000_000));
                    market
                        .apply_detection_fixture(FeedItem::Event(MarketEvent::Trade(trade)))
                        .unwrap();
                }
            }
            market
                .apply_detection_fixture(FeedItem::Advance(UnixNanos(
                    start() + ((slot as i64 + 1) * 15 + 2) * 1_000_000_000,
                )))
                .unwrap();
        }
    }
    let disabled = Market::detection_fixture(day(), None);
    disabled.set_detection_panel(false);
    let baseline = std::time::Instant::now();
    feed(&disabled);
    let baseline_us = baseline.elapsed().as_micros();
    let enabled = Market::detection_fixture(day(), None);
    let began = std::time::Instant::now();
    feed(&enabled);
    let ingest_us = began.elapsed().as_micros();
    let before = frame(&enabled);
    assert_eq!(
        before["telemetry"]["snapshot_captures"], 1,
        "only startup, despite many significant transitions"
    );
    assert!(
        before["telemetry"]["transitions_captured"]
            .as_u64()
            .unwrap()
            >= 32
    );
    enabled.publish_detection();
    let after = frame(&enabled);
    assert_eq!(
        after["telemetry"]["snapshot_captures"], 2,
        "one explicit display refresh"
    );
    eprintln!(
        "synthetic actual-Market burst: 32 symbols, 2048 prints, 16 barriers, disabled_ingest_us={baseline_us}, enabled_ingest_us={ingest_us}, snapshot_captures=2, capture_batches={}, transitions_captured={}",
        after["telemetry"]["capture_batches"], after["telemetry"]["transitions_captured"]
    );
}

#[tokio::test]
async fn every_core_script_matches_actual_market_semantic_transitions() {
    for scenario in stock_context::detect::fixture::scenario_names() {
        let script = stock_context::detect::fixture::script(scenario).unwrap();
        let mut engine = stock_context::detect::fixture::engine(&format!("fixture:{}", day()));
        let expected = stock_context::detect::fixture::drive(&mut engine, &script);
        let market = Market::run_detection_fixture(scenario, None).unwrap();
        let actual: Vec<Value> = frame(&market)["transitions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["transition"].clone())
            .collect();
        assert_eq!(
            actual,
            serde_json::to_value(expected)
                .unwrap()
                .as_array()
                .unwrap()
                .clone(),
            "{scenario}: real chart verdict/revision/clock path must preserve the same semantic detector"
        );
    }
}

#[tokio::test]
async fn seek_rebuild_has_new_epoch_same_semantics_and_no_stale_notifications() {
    use market_providers::{ClockMode, ClockState, Speed};
    let clock = |now| ClockState {
        mode: ClockMode::Replay,
        now: UnixNanos(now),
        speed: Speed::Times(1),
        playing: false,
        day: Some(day()),
        start: None,
        end: None,
        finished: false,
        events: 0,
    };
    let market = Market::run_detection_fixture("micro_valid", None).unwrap();
    let before = frame(&market);
    let end = before["snapshot"]["as_of"].as_i64().unwrap();
    market
        .apply_detection_fixture(FeedItem::Reset(clock(start())))
        .unwrap();
    assert_eq!(frame(&market)["rebuilding"], true);
    for step in stock_context::detect::fixture::script("micro_valid").unwrap() {
        market.apply_detection_fixture_step(&step).unwrap();
    }
    market
        .apply_detection_fixture(FeedItem::Clock(clock(end)))
        .unwrap();
    let after = frame(&market);
    assert_ne!(before["runEpoch"], after["runEpoch"]);
    assert_eq!(after["rebuilding"], false);
    assert!(!market.detection_epoch_is_current(before["runEpoch"].as_str().unwrap()));
    let semantic = |v: &Value| {
        v["transitions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["transition"].clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(semantic(&before), semantic(&after));
    assert!(
        after["transitions"]
            .as_array()
            .unwrap()
            .iter()
            .all(|e| e["restored"] == true)
    );
    assert_eq!(after["alertsEnabled"], false);
}

#[tokio::test]
async fn late_socket_subscriber_gets_bounded_history_and_seek_resync_without_duplicate_alerts() {
    type Socket = tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >;
    async fn next(socket: &mut Socket, kind: &str) -> Value {
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            loop {
                let Message::Text(text) = socket.next().await.unwrap().unwrap() else {
                    continue;
                };
                let value: Value = serde_json::from_str(&text).unwrap();
                if value["type"] == kind {
                    return value;
                }
            }
        })
        .await
        .unwrap()
    }
    let market = Market::run_detection_fixture("micro_valid", None).unwrap();
    let before = frame(&market);
    let dir = tempfile::tempdir().unwrap();
    let app = router(
        AppState::with_market(None, market.clone()),
        dir.path(),
        dir.path(),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let (mut socket, _) = tokio_tungstenite::connect_async(format!("ws://{address}/ws"))
        .await
        .unwrap();
    socket
        .send(Message::Text(
            json!({"type":"hello","client":"test","source":"replay"})
                .to_string()
                .into(),
        ))
        .await
        .unwrap();
    socket
        .send(Message::Text(
            json!({"type":"detection.subscribe"}).to_string().into(),
        ))
        .await
        .unwrap();
    let initial = next(&mut socket, "detection.snapshot").await;
    assert_eq!(initial["transitions"], before["transitions"]);
    assert_eq!(initial["source"], "fixture");
    assert_eq!(initial["alertsEnabled"], false);
    socket
        .send(Message::Text(
            json!({"type":"detection.subscribe"}).to_string().into(),
        ))
        .await
        .unwrap();
    assert_eq!(
        next(&mut socket, "detection.snapshot").await["transitions"],
        initial["transitions"]
    );
    market
        .apply_detection_fixture(FeedItem::Link(market_providers::Link {
            state: market_providers::LinkState::Down,
            detail: None,
        }))
        .unwrap();
    market.publish_detection();
    let update = next(&mut socket, "detection.update").await;
    assert_eq!(update["replacement"], true);
    assert!(update["cursor"].as_u64().unwrap() > initial["cursor"].as_u64().unwrap());
    let ids: Vec<&str> = update["transitions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["transition"]["transition_id"].as_str().unwrap())
        .collect();
    assert_eq!(
        ids.len(),
        ids.iter()
            .copied()
            .collect::<std::collections::BTreeSet<_>>()
            .len()
    );
    // Queue another old-epoch update, then rebuild before yielding to the socket task. Its
    // receiver must discard that queued update and recover the new epoch via Resync.
    market
        .apply_detection_fixture(FeedItem::Link(market_providers::Link {
            state: market_providers::LinkState::Up,
            detail: None,
        }))
        .unwrap();
    market.publish_detection();
    let end = before["snapshot"]["as_of"].as_i64().unwrap();
    let clock = |now| market_providers::ClockState {
        mode: market_providers::ClockMode::Replay,
        now: UnixNanos(now),
        speed: market_providers::Speed::Times(1),
        playing: false,
        day: Some(day()),
        start: None,
        end: None,
        finished: false,
        events: 0,
    };
    market
        .apply_detection_fixture(FeedItem::Reset(clock(start())))
        .unwrap();
    for step in stock_context::detect::fixture::script("micro_valid").unwrap() {
        market.apply_detection_fixture_step(&step).unwrap();
    }
    market
        .apply_detection_fixture(FeedItem::Clock(clock(end)))
        .unwrap();
    let reset = next(&mut socket, "detection.snapshot").await;
    assert_ne!(reset["runEpoch"], initial["runEpoch"]);
    assert_eq!(
        reset["snapshot"]["data_status"]["global_interruption"],
        Value::Null
    );
    assert_eq!(reset["alertsEnabled"], false);
    socket.close(None).await.unwrap();
    server.abort();
}

/// 2026-11-24: every `detection.update` is a full replacement (~4.5 MB on a live morning) and
/// was pushed to each dashboard at ~4 Hz (~18 MB/s), queueing every other frame behind it
/// (a switched chart sat on "Waiting for data…"). Updates are now coalesced per connection
/// (`DETECTION_PUSH_MS`, default 3 s): the first after the snapshot goes out at once, a burst
/// after it collapses into ONE frame, the newest, once the interval has passed.
#[tokio::test]
async fn a_burst_of_detection_updates_reaches_a_socket_as_one_frame_the_newest() {
    use std::time::{Duration, Instant};
    type Socket = tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >;
    async fn next_of(socket: &mut Socket, kind: &str, within: Duration) -> Option<Value> {
        tokio::time::timeout(within, async {
            loop {
                let Message::Text(text) = socket.next().await.unwrap().unwrap() else {
                    continue;
                };
                let value: Value = serde_json::from_str(&text).unwrap();
                if value["type"] == kind {
                    return value;
                }
            }
        })
        .await
        .ok()
    }
    let market = Market::run_detection_fixture("micro_valid", None).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let app = router(AppState::with_market(None, market.clone()), dir.path(), dir.path());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let (mut socket, _) = tokio_tungstenite::connect_async(format!("ws://{address}/ws")).await.unwrap();
    for m in [json!({"type":"hello","client":"test","source":"replay"}), json!({"type":"detection.subscribe"})] {
        socket.send(Message::Text(m.to_string().into())).await.unwrap();
    }
    let snapshot = next_of(&mut socket, "detection.snapshot", Duration::from_secs(3)).await.unwrap();
    let mut flip = true;
    let mut bump = || {
        let state = if flip { market_providers::LinkState::Down } else { market_providers::LinkState::Up };
        flip = !flip;
        market.apply_detection_fixture(FeedItem::Link(market_providers::Link { state, detail: None })).unwrap();
        market.publish_detection();
        frame(&market)["cursor"].as_u64().unwrap()
    };

    // The first update after the snapshot: at once.
    let c1 = bump();
    let first = next_of(&mut socket, "detection.update", Duration::from_secs(2)).await.expect("first update at once");
    assert_eq!(first["cursor"].as_u64().unwrap(), c1);
    assert!(c1 > snapshot["cursor"].as_u64().unwrap());

    // A burst: held, then only the newest arrives, no sooner than the interval.
    let sent_at = Instant::now();
    let _c2 = bump();
    let _c3 = bump();
    let c4 = bump();
    let held = next_of(&mut socket, "detection.update", Duration::from_secs(6)).await.expect("the held update arrives");
    assert_eq!(held["cursor"].as_u64().unwrap(), c4, "the newest of the burst, not an older one");
    assert!(sent_at.elapsed() >= Duration::from_millis(2500), "held for the interval: {:?}", sent_at.elapsed());
    // And nothing else of that burst follows.
    assert!(next_of(&mut socket, "detection.update", Duration::from_millis(800)).await.is_none());
    socket.close(None).await.unwrap();
    server.abort();
}

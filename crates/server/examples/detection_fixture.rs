//! Local-only actual Market demonstration. It cannot start a paid/live provider. The market
//! occupies only the replay slot and every frame is labeled fixture/SyntheticFixture.
use std::path::PathBuf;
use stockwatcher_server::{AppState, Market, router};

fn fixture(scenario: &str, state: Option<&str>, event: Option<&str>) -> Result<Market, String> {
    if state.is_some() && event.is_some() {
        return Err("choose --until-state or --until-event".into());
    }
    let market = if state.is_none() && event.is_none() {
        Market::run_detection_fixture(scenario, None)?
    } else {
        let steps = stock_context::detect::fixture::script(scenario)
            .ok_or_else(|| format!("unknown fixture {scenario}"))?;
        let market = Market::detection_fixture(stock_context::detect::fixture::day(), None);
        let mut matched = false;
        for step in &steps {
            market.apply_detection_fixture_step(step)?;
            market.publish_detection();
            let frame: serde_json::Value =
                serde_json::from_str(&market.detection_snapshot().2).map_err(|e| e.to_string())?;
            // Selection only reads authoritative Rust state/history. It does not fabricate an
            // armed setup or rewind a terminal record; later fixture inputs are never applied.
            matched = state.is_some_and(|wanted| {
                frame["snapshot"]["active"]
                    .as_array()
                    .is_some_and(|episodes| episodes.iter().any(|e| e["state"] == wanted))
            }) || event.is_some_and(|wanted| {
                frame["transitions"].as_array().is_some_and(|transitions| {
                    transitions
                        .iter()
                        .any(|e| e["transition"]["event"] == wanted)
                })
            });
            if matched {
                break;
            }
        }
        if !matched {
            return Err("fixture never reached the requested authoritative state/event".into());
        }
        market
    };
    let frame: serde_json::Value =
        serde_json::from_str(&market.detection_snapshot().2).map_err(|e| e.to_string())?;
    let now = frame["snapshot"]["as_of"]
        .as_i64()
        .ok_or("fixture has no logical clock")?;
    market
        .apply_detection_fixture(market_providers::FeedItem::Clock(
            market_providers::ClockState {
                mode: market_providers::ClockMode::Replay,
                now: market_events::UnixNanos(now),
                speed: market_providers::Speed::Times(1),
                playing: false,
                day: Some(stock_context::detect::fixture::day()),
                start: None,
                end: None,
                finished: false,
                events: 0,
            },
        ))
        .map_err(str::to_string)?;
    market.publish_detection();
    Ok(market)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let scenario = args
        .first()
        .filter(|s| !s.starts_with("--"))
        .map(String::as_str)
        .unwrap_or("micro_valid");
    let option = |name| {
        args.iter()
            .position(|s| s == name)
            .and_then(|i| args.get(i + 1))
            .map(String::as_str)
    };
    let market = fixture(scenario, option("--until-state"), option("--until-event"))?;
    if !args.iter().any(|s| s == "--serve") {
        println!("{}", market.detection_snapshot().2);
        return Ok(());
    }
    let static_dir = args
        .iter()
        .position(|s| s == "--static")
        .and_then(|i| args.get(i + 1))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("apps/web/dist"));
    let data = tempfile::tempdir()?;
    let app = router(
        AppState::with_market(None, market),
        &static_dir,
        data.path(),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:8792").await?;
    eprintln!(
        "Paused synthetic detection fixture {scenario}: http://127.0.0.1:8792/api/detection?source=replay (live slot idle; no alerts)"
    );
    axum::serve(listener, app).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn selected_fixture_stops_on_real_armed_state_before_script_expiry() {
        let market = fixture("micro_retracted", Some("armed"), None).unwrap();
        let frame: serde_json::Value =
            serde_json::from_str(&market.detection_snapshot().2).unwrap();
        assert!(
            frame["snapshot"]["active"]
                .as_array()
                .unwrap()
                .iter()
                .any(|e| e["family"] == "micro_pullback" && e["state"] == "armed")
        );
        assert_eq!(frame["source"], "fixture");
        assert!(!market.clock().unwrap().playing);
        assert_eq!(frame["alertsEnabled"], false);
    }
}

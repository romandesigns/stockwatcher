//! Local-only actual Market demonstration. It cannot start a paid/live provider. The market
//! occupies only the replay slot and every frame is labeled fixture/SyntheticFixture.
use std::path::PathBuf;
use stockwatcher_server::{AppState, Market, router};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let scenario = args
        .first()
        .filter(|s| !s.starts_with("--"))
        .map(String::as_str)
        .unwrap_or("micro_valid");
    let market = Market::run_detection_fixture(scenario, None)?;
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
        "Synthetic detection fixture {scenario}: http://127.0.0.1:8792/api/detection?source=replay (live slot idle; no alerts)"
    );
    axum::serve(listener, app).await?;
    Ok(())
}

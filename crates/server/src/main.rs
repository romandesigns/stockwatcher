use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::process::ExitCode;
use std::time::Duration;

use stockwatcher_server::{AppState, COMMIT, Config, Markets, Settings, VERSION, router};
use tracing_subscriber::EnvFilter;

/// mimalloc instead of glibc's malloc, for the server binary only (tests keep the system one).
///
/// Why (2026-10-22): the server's RSS climbed from ~7 GB after a restart to ~24 GB by the close
/// and never came back down, also on days its live heap could not account for that. Its heap is
/// many small, mixed-lifetime allocations made and freed across a dozen threads: a whole-market
/// open fills the trade-retention windows (chart engine, scanner ledger, held events of symbols
/// still loading) several times over what they hold an hour later, interleaved with candle
/// buckets that live all day. glibc keeps per-thread arenas and only returns memory at an
/// arena's top, so the open's high-water mark stayed resident for the rest of the day — the
/// "+7 GB in the first half hour, then never down" shape production showed. mimalloc frees
/// whole pages back to the OS as they empty and does not strand freed memory in another
/// thread's arena. Measured live heap and production RSS are compared in the commit that added
/// this; `crates/server/tests/memory_bench.rs` reproduces the heap side.
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

fn main() -> ExitCode {
    // The runtime image is distroless (no shell, no curl), so the Docker
    // HEALTHCHECK runs this binary again with --healthcheck.
    if std::env::args().any(|a| a == "--healthcheck") {
        return healthcheck();
    }

    init_tracing();
    let config = match Config::from_env() {
        Ok(c) => c,
        Err(e) => {
            tracing::error!("invalid configuration: {e}");
            return ExitCode::FAILURE;
        }
    };

    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
    match runtime.block_on(serve(config)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            tracing::error!("server error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn init_tracing() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let builder = tracing_subscriber::fmt().with_env_filter(filter);
    if std::env::var("LOG_FORMAT").is_ok_and(|f| f == "json") {
        builder.json().flatten_event(true).init();
    } else {
        builder.init();
    }
}

async fn serve(config: Config) -> std::io::Result<()> {
    std::fs::create_dir_all(&config.data_dir)?;
    if config.api_token.is_none() {
        tracing::warn!("STOCKWATCHER_API_TOKEN is not set: auth is OFF (local dev only)");
    }
    if !config.static_dir.join("index.html").is_file() {
        tracing::warn!(static_dir = %config.static_dir.display(), "no index.html; web app will 404 (run the web build)");
    }

    // Synced settings live on the persistent volume (DATA_DIR = /app/data, the stockwatcher_data
    // volume, in the image), beside the archive but not in it. An unusable directory is fatal,
    // like an unusable DATA_DIR: running on would accept writes it cannot keep.
    let settings = Settings::open(config.data_dir.join("settings"))?;
    // Live and replay side by side (the dashboard reads live, the chart page practises on replay).
    let (markets, sim) = Markets::start(&config.market);
    spawn_memory_stats(&markets);
    // Price alerts watch the live market (never replay), with their rules in the same store and
    // Activity + the registered phones under DATA_DIR/alerts.
    let alerts = stockwatcher_server::alerts::Alerts::start(
        settings.clone(),
        markets.live.clone(),
        Some(config.data_dir.join("alerts")),
        stockwatcher_server::alerts::push::PushConfig::from_lookup(|k| std::env::var(k).ok()),
    );
    let state = AppState::with_alerts(config.api_token.clone(), markets, settings, alerts);
    if let Some(sim) = sim {
        // The provider-compatible simulator on its own port (loopback by default), so Alpaca/FMP
        // base URLs can point at it by configuration. Dummy credentials only.
        let addr = SocketAddr::new(sim.config().host, sim.config().port);
        match tokio::net::TcpListener::bind(addr).await {
            Ok(listener) => {
                tracing::info!(%addr, "provider simulator listening (Alpaca/FMP subset, dummy auth)");
                let mut stop = state.shutdown_signal();
                tokio::spawn(async move {
                    let res = axum::serve(listener, sim.router())
                        .with_graceful_shutdown(async move {
                            let _ = stop.changed().await;
                        })
                        .await;
                    if let Err(e) = res {
                        tracing::error!(error = %e, "provider simulator stopped");
                    }
                });
            }
            Err(e) => {
                tracing::error!(%addr, error = %e, "provider simulator could not bind; continuing without it")
            }
        }
    }
    let app = router(state.clone(), &config.static_dir, &config.data_dir);
    let addr = SocketAddr::new(config.host, config.port);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!(%addr, version = VERSION, commit = COMMIT, data_dir = %config.data_dir.display(), "listening");

    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            shutdown_requested().await;
            tracing::info!("shutdown requested; draining connections");
            state.close_sessions(Duration::from_secs(5)).await;
            // Inside compose's 15 s stop grace: sessions (≤5 s), then the recording (≤5 s), so
            // a deploy loses none of the day's recorded events.
            state.close_recording(Duration::from_secs(5)).await;
        })
        .await?;
    tracing::info!("stopped");
    Ok(())
}

/// `MEM_STATS_SECS=<n>` (unset or 0 = off): every `n` seconds, one `memory` log line with
/// mimalloc's view of the process (RSS, committed memory and their peaks) and what the live
/// market's per-symbol structures hold (`Market::memory_report`).
///
/// Why (2026-10-23): production RSS reached 19 GB with no way to tell the heap from what the
/// kernel had attached to it. The host's transparent huge pages had collapsed ~11 GB of the
/// process into 2 MiB pages, which is invisible from inside without this. The process cannot
/// read its own `smaps_rollup` in the distroless image either, so `AnonHugePages` is read from
/// `/proc/self/smaps_rollup` when it can be and left out when it cannot.
fn spawn_memory_stats(markets: &Markets) {
    let secs: u64 = std::env::var("MEM_STATS_SECS")
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(0);
    if secs == 0 {
        return;
    }
    let live = markets.live.clone();
    let spawned = std::thread::Builder::new()
        .name("memory-stats".into())
        .spawn(move || {
            loop {
                let (mut rss, mut peak_rss, mut commit, mut peak_commit) = (0, 0, 0, 0);
                // SAFETY: every out-pointer is a valid, writable usize or null.
                unsafe {
                    libmimalloc_sys::mi_process_info(
                        std::ptr::null_mut(),
                        std::ptr::null_mut(),
                        std::ptr::null_mut(),
                        &mut rss,
                        &mut peak_rss,
                        &mut commit,
                        &mut peak_commit,
                        std::ptr::null_mut(),
                    );
                }
                let huge_kb = std::fs::read_to_string("/proc/self/smaps_rollup")
                    .ok()
                    .and_then(|s| {
                        s.lines()
                            .find(|l| l.starts_with("AnonHugePages:"))
                            .and_then(|l| l.split_whitespace().nth(1)?.parse::<u64>().ok())
                    });
                tracing::info!(
                    rss_mb = rss / 1_000_000,
                    peak_rss_mb = peak_rss / 1_000_000,
                    commit_mb = commit / 1_000_000,
                    peak_commit_mb = peak_commit / 1_000_000,
                    anon_huge_mb = huge_kb.map(|k| k / 1000),
                    live = %live.memory_report(),
                    "memory"
                );
                std::thread::sleep(Duration::from_secs(secs));
            }
        });
    if let Err(e) = spawned {
        tracing::warn!(error = %e, "MEM_STATS_SECS: could not start the stats thread");
    }
}

async fn shutdown_requested() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let term = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut s) => {
                s.recv().await;
            }
            Err(_) => std::future::pending().await,
        }
    };
    #[cfg(not(unix))]
    let term = std::future::pending::<()>();
    tokio::select! {
        _ = ctrl_c => {}
        _ = term => {}
    }
}

fn healthcheck() -> ExitCode {
    let port: u16 = std::env::var("PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(8790);
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let check = || -> std::io::Result<bool> {
        let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(3))?;
        stream.set_read_timeout(Some(Duration::from_secs(3)))?;
        stream.write_all(
            b"GET /api/health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
        )?;
        let mut body = String::new();
        stream.read_to_string(&mut body)?;
        Ok(body.starts_with("HTTP/1.1 200") && body.contains(r#""status":"ok""#))
    };
    match check() {
        Ok(true) => ExitCode::SUCCESS,
        _ => ExitCode::FAILURE,
    }
}

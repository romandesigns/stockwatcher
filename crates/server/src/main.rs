use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::process::ExitCode;
use std::time::Duration;

use stockwatcher_server::{AppState, COMMIT, Config, VERSION, router};
use tracing_subscriber::EnvFilter;

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

    let state = AppState::new(config.api_token.clone());
    let app = router(state.clone(), &config.static_dir, &config.data_dir);
    let addr = SocketAddr::new(config.host, config.port);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!(%addr, version = VERSION, commit = COMMIT, data_dir = %config.data_dir.display(), "listening");

    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            shutdown_requested().await;
            tracing::info!("shutdown requested; draining connections");
            state.close_sessions(Duration::from_secs(5)).await;
        })
        .await?;
    tracing::info!("stopped");
    Ok(())
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

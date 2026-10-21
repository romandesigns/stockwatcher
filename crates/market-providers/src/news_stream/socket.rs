//! The real WebSocket behind [`super::runner::Dial`].
//!
//! The runner is synchronous, because it is pumped from the news worker's own thread and must not
//! put provider I/O behind a lock shared with anything else. `tokio-tungstenite` is async, so this
//! owns a current-thread runtime and blocks on it — one socket, one thread, no runtime shared with
//! the rest of the process.
//!
//! TLS is built the same way the market-data stream builds it (rustls with the ring provider and
//! the OS trust store through the platform verifier), so no process-wide default provider has to
//! be installed first.
//!
//! Nothing here is exercised against a provider: the stream is off by default, and the agreed
//! boundaries permit no authenticated probe. What *is* tested is everything above it — the
//! protocol, the session and the runner, against an injected fake transport — and the URL this
//! builds.

use super::runner::{Dial, Recv, Transport};
use futures_util::{SinkExt, StreamExt};
use std::sync::Arc;
use std::time::Duration;
use tokio::runtime::Runtime;
use tokio_tungstenite::Connector;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

type Socket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// Opens the news socket over TLS.
pub struct WebSocketDial {
    rt: Arc<Runtime>,
}

impl WebSocketDial {
    /// `Err` when a current-thread runtime cannot be built.
    pub fn new() -> Result<WebSocketDial, String> {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| format!("news stream runtime: {e}"))?;
        Ok(WebSocketDial { rt: Arc::new(rt) })
    }
}

impl Dial for WebSocketDial {
    fn dial(&mut self, url: &str, timeout: Duration) -> Result<Box<dyn Transport>, String> {
        let request = url
            .into_client_request()
            .map_err(|e| format!("news stream url: {e}"))?;
        let connector = tls_connector()?;
        let rt = self.rt.clone();
        let (socket, _) = rt
            .block_on(async {
                tokio::time::timeout(
                    timeout,
                    tokio_tungstenite::connect_async_tls_with_config(
                        request,
                        None,
                        false,
                        Some(connector),
                    ),
                )
                .await
            })
            .map_err(|_| "news stream: connect timed out".to_string())?
            .map_err(|e| format!("news stream: {e}"))?;
        Ok(Box::new(WebSocketTransport { rt, socket }))
    }
}

struct WebSocketTransport {
    rt: Arc<Runtime>,
    socket: Socket,
}

impl Transport for WebSocketTransport {
    fn send(&mut self, text: &str, timeout: Duration) -> Result<(), String> {
        let msg = Message::Text(text.to_string().into());
        // Bounded: a half-open socket accepts a write and never completes it, and this runs on
        // the news worker's own thread.
        self.rt
            .block_on(async { tokio::time::timeout(timeout, self.socket.send(msg)).await })
            .map_err(|_| "send: timed out".to_string())?
            .map_err(|e| format!("send: {e}"))
    }

    fn recv(&mut self, timeout: Duration) -> Recv {
        let got = self
            .rt
            .block_on(async { tokio::time::timeout(timeout, self.socket.next()).await });
        match got {
            // Nothing within the time allowed: the socket is still open.
            Err(_) => Recv::Idle,
            Ok(None) => Recv::Closed("closed by the server".into()),
            Ok(Some(Err(e))) => Recv::Closed(format!("{e}")),
            Ok(Some(Ok(Message::Text(t)))) => Recv::Frame(t.to_string()),
            // Alpaca's news socket is asked for JSON; a binary frame is refused rather than
            // guessed at, exactly as the market-data stream refuses one.
            Ok(Some(Ok(Message::Binary(_)))) => {
                Recv::Closed("a binary frame, where JSON was asked for".into())
            }
            Ok(Some(Ok(Message::Close(_)))) => Recv::Closed("close frame".into()),
            // Ping, pong and raw frames: tungstenite answers pings itself.
            Ok(Some(Ok(_))) => Recv::Idle,
        }
    }
}

/// rustls with the ring provider and the OS trust store via the platform verifier, as the
/// market-data stream does.
fn tls_connector() -> Result<Connector, String> {
    use rustls_platform_verifier::BuilderVerifierExt;
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let cfg = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| format!("tls: {e}"))?
        .with_platform_verifier()
        .map_err(|e| format!("tls verifier: {e}"))?
        .with_no_client_auth();
    Ok(Connector::Rustls(Arc::new(cfg)))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The default URL is Alpaca's documented news stream endpoint
    /// (`docs.alpaca.markets/us/docs/streaming-real-time-news`, read 2026-10-21), and it is a
    /// well-formed request. No connection is attempted.
    #[test]
    fn the_default_url_is_a_well_formed_request() {
        let cfg = super::super::session::NewsStreamConfig::default();
        assert_eq!(cfg.url, "wss://stream.data.alpaca.markets/v1beta1/news");
        let req = cfg
            .url
            .as_str()
            .into_client_request()
            .expect("a client request");
        assert_eq!(req.uri().host(), Some("stream.data.alpaca.markets"));
        assert_eq!(req.uri().path(), "/v1beta1/news");
    }

    /// A URL that is not a WebSocket URL is refused before anything is dialled.
    #[test]
    fn a_bad_url_is_refused_without_dialling() {
        let mut d = WebSocketDial::new().expect("a runtime");
        let err = match d.dial("not a url", Duration::from_secs(1)) {
            Err(e) => e,
            Ok(_) => panic!("a malformed url must be refused"),
        };
        assert!(err.contains("news stream url"), "{err}");
    }
}

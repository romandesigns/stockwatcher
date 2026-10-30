//! The HTTP seam, mirroring market-providers' `import::http` (a tiny trait, a real client, scripted
//! responses in tests), but async and with methods other than GET.
//!
//! - [`Http`] sends one request. The real implementation ([`ReqwestHttp`]) holds the credentials
//!   and adds them as headers itself, so a [`Request`] never carries a secret and a test double
//!   never sees one. It re-checks the paper-host lock on every request and follows no redirects
//!   (a redirect could carry the key headers to another host).
//! - [`Api::call`] adds the retry policy: rate limits (429) are retried for every method,
//!   honouring `Retry-After`, because Alpaca refused the request before acting on it; network
//!   errors and 5xx are retried only for idempotent methods (GET, DELETE). A POST or PATCH whose
//!   outcome is unknown is never resent: the stream and the resync that follows show what
//!   happened.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use crate::creds::Credentials;
use crate::host::{ensure_paper, paper_url};

pub type BoxFut<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    Get,
    Post,
    Patch,
    Delete,
}

impl Method {
    pub fn idempotent(self) -> bool {
        matches!(self, Method::Get | Method::Delete)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Method::Get => "GET",
            Method::Post => "POST",
            Method::Patch => "PATCH",
            Method::Delete => "DELETE",
        }
    }
}

/// One request. Never holds a credential.
#[derive(Clone, Debug, PartialEq)]
pub struct Request {
    pub method: Method,
    pub url: String,
    /// JSON body.
    pub body: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Response {
    pub status: u16,
    pub body: String,
    /// `Retry-After` in seconds, when sent.
    pub retry_after: Option<Duration>,
}

pub trait Http: Send + Sync {
    /// Sends one request. Implementations must not log the URL's headers or any secret.
    fn send<'a>(&'a self, req: &'a Request) -> BoxFut<'a, Result<Response, String>>;
}

/// The real client: reqwest with rustls (ring) and the OS trust store, as the Expo push client.
pub struct ReqwestHttp {
    client: reqwest::Client,
    creds: Credentials,
}

impl ReqwestHttp {
    pub fn new(creds: Credentials) -> Result<Self, String> {
        use rustls_platform_verifier::BuilderVerifierExt;
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let tls = rustls::ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .map_err(|e| format!("tls: {e}"))?
            .with_platform_verifier()
            .map_err(|e| format!("tls verifier: {e}"))?
            .with_no_client_auth();
        let client = reqwest::Client::builder()
            .use_preconfigured_tls(tls)
            .timeout(Duration::from_secs(10))
            .connect_timeout(Duration::from_secs(5))
            // Never follow a redirect: the key headers must only ever reach the paper host.
            .redirect(reqwest::redirect::Policy::none())
            .user_agent(concat!("stockwatcher-server/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| e.to_string())?;
        Ok(ReqwestHttp { client, creds })
    }
}

impl Http for ReqwestHttp {
    fn send<'a>(&'a self, req: &'a Request) -> BoxFut<'a, Result<Response, String>> {
        Box::pin(async move {
            ensure_paper(&req.url).map_err(|e| e.to_string())?;
            let method = match req.method {
                Method::Get => reqwest::Method::GET,
                Method::Post => reqwest::Method::POST,
                Method::Patch => reqwest::Method::PATCH,
                Method::Delete => reqwest::Method::DELETE,
            };
            let mut rb = self
                .client
                .request(method, &req.url)
                .header("APCA-API-KEY-ID", self.creds.key())
                .header("APCA-API-SECRET-KEY", self.creds.secret())
                .header("Accept", "application/json");
            if let Some(b) = &req.body {
                rb = rb
                    .header("Content-Type", "application/json")
                    .body(b.clone());
            }
            // reqwest's error text can include the URL but never headers; scrub anyway.
            let resp = rb
                .send()
                .await
                .map_err(|e| self.creds.scrub(&e.to_string()))?;
            let status = resp.status().as_u16();
            let retry_after = resp
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.trim().parse::<u64>().ok())
                .map(Duration::from_secs);
            let body = resp.text().await.unwrap_or_default();
            Ok(Response {
                status,
                body: self.creds.scrub(&body),
                retry_after,
            })
        })
    }
}

/// What a failed call reports. `message` is Alpaca's own text when it sent one (its rejection
/// reason, e.g. "insufficient buying power"), and never contains a secret.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ApiError {
    /// Alpaca answered with a non-2xx status.
    Status { status: u16, message: String },
    /// No answer (network, timeout). For a POST/PATCH the outcome is unknown.
    Network(String),
}

impl ApiError {
    pub fn message(&self) -> String {
        match self {
            ApiError::Status { status, message } if message.is_empty() => {
                format!("Alpaca answered HTTP {status}")
            }
            ApiError::Status { message, .. } => message.clone(),
            ApiError::Network(m) => format!("could not reach Alpaca paper: {m}"),
        }
    }

    pub fn status(&self) -> Option<u16> {
        match self {
            ApiError::Status { status, .. } => Some(*status),
            ApiError::Network(_) => None,
        }
    }
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ApiError::Status { status, .. } => write!(f, "HTTP {status}: {}", self.message()),
            ApiError::Network(_) => write!(f, "{}", self.message()),
        }
    }
}

/// Alpaca's error body is `{"code": 40310000, "message": "..."}`; anything else is quoted
/// (shortened).
pub fn alpaca_message(body: &str) -> String {
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(body)
        && let Some(m) = v.get("message").and_then(|m| m.as_str())
    {
        return m.to_owned();
    }
    body.chars().take(200).collect()
}

/// Retry bounds. Small on purpose: a user is waiting on an order.
#[derive(Clone, Copy, Debug)]
pub struct RetryPolicy {
    pub max_attempts: u32,
    pub base_delay: Duration,
    /// Also caps a server's `Retry-After`.
    pub max_delay: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        RetryPolicy {
            max_attempts: 3,
            base_delay: Duration::from_millis(400),
            max_delay: Duration::from_secs(5),
        }
    }
}

fn backoff(p: RetryPolicy, attempt: u32) -> Duration {
    p.base_delay
        .saturating_mul(1 << attempt.saturating_sub(1).min(8))
        .min(p.max_delay)
}

/// The REST client: an [`Http`] plus the retry policy, addressed by path on the paper base only.
#[derive(Clone)]
pub struct Api {
    http: Arc<dyn Http>,
    policy: RetryPolicy,
}

impl Api {
    pub fn new(http: Arc<dyn Http>) -> Self {
        Api {
            http,
            policy: RetryPolicy::default(),
        }
    }

    pub fn with_policy(mut self, policy: RetryPolicy) -> Self {
        self.policy = policy;
        self
    }

    /// One call to `PAPER_BASE + path`, with the retry rules in the module docs. Returns the 2xx
    /// response.
    pub async fn call(
        &self,
        method: Method,
        path: &str,
        body: Option<String>,
    ) -> Result<Response, ApiError> {
        let req = Request {
            method,
            url: paper_url(path),
            body,
        };
        if let Err(e) = ensure_paper(&req.url) {
            return Err(ApiError::Network(e.to_string()));
        }
        let attempts = self.policy.max_attempts.max(1);
        let mut last = ApiError::Network("no attempt made".into());
        for attempt in 1..=attempts {
            let wait = match self.http.send(&req).await {
                Ok(r) if (200..300).contains(&r.status) => return Ok(r),
                Ok(r) if r.status == 429 => {
                    last = ApiError::Status {
                        status: 429,
                        message: "Alpaca rate limit reached; try again in a moment".into(),
                    };
                    r.retry_after
                        .unwrap_or_else(|| backoff(self.policy, attempt))
                        .min(self.policy.max_delay)
                }
                Ok(r) if method.idempotent() && matches!(r.status, 408 | 500 | 502 | 503 | 504) => {
                    last = ApiError::Status {
                        status: r.status,
                        message: alpaca_message(&r.body),
                    };
                    backoff(self.policy, attempt)
                }
                Ok(r) => {
                    return Err(ApiError::Status {
                        status: r.status,
                        message: alpaca_message(&r.body),
                    });
                }
                Err(e) if method.idempotent() => {
                    last = ApiError::Network(e);
                    backoff(self.policy, attempt)
                }
                Err(e) => return Err(ApiError::Network(e)),
            };
            if attempt < attempts {
                tracing::debug!(
                    target: "alpaca_paper",
                    method = method.as_str(),
                    path = path.split('?').next().unwrap_or(""),
                    attempt,
                    wait_ms = wait.as_millis() as u64,
                    "retrying"
                );
                tokio::time::sleep(wait).await;
            }
        }
        Err(last)
    }
}

#[cfg(test)]
pub mod mock {
    //! Scripted responses and a record of every request.
    use super::*;
    use std::collections::VecDeque;
    use std::sync::Mutex;

    type Scripted = (Method, String, Result<Response, String>);

    #[derive(Default)]
    pub struct MockHttp {
        /// Matched by (method, path prefix) in order; an unmatched request answers 404.
        pub script: Mutex<VecDeque<Scripted>>,
        pub sent: Mutex<Vec<Request>>,
    }

    pub fn ok(status: u16, body: &str) -> Result<Response, String> {
        Ok(Response {
            status,
            body: body.to_owned(),
            retry_after: None,
        })
    }

    impl MockHttp {
        pub fn push(&self, method: Method, path: &str, r: Result<Response, String>) {
            self.script
                .lock()
                .unwrap()
                .push_back((method, path.to_owned(), r));
        }

        pub fn sent(&self) -> Vec<Request> {
            self.sent.lock().unwrap().clone()
        }
    }

    impl Http for MockHttp {
        fn send<'a>(&'a self, req: &'a Request) -> BoxFut<'a, Result<Response, String>> {
            Box::pin(async move {
                self.sent.lock().unwrap().push(req.clone());
                let path = req.url.strip_prefix(crate::host::PAPER_BASE).unwrap_or("?");
                let mut script = self.script.lock().unwrap();
                let pos = script
                    .iter()
                    .position(|(m, p, _)| *m == req.method && path.starts_with(p.as_str()));
                match pos {
                    Some(i) => script.remove(i).unwrap().2,
                    None => ok(404, r#"{"message":"not scripted"}"#),
                }
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::mock::*;
    use super::*;

    fn api(m: &Arc<MockHttp>) -> Api {
        Api::new(m.clone() as Arc<dyn Http>)
    }

    #[tokio::test(start_paused = true)]
    async fn rate_limits_are_retried_for_any_method_honouring_retry_after() {
        let m = Arc::new(MockHttp::default());
        m.push(
            Method::Post,
            "/v2/orders",
            Ok(Response {
                status: 429,
                body: String::new(),
                retry_after: Some(Duration::from_secs(2)),
            }),
        );
        m.push(Method::Post, "/v2/orders", ok(200, "{}"));
        let t0 = tokio::time::Instant::now();
        let r = api(&m)
            .call(Method::Post, "/v2/orders", Some("{}".into()))
            .await
            .unwrap();
        assert_eq!(r.status, 200);
        assert_eq!(m.sent().len(), 2);
        assert!(t0.elapsed() >= Duration::from_secs(2));
    }

    #[tokio::test(start_paused = true)]
    async fn a_post_is_never_resent_after_a_network_error_or_5xx() {
        let m = Arc::new(MockHttp::default());
        m.push(Method::Post, "/v2/orders", Err("connection reset".into()));
        let e = api(&m)
            .call(Method::Post, "/v2/orders", Some("{}".into()))
            .await
            .unwrap_err();
        assert!(matches!(e, ApiError::Network(_)));
        assert_eq!(m.sent().len(), 1);

        let m = Arc::new(MockHttp::default());
        m.push(Method::Patch, "/v2/orders/x", ok(503, ""));
        let e = api(&m)
            .call(Method::Patch, "/v2/orders/x", Some("{}".into()))
            .await
            .unwrap_err();
        assert_eq!(e.status(), Some(503));
        assert_eq!(m.sent().len(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn gets_retry_transient_failures_and_give_up_after_the_bound() {
        let m = Arc::new(MockHttp::default());
        m.push(Method::Get, "/v2/account", Err("timeout".into()));
        m.push(Method::Get, "/v2/account", ok(502, ""));
        m.push(
            Method::Get,
            "/v2/account",
            ok(200, r#"{"status":"ACTIVE"}"#),
        );
        assert_eq!(
            api(&m)
                .call(Method::Get, "/v2/account", None)
                .await
                .unwrap()
                .status,
            200
        );
        let m = Arc::new(MockHttp::default());
        for _ in 0..5 {
            m.push(Method::Get, "/v2/account", ok(503, ""));
        }
        assert!(
            api(&m)
                .call(Method::Get, "/v2/account", None)
                .await
                .is_err()
        );
        assert_eq!(m.sent().len(), 3);
    }

    #[tokio::test]
    async fn a_rejection_carries_alpacas_own_text() {
        let m = Arc::new(MockHttp::default());
        m.push(
            Method::Post,
            "/v2/orders",
            ok(
                403,
                r#"{"code":40310000,"message":"insufficient buying power"}"#,
            ),
        );
        let e = api(&m)
            .call(Method::Post, "/v2/orders", Some("{}".into()))
            .await
            .unwrap_err();
        assert_eq!(e.message(), "insufficient buying power");
        assert_eq!(e.status(), Some(403));
    }

    #[tokio::test]
    async fn every_request_goes_to_the_paper_host() {
        let m = Arc::new(MockHttp::default());
        let _ = api(&m).call(Method::Get, "/v2/clock", None).await;
        let sent = m.sent();
        assert_eq!(sent.len(), 1);
        assert!(sent[0].url.starts_with("https://paper-api.alpaca.markets/"));
        assert!(ensure_paper(&sent[0].url).is_ok());
    }
}

//! The importers' HTTP seam: a tiny synchronous GET trait (a real client behind the `import`
//! feature, scripted responses in tests), bounded retries and credential redaction.
//!
//! Nothing here ever logs or formats a credential: Alpaca's key travels in headers that are never
//! printed, FMP's `apikey` query parameter is replaced by `REDACTED` in every URL that reaches a
//! log line or an error, and response bodies quoted in errors are scrubbed of every secret.

use std::time::Duration;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HttpResponse {
    pub status: u16,
    pub body: Vec<u8>,
    /// `Retry-After` in seconds, when the server sent one.
    pub retry_after: Option<Duration>,
}

/// One GET. Implementations must not log the URL or the headers themselves.
pub trait Http {
    fn get(&mut self, url: &str, headers: &[(&str, &str)]) -> Result<HttpResponse, String>;
}

/// Retries with exponential backoff for rate limits and transient failures only.
#[derive(Clone, Copy, Debug)]
pub struct RetryPolicy {
    /// Total attempts per request, including the first.
    pub max_attempts: u32,
    pub base_delay: Duration,
    pub max_delay: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        RetryPolicy {
            max_attempts: 5,
            base_delay: Duration::from_millis(500),
            max_delay: Duration::from_secs(30),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FetchError {
    /// Not worth retrying: authentication, entitlement, malformed request, not found.
    Permanent { status: u16, body: String },
    /// Every attempt failed with a transient status or a network error.
    Exhausted { attempts: u32, last: String },
}

impl std::fmt::Display for FetchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FetchError::Permanent { status, body } => write!(f, "HTTP {status}: {body}"),
            FetchError::Exhausted { attempts, last } => {
                write!(f, "gave up after {attempts} attempts: {last}")
            }
        }
    }
}

/// 408, 429 and 5xx gateway/availability errors are transient; everything else is not.
pub fn is_transient(status: u16) -> bool {
    matches!(status, 408 | 429 | 500 | 502 | 503 | 504)
}

/// GET with bounded retries. `sleep` is injected so tests do not wait; `log` receives only
/// redacted text. Returns the first 2xx response.
pub fn get_with_retry(
    http: &mut dyn Http,
    url: &str,
    headers: &[(&str, &str)],
    policy: RetryPolicy,
    secrets: &[&str],
    sleep: &mut dyn FnMut(Duration),
    log: &mut dyn FnMut(&str),
) -> Result<HttpResponse, FetchError> {
    let mut last = String::new();
    let attempts = policy.max_attempts.max(1);
    for attempt in 1..=attempts {
        match http.get(url, headers) {
            Ok(r) if (200..300).contains(&r.status) => return Ok(r),
            Ok(r) if is_transient(r.status) => {
                last = format!("HTTP {}", r.status);
                if attempt < attempts {
                    let wait = r
                        .retry_after
                        .unwrap_or_else(|| backoff(policy, attempt))
                        .min(policy.max_delay);
                    log(&format!(
                        "{} -> HTTP {}; retry {attempt}/{} in {:.1}s",
                        redact_url(url),
                        r.status,
                        attempts - 1,
                        wait.as_secs_f64()
                    ));
                    sleep(wait);
                }
            }
            Ok(r) => {
                return Err(FetchError::Permanent {
                    status: r.status,
                    body: scrub(&snippet(&r.body), secrets),
                });
            }
            Err(e) => {
                last = scrub(&e, secrets);
                if attempt < attempts {
                    let wait = backoff(policy, attempt);
                    log(&format!(
                        "{} -> network error; retry {attempt}/{} in {:.1}s",
                        redact_url(url),
                        attempts - 1,
                        wait.as_secs_f64()
                    ));
                    sleep(wait);
                }
            }
        }
    }
    Err(FetchError::Exhausted { attempts, last })
}

fn backoff(policy: RetryPolicy, attempt: u32) -> Duration {
    let factor = 1u32 << (attempt - 1).min(16);
    policy
        .base_delay
        .saturating_mul(factor)
        .min(policy.max_delay)
}

/// The URL with the value of any `apikey`, `token` or `secret` query parameter replaced.
pub fn redact_url(url: &str) -> String {
    let Some((base, query)) = url.split_once('?') else {
        return url.to_string();
    };
    let parts: Vec<String> = query
        .split('&')
        .map(|kv| match kv.split_once('=') {
            Some((k, _)) if is_secret_param(k) => format!("{k}=REDACTED"),
            _ => kv.to_string(),
        })
        .collect();
    format!("{base}?{}", parts.join("&"))
}

fn is_secret_param(k: &str) -> bool {
    let k = k.to_ascii_lowercase();
    k == "apikey"
        || k == "api_key"
        || k.contains("token") && k != "page_token"
        || k.contains("secret")
}

/// Every occurrence of every secret replaced by `REDACTED`.
pub fn scrub(text: &str, secrets: &[&str]) -> String {
    let mut out = text.to_string();
    for s in secrets.iter().filter(|s| s.len() >= 4) {
        out = out.replace(s, "REDACTED");
    }
    out
}

fn snippet(body: &[u8]) -> String {
    let s = String::from_utf8_lossy(body);
    let s = s.trim();
    if s.chars().count() > 300 {
        format!("{}…", s.chars().take(300).collect::<String>())
    } else {
        s.to_string()
    }
}

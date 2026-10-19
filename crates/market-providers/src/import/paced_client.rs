//! The research downloader's HTTP client (feature `import`): the same blocking reqwest + rustls
//! setup as [`super::client::ReqwestHttp`], plus a [`Pacer`] that every request passes through
//! and that reads the provider's `X-RateLimit-*` headers (see `pacing`). One instance per
//! process, used from one thread: concurrency is 1 by construction.
//!
//! It also counts bytes on the wire (gzip as received) so the run report can compare transfer
//! size with the decoded JSON size counted by `pilot::Metered`.

use super::http::{Http, HttpResponse};
use super::pacing::{Pacer, PacerReport, rate_headers};
use std::time::{Duration, Instant};

pub struct PacedHttp {
    client: reqwest::blocking::Client,
    pacer: Pacer,
    pub wire_bytes: u64,
}

impl PacedHttp {
    pub fn new(cap_per_minute: u32) -> Result<Self, String> {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(120))
            .connect_timeout(Duration::from_secs(15))
            .user_agent(concat!(
                "stockwatcher-swimport-research/",
                env!("CARGO_PKG_VERSION")
            ))
            .build()
            .map_err(|e| e.without_url().to_string())?;
        Ok(PacedHttp {
            client,
            pacer: Pacer::new(cap_per_minute),
            wire_bytes: 0,
        })
    }

    pub fn pacer_report(&self) -> PacerReport {
        self.pacer.report()
    }
}

impl Http for PacedHttp {
    fn get(&mut self, url: &str, headers: &[(&str, &str)]) -> Result<HttpResponse, String> {
        let wait = self.pacer.reserve(Instant::now());
        if !wait.is_zero() {
            std::thread::sleep(wait);
        }
        let mut req = self.client.get(url).header("Accept-Encoding", "gzip");
        for (k, v) in headers {
            req = req.header(*k, *v);
        }
        let resp = req.send().map_err(|e| e.without_url().to_string())?;
        let status = resp.status().as_u16();
        let h = resp.headers();
        let lookup = |k: &str| {
            h.get(k)
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_string())
        };
        let rl = rate_headers(lookup);
        let mut retry_after = lookup("retry-after")
            .and_then(|v| v.trim().parse::<u64>().ok())
            .map(Duration::from_secs);
        let gzipped = lookup("content-encoding").is_some_and(|v| v.eq_ignore_ascii_case("gzip"));
        let now_unix = chrono::Utc::now().timestamp();
        if let Some(w) = self
            .pacer
            .observe(Instant::now(), now_unix, status, &rl, retry_after)
        {
            retry_after.get_or_insert(w);
        }
        let raw = resp.bytes().map_err(|e| e.without_url().to_string())?;
        self.wire_bytes += raw.len() as u64;
        let body = if gzipped {
            let mut out = Vec::with_capacity(raw.len() * 8);
            std::io::Read::read_to_end(&mut flate2::read::GzDecoder::new(&raw[..]), &mut out)
                .map_err(|e| format!("gzip: {e}"))?;
            out
        } else {
            raw.to_vec()
        };
        Ok(HttpResponse {
            status,
            body,
            retry_after,
        })
    }
}

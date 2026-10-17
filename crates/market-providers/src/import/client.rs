//! The real HTTP client for the importers (feature `import`): blocking reqwest over rustls with
//! the `ring` provider and the platform's certificate verifier.

use super::http::{Http, HttpResponse};
use std::time::Duration;

pub struct ReqwestHttp {
    client: reqwest::blocking::Client,
}

impl ReqwestHttp {
    pub fn new() -> Result<Self, String> {
        // Only the ring provider is compiled in; installing it twice is harmless.
        let _ = rustls::crypto::ring::default_provider().install_default();
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(60))
            .connect_timeout(Duration::from_secs(15))
            .user_agent(concat!("stockwatcher-swimport/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| e.without_url().to_string())?;
        Ok(ReqwestHttp { client })
    }
}

impl Http for ReqwestHttp {
    fn get(&mut self, url: &str, headers: &[(&str, &str)]) -> Result<HttpResponse, String> {
        // Ask for gzip and decode it here (flate2): trade pages shrink about 7x on the wire.
        let mut req = self.client.get(url).header("Accept-Encoding", "gzip");
        for (k, v) in headers {
            req = req.header(*k, *v);
        }
        // `without_url`: reqwest puts the URL in its errors, and an FMP URL carries the key.
        let resp = req.send().map_err(|e| e.without_url().to_string())?;
        let status = resp.status().as_u16();
        let retry_after = resp
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.trim().parse::<u64>().ok())
            .map(Duration::from_secs);
        let gzipped = resp
            .headers()
            .get(reqwest::header::CONTENT_ENCODING)
            .is_some_and(|v| v.as_bytes().eq_ignore_ascii_case(b"gzip"));
        let raw = resp.bytes().map_err(|e| e.without_url().to_string())?;
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

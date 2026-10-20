//! FMP public float (`GET /stable/shares-float`) into append-only [`FloatRecord`]s.
//!
//! What this endpoint is, from the account's own probes (research report 02, finding F1): one
//! **current** row per symbol (`floatShares`, `outstandingShares`, `freeFloat`, `source`, `date`),
//! where `date` is FMP's refresh time in UTC, not a date the float was true. The historical-float
//! route is refused (v4 legacy: 403; no stable equivalent: 404). So this is a forward source only:
//! each retrieval becomes a record observed at its retrieval time, nothing is ever dated earlier,
//! and a figure that changes between two daily retrievals says the provider's number changed, not
//! that the issuer's float changed that day.
//!
//! `floatShares` is the only float. `outstandingShares` is stored solely to flag a row whose float
//! equals it (indistinguishable from FMP substituting outstanding shares; such a row is not used
//! as float evidence, `market_archive::evidence_load`).
//!
//! `shares-float-all` (paged, every symbol) answered page 0 on this account; whether later pages
//! were allowed was unknown (FMP refused `page=1` on `delisted-companies`); `page=1` was probed on
//! 2026-10-20 and returned 1,000 rows. [`fetch_float_all`] is the collector's default, stops on
//! the first refusal, and the caller falls back to per-symbol.
//!
//! The key travels as the `apikey` query parameter: URLs reach logs only through `redact_url`,
//! the stored `endpoint` never contains it, and error bodies are scrubbed.

use super::fmp::{FmpError, FmpKey};
use super::http::{FetchError, Http, RetryPolicy, get_with_retry, scrub};
use market_archive::evidence::EVIDENCE_SCHEMA;
use market_archive::evidence_load::float_quality;
use market_archive::{FloatRecord, sha256_hex};
use std::time::Duration;

pub const PROVIDER: &str = "fmp";
/// Rows per `shares-float-all` page when the bulk mode is used.
pub const ALL_PAGE_LIMIT: u32 = 1000;
/// Guard against a provider that never returns an empty page.
const MAX_ALL_PAGES: u32 = 200;

/// FMP spells share classes with a dash (`BRK-B`); Alpaca and the archive with a dot (`BRK.B`).
pub fn fmp_symbol(symbol: &str) -> String {
    symbol.replace('.', "-")
}

pub fn single_endpoint(symbol: &str) -> String {
    format!("/stable/shares-float?symbol={}", fmp_symbol(symbol))
}

pub fn all_endpoint(page: u32) -> String {
    format!("/stable/shares-float-all?page={page}&limit={ALL_PAGE_LIMIT}")
}

fn get_json(
    http: &mut dyn Http,
    base_url: &str,
    key: &FmpKey,
    endpoint: &str,
    retry: RetryPolicy,
    sleep: &mut dyn FnMut(Duration),
    log: &mut dyn FnMut(&str),
) -> Result<(serde_json::Value, Vec<u8>), FmpError> {
    let url = format!(
        "{}{endpoint}&apikey={}",
        base_url.trim_end_matches('/'),
        key.0
    );
    let secrets = [key.0.as_str()];
    let resp = match get_with_retry(
        http,
        &url,
        &[("Accept", "application/json")],
        retry,
        &secrets,
        sleep,
        log,
    ) {
        Ok(r) => r,
        // FMP answers plan limits with 402 ("Premium Query Parameter", "Restricted Endpoint"),
        // legacy routes with 403: report them, never work around them.
        Err(FetchError::Permanent { status, body }) if matches!(status, 401..=403) => {
            return Err(FmpError::NotEntitled {
                status,
                message: body,
            });
        }
        Err(e) => return Err(FmpError::Fetch(e)),
    };
    let value: serde_json::Value =
        serde_json::from_slice(&resp.body).map_err(|e| FmpError::Malformed(e.to_string()))?;
    if let Some(msg) = value.get("Error Message").and_then(|m| m.as_str()) {
        return Err(FmpError::NotEntitled {
            status: resp.status,
            message: scrub(msg, &secrets),
        });
    }
    Ok((value, resp.body))
}

/// A share count as FMP delivers it (integer, or a float for some rows). Negative or non-finite
/// is not a count.
fn shares(v: Option<&serde_json::Value>) -> Option<u64> {
    let v = v?;
    v.as_u64().or_else(|| {
        v.as_f64()
            .filter(|f| f.is_finite() && *f >= 0.0)
            .map(|f| f.round() as u64)
    })
}

fn text(v: Option<&serde_json::Value>) -> Option<String> {
    v.and_then(|x| x.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// One row into a record (`symbol` is ours, never FMP's spelling).
pub fn record_from_row(
    symbol: &str,
    row: Option<&serde_json::Value>,
    endpoint: &str,
    retrieved_at: i64,
    response_sha256: &str,
) -> FloatRecord {
    let mut r = FloatRecord {
        v: EVIDENCE_SCHEMA,
        symbol: symbol.to_string(),
        provider: PROVIDER.into(),
        endpoint: endpoint.to_string(),
        float_shares: row.and_then(|r| shares(r.get("floatShares"))),
        outstanding_shares: row.and_then(|r| shares(r.get("outstandingShares"))),
        free_float_pct: row
            .and_then(|r| r.get("freeFloat"))
            .and_then(|v| v.as_f64()),
        source_url: row.and_then(|r| text(r.get("source"))),
        provider_date: row.and_then(|r| text(r.get("date"))),
        // FMP supplies no as-of date for the figure; its `date` is a refresh time (F1).
        effective_as_of: None,
        retrieved_at,
        response_sha256: response_sha256.to_string(),
        quality: Vec::new(),
    };
    r.quality = float_quality(&r);
    r
}

/// One symbol's current float. An empty array is still a retrieval (a `no_row` record): the
/// provider was asked and had nothing, which is different from not having asked.
#[allow(clippy::too_many_arguments)]
pub fn fetch_float(
    http: &mut dyn Http,
    base_url: &str,
    key: &FmpKey,
    symbol: &str,
    retry: RetryPolicy,
    now: &mut dyn FnMut() -> i64,
    sleep: &mut dyn FnMut(Duration),
    log: &mut dyn FnMut(&str),
) -> Result<FloatRecord, FmpError> {
    let endpoint = single_endpoint(symbol);
    let (value, body) = get_json(http, base_url, key, &endpoint, retry, sleep, log)?;
    let retrieved_at = now();
    let rows = value
        .as_array()
        .ok_or_else(|| FmpError::Malformed("expected a JSON array".into()))?;
    let want = fmp_symbol(symbol);
    let row = rows
        .iter()
        .find(|r| r.get("symbol").and_then(|s| s.as_str()) == Some(want.as_str()));
    Ok(record_from_row(
        symbol,
        row,
        &endpoint,
        retrieved_at,
        &sha256_hex(&body),
    ))
}

/// Every page of `shares-float-all`, keeping rows whose symbol `want` accepts (in our spelling).
/// Stops at the first empty page. A refusal on any page is returned as the error and the records
/// already gathered are discarded by the caller's choice (it falls back to per-symbol requests).
#[allow(clippy::too_many_arguments)]
pub fn fetch_float_all(
    http: &mut dyn Http,
    base_url: &str,
    key: &FmpKey,
    want: &dyn Fn(&str) -> bool,
    retry: RetryPolicy,
    now: &mut dyn FnMut() -> i64,
    sleep: &mut dyn FnMut(Duration),
    log: &mut dyn FnMut(&str),
) -> Result<(Vec<FloatRecord>, u32), FmpError> {
    let mut out = Vec::new();
    for page in 0..MAX_ALL_PAGES {
        let endpoint = all_endpoint(page);
        let (value, body) = get_json(http, base_url, key, &endpoint, retry, sleep, log)?;
        let retrieved_at = now();
        let rows = value
            .as_array()
            .ok_or_else(|| FmpError::Malformed("expected a JSON array".into()))?;
        if rows.is_empty() {
            return Ok((out, page + 1));
        }
        let hash = sha256_hex(&body);
        for row in rows {
            let Some(fmp) = row.get("symbol").and_then(|s| s.as_str()) else {
                continue;
            };
            let ours = fmp.replace('-', ".");
            if want(&ours) {
                out.push(record_from_row(
                    &ours,
                    Some(row),
                    &endpoint,
                    retrieved_at,
                    &hash,
                ));
            }
        }
    }
    Err(FmpError::Malformed(format!(
        "shares-float-all did not end within {MAX_ALL_PAGES} pages"
    )))
}

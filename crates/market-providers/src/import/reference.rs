//! Reference data for research acquisition, kept **raw**: daily bars (`adjustment=raw`),
//! corporate actions (`/v1/corporate-actions`) and asset-list snapshots (`/v2/assets?status=`).
//!
//! These are stored as the provider's own JSON pages (plus the credential-free request), not
//! normalized: they are inputs to research decisions (eligibility, warm-up, survivorship,
//! split handling) whose interpretation is still open, and a raw copy can be re-read under any
//! later rule. Summaries ([`summarize_corporate_actions`]) are derived views, never the store.
//!
//! Time conventions:
//! - Daily bars are requested with RFC 3339 bounds in UTC computed from ET dates, so "the 30
//!   sessions before D" ends one second before midnight ET of D and can never include D's own
//!   bar (which Alpaca stamps at midnight ET), whatever the endpoint's inclusivity.
//! - Corporate-action `start`/`end` are plain dates as the endpoint takes them; which date of an
//!   action they filter on (ex, process, effective) is the provider's definition and is not
//!   assumed here; the raw rows keep every date field.
//! - The asset list is a **current snapshot** (no history); the file name carries its date.

use super::alpaca::{AlpacaCredentials, rfc3339};
use super::http::{Http, RetryPolicy, get_with_retry};
use chrono::{DateTime, Duration as ChronoDuration, NaiveDate, NaiveTime, Utc};
use market_events::is_trading_day;
use market_events::session::et_instant;
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;
use std::time::Duration;

const MAX_PAGES: usize = 10_000;

/// Page tokens are opaque base64-ish strings; encode everything outside the URL-safe set.
pub fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Every page of a paginated JSON endpoint (`next_page_token`), as parsed JSON. A response that
/// is not an object (the assets list is a bare array) is one page. `make_url` gets the token.
pub fn fetch_pages(
    http: &mut dyn Http,
    creds: &AlpacaCredentials,
    make_url: &dyn Fn(Option<&str>) -> String,
    sleep: &mut dyn FnMut(Duration),
    log: &mut dyn FnMut(&str),
) -> Result<Vec<Value>, String> {
    let headers = [
        ("APCA-API-KEY-ID", creds.key_id.as_str()),
        ("APCA-API-SECRET-KEY", creds.secret.as_str()),
        ("Accept", "application/json"),
    ];
    let secrets = [creds.key_id.as_str(), creds.secret.as_str()];
    let mut pages = Vec::new();
    let mut token: Option<String> = None;
    loop {
        if pages.len() >= MAX_PAGES {
            return Err(format!("more than {MAX_PAGES} pages; stopped"));
        }
        let url = make_url(token.as_deref());
        let resp = get_with_retry(
            http,
            &url,
            &headers,
            RetryPolicy::default(),
            &secrets,
            sleep,
            log,
        )
        .map_err(|e| e.to_string())?;
        let v: Value = serde_json::from_slice(&resp.body)
            .map_err(|e| format!("page {}: malformed JSON: {e}", pages.len() + 1))?;
        let next = v
            .get("next_page_token")
            .and_then(Value::as_str)
            .filter(|t| !t.is_empty())
            .map(str::to_string);
        pages.push(v);
        match next {
            None => break,
            Some(n) if token.as_deref() == Some(n.as_str()) => {
                return Err("repeated page token; refusing to loop".into());
            }
            Some(n) => token = Some(n),
        }
    }
    Ok(pages)
}

/// The `n` trading dates strictly before `day`, oldest first.
pub fn sessions_before(day: NaiveDate, n: usize) -> Vec<NaiveDate> {
    let mut out = Vec::with_capacity(n);
    let mut d = day;
    // A generous bound: n sessions never span more than 2n + 20 calendar days.
    for _ in 0..(2 * n + 20) {
        if out.len() == n {
            break;
        }
        d -= ChronoDuration::days(1);
        if is_trading_day(d) {
            out.push(d);
        }
    }
    out.reverse();
    out
}

/// Midnight ET of `day` as UTC.
pub fn et_midnight(day: NaiveDate) -> Option<DateTime<Utc>> {
    et_instant(day, NaiveTime::MIN).map(|t| t.to_utc())
}

/// `[midnight ET of first, one second before midnight ET of the day after last]`.
pub fn et_date_range(first: NaiveDate, last: NaiveDate) -> Option<(DateTime<Utc>, DateTime<Utc>)> {
    let start = et_midnight(first)?;
    let end = et_midnight(last + ChronoDuration::days(1))? - ChronoDuration::seconds(1);
    Some((start, end))
}

/// Raw SIP daily bars of `symbols` over `[start, end]`.
pub fn daily_bars_url(
    data_base: &str,
    symbols: &[String],
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    token: Option<&str>,
) -> String {
    let mut u = format!(
        "{}/v2/stocks/bars?symbols={}&timeframe=1Day&start={}&end={}&feed=sip&adjustment=raw&limit=10000&sort=asc",
        data_base.trim_end_matches('/'),
        symbols
            .iter()
            .map(|s| percent_encode(s))
            .collect::<Vec<_>>()
            .join("%2C"),
        rfc3339(start),
        rfc3339(end),
    );
    if let Some(t) = token {
        u.push_str("&page_token=");
        u.push_str(&percent_encode(t));
    }
    u
}

/// The corporate-action types that change what a symbol or a price means (dividends left out:
/// they do not move a raw intraday series and would dominate the response).
pub const STRUCTURAL_ACTION_TYPES: &str = "forward_split,reverse_split,unit_split,stock_dividend,spin_off,cash_merger,stock_merger,stock_and_cash_merger,redemption,name_change,worthless_removal";

/// `/v1/corporate-actions` over `[start, end]` (dates), optionally for some symbols only.
pub fn corporate_actions_url(
    data_base: &str,
    symbols: Option<&[String]>,
    types: &str,
    start: NaiveDate,
    end: NaiveDate,
    token: Option<&str>,
) -> String {
    let mut u = format!(
        "{}/v1/corporate-actions?start={start}&end={end}&limit=1000&sort=asc&types={}",
        data_base.trim_end_matches('/'),
        percent_encode(types)
    );
    if let Some(s) = symbols {
        u.push_str("&symbols=");
        u.push_str(
            &s.iter()
                .map(|s| percent_encode(s))
                .collect::<Vec<_>>()
                .join("%2C"),
        );
    }
    if let Some(t) = token {
        u.push_str("&page_token=");
        u.push_str(&percent_encode(t));
    }
    u
}

/// `/v2/assets?status=<status>&asset_class=us_equity` (one response, no pagination).
pub fn assets_url(trading_base: &str, status: &str) -> String {
    format!(
        "{}/v2/assets?status={}&asset_class=us_equity",
        trading_base.trim_end_matches('/'),
        percent_encode(status)
    )
}

/// One daily bar, flattened from the bars pages: `(symbol, bar object)`.
pub fn bar_rows(pages: &[Value]) -> Vec<(String, Value)> {
    let mut out = Vec::new();
    for p in pages {
        if let Some(map) = p.get("bars").and_then(Value::as_object) {
            for (sym, bars) in map {
                for b in bars.as_array().into_iter().flatten() {
                    out.push((sym.clone(), b.clone()));
                }
            }
        }
    }
    out
}

/// One corporate action, flattened: its type key (e.g. `forward_splits`) and the raw row.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ActionRow {
    pub kind: String,
    pub row: Value,
}

/// Every action in the pages, with counts per type.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct ActionSummary {
    pub counts: BTreeMap<String, u64>,
    pub rows: Vec<ActionRow>,
}

impl ActionSummary {
    /// Every symbol an action names (`symbol`, `old_symbol`, `new_symbol`, `acquiree_symbol`,
    /// `acquirer_symbol`, `source_symbol`, `target_symbol`).
    pub fn symbols(&self) -> Vec<String> {
        let mut out: Vec<String> = self
            .rows
            .iter()
            .flat_map(|r| {
                [
                    "symbol",
                    "old_symbol",
                    "new_symbol",
                    "acquiree_symbol",
                    "acquirer_symbol",
                    "source_symbol",
                    "target_symbol",
                ]
                .into_iter()
                .filter_map(|k| r.row.get(k).and_then(Value::as_str).map(str::to_string))
            })
            .collect();
        out.sort();
        out.dedup();
        out
    }
}

pub fn summarize_corporate_actions(pages: &[Value]) -> ActionSummary {
    let mut s = ActionSummary::default();
    for p in pages {
        let Some(map) = p.get("corporate_actions").and_then(Value::as_object) else {
            continue;
        };
        for (kind, rows) in map {
            for row in rows.as_array().into_iter().flatten() {
                *s.counts.entry(kind.clone()).or_default() += 1;
                s.rows.push(ActionRow {
                    kind: kind.clone(),
                    row: row.clone(),
                });
            }
        }
    }
    s
}

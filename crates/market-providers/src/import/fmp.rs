//! FMP historical 1-minute bars (`GET /stable/historical-chart/1min`) as a **separate** dataset.
//!
//! Reference: <https://site.financialmodelingprep.com/developer/docs/stable/intraday-1-min>.
//!
//! These bars are a comparison / fallback input only. A minute bar cannot say in which order the
//! minute's trades happened, so it never becomes trades, never feeds the chart engine, never makes
//! a 15 s / 30 s candle or a trade-based volume profile. It is stored with
//! `Archive::write_bars` under `bars/fmp-1min/`, labelled with provider and granularity.
//!
//! FMP authenticates with an `apikey` query parameter: the URL is only ever logged through
//! `redact_url`, and the stored `query` omits the key entirely. A plan that does not include the
//! endpoint answers 401/402/403 (or 200 with an `"Error Message"` object): that is reported as an
//! entitlement limitation, never worked around.

use super::http::{FetchError, Http, RetryPolicy, get_with_retry, scrub};
use chrono::{NaiveDate, NaiveDateTime, TimeZone, Utc};
use chrono_tz::America::New_York;
use market_archive::{Bar, BarDataset};
use market_events::{Price, UnixNanos};
use std::time::Duration;

pub const PROVIDER: &str = "fmp";
pub const GRANULARITY: &str = "1min";

#[derive(Clone)]
pub struct FmpKey(pub String);

impl std::fmt::Debug for FmpKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("FmpKey(REDACTED)")
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum FmpError {
    /// The plan does not include this endpoint or history (or the key was refused).
    NotEntitled {
        status: u16,
        message: String,
    },
    Fetch(FetchError),
    Malformed(String),
}

impl std::fmt::Display for FmpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FmpError::NotEntitled { status, message } => {
                write!(
                    f,
                    "not available on this FMP plan or key (HTTP {status}): {message}"
                )
            }
            FmpError::Fetch(e) => write!(f, "{e}"),
            FmpError::Malformed(m) => write!(f, "malformed response: {m}"),
        }
    }
}

/// The request path and query **without** the key: what is stored and logged.
pub fn public_query(symbol: &str, day: NaiveDate) -> String {
    format!("/stable/historical-chart/1min?symbol={symbol}&from={day}&to={day}")
}

/// Fetches one symbol's 1-minute bars for one trading date (ascending, inside that date in ET).
#[allow(clippy::too_many_arguments)]
pub fn fetch_minute_bars(
    http: &mut dyn Http,
    base_url: &str,
    key: &FmpKey,
    symbol: &str,
    day: NaiveDate,
    retry: RetryPolicy,
    sleep: &mut dyn FnMut(Duration),
    log: &mut dyn FnMut(&str),
) -> Result<BarDataset, FmpError> {
    let query = public_query(symbol, day);
    let url = format!("{}{query}&apikey={}", base_url.trim_end_matches('/'), key.0);
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
    let rows = value
        .as_array()
        .ok_or_else(|| FmpError::Malformed("expected a JSON array of bars".into()))?;
    let mut bars = Vec::with_capacity(rows.len());
    for r in rows {
        bars.push(parse_bar(r).map_err(FmpError::Malformed)?);
    }
    bars.retain(|b| et_date(b.t) == Some(day));
    bars.sort_by_key(|b| b.t);
    bars.dedup_by_key(|b| b.t);
    Ok(BarDataset {
        provider: PROVIDER.into(),
        granularity: GRANULARITY.into(),
        symbol: symbol.into(),
        day: day.to_string(),
        query,
        time_zone: "America/New_York".into(),
        fetched_at: super::alpaca::rfc3339(Utc::now()),
        bars,
    })
}

/// `{"date":"2026-10-15 09:30:00","open":..,"low":..,"high":..,"close":..,"volume":..}`; the date
/// is exchange (ET) local time.
fn parse_bar(r: &serde_json::Value) -> Result<Bar, String> {
    let date = r
        .get("date")
        .and_then(|d| d.as_str())
        .ok_or("bar without a date")?;
    let local = NaiveDateTime::parse_from_str(date, "%Y-%m-%d %H:%M:%S")
        .map_err(|e| format!("bad bar date {date:?}: {e}"))?;
    let utc = New_York
        .from_local_datetime(&local)
        .earliest()
        .ok_or_else(|| format!("bar date {date:?} does not exist in America/New_York"))?
        .with_timezone(&Utc);
    let t = UnixNanos::from_utc(utc)
        .ok_or_else(|| format!("bar date {date:?} out of range"))?
        .0;
    let px = |k: &str| -> Result<i64, String> {
        let n = r.get(k).ok_or_else(|| format!("bar without {k}"))?;
        let s = n.to_string();
        s.parse::<Price>()
            .ok()
            .or_else(|| n.as_f64().and_then(Price::from_f64_rounded))
            .map(Price::micros)
            .ok_or_else(|| format!("bad {k} {s}"))
    };
    let v = r
        .get("volume")
        .and_then(|v| v.as_u64().or_else(|| v.as_f64().map(|f| f.max(0.0) as u64)))
        .ok_or("bar without volume")?;
    Ok(Bar {
        t,
        o: px("open")?,
        h: px("high")?,
        l: px("low")?,
        c: px("close")?,
        v,
    })
}

fn et_date(t: i64) -> Option<NaiveDate> {
    Some(UnixNanos(t).to_utc().with_timezone(&New_York).date_naive())
}

//! Provider bar datasets: a separate store beside the event archive.
//!
//! A bar dataset (FMP 1-minute OHLCV today) is **not** market events. It cannot say in which
//! order trades happened inside a minute, cannot make a 15 s or 30 s candle and cannot build a
//! trade-based volume profile, so it is never written into a `.swa` event file and never replayed
//! through the chart engine. It is stored as its own JSON file, keyed by provider and granularity,
//! and listed in the day manifest's `bars` map with its own checksum. Its uses are comparison
//! against trade-built candles and fallback context, always labelled with the provider and the
//! granularity.
//!
//! Layout: `$ARCHIVE_ROOT/<trading date>/bars/<provider>-<granularity>/<SYMBOL>.json`.

use serde::{Deserialize, Serialize};

/// Directory under a day directory that holds bar datasets.
pub const BARS_DIR: &str = "bars";

/// One OHLCV bar. Prices are micro-dollars (the event contract's unit); `t` is the bar's start in
/// UTC nanoseconds.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Bar {
    pub t: i64,
    pub o: i64,
    pub h: i64,
    pub l: i64,
    pub c: i64,
    pub v: u64,
}

/// A provider's bars for one symbol on one trading date, with where they came from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BarDataset {
    /// e.g. `"fmp"`.
    pub provider: String,
    /// e.g. `"1min"`. Bars are never finer-grained than this, and never trades.
    pub granularity: String,
    pub symbol: String,
    /// Trading date, `YYYY-MM-DD`.
    pub day: String,
    /// The endpoint path and query **without credentials**, e.g.
    /// `/stable/historical-chart/1min?symbol=X&from=…&to=…`.
    pub query: String,
    /// How the provider's local bar times were read (FMP intraday times are exchange time).
    pub time_zone: String,
    /// UTC RFC 3339 time of the fetch.
    pub fetched_at: String,
    /// Bars ascending by `t`.
    pub bars: Vec<Bar>,
}

/// The manifest's record of a bar dataset file.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BarEntry {
    pub provider: String,
    pub granularity: String,
    pub symbol: String,
    /// Path relative to the day directory, e.g. `"bars/fmp-1min/ABC.json"`.
    pub file: String,
    pub bars: u64,
    pub first_ts: Option<i64>,
    pub last_ts: Option<i64>,
    pub query: String,
    pub fetched_at: String,
    pub bytes: u64,
    pub sha256: String,
}

impl BarEntry {
    /// The manifest key, e.g. `"fmp/1min/ABC"`.
    pub fn key(provider: &str, granularity: &str, symbol: &str) -> String {
        format!("{provider}/{granularity}/{symbol}")
    }
}

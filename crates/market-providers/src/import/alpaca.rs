//! Alpaca historical trades (`GET /v2/stocks/{symbol}/trades`) into the event archive.
//!
//! The trade-faithful source for replay: every print with its exchange timestamp (nanoseconds),
//! price, size, venue, tape, sale conditions and id, normalized once into the event contract's
//! [`Trade`] and streamed page by page into the archive writer, so a session is never held in
//! memory. Reference: <https://docs.alpaca.markets/us/reference/stocktrades-1>.
//!
//! What is kept and what is not:
//! - `t` → `event_ts` exactly (nanoseconds, UTC). No receive time is invented: historical prints
//!   have none (`receive_ts` stays `None`).
//! - `p`, `s`, `x`, `z`, `c`, `i` → price (exact decimal), size, exchange, tape, conditions,
//!   `source_seq`.
//! - `u` (a later verdict on the print: `canceled`, `incorrect`, `corrected`) is kept raw in
//!   [`Trade::update`]. The response gives it no publication time and no link from a corrected
//!   print to the one it replaces, so it is **not** turned into a `Correction` or `Cancel` event;
//!   `Trade::effect` excludes `canceled` / `incorrect` prints from candles, and `corrected` prints
//!   count normally. The counts are recorded in the manifest.
//! - Feed: `sip` → `Source::AlpacaSip`, `iex` → `AlpacaIex`, `boats` / `overnight` →
//!   `AlpacaBoats`. The feed is part of the request and of the provenance, never assumed.
//!
//! Completeness: pages are followed through `next_page_token` until the provider returns none,
//! whatever a page's length. Any failure leaves the `.partial` file and **no** manifest entry, so
//! an interrupted import is never labelled complete; re-running it starts over and replaces the
//! partial (idempotent for the same provider, feed, symbol and range).

use super::http::{FetchError, Http, RetryPolicy, get_with_retry};
use chrono::{DateTime, NaiveDate, SecondsFormat, Utc};
use market_archive::{Archive, ArchiveError, FsyncPolicy, ImportProvenance, SymbolEntry};
use market_events::{MarketEvent, Price, Source, Tape, Trade, UnixNanos};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::time::Duration;

/// Alpaca's page size ceiling for this endpoint.
pub const MAX_LIMIT: u32 = 10_000;
/// Guard against a provider that never stops paginating.
const MAX_PAGES: u64 = 100_000;

/// The keys, read from the environment by the caller. `Debug` never shows them.
#[derive(Clone, PartialEq, Eq)]
pub struct AlpacaCredentials {
    pub key_id: String,
    pub secret: String,
}

impl AlpacaCredentials {
    /// The env names every Alpaca path here reads (importer and live stream alike):
    /// `ALPACA_API_KEY` / `ALPACA_API_SECRET`, else Alpaca's own SDK names `APCA_API_KEY_ID` /
    /// `APCA_API_SECRET_KEY`. `None` unless both halves are set and non-empty.
    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Option<Self> {
        let var = |k: &str| {
            get(k)
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
        };
        Some(AlpacaCredentials {
            key_id: var("ALPACA_API_KEY").or_else(|| var("APCA_API_KEY_ID"))?,
            secret: var("ALPACA_API_SECRET").or_else(|| var("APCA_API_SECRET_KEY"))?,
        })
    }
}

impl std::fmt::Debug for AlpacaCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("AlpacaCredentials { key_id: REDACTED, secret: REDACTED }")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Feed {
    Sip,
    Iex,
    Boats,
    Overnight,
}

impl Feed {
    pub fn parse(s: &str) -> Result<Feed, String> {
        match s.to_ascii_lowercase().as_str() {
            "sip" => Ok(Feed::Sip),
            "iex" => Ok(Feed::Iex),
            "boats" => Ok(Feed::Boats),
            "overnight" => Ok(Feed::Overnight),
            other => Err(format!(
                "unknown feed {other:?} (sip, iex, boats, overnight)"
            )),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Feed::Sip => "sip",
            Feed::Iex => "iex",
            Feed::Boats => "boats",
            Feed::Overnight => "overnight",
        }
    }

    pub fn source(self) -> Source {
        match self {
            Feed::Sip => Source::AlpacaSip,
            Feed::Iex => Source::AlpacaIex,
            Feed::Boats | Feed::Overnight => Source::AlpacaBoats,
        }
    }
}

#[derive(Clone, Debug)]
pub struct TradesQuery {
    pub symbol: String,
    /// The archive day (ET trading date) the events belong to.
    pub day: NaiveDate,
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    pub feed: Feed,
    pub limit: u32,
}

impl TradesQuery {
    pub fn endpoint(&self) -> String {
        format!("/v2/stocks/{}/trades", self.symbol)
    }

    fn url(&self, base: &str, page_token: Option<&str>) -> String {
        let mut u = format!(
            "{}{}?start={}&end={}&limit={}&feed={}&sort=asc",
            base.trim_end_matches('/'),
            self.endpoint(),
            rfc3339(self.start),
            rfc3339(self.end),
            self.limit.clamp(1, MAX_LIMIT),
            self.feed.as_str()
        );
        if let Some(t) = page_token {
            u.push_str("&page_token=");
            u.push_str(&percent_encode(t));
        }
        u
    }

    pub fn provenance(&self) -> ImportProvenance {
        ImportProvenance {
            provider: "alpaca".into(),
            dataset: "trades".into(),
            endpoint: self.endpoint(),
            feed: self.feed.as_str().into(),
            start: rfc3339(self.start),
            end: rfc3339(self.end),
            ..ImportProvenance::default()
        }
    }
}

/// One element of the response's `trades` array, as documented.
#[derive(Clone, Debug, Deserialize)]
pub struct RawTrade {
    pub t: String,
    #[serde(default)]
    pub x: String,
    pub p: serde_json::Number,
    pub s: u64,
    #[serde(default)]
    pub c: Option<Vec<String>>,
    pub i: u64,
    #[serde(default)]
    pub z: Option<String>,
    #[serde(default)]
    pub u: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TradesPage {
    #[serde(default)]
    trades: Option<Vec<RawTrade>>,
    #[serde(default)]
    symbol: Option<String>,
    #[serde(default)]
    next_page_token: Option<String>,
}

/// One provider print → the event contract. Fails on a malformed time or price, never guesses.
///
/// The live stream's `"T":"t"` message carries the same fields (plus `T`/`S`), so the stream
/// client deserializes it into [`RawTrade`] and calls this too: one print yields the same
/// [`Trade`] whether it came from history or from the socket.
pub fn to_trade(raw: &RawTrade, symbol: &str, source: Source) -> Result<Trade, String> {
    let event_ts = parse_time(&raw.t).map_err(|e| format!("bad trade time: {e}"))?;
    let price = parse_price(&raw.p).ok_or_else(|| format!("bad price {}", raw.p))?;
    let mut t = Trade::new(symbol, event_ts, price, raw.s, raw.i)
        .with_exchange(raw.x.clone())
        .with_tape(raw.z.as_deref().map_or(Tape::Unknown, Tape::from_code))
        .with_source(source)
        .with_conditions(raw.c.clone().unwrap_or_default());
    if let Some(u) = raw.u.as_deref().filter(|u| !u.trim().is_empty()) {
        t = t.with_update(u.trim());
    }
    Ok(t)
}

/// Alpaca's RFC 3339 time (nanosecond precision) → contract time.
pub(crate) fn parse_time(t: &str) -> Result<UnixNanos, String> {
    let ts = DateTime::parse_from_rfc3339(t)
        .map_err(|e| format!("{t:?}: {e}"))?
        .with_timezone(&Utc);
    UnixNanos::from_utc(ts).ok_or_else(|| format!("{t:?} out of range"))
}

/// Exact decimal when the JSON number is plain (`3.18`), rounding to the micro-dollar otherwise.
pub(crate) fn parse_price(n: &serde_json::Number) -> Option<Price> {
    let s = n.to_string();
    s.parse::<Price>()
        .ok()
        .or_else(|| n.as_f64().and_then(Price::from_f64_rounded))
        .filter(|p| p.is_positive())
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ImportReport {
    /// True when an identical, complete import already existed and nothing was fetched.
    pub skipped: bool,
    pub pages: u64,
    /// Pages that held no trades (an empty final page is normal).
    pub empty_pages: u64,
    pub events: u64,
    pub first_ts: Option<i64>,
    pub last_ts: Option<i64>,
    pub updates: BTreeMap<String, u64>,
    pub out_of_order: u64,
    pub entry: Option<SymbolEntry>,
}

#[derive(Debug)]
pub enum ImportError {
    Fetch {
        page: u64,
        error: FetchError,
    },
    Malformed {
        page: u64,
        message: String,
    },
    Pagination(String),
    Archive(ArchiveError),
    /// The day already holds a different dataset for this symbol; pass `force` to replace it.
    Occupied(String),
}

impl std::fmt::Display for ImportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ImportError::Fetch { page, error } => write!(f, "page {page}: {error}"),
            ImportError::Malformed { page, message } => {
                write!(f, "page {page}: malformed response: {message}")
            }
            ImportError::Pagination(m) => write!(f, "pagination: {m}"),
            ImportError::Archive(e) => write!(f, "archive: {e}"),
            ImportError::Occupied(m) => f.write_str(m),
        }
    }
}

impl From<ArchiveError> for ImportError {
    fn from(e: ArchiveError) -> Self {
        ImportError::Archive(e)
    }
}

#[derive(Default)]
pub struct ImportOptions {
    pub retry: RetryPolicy,
    /// Replace an existing different (or incomplete) dataset for this day and symbol.
    pub force: bool,
}

/// Imports one symbol and range into `archive` under `query.day`. See the module docs.
#[allow(clippy::too_many_arguments)]
pub fn import_trades(
    http: &mut dyn Http,
    base_url: &str,
    creds: &AlpacaCredentials,
    query: &TradesQuery,
    archive: &Archive,
    opts: &ImportOptions,
    sleep: &mut dyn FnMut(Duration),
    log: &mut dyn FnMut(&str),
) -> Result<ImportReport, ImportError> {
    let wanted = query.provenance();
    if let Some(existing) = archive
        .manifest(query.day)?
        .and_then(|m| m.symbols.get(&query.symbol).cloned())
    {
        match &existing.import {
            Some(p) if p.complete && p.same_request(&wanted) && !opts.force => {
                log("an identical complete import already exists: nothing fetched");
                return Ok(ImportReport {
                    skipped: true,
                    pages: p.pages,
                    events: existing.events,
                    first_ts: existing.first_ts,
                    last_ts: existing.last_ts,
                    updates: p.updates.clone(),
                    out_of_order: p.out_of_order,
                    entry: Some(existing),
                    ..ImportReport::default()
                });
            }
            _ if !opts.force => {
                return Err(ImportError::Occupied(format!(
                    "{} {} already holds a different dataset ({}); pass --force to replace it",
                    query.day,
                    query.symbol,
                    describe(&existing)
                )));
            }
            _ => {}
        }
    }

    let source = query.feed.source();
    let headers = [
        ("APCA-API-KEY-ID", creds.key_id.as_str()),
        ("APCA-API-SECRET-KEY", creds.secret.as_str()),
        ("Accept", "application/json"),
    ];
    let secrets = [creds.key_id.as_str(), creds.secret.as_str()];
    let mut writer = archive.writer(query.day, &query.symbol, FsyncPolicy::OnFinalize)?;
    let mut report = ImportReport::default();
    let mut token: Option<String> = None;
    let mut last_ts: Option<i64> = None;
    loop {
        let page = report.pages + 1;
        if page > MAX_PAGES {
            return Err(ImportError::Pagination(format!(
                "more than {MAX_PAGES} pages; stopped"
            )));
        }
        let url = query.url(base_url, token.as_deref());
        let resp = get_with_retry(http, &url, &headers, opts.retry, &secrets, sleep, log)
            .map_err(|error| ImportError::Fetch { page, error })?;
        let body: TradesPage =
            serde_json::from_slice(&resp.body).map_err(|e| ImportError::Malformed {
                page,
                message: e.to_string(),
            })?;
        if let Some(sym) = body.symbol.as_deref()
            && !sym.eq_ignore_ascii_case(&query.symbol)
        {
            return Err(ImportError::Malformed {
                page,
                message: format!("response is for {sym}, not {}", query.symbol),
            });
        }
        let trades = body.trades.unwrap_or_default();
        if trades.is_empty() {
            report.empty_pages += 1;
        }
        for raw in &trades {
            let t = to_trade(raw, &query.symbol, source)
                .map_err(|message| ImportError::Malformed { page, message })?;
            let ts = t.event_ts.0;
            if last_ts.is_some_and(|l| ts < l) {
                report.out_of_order += 1;
            }
            last_ts = Some(last_ts.map_or(ts, |l| l.max(ts)));
            if let Some(u) = &t.update {
                *report.updates.entry(u.clone()).or_default() += 1;
            }
            report.first_ts = Some(report.first_ts.map_or(ts, |f| f.min(ts)));
            report.last_ts = Some(report.last_ts.map_or(ts, |l| l.max(ts)));
            writer.append(&MarketEvent::Trade(t))?;
            report.events += 1;
        }
        report.pages = page;
        log(&format!(
            "page {page}: {} trades (total {})",
            trades.len(),
            report.events
        ));
        match body.next_page_token.filter(|t| !t.is_empty()) {
            None => break,
            Some(next) if token.as_deref() == Some(next.as_str()) => {
                return Err(ImportError::Pagination(format!(
                    "page {page} returned the token it was requested with; refusing to loop"
                )));
            }
            Some(next) => token = Some(next),
        }
    }

    let mut entry = writer.finalize()?;
    entry.import = Some(ImportProvenance {
        imported_at: rfc3339(Utc::now()),
        pages: report.pages,
        complete: true,
        updates: report.updates.clone(),
        out_of_order: report.out_of_order,
        ..wanted
    });
    archive.commit(query.day, entry.clone())?;
    report.entry = Some(entry);
    Ok(report)
}

fn describe(e: &SymbolEntry) -> String {
    match &e.import {
        Some(p) => format!(
            "{} {} {} {}..{}{}",
            p.provider,
            p.dataset,
            p.feed,
            p.start,
            p.end,
            if p.complete { "" } else { ", incomplete" }
        ),
        None if e.is_fixture() => "FIXTURE".into(),
        None => format!("{:?}", e.sources),
    }
}

pub fn rfc3339(t: DateTime<Utc>) -> String {
    t.to_rfc3339_opts(SecondsFormat::AutoSi, true)
}

/// Page tokens are opaque base64-ish strings; encode everything outside the URL-safe set.
fn percent_encode(s: &str) -> String {
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

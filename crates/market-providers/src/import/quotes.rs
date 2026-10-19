//! Alpaca historical quotes (`GET /v2/stocks/{symbol}/quotes`, NBBO on the SIP feed) into the
//! event archive as [`MarketEvent::Quote`]. Reference:
//! <https://docs.alpaca.markets/us/reference/stockquotes-1>.
//!
//! Same shape and guarantees as the trades importer ([`super::alpaca::import_trades`]): streamed
//! page by page into one `.swa` file per (day, symbol), followed through `next_page_token` until
//! the provider returns none, a failure leaves only the `.partial` file and no manifest entry,
//! and an identical complete import is skipped. Differences, all deliberate:
//!
//! - **A separate archive root.** The archive holds one file per (day, symbol), so quotes go to
//!   their own root (the research layout uses `archive-quotes/`), never into the trades file.
//!   Mixing them would change what replay feeds the chart engine without anyone asking for it.
//! - **Zero prices are valid.** A quote with no bid (or no ask) arrives as price 0 / size 0;
//!   that is information about liquidity, not a malformed record, so it is kept and counted.
//!   A negative price, an unparseable time or a record that does not deserialize is
//!   **quarantined**: handed to the caller's `quarantine` callback with the reason, counted, and
//!   left out of the archive. One bad record does not throw a session away, and nothing is
//!   dropped silently.
//! - Sizes are stored exactly as delivered (`bs`, `as`). Whether they are shares or round lots
//!   is a property of the provider's feed version; [`QuoteReport::size_multiple_of_100`] lets a
//!   reader check the data rather than assume.
//! - `event_ts` = the quote's SIP/exchange time (`t`); `receive_ts` stays `None` (history has no
//!   arrival time).

use super::alpaca::{AlpacaCredentials, Feed, ImportError, ImportOptions, parse_time, rfc3339};
use super::http::{Http, get_with_retry};
use chrono::{DateTime, NaiveDate, Utc};
use market_archive::{Archive, FsyncPolicy, ImportProvenance, SymbolEntry};
use market_events::{MarketEvent, Price, Quote, Tape};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::hash::{Hash, Hasher};
use std::time::Duration;

const MAX_PAGES: u64 = 100_000;

#[derive(Clone, Debug)]
pub struct QuotesQuery {
    pub symbol: String,
    pub day: NaiveDate,
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    pub feed: Feed,
    pub limit: u32,
}

impl QuotesQuery {
    pub fn endpoint(&self) -> String {
        format!("/v2/stocks/{}/quotes", self.symbol)
    }

    fn url(&self, base: &str, page_token: Option<&str>) -> String {
        let mut u = format!(
            "{}{}?start={}&end={}&limit={}&feed={}&sort=asc",
            base.trim_end_matches('/'),
            self.endpoint(),
            rfc3339(self.start),
            rfc3339(self.end),
            self.limit.clamp(1, super::alpaca::MAX_LIMIT),
            self.feed.as_str()
        );
        if let Some(t) = page_token {
            u.push_str("&page_token=");
            u.push_str(&super::reference::percent_encode(t));
        }
        u
    }

    pub fn provenance(&self) -> ImportProvenance {
        ImportProvenance {
            provider: "alpaca".into(),
            dataset: "quotes".into(),
            endpoint: self.endpoint(),
            feed: self.feed.as_str().into(),
            start: rfc3339(self.start),
            end: rfc3339(self.end),
            ..ImportProvenance::default()
        }
    }
}

/// One element of the response's `quotes` array, as documented.
#[derive(Clone, Debug, Deserialize)]
pub struct RawQuote {
    pub t: String,
    #[serde(default)]
    pub bx: String,
    pub bp: serde_json::Number,
    pub bs: u64,
    #[serde(default)]
    pub ax: String,
    pub ap: serde_json::Number,
    #[serde(rename = "as")]
    pub as_: u64,
    #[serde(default)]
    pub c: Option<Vec<String>>,
    #[serde(default)]
    pub z: Option<String>,
}

/// A quote price: exact decimal, zero allowed (an empty side), negative refused.
fn parse_quote_price(n: &serde_json::Number) -> Option<Price> {
    n.to_string()
        .parse::<Price>()
        .ok()
        .or_else(|| n.as_f64().and_then(Price::from_f64_rounded))
        .filter(|p| p.micros() >= 0)
}

/// One provider quote → the event contract. Errors name the field; the caller quarantines.
pub fn to_quote(
    raw: &RawQuote,
    symbol: &str,
    source: market_events::Source,
) -> Result<Quote, String> {
    let event_ts = parse_time(&raw.t).map_err(|e| format!("bad quote time: {e}"))?;
    let bid_price = parse_quote_price(&raw.bp).ok_or_else(|| format!("bad bid {}", raw.bp))?;
    let ask_price = parse_quote_price(&raw.ap).ok_or_else(|| format!("bad ask {}", raw.ap))?;
    Ok(Quote {
        symbol: symbol.to_string(),
        event_ts,
        receive_ts: None,
        bid_price,
        bid_size: raw.bs,
        bid_exchange: raw.bx.clone(),
        ask_price,
        ask_size: raw.as_,
        ask_exchange: raw.ax.clone(),
        conditions: raw.c.clone().unwrap_or_default(),
        tape: raw.z.as_deref().map_or(Tape::Unknown, Tape::from_code),
        source,
    })
}

#[derive(Debug, Deserialize)]
struct QuotesPage {
    #[serde(default)]
    quotes: Option<Vec<serde_json::Value>>,
    #[serde(default)]
    symbol: Option<String>,
    #[serde(default)]
    next_page_token: Option<String>,
}

/// What one quotes import saw. Counts are over archived quotes unless named otherwise.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct QuoteReport {
    pub skipped: bool,
    pub pages: u64,
    pub empty_pages: u64,
    pub events: u64,
    pub first_ts: Option<i64>,
    pub last_ts: Option<i64>,
    pub out_of_order: u64,
    /// Records refused (see the module docs); not in the archive.
    pub quarantined: u64,
    /// Records identical in every field to an earlier one of the same import (kept, counted).
    pub exact_duplicates: u64,
    /// bid > ask (both sides present).
    pub crossed: u64,
    /// bid == ask (both sides present).
    pub locked: u64,
    pub zero_bid: u64,
    pub zero_ask: u64,
    /// Quotes whose bid and ask sizes are both multiples of 100 (shares-vs-lots evidence).
    pub size_multiple_of_100: u64,
    pub max_size: u64,
    pub conditions: BTreeMap<String, u64>,
    #[serde(skip)]
    pub entry: Option<SymbolEntry>,
}

/// Imports one symbol's quotes for `query` into `archive` (a quotes-only root). `quarantine`
/// receives each refused record (raw JSON) and the reason.
#[allow(clippy::too_many_arguments)]
pub fn import_quotes(
    http: &mut dyn Http,
    base_url: &str,
    creds: &AlpacaCredentials,
    query: &QuotesQuery,
    archive: &Archive,
    opts: &ImportOptions,
    sleep: &mut dyn FnMut(Duration),
    log: &mut dyn FnMut(&str),
    quarantine: &mut dyn FnMut(&str, &str),
) -> Result<QuoteReport, ImportError> {
    let wanted = query.provenance();
    if let Some(existing) = archive
        .manifest(query.day)?
        .and_then(|m| m.symbols.get(&query.symbol).cloned())
    {
        match &existing.import {
            Some(p) if p.complete && p.same_request(&wanted) && !opts.force => {
                log("an identical complete quotes import already exists: nothing fetched");
                return Ok(QuoteReport {
                    skipped: true,
                    pages: p.pages,
                    events: existing.events,
                    first_ts: existing.first_ts,
                    last_ts: existing.last_ts,
                    entry: Some(existing),
                    ..QuoteReport::default()
                });
            }
            _ if !opts.force => {
                return Err(ImportError::Occupied(format!(
                    "{} {} already holds a different dataset; pass force to replace it",
                    query.day, query.symbol
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
    let mut report = QuoteReport::default();
    let mut seen: HashSet<u64> = HashSet::new();
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
        let body: QuotesPage =
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
        let quotes = body.quotes.unwrap_or_default();
        if quotes.is_empty() {
            report.empty_pages += 1;
        }
        for value in &quotes {
            let parsed = serde_json::from_value::<RawQuote>(value.clone())
                .map_err(|e| e.to_string())
                .and_then(|raw| to_quote(&raw, &query.symbol, source));
            let q = match parsed {
                Ok(q) => q,
                Err(reason) => {
                    report.quarantined += 1;
                    quarantine(&value.to_string(), &reason);
                    continue;
                }
            };
            let mut h = std::collections::hash_map::DefaultHasher::new();
            value.to_string().hash(&mut h);
            if !seen.insert(h.finish()) {
                report.exact_duplicates += 1;
            }
            tally(&mut report, &q);
            let ts = q.event_ts.0;
            if last_ts.is_some_and(|l| ts < l) {
                report.out_of_order += 1;
            }
            last_ts = Some(last_ts.map_or(ts, |l| l.max(ts)));
            report.first_ts = Some(report.first_ts.map_or(ts, |f| f.min(ts)));
            report.last_ts = Some(report.last_ts.map_or(ts, |l| l.max(ts)));
            writer.append(&MarketEvent::Quote(q))?;
            report.events += 1;
        }
        report.pages = page;
        log(&format!(
            "quotes page {page}: {} (total {})",
            quotes.len(),
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
        out_of_order: report.out_of_order,
        ..wanted
    });
    archive.commit(query.day, entry.clone())?;
    report.entry = Some(entry);
    Ok(report)
}

fn tally(r: &mut QuoteReport, q: &Quote) {
    let (b, a) = (q.bid_price.micros(), q.ask_price.micros());
    if b == 0 {
        r.zero_bid += 1;
    }
    if a == 0 {
        r.zero_ask += 1;
    }
    if b > 0 && a > 0 {
        if b > a {
            r.crossed += 1;
        } else if b == a {
            r.locked += 1;
        }
    }
    if q.bid_size.is_multiple_of(100) && q.ask_size.is_multiple_of(100) {
        r.size_multiple_of_100 += 1;
    }
    r.max_size = r.max_size.max(q.bid_size).max(q.ask_size);
    for c in &q.conditions {
        *r.conditions.entry(c.clone()).or_default() += 1;
    }
}

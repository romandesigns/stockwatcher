//! SEC EDGAR filings as the Ross Panel's second news source (owner, 2026-11-22; pillars-v6):
//! `swimport evidence`'s third worker, beside the Benzinga news and FMP float halves.
//!
//! # Why
//!
//! The 2026-11-20 completeness audit (`crates/server/src/ross_audit.rs`) found 36 symbols passing
//! price, gain and RVOL and none on the panel: every one was held off by the news pillar, whose
//! only source was Benzinga. Small caps put their catalysts out as SEC filings first, and
//! Benzinga often has no story for them. EDGAR is the regulator's own record, free, keyless and
//! public: no vendor account, no metered API (workspace rule "open source first": a runtime call,
//! but to the primary source, which no vendor can withdraw).
//!
//! # What it reads (and how often)
//!
//! | What | Endpoint | When |
//! |---|---|---|
//! | ticker -> CIK | `/files/company_tickers.json` (~1 MB) | at start, then once every `EDGAR_TICKERS_REFRESH_HOURS` (24) |
//! | new filings | `/cgi-bin/browse-edgar?action=getcurrent&owner=exclude&count=100&start=N&output=atom` | every `EDGAR_POLL_SECS` (60) 06:00-22:00 ET on weekdays (EDGAR's filing hours), every `EDGAR_IDLE_POLL_SECS` (600) otherwise; a poll pages `start=0,100,...` until it reaches what the previous poll already covered |
//! | back-fill | `/Archives/edgar/daily-index/YYYY/QTRn/form.YYYYMMDD.idx`, then each relevant filing's index page for its acceptance time | once per start, only for the dates in the freshness window the feed could not reach |
//!
//! Every request goes through one pacer, `EDGAR_PER_MINUTE` (60: one a second, a tenth of SEC's
//! 10-a-second fair-access limit), retries included. A normal minute is **one** request.
//! `owner=exclude` leaves out Forms 3/4/5, which are most of EDGAR's volume and never a catalyst.
//!
//! **User-Agent.** SEC requires a descriptive User-Agent with contact details
//! (<https://www.sec.gov/os/accessing-edgar-data>). It is read from `EDGAR_USER_AGENT` and never
//! defaulted: unset, this worker does not start and the log says so once. Nothing here contains a
//! contact address.
//!
//! # Point in time
//!
//! - A filing's event time is its **acceptance** time (the feed's `<updated>`, which EDGAR's
//!   "Latest Filings" page lists as *Accepted*). The pillar never counts it before that
//!   (`published_at <= cutoff`), nor before `first_observed_at`, our first sighting.
//! - A daily index row has a filing date and no time. It is fetched only to back-fill, and the
//!   filing's own index page is then read for its acceptance time (`Accepted`, ET). When that page
//!   cannot be read (or the per-start cap is spent) the row is dated by the **earliest**
//!   acceptance its filing date allows, [`earliest_acceptance`]: 17:30 ET on the previous SEC
//!   business day (EDGAR dates a filing accepted after 17:30 with the next business day). Earliest
//!   so the 24 h freshness runs out no later than the real filing's would; it cannot count early
//!   either, because the index is published after its day and the record joins decisions only
//!   from `first_observed_at`.
//! - Tickers are mapped when the filing is first seen, with the map held then, and stored on the
//!   record: a later ticker change never rewrites what an earlier decision read.
//!
//! # What is kept
//!
//! Only forms the panel reads (`market_archive::evidence_load::filing_class`): 8-K and 6-K (a
//! catalyst), 8-K/A and 6-K/A (recorded, not a pass), and offering registrations and prospectuses
//! (S-1, S-3, F-1, F-3 and variants, 424B*: dilution, flagged, never a pass), each only when its
//! CIK has a ticker. One record per accession (`market_archive::FilingRecord`) in
//! `evidence/filings/`; each complete poll writes one coverage record to
//! `evidence/filings-coverage/`, the collector's own restart state (the pillar never reads it: a
//! news FAIL still rests on Benzinga's market-wide coverage).
//!
//! # Ticker notation
//!
//! SEC lists class shares as `BRK-B`; the app (Alpaca) writes `BRK.B`. [`app_symbol`] converts
//! only notations with a known one-to-one form (a one-letter class, a preferred series `-P<x>`
//! to `.PR<x>`, units `-UN` to `.U`) and drops everything else, whole. It never splits a ticker
//! into parts: research once read class tickers like `FHN PR E` and `MS/PL` as `FHN`, `PR`, `E`,
//! `MS` and `PL`, and so mapped the real tickers `F`, `PR` and `PL` to banks' filings.

use super::evidence_collect::Pacer;
use super::http::{FetchError, Http, RetryPolicy, get_with_retry};
use chrono::{
    DateTime, Datelike, Duration as Days, NaiveDate, NaiveDateTime, TimeZone, Timelike, Utc,
    Weekday,
};
use chrono_tz::America::New_York;
use market_archive::evidence_load::{FilingClass, filing_class};
use market_archive::{
    Archive, CoverageScope, DATED_BY_ACCEPTANCE, DATED_BY_DAILY_INDEX, FilingRecord,
    NewsCoverageRecord,
};
use std::collections::{BTreeMap, HashMap};
use std::time::Duration;

const SEC: i64 = 1_000_000_000;
pub const PROVIDER: &str = "sec_edgar";
/// Entries per feed page: the largest `count` the feed accepts.
pub const FEED_PAGE: usize = 100;

/// Environment of the EDGAR worker.
///
/// | Variable | Default | Meaning |
/// |---|---|---|
/// | `EDGAR_USER_AGENT` | **none: the worker stays off** | SEC's required descriptive User-Agent with a contact, e.g. `ExampleCo market-data admin@example.com` |
/// | `EDGAR_BASE` | `https://www.sec.gov` | the host (tests point it elsewhere) |
/// | `EDGAR_POLL_SECS` | `60` | feed poll interval, 06:00-22:00 ET on weekdays |
/// | `EDGAR_IDLE_POLL_SECS` | `600` | feed poll interval otherwise |
/// | `EDGAR_PER_MINUTE` | `60` | request ceiling for this worker, retries included |
/// | `EDGAR_POLL_MAX_PAGES` | `10` | feed pages one forward poll may read |
/// | `EDGAR_BACKFILL_MAX_PAGES` | `60` | feed pages the first poll after a start may read |
/// | `EDGAR_BACKFILL_DETAIL_MAX` | `600` | filing index pages a back-fill may read for acceptance times |
/// | `EDGAR_TICKERS_REFRESH_HOURS` | `24` | how often the ticker map is fetched again |
#[derive(Clone, Debug)]
pub struct EdgarConfig {
    pub user_agent: String,
    pub base: String,
    pub poll_secs: i64,
    pub idle_poll_secs: i64,
    pub per_minute: u32,
    pub poll_max_pages: usize,
    pub backfill_max_pages: usize,
    pub backfill_detail_max: usize,
    pub tickers_refresh_secs: i64,
    /// How far back a start looks: the pillar's 24 h freshness plus an hour.
    pub lookback_secs: i64,
    /// Each forward poll re-reads this far behind the previous one (accessions dedupe it).
    pub overlap_secs: i64,
    /// A poll claims coverage only up to this long before its request: dissemination lags
    /// acceptance by seconds to minutes.
    pub settle_secs: i64,
}

impl EdgarConfig {
    /// `Ok(None)` when `EDGAR_USER_AGENT` is unset or blank: the worker stays off.
    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Result<Option<Self>, String> {
        let Some(user_agent) = get("EDGAR_USER_AGENT")
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
        else {
            return Ok(None);
        };
        let num = |k: &str, d: i64, min: i64| -> Result<i64, String> {
            match get(k)
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
            {
                None => Ok(d),
                Some(v) => v
                    .parse::<i64>()
                    .ok()
                    .filter(|n| *n >= min)
                    .ok_or_else(|| format!("{k} must be an integer >= {min}, got {v:?}")),
            }
        };
        Ok(Some(EdgarConfig {
            user_agent,
            base: get("EDGAR_BASE")
                .map(|v| v.trim().trim_end_matches('/').to_string())
                .filter(|v| !v.is_empty())
                .unwrap_or_else(|| "https://www.sec.gov".into()),
            poll_secs: num("EDGAR_POLL_SECS", 60, 10)?,
            idle_poll_secs: num("EDGAR_IDLE_POLL_SECS", 600, 10)?,
            per_minute: num("EDGAR_PER_MINUTE", 60, 1)?.min(300) as u32,
            poll_max_pages: num("EDGAR_POLL_MAX_PAGES", 10, 1)? as usize,
            backfill_max_pages: num("EDGAR_BACKFILL_MAX_PAGES", 60, 1)? as usize,
            backfill_detail_max: num("EDGAR_BACKFILL_DETAIL_MAX", 600, 0)? as usize,
            tickers_refresh_secs: num("EDGAR_TICKERS_REFRESH_HOURS", 24, 1)? * 3600,
            lookback_secs: 25 * 3600,
            overlap_secs: 300,
            settle_secs: 120,
        }))
    }
}

/// The one line `swimport evidence` logs when the worker is off.
pub const OFF_LINE: &str = "evidence: SEC EDGAR filings OFF (EDGAR_USER_AGENT is not set; SEC requires a descriptive User-Agent with a contact). The news pillar reads Benzinga only until it is set.";

// ---------------------------------------------------------------------------------------------
// Ticker map
// ---------------------------------------------------------------------------------------------

/// SEC's ticker as the app's symbol, or `None` when the notation has no known one-to-one form.
///
/// Whole tickers only, never parts ([module docs](self), "Ticker notation").
pub fn app_symbol(sec: &str) -> Option<String> {
    let t = sec.trim().to_ascii_uppercase();
    let plain = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric());
    if plain(&t) {
        return Some(t);
    }
    let (root, suffix) = t.split_once('-')?;
    if !plain(root) || !root.bytes().next()?.is_ascii_alphabetic() {
        return None;
    }
    let letters = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_uppercase());
    match suffix {
        // A share class: BRK-B -> BRK.B.
        s if s.len() == 1 && letters(s) => Some(format!("{root}.{s}")),
        // Units: KCAC-UN -> KCAC.U.
        "UN" => Some(format!("{root}.U")),
        // A preferred series: USB-PA -> USB.PRA; a bare `-P` has no series and is dropped.
        s if s.len() == 2 && s.starts_with('P') && letters(s) => {
            Some(format!("{root}.PR{}", &s[1..]))
        }
        _ => None,
    }
}

/// CIK -> the app's symbols, from `company_tickers.json`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TickerMap {
    by_cik: HashMap<u64, Vec<String>>,
}

impl TickerMap {
    /// `{"0": {"cik_str": 320193, "ticker": "AAPL", "title": "Apple Inc."}, ...}`. A row whose
    /// ticker has no app notation is skipped; a malformed row is skipped; an empty or
    /// unparsable document is an error (a refresh then keeps the previous map).
    pub fn parse(body: &[u8]) -> Result<TickerMap, String> {
        let v: serde_json::Value =
            serde_json::from_slice(body).map_err(|e| format!("company_tickers.json: {e}"))?;
        let rows = v.as_object().ok_or("company_tickers.json: not an object")?;
        let mut by_cik: HashMap<u64, Vec<String>> = HashMap::new();
        for row in rows.values() {
            let cik = row["cik_str"]
                .as_u64()
                .or_else(|| row["cik_str"].as_str().and_then(|s| s.parse().ok()));
            let (Some(cik), Some(t)) = (cik, row["ticker"].as_str()) else {
                continue;
            };
            if let Some(sym) = app_symbol(t) {
                let e = by_cik.entry(cik).or_default();
                if !e.contains(&sym) {
                    e.push(sym);
                }
            }
        }
        if by_cik.is_empty() {
            return Err("company_tickers.json: no usable rows".into());
        }
        for v in by_cik.values_mut() {
            v.sort();
        }
        Ok(TickerMap { by_cik })
    }

    pub fn symbols(&self, cik: u64) -> &[String] {
        self.by_cik.get(&cik).map(Vec::as_slice).unwrap_or(&[])
    }

    pub fn len(&self) -> usize {
        self.by_cik.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_cik.is_empty()
    }
}

// ---------------------------------------------------------------------------------------------
// The current-filings feed
// ---------------------------------------------------------------------------------------------

/// One `<entry>` of the getcurrent Atom feed. A filing appears once per entity role (a filer and
/// each co-registrant); only `(Filer)` entries are taken.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FeedEntry {
    pub accession: String,
    pub cik: u64,
    pub company: String,
    pub form: String,
    pub role: String,
    /// `<updated>` as delivered (RFC 3339 with the ET offset): the acceptance time.
    pub accepted_raw: String,
    pub accepted_ns: i64,
    pub filed: Option<NaiveDate>,
    pub items: Vec<String>,
    pub url: String,
}

fn decode_entities(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let Some(end) = rest.find(';').filter(|&e| e <= 10) else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let ent = &rest[1..end];
        let ch = match ent {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            e if e.starts_with("#x") => u32::from_str_radix(&e[2..], 16)
                .ok()
                .and_then(char::from_u32),
            e if e.starts_with('#') => e[1..].parse::<u32>().ok().and_then(char::from_u32),
            _ => None,
        };
        match ch {
            Some(c) => {
                out.push(c);
                rest = &rest[end + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// The text of the first `<tag ...>...</tag>` in `block`.
fn tag_text<'a>(block: &'a str, tag: &str) -> Option<&'a str> {
    let open = format!("<{tag}");
    let mut from = 0;
    while let Some(i) = block[from..].find(&open) {
        let start = from + i;
        let after = &block[start + open.len()..];
        // `<title` must not match `<titlex`.
        if after.starts_with('>') || after.starts_with(' ') {
            let gt = start + open.len() + after.find('>')?;
            if block[..gt].ends_with('/') {
                return Some("");
            }
            let close = format!("</{tag}>");
            let end = block[gt + 1..].find(&close)? + gt + 1;
            return Some(&block[gt + 1..end]);
        }
        from = start + open.len();
    }
    None
}

/// An attribute of the first `<tag .../>` in `block`.
fn tag_attr<'a>(block: &'a str, tag: &str, attr: &str) -> Option<&'a str> {
    let open = format!("<{tag} ");
    let start = block.find(&open)?;
    let end = start + block[start..].find('>')?;
    let el = &block[start..end];
    let key = format!("{attr}=\"");
    let a = el.find(&key)? + key.len();
    let b = a + el[a..].find('"')?;
    Some(&el[a..b])
}

/// The entries of one feed page, in feed order (newest first). Entries that do not carry an
/// accession, a CIK, a form and a parsable acceptance time are skipped, never guessed.
pub fn parse_current_feed(xml: &str) -> Vec<FeedEntry> {
    let mut out = Vec::new();
    for block in xml.split("<entry>").skip(1) {
        let block = block.split("</entry>").next().unwrap_or(block);
        let title = decode_entities(tag_text(block, "title").unwrap_or("").trim());
        let id = tag_text(block, "id").unwrap_or("");
        let Some(accession) = id
            .split("accession-number=")
            .nth(1)
            .map(|a| a.trim().to_string())
            .filter(|a| a.len() == 20 && a.bytes().filter(|b| *b == b'-').count() == 2)
        else {
            continue;
        };
        let form = tag_attr(block, "category", "term")
            .map(decode_entities)
            .or_else(|| title.split(" - ").next().map(str::to_string))
            .unwrap_or_default();
        // "8-K - Acme Corp (0001234567) (Filer)": company, CIK and role from the right.
        let Some((head, role)) = title.rsplit_once(" (") else {
            continue;
        };
        let role = role.trim_end_matches(')').to_string();
        let Some((head, cik)) = head.rsplit_once(" (") else {
            continue;
        };
        let Ok(cik) = cik.trim_end_matches(')').parse::<u64>() else {
            continue;
        };
        let company = head
            .split_once(" - ")
            .map(|(_, c)| c.trim().to_string())
            .unwrap_or_default();
        let accepted_raw = tag_text(block, "updated").unwrap_or("").trim().to_string();
        let Some(accepted_ns) = DateTime::parse_from_rfc3339(&accepted_raw)
            .ok()
            .and_then(|t| t.with_timezone(&Utc).timestamp_nanos_opt())
        else {
            continue;
        };
        let url = tag_attr(block, "link", "href")
            .map(decode_entities)
            .unwrap_or_default();
        let summary = decode_entities(tag_text(block, "summary").unwrap_or(""));
        let filed = summary
            .split("Filed:")
            .nth(1)
            .map(|s| s.trim_start_matches(|c: char| !c.is_ascii_digit()))
            .and_then(|s| s.get(..10))
            .and_then(|d| NaiveDate::parse_from_str(d, "%Y-%m-%d").ok());
        let mut items: Vec<String> = Vec::new();
        for part in summary.split("Item ").skip(1) {
            let code: String = part
                .chars()
                .take_while(|c| c.is_ascii_digit() || *c == '.')
                .collect();
            let code = code.trim_end_matches('.').to_string();
            if code.contains('.') && !items.contains(&code) {
                items.push(code);
            }
        }
        if form.is_empty() || url.is_empty() {
            continue;
        }
        out.push(FeedEntry {
            accession,
            cik,
            company,
            form,
            role,
            accepted_raw,
            accepted_ns,
            filed,
            items,
            url,
        });
    }
    out
}

/// The feed URL for one page.
pub fn feed_url(base: &str, start: usize) -> String {
    format!(
        "{base}/cgi-bin/browse-edgar?action=getcurrent&type=&company=&dateb=&owner=exclude&start={start}&count={FEED_PAGE}&output=atom"
    )
}

// ---------------------------------------------------------------------------------------------
// The daily index
// ---------------------------------------------------------------------------------------------

/// One row of a daily form index (`form.YYYYMMDD.idx`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IndexRow {
    pub form: String,
    pub company: String,
    pub cik: u64,
    pub filed: NaiveDate,
    pub accession: String,
    pub url: String,
}

/// The filing's index page: `/Archives/edgar/data/<cik>/<accession without dashes>/<accession>-index.htm`.
pub fn index_url(base: &str, cik: u64, accession: &str) -> String {
    format!(
        "{base}/Archives/edgar/data/{cik}/{}/{accession}-index.htm",
        accession.replace('-', "")
    )
}

/// The rows of a daily form index. The columns are fixed-width but company names may be long, so
/// a row is read from the right: file name, date, CIK; the form is the first field and the
/// company what lies between. Header and malformed lines are skipped.
pub fn parse_daily_index(text: &str, base: &str) -> Vec<IndexRow> {
    let mut out = Vec::new();
    for line in text.lines() {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() < 5 {
            continue;
        }
        let n = fields.len();
        let (file, date, cik) = (fields[n - 1], fields[n - 2], fields[n - 3]);
        let (Ok(cik), Ok(filed)) = (
            cik.parse::<u64>(),
            NaiveDate::parse_from_str(date, "%Y%m%d"),
        ) else {
            continue;
        };
        let Some(accession) = file
            .rsplit('/')
            .next()
            .and_then(|f| f.strip_suffix(".txt"))
            .filter(|a| a.len() == 20)
        else {
            continue;
        };
        // The form is the line's first column (it can hold a space: "SC 13D"); the classified
        // forms never do, and those are the only rows kept.
        let form = fields[0].to_string();
        let company = fields[1..n - 3].join(" ");
        out.push(IndexRow {
            form,
            company,
            cik,
            filed,
            accession: accession.to_string(),
            url: index_url(base, cik, accession),
        });
    }
    out
}

/// The daily form index URL for `date`.
pub fn daily_index_url(base: &str, date: NaiveDate) -> String {
    let q = (date.month() - 1) / 3 + 1;
    format!(
        "{base}/Archives/edgar/daily-index/{}/QTR{q}/form.{}.idx",
        date.year(),
        date.format("%Y%m%d")
    )
}

/// SEC holidays the NYSE trades through (Columbus Day, Veterans Day): EDGAR is closed, the
/// exchange calendar says business as usual.
fn sec_only_holiday(d: NaiveDate) -> bool {
    let columbus = d.month() == 10 && d.weekday() == Weekday::Mon && (8..=14).contains(&d.day());
    let veterans = d.month() == 11
        && match d.weekday() {
            Weekday::Mon => d.day() == 12, // the 11th on a Sunday
            Weekday::Fri => d.day() == 10, // the 11th on a Saturday
            Weekday::Sat | Weekday::Sun => false,
            _ => d.day() == 11,
        };
    columbus || veterans
}

/// Whether EDGAR is open on `d`: a weekday that is neither an exchange holiday nor an SEC-only
/// one. (Good Friday is an exchange holiday EDGAR works through: treating it as closed only moves
/// [`earliest_acceptance`] earlier, the conservative direction.)
fn sec_business_day(d: NaiveDate) -> bool {
    market_events::is_trading_day(d) && !sec_only_holiday(d)
}

/// The earliest acceptance a filing dated `filed` can have (UTC ns): 17:30 ET on the previous
/// SEC business day, since EDGAR gives a filing accepted after 17:30 the next business day's date.
/// A daily-index row without its index page is dated by this (module docs, "Point in time").
pub fn earliest_acceptance(filed: NaiveDate) -> i64 {
    let mut d = filed - Days::days(1);
    while !sec_business_day(d) {
        d -= Days::days(1);
    }
    New_York
        .from_local_datetime(&d.and_hms_opt(17, 30, 0).expect("valid time"))
        .earliest()
        .and_then(|t| t.with_timezone(&Utc).timestamp_nanos_opt())
        .expect("a representable time")
}

/// The `Accepted` time on a filing's index page (`2026-11-20 16:05:12`, ET), in UTC ns.
pub fn parse_accepted(html: &str) -> Option<(String, i64)> {
    let i = html.find(">Accepted<")?;
    let rest = &html[i..];
    let open = rest.find("class=\"info\">")? + "class=\"info\">".len();
    let close = open + rest[open..].find('<')?;
    let raw = rest[open..close].trim().to_string();
    let local = NaiveDateTime::parse_from_str(&raw, "%Y-%m-%d %H:%M:%S").ok()?;
    let ns = New_York
        .from_local_datetime(&local)
        .earliest()?
        .with_timezone(&Utc)
        .timestamp_nanos_opt()?;
    Some((raw, ns))
}

// ---------------------------------------------------------------------------------------------
// The collector
// ---------------------------------------------------------------------------------------------

/// Whether EDGAR is in filing hours (06:00-22:00 ET on a business day).
pub fn filing_hours(now: DateTime<Utc>) -> bool {
    let et = now.with_timezone(&New_York);
    sec_business_day(et.date_naive()) && (6..22).contains(&et.hour())
}

/// What one EDGAR tick did (one log line).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct EdgarReport {
    pub requests: u32,
    pub pages: u32,
    pub filings: u32,
    pub from_index: u32,
    pub tickers: usize,
    /// The poll read back to where the previous one ended (or the back-fill window's start).
    pub complete: bool,
    pub polled: bool,
    pub errors: Vec<String>,
}

impl EdgarReport {
    pub fn summary(&self) -> String {
        format!(
            "edgar: {} requests ({} feed pages), {} new filings ({} from the daily index){}{}",
            self.requests,
            self.pages,
            self.filings,
            self.from_index,
            if self.polled && !self.complete {
                ", INCOMPLETE (did not reach the previous poll)"
            } else {
                ""
            },
            if self.errors.is_empty() {
                String::new()
            } else {
                format!("; errors: {}", self.errors.join(" | "))
            }
        )
    }
}

/// The EDGAR worker's state. Everything it needs after a restart comes from the stored records:
/// the accessions already kept (never written twice) and the last poll's covered-through point.
pub struct EdgarCollector {
    pub cfg: EdgarConfig,
    seen: HashMap<String, i64>,
    last_through: Option<i64>,
    tickers: Option<TickerMap>,
    tickers_at: Option<i64>,
    next_poll: i64,
    backfilled: bool,
    pacer: Pacer,
}

impl EdgarCollector {
    pub fn open(cfg: EdgarConfig, archive: &Archive, now: DateTime<Utc>) -> Result<Self, String> {
        let since = (now - Days::days(3)).timestamp_nanos_opt();
        let filings = archive.read_filings(since).map_err(|e| e.to_string())?;
        let cov = archive
            .read_filings_coverage(since)
            .map_err(|e| e.to_string())?;
        let seen = filings
            .iter()
            .map(|r| (r.accession.clone(), r.first_observed_at))
            .collect();
        Ok(EdgarCollector {
            pacer: Pacer::per_minute(cfg.per_minute),
            cfg,
            seen,
            last_through: cov.iter().map(|c| c.covered_through).max(),
            tickers: None,
            tickers_at: None,
            next_poll: i64::MIN,
            backfilled: false,
        })
    }

    /// The covered-through point (tests, the log line).
    pub fn last_through(&self) -> Option<i64> {
        self.last_through
    }

    fn get(
        &mut self,
        http: &mut dyn Http,
        url: &str,
        sleep: &mut dyn FnMut(Duration),
        log: &mut dyn FnMut(&str),
        report: &mut EdgarReport,
    ) -> Result<Vec<u8>, FetchError> {
        let ua = self.cfg.user_agent.clone();
        let headers = [("User-Agent", ua.as_str()), ("Accept", "*/*")];
        let pacer = &mut self.pacer;
        let mut paced = PacedEdgar {
            inner: http,
            pacer,
            sleep: &mut *sleep,
            count: &mut report.requests,
        };
        get_with_retry(
            &mut paced,
            url,
            &headers,
            RetryPolicy::default(),
            &[],
            &mut |_| {},
            log,
        )
        .map(|r| r.body)
    }

    /// One pass: refresh the ticker map when due, then a feed poll when due.
    pub fn tick(
        &mut self,
        archive: &Archive,
        http: &mut dyn Http,
        clock: &mut dyn FnMut() -> DateTime<Utc>,
        sleep: &mut dyn FnMut(Duration),
        log: &mut dyn FnMut(&str),
    ) -> EdgarReport {
        let mut report = EdgarReport::default();
        let now = clock();
        let now_ns = now.timestamp_nanos_opt().unwrap_or(i64::MAX);
        if self
            .tickers_at
            .is_none_or(|t| now_ns - t >= self.cfg.tickers_refresh_secs * SEC)
        {
            // A failed refresh keeps the previous map and is retried at the next poll.
            self.tickers_at = Some(now_ns);
            let url = format!("{}/files/company_tickers.json", self.cfg.base);
            match self
                .get(http, &url, sleep, log, &mut report)
                .map_err(|e| e.to_string())
                .and_then(|b| TickerMap::parse(&b))
            {
                Ok(m) => self.tickers = Some(m),
                Err(e) => {
                    report.errors.push(format!("tickers: {e}"));
                    if self.tickers.is_some() {
                        // Try again in an hour rather than tomorrow.
                        self.tickers_at =
                            Some(now_ns - self.cfg.tickers_refresh_secs * SEC + 3600 * SEC);
                    } else {
                        // No map yet: try again in five minutes, not every tick.
                        self.tickers_at =
                            Some(now_ns - self.cfg.tickers_refresh_secs * SEC + 300 * SEC);
                    }
                }
            }
        }
        report.tickers = self.tickers.as_ref().map_or(0, TickerMap::len);
        // Without a map nothing could be attributed to a symbol: no poll, no coverage claim.
        if self.tickers.is_none() || now_ns < self.next_poll {
            return report;
        }
        let interval = if filing_hours(now) {
            self.cfg.poll_secs
        } else {
            self.cfg.idle_poll_secs
        };
        self.next_poll = now_ns + interval * SEC;
        report.polled = true;
        self.poll(archive, http, now_ns, clock, sleep, log, &mut report);
        report
    }

    #[allow(clippy::too_many_arguments)]
    fn poll(
        &mut self,
        archive: &Archive,
        http: &mut dyn Http,
        now_ns: i64,
        clock: &mut dyn FnMut() -> DateTime<Utc>,
        sleep: &mut dyn FnMut(Duration),
        log: &mut dyn FnMut(&str),
        report: &mut EdgarReport,
    ) {
        let floor = now_ns - self.cfg.lookback_secs * SEC;
        let from = self
            .last_through
            .map_or(floor, |t| (t - self.cfg.overlap_secs * SEC).max(floor));
        let max_pages = if self.backfilled {
            self.cfg.poll_max_pages
        } else {
            self.cfg.backfill_max_pages
        };
        // Pages newest first, until one reaches `from` or the feed ends.
        let mut entries: Vec<FeedEntry> = Vec::new();
        let mut reached = false;
        let mut failed = false;
        let mut oldest: Option<i64> = None;
        for page in 0..max_pages {
            let url = feed_url(&self.cfg.base, page * FEED_PAGE);
            let body = match self.get(http, &url, sleep, log, report) {
                Ok(b) => b,
                Err(e) => {
                    report.errors.push(format!("feed page {page}: {e}"));
                    failed = true;
                    break;
                }
            };
            report.pages += 1;
            let text = String::from_utf8_lossy(&body);
            // A 200 that is not the feed (a maintenance or rate-limit page) must not read as "no
            // more filings": that would claim coverage over a window nothing was read for.
            if !text.contains("<feed") {
                report.errors.push(format!(
                    "feed page {page}: the response is not an Atom feed"
                ));
                failed = true;
                break;
            }
            let got = parse_current_feed(&text);
            let n = got.len();
            if let Some(o) = got.iter().map(|e| e.accepted_ns).min() {
                oldest = Some(oldest.map_or(o, |x: i64| x.min(o)));
            }
            entries.extend(got);
            if oldest.is_some_and(|o| o <= from) || n < FEED_PAGE {
                reached = true;
                break;
            }
        }
        let observed = clock().timestamp_nanos_opt().unwrap_or(now_ns);
        let rows = self.feed_records(&entries, observed);
        // The rest of the window the feed did not reach, once per start: the daily indexes and
        // each relevant filing's index page.
        let mut index_rows = Vec::new();
        if !self.backfilled && !reached && !failed {
            let to = oldest.unwrap_or(now_ns);
            index_rows = self.backfill(http, from, to, &rows, clock, sleep, log, report);
        }
        // A start whose first poll failed outright still owes its back-fill to the next one.
        if !failed {
            self.backfilled = true;
        }
        let mut all = rows;
        all.extend(index_rows);
        // Filings first, then the coverage they complete: a crash between leaves records without
        // a claim, never a claim without its records. Nothing is believed stored until it is.
        if let Err(e) = archive.append_filings(&all) {
            report.errors.push(format!("filings store: {e}"));
            return;
        }
        for r in &all {
            self.seen.insert(r.accession.clone(), r.first_observed_at);
            if r.source == "daily_index" {
                report.from_index += 1;
            }
        }
        report.filings += all.len() as u32;
        // Forget accessions no window can reach again (bounded memory over weeks).
        let keep_from = now_ns - 3 * 86_400 * SEC;
        self.seen.retain(|_, at| *at >= keep_from);
        if failed {
            return;
        }
        // What the feed read is contiguous from its oldest entry (or `from`) to the request.
        let covered_from = if reached {
            from
        } else {
            oldest.unwrap_or(now_ns)
        };
        let covered_through = now_ns - self.cfg.settle_secs * SEC;
        report.complete = reached;
        if covered_through <= covered_from {
            return;
        }
        let c = NewsCoverageRecord {
            v: market_archive::evidence::EVIDENCE_SCHEMA,
            id: format!("filingcov:{PROVIDER}:{covered_from}-{covered_through}@{observed}"),
            provider: PROVIDER.into(),
            sources: vec!["current_feed".into()],
            scope: CoverageScope::All,
            covered_from,
            covered_through,
            observed_at: observed,
            requests: report.requests,
            stories: entries.len() as u32,
        };
        match archive.append_filings_coverage(std::slice::from_ref(&c)) {
            Ok(()) => {
                self.last_through = Some(
                    self.last_through
                        .map_or(covered_through, |t| t.max(covered_through)),
                );
            }
            Err(e) => report.errors.push(format!("filings coverage store: {e}")),
        }
    }

    /// New records from feed entries: classified forms with a mapped ticker, one per accession
    /// (a co-registrant's entry adds its tickers to the filer's record), not already kept.
    fn feed_records(&self, entries: &[FeedEntry], observed: i64) -> Vec<FilingRecord> {
        let map = self.tickers.as_ref().expect("polled with a map");
        let mut by_acc: BTreeMap<String, FilingRecord> = BTreeMap::new();
        for e in entries {
            if e.role != "Filer" || self.seen.contains_key(&e.accession) {
                continue;
            }
            if filing_class(&e.form).is_none() {
                continue;
            }
            let syms = map.symbols(e.cik);
            if syms.is_empty() {
                continue;
            }
            let r = by_acc
                .entry(e.accession.clone())
                .or_insert_with(|| FilingRecord {
                    v: market_archive::evidence::EVIDENCE_SCHEMA,
                    provider: PROVIDER.into(),
                    source: "current_feed".into(),
                    accession: e.accession.clone(),
                    cik: e.cik,
                    company: e.company.clone(),
                    form: e.form.clone(),
                    accepted_at: Some(e.accepted_raw.clone()),
                    accepted_at_ns: e.accepted_ns,
                    dated_by: DATED_BY_ACCEPTANCE.into(),
                    filed: e.filed,
                    items: e.items.clone(),
                    symbols: Vec::new(),
                    url: e.url.clone(),
                    first_observed_at: observed,
                });
            for s in syms {
                if !r.symbols.contains(s) {
                    r.symbols.push(s.clone());
                }
            }
        }
        by_acc.into_values().collect()
    }

    /// Daily indexes for the ET dates in `[from, to)` before today, each relevant filing's
    /// acceptance read from its index page while the cap lasts, else dated by
    /// [`earliest_acceptance`].
    #[allow(clippy::too_many_arguments)]
    fn backfill(
        &mut self,
        http: &mut dyn Http,
        from: i64,
        to: i64,
        already: &[FilingRecord],
        clock: &mut dyn FnMut() -> DateTime<Utc>,
        sleep: &mut dyn FnMut(Duration),
        log: &mut dyn FnMut(&str),
        report: &mut EdgarReport,
    ) -> Vec<FilingRecord> {
        let et = |ns: i64| {
            DateTime::from_timestamp_nanos(ns)
                .with_timezone(&New_York)
                .date_naive()
        };
        let today = et(clock().timestamp_nanos_opt().unwrap_or(to));
        let mut d = et(from);
        let last = et(to).min(today - Days::days(1));
        let mut out: Vec<FilingRecord> = Vec::new();
        let mut details = self.cfg.backfill_detail_max;
        let base = self.cfg.base.clone();
        while d <= last {
            if !sec_business_day(d) {
                d += Days::days(1);
                continue;
            }
            let url = daily_index_url(&base, d);
            let body = match self.get(http, &url, sleep, log, report) {
                Ok(b) => b,
                // Not published (yet) or no index for the date: nothing to add.
                Err(FetchError::Permanent {
                    status: 404 | 403, ..
                }) => {
                    d += Days::days(1);
                    continue;
                }
                Err(e) => {
                    report.errors.push(format!("daily index {d}: {e}"));
                    d += Days::days(1);
                    continue;
                }
            };
            let rows = parse_daily_index(&String::from_utf8_lossy(&body), &base);
            for row in rows {
                if filing_class(&row.form).is_none()
                    || self.seen.contains_key(&row.accession)
                    || already.iter().any(|r| r.accession == row.accession)
                    || out.iter().any(|r| r.accession == row.accession)
                {
                    continue;
                }
                let syms = self
                    .tickers
                    .as_ref()
                    .map(|m| m.symbols(row.cik).to_vec())
                    .unwrap_or_default();
                if syms.is_empty() {
                    continue;
                }
                let earliest = earliest_acceptance(row.filed);
                // Only a filing that could still be fresh is worth a request for its time: one
                // dated `filed` was accepted by 22:00 ET that day at the latest. Amendments never
                // pass, so their time changes nothing either.
                let latest = New_York
                    .from_local_datetime(&row.filed.and_hms_opt(22, 0, 0).expect("valid time"))
                    .latest()
                    .and_then(|t| t.with_timezone(&Utc).timestamp_nanos_opt())
                    .unwrap_or(i64::MAX);
                let now_ns = clock().timestamp_nanos_opt().unwrap_or(to);
                let fresh_possible = latest.saturating_add(self.cfg.lookback_secs * SEC) >= now_ns
                    && filing_class(&row.form) != Some(FilingClass::Amendment);
                let accepted = if details > 0 && fresh_possible {
                    details -= 1;
                    self.get(http, &row.url, sleep, log, report)
                        .ok()
                        .and_then(|b| parse_accepted(&String::from_utf8_lossy(&b)))
                } else {
                    None
                };
                let observed = clock().timestamp_nanos_opt().unwrap_or(to);
                let (accepted_at, accepted_at_ns, dated_by) = match accepted {
                    Some((raw, ns)) => (
                        Some(format!("{raw} America/New_York")),
                        ns,
                        DATED_BY_ACCEPTANCE,
                    ),
                    None => (None, earliest, DATED_BY_DAILY_INDEX),
                };
                out.push(FilingRecord {
                    v: market_archive::evidence::EVIDENCE_SCHEMA,
                    provider: PROVIDER.into(),
                    source: "daily_index".into(),
                    accession: row.accession.clone(),
                    cik: row.cik,
                    company: row.company.clone(),
                    form: row.form.clone(),
                    accepted_at,
                    accepted_at_ns,
                    dated_by: dated_by.into(),
                    filed: Some(row.filed),
                    items: Vec::new(),
                    symbols: syms,
                    url: row.url.clone(),
                    first_observed_at: observed,
                });
            }
            d += Days::days(1);
        }
        out
    }
}

/// The pacer in front of every EDGAR request, counting them.
struct PacedEdgar<'a> {
    inner: &'a mut dyn Http,
    pacer: &'a mut Pacer,
    sleep: &'a mut dyn FnMut(Duration),
    count: &'a mut u32,
}

impl Http for PacedEdgar<'_> {
    fn get(
        &mut self,
        url: &str,
        headers: &[(&str, &str)],
    ) -> Result<super::http::HttpResponse, String> {
        self.pacer.wait(&mut *self.sleep);
        *self.count += 1;
        self.inner.get(url, headers)
    }
}

//! FMP market-wide stock news as a Ross Panel news source, **record-only** until switched on
//! (owner, 2026-11-23: "Add FMP news, recording for one day before it counts").
//!
//! `swimport evidence`'s fourth worker, beside the Benzinga news, FMP float and SEC EDGAR halves.
//!
//! # Why
//!
//! The news pillar's FAILs are mostly "no story found" on small caps whose catalyst went out on a
//! press wire Benzinga did not carry. FMP's `stock-latest` feed is market-wide, tagged by symbol,
//! carries the wires (PR Newswire, GlobeNewswire, Business Wire, ...) and is on the plan the
//! evidence container already holds a key for. The wire scrapers (`stockwatcher_server::wires`)
//! are off; this reads the same releases through one keyed endpoint.
//!
//! # Record-only first
//!
//! Every mapped item is stored as a `market_archive::SourceNewsRecord` (provider `press_wire`, so
//! pillars-v6 can read it with no policy bump), but while `FMP_NEWS_COUNT` is unset or `0` its
//! `kind` is **`recorded`**, which can never pass the pillar (`stock_context::SourceKind`). The
//! class it *would* have had is kept in `event_type` as `shadow_<class>`, so the comparison the
//! owner asked for can be run from the archive alone: every item, its publication time, our first
//! sighting, and what it would have done. The server's evidence loader skips these records
//! entirely ([`is_shadow`], `stockwatcher_server::evidence`): a never-passing item would still
//! schedule a re-evaluation of its symbol when it arrives and so could move *when* a decision is
//! logged. The News pending cards also skip FMP items that do not count
//! (`stockwatcher_server::pending::hit_from_item`: noise, third-party, stale, once counting).
//! Record-only therefore changes nothing a decision or a user sees.
//!
//! `FMP_NEWS_COUNT=1` makes **new** items carry their class as their kind. Items already stored
//! keep the kind they were stored with (the archive is append-only and the collector never writes
//! an id twice), so the switch takes effect from the first poll after the restart.
//!
//! # Classes ([`classify`])
//!
//! | Class | When | Kind when counting | `event_type` |
//! |---|---|---|---|
//! | noise | a law-firm solicitation ("Shareholder Alert", "Sued for Securities Law Violations", "class action") | `recorded` | `noise` |
//! | third_party | the publisher is not a press wire (an analyst site, a news aggregator): not the issuer's own news | `recorded` | `third_party` |
//! | offering | a wire release whose headline announces a financing ([`OFFERING_PHRASES`]) | `offering` | none (the seam's `offering_filed`) |
//! | catalyst | any other wire release | `catalyst` | none |
//!
//! An item first seen more than 24 h after its publication is `recorded` with `stale_<class>`
//! when counting: the pillar's freshness is 24 h, and a back-fill must never look like news.
//! Record-only, every item is `shadow_<class>` (the audit reads staleness from the two times).
//!
//! The phrase lists mirror `stockwatcher_server::wires::classify` (tested there on the wires'
//! own samples), plus the law-firm forms the FMP feed carries ("... Sued for Securities Law
//! Violations - Contact ..."). Over-matching noise or offering only withholds a pass.
//!
//! # What it reads (and how often)
//!
//! `GET {FMP_BASE}/stable/news/stock-latest?page=N&limit=FMP_NEWS_LIMIT&apikey=...`, newest
//! first. Every `FMP_NEWS_POLL_SECS` (60) from 04:00 to 20:00 ET on weekdays, every
//! `FMP_NEWS_IDLE_POLL_SECS` (600) otherwise. A poll reads page 0 and keeps paging only while
//! the page was full and **every** item on it was new (not stored before and not older than the
//! newest item the previous poll saw), up to `FMP_NEWS_MAX_PAGES` (3): [`next_page_wanted`].
//! One request in flight, every request through one pacer (`FMP_NEWS_PER_MINUTE`, 20) with
//! `Retry-After` honoured on 429; a failed poll backs the next one off (doubling, at most 15
//! min). A normal minute is **one** request: ~1,000 a day, far inside any FMP plan.
//! (`/stable/news/press-releases*` is restricted on the plan and is not used.)
//!
//! # Point in time
//!
//! - `publishedDate` (`2026-11-23 00:26:00`) is **US Eastern wall time** (verified live on
//!   2026-11-23: an item stamped 00:26 was seven minutes old at 05:33 UTC). [`parse_published`]
//!   converts it with `America/New_York`; a value carrying its own offset or `Z` is read as such.
//!   In the repeated autumn hour the earlier instant is taken (the 24 h freshness then ends no
//!   later than the real one); a time inside the spring-forward gap is read an hour later.
//! - `first_observed_at` is our fetch time; the pillar never joins an item before it.
//!
//! # Symbols
//!
//! FMP's `symbol` is taken whole and strictly ([`super::edgar::app_symbol`]: `BRK-B` becomes
//! `BRK.B`, anything without a known one-to-one notation is dropped, never split) and kept only
//! when it is in the tradable universe: every symbol in the last `FMP_NEWS_UNIVERSE_DAYS` (5)
//! dates of `daily-closes.json`, plus today's recorded live quotes. Everything else is dropped
//! and counted by reason in the summary line.

use super::edgar::app_symbol;
use super::evidence_collect::Pacer;
use super::fmp::FmpKey;
use super::http::{FetchError, Http, HttpResponse, RetryPolicy, get_with_retry, redact_url};
use chrono::{DateTime, Datelike, NaiveDateTime, TimeZone, Timelike, Utc, Weekday};
use chrono_tz::America::New_York;
use market_archive::{Archive, SourceNewsRecord, sha256_hex};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::time::Duration;

const SEC: i64 = 1_000_000_000;
/// The pillar policy's provider name for wire releases (pillars-v6 `news.sources`).
pub const PROVIDER: &str = "press_wire";
/// `SourceNewsRecord::rules` of every record this collector writes.
pub const RULES: &str = "fmp-news-v1";
/// The `event_type` prefix of a record-only item: the class it would have had.
pub const SHADOW_PREFIX: &str = "shadow_";
/// The pillar's news freshness: an item first seen later than this after publication is
/// recorded only.
pub const STALE_AFTER_SECS: i64 = 24 * 3600;

/// Environment of the FMP news worker.
///
/// | Variable | Default | Meaning |
/// |---|---|---|
/// | `FMP_NEWS` | on when `FMP_API_KEY` is set | `0` turns the worker off |
/// | `FMP_NEWS_COUNT` | `0` (record-only) | `1`: new items carry their class as their kind, so catalysts can pass the news pillar |
/// | `FMP_BASE` | `https://financialmodelingprep.com` | the host (shared with the other FMP calls) |
/// | `FMP_NEWS_POLL_SECS` | `60` | poll interval, 04:00-20:00 ET on weekdays |
/// | `FMP_NEWS_IDLE_POLL_SECS` | `600` | poll interval otherwise |
/// | `FMP_NEWS_LIMIT` | `100` | items per page (`limit=`) |
/// | `FMP_NEWS_MAX_PAGES` | `3` | pages one poll may read |
/// | `FMP_NEWS_PER_MINUTE` | `20` | request ceiling for this worker, retries included |
/// | `FMP_NEWS_UNIVERSE_DAYS` | `5` | dates of `daily-closes.json` that define the tradable universe |
/// | `FMP_NEWS_DEBUG` | `0` | `1`: one log line per poll (the summary line comes every 10 min regardless) |
#[derive(Clone)]
pub struct FmpNewsConfig {
    pub key: FmpKey,
    pub base: String,
    pub count: bool,
    pub poll_secs: i64,
    pub idle_poll_secs: i64,
    pub limit: usize,
    pub max_pages: usize,
    pub per_minute: u32,
    pub universe_days: usize,
    pub debug: bool,
    /// How often the summary line is due.
    pub summary_secs: i64,
}

/// Whether the worker runs, and why not.
pub enum FmpNewsSwitch {
    On(FmpNewsConfig),
    Off(&'static str),
}

/// The start line when the worker is off because `FMP_NEWS=0`.
pub const OFF_SWITCHED: &str = "evidence: FMP news OFF (FMP_NEWS=0)";
/// The start line when the worker is off because there is no key.
pub const OFF_NO_KEY: &str = "evidence: FMP news OFF (FMP_API_KEY is not set)";

impl FmpNewsConfig {
    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Result<FmpNewsSwitch, String> {
        let val = |k: &str| {
            get(k)
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
        };
        let switch = |k: &str, d: bool| -> Result<bool, String> {
            match val(k).as_deref() {
                None => Ok(d),
                Some("1" | "true" | "on" | "yes") => Ok(true),
                Some("0" | "false" | "off" | "no") => Ok(false),
                Some(v) => Err(format!("{k} must be 0 or 1, got {v:?}")),
            }
        };
        let num = |k: &str, d: i64, min: i64| -> Result<i64, String> {
            match val(k) {
                None => Ok(d),
                Some(v) => v
                    .parse::<i64>()
                    .ok()
                    .filter(|n| *n >= min)
                    .ok_or_else(|| format!("{k} must be an integer >= {min}, got {v:?}")),
            }
        };
        if !switch("FMP_NEWS", true)? {
            return Ok(FmpNewsSwitch::Off(OFF_SWITCHED));
        }
        let Some(key) = val("FMP_API_KEY") else {
            return Ok(FmpNewsSwitch::Off(OFF_NO_KEY));
        };
        Ok(FmpNewsSwitch::On(FmpNewsConfig {
            key: FmpKey(key),
            base: val("FMP_BASE")
                .map(|v| v.trim_end_matches('/').to_string())
                .unwrap_or_else(|| "https://financialmodelingprep.com".into()),
            count: switch("FMP_NEWS_COUNT", false)?,
            poll_secs: num("FMP_NEWS_POLL_SECS", 60, 15)?,
            idle_poll_secs: num("FMP_NEWS_IDLE_POLL_SECS", 600, 15)?,
            limit: num("FMP_NEWS_LIMIT", 100, 1)?.min(250) as usize,
            max_pages: num("FMP_NEWS_MAX_PAGES", 3, 1)?.min(20) as usize,
            per_minute: num("FMP_NEWS_PER_MINUTE", 20, 1)?.min(300) as u32,
            universe_days: num("FMP_NEWS_UNIVERSE_DAYS", 5, 1)? as usize,
            debug: switch("FMP_NEWS_DEBUG", false)?,
            summary_secs: 600,
        }))
    }

    /// The one start line when the worker is on.
    pub fn on_line(&self) -> String {
        format!(
            "evidence: FMP news ON ({}): every {}s ({}s outside 04:00-20:00 ET weekdays), limit {}, <= {} pages, <= {}/min",
            if self.count {
                "COUNTING: FMP_NEWS_COUNT=1, catalysts can pass the news pillar"
            } else {
                "record-only; FMP_NEWS_COUNT=1 makes it count"
            },
            self.poll_secs,
            self.idle_poll_secs,
            self.limit,
            self.max_pages,
            self.per_minute
        )
    }
}

// ---------------------------------------------------------------------------------------------
// The feed
// ---------------------------------------------------------------------------------------------

/// One usable feed item.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FeedItem {
    /// FMP's symbol(s) as delivered (a comma list is split into whole tickers).
    pub symbols: Vec<String>,
    pub published_raw: String,
    pub published_at: i64,
    pub publisher: String,
    pub site: String,
    pub title: String,
    pub text: String,
    pub url: String,
}

/// A page: the usable items in feed order, plus how many were skipped and why.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FeedPage {
    pub items: Vec<FeedItem>,
    /// Items on the page, usable or not.
    pub raw: usize,
    pub skipped: BTreeMap<&'static str, u64>,
}

fn str_field<'a>(o: &'a serde_json::Map<String, serde_json::Value>, k: &str) -> Option<&'a str> {
    o.get(k)
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

/// `publishedDate` in UTC ns. A bare wall time is US Eastern (module docs, "Point in time");
/// a value with an offset or `Z` is read as such. `None` when it cannot be read.
pub fn parse_published(raw: &str) -> Option<i64> {
    let raw = raw.trim();
    if let Ok(t) = DateTime::parse_from_rfc3339(raw) {
        return t.with_timezone(&Utc).timestamp_nanos_opt();
    }
    let local = ["%Y-%m-%d %H:%M:%S", "%Y-%m-%dT%H:%M:%S", "%Y-%m-%d %H:%M"]
        .iter()
        .find_map(|f| NaiveDateTime::parse_from_str(raw, f).ok())
        .or_else(|| {
            // Fractional seconds without an offset.
            NaiveDateTime::parse_from_str(raw, "%Y-%m-%d %H:%M:%S%.f")
                .or_else(|_| NaiveDateTime::parse_from_str(raw, "%Y-%m-%dT%H:%M:%S%.f"))
                .ok()
        })?;
    let t = New_York
        .from_local_datetime(&local)
        .earliest()
        .or_else(|| {
            // The spring-forward gap (02:00-03:00 does not exist): the wall clock read an hour on.
            New_York
                .from_local_datetime(&(local + chrono::Duration::hours(1)))
                .earliest()
        })?;
    t.with_timezone(&Utc).timestamp_nanos_opt()
}

/// One page of `stock-latest`. A page that is not a JSON array (FMP's `{"Error Message": ...}`)
/// is an error; an element without a symbol, a title or a readable `publishedDate` is skipped
/// and counted, never guessed. Unknown fields are ignored.
pub fn parse_page(body: &[u8]) -> Result<FeedPage, String> {
    let v: serde_json::Value =
        serde_json::from_slice(body).map_err(|e| format!("not JSON: {e}"))?;
    let rows = match v {
        serde_json::Value::Array(rows) => rows,
        serde_json::Value::Object(o) => {
            let msg = o
                .get("Error Message")
                .or_else(|| o.get("error"))
                .or_else(|| o.get("message"))
                .and_then(|m| m.as_str())
                .unwrap_or("an object, not a list");
            return Err(format!("FMP answered: {}", truncate(msg, 200)));
        }
        _ => return Err("not a list".into()),
    };
    let mut page = FeedPage {
        raw: rows.len(),
        ..FeedPage::default()
    };
    let mut skip = |why: &'static str| *page.skipped.entry(why).or_default() += 1;
    let mut items = Vec::new();
    for row in &rows {
        let Some(o) = row.as_object() else {
            skip("not_an_object");
            continue;
        };
        let Some(sym) = str_field(o, "symbol") else {
            skip("missing_symbol");
            continue;
        };
        let Some(title) = str_field(o, "title") else {
            skip("missing_title");
            continue;
        };
        let Some(raw) = str_field(o, "publishedDate") else {
            skip("missing_date");
            continue;
        };
        let Some(published_at) = parse_published(raw) else {
            skip("bad_date");
            continue;
        };
        let symbols: Vec<String> = sym
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect();
        items.push(FeedItem {
            symbols,
            published_raw: raw.to_string(),
            published_at,
            publisher: str_field(o, "publisher").unwrap_or("").to_string(),
            site: str_field(o, "site").unwrap_or("").to_string(),
            title: title.to_string(),
            text: str_field(o, "text").unwrap_or("").to_string(),
            url: str_field(o, "url").unwrap_or("").to_string(),
        });
    }
    page.items = items;
    Ok(page)
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() > n {
        format!("{}…", s.chars().take(n).collect::<String>())
    } else {
        s.to_string()
    }
}

/// The request path and query **without** the key (what reaches logs).
pub fn endpoint(page: usize, limit: usize) -> String {
    format!("/stable/news/stock-latest?page={page}&limit={limit}")
}

/// Whether a poll should read the page after this one: the page was full, every item on it was
/// new (`known[i]`: stored before, or not newer than the previous poll's newest), and the cap
/// allows another page.
pub fn next_page_wanted(known: &[bool], limit: usize, pages_read: usize, max_pages: usize) -> bool {
    pages_read < max_pages && known.len() >= limit && !known.is_empty() && !known.iter().any(|k| *k)
}

// ---------------------------------------------------------------------------------------------
// Classification
// ---------------------------------------------------------------------------------------------

/// What an item would be to the news pillar ([module docs](self), "Classes").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FmpClass {
    Catalyst,
    Offering,
    Noise,
    ThirdParty,
}

impl FmpClass {
    pub fn as_str(self) -> &'static str {
        match self {
            FmpClass::Catalyst => "catalyst",
            FmpClass::Offering => "offering",
            FmpClass::Noise => "noise",
            FmpClass::ThirdParty => "third_party",
        }
    }
}

/// Headline phrases of a financing (mirrors `stockwatcher_server::wires::classify::OFFERING_PHRASES`).
pub const OFFERING_PHRASES: &[&str] = &[
    "public offering",
    "registered direct",
    "direct offering",
    "private placement",
    "at-the-market",
    "at the market offering",
    "atm offering",
    "atm program",
    "underwritten offering",
    "best efforts offering",
    "best-efforts offering",
    "proposed offering",
    "warrant inducement",
    "inducement of warrant",
    "exercise of warrants",
    "warrant exercise",
    "announces pricing",
    "pricing of",
    "prices offering",
    "equity line",
    "share subscription facility",
    "securities purchase agreement",
    "shelf registration",
];

/// Headline phrases of a law-firm solicitation (the wires module's list, plus the FMP feed's
/// "Sued for Securities Law Violations" form).
pub const NOISE_HEADLINE: &[&str] = &[
    "shareholder alert",
    "investor alert",
    "shareholder rights",
    "class action",
    "securities fraud",
    "investors who lost",
    "lead plaintiff",
    "deadline alert",
    "reminds investors",
    "encourages investors",
    "investigation on behalf",
    "announces investigation",
    "investigation of",
    "lawsuit filed",
    "sued for securities",
    "securities law violation",
    "investors to contact",
    "law firm",
];

/// The same in the body: phrases a solicitation uses and an issuer's own release does not.
pub const NOISE_TEXT: &[&str] = &[
    "law firm",
    "securities class action",
    "class action lawsuit",
    "on behalf of investors",
    "on behalf of shareholders",
];

/// Publishers that are press wires: what an issuer itself puts out. Compared on the
/// [`source_slug`] of the publisher (or of the site when the publisher is blank).
pub const WIRE_PUBLISHERS: &[&str] = &[
    "prnewswire",
    "globenewswire",
    "businesswire",
    "accesswire",
    "accessnewswire",
    "newsfile",
    "newsfilecorp",
];

/// Lower-case alphanumerics only: `PRNewsWire` -> `prnewswire`, `Business Wire` -> `businesswire`.
pub fn source_slug(s: &str) -> String {
    s.chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

fn is_wire(publisher: &str, site: &str) -> bool {
    let p = source_slug(publisher);
    let s = source_slug(site.split('.').next().unwrap_or(""));
    WIRE_PUBLISHERS
        .iter()
        .any(|w| p == *w || (p.is_empty() && s == *w))
}

/// The item's class: noise first (a solicitation can mention an offering), then third-party
/// publishers, then offering phrases on the headline only (a body can mention an "offering" of
/// products), else a catalyst.
pub fn classify(title: &str, text: &str, publisher: &str, site: &str) -> FmpClass {
    let (h, t) = (title.to_lowercase(), text.to_lowercase());
    if NOISE_HEADLINE.iter().any(|p| h.contains(p)) || NOISE_TEXT.iter().any(|p| t.contains(p)) {
        return FmpClass::Noise;
    }
    if !is_wire(publisher, site) {
        return FmpClass::ThirdParty;
    }
    if OFFERING_PHRASES.iter().any(|p| h.contains(p)) {
        FmpClass::Offering
    } else {
        FmpClass::Catalyst
    }
}

/// Whether a stored record is record-only FMP news (`shadow_*`): archived for the audit, never
/// handed to the engine (`stockwatcher_server::evidence` skips it).
pub fn is_shadow(r: &SourceNewsRecord) -> bool {
    r.rules == RULES
        && r.kind == "recorded"
        && r.event_type
            .as_deref()
            .is_some_and(|e| e.starts_with(SHADOW_PREFIX))
}

/// `(kind, event_type)` for a record ([module docs](self), "Classes").
pub fn kind_of(class: FmpClass, count: bool, stale: bool) -> (&'static str, Option<String>) {
    if !count {
        return (
            "recorded",
            Some(format!("{SHADOW_PREFIX}{}", class.as_str())),
        );
    }
    match class {
        _ if stale => ("recorded", Some(format!("stale_{}", class.as_str()))),
        FmpClass::Catalyst => ("catalyst", None),
        FmpClass::Offering => ("offering", None),
        FmpClass::Noise | FmpClass::ThirdParty => ("recorded", Some(class.as_str().to_string())),
    }
}

/// The record id: a hash of the URL, or of symbol, title and `publishedDate` when there is none.
pub fn item_id(item: &FeedItem) -> String {
    let basis = if item.url.is_empty() {
        format!(
            "{}\n{}\n{}",
            item.symbols.join(","),
            item.title,
            item.published_raw
        )
    } else {
        item.url.clone()
    };
    format!("fmp:{}", &sha256_hex(basis.as_bytes())[..24])
}

// ---------------------------------------------------------------------------------------------
// The universe
// ---------------------------------------------------------------------------------------------

/// The tradable universe: the last `days` dates of daily closes plus today's live quotes.
pub fn read_universe(archive: &Archive, now: DateTime<Utc>, days: usize) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    if let Ok(Some(closes)) = archive.read_daily_closes() {
        for (_, m) in closes.iter().rev().take(days) {
            out.extend(m.keys().cloned());
        }
    }
    let today = now.with_timezone(&New_York).date_naive();
    if let Ok(Some(quotes)) = archive.read_live_quotes(today) {
        out.extend(quotes.into_iter().map(|q| q.symbol));
    }
    out
}

/// FMP's symbol as the app's, when it is a whole ticker in `universe` (`Err`: the drop reason).
pub fn map_symbol(fmp: &str, universe: &BTreeSet<String>) -> Result<String, &'static str> {
    if universe.is_empty() {
        return Err("no_universe");
    }
    let sym = app_symbol(fmp).ok_or("bad_notation")?;
    if universe.contains(&sym) {
        Ok(sym)
    } else {
        Err("not_in_universe")
    }
}

// ---------------------------------------------------------------------------------------------
// The collector
// ---------------------------------------------------------------------------------------------

/// Whether the fast cadence applies: 04:00-20:00 ET on a weekday.
pub fn active_hours(now: DateTime<Utc>) -> bool {
    let et = now.with_timezone(&New_York);
    !matches!(et.weekday(), Weekday::Sat | Weekday::Sun) && (4..20).contains(&et.hour())
}

/// What one poll did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PollReport {
    pub polled: bool,
    pub requests: u32,
    pub pages: u32,
    pub items: u64,
    pub mapped: u64,
    pub written: u64,
    pub dropped: BTreeMap<&'static str, u64>,
    /// Written records by the class they have (or would have).
    pub classes: BTreeMap<&'static str, u64>,
    /// Fetch time minus the newest item's publication (ns), when the page had an item.
    pub newest_lag_ns: Option<i64>,
    pub errors: Vec<String>,
}

impl PollReport {
    pub fn line(&self) -> String {
        format!(
            "fmp news poll: {} requests, {} items, {} mapped, {} new{}{}{}",
            self.requests,
            self.items,
            self.mapped,
            self.written,
            fmt_lag(self.newest_lag_ns),
            fmt_counts(" dropped", &self.dropped),
            if self.errors.is_empty() {
                String::new()
            } else {
                format!("; errors: {}", self.errors.join(" | "))
            }
        )
    }
}

fn fmt_lag(ns: Option<i64>) -> String {
    ns.map_or(String::new(), |l| format!(", newest lag {}s", l / SEC))
}

fn fmt_counts(label: &str, m: &BTreeMap<&'static str, u64>) -> String {
    if m.is_empty() {
        return String::new();
    }
    let parts: Vec<String> = m.iter().map(|(k, v)| format!("{k} {v}")).collect();
    format!(";{label}: {}", parts.join(", "))
}

/// The ~10-minute summary, accumulated over polls.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Summary {
    pub polls: u64,
    pub failed_polls: u64,
    pub requests: u64,
    pub items: u64,
    pub mapped: u64,
    pub written: u64,
    pub dropped: BTreeMap<&'static str, u64>,
    pub classes: BTreeMap<&'static str, u64>,
    pub lag_last_ns: Option<i64>,
    pub lag_min_ns: Option<i64>,
    pub lag_max_ns: Option<i64>,
}

impl Summary {
    pub fn add(&mut self, r: &PollReport) {
        if !r.polled {
            return;
        }
        self.polls += 1;
        if !r.errors.is_empty() {
            self.failed_polls += 1;
        }
        self.requests += u64::from(r.requests);
        self.items += r.items;
        self.mapped += r.mapped;
        self.written += r.written;
        for (k, v) in &r.dropped {
            *self.dropped.entry(k).or_default() += v;
        }
        for (k, v) in &r.classes {
            *self.classes.entry(k).or_default() += v;
        }
        if let Some(l) = r.newest_lag_ns {
            self.lag_last_ns = Some(l);
            self.lag_min_ns = Some(self.lag_min_ns.map_or(l, |m| m.min(l)));
            self.lag_max_ns = Some(self.lag_max_ns.map_or(l, |m| m.max(l)));
        }
    }

    pub fn line(&self, count: bool) -> String {
        let lag = match (self.lag_last_ns, self.lag_min_ns, self.lag_max_ns) {
            (Some(l), Some(a), Some(b)) => format!(
                ", newest-item lag {}s (min {}s, max {}s)",
                l / SEC,
                a / SEC,
                b / SEC
            ),
            _ => ", newest-item lag n/a".into(),
        };
        format!(
            "fmp news summary ({}): {} polls ({} failed), {} requests, {} items, {} mapped, {} new{}{}{}",
            if count { "counting" } else { "record-only" },
            self.polls,
            self.failed_polls,
            self.requests,
            self.items,
            self.mapped,
            self.written,
            lag,
            fmt_counts(" classes", &self.classes),
            fmt_counts(" dropped", &self.dropped),
        )
    }
}

/// The worker's state. After a restart it reads what it stored in the last three days, so no id
/// is written twice and paging resumes from the newest item already seen.
pub struct FmpNewsCollector {
    pub cfg: FmpNewsConfig,
    /// `id|symbol` -> first_observed_at.
    seen: HashMap<String, i64>,
    /// The newest publication time any poll has seen.
    newest: Option<i64>,
    universe: BTreeSet<String>,
    universe_at: Option<i64>,
    next_poll: i64,
    failures: u32,
    pacer: Pacer,
    pub summary: Summary,
    summary_at: Option<i64>,
}

fn seen_key(id: &str, symbol: &str) -> String {
    format!("{id}|{symbol}")
}

impl FmpNewsCollector {
    /// Never fails: an unreadable archive starts with nothing seen (the worst case is a record
    /// written again, observed later, which only ever delays a count).
    pub fn open(
        cfg: FmpNewsConfig,
        archive: &Archive,
        now: DateTime<Utc>,
        log: &mut dyn FnMut(&str),
    ) -> Self {
        let since = (now - chrono::Duration::days(3)).timestamp_nanos_opt();
        let mut seen = HashMap::new();
        let mut newest = None;
        match archive.read_source_news(since) {
            Ok(rows) => {
                for r in rows.iter().filter(|r| r.rules == RULES) {
                    for s in &r.symbols {
                        seen.insert(seen_key(&r.id, s), r.first_observed_at);
                    }
                    newest = Some(newest.map_or(r.published_at, |n: i64| n.max(r.published_at)));
                }
            }
            Err(e) => log(&format!(
                "fmp news: could not read stored source news ({e}); starting with none seen"
            )),
        }
        FmpNewsCollector {
            pacer: Pacer::per_minute(cfg.per_minute),
            cfg,
            seen,
            newest,
            universe: BTreeSet::new(),
            universe_at: None,
            next_poll: i64::MIN,
            failures: 0,
            summary: Summary::default(),
            summary_at: None,
        }
    }

    /// Items already stored (tests).
    pub fn seen_len(&self) -> usize {
        self.seen.len()
    }

    /// One pass: refresh the universe hourly, poll when due, and return the summary line when
    /// one is due.
    pub fn tick(
        &mut self,
        archive: &Archive,
        http: &mut dyn Http,
        clock: &mut dyn FnMut() -> DateTime<Utc>,
        sleep: &mut dyn FnMut(Duration),
        log: &mut dyn FnMut(&str),
    ) -> (PollReport, Option<String>) {
        let now = clock();
        let now_ns = now.timestamp_nanos_opt().unwrap_or(i64::MAX);
        let mut report = PollReport::default();
        if self.universe_at.is_none_or(|t| now_ns - t >= 3600 * SEC) || self.universe.is_empty() {
            let u = read_universe(archive, now, self.cfg.universe_days);
            if !u.is_empty() || self.universe.is_empty() {
                self.universe = u;
            }
            self.universe_at = Some(now_ns);
        }
        if now_ns >= self.next_poll {
            let interval = if active_hours(now) {
                self.cfg.poll_secs
            } else {
                self.cfg.idle_poll_secs
            };
            report.polled = true;
            self.poll(archive, http, clock, sleep, log, &mut report);
            let wait = if report.errors.is_empty() {
                self.failures = 0;
                interval
            } else {
                self.failures = self.failures.saturating_add(1);
                (interval << self.failures.min(6)).clamp(interval, 900.max(interval))
            };
            self.next_poll = now_ns + wait * SEC;
            self.summary.add(&report);
        }
        let due = self
            .summary_at
            .is_none_or(|t| now_ns - t >= self.cfg.summary_secs * SEC);
        let line = if due && self.summary.polls > 0 {
            self.summary_at = Some(now_ns);
            let l = self.summary.line(self.cfg.count);
            self.summary = Summary::default();
            Some(l)
        } else {
            if self.summary_at.is_none() {
                self.summary_at = Some(now_ns);
            }
            None
        };
        (report, line)
    }

    /// One paced GET with bounded retries. The retry backoff (and a 429's `Retry-After`) really
    /// waits, through the same `sleep` the pacer uses.
    fn get(
        &mut self,
        http: &mut dyn Http,
        url: &str,
        sleep: &mut dyn FnMut(Duration),
        log: &mut dyn FnMut(&str),
        report: &mut PollReport,
    ) -> Result<HttpResponse, FetchError> {
        let key = self.cfg.key.0.clone();
        let sleep = std::cell::RefCell::new(sleep);
        let mut paced = Paced {
            inner: http,
            pacer: &mut self.pacer,
            sleep: &sleep,
            count: &mut report.requests,
        };
        get_with_retry(
            &mut paced,
            url,
            &[("Accept", "application/json")],
            RetryPolicy {
                max_attempts: 3,
                base_delay: Duration::from_secs(2),
                max_delay: Duration::from_secs(60),
            },
            &[key.as_str()],
            &mut |d| (sleep.borrow_mut())(d),
            log,
        )
    }

    fn poll(
        &mut self,
        archive: &Archive,
        http: &mut dyn Http,
        clock: &mut dyn FnMut() -> DateTime<Utc>,
        sleep: &mut dyn FnMut(Duration),
        log: &mut dyn FnMut(&str),
        report: &mut PollReport,
    ) {
        let prev_newest = self.newest;
        let mut records: Vec<SourceNewsRecord> = Vec::new();
        let mut batch_keys: BTreeSet<String> = BTreeSet::new();
        for page in 0..self.cfg.max_pages {
            let url = format!(
                "{}{}&apikey={}",
                self.cfg.base,
                endpoint(page, self.cfg.limit),
                self.cfg.key.0
            );
            let resp = match self.get(http, &url, sleep, log, report) {
                Ok(r) => r,
                Err(e) => {
                    report.errors.push(
                        format!("{} -> {e}", redact_url(&url)).replace(&self.cfg.key.0, "REDACTED"),
                    );
                    break;
                }
            };
            let observed = clock().timestamp_nanos_opt().unwrap_or(i64::MAX);
            report.pages += 1;
            let parsed = match parse_page(&resp.body) {
                Ok(p) => p,
                Err(e) => {
                    report.errors.push(format!(
                        "page {page}: {}",
                        e.replace(&self.cfg.key.0, "REDACTED")
                    ));
                    break;
                }
            };
            report.items += parsed.raw as u64;
            for (k, v) in &parsed.skipped {
                *report.dropped.entry(k).or_default() += v;
            }
            if page == 0
                && let Some(n) = parsed.items.iter().map(|i| i.published_at).max()
            {
                report.newest_lag_ns = Some(observed - n);
            }
            let mut known: Vec<bool> = Vec::with_capacity(parsed.raw);
            // Skipped elements count as new for paging: they say nothing about overlap.
            known.extend(std::iter::repeat_n(
                false,
                parsed.raw.saturating_sub(parsed.items.len()),
            ));
            for item in &parsed.items {
                let id = item_id(item);
                let mut any_seen = false;
                let mut syms: Vec<String> = Vec::new();
                for s in &item.symbols {
                    match map_symbol(s, &self.universe) {
                        Ok(sym) => {
                            let key = seen_key(&id, &sym);
                            if self.seen.contains_key(&key) || batch_keys.contains(&key) {
                                any_seen = true;
                            } else if !syms.contains(&sym) {
                                syms.push(sym);
                            }
                        }
                        Err(why) => *report.dropped.entry(why).or_default() += 1,
                    }
                }
                let older = prev_newest.is_some_and(|n| item.published_at <= n);
                known.push(any_seen || older);
                self.newest = Some(
                    self.newest
                        .map_or(item.published_at, |n| n.max(item.published_at)),
                );
                if syms.is_empty() {
                    continue;
                }
                report.mapped += syms.len() as u64;
                let class = classify(&item.title, &item.text, &item.publisher, &item.site);
                let stale = observed - item.published_at > STALE_AFTER_SECS * SEC;
                let (kind, event_type) = kind_of(class, self.cfg.count, stale);
                let publisher = if item.publisher.is_empty() {
                    &item.site
                } else {
                    &item.publisher
                };
                let slug = source_slug(publisher);
                for sym in syms {
                    batch_keys.insert(seen_key(&id, &sym));
                    *report.classes.entry(class.as_str()).or_default() += 1;
                    records.push(SourceNewsRecord {
                        v: market_archive::evidence::EVIDENCE_SCHEMA,
                        provider: PROVIDER.into(),
                        source: format!("fmp/{}", if slug.is_empty() { "unknown" } else { &slug }),
                        id: id.clone(),
                        version: 1,
                        symbols: vec![sym],
                        published_at: item.published_at,
                        first_observed_at: observed,
                        headline: Some(item.title.clone()),
                        url: (!item.url.is_empty()).then(|| item.url.clone()),
                        kind: kind.into(),
                        event_type: event_type.clone(),
                        rules: RULES.into(),
                    });
                }
            }
            if !next_page_wanted(&known, self.cfg.limit, page + 1, self.cfg.max_pages) {
                break;
            }
        }
        if records.is_empty() {
            return;
        }
        // Nothing is believed stored until it is: a failed append is retried by the next poll
        // (observed later, which only ever delays a count).
        match archive.append_source_news(&records) {
            Ok(()) => {
                report.written = records.len() as u64;
                for r in &records {
                    for s in &r.symbols {
                        self.seen.insert(seen_key(&r.id, s), r.first_observed_at);
                    }
                }
            }
            Err(e) => report.errors.push(format!("source-news store: {e}")),
        }
        let keep_from = clock().timestamp_nanos_opt().unwrap_or(i64::MAX) - 3 * 86_400 * SEC;
        self.seen.retain(|_, at| *at >= keep_from);
    }
}

/// The pacer in front of every request, counting them.
struct Paced<'a, 's> {
    inner: &'a mut dyn Http,
    pacer: &'a mut Pacer,
    sleep: &'a std::cell::RefCell<&'s mut dyn FnMut(Duration)>,
    count: &'a mut u32,
}

impl Http for Paced<'_, '_> {
    fn get(&mut self, url: &str, headers: &[(&str, &str)]) -> Result<HttpResponse, String> {
        self.pacer.wait(&mut |d| (self.sleep.borrow_mut())(d));
        *self.count += 1;
        self.inner.get(url, headers)
    }
}

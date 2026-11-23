//! **Press-release wires** as a news source for the Ross Panel's news pillar (owner,
//! 2026-11-22): small caps publish catalysts on press-release wires first, and on 2026-11-20
//! every one of the 36 symbols that passed price + gain + RVOL was held back by news because the
//! pillar only had Alpaca/Benzinga.
//!
//! # Off by default, and no built-in feed
//!
//! The source check of 2026-11-23 (`outputs/stockwatcher-ops/WIRES-SOURCES.md`) approved **no**
//! wire: PR Newswire's terms forbid automatic retrieval from its site, GlobeNewswire and
//! Business Wire refuse an identified non-browser client (or have deactivated their feeds),
//! Accesswire's robots.txt disallows `/rss`, and Newsfile answers with a WAF challenge. So
//! `WIRES_NEWS` defaults to off and the code names no feed URL; feeds are configuration
//! (`WIRES_FEEDS=id=url,...`), to be set only for a feed whose terms are cleared.
//!
//! # What it does
//!
//! One task per feed polls it at most once per [`WiresConfig::poll`] (never under 60 s), with at
//! most one request in flight per feed (the loop awaits each request), a conditional GET
//! (`If-None-Match` / `If-Modified-Since` from the previous answer) and exponential backoff on
//! errors (doubling from the poll period to 30 min). Each new entry is:
//!
//! 1. **archived** raw (`evidence/wires/YYYY-MM.jsonl`, month of first sight, append-only, one
//!    [`WireRecord`] per entry first seen, the teaser included) so an audit can replay what was
//!    seen and when, and re-run extraction under a later rule set;
//! 2. scanned for **exchange-qualified tickers** only ([`tickers`]), each mapped to the tradable
//!    universe (`assets.json`) on the same venue; unknown and foreign listings are dropped;
//! 3. **classified** ([`classify`]): `catalyst` by default, `offering` for financing headlines;
//! 4. **deduplicated** across wires: the same symbol with a near-identical headline seen within
//!    [`DEDUPE_WINDOW_NS`] is one release (the later copy is archived with `dup_of`, not
//!    delivered);
//! 5. handed to the pillar as one [`WireNewsItem`] per symbol through a [`WireSink`].
//!
//! # Point in time
//!
//! An item carries two times. `published_at_ns` is the feed's own published time, for display
//! and for the pillar's freshness. `observed_at_ns` is when **this server first fetched it**,
//! and is the evidence's effective time: a release cannot have been usable before we had it,
//! however early the wire stamps it, so a decision replayed at time T sees only items with
//! `observed_at_ns <= T`. Entries without a zoned published time, or published more than
//! [`MAX_AGE_NS`] before we first saw them (a feed's back catalogue on the first poll), are
//! archived but not delivered: they would otherwise read as fresh news on first sight.
//!
//! A restart reads the archive back ([`Collector::restore`]): entries already archived keep
//! their original first-seen time (no re-stamping later, no re-archiving) and still dedupe
//! against new copies. They are **not delivered again**: the shared news seam (ross/news,
//! `NEWS-SEAM.md`) persists what it is handed (`evidence/source-news/`), and the server's
//! evidence loader reads that back after a restart, so a second delivery would be a second
//! record of the same release.
//!
//! # The seam
//!
//! [`WireSink`] is the hand-off. Until ross/news is merged it is [`LogSink`] (items are archived
//! here and logged, nothing reaches the pillar). At merge it becomes a sink that writes one
//! `market_archive::SourceNewsRecord` per release (provider `press_wire`, source = the feed id,
//! all its symbols, `published_at`, `first_observed_at`, kind) with
//! `Archive::append_source_news`.

pub mod classify;
pub mod parse;
#[cfg(test)]
mod tests;
pub mod tickers;

pub use classify::WireKind;

use market_archive::Archive;
use market_archive::assets::AssetMap;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// The rule set (extraction + classification + dedupe) stamped on every record.
pub const RULES: &str = "wires-rules-v1";
/// Archive directory under `evidence/`.
pub const WIRES_DIR: &str = "wires";
/// Record schema.
pub const RECORD_V: u32 = 1;
const SEC: i64 = 1_000_000_000;
/// Cross-wire dedupe window (on first-seen times).
pub const DEDUPE_WINDOW_NS: i64 = 10 * 60 * SEC;
/// Older than this at first sight: archived, not delivered (module docs).
pub const MAX_AGE_NS: i64 = 24 * 3600 * SEC;
/// A response bigger than this is refused (a feed is tens of KB).
const MAX_BODY: usize = 4 << 20;
/// The floor and default of the poll period.
pub const MIN_POLL: Duration = Duration::from_secs(60);
/// The backoff ceiling.
const MAX_BACKOFF: Duration = Duration::from_secs(30 * 60);
/// The project contact, sent as the User-Agent so a wire's operator can reach us.
pub const USER_AGENT: &str = "StockWatcher sw@wavystyle.io";

/// One item handed to the news pillar: one release, one symbol.
///
/// The fields are those of the shared news seam on ross/news (source id, symbol, published
/// time, headline, url, kind) plus the first-seen time the pillar must gate on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WireNewsItem {
    /// `wire:<feed id>` (`wire:prnewswire`).
    pub source: String,
    /// Stable per release and feed: `wire:<feed id>:<entry key>`.
    pub id: String,
    pub symbol: String,
    pub published_at_ns: Option<i64>,
    /// First fetched by this server (the effective time; module docs).
    pub observed_at_ns: i64,
    pub headline: String,
    pub url: Option<String>,
    pub kind: WireKind,
}

/// Where items go. The ross/news seam is the real one; until it is merged, [`LogSink`].
pub trait WireSink: Send + Sync + 'static {
    fn deliver(&self, items: Vec<WireNewsItem>);
}

/// Logs each item; nothing reaches the pillar. Stands in until the news seam is merged.
pub struct LogSink;

impl WireSink for LogSink {
    fn deliver(&self, items: Vec<WireNewsItem>) {
        for i in items {
            tracing::info!(
                source = %i.source,
                symbol = %i.symbol,
                kind = i.kind.as_str(),
                headline = %i.headline,
                "wires: item (no news seam wired yet; not reaching the pillar)"
            );
        }
    }
}

/// One configured feed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FeedSpec {
    /// Short id (`[a-z0-9_]`), used in the source id and the archive.
    pub id: String,
    pub url: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WiresConfig {
    /// `WIRES_NEWS` (default off; `1` on, `0` off).
    pub on: bool,
    /// `WIRES_FEEDS`: `id=url` pairs, comma-separated. No default (module docs).
    pub feeds: Vec<FeedSpec>,
    /// `WIRES_POLL_SECS` (default and floor 60).
    pub poll: Duration,
}

impl Default for WiresConfig {
    fn default() -> Self {
        WiresConfig {
            on: false,
            feeds: Vec::new(),
            poll: MIN_POLL,
        }
    }
}

impl WiresConfig {
    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Result<Self, String> {
        let var = |k: &str| {
            get(k)
                .map(|v| v.trim().to_owned())
                .filter(|v| !v.is_empty())
        };
        let on = match var("WIRES_NEWS").as_deref() {
            None | Some("0") | Some("false") | Some("no") | Some("off") => false,
            Some("1") | Some("true") | Some("yes") | Some("on") => true,
            Some(v) => return Err(format!("WIRES_NEWS: expected 0 or 1, got {v:?}")),
        };
        let mut feeds = Vec::new();
        for pair in var("WIRES_FEEDS").unwrap_or_default().split(',') {
            let pair = pair.trim();
            if pair.is_empty() {
                continue;
            }
            let (id, url) = pair
                .split_once('=')
                .ok_or_else(|| format!("WIRES_FEEDS: expected id=url, got {pair:?}"))?;
            let (id, url) = (id.trim(), url.trim());
            if id.is_empty()
                || !id
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
            {
                return Err(format!("WIRES_FEEDS: feed id {id:?} must be [a-z0-9_]+"));
            }
            if !url.starts_with("https://") {
                return Err(format!(
                    "WIRES_FEEDS: {id}: only https:// feeds, got {url:?}"
                ));
            }
            if feeds.iter().any(|f: &FeedSpec| f.id == id) {
                return Err(format!("WIRES_FEEDS: feed id {id:?} given twice"));
            }
            feeds.push(FeedSpec {
                id: id.into(),
                url: url.into(),
            });
        }
        let poll = match var("WIRES_POLL_SECS") {
            None => MIN_POLL,
            Some(v) => {
                let s: u64 = v
                    .parse()
                    .map_err(|_| format!("WIRES_POLL_SECS: not a number: {v:?}"))?;
                Duration::from_secs(s).max(MIN_POLL)
            }
        };
        Ok(WiresConfig { on, feeds, poll })
    }
}

/// One entry first seen, as archived (`evidence/wires/YYYY-MM.jsonl`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WireRecord {
    pub v: u32,
    pub feed: String,
    /// [`parse::Entry::key`].
    pub key: String,
    pub guid: String,
    pub link: String,
    pub title: String,
    /// The feed's teaser as text (not the release body: feeds carry only the first lines).
    pub summary: String,
    pub published_raw: Option<String>,
    pub published_at_ns: Option<i64>,
    /// When this server first fetched it (UTC ns).
    pub first_observed_at: i64,
    /// The exchange-qualified listings found, `VENUE:TICKER`, mapped or not.
    pub listings: Vec<String>,
    /// The listings that mapped to the tradable universe.
    pub symbols: Vec<String>,
    pub kind: WireKind,
    /// Whether it went to the pillar, and if not why: `delivered`, `no_symbol`, `noise` (a
    /// law-firm solicitation, [`classify::is_noise`]), `duplicate`, `no_published_time`,
    /// `stale`.
    pub status: String,
    /// The earlier release's id when this is a cross-wire duplicate.
    pub dup_of: Option<String>,
    pub rules: String,
}

/// The pure part: entries in, records and items out. One per server, shared by every feed task
/// (the dedupe is across wires).
pub struct Collector {
    universe: Arc<AssetMap>,
    /// `(feed, key)` already archived.
    seen: HashMap<(String, String), i64>,
    /// Recently delivered per symbol: (first seen, headline words, item id).
    recent: HashMap<String, Vec<(i64, BTreeSet<String>, String)>>,
}

/// What one ingest produced.
#[derive(Debug, Default)]
pub struct Ingested {
    pub records: Vec<WireRecord>,
    pub items: Vec<WireNewsItem>,
}

impl Collector {
    pub fn new(universe: Arc<AssetMap>) -> Self {
        Collector {
            universe,
            seen: HashMap::new(),
            recent: HashMap::new(),
        }
    }

    pub fn set_universe(&mut self, universe: Arc<AssetMap>) {
        self.universe = universe;
    }

    /// Seeds the state from archived records (a restart) and returns the items they delivered
    /// within [`MAX_AGE_NS`] of `now`, with their original times: what the pillar's own store
    /// should already hold. The caller does not deliver them again (module docs).
    pub fn restore(&mut self, records: &[WireRecord], now: i64) -> Vec<WireNewsItem> {
        let mut out = Vec::new();
        for r in records {
            self.seen
                .entry((r.feed.clone(), r.key.clone()))
                .or_insert(r.first_observed_at);
            if r.status != "delivered" || now - r.first_observed_at > MAX_AGE_NS {
                continue;
            }
            let words = classify::words(&r.title);
            for s in &r.symbols {
                let item = item_of(&r.feed, &r.key, s, r, r.kind);
                self.recent.entry(s.clone()).or_default().push((
                    r.first_observed_at,
                    words.clone(),
                    item.id.clone(),
                ));
                out.push(item);
            }
        }
        out
    }

    /// Parses `body` from `feed` fetched at `now` (UTC ns). A malformed body is an error and
    /// changes nothing.
    pub fn ingest(&mut self, feed: &str, body: &str, now: i64) -> Result<Ingested, String> {
        let entries = parse::parse_feed(body)?;
        let mut out = Ingested::default();
        for e in entries {
            let key = e.key().to_owned();
            if key.is_empty() {
                continue;
            }
            let k = (feed.to_owned(), key.clone());
            if self.seen.contains_key(&k) {
                continue;
            }
            self.seen.insert(k, now);
            out.records
                .push(self.record(feed, key, e, now, &mut out.items));
        }
        self.prune(now);
        Ok(out)
    }

    fn record(
        &mut self,
        feed: &str,
        key: String,
        e: parse::Entry,
        now: i64,
        items: &mut Vec<WireNewsItem>,
    ) -> WireRecord {
        let text = format!("{} {}", e.title, e.summary);
        let found = tickers::listings(&text);
        let mut symbols: Vec<String> = Vec::new();
        for l in &found {
            let ok = self
                .universe
                .get(&l.ticker)
                .is_some_and(|a| l.venue.matches(&a.exchange));
            if ok && !symbols.contains(&l.ticker) {
                symbols.push(l.ticker.clone());
            }
        }
        let kind = classify::classify(&e.title);
        let mut r = WireRecord {
            v: RECORD_V,
            feed: feed.into(),
            key,
            guid: e.guid,
            link: e.link,
            title: e.title,
            summary: e.summary,
            published_raw: e.published_raw,
            published_at_ns: e.published_at_ns,
            first_observed_at: now,
            listings: found
                .iter()
                .map(|l| format!("{}:{}", l.venue.as_str(), l.ticker))
                .collect(),
            symbols,
            kind,
            status: String::new(),
            dup_of: None,
            rules: RULES.into(),
        };
        r.status = if r.symbols.is_empty() {
            "no_symbol"
        } else if classify::is_noise(&r.title, &r.summary) {
            "noise"
        } else if r.published_at_ns.is_none() {
            "no_published_time"
        } else if r.published_at_ns.is_some_and(|p| now - p > MAX_AGE_NS) {
            "stale"
        } else {
            let words = classify::words(&r.title);
            let dup = r.symbols.iter().find_map(|s| {
                self.recent.get(s)?.iter().find_map(|(t, w, id)| {
                    (now - t <= DEDUPE_WINDOW_NS && classify::similar(w, &words))
                        .then(|| id.clone())
                })
            });
            match dup {
                Some(id) => {
                    r.dup_of = Some(id);
                    "duplicate"
                }
                None => {
                    for s in &r.symbols {
                        let item = item_of(feed, &r.key, s, &r, kind);
                        self.recent.entry(s.clone()).or_default().push((
                            now,
                            words.clone(),
                            item.id.clone(),
                        ));
                        items.push(item);
                    }
                    "delivered"
                }
            }
        }
        .into();
        r
    }

    /// Forgets dedupe history past its window, and seen keys after a week (a feed lists a few
    /// days at most; a week-old key reappearing is a re-post, archived again).
    fn prune(&mut self, now: i64) {
        self.recent.retain(|_, v| {
            v.retain(|(t, _, _)| now - t <= DEDUPE_WINDOW_NS);
            !v.is_empty()
        });
        self.seen.retain(|_, t| now - *t <= 7 * 24 * 3600 * SEC);
    }
}

fn item_of(feed: &str, key: &str, symbol: &str, r: &WireRecord, kind: WireKind) -> WireNewsItem {
    WireNewsItem {
        source: format!("wire:{feed}"),
        id: format!("wire:{feed}:{key}"),
        symbol: symbol.into(),
        published_at_ns: r.published_at_ns,
        observed_at_ns: r.first_observed_at,
        headline: r.title.clone(),
        url: (!r.link.is_empty()).then(|| r.link.clone()),
        kind,
    }
}

/// The archive directory for wire records.
pub fn wires_dir(archive_root: &Path) -> PathBuf {
    archive_root
        .join(market_archive::EVIDENCE_DIR)
        .join(WIRES_DIR)
}

/// Appends records to their month's file (month of first sight, UTC).
pub fn append_records(dir: &Path, records: &[WireRecord]) -> std::io::Result<()> {
    let mut by_month: HashMap<String, Vec<&WireRecord>> = HashMap::new();
    for r in records {
        let m = chrono::DateTime::from_timestamp_nanos(r.first_observed_at)
            .format("%Y-%m")
            .to_string();
        by_month.entry(m).or_default().push(r);
    }
    std::fs::create_dir_all(dir)?;
    for (m, rs) in by_month {
        let mut buf = Vec::new();
        for r in rs {
            serde_json::to_writer(&mut buf, r).map_err(std::io::Error::other)?;
            buf.push(b'\n');
        }
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join(format!("{m}.jsonl")))?;
        f.write_all(&buf)?;
        f.sync_data()?;
    }
    Ok(())
}

/// Reads the records of the current and previous month (enough for [`Collector::restore`]).
/// Unreadable lines (a torn last line after a crash) are skipped.
pub fn read_recent_records(dir: &Path, now: i64) -> Vec<WireRecord> {
    let t = chrono::DateTime::from_timestamp_nanos(now);
    let prev = t - chrono::Duration::days(32);
    let mut months = vec![
        prev.format("%Y-%m").to_string(),
        t.format("%Y-%m").to_string(),
    ];
    months.dedup();
    let mut out = Vec::new();
    for m in months {
        let Ok(s) = std::fs::read_to_string(dir.join(format!("{m}.jsonl"))) else {
            continue;
        };
        out.extend(
            s.lines()
                .filter_map(|l| serde_json::from_str::<WireRecord>(l).ok()),
        );
    }
    out
}

/// The validators of the last answer, for the conditional GET.
#[derive(Default)]
struct Validators {
    etag: Option<String>,
    last_modified: Option<String>,
}

enum Fetched {
    NotModified,
    Body(String),
}

async fn fetch(
    http: &reqwest::Client,
    feed: &FeedSpec,
    v: &mut Validators,
) -> Result<Fetched, String> {
    use reqwest::header;
    let mut req = http
        .get(&feed.url)
        .header(header::USER_AGENT, USER_AGENT)
        .header(
            header::ACCEPT,
            "application/rss+xml, application/atom+xml, application/xml;q=0.9, text/xml;q=0.8",
        );
    if let Some(e) = &v.etag {
        req = req.header(header::IF_NONE_MATCH, e);
    }
    if let Some(m) = &v.last_modified {
        req = req.header(header::IF_MODIFIED_SINCE, m);
    }
    let resp = req.send().await.map_err(|e| format!("request: {e}"))?;
    let status = resp.status();
    if status == reqwest::StatusCode::NOT_MODIFIED {
        return Ok(Fetched::NotModified);
    }
    if !status.is_success() {
        return Err(format!("HTTP {status}"));
    }
    if resp.content_length().is_some_and(|n| n as usize > MAX_BODY) {
        return Err("response too large".into());
    }
    let hv = |h: header::HeaderName| {
        resp.headers()
            .get(h)
            .and_then(|x| x.to_str().ok())
            .map(str::to_owned)
    };
    let (etag, lm) = (hv(header::ETAG), hv(header::LAST_MODIFIED));
    let bytes = resp.bytes().await.map_err(|e| format!("body: {e}"))?;
    if bytes.len() > MAX_BODY {
        return Err("response too large".into());
    }
    let body = String::from_utf8(bytes.to_vec()).map_err(|_| "body is not UTF-8".to_string())?;
    // Stored only after a whole body arrived and decoded: a failed read must not make the next
    // poll a 304 for content never processed.
    v.etag = etag;
    v.last_modified = lm;
    Ok(Fetched::Body(body))
}

/// The delay before the next poll after `failures` consecutive failures.
pub fn backoff(poll: Duration, failures: u32) -> Duration {
    if failures == 0 {
        return poll;
    }
    let mult = 1u32 << failures.min(10);
    poll.saturating_mul(mult).min(MAX_BACKOFF).max(poll)
}

struct Shared {
    collector: Mutex<Collector>,
    dir: PathBuf,
    sink: Arc<dyn WireSink>,
}

/// Starts the wires source (live market only; the caller checks). Logs one start line naming
/// the active feeds, or why it is off. Must run inside a Tokio runtime.
pub fn start(cfg: WiresConfig, archive: Archive, sink: Arc<dyn WireSink>) {
    if !cfg.on {
        tracing::info!("wires: off (WIRES_NEWS=1 and WIRES_FEEDS turn it on)");
        return;
    }
    if cfg.feeds.is_empty() {
        tracing::warn!("wires: WIRES_NEWS=1 but WIRES_FEEDS names no feed; off");
        return;
    }
    let http = match crate::alerts::push::https_client(Duration::from_secs(20)) {
        Ok(h) => h,
        Err(e) => {
            tracing::error!(error = %e, "wires: no HTTP client; off");
            return;
        }
    };
    let names: Vec<String> = cfg
        .feeds
        .iter()
        .map(|f| format!("{}={}", f.id, f.url))
        .collect();
    tracing::info!(
        feeds = %names.join(","),
        poll_secs = cfg.poll.as_secs(),
        rules = RULES,
        "wires: press-release wires on"
    );
    tokio::spawn(async move {
        let dir = wires_dir(archive.root());
        let (universe, restored, items) = {
            let (archive, dir) = (archive.clone(), dir.clone());
            tokio::task::spawn_blocking(move || {
                let now = crate::observe::wall_now();
                let universe = Arc::new(read_universe(&archive));
                let records = read_recent_records(&dir, now);
                let mut c = Collector::new(universe.clone());
                let items = c.restore(&records, now);
                (universe, c, items)
            })
            .await
            .unwrap_or_else(|_| {
                let u = Arc::new(AssetMap::new());
                (u.clone(), Collector::new(u), Vec::new())
            })
        };
        // Not delivered again: the seam's own store already has them (module docs).
        tracing::info!(
            universe = universe.len(),
            delivered_last_24h = items.len(),
            "wires: state restored from the archive"
        );
        let shared = Arc::new(Shared {
            collector: Mutex::new(restored),
            dir,
            sink,
        });
        for feed in cfg.feeds {
            tokio::spawn(poll_feed(http.clone(), feed, cfg.poll, shared.clone()));
        }
        // `assets.json` is refreshed nightly; re-read it every 10 minutes.
        loop {
            tokio::time::sleep(Duration::from_secs(600)).await;
            let a = archive.clone();
            if let Ok(u) = tokio::task::spawn_blocking(move || read_universe(&a)).await
                && !u.is_empty()
            {
                shared
                    .collector
                    .lock()
                    .expect("wires lock")
                    .set_universe(Arc::new(u));
            }
        }
    });
}

fn read_universe(a: &Archive) -> AssetMap {
    match a.read_assets() {
        Ok(Some(m)) => m,
        Ok(None) => {
            tracing::warn!("wires: no assets.json; no release can map to a symbol");
            AssetMap::new()
        }
        Err(e) => {
            tracing::warn!(error = %e, "wires: assets.json unreadable; no release can map");
            AssetMap::new()
        }
    }
}

async fn poll_feed(http: reqwest::Client, feed: FeedSpec, poll: Duration, sh: Arc<Shared>) {
    let mut v = Validators::default();
    let mut failures: u32 = 0;
    loop {
        let res = fetch(&http, &feed, &mut v).await;
        let now = crate::observe::wall_now();
        let outcome = match res {
            Ok(Fetched::NotModified) => Ok(()),
            Ok(Fetched::Body(body)) => {
                let ing = sh
                    .collector
                    .lock()
                    .expect("wires lock")
                    .ingest(&feed.id, &body, now);
                match ing {
                    Ok(ing) => {
                        if !ing.records.is_empty() {
                            let (dir, recs) = (sh.dir.clone(), ing.records);
                            let n = recs.len();
                            match tokio::task::spawn_blocking(move || append_records(&dir, &recs))
                                .await
                            {
                                Ok(Ok(())) => {}
                                Ok(Err(e)) => {
                                    tracing::warn!(feed = %feed.id, error = %e, n, "wires: archive write failed")
                                }
                                Err(e) => {
                                    tracing::warn!(feed = %feed.id, error = %e, "wires: archive task failed")
                                }
                            }
                        }
                        if !ing.items.is_empty() {
                            sh.sink.deliver(ing.items);
                        }
                        Ok(())
                    }
                    // Malformed XML: nothing ingested, the validators are dropped so the next
                    // poll refetches in full rather than getting a 304 for the bad body.
                    Err(e) => {
                        v = Validators::default();
                        Err(e)
                    }
                }
            }
            Err(e) => Err(e),
        };
        match outcome {
            Ok(()) => {
                if failures > 0 {
                    tracing::info!(feed = %feed.id, failures, "wires: feed recovered");
                }
                failures = 0;
            }
            Err(e) => {
                failures = failures.saturating_add(1);
                if failures == 1 || failures.is_multiple_of(10) {
                    tracing::warn!(feed = %feed.id, error = %e, failures, "wires: poll failed; backing off");
                }
            }
        }
        tokio::time::sleep(backoff(poll, failures)).await;
    }
}

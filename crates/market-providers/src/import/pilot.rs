//! A bounded research acquisition: a fixed list of (trading day, symbol) pairs, five data kinds
//! each, checkpointed so a re-run resumes, metered so the run can be measured and stopped at a
//! budget, and summarized into one versioned `MANIFEST.json` (`swimport research-pilot`).
//!
//! # Layout under the output directory (the research data root, outside git)
//!
//! ```text
//! archive/<day>/<SYM>.swa            SIP trades 04:00-20:00 ET (the production importer's path)
//! archive-boats/<day>/<SYM>.swa      BOATS overnight trades, 20:00 ET the evening before -> 04:00
//! archive-quotes/<day>/<SYM>.swa     SIP NBBO quotes 04:00-20:00 ET as MarketEvent::Quote
//! raw/daily-bars/<day>/<SYM>.json    raw (unadjusted) SIP 1Day bars, the N sessions before <day>
//! raw/corporate-actions/<day>/<SYM>.json   actions from the first warm-up session through <day>
//! quarantine/<day>/<SYM>.quotes.jsonl      quote records refused by the importer (reason + raw)
//! checkpoints/<day>/<SYM>.<kind>.json      one per finished kind; the resume point
//! runs.jsonl                               one line per run: stats, pacing, budget use
//! MANIFEST.json                            built by [`write_manifest`] from all of the above
//! ```
//!
//! Each archive root is a separate `market_archive::Archive` because the archive keeps one file
//! per (day, symbol): trades, overnight trades and quotes in one root would overwrite each other.
//!
//! # Resume and failure
//!
//! A kind whose checkpoint says `complete` or `inaccessible` is skipped on a re-run; anything
//! else (failed, budget-stopped, never started) is fetched again from scratch. The archive
//! importers add their own guard (an identical complete import is never refetched).
//!
//! # Budget
//!
//! [`Metered`] wraps the HTTP client: it counts per kind and, once the active-time budget is
//! spent, answers every request locally with status [`BUDGET_STATUS`] (a non-transient status,
//! so the importers stop at once instead of retrying). The stored-bytes budget is checked
//! between kinds. Both are cumulative over runs (read back from `runs.jsonl`).

use super::alpaca::{
    AlpacaCredentials, Feed, ImportError, ImportOptions, TradesQuery, import_trades, rfc3339,
};
use super::batch::sip_range;
use super::http::{FetchError, Http, HttpResponse, RetryPolicy};
use super::quotes::{QuotesQuery, import_quotes};
use super::reference::{
    STRUCTURAL_ACTION_TYPES, corporate_actions_url, daily_bars_url, et_date_range, fetch_pages,
    sessions_before, summarize_corporate_actions,
};
use chrono::{DateTime, NaiveDate, Utc};
use chrono_tz::America::New_York;
use market_archive::{Archive, ArchiveConfig, SymbolReader};
use market_events::session::sessions_for;
use market_events::{MarketEvent, SessionKind, UnixNanos};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Status [`Metered`] answers with once the time budget is spent. Not a real HTTP status and
/// not transient, so `get_with_retry` gives up immediately.
pub const BUDGET_STATUS: u16 = 0;
pub const MANIFEST_SCHEMA: u32 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Kind {
    DailyBars,
    CorporateActions,
    TradesSip,
    TradesBoats,
    QuotesSip,
}

impl Kind {
    pub const ALL: [Kind; 5] = [
        Kind::DailyBars,
        Kind::CorporateActions,
        Kind::TradesSip,
        Kind::TradesBoats,
        Kind::QuotesSip,
    ];
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::DailyBars => "daily-bars",
            Kind::CorporateActions => "corporate-actions",
            Kind::TradesSip => "trades-sip",
            Kind::TradesBoats => "trades-boats",
            Kind::QuotesSip => "quotes-sip",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Status {
    Complete,
    /// The provider refused the request as not permitted for this account (401/403/422 etc.).
    Inaccessible,
    Failed,
    BudgetStopped,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SymbolDay {
    pub day: NaiveDate,
    pub symbol: String,
}

/// Per-kind request accounting.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct KindStats {
    pub requests: u64,
    pub ok: u64,
    /// Responses with a transient status (each one triggers a retry or ends the attempts).
    pub transient: u64,
    pub throttled_429: u64,
    pub network_errors: u64,
    /// Requests answered locally because the time budget was spent.
    pub budget_refused: u64,
    pub statuses: BTreeMap<String, u64>,
    /// Decoded (uncompressed) response bytes.
    pub decoded_bytes: u64,
    /// Wall time spent inside `get` (includes pacing waits of a paced client).
    pub http_ms: u64,
}

/// Counting, budget-enforcing wrapper around the real client. See the module docs.
pub struct Metered<'a> {
    inner: &'a mut dyn Http,
    kind: String,
    deadline: Option<Instant>,
    pub stats: BTreeMap<String, KindStats>,
    pub budget_hit: bool,
}

impl<'a> Metered<'a> {
    pub fn new(inner: &'a mut dyn Http, deadline: Option<Instant>) -> Self {
        Metered {
            inner,
            kind: "other".into(),
            deadline,
            stats: BTreeMap::new(),
            budget_hit: false,
        }
    }
    pub fn set_kind(&mut self, kind: &str) {
        self.kind = kind.to_string();
    }
    fn current(&mut self) -> &mut KindStats {
        self.stats.entry(self.kind.clone()).or_default()
    }
}

impl Http for Metered<'_> {
    fn get(&mut self, url: &str, headers: &[(&str, &str)]) -> Result<HttpResponse, String> {
        if self.deadline.is_some_and(|d| Instant::now() >= d) {
            self.budget_hit = true;
            self.current().budget_refused += 1;
            return Ok(HttpResponse {
                status: BUDGET_STATUS,
                body: b"local time budget exhausted".to_vec(),
                retry_after: None,
            });
        }
        let t = Instant::now();
        let r = self.inner.get(url, headers);
        let ms = t.elapsed().as_millis() as u64;
        let s = self.current();
        s.requests += 1;
        s.http_ms += ms;
        match &r {
            Ok(resp) => {
                *s.statuses.entry(resp.status.to_string()).or_default() += 1;
                s.decoded_bytes += resp.body.len() as u64;
                if (200..300).contains(&resp.status) {
                    s.ok += 1;
                } else if super::http::is_transient(resp.status) {
                    s.transient += 1;
                }
                if resp.status == 429 {
                    s.throttled_429 += 1;
                }
            }
            Err(_) => s.network_errors += 1,
        }
        r
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Checkpoint {
    pub day: NaiveDate,
    pub symbol: String,
    pub kind: Kind,
    pub status: Status,
    #[serde(default)]
    pub http_status: Option<u16>,
    #[serde(default)]
    pub error: Option<String>,
    /// What was asked for: endpoint, feed, range (no credentials, no full URL).
    pub requested: Value,
    /// Records stored (events, bars or action rows).
    pub records: u64,
    pub pages: u64,
    /// First / last event time actually present, RFC 3339 UTC.
    #[serde(default)]
    pub first_ts: Option<String>,
    #[serde(default)]
    pub last_ts: Option<String>,
    /// Data files written, relative to the output root (forward slashes).
    pub files: Vec<String>,
    pub stored_bytes: u64,
    pub stats: KindStats,
    pub elapsed_ms: u64,
    pub finished_at: String,
    /// Kind-specific quality detail (update flags, duplicates, quote quality, missing sessions…).
    pub details: Value,
}

pub fn checkpoint_path(out: &Path, sd: &SymbolDay, kind: Kind) -> PathBuf {
    out.join("checkpoints")
        .join(sd.day.to_string())
        .join(format!("{}.{}.json", sd.symbol, kind.as_str()))
}

pub fn read_checkpoint(out: &Path, sd: &SymbolDay, kind: Kind) -> Option<Checkpoint> {
    let s = std::fs::read(checkpoint_path(out, sd, kind)).ok()?;
    serde_json::from_slice(&s).ok()
}

/// Writes `bytes` to `path` atomically (temp file + rename), creating parent directories.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let tmp = path.with_extension("tmp");
    let mut f = std::fs::File::create(&tmp).map_err(|e| format!("{}: {e}", tmp.display()))?;
    f.write_all(bytes)
        .and_then(|_| f.sync_all())
        .map_err(|e| format!("{}: {e}", tmp.display()))?;
    drop(f);
    std::fs::rename(&tmp, path).map_err(|e| format!("{}: {e}", path.display()))
}

pub struct Budget {
    /// Active download time, cumulative over runs.
    pub active: Duration,
    /// New stored data, cumulative (measured as the size of the output root).
    pub bytes: u64,
}

pub struct PilotConfig<'a> {
    pub out: &'a Path,
    pub data_base: &'a str,
    pub creds: &'a AlpacaCredentials,
    pub budget: Budget,
    /// Active time already used by earlier runs (from `runs.jsonl`).
    pub used_active: Duration,
    /// Output-root size before the first run (so the byte budget counts only new data).
    pub baseline_bytes: u64,
    pub warmup_sessions: usize,
    pub kinds: Vec<Kind>,
    pub retry: RetryPolicy,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct PilotRun {
    pub started_at: String,
    pub finished_at: String,
    /// Wall time spent inside kinds that fetched (not skipped ones).
    pub active_ms: u64,
    pub stats: BTreeMap<String, KindStats>,
    pub ran: u64,
    pub skipped: u64,
    pub stopped_by: Option<String>,
}

/// Total size of the regular files under `dir` (0 if missing).
pub fn dir_bytes(dir: &Path) -> u64 {
    let mut n = 0;
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            match e.file_type() {
                Ok(t) if t.is_dir() => n += dir_bytes(&e.path()),
                Ok(t) if t.is_file() => n += e.metadata().map(|m| m.len()).unwrap_or(0),
                _ => {}
            }
        }
    }
    n
}

fn archive_at(root: PathBuf) -> Result<Archive, String> {
    let root = root.to_string_lossy().to_string();
    ArchiveConfig::from_lookup(|k| (k == "ARCHIVE_ROOT").then(|| root.clone())).map(Archive::new)
}

fn rel(out: &Path, p: &Path) -> String {
    p.strip_prefix(out)
        .unwrap_or(p)
        .to_string_lossy()
        .replace('\\', "/")
}

fn ts_str(ns: Option<i64>) -> Option<String> {
    ns.map(|n| rfc3339(market_events::UnixNanos(n).to_utc()))
}

fn fetch_status(e: &ImportError) -> Option<u16> {
    match e {
        ImportError::Fetch {
            error: FetchError::Permanent { status, .. },
            ..
        } => Some(*status),
        _ => None,
    }
}

/// Runs every (symbol-day, kind) not already checkpointed. `log` gets progress lines (never URLs
/// with credentials: none are ever in a URL here).
pub fn run_pilot(
    http: &mut dyn Http,
    cfg: &PilotConfig,
    tasks: &[SymbolDay],
    sleep: &mut dyn FnMut(Duration),
    log: &mut dyn FnMut(&str),
) -> Result<PilotRun, String> {
    let started = Instant::now();
    let remaining = cfg.budget.active.saturating_sub(cfg.used_active);
    let mut metered = Metered::new(http, Some(started + remaining));
    let mut run = PilotRun {
        started_at: rfc3339(Utc::now()),
        ..PilotRun::default()
    };
    let mut active = Duration::ZERO;
    'outer: for sd in tasks {
        for &kind in &cfg.kinds {
            if let Some(c) = read_checkpoint(cfg.out, sd, kind)
                && matches!(c.status, Status::Complete | Status::Inaccessible)
            {
                run.skipped += 1;
                continue;
            }
            let used = dir_bytes(cfg.out).saturating_sub(cfg.baseline_bytes);
            if used >= cfg.budget.bytes {
                run.stopped_by = Some(format!("stored-bytes budget ({used} B)"));
                break 'outer;
            }
            if metered.budget_hit || cfg.used_active + active >= cfg.budget.active {
                run.stopped_by = Some("active-time budget".into());
                break 'outer;
            }
            log(&format!("{} {} {}", sd.day, sd.symbol, kind.as_str()));
            metered.set_kind(kind.as_str());
            let before = metered
                .stats
                .get(kind.as_str())
                .cloned()
                .unwrap_or_default();
            let t = Instant::now();
            let mut cp = run_kind(&mut metered, cfg, sd, kind, sleep, log);
            let elapsed = t.elapsed();
            active += elapsed;
            let after = metered
                .stats
                .get(kind.as_str())
                .cloned()
                .unwrap_or_default();
            cp.stats = diff(&after, &before);
            cp.elapsed_ms = elapsed.as_millis() as u64;
            if metered.budget_hit && cp.status != Status::Complete {
                cp.status = Status::BudgetStopped;
            }
            log(&format!(
                "  -> {:?}: {} records, {} pages, {} B stored, {} requests, {:.1}s{}",
                cp.status,
                cp.records,
                cp.pages,
                cp.stored_bytes,
                cp.stats.requests,
                elapsed.as_secs_f64(),
                cp.error
                    .as_deref()
                    .map(|e| format!(" ({e})"))
                    .unwrap_or_default()
            ));
            let path = checkpoint_path(cfg.out, sd, kind);
            write_atomic(
                &path,
                &serde_json::to_vec_pretty(&cp).map_err(|e| e.to_string())?,
            )?;
            run.ran += 1;
        }
    }
    if run.stopped_by.is_none() && metered.budget_hit {
        run.stopped_by = Some("active-time budget".into());
    }
    run.active_ms = active.as_millis() as u64;
    run.stats = metered.stats.clone();
    run.finished_at = rfc3339(Utc::now());
    Ok(run)
}

fn diff(after: &KindStats, before: &KindStats) -> KindStats {
    let mut statuses = after.statuses.clone();
    for (k, v) in &before.statuses {
        if let Some(x) = statuses.get_mut(k) {
            *x -= v;
        }
    }
    statuses.retain(|_, v| *v > 0);
    KindStats {
        requests: after.requests - before.requests,
        ok: after.ok - before.ok,
        transient: after.transient - before.transient,
        throttled_429: after.throttled_429 - before.throttled_429,
        network_errors: after.network_errors - before.network_errors,
        budget_refused: after.budget_refused - before.budget_refused,
        statuses,
        decoded_bytes: after.decoded_bytes - before.decoded_bytes,
        http_ms: after.http_ms - before.http_ms,
    }
}

fn blank(sd: &SymbolDay, kind: Kind, requested: Value) -> Checkpoint {
    Checkpoint {
        day: sd.day,
        symbol: sd.symbol.clone(),
        kind,
        status: Status::Failed,
        http_status: None,
        error: None,
        requested,
        records: 0,
        pages: 0,
        first_ts: None,
        last_ts: None,
        files: Vec::new(),
        stored_bytes: 0,
        stats: KindStats::default(),
        elapsed_ms: 0,
        finished_at: String::new(),
        details: Value::Null,
    }
}

/// A permanent refusal the account is not entitled to (vs. a request we got wrong).
fn is_entitlement(status: u16) -> bool {
    matches!(status, 401 | 402 | 403 | 422)
}

fn run_kind(
    http: &mut dyn Http,
    cfg: &PilotConfig,
    sd: &SymbolDay,
    kind: Kind,
    sleep: &mut dyn FnMut(Duration),
    log: &mut dyn FnMut(&str),
) -> Checkpoint {
    let mut cp = match kind {
        Kind::DailyBars => daily_bars(http, cfg, sd, sleep, log),
        Kind::CorporateActions => corporate_actions(http, cfg, sd, sleep, log),
        Kind::TradesSip | Kind::TradesBoats => trades(http, cfg, sd, kind, sleep, log),
        Kind::QuotesSip => quotes(http, cfg, sd, sleep, log),
    };
    cp.finished_at = rfc3339(Utc::now());
    cp
}

#[allow(clippy::too_many_arguments)]
fn raw_json_kind(
    http: &mut dyn Http,
    cfg: &PilotConfig,
    sd: &SymbolDay,
    kind: Kind,
    requested: Value,
    url: &dyn Fn(Option<&str>) -> String,
    sleep: &mut dyn FnMut(Duration),
    log: &mut dyn FnMut(&str),
    count: &dyn Fn(&[Value]) -> (u64, Value),
) -> Checkpoint {
    let mut cp = blank(sd, kind, requested.clone());
    match fetch_pages(http, cfg.creds, url, sleep, log) {
        Ok(pages) => {
            let (records, details) = count(&pages);
            let path = cfg
                .out
                .join("raw")
                .join(kind.as_str())
                .join(sd.day.to_string())
                .join(format!("{}.json", sd.symbol));
            let doc = json!({
                "request": requested,
                "fetched_at": rfc3339(Utc::now()),
                "pages": pages,
            });
            match serde_json::to_vec(&doc)
                .map_err(|e| e.to_string())
                .and_then(|b| write_atomic(&path, &b).map(|_| b.len() as u64))
            {
                Ok(n) => {
                    cp.status = Status::Complete;
                    cp.records = records;
                    cp.pages = pages.len() as u64;
                    cp.files = vec![rel(cfg.out, &path)];
                    cp.stored_bytes = n;
                    cp.details = details;
                }
                Err(e) => cp.error = Some(e),
            }
        }
        Err(e) => {
            cp.http_status = e
                .strip_prefix("HTTP ")
                .and_then(|s| s.split(':').next())
                .and_then(|s| s.trim().parse().ok());
            if cp.http_status.is_some_and(is_entitlement) {
                cp.status = Status::Inaccessible;
            }
            cp.error = Some(e);
        }
    }
    cp
}

fn daily_bars(
    http: &mut dyn Http,
    cfg: &PilotConfig,
    sd: &SymbolDay,
    sleep: &mut dyn FnMut(Duration),
    log: &mut dyn FnMut(&str),
) -> Checkpoint {
    let sessions = sessions_before(sd.day, cfg.warmup_sessions);
    let (Some(first), Some(last)) = (sessions.first().copied(), sessions.last().copied()) else {
        let mut cp = blank(sd, Kind::DailyBars, Value::Null);
        cp.error = Some("no sessions before the day".into());
        return cp;
    };
    let Some((start, end)) = et_date_range(first, last) else {
        let mut cp = blank(sd, Kind::DailyBars, Value::Null);
        cp.error = Some("calendar does not cover the range".into());
        return cp;
    };
    let symbols = vec![sd.symbol.clone()];
    let requested = json!({
        "endpoint": "/v2/stocks/bars",
        "timeframe": "1Day", "feed": "sip", "adjustment": "raw",
        "start": rfc3339(start), "end": rfc3339(end),
        "sessions_requested": sessions.len(),
        "first_session": first, "last_session": last,
    });
    let base = cfg.data_base.to_string();
    let url = move |t: Option<&str>| daily_bars_url(&base, &symbols, start, end, t);
    let want: Vec<NaiveDate> = sessions.clone();
    let count = move |pages: &[Value]| {
        let rows = super::reference::bar_rows(pages);
        let got: HashSet<NaiveDate> = rows
            .iter()
            .filter_map(|(_, b)| b.get("t").and_then(Value::as_str))
            .filter_map(|t| DateTime::parse_from_rfc3339(t).ok())
            .map(|t| t.with_timezone(&New_York).date_naive())
            .collect();
        let missing: Vec<NaiveDate> = want.iter().filter(|d| !got.contains(d)).copied().collect();
        let unexpected = got.iter().filter(|d| !want.contains(d)).count();
        (
            rows.len() as u64,
            json!({ "bars": rows.len(), "missing_sessions": missing, "unexpected_dates": unexpected }),
        )
    };
    raw_json_kind(
        http,
        cfg,
        sd,
        Kind::DailyBars,
        requested,
        &url,
        sleep,
        log,
        &count,
    )
}

fn corporate_actions(
    http: &mut dyn Http,
    cfg: &PilotConfig,
    sd: &SymbolDay,
    sleep: &mut dyn FnMut(Duration),
    log: &mut dyn FnMut(&str),
) -> Checkpoint {
    let sessions = sessions_before(sd.day, cfg.warmup_sessions);
    let start = sessions.first().copied().unwrap_or(sd.day);
    let types = format!("{STRUCTURAL_ACTION_TYPES},cash_dividend");
    let requested = json!({
        "endpoint": "/v1/corporate-actions", "symbols": [sd.symbol],
        "types": types, "start": start, "end": sd.day,
    });
    let base = cfg.data_base.to_string();
    let symbols = vec![sd.symbol.clone()];
    let url = move |t: Option<&str>| {
        corporate_actions_url(&base, Some(&symbols), &types, start, sd.day, t)
    };
    let count = |pages: &[Value]| {
        let s = summarize_corporate_actions(pages);
        (
            s.rows.len() as u64,
            serde_json::to_value(&s).unwrap_or_default(),
        )
    };
    raw_json_kind(
        http,
        cfg,
        sd,
        Kind::CorporateActions,
        requested,
        &url,
        sleep,
        log,
        &count,
    )
}

fn window(sd: &SymbolDay, kind: Kind) -> Option<(DateTime<Utc>, DateTime<Utc>)> {
    match kind {
        Kind::TradesBoats => sessions_for(sd.day)
            .into_iter()
            .find(|(id, ..)| id.kind == SessionKind::Overnight)
            .map(|(_, s, e)| (s.to_utc(), e.to_utc())),
        _ => sip_range(sd.day),
    }
}

fn remove_partial(archive: &Archive, sd: &SymbolDay) {
    let p = archive
        .day_dir(sd.day)
        .join(format!("{}.swa.partial", sd.symbol));
    let _ = std::fs::remove_file(p);
}

fn trades(
    http: &mut dyn Http,
    cfg: &PilotConfig,
    sd: &SymbolDay,
    kind: Kind,
    sleep: &mut dyn FnMut(Duration),
    log: &mut dyn FnMut(&str),
) -> Checkpoint {
    let (feed, root) = match kind {
        Kind::TradesBoats => (Feed::Boats, "archive-boats"),
        _ => (Feed::Sip, "archive"),
    };
    let Some((start, end)) = window(sd, kind) else {
        let mut cp = blank(sd, kind, Value::Null);
        cp.error = Some(format!("{} has no {} window", sd.day, kind.as_str()));
        return cp;
    };
    let q = TradesQuery {
        symbol: sd.symbol.clone(),
        day: sd.day,
        start,
        end,
        feed,
        limit: super::alpaca::MAX_LIMIT,
    };
    let requested = json!({
        "endpoint": q.endpoint(), "feed": feed.as_str(),
        "start": rfc3339(start), "end": rfc3339(end), "sort": "asc", "limit": q.limit,
    });
    let mut cp = blank(sd, kind, requested);
    let archive = match archive_at(cfg.out.join(root)) {
        Ok(a) => a,
        Err(e) => {
            cp.error = Some(e);
            return cp;
        }
    };
    let opts = ImportOptions {
        retry: cfg.retry,
        force: false,
    };
    match import_trades(
        http,
        cfg.data_base,
        cfg.creds,
        &q,
        &archive,
        &opts,
        sleep,
        log,
    ) {
        Ok(r) => {
            let entry = r.entry.unwrap_or_default();
            let path = archive.day_dir(sd.day).join(&entry.file);
            cp.status = Status::Complete;
            cp.records = r.events;
            cp.pages = r.pages;
            cp.first_ts = ts_str(r.first_ts);
            cp.last_ts = ts_str(r.last_ts);
            cp.files = vec![rel(cfg.out, &path)];
            cp.stored_bytes = entry.bytes;
            let verify = verify_trades(&path);
            cp.details = json!({
                "skipped_identical": r.skipped,
                "empty_pages": r.empty_pages,
                "update_flags_u": r.updates,
                "out_of_order": r.out_of_order,
                "sha256": entry.sha256,
                "verify": verify,
            });
        }
        Err(e) => {
            remove_partial(&archive, sd);
            cp.http_status = fetch_status(&e);
            if cp.http_status.is_some_and(is_entitlement) {
                cp.status = Status::Inaccessible;
            }
            cp.error = Some(e.to_string());
        }
    }
    cp
}

/// Reads a finished trades file back: duplicate trade ids, `u` flags, condition codes, session
/// split and the decoded (JSON record) size.
pub fn verify_trades(path: &Path) -> Value {
    let reader = match SymbolReader::open(path) {
        Ok(r) => r,
        Err(e) => return json!({ "error": e.to_string() }),
    };
    // (exchange, id) -> (event_ts, price, size, tape) of the first print seen with that identity.
    let mut ids: HashMap<(String, u64), (i64, i64, u64, String)> = HashMap::new();
    let mut dup_ids = 0u64;
    let mut dup_identical = 0u64;
    // Collisions the chart engine would treat as a repeat: within its default 10-minute
    // trade-retention window of the first print with that identity (chart-engine `retention_ns`).
    let mut dup_within_10min = 0u64;
    let mut dup_within_10min_shares = 0u64;
    let mut total_shares = 0u64;
    let mut dup_by_exchange: BTreeMap<String, u64> = BTreeMap::new();
    let mut dup_by_tape_pair: BTreeMap<String, u64> = BTreeMap::new();
    let mut dup_examples: Vec<Value> = Vec::new();
    let mut events = 0u64;
    let mut json_bytes = 0u64;
    let mut updates: BTreeMap<String, u64> = BTreeMap::new();
    let mut conditions: BTreeMap<String, u64> = BTreeMap::new();
    let mut sessions: BTreeMap<String, u64> = BTreeMap::new();
    let mut update_examples: Vec<Value> = Vec::new();
    for ev in reader {
        let ev = match ev {
            Ok(e) => e,
            Err(e) => return json!({ "error": e.to_string(), "events_read": events }),
        };
        events += 1;
        json_bytes += serde_json::to_vec(&ev)
            .map(|v| v.len() as u64 + 4)
            .unwrap_or(0);
        if let MarketEvent::Trade(t) = &ev.event {
            total_shares += t.size;
            let tape = format!("{:?}", t.tape);
            let this = (t.event_ts.0, t.price.micros(), t.size, tape.clone());
            match ids.get(&(t.exchange.clone(), t.source_seq)) {
                None => {
                    ids.insert((t.exchange.clone(), t.source_seq), this);
                }
                Some(first) => {
                    dup_ids += 1;
                    if (this.0 - first.0).abs() <= 600_000_000_000 {
                        dup_within_10min += 1;
                        dup_within_10min_shares += this.2;
                    }
                    if first.0 == this.0 && first.1 == this.1 && first.2 == this.2 {
                        dup_identical += 1;
                    }
                    *dup_by_exchange.entry(t.exchange.clone()).or_default() += 1;
                    *dup_by_tape_pair
                        .entry(format!("{}/{}", first.3, tape))
                        .or_default() += 1;
                    if dup_examples.len() < 5 {
                        dup_examples.push(json!({
                            "exchange": t.exchange, "id": t.source_seq,
                            "first": { "event_ts": rfc3339(UnixNanos(first.0).to_utc()), "price_micros": first.1, "size": first.2, "tape": first.3 },
                            "again": { "event_ts": rfc3339(t.event_ts.to_utc()), "price_micros": this.1, "size": this.2, "tape": this.3 },
                        }));
                    }
                }
            }
            if let Some(u) = &t.update {
                *updates.entry(u.clone()).or_default() += 1;
                if update_examples.len() < 10 {
                    update_examples.push(json!({
                        "u": u, "event_ts": rfc3339(t.event_ts.to_utc()), "id": t.source_seq,
                        "exchange": t.exchange, "price": t.price.to_f64(), "size": t.size,
                        "conditions": t.conditions,
                    }));
                }
            }
            for c in &t.conditions {
                *conditions.entry(c.clone()).or_default() += 1;
            }
            let s = t
                .session
                .map(|s| s.kind.as_str().to_string())
                .unwrap_or_else(|| "none".into());
            *sessions.entry(s).or_default() += 1;
        }
    }
    json!({
        "events_read": events,
        "decoded_record_bytes": json_bytes,
        "duplicate_ids": dup_ids,
        "duplicate_ids_identical_print": dup_identical,
        "duplicate_ids_within_10min": dup_within_10min,
        "duplicate_ids_within_10min_shares": dup_within_10min_shares,
        "total_shares": total_shares,
        "duplicate_ids_by_exchange": dup_by_exchange,
        "duplicate_ids_by_tape_pair": dup_by_tape_pair,
        "duplicate_id_examples": dup_examples,
        "update_flags_u": updates,
        "update_examples": update_examples,
        "conditions": conditions,
        "sessions": sessions,
    })
}

fn quotes(
    http: &mut dyn Http,
    cfg: &PilotConfig,
    sd: &SymbolDay,
    sleep: &mut dyn FnMut(Duration),
    log: &mut dyn FnMut(&str),
) -> Checkpoint {
    let Some((start, end)) = sip_range(sd.day) else {
        let mut cp = blank(sd, Kind::QuotesSip, Value::Null);
        cp.error = Some("no SIP window".into());
        return cp;
    };
    let q = QuotesQuery {
        symbol: sd.symbol.clone(),
        day: sd.day,
        start,
        end,
        feed: Feed::Sip,
        limit: super::alpaca::MAX_LIMIT,
    };
    let requested = json!({
        "endpoint": q.endpoint(), "feed": "sip",
        "start": rfc3339(start), "end": rfc3339(end), "sort": "asc", "limit": q.limit,
    });
    let mut cp = blank(sd, Kind::QuotesSip, requested);
    let archive = match archive_at(cfg.out.join("archive-quotes")) {
        Ok(a) => a,
        Err(e) => {
            cp.error = Some(e);
            return cp;
        }
    };
    let qpath = cfg
        .out
        .join("quarantine")
        .join(sd.day.to_string())
        .join(format!("{}.quotes.jsonl", sd.symbol));
    let _ = std::fs::remove_file(&qpath);
    let mut qlines: Vec<String> = Vec::new();
    let opts = ImportOptions {
        retry: cfg.retry,
        force: false,
    };
    let res = import_quotes(
        http,
        cfg.data_base,
        cfg.creds,
        &q,
        &archive,
        &opts,
        sleep,
        log,
        &mut |raw: &str, reason: &str| {
            qlines.push(json!({ "reason": reason, "raw": raw }).to_string());
        },
    );
    let mut files = Vec::new();
    if !qlines.is_empty() {
        let body = qlines.join("\n") + "\n";
        if write_atomic(&qpath, body.as_bytes()).is_ok() {
            files.push(rel(cfg.out, &qpath));
        }
    }
    match res {
        Ok(r) => {
            let entry = r.entry.clone().unwrap_or_default();
            let path = archive.day_dir(sd.day).join(&entry.file);
            files.insert(0, rel(cfg.out, &path));
            cp.status = Status::Complete;
            cp.records = r.events;
            cp.pages = r.pages;
            cp.first_ts = ts_str(r.first_ts);
            cp.last_ts = ts_str(r.last_ts);
            cp.files = files;
            cp.stored_bytes = entry.bytes;
            let mut d = serde_json::to_value(&r).unwrap_or_default();
            d["sha256"] = json!(entry.sha256);
            d["decoded_record_bytes"] = json!(record_bytes(&path));
            cp.details = d;
        }
        Err(e) => {
            remove_partial(&archive, sd);
            cp.http_status = fetch_status(&e);
            if cp.http_status.is_some_and(is_entitlement) {
                cp.status = Status::Inaccessible;
            }
            cp.files = files;
            cp.error = Some(e.to_string());
        }
    }
    cp
}

/// Size of a `.swa` file's decoded records (what the zstd frame decompresses to, minus framing).
fn record_bytes(path: &Path) -> Option<u64> {
    let r = SymbolReader::open(path).ok()?;
    let mut n = 0u64;
    for ev in r {
        n += serde_json::to_vec(&ev.ok()?).ok()?.len() as u64 + 4;
    }
    Some(n)
}

/// `(lowercase hex sha256, size)` of a file, streamed.
pub fn sha256_file(path: &Path) -> Result<(String, u64), String> {
    let mut f = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 256 * 1024];
    let mut total = 0u64;
    loop {
        let n = f
            .read(&mut buf)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
        total += n as u64;
    }
    Ok((hex::encode(h.finalize()), total))
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    if let Ok(rd) = std::fs::read_dir(dir) {
        let mut entries: Vec<_> = rd.flatten().collect();
        entries.sort_by_key(|e| e.path());
        for e in entries {
            let p = e.path();
            match e.file_type() {
                Ok(t) if t.is_dir() => walk(&p, out),
                Ok(t) if t.is_file() => out.push(p),
                _ => {}
            }
        }
    }
}

/// Reads every checkpoint under `out/checkpoints`.
pub fn all_checkpoints(out: &Path) -> Vec<Checkpoint> {
    let mut files = Vec::new();
    walk(&out.join("checkpoints"), &mut files);
    files
        .iter()
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .filter_map(|p| serde_json::from_slice(&std::fs::read(p).ok()?).ok())
        .collect()
}

/// One line of `runs.jsonl`.
pub fn read_runs(out: &Path) -> Vec<Value> {
    std::fs::read_to_string(out.join("runs.jsonl"))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

pub fn append_run(out: &Path, run: &Value) -> Result<(), String> {
    std::fs::create_dir_all(out).map_err(|e| e.to_string())?;
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(out.join("runs.jsonl"))
        .map_err(|e| e.to_string())?;
    writeln!(f, "{run}").map_err(|e| e.to_string())
}

/// Builds `MANIFEST.json`: static description, every file with sha256, every checkpoint grouped
/// per symbol-day, run history, and aggregated quality counts. `meta` is merged in (code version,
/// selection, notes). The revision increments on every rewrite.
pub fn write_manifest(out: &Path, meta: Value) -> Result<Value, String> {
    let path = out.join("MANIFEST.json");
    let revision = std::fs::read(&path)
        .ok()
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        .and_then(|v| v.get("revision").and_then(Value::as_u64))
        .unwrap_or(0)
        + 1;
    let mut files = Vec::new();
    walk(out, &mut files);
    let mut file_rows = Vec::new();
    let mut by_area: BTreeMap<String, (u64, u64)> = BTreeMap::new();
    for p in &files {
        let r = rel(out, p);
        if r == "MANIFEST.json" || r.ends_with(".tmp") {
            continue;
        }
        let (sha, bytes) = sha256_file(p)?;
        let area = r.split('/').next().unwrap_or("").to_string();
        let a = by_area.entry(area).or_default();
        a.0 += 1;
        a.1 += bytes;
        file_rows.push(json!({ "path": r, "bytes": bytes, "sha256": sha }));
    }
    let cps = all_checkpoints(out);
    let mut per: BTreeMap<String, BTreeMap<String, Value>> = BTreeMap::new();
    let mut missing = Vec::new();
    let mut inaccessible = Vec::new();
    let (mut dups, mut quarantined) = (0u64, 0u64);
    let mut u_flags: BTreeMap<String, u64> = BTreeMap::new();
    for c in &cps {
        let key = format!("{} {}", c.day, c.symbol);
        if c.status != Status::Complete {
            missing.push(json!({ "day": c.day, "symbol": c.symbol, "kind": c.kind, "status": c.status, "http_status": c.http_status, "error": c.error }));
        }
        if c.status == Status::Inaccessible {
            inaccessible.push(json!({ "kind": c.kind, "http_status": c.http_status }));
        }
        dups += c.details["verify"]["duplicate_ids"].as_u64().unwrap_or(0)
            + c.details["exact_duplicates"].as_u64().unwrap_or(0);
        quarantined += c.details["quarantined"].as_u64().unwrap_or(0);
        if let Some(m) = c.details["verify"]["update_flags_u"].as_object() {
            for (k, v) in m {
                *u_flags.entry(k.clone()).or_default() += v.as_u64().unwrap_or(0);
            }
        }
        per.entry(key).or_default().insert(
            c.kind.as_str().into(),
            serde_json::to_value(c).unwrap_or_default(),
        );
    }
    let areas: BTreeMap<String, Value> = by_area
        .into_iter()
        .map(|(k, (n, b))| (k, json!({ "files": n, "bytes": b })))
        .collect();
    let mut m = json!({
        "manifest_schema": MANIFEST_SCHEMA,
        "revision": revision,
        "written_at": rfc3339(Utc::now()),
        "root": out.to_string_lossy(),
        "description": "Bounded Alpaca historical acquisition pilot (research). Not production data; not used by the server.",
        "sources": {
            "provider": "Alpaca Markets Data API v2 (historical REST only; no stream opened)",
            "endpoints": {
                "trades-sip": "GET /v2/stocks/{symbol}/trades feed=sip sort=asc limit=10000, 04:00-20:00 ET",
                "trades-boats": "GET /v2/stocks/{symbol}/trades feed=boats, 20:00 ET previous evening -> 04:00 ET",
                "quotes-sip": "GET /v2/stocks/{symbol}/quotes feed=sip sort=asc limit=10000, 04:00-20:00 ET",
                "daily-bars": "GET /v2/stocks/bars timeframe=1Day feed=sip adjustment=raw",
                "corporate-actions": "GET /v1/corporate-actions",
                "assets": "GET /v2/assets?status=active|inactive&asset_class=us_equity (trading API, current snapshot)",
            },
        },
        "formats": {
            "archive": ".swa v1 (market-archive): zstd frame of length-prefixed JSON VersionedEvent records, contract v1; one file per (trading day, symbol) per archive root; prices in micro-dollars",
            "archive_roots": { "archive": "SIP trades", "archive-boats": "BOATS overnight trades", "archive-quotes": "SIP NBBO quotes (MarketEvent::Quote)" },
            "raw": "provider JSON pages verbatim with the credential-free request",
        },
        "timestamps": {
            "event_ts": "exchange/SIP timestamp from the provider (t), ns UTC",
            "receive_ts": "absent (null) for all historical records",
            "fidelity_class": "reconstructed-history",
            "trading_day": "ET trading date; the overnight session 20:00-04:00 belongs to the following trading date",
        },
        "adjustment": "all prices raw (as traded); daily bars adjustment=raw; no corporate-action adjustment applied anywhere",
        "update_flags": "trade u field (canceled/incorrect/corrected) is the provider's final hindsight verdict with no publication time; kept raw on the trade, not turned into events",
        "areas": areas,
        "files": file_rows,
        "symbol_days": per,
        "missing_partitions": missing,
        "inaccessible": inaccessible,
        "totals": {
            "duplicates": dups,
            "quarantined": quarantined,
            "trade_update_flags_u": u_flags,
            "checkpoints": cps.len(),
        },
        "runs": read_runs(out),
    });
    if let (Some(dst), Some(src)) = (m.as_object_mut(), meta.as_object()) {
        for (k, v) in src {
            dst.insert(k.clone(), v.clone());
        }
    }
    let bytes = serde_json::to_vec_pretty(&m).map_err(|e| e.to_string())?;
    write_atomic(&path, &bytes)?;
    Ok(m)
}

/// Recomputes [`verify_trades`] for every complete trades checkpoint from the files on disk (no
/// network) and rewrites the checkpoints. Returns how many were updated.
pub fn reverify_trades(out: &Path) -> Result<usize, String> {
    let mut n = 0;
    for mut c in all_checkpoints(out) {
        if c.status != Status::Complete || !matches!(c.kind, Kind::TradesSip | Kind::TradesBoats) {
            continue;
        }
        let Some(file) = c.files.first() else {
            continue;
        };
        c.details["verify"] = verify_trades(&out.join(file));
        let sd = SymbolDay {
            day: c.day,
            symbol: c.symbol.clone(),
        };
        write_atomic(
            &checkpoint_path(out, &sd, c.kind),
            &serde_json::to_vec_pretty(&c).map_err(|e| e.to_string())?,
        )?;
        n += 1;
    }
    Ok(n)
}

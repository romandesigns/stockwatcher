//! The forward float and news collector (`swimport evidence`): bounded, cached, append-only.
//!
//! It runs beside the server (the archiver's image and volume), never inside it: the server must
//! not make an HTTP call per market event, and evidence only has to reach it as records in the
//! archive (`market_archive::evidence`), which the context engine loads (`evidence_load`). Plan,
//! request estimates and the reasoning: `docs/stock-context/float-news-plan.md`.
//!
//! # What it collects
//!
//! - **Float** (FMP `shares-float`, current-only): once per trading day at `EVIDENCE_FLOAT_AT`
//!   ET (default 07:00, before the open) for the watch universe, plus once for a symbol that
//!   enters the band intraday and was not fetched that day ("lazy warmup", capped per day). A
//!   symbol is fetched at most once **successfully** per ET date, restarts included: the day's
//!   stored records say what is already done. The refresh is spread over ticks so news polling
//!   never waits on it.
//!
//!   Done means **durably appended**, not "the request returned": a fetch that could not be
//!   stored, exhausted HTTP retries or hit a network error is scheduled for another bounded
//!   attempt (`EVIDENCE_FLOAT_RETRY_*`, exponential and jittered, capped per symbol per day and
//!   with a bounded waiting queue whose drops are counted). Only a plan refusal is permanent: it
//!   pauses the day's float work and clears the pending retries, because a provider that has
//!   already said no will not change its mind before tomorrow.
//! - **News** (Alpaca `/v1beta1/news`, all symbols in one query): every `EVIDENCE_NEWS_POLL_SECS`
//!   (60 s) from 04:00 to 20:00 ET on trading days, every `EVIDENCE_NEWS_IDLE_POLL_SECS` (300 s)
//!   otherwise. Each poll searches from the previous complete search's end minus an overlap
//!   (5 min) to now minus a settle delay (30 s), so a story indexed a little after its
//!   `created_at` is still found. Every `EVIDENCE_NEWS_EDIT_EVERY_SECS` (15 min) a trailing
//!   window (`EVIDENCE_NEWS_EDIT_WINDOW_MINS`, 3 h) is searched again to catch edits.
//!
//! # The universe
//!
//! The "watch list" split used elsewhere: symbols whose official close was inside the production
//! band ($0.10–$25, `PRICE_BAND_MIN`/`MAX`) on any of the last `EVIDENCE_UNIVERSE_DAYS` (5)
//! trading dates in `daily-closes.json`, plus intraday entrants read from today's live quotes
//! (`<day>/live/quotes.json`, written by the recorder). Reading the recorder's file rather than
//! asking the server keeps the server free of any provider call and needs no new endpoint.

use super::alpaca::AlpacaCredentials;
use super::alpaca_news::{NewsQuery, NewsVersions, coverage_record, fetch_news};
use super::fmp::{FmpError, FmpKey};
use super::fmp_float::{fetch_float, fetch_float_all};
use super::http::{Http, RetryPolicy};
use chrono::{DateTime, Duration as Days, NaiveDate, NaiveTime, TimeZone, Utc};
use chrono_tz::America::New_York;
use market_archive::{Archive, CoverageScope, DailyCloses, FloatRecord, LiveQuote};
use market_events::{Price, PriceBand, is_trading_day};
use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use std::time::{Duration, Instant};

const SEC: i64 = 1_000_000_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FloatMode {
    /// One `shares-float?symbol=` per symbol (probed and working): ~7,500 requests a day.
    PerSymbol,
    /// `shares-float-all` pages (~10 requests for the whole market), falling back to per-symbol
    /// on the first refusal. Default: pages 0 and 1 probed on this account (2026-10-20), and FMP
    /// exposes no plan limit to check a 7,500-a-day per-symbol refresh against.
    Bulk,
}

/// Environment of `swimport evidence` (credentials, `ARCHIVE_*` and the band are read elsewhere).
///
/// | Variable | Default | Meaning |
/// |---|---|---|
/// | `EVIDENCE_FLOAT_AT` | `07:00` | ET time of the daily float refresh (`HH:MM`) |
/// | `EVIDENCE_FLOAT_MODE` | `bulk` | `shares-float-all` pages, per-symbol on refusal; or `per-symbol` |
/// | `EVIDENCE_FMP_PER_MINUTE` | `120` | FMP request ceiling for this process |
/// | `EVIDENCE_FLOAT_WARMUP_MAX` | `500` | intraday entrants fetched per ET date, at most |
/// | `EVIDENCE_FLOAT_RETRY_ATTEMPTS` | `4` | transient float retries per symbol per ET date |
/// | `EVIDENCE_FLOAT_RETRY_BASE_SECS` | `30` | first transient retry delay (doubling, jittered) |
/// | `EVIDENCE_FLOAT_RETRY_MAX_SECS` | `900` | transient retry delay ceiling |
/// | `EVIDENCE_FLOAT_RETRY_QUEUE_MAX` | `2000` | symbols waiting for a retry, at most |
/// | `EVIDENCE_UNIVERSE_DAYS` | `5` | trading dates of closes that define the watch universe |
/// | `EVIDENCE_NEWS_POLL_SECS` | `60` | news poll interval, 04:00–20:00 ET on trading days |
/// | `EVIDENCE_NEWS_IDLE_POLL_SECS` | `300` | news poll interval otherwise |
/// | `EVIDENCE_NEWS_EDIT_EVERY_SECS` | `900` | how often the trailing edit window is searched |
/// | `EVIDENCE_NEWS_EDIT_WINDOW_MINS` | `180` | the trailing edit window |
/// | `EVIDENCE_ALPACA_PER_MINUTE` | `60` | Alpaca news request ceiling for this process |
/// | `ALPACA_DATA_BASE` / `FMP_BASE` | providers' | API bases |
#[derive(Clone, Debug)]
pub struct EvidenceConfig {
    pub float_at: NaiveTime,
    pub float_mode: FloatMode,
    pub fmp_per_minute: u32,
    pub float_warmup_max: usize,
    /// Transient float retries per symbol per ET date (0: none). A provider or disk failure is
    /// not the symbol's whole day.
    pub float_retry_attempts: u32,
    pub float_retry_base_secs: i64,
    pub float_retry_max_secs: i64,
    /// Symbols waiting for a retry at once; over it the oldest wait is dropped and counted.
    pub float_retry_queue_max: usize,
    pub universe_days: usize,
    pub news_poll_secs: i64,
    pub news_idle_poll_secs: i64,
    pub news_edit_every_secs: i64,
    pub news_edit_window_secs: i64,
    pub news_settle_secs: i64,
    pub news_overlap_secs: i64,
    /// First start (or a gap longer than `news_max_catchup_secs`): search this far back, so the
    /// pillar's 24 h freshness window is covered at once.
    pub news_initial_lookback_secs: i64,
    pub news_max_catchup_secs: i64,
    pub alpaca_per_minute: u32,
    pub band: PriceBand,
    pub data_base: String,
    pub fmp_base: String,
}

impl Default for EvidenceConfig {
    fn default() -> Self {
        EvidenceConfig {
            float_at: NaiveTime::from_hms_opt(7, 0, 0).expect("valid"),
            float_mode: FloatMode::Bulk,
            fmp_per_minute: 120,
            float_warmup_max: 500,
            float_retry_attempts: 4,
            float_retry_base_secs: 30,
            float_retry_max_secs: 900,
            float_retry_queue_max: 2_000,
            universe_days: 5,
            news_poll_secs: 60,
            news_idle_poll_secs: 300,
            news_edit_every_secs: 900,
            news_edit_window_secs: 3 * 3600,
            news_settle_secs: 30,
            news_overlap_secs: 300,
            news_initial_lookback_secs: 25 * 3600,
            news_max_catchup_secs: 3 * 86_400,
            alpaca_per_minute: 60,
            band: PriceBand::DEFAULT,
            data_base: "https://data.alpaca.markets".into(),
            fmp_base: "https://financialmodelingprep.com".into(),
        }
    }
}

impl EvidenceConfig {
    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Result<Self, String> {
        let d = EvidenceConfig::default();
        let var = |k: &str| {
            get(k)
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
        };
        let num = |k: &str, def: i64, min: i64| -> Result<i64, String> {
            match var(k) {
                None => Ok(def),
                Some(v) => match v.parse::<i64>() {
                    Ok(n) if n >= min => Ok(n),
                    _ => Err(format!("{k} must be an integer >= {min}, got {v:?}")),
                },
            }
        };
        let float_at = match var("EVIDENCE_FLOAT_AT") {
            None => d.float_at,
            Some(v) => NaiveTime::parse_from_str(&v, "%H:%M")
                .map_err(|_| format!("EVIDENCE_FLOAT_AT must be HH:MM, got {v:?}"))?,
        };
        let float_mode = match var("EVIDENCE_FLOAT_MODE").as_deref() {
            Some("per-symbol") => FloatMode::PerSymbol,
            None | Some("bulk") => FloatMode::Bulk,
            Some(v) => {
                return Err(format!(
                    "EVIDENCE_FLOAT_MODE must be per-symbol or bulk, got {v:?}"
                ));
            }
        };
        Ok(EvidenceConfig {
            float_at,
            float_mode,
            fmp_per_minute: num("EVIDENCE_FMP_PER_MINUTE", d.fmp_per_minute as i64, 1)? as u32,
            float_warmup_max: num("EVIDENCE_FLOAT_WARMUP_MAX", d.float_warmup_max as i64, 0)?
                as usize,
            float_retry_attempts: num(
                "EVIDENCE_FLOAT_RETRY_ATTEMPTS",
                d.float_retry_attempts as i64,
                0,
            )? as u32,
            float_retry_base_secs: num(
                "EVIDENCE_FLOAT_RETRY_BASE_SECS",
                d.float_retry_base_secs,
                1,
            )?,
            float_retry_max_secs: num("EVIDENCE_FLOAT_RETRY_MAX_SECS", d.float_retry_max_secs, 1)?,
            float_retry_queue_max: num(
                "EVIDENCE_FLOAT_RETRY_QUEUE_MAX",
                d.float_retry_queue_max as i64,
                0,
            )? as usize,
            universe_days: num("EVIDENCE_UNIVERSE_DAYS", d.universe_days as i64, 1)? as usize,
            news_poll_secs: num("EVIDENCE_NEWS_POLL_SECS", d.news_poll_secs, 10)?,
            news_idle_poll_secs: num("EVIDENCE_NEWS_IDLE_POLL_SECS", d.news_idle_poll_secs, 10)?,
            news_edit_every_secs: num("EVIDENCE_NEWS_EDIT_EVERY_SECS", d.news_edit_every_secs, 60)?,
            news_edit_window_secs: num(
                "EVIDENCE_NEWS_EDIT_WINDOW_MINS",
                d.news_edit_window_secs / 60,
                1,
            )? * 60,
            alpaca_per_minute: num("EVIDENCE_ALPACA_PER_MINUTE", d.alpaca_per_minute as i64, 1)?
                as u32,
            band: super::maintain::price_band_from_lookup(&get)?,
            data_base: var("ALPACA_DATA_BASE").unwrap_or(d.data_base.clone()),
            fmp_base: var("FMP_BASE").unwrap_or(d.fmp_base.clone()),
            ..d
        })
    }
}

/// Symbols whose close was inside `band` on any of the last `days` dates before `before`.
pub fn watch_universe(
    closes: &DailyCloses,
    band: PriceBand,
    before: NaiveDate,
    days: usize,
) -> BTreeSet<String> {
    closes
        .range(..before)
        .rev()
        .take(days)
        .flat_map(|(_, m)| m.iter())
        .filter(|(_, c)| Price::from_f64_rounded(**c).is_some_and(|p| band.contains(p)))
        .map(|(s, _)| s.clone())
        .collect()
}

/// Today's recorded symbols whose last print is inside `band` and that `known` does not hold.
pub fn intraday_entrants(
    quotes: &[LiveQuote],
    band: PriceBand,
    known: &dyn Fn(&str) -> bool,
) -> Vec<String> {
    let mut out: Vec<String> = quotes
        .iter()
        .filter(|q| q.last.is_some_and(|p| band.contains(p)))
        .filter(|q| !known(&q.symbol))
        .map(|q| q.symbol.clone())
        .collect();
    out.sort();
    out.dedup();
    out
}

fn et_date(t: DateTime<Utc>) -> NaiveDate {
    t.with_timezone(&New_York).date_naive()
}

/// The ET date whose float refresh is due at `now`, if any: a trading day, past `at`, not yet run.
pub fn float_refresh_due(
    now: DateTime<Utc>,
    at: NaiveTime,
    last: Option<NaiveDate>,
) -> Option<NaiveDate> {
    let local = now.with_timezone(&New_York);
    let today = local.date_naive();
    (is_trading_day(today) && local.time() >= at && last != Some(today)).then_some(today)
}

/// 04:00–20:00 ET on a trading day: the extended session, when news moves prices.
pub fn news_active(now: DateTime<Utc>) -> bool {
    let local = now.with_timezone(&New_York);
    let t = local.time();
    is_trading_day(local.date_naive())
        && t >= NaiveTime::from_hms_opt(4, 0, 0).expect("valid")
        && t < NaiveTime::from_hms_opt(20, 0, 0).expect("valid")
}

/// The next forward search window `(start, end)` in UTC ns.
pub fn news_window(cfg: &EvidenceConfig, last_through: Option<i64>, now: i64) -> (i64, i64) {
    let end = now - cfg.news_settle_secs * SEC;
    let start = match last_through {
        Some(t) if end - t <= cfg.news_max_catchup_secs * SEC => t - cfg.news_overlap_secs * SEC,
        // First run, or down for longer than the catch-up limit: the gap stays uncovered.
        _ => end - cfg.news_initial_lookback_secs * SEC,
    };
    (start.min(end), end)
}

/// The end of the contiguous all-symbol coverage that reaches furthest, from stored records.
pub fn last_covered_through(records: &[market_archive::NewsCoverageRecord]) -> Option<i64> {
    records
        .iter()
        .filter(|r| r.scope == CoverageScope::All)
        .map(|r| r.covered_through)
        .max()
}

/// Minimum spacing between requests to one provider. The sleep is injected (tests do not wait).
pub struct Pacer {
    next: Option<Instant>,
    spacing: Duration,
}

impl Pacer {
    pub fn per_minute(n: u32) -> Self {
        Pacer {
            next: None,
            spacing: Duration::from_secs(60) / n.max(1),
        }
    }
    pub fn wait(&mut self, sleep: &mut dyn FnMut(Duration)) {
        let now = Instant::now();
        if let Some(at) = self.next
            && at > now
        {
            sleep(at - now);
        }
        self.next = Some(Instant::now().max(self.next.unwrap_or(now)) + self.spacing);
    }
}

/// What one tick did (one log line).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct TickReport {
    pub float_requests: u32,
    /// Observations **durably appended** this tick: a fetch that could not be stored is not one.
    pub float_records: u32,
    pub float_queued: usize,
    pub warmups: u32,
    /// Symbols scheduled for a transient retry this tick (a provider hiccup or a failed append).
    pub float_retries_scheduled: u32,
    /// Retries whose backoff had elapsed and that were put back on the queue this tick.
    pub float_retries_due: u32,
    /// Symbols that used their last retry and are not asked again today, counted not hidden.
    pub float_retry_exhausted: u32,
    /// Waiting retries dropped because the retry queue was full.
    pub float_retry_dropped: u32,
    /// Symbols waiting for a retry after this tick.
    pub float_retry_pending: usize,
    pub news_requests: u32,
    pub news_versions: u32,
    pub news_complete: u32,
    pub news_through: Option<i64>,
    pub errors: Vec<String>,
}

impl TickReport {
    pub fn summary(&self) -> String {
        format!(
            "evidence: float {} req, {} stored, {} queued, {} warmups, {} retries pending ({} scheduled, {} due, {} exhausted, {} dropped); news {} req, {} new versions, {} complete searches, covered through {}{}",
            self.float_requests,
            self.float_records,
            self.float_queued,
            self.warmups,
            self.float_retry_pending,
            self.float_retries_scheduled,
            self.float_retries_due,
            self.float_retry_exhausted,
            self.float_retry_dropped,
            self.news_requests,
            self.news_versions,
            self.news_complete,
            self.news_through
                .map(|t| DateTime::from_timestamp_nanos(t).to_rfc3339())
                .unwrap_or_else(|| "nothing yet".into()),
            if self.errors.is_empty() {
                String::new()
            } else {
                format!("; errors: {}", self.errors.join(" | "))
            }
        )
    }
}

/// A symbol waiting for another float attempt after a transient failure.
#[derive(Clone, Debug, PartialEq, Eq)]
struct FloatRetry {
    symbol: String,
    /// Earliest UTC ns to ask again.
    next_at: i64,
}

/// FNV-1a, so a retry's jitter is spread across symbols and still reproducible in a test (a
/// random jitter would make the collector's own tests depend on the seed).
fn fnv1a(parts: &[&str], salt: u32) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for p in parts {
        for b in p.as_bytes() {
            h ^= u64::from(*b);
            h = h.wrapping_mul(0x100_0000_01b3);
        }
    }
    for b in salt.to_le_bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x100_0000_01b3);
    }
    h
}

/// The collector's state between ticks. Everything it needs after a restart is rebuilt from the
/// stored records, so a restart repeats no request it already made that day.
pub struct Collector {
    pub cfg: EvidenceConfig,
    versions: NewsVersions,
    last_through: Option<i64>,
    last_edit_poll: Option<i64>,
    next_news_poll: i64,
    float_day: Option<NaiveDate>,
    float_refreshed: Option<NaiveDate>,
    /// Symbols whose float for the day is **durably stored**: the only state that means done.
    /// A fetch that has not been appended yet, or an append that failed, is not in here.
    float_durable: HashSet<String>,
    /// Symbols that spent every retry today: not asked again, and not claimed as done either.
    float_exhausted: HashSet<String>,
    float_queue: VecDeque<String>,
    /// Transient failures waiting for their backoff, oldest wait first.
    float_retry: VecDeque<FloatRetry>,
    /// Requests already spent per symbol today (the first fetch counts as one).
    float_attempts: HashMap<String, u32>,
    float_blocked: Option<NaiveDate>,
    warmups_today: usize,
    fmp_pacer: Pacer,
    alpaca_pacer: Pacer,
}

impl Collector {
    /// State from the archive: news versions and coverage of the last 60 days, today's floats.
    pub fn open(
        cfg: EvidenceConfig,
        archive: &Archive,
        now: DateTime<Utc>,
    ) -> Result<Self, String> {
        let since = (now - Days::days(60)).timestamp_nanos_opt();
        let news = archive.read_news(since).map_err(|e| e.to_string())?;
        let cov = archive
            .read_news_coverage(since)
            .map_err(|e| e.to_string())?;
        let today = et_date(now);
        let floats = archive.read_float().map_err(|e| e.to_string())?;
        // Stored records are durable by definition: a restart neither repeats them nor inherits
        // a pending retry (the symbol is simply queued again at the next due check).
        let float_durable: HashSet<String> = floats
            .iter()
            .filter(|r| et_date(DateTime::from_timestamp_nanos(r.retrieved_at)) == today)
            .map(|r| r.symbol.clone())
            .collect();
        let fmp_pacer = Pacer::per_minute(cfg.fmp_per_minute);
        let alpaca_pacer = Pacer::per_minute(cfg.alpaca_per_minute);
        Ok(Collector {
            versions: NewsVersions::from_records(&news),
            last_through: last_covered_through(&cov),
            last_edit_poll: None,
            next_news_poll: i64::MIN,
            float_day: Some(today),
            // A refresh that already ran today left a record per universe symbol; the queue is
            // rebuilt at the next due check and skips them, so marking it not-run is harmless.
            float_refreshed: None,
            float_durable,
            float_exhausted: HashSet::new(),
            float_queue: VecDeque::new(),
            float_retry: VecDeque::new(),
            float_attempts: HashMap::new(),
            float_blocked: None,
            warmups_today: 0,
            fmp_pacer,
            alpaca_pacer,
            cfg,
        })
    }

    /// One pass: queue/fetch floats (bounded per tick), then a news search when one is due.
    #[allow(clippy::too_many_arguments)]
    pub fn tick(
        &mut self,
        archive: &Archive,
        http: &mut dyn Http,
        fmp_key: Option<&FmpKey>,
        alpaca: Option<&AlpacaCredentials>,
        clock: &mut dyn FnMut() -> DateTime<Utc>,
        sleep: &mut dyn FnMut(Duration),
        log: &mut dyn FnMut(&str),
    ) -> TickReport {
        let mut report = TickReport::default();
        let now = clock();
        if let Some(key) = fmp_key {
            self.floats(archive, http, key, now, clock, sleep, log, &mut report);
        }
        if let Some(creds) = alpaca {
            self.news(archive, http, creds, now, clock, sleep, log, &mut report);
        }
        report.float_queued = self.float_queue.len();
        report.float_retry_pending = self.float_retry.len();
        report.news_through = self.last_through;
        report
    }

    #[allow(clippy::too_many_arguments)]
    fn floats(
        &mut self,
        archive: &Archive,
        http: &mut dyn Http,
        key: &FmpKey,
        now: DateTime<Utc>,
        clock: &mut dyn FnMut() -> DateTime<Utc>,
        sleep: &mut dyn FnMut(Duration),
        log: &mut dyn FnMut(&str),
        report: &mut TickReport,
    ) {
        let today = et_date(now);
        if self.float_day != Some(today) {
            self.float_day = Some(today);
            self.float_durable.clear();
            self.float_exhausted.clear();
            self.float_queue.clear();
            self.float_retry.clear();
            self.float_attempts.clear();
            self.warmups_today = 0;
        }
        if self.float_blocked == Some(today) {
            return;
        }
        let tick_now_ns = now.timestamp_nanos_opt().unwrap_or(i64::MAX);
        let mut universe = BTreeSet::new();
        if let Some(day) = float_refresh_due(now, self.cfg.float_at, self.float_refreshed) {
            self.float_refreshed = Some(day);
            match archive.read_daily_closes() {
                Ok(Some(closes)) => {
                    universe = watch_universe(&closes, self.cfg.band, day, self.cfg.universe_days);
                }
                Ok(None) => report
                    .errors
                    .push("no daily-closes.json: watch universe empty".into()),
                Err(e) => report.errors.push(format!("daily closes: {e}")),
            }
            log(&format!(
                "float refresh for {day}: {} symbols in the watch universe",
                universe.len()
            ));
            if self.cfg.float_mode == FloatMode::Bulk && !universe.is_empty() {
                let want = |s: &str| universe.contains(s);
                let mut now_ns = || clock().timestamp_nanos_opt().unwrap_or(i64::MAX);
                match fetch_float_all(
                    http,
                    &self.cfg.fmp_base,
                    key,
                    &want,
                    RetryPolicy::default(),
                    &mut now_ns,
                    sleep,
                    log,
                ) {
                    Ok((rows, requests)) => {
                        report.float_requests += requests;
                        self.store_floats(archive, rows, tick_now_ns, report);
                    }
                    Err(e) => {
                        report
                            .errors
                            .push(format!("float bulk: {e}; per-symbol instead"));
                    }
                }
            }
            for s in &universe {
                if !self.known_float(s) {
                    self.float_queue.push_back(s.clone());
                }
            }
        }
        // Retries whose backoff has elapsed go to the front of the queue: each is one symbol
        // already judged worth asking about, and the daily refresh queue is thousands long.
        let due: Vec<FloatRetry> = {
            let mut due = Vec::new();
            let mut keep = VecDeque::with_capacity(self.float_retry.len());
            for r in self.float_retry.drain(..) {
                if r.next_at <= tick_now_ns {
                    due.push(r);
                } else {
                    keep.push_back(r);
                }
            }
            self.float_retry = keep;
            due
        };
        report.float_retries_due += due.len() as u32;
        for r in due.into_iter().rev() {
            if !self.float_durable.contains(&r.symbol) {
                self.float_queue.push_front(r.symbol);
            }
        }
        // Lazy warmup: intraday entrants jump the queue, capped per day.
        if let Ok(Some(quotes)) = archive.read_live_quotes(today) {
            let known = |s: &str| self.known_float(s);
            let entrants = intraday_entrants(&quotes, self.cfg.band, &known);
            for s in entrants {
                if self.warmups_today >= self.cfg.float_warmup_max {
                    break;
                }
                self.warmups_today += 1;
                report.warmups += 1;
                self.float_queue.push_front(s);
            }
        }
        // Half a minute's budget per tick, so a 7,000-symbol refresh never holds up news.
        let budget = (self.cfg.fmp_per_minute / 2).max(1);
        let mut rows: Vec<FloatRecord> = Vec::new();
        for _ in 0..budget {
            let Some(sym) = self.float_queue.pop_front() else {
                break;
            };
            self.fmp_pacer.wait(sleep);
            report.float_requests += 1;
            let attempts = self
                .float_attempts
                .entry(sym.clone())
                .and_modify(|n| *n += 1)
                .or_insert(1);
            let attempts = *attempts;
            let mut now_ns = || clock().timestamp_nanos_opt().unwrap_or(i64::MAX);
            match fetch_float(
                http,
                &self.cfg.fmp_base,
                key,
                &sym,
                RetryPolicy::default(),
                &mut now_ns,
                sleep,
                log,
            ) {
                // Fetched, not done: the symbol is complete only once `store_floats` has
                // appended it durably.
                Ok(r) => rows.push(r),
                Err(FmpError::NotEntitled { status, message }) => {
                    // A plan refusal will not change within the day: stop asking until tomorrow,
                    // pending retries included (a provider that said no is not a hiccup).
                    report.errors.push(format!(
                        "float not entitled (HTTP {status}): {message}; float paused until tomorrow"
                    ));
                    self.float_blocked = Some(today);
                    self.float_queue.clear();
                    self.float_retry.clear();
                    break;
                }
                Err(e) => {
                    report.errors.push(format!("float {sym}: {e}"));
                    self.schedule_retry(&sym, attempts, now_ns(), report);
                }
            }
        }
        self.store_floats(archive, rows, tick_now_ns, report);
    }

    /// Queues another attempt for `sym` after an exponential, jittered backoff, unless it has
    /// spent its allowance for the day (counted, never silently dropped).
    ///
    /// A genuine refusal is handled elsewhere and still pauses the whole day: this path is for
    /// transient failures only — exhausted HTTP retries, a network error, a failed append.
    fn schedule_retry(&mut self, sym: &str, attempts: u32, now_ns: i64, report: &mut TickReport) {
        if attempts > self.cfg.float_retry_attempts {
            self.float_exhausted.insert(sym.to_string());
            report.float_retry_exhausted += 1;
            return;
        }
        if self.float_retry.iter().any(|r| r.symbol == sym) {
            return;
        }
        let step = self
            .cfg
            .float_retry_base_secs
            .saturating_mul(1i64 << attempts.min(16).saturating_sub(1))
            .min(self.cfg.float_retry_max_secs);
        // Up to a quarter of the step, spread by symbol so a provider outage does not bring the
        // whole queue back at the same instant.
        let jitter = (fnv1a(&[sym], attempts) % (step / 4 + 1) as u64) as i64;
        if self.cfg.float_retry_queue_max == 0 {
            report.float_retry_dropped += 1;
            return;
        }
        while self.float_retry.len() >= self.cfg.float_retry_queue_max {
            self.float_retry.pop_front();
            report.float_retry_dropped += 1;
        }
        self.float_retry.push_back(FloatRetry {
            symbol: sym.to_string(),
            next_at: now_ns + (step + jitter) * SEC,
        });
        report.float_retries_scheduled += 1;
    }

    /// Appends `rows` and marks a symbol done **only** when the append succeeded. An append that
    /// fails leaves the observation unrecorded, so the symbol is scheduled for another attempt
    /// rather than counted as collected.
    fn store_floats(
        &mut self,
        archive: &Archive,
        rows: Vec<FloatRecord>,
        now_ns: i64,
        report: &mut TickReport,
    ) {
        if rows.is_empty() {
            return;
        }
        match archive.append_float(&rows) {
            Ok(()) => {
                for r in &rows {
                    self.float_durable.insert(r.symbol.clone());
                }
                report.float_records += rows.len() as u32;
            }
            Err(e) => {
                report.errors.push(format!("float store: {e}"));
                for r in &rows {
                    let attempts = self
                        .float_attempts
                        .get(&r.symbol)
                        .copied()
                        .unwrap_or(1)
                        .max(1);
                    self.schedule_retry(&r.symbol, attempts, now_ns, report);
                }
            }
        }
    }

    /// Whether this symbol's float needs no new queue entry today: stored, queued, waiting for a
    /// retry, or out of attempts.
    fn known_float(&self, sym: &str) -> bool {
        self.float_durable.contains(sym)
            || self.float_exhausted.contains(sym)
            || self.float_queue.iter().any(|q| q == sym)
            || self.float_retry.iter().any(|r| r.symbol == sym)
    }

    #[allow(clippy::too_many_arguments)]
    fn news(
        &mut self,
        archive: &Archive,
        http: &mut dyn Http,
        creds: &AlpacaCredentials,
        now: DateTime<Utc>,
        clock: &mut dyn FnMut() -> DateTime<Utc>,
        sleep: &mut dyn FnMut(Duration),
        log: &mut dyn FnMut(&str),
        report: &mut TickReport,
    ) {
        let now_ns = now.timestamp_nanos_opt().unwrap_or(i64::MAX);
        if now_ns < self.next_news_poll {
            return;
        }
        let interval = if news_active(now) {
            self.cfg.news_poll_secs
        } else {
            self.cfg.news_idle_poll_secs
        };
        self.next_news_poll = now_ns + interval * SEC;
        let mut windows = vec![(news_window(&self.cfg, self.last_through, now_ns), true)];
        if self
            .last_edit_poll
            .is_none_or(|t| now_ns - t >= self.cfg.news_edit_every_secs * SEC)
            && self.last_through.is_some()
        {
            self.last_edit_poll = Some(now_ns);
            let end = now_ns - self.cfg.news_settle_secs * SEC;
            windows.push(((end - self.cfg.news_edit_window_secs * SEC, end), false));
        }
        let sleep = RefCell::new(sleep);
        for ((start, end), forward) in windows {
            let q = NewsQuery {
                start: Utc.timestamp_nanos(start),
                end: Utc.timestamp_nanos(end),
                symbols: None,
            };
            let mut now_ns = || clock().timestamp_nanos_opt().unwrap_or(i64::MAX);
            let mut paced = PacedHttp {
                inner: &mut *http,
                pacer: &mut self.alpaca_pacer,
                sleep: &sleep,
            };
            let f = fetch_news(
                &mut paced,
                &self.cfg.data_base,
                creds,
                &q,
                RetryPolicy::default(),
                &mut now_ns,
                &mut |d| (sleep.borrow_mut())(d),
                log,
            );
            report.news_requests += f.requests;
            // Versions first, then the coverage they complete: a crash in between leaves stories
            // without a coverage claim (UNKNOWN), never a claim without its stories (a false FAIL).
            let new = self.versions.observe(&f.stories);
            if let Err(e) = archive.append_news(&new) {
                report.errors.push(format!("news store: {e}"));
                continue;
            }
            report.news_versions += new.len() as u32;
            if let Some(e) = &f.error {
                report.errors.push(format!("news search: {e}"));
            }
            if let Some(c) = coverage_record(&q, &f) {
                match archive.append_news_coverage(std::slice::from_ref(&c)) {
                    Ok(()) => {
                        report.news_complete += 1;
                        if forward || self.last_through.is_some_and(|t| c.covered_from <= t) {
                            self.last_through = Some(
                                self.last_through
                                    .map_or(c.covered_through, |t| t.max(c.covered_through)),
                            );
                        }
                    }
                    Err(e) => report.errors.push(format!("coverage store: {e}")),
                }
            }
        }
    }
}

/// The Alpaca pacer in front of every page request (retries included).
struct PacedHttp<'a, 's> {
    inner: &'a mut dyn Http,
    pacer: &'a mut Pacer,
    sleep: &'a RefCell<&'s mut dyn FnMut(Duration)>,
}

impl Http for PacedHttp<'_, '_> {
    fn get(
        &mut self,
        url: &str,
        headers: &[(&str, &str)],
    ) -> Result<super::http::HttpResponse, String> {
        self.pacer.wait(&mut **self.sleep.borrow_mut());
        self.inner.get(url, headers)
    }
}

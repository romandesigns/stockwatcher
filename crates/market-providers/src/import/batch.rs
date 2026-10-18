//! Batch import: every symbol that was inside the price band on each day of a date range.
//!
//! 1. **Universe.** Active tradable US equities from Alpaca's assets endpoint, then their daily
//!    bars (`/v2/stocks/bars`, `timeframe=1Day`, raw, SIP). A `(day, symbol)` is in the plan when
//!    that day's close is inside the band. The server decides membership by the latest
//!    price-qualifying trade; the daily close is the only price available before downloading, so
//!    it stands in for it. Each bar's trade count `n` sizes the work.
//! 2. **Import.** Workers take tasks busiest-first and run the single-day importer
//!    ([`super::alpaca::import_trades`]) for each, through one shared rate limiter. Every guarantee
//!    of the single-day import holds per task: a complete identical import is skipped, a failure
//!    leaves no manifest entry. The run collects failures instead of stopping, so re-running the
//!    same command retries exactly what is missing.
//!
//! The same assets list also gives the archive's display metadata ([`asset_metadata`], written
//! to `$ARCHIVE_ROOT/assets.json` by `swimport alpaca-assets`): each symbol's name and listing
//! exchange as Alpaca reports them, for the ticker search. And the same daily bars give the
//! official closes ([`daily_closes`], merged into `$ARCHIVE_ROOT/daily-closes.json` by
//! `swimport alpaca-daily-closes`) that the server quotes change % against.

use super::alpaca::{
    AlpacaCredentials, Feed, ImportError, ImportOptions, TradesQuery, import_trades,
};
use super::http::{FetchError, Http, HttpResponse, RetryPolicy, get_with_retry};
use chrono::{DateTime, NaiveDate, Utc};
use chrono_tz::America::New_York;
use market_archive::{Archive, AssetMap, AssetMeta, DailyCloses};
use market_events::session::sessions_for;
use market_events::{PriceBand, SessionKind};
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// One unit of work: a symbol on a trading date, with the day's trade count from its daily bar.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Task {
    pub day: NaiveDate,
    pub symbol: String,
    pub close: f64,
    pub trades: u64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Plan {
    pub from: NaiveDate,
    pub to: NaiveDate,
    pub band_min: String,
    pub band_max: String,
    pub assets: usize,
    pub tasks: Vec<Task>,
    /// Symbols the archive cannot store as a file name (reported, never silently dropped).
    pub skipped_symbols: Vec<String>,
}

impl Plan {
    pub fn trades(&self) -> u64 {
        self.tasks.iter().map(|t| t.trades).sum()
    }
    /// Requests needed: one per 10,000 trades, at least one per task.
    pub fn requests(&self, limit: u32) -> u64 {
        self.tasks
            .iter()
            .map(|t| t.trades.div_ceil(u64::from(limit.max(1))).max(1))
            .sum()
    }
}

/// One entry of Alpaca's `/v2/assets` (only the fields used here).
#[derive(Deserialize)]
struct Asset {
    symbol: String,
    #[serde(default)]
    tradable: bool,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    exchange: Option<String>,
}

#[derive(Deserialize)]
struct DailyBar {
    t: String,
    c: f64,
    #[serde(default)]
    n: u64,
}

#[derive(Deserialize)]
struct BarsPage {
    #[serde(default)]
    bars: Option<std::collections::BTreeMap<String, Vec<DailyBar>>>,
    #[serde(default)]
    next_page_token: Option<String>,
}

fn alpaca_headers(c: &AlpacaCredentials) -> [(&str, &str); 3] {
    [
        ("APCA-API-KEY-ID", c.key_id.as_str()),
        ("APCA-API-SECRET-KEY", c.secret.as_str()),
        ("Accept", "application/json"),
    ]
}

/// Builds the plan from the assets list and daily bars. `symbols_per_request` keeps URLs short.
#[allow(clippy::too_many_arguments)]
pub fn plan_universe(
    http: &mut dyn Http,
    trading_base: &str,
    data_base: &str,
    creds: &AlpacaCredentials,
    from: NaiveDate,
    to: NaiveDate,
    band: PriceBand,
    log: &mut dyn FnMut(&str),
) -> Result<Plan, String> {
    let assets = fetch_assets(http, trading_base, creds, log)?;
    let mut symbols: Vec<String> = assets
        .into_iter()
        .filter(|a| a.tradable)
        .map(|a| a.symbol)
        .collect();
    symbols.sort();
    symbols.dedup();
    log(&format!("{} active tradable US equities", symbols.len()));

    let mut plan = Plan {
        from,
        to,
        band_min: band.min.to_string(),
        band_max: band.max.to_string(),
        assets: symbols.len(),
        ..Plan::default()
    };
    let (lo, hi) = (band.min.to_f64(), band.max.to_f64());
    for_each_daily_bar(
        http,
        data_base,
        creds,
        &symbols,
        from,
        to,
        log,
        &mut |symbol, day, b| {
            if !(lo..=hi).contains(&b.c) {
                return;
            }
            if !market_archive::valid_symbol(symbol) {
                if !plan.skipped_symbols.iter().any(|s| s == symbol) {
                    plan.skipped_symbols.push(symbol.to_string());
                }
                return;
            }
            plan.tasks.push(Task {
                day,
                symbol: symbol.to_string(),
                close: b.c,
                trades: b.n,
            });
        },
    )?;
    // Busiest first, so the long downloads start early and the tail is short tasks.
    plan.tasks.sort_by(|a, b| {
        b.trades
            .cmp(&a.trades)
            .then(a.day.cmp(&b.day))
            .then(a.symbol.cmp(&b.symbol))
    });
    Ok(plan)
}

/// Every daily bar of `symbols` over `from..=to` (ET trading dates), in chunks of 200 symbols per
/// request (keeps URLs short) and following `next_page_token`. `on_bar` gets each bar with its
/// trading date; a bar stamped outside the range or with an unparseable `t` is skipped.
///
/// `adjustment=raw` on purpose, for both callers: the archive's prints are as traded that day, so
/// a band test or a change-% reference must be in the same units. A split-adjusted close would
/// also change retroactively every time a later split is announced, so two fetches of the same
/// day would disagree and a merged `daily-closes.json` would mix bases. The cost is one false move
/// on an ex-split date (yesterday's raw close in pre-split units), which is visible and rare.
#[allow(clippy::too_many_arguments)]
fn for_each_daily_bar(
    http: &mut dyn Http,
    data_base: &str,
    creds: &AlpacaCredentials,
    symbols: &[String],
    from: NaiveDate,
    to: NaiveDate,
    log: &mut dyn FnMut(&str),
    on_bar: &mut dyn FnMut(&str, NaiveDate, &DailyBar),
) -> Result<(), String> {
    let headers = alpaca_headers(creds);
    let secrets = [creds.key_id.as_str(), creds.secret.as_str()];
    let retry = RetryPolicy::default();
    let mut sleep = |d: Duration| std::thread::sleep(d);
    for chunk in symbols.chunks(200) {
        let mut token: Option<String> = None;
        loop {
            let mut u = format!(
                "{}/v2/stocks/bars?symbols={}&timeframe=1Day&start={from}&end={to}&feed=sip&adjustment=raw&limit=10000",
                data_base.trim_end_matches('/'),
                chunk.join("%2C")
            );
            if let Some(t) = &token {
                u.push_str("&page_token=");
                u.push_str(t);
            }
            let resp = get_with_retry(http, &u, &headers, retry, &secrets, &mut sleep, log)
                .map_err(|e| format!("daily bars: {e}"))?;
            let page: BarsPage =
                serde_json::from_slice(&resp.body).map_err(|e| format!("daily bars: {e}"))?;
            for (symbol, bars) in page.bars.unwrap_or_default() {
                for b in bars {
                    let Some(day) = bar_day(&b.t) else { continue };
                    if day < from || day > to {
                        continue;
                    }
                    on_bar(&symbol, day, &b);
                }
            }
            match page.next_page_token.filter(|t| !t.is_empty()) {
                Some(t) if token.as_deref() != Some(t.as_str()) => token = Some(t),
                Some(_) => return Err("daily bars: repeated page token".into()),
                None => break,
            }
        }
    }
    Ok(())
}

/// The official daily close of every active US equity on each trading date of `from..=to`, for
/// `$ARCHIVE_ROOT/daily-closes.json` (`swimport alpaca-daily-closes`; format and use in
/// market_archive::closes). The close is the `c` of Alpaca's SIP `1Day` bar, raw (see
/// [`for_each_daily_bar`] for why raw): the consolidated regular-session close, which is the
/// reference brokers quote "today's change" against.
///
/// Every active asset, tradable or not, like [`asset_metadata`]: a symbol that is not tradable
/// today may still be in an archived day, and a missing close only costs that row its reference.
/// The symbol is kept as Alpaca spells it (no file-name check: it is a JSON key, never a path).
/// A close that is not a finite positive number is left out rather than stored as a reference.
pub fn daily_closes(
    http: &mut dyn Http,
    trading_base: &str,
    data_base: &str,
    creds: &AlpacaCredentials,
    from: NaiveDate,
    to: NaiveDate,
    log: &mut dyn FnMut(&str),
) -> Result<DailyCloses, String> {
    let mut symbols: Vec<String> = fetch_assets(http, trading_base, creds, log)?
        .into_iter()
        .map(|a| a.symbol.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    symbols.sort();
    symbols.dedup();
    log(&format!("{} active US equities", symbols.len()));
    let mut out = DailyCloses::new();
    let mut bad = 0usize;
    for_each_daily_bar(
        http,
        data_base,
        creds,
        &symbols,
        from,
        to,
        log,
        &mut |symbol, day, b| {
            if b.c.is_finite() && b.c > 0.0 {
                out.entry(day).or_default().insert(symbol.to_string(), b.c);
            } else {
                bad += 1;
            }
        },
    )?;
    let n: usize = out.values().map(|m| m.len()).sum();
    log(&format!(
        "{n} closes over {} trading dates ({bad} non-positive closes left out)",
        out.len()
    ));
    Ok(out)
}

/// Active US equities from Alpaca's trading API (`/v2/assets`), one request, with the shared
/// retry policy. Credentials go only into headers and are scrubbed from any logged error.
fn fetch_assets(
    http: &mut dyn Http,
    trading_base: &str,
    creds: &AlpacaCredentials,
    log: &mut dyn FnMut(&str),
) -> Result<Vec<Asset>, String> {
    let headers = alpaca_headers(creds);
    let secrets = [creds.key_id.as_str(), creds.secret.as_str()];
    let mut sleep = |d: Duration| std::thread::sleep(d);
    let url = format!(
        "{}/v2/assets?status=active&asset_class=us_equity",
        trading_base.trim_end_matches('/')
    );
    let resp = get_with_retry(
        http,
        &url,
        &headers,
        RetryPolicy::default(),
        &secrets,
        &mut sleep,
        log,
    )
    .map_err(|e| format!("assets: {e}"))?;
    serde_json::from_slice(&resp.body).map_err(|e| format!("assets: {e}"))
}

/// Name and listing exchange for every active US equity, tradable or not (a symbol that is not
/// tradable today may still be in an archived day). Trimmed as delivered and never made up: an
/// asset whose name and exchange are both empty is left out, so the server falls back to the
/// symbol instead of showing a blank. A symbol listed twice keeps its first entry.
pub fn asset_metadata(
    http: &mut dyn Http,
    trading_base: &str,
    creds: &AlpacaCredentials,
    log: &mut dyn FnMut(&str),
) -> Result<AssetMap, String> {
    let mut out = AssetMap::new();
    let mut blank = 0usize;
    for a in fetch_assets(http, trading_base, creds, log)? {
        let name = a.name.as_deref().unwrap_or("").trim().to_string();
        let exchange = a.exchange.as_deref().unwrap_or("").trim().to_string();
        if name.is_empty() && exchange.is_empty() {
            blank += 1;
            continue;
        }
        out.entry(a.symbol.trim().to_string())
            .or_insert(AssetMeta { name, exchange });
    }
    log(&format!(
        "{} active US equities with metadata ({blank} without name or exchange, left out)",
        out.len()
    ));
    Ok(out)
}

/// A daily bar's `t` (midnight ET as UTC) → its ET trading date.
fn bar_day(t: &str) -> Option<NaiveDate> {
    DateTime::parse_from_rfc3339(t)
        .ok()
        .map(|d| d.with_timezone(&New_York).date_naive())
}

/// One request at a time through a minimum spacing, shared by every worker.
#[derive(Clone)]
pub struct RateLimiter {
    next: Arc<Mutex<Instant>>,
    spacing: Duration,
}

impl RateLimiter {
    pub fn per_minute(n: u32) -> Self {
        RateLimiter {
            next: Arc::new(Mutex::new(Instant::now())),
            spacing: Duration::from_secs(60) / n.max(1),
        }
    }
    fn wait(&self) {
        let at = {
            let mut next = self.next.lock().unwrap_or_else(|p| p.into_inner());
            let now = Instant::now();
            let at = (*next).max(now);
            *next = at + self.spacing;
            at
        };
        let now = Instant::now();
        if at > now {
            std::thread::sleep(at - now);
        }
    }
}

/// Wraps a worker's client so every request first passes the shared limiter, and counts it.
struct Limited<H> {
    inner: H,
    limiter: RateLimiter,
    requests: Arc<AtomicU64>,
}

impl<H: Http> Http for Limited<H> {
    fn get(&mut self, url: &str, headers: &[(&str, &str)]) -> Result<HttpResponse, String> {
        self.limiter.wait();
        self.requests.fetch_add(1, Ordering::Relaxed);
        self.inner.get(url, headers)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Failure {
    pub day: NaiveDate,
    pub symbol: String,
    pub error: String,
}

#[derive(Debug, Default)]
pub struct BatchReport {
    pub tasks: u64,
    pub imported: u64,
    pub skipped: u64,
    pub failed: Vec<Failure>,
    pub events: u64,
    pub requests: u64,
    pub elapsed: Duration,
}

/// The SIP range for a trading date: premarket open through the end of after hours.
pub fn sip_range(day: NaiveDate) -> Option<(DateTime<Utc>, DateTime<Utc>)> {
    let s: Vec<_> = sessions_for(day)
        .into_iter()
        .filter(|(id, ..)| {
            matches!(
                id.kind,
                SessionKind::Pre | SessionKind::Regular | SessionKind::After
            )
        })
        .collect();
    Some((s.first()?.1.to_utc(), s.last()?.2.to_utc()))
}

/// Runs the plan with `jobs` workers. `make_http` builds one client per worker. `progress` is
/// called about every `every` with a one-line status; `log` gets retries and failures.
#[allow(clippy::too_many_arguments)]
pub fn run_batch<H: Http + Send + 'static>(
    plan: &Plan,
    make_http: &(dyn Fn() -> Result<H, String> + Sync),
    data_base: &str,
    creds: &AlpacaCredentials,
    archive: &Archive,
    jobs: usize,
    per_minute: u32,
    every: Duration,
    progress: &(dyn Fn(&str) + Sync),
    log: &(dyn Fn(&str) + Sync),
) -> BatchReport {
    let started = Instant::now();
    let total_trades = plan.trades().max(1);
    let next = AtomicU64::new(0);
    let done = AtomicU64::new(0);
    let imported = AtomicU64::new(0);
    let skipped = AtomicU64::new(0);
    let events = AtomicU64::new(0);
    let trades_done = AtomicU64::new(0);
    let requests = Arc::new(AtomicU64::new(0));
    let failed: Mutex<Vec<Failure>> = Mutex::new(Vec::new());
    let limiter = RateLimiter::per_minute(per_minute);
    let finished = std::sync::atomic::AtomicBool::new(false);
    let n_tasks = plan.tasks.len() as u64;

    std::thread::scope(|scope| {
        // Progress reporter.
        scope.spawn(|| {
            let mut last = Instant::now();
            while !finished.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(250));
                if last.elapsed() < every {
                    continue;
                }
                last = Instant::now();
                let el = started.elapsed().as_secs_f64();
                let frac = trades_done.load(Ordering::Relaxed) as f64 / total_trades as f64;
                let eta = if frac > 0.0 { el / frac - el } else { f64::NAN };
                progress(&format!(
                    "{}/{} tasks ({} imported, {} skipped, {} failed)  {:.1}% of trades  {} events  {} requests ({:.0}/min)  elapsed {}  eta {}",
                    done.load(Ordering::Relaxed),
                    n_tasks,
                    imported.load(Ordering::Relaxed),
                    skipped.load(Ordering::Relaxed),
                    failed.lock().map(|f| f.len()).unwrap_or(0),
                    frac * 100.0,
                    events.load(Ordering::Relaxed),
                    requests.load(Ordering::Relaxed),
                    requests.load(Ordering::Relaxed) as f64 / el.max(1.0) * 60.0,
                    hms(el),
                    if eta.is_finite() { hms(eta) } else { "-".into() }
                ));
            }
        });
        let workers: Vec<_> = (0..jobs.max(1))
            .map(|_| {
                let limiter = limiter.clone();
                let requests = requests.clone();
                let (next, done, imported, skipped, events, trades_done, failed) = (
                    &next,
                    &done,
                    &imported,
                    &skipped,
                    &events,
                    &trades_done,
                    &failed,
                );
                scope.spawn(move || {
                    let inner = match make_http() {
                        Ok(h) => h,
                        Err(e) => {
                            log(&format!("worker could not start: {e}"));
                            return;
                        }
                    };
                    let mut http = Limited {
                        inner,
                        limiter,
                        requests,
                    };
                    loop {
                        let i = next.fetch_add(1, Ordering::Relaxed) as usize;
                        let Some(task) = plan.tasks.get(i) else { break };
                        let outcome = match sip_range(task.day) {
                            None => Err(format!("{} has no SIP sessions", task.day)),
                            Some((start, end)) => {
                                let q = TradesQuery {
                                    symbol: task.symbol.clone(),
                                    day: task.day,
                                    start,
                                    end,
                                    feed: Feed::Sip,
                                    limit: super::alpaca::MAX_LIMIT,
                                };
                                let label = format!("{} {}", task.day, task.symbol);
                                import_trades(
                                    &mut http,
                                    data_base,
                                    creds,
                                    &q,
                                    archive,
                                    &ImportOptions::default(),
                                    &mut |d| std::thread::sleep(d),
                                    &mut |l: &str| {
                                        if l.contains("retry") {
                                            log(&format!("{label}: {l}"));
                                        }
                                    },
                                )
                                .map_err(|e| describe(&e))
                            }
                        };
                        match outcome {
                            Ok(r) => {
                                events.fetch_add(r.events, Ordering::Relaxed);
                                if r.skipped {
                                    skipped.fetch_add(1, Ordering::Relaxed);
                                } else {
                                    imported.fetch_add(1, Ordering::Relaxed);
                                }
                            }
                            Err(error) => {
                                log(&format!("FAILED {} {}: {error}", task.day, task.symbol));
                                failed
                                    .lock()
                                    .unwrap_or_else(|p| p.into_inner())
                                    .push(Failure {
                                        day: task.day,
                                        symbol: task.symbol.clone(),
                                        error,
                                    });
                            }
                        }
                        trades_done.fetch_add(task.trades.max(1), Ordering::Relaxed);
                        done.fetch_add(1, Ordering::Relaxed);
                    }
                })
            })
            .collect();
        for w in workers {
            let _ = w.join();
        }
        finished.store(true, Ordering::Relaxed);
    });

    BatchReport {
        tasks: n_tasks,
        imported: imported.into_inner(),
        skipped: skipped.into_inner(),
        failed: failed.into_inner().unwrap_or_else(|p| p.into_inner()),
        events: events.into_inner(),
        requests: requests.load(Ordering::Relaxed),
        elapsed: started.elapsed(),
    }
}

fn describe(e: &ImportError) -> String {
    match e {
        ImportError::Fetch {
            error: FetchError::Permanent { status, .. },
            page,
        } => format!("page {page}: HTTP {status}"),
        other => other.to_string(),
    }
}

fn hms(secs: f64) -> String {
    let s = secs.max(0.0) as u64;
    format!("{}h{:02}m{:02}s", s / 3600, s / 60 % 60, s % 60)
}

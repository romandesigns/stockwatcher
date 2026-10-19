//! The rolling replay archive: always the last 21 SIP trading days (a trading month), kept current by
//! `swimport maintain`, a long-running process beside the server (the `archiver` compose service
//! on the VPS, same image, same volume).
//!
//! # The window
//!
//! Let **L** be the latest *completed* trading day: today in New York if it is a trading day and
//! the clock has passed the daily run time (`MAINTAIN_AT`, 21:15 ET by default: the extended
//! session ends at 20:00 and Alpaca's historical SIP is final shortly after), else the trading day
//! before. The window is the last `ARCHIVE_WINDOW_DAYS` trading days ending at L (default 21:
//! 252 trading days a year / 12, the market's "one month").
//!
//! A fixed count of sessions, not a calendar month, by the owner's decision (2026-10-18): each
//! completed trading day brings one day in and pushes exactly one out, the archive's size and disk
//! use stay flat (~0.6 GB a day), and a month that happens to hold holidays does not quietly offer
//! fewer sessions to replay. A calendar month drifted between 19 and 23 days and could drop two
//! days at once around a month end. Anchoring on L rather than on today keeps the window the same
//! over a weekend or a holiday: nothing comes in, so nothing goes out. Example: on Sunday
//! 2026-10-18, L = Friday 10-16 and the window is 2026-09-18 ..= 2026-10-16; the VPS was seeded
//! with 22 days from 09-17, so the first cycle prunes 09-17 (unless it is being replayed).
//!
//! # A cycle
//!
//! Once at start, then daily at `MAINTAIN_AT` ET ([`next_run`], DST-correct; weekends too, so a
//! Friday that failed is retried over the weekend rather than on Monday night):
//!
//! 1. Finish any interrupted day removal ([`Archive::sweep_removed`]).
//! 2. Refresh `assets.json` and `daily-closes.json` (the window plus the trading day before its
//!    first day; dates older than the earliest day still archived, minus one, are trimmed). Each
//!    is written atomically and only when it changed, so the server's mtime check reloads it only
//!    when there is something new.
//! 3. Disk guard: below the archive's free-space floor (`ARCHIVE_MIN_FREE_MB`) nothing is
//!    imported; old days are pruned first (if allowed, step 5) and the floor checked again.
//! 4. Import: plan the window's universe from Alpaca's daily bars (same band and rules as
//!    `swimport alpaca-batch`; ~60 requests for a month), drop every symbol-day already complete
//!    in the manifests ([`pending_tasks`]: each manifest is read once, so the 150k-task window
//!    costs ~22 manifest reads, not 150k), and import the rest with the batch importer's workers
//!    and rate limiter. Failed symbol-days are retried `MAINTAIN_RETRIES` times with doubling
//!    delays inside the cycle and then on the next cycle; complete ones are never re-fetched.
//!    A day that had no manifest yet is marked `.importing` first (market_archive::lifecycle), so
//!    the server neither offers it nor starts on it until its first pass is over. A day the live
//!    market recorded (`<day>/live/`, market_archive::live) is such a day: the marker also stops
//!    the recorder from writing to it, and clearing it ([`Archive::finish_import`]) is the atomic
//!    switch from the provisional recording to the import, which then deletes the recording.
//! 5. Prune day directories older than the window, but only when all of these hold:
//!    - the archive is caught up (its newest ready day is L), or the disk is below the floor. An
//!      import that is failing (no keys yet, Alpaca down) must not turn into a window that shrinks
//!      by a day every evening until nothing is left: old days are irreversible to lose and the
//!      disk has room for a few extra;
//!    - the server answered `GET /api/replay/state`. The day it is replaying is never deleted. If
//!      it cannot be asked (down, restarting, wrong token), nothing is deleted this cycle.
//! 6. Write `plans/maintainer-status.json` and log one line.
//!
//! SIGTERM stops the batch from taking new tasks; running ones finish and commit (each commit is
//! a temp file + rename, so a manifest is never half-written, and an import killed outright leaves
//! only a `.partial` file and no manifest entry). An unfinished day keeps its marker.

use super::alpaca::{AlpacaCredentials, Feed, TradesQuery};
use super::batch::{
    Failure, Plan, Task, asset_metadata, daily_closes, plan_universe, run_batch_until, sip_range,
};
use super::http::{Http, HttpResponse};
use chrono::{DateTime, NaiveDate, NaiveTime, SecondsFormat, TimeZone, Utc};
use chrono_tz::America::New_York;
use market_archive::{Archive, DailyCloses, merge_daily_closes};
use market_events::{Price, PriceBand, is_trading_day};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// Status file, under the archive's `plans/`.
pub const STATUS_FILE: &str = "maintainer-status.json";
/// Every failure of the last cycle (the status file carries a sample).
pub const FAILURES_FILE: &str = "maintainer-failures.json";
/// A new day's marker is cleared after its first pass when at most this share of its planned
/// symbol-days failed (per mille). A symbol Alpaca refuses outright (403/404) would otherwise keep
/// a whole day hidden forever; a day where Alpaca was down for half the run stays hidden.
pub const MAX_FAILED_PER_MILLE: usize = 10;

/// Environment of `swimport maintain` (credentials and `ARCHIVE_*` are read elsewhere).
///
/// | Variable | Default | Meaning |
/// |---|---|---|
/// | `ARCHIVE_WINDOW_DAYS` | `21` | window length in trading days (module docs), 1–504 |
/// | `MAINTAIN_AT` | `21:15` | ET wall time of the daily cycle (`HH:MM`) |
/// | `MAINTAIN_JOBS` | `12` | parallel import workers |
/// | `MAINTAIN_PER_MINUTE` | `9000` | request ceiling across all workers (plan limit is 10,000) |
/// | `MAINTAIN_RETRIES` | `3` | extra passes over failed symbol-days within one cycle |
/// | `MAINTAIN_RETRY_DELAY_SECS` | `120` | delay before the first extra pass; doubles each pass |
/// | `STOCKWATCHER_SERVER_URL` | `http://127.0.0.1:8790` | the server to ask which day it replays |
/// | `STOCKWATCHER_API_TOKEN` | none | its bearer token (the same `.env` the server reads) |
/// | `PRICE_BAND_MIN` / `PRICE_BAND_MAX` | `0.10` / `25.00` | the import band (daily close) |
/// | `ALPACA_DATA_BASE` / `ALPACA_TRADING_BASE` | Alpaca's | API bases |
#[derive(Clone, Debug)]
pub struct MaintainConfig {
    pub window_days: u32,
    pub run_at: NaiveTime,
    pub jobs: usize,
    pub per_minute: u32,
    pub retries: u32,
    pub retry_delay: Duration,
    pub server_url: String,
    pub api_token: Option<String>,
    pub band: PriceBand,
    pub data_base: String,
    pub trading_base: String,
}

impl Default for MaintainConfig {
    fn default() -> Self {
        MaintainConfig {
            window_days: 21,
            run_at: NaiveTime::from_hms_opt(21, 15, 0).expect("valid"),
            jobs: 12,
            per_minute: 9000,
            retries: 3,
            retry_delay: Duration::from_secs(120),
            server_url: "http://127.0.0.1:8790".into(),
            api_token: None,
            band: PriceBand::DEFAULT,
            data_base: "https://data.alpaca.markets".into(),
            trading_base: "https://paper-api.alpaca.markets".into(),
        }
    }
}

/// `PRICE_BAND_MIN` / `PRICE_BAND_MAX` (shared with the other `swimport` commands).
pub fn price_band_from_lookup(get: impl Fn(&str) -> Option<String>) -> Result<PriceBand, String> {
    let p = |k: &str, d: Price| -> Result<Price, String> {
        get(k).filter(|v| !v.trim().is_empty()).map_or(Ok(d), |v| {
            v.trim().parse().map_err(|_| format!("invalid {k} {v:?}"))
        })
    };
    PriceBand::new(
        p("PRICE_BAND_MIN", PriceBand::DEFAULT.min)?,
        p("PRICE_BAND_MAX", PriceBand::DEFAULT.max)?,
    )
}

impl MaintainConfig {
    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Result<Self, String> {
        let var = |k: &str| {
            get(k)
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
        };
        fn num<T: std::str::FromStr>(k: &str, v: Option<String>, d: T) -> Result<T, String> {
            v.map_or(Ok(d), |v| {
                v.parse()
                    .map_err(|_| format!("{k} is not a whole number: {v:?}"))
            })
        }
        let d = MaintainConfig::default();
        let window_days = num(
            "ARCHIVE_WINDOW_DAYS",
            var("ARCHIVE_WINDOW_DAYS"),
            d.window_days,
        )?;
        if !(1..=504).contains(&window_days) {
            return Err(format!(
                "ARCHIVE_WINDOW_DAYS must be 1..=504 trading days, got {window_days}"
            ));
        }
        let run_at = match var("MAINTAIN_AT") {
            Some(v) => NaiveTime::parse_from_str(&v, "%H:%M")
                .map_err(|_| format!("MAINTAIN_AT is not HH:MM (ET): {v:?}"))?,
            None => d.run_at,
        };
        let jobs = num("MAINTAIN_JOBS", var("MAINTAIN_JOBS"), d.jobs)?.clamp(1, 64);
        let per_minute = num(
            "MAINTAIN_PER_MINUTE",
            var("MAINTAIN_PER_MINUTE"),
            d.per_minute,
        )?;
        if per_minute == 0 {
            return Err("MAINTAIN_PER_MINUTE must be at least 1".into());
        }
        Ok(MaintainConfig {
            window_days,
            run_at,
            jobs,
            per_minute,
            retries: num("MAINTAIN_RETRIES", var("MAINTAIN_RETRIES"), d.retries)?,
            retry_delay: Duration::from_secs(num(
                "MAINTAIN_RETRY_DELAY_SECS",
                var("MAINTAIN_RETRY_DELAY_SECS"),
                d.retry_delay.as_secs(),
            )?),
            server_url: var("STOCKWATCHER_SERVER_URL")
                .unwrap_or(d.server_url)
                .trim_end_matches('/')
                .to_string(),
            api_token: var("STOCKWATCHER_API_TOKEN"),
            band: price_band_from_lookup(&get)?,
            data_base: var("ALPACA_DATA_BASE").unwrap_or(d.data_base),
            trading_base: var("ALPACA_TRADING_BASE").unwrap_or(d.trading_base),
        })
    }
}

/// The trading day before `day` (calendar: market_events::session).
pub fn prev_trading_day(day: NaiveDate) -> NaiveDate {
    let mut d = day;
    // A holiday table never has 10 closed days in a row; the bound only guards a bad calendar.
    for _ in 0..10 {
        d = d.pred_opt().expect("date in range");
        if is_trading_day(d) {
            return d;
        }
    }
    d
}

/// L in the module docs: the latest trading day whose SIP history is final at `now`.
pub fn latest_completed_day(now: DateTime<Utc>, run_at: NaiveTime) -> NaiveDate {
    let et = now.with_timezone(&New_York);
    let today = et.date_naive();
    if is_trading_day(today) && et.time() >= run_at {
        today
    } else {
        prev_trading_day(today)
    }
}

/// The trading days kept (module docs), ascending.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Window {
    pub first: NaiveDate,
    pub last: NaiveDate,
    pub days: Vec<NaiveDate>,
}

/// The window ending at `latest` (a trading day): the last `count` trading days, `latest` included.
pub fn window(latest: NaiveDate, count: u32) -> Window {
    let count = count.max(1) as usize;
    let mut days = Vec::with_capacity(count);
    let mut d = latest;
    while days.len() < count {
        if is_trading_day(d) {
            days.push(d);
        }
        d = d.pred_opt().expect("date in range");
    }
    days.reverse();
    Window {
        first: *days.first().unwrap_or(&latest),
        last: latest,
        days,
    }
}

/// The next daily run strictly after `now`: `run_at` ET on the earliest date that is still ahead.
/// Wall-clock ET, so it stays at 21:15 local across the DST changes (a 23- or 25-hour gap that
/// day). A time that does not exist that day (a spring-forward gap) runs an hour later; one that
/// exists twice (fall-back) runs at the first.
pub fn next_run(now: DateTime<Utc>, run_at: NaiveTime) -> DateTime<Utc> {
    let mut date = now.with_timezone(&New_York).date_naive();
    loop {
        let local = date.and_time(run_at);
        let at = New_York
            .from_local_datetime(&local)
            .earliest()
            .or_else(|| {
                New_York
                    .from_local_datetime(&(local + chrono::Duration::hours(1)))
                    .earliest()
            })
            .map(|t| t.with_timezone(&Utc));
        if let Some(at) = at
            && at > now
        {
            return at;
        }
        date = date.succ_opt().expect("date in range");
    }
}

/// Window days the archive has no ready manifest for (informational: what this cycle will fill).
pub fn missing_days(w: &Window, archive: &Archive) -> Vec<NaiveDate> {
    let ready: BTreeSet<NaiveDate> = archive
        .ready_days()
        .unwrap_or_default()
        .into_iter()
        .collect();
    w.days
        .iter()
        .copied()
        .filter(|d| !ready.contains(d))
        .collect()
}

/// What the server said about the day it replays.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ServerView {
    /// It answered; `Some(day)` is being replayed (live mode or no provider: `None`).
    Replaying(Option<NaiveDate>),
    /// It could not be asked; the reason is safe to log (never the token).
    Unreachable(String),
}

/// `GET {server}/api/replay/state` with the bearer token. Only a 200 with a JSON body whose `day`
/// is a date or null counts as an answer.
pub fn probe_server(http: &mut dyn Http, server_url: &str, token: Option<&str>) -> ServerView {
    let url = format!("{}/api/replay/state", server_url.trim_end_matches('/'));
    let auth = token.map(|t| format!("Bearer {t}"));
    let mut headers: Vec<(&str, &str)> = vec![("Accept", "application/json")];
    if let Some(a) = &auth {
        headers.push(("Authorization", a.as_str()));
    }
    let resp: HttpResponse = match http.get(&url, &headers) {
        Ok(r) => r,
        Err(e) => {
            let e = token.map_or(e.clone(), |t| e.replace(t, "REDACTED"));
            return ServerView::Unreachable(format!("{url}: {e}"));
        }
    };
    if resp.status != 200 {
        return ServerView::Unreachable(format!("{url}: HTTP {}", resp.status));
    }
    #[derive(Deserialize)]
    struct State {
        day: Option<String>,
    }
    match serde_json::from_slice::<State>(&resp.body) {
        Ok(State { day: None }) => ServerView::Replaying(None),
        Ok(State { day: Some(d) }) => match NaiveDate::parse_from_str(&d, "%Y-%m-%d") {
            Ok(d) => ServerView::Replaying(Some(d)),
            Err(_) => ServerView::Unreachable(format!("{url}: day {d:?} is not a date")),
        },
        Err(e) => ServerView::Unreachable(format!("{url}: not a replay state ({e})")),
    }
}

/// Days to delete: those of `existing` before the window's first day, except the replayed one.
/// `Err(reason)` when there is something to delete but the server could not be asked.
pub fn prune_plan(
    existing: &[NaiveDate],
    window_first: NaiveDate,
    view: &ServerView,
) -> Result<(Vec<NaiveDate>, Option<NaiveDate>), String> {
    let old: Vec<NaiveDate> = existing
        .iter()
        .copied()
        .filter(|d| *d < window_first)
        .collect();
    if old.is_empty() {
        return Ok((old, None));
    }
    match view {
        ServerView::Unreachable(why) => Err(format!(
            "server unreachable ({why}): {} old day(s) kept this cycle",
            old.len()
        )),
        ServerView::Replaying(current) => {
            let protected = current.filter(|c| old.contains(c));
            Ok((
                old.into_iter().filter(|d| Some(*d) != protected).collect(),
                protected,
            ))
        }
    }
}

/// Removes every date before `keep_from`. Returns how many dates were removed.
pub fn trim_closes(closes: &mut DailyCloses, keep_from: NaiveDate) -> usize {
    let before = closes.len();
    closes.retain(|d, _| *d >= keep_from);
    before - closes.len()
}

/// The plan's symbol-days that still need importing.
#[derive(Debug, Default)]
pub struct Pending {
    pub tasks: Vec<Task>,
    /// Already complete for exactly this request: never fetched again.
    pub done: u64,
    /// The manifest holds a different dataset for that symbol-day (a fixture, another range):
    /// left alone, as `alpaca-batch` does without `--force`.
    pub occupied: u64,
    /// Days whose manifest could not be read: none of their tasks run (an import would refuse
    /// them one by one anyway), each is reported once.
    pub unreadable: Vec<(NaiveDate, String)>,
}

/// Splits `plan` into what is done and what is pending, reading each day's manifest once.
pub fn pending_tasks(plan: &Plan, archive: &Archive) -> Pending {
    let mut out = Pending::default();
    let mut by_day: BTreeMap<NaiveDate, Vec<&Task>> = BTreeMap::new();
    for t in &plan.tasks {
        by_day.entry(t.day).or_default().push(t);
    }
    for (day, tasks) in by_day {
        let manifest = match archive.manifest(day) {
            Ok(m) => m,
            Err(e) => {
                out.unreadable.push((day, e.to_string()));
                continue;
            }
        };
        let wanted = sip_range(day);
        for t in tasks {
            let entry = manifest.as_ref().and_then(|m| m.symbols.get(&t.symbol));
            match (entry, wanted) {
                (None, _) => out.tasks.push(t.clone()),
                (Some(e), Some((start, end))) => {
                    let want = TradesQuery {
                        symbol: t.symbol.clone(),
                        day,
                        start,
                        end,
                        feed: Feed::Sip,
                        limit: super::alpaca::MAX_LIMIT,
                    }
                    .provenance();
                    match &e.import {
                        Some(p) if p.complete && p.same_request(&want) => out.done += 1,
                        _ => out.occupied += 1,
                    }
                }
                (Some(_), None) => out.occupied += 1,
            }
        }
    }
    // Same order rule as the plan: busiest first.
    out.tasks.sort_by(|a, b| {
        b.trades
            .cmp(&a.trades)
            .then(a.day.cmp(&b.day))
            .then(a.symbol.cmp(&b.symbol))
    });
    out
}

/// One cycle's account; also the status file's body.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CycleReport {
    pub started_at: String,
    pub finished_at: Option<String>,
    /// `running`, `ok`, `partial` (failures or errors, see below), `stopped` (SIGTERM).
    pub outcome: String,
    pub latest_completed_day: Option<NaiveDate>,
    pub window: Option<Window>,
    /// Window days without a ready manifest when the cycle started.
    pub missing_days: Vec<NaiveDate>,
    pub imported_symbol_days: u64,
    pub already_complete: u64,
    pub occupied: u64,
    /// Days that received newly imported symbols, with how many.
    pub imported_by_day: BTreeMap<NaiveDate, u64>,
    pub failed: u64,
    /// The first failures (all of them are in `maintainer-failures.json`).
    pub failure_sample: Vec<Failure>,
    /// Days still hidden behind their `.importing` marker after the cycle.
    pub still_importing: Vec<NaiveDate>,
    pub pruned: Vec<NaiveDate>,
    /// Why old days were kept, when they were.
    pub prune_skipped: Option<String>,
    /// The replayed day, when it was older than the window and therefore spared.
    pub protected_day: Option<NaiveDate>,
    pub closes_dates: Option<usize>,
    pub closes_changed: u64,
    pub closes_trimmed: usize,
    pub assets: Option<usize>,
    pub free_bytes: Option<u64>,
    pub requests: u64,
    pub errors: Vec<String>,
    /// Filled in by the loop before the final write.
    pub next_run: Option<String>,
}

fn ts(t: DateTime<Utc>) -> String {
    t.to_rfc3339_opts(SecondsFormat::Secs, true)
}

impl CycleReport {
    /// The one line logged per cycle.
    pub fn summary(&self) -> String {
        let window = self.window.as_ref().map_or("-".into(), |w| {
            format!("{}..{} ({} days)", w.first, w.last, w.days.len())
        });
        let days: Vec<String> = self
            .imported_by_day
            .iter()
            .map(|(d, n)| format!("{d}:{n}"))
            .collect();
        format!(
            "maintain {}: window {window}, missing at start {}, imported {} symbol-days [{}], {} already complete, {} failed, still importing {:?}, pruned {:?}{}{}, closes {} (+{} -{} dates), assets {}, free {}, {} requests, {} errors{}, next run {}",
            self.outcome,
            match self.missing_days.as_slice() {
                [] => "none".to_string(),
                [one] => one.to_string(),
                [a, .., b] => format!("{} ({a}..{b})", self.missing_days.len()),
            },
            self.imported_symbol_days,
            days.join(" "),
            self.already_complete,
            self.failed,
            self.still_importing
                .iter()
                .map(|d| d.to_string())
                .collect::<Vec<_>>(),
            self.pruned
                .iter()
                .map(|d| d.to_string())
                .collect::<Vec<_>>(),
            self.protected_day
                .map_or(String::new(), |d| format!(" (kept {d}: being replayed)")),
            self.prune_skipped
                .as_ref()
                .map_or(String::new(), |r| format!(" (prune skipped: {r})")),
            self.closes_dates.map_or("-".into(), |n| n.to_string()),
            self.closes_changed,
            self.closes_trimmed,
            self.assets.map_or("-".into(), |n| n.to_string()),
            self.free_bytes
                .map_or("-".into(), |b| format!("{:.1} GB", b as f64 / 1e9)),
            self.requests,
            self.errors.len(),
            if self.errors.is_empty() {
                String::new()
            } else {
                format!(" ({})", self.errors.join("; "))
            },
            self.next_run.as_deref().unwrap_or("-"),
        )
    }
}

pub fn status_path(archive: &Archive) -> PathBuf {
    archive.root().join("plans").join(STATUS_FILE)
}

/// Writes `json` to `path` atomically (temp file, rename), creating the directory.
fn write_atomic(path: &std::path::Path, json: &[u8]) -> Result<(), String> {
    let dir = path.parent().expect("has a parent");
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, json).map_err(|e| format!("{}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("{}: {e}", path.display()))
}

pub fn write_status(archive: &Archive, report: &CycleReport) -> Result<PathBuf, String> {
    let path = status_path(archive);
    let json = serde_json::to_vec_pretty(report).map_err(|e| e.to_string())?;
    write_atomic(&path, &json)?;
    Ok(path)
}

pub fn read_status(archive: &Archive) -> Option<CycleReport> {
    serde_json::from_slice(&std::fs::read(status_path(archive)).ok()?).ok()
}

/// How many failures the status file quotes.
const FAILURE_SAMPLE: usize = 25;

/// Everything a cycle needs. `make_http` builds the clients (one per batch worker, one for
/// planning and metadata, one for the server probe); `wait` sleeps and returns false when the
/// stop flag cut it short; `log` gets progress lines.
pub struct Maintainer<'a, H> {
    pub cfg: &'a MaintainConfig,
    pub creds: Option<&'a AlpacaCredentials>,
    pub archive: &'a Archive,
    pub make_http: &'a (dyn Fn() -> Result<H, String> + Sync),
    pub stop: &'a AtomicBool,
    pub wait: &'a dyn Fn(Duration) -> bool,
    pub log: &'a (dyn Fn(&str) + Sync),
}

impl<H: Http + Send + 'static> Maintainer<'_, H> {
    fn stopped(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }

    /// One cycle at `now` (module docs). Never panics on provider or disk trouble: every problem
    /// is recorded in the report and the cycle goes on with what it can still do.
    pub fn run_cycle(&self, now: DateTime<Utc>) -> CycleReport {
        let mut r = CycleReport {
            started_at: ts(now),
            outcome: "running".into(),
            ..CycleReport::default()
        };
        match self.archive.sweep_removed() {
            Ok(0) => {}
            Ok(n) => (self.log)(&format!("finished {n} interrupted day removal(s)")),
            Err(e) => r.errors.push(format!("sweep: {e}")),
        }
        let latest = latest_completed_day(now, self.cfg.run_at);
        let w = window(latest, self.cfg.window_days);
        r.latest_completed_day = Some(latest);
        r.missing_days = missing_days(&w, self.archive);
        r.window = Some(w.clone());
        let _ = write_status(self.archive, &r);

        match self.creds {
            Some(_) if self.stopped() => {}
            Some(creds) => {
                self.refresh_assets(creds, &mut r);
                if !self.stopped() {
                    self.refresh_closes(creds, &w, &mut r);
                }
            }
            None => r.errors.push(
                "ALPACA_API_KEY / ALPACA_API_SECRET are not set: nothing imported or refreshed"
                    .into(),
            ),
        }

        let mut pruned_already = false;
        let mut disk_ok = self.disk_ok(&mut r);
        if !disk_ok && !self.stopped() {
            (self.log)("free space is below the archive floor: pruning before any import");
            self.prune(&w, &mut r);
            pruned_already = true;
            disk_ok = self.disk_ok(&mut r);
        }
        if !disk_ok {
            r.errors.push(format!(
                "free space below ARCHIVE_MIN_FREE_MB ({} MB): import refused",
                self.archive.config().min_free_bytes / (1024 * 1024)
            ));
        }
        if let Some(creds) = self.creds
            && disk_ok
            && !self.stopped()
        {
            self.import(creds, &w, &mut r);
        }
        if !pruned_already && !self.stopped() {
            let newest = self
                .archive
                .ready_days()
                .ok()
                .and_then(|d| d.last().copied());
            if newest.is_some_and(|n| n >= latest) {
                self.prune(&w, &mut r);
            } else {
                // Only worth saying when there is something old to keep.
                let old = self
                    .archive
                    .days()
                    .unwrap_or_default()
                    .iter()
                    .any(|d| *d < w.first);
                if old {
                    r.prune_skipped = Some(format!(
                        "the archive is not caught up to {latest} (newest ready day {}): old days kept rather than shrinking the window",
                        newest.map_or("none".into(), |d| d.to_string())
                    ));
                }
            }
        }
        r.free_bytes = self.archive.free_bytes().ok();
        r.finished_at = Some(ts(Utc::now()));
        r.outcome = if self.stopped() {
            "stopped"
        } else if r.failed > 0 || !r.errors.is_empty() || !r.still_importing.is_empty() {
            "partial"
        } else {
            "ok"
        }
        .into();
        r
    }

    fn client(&self, r: &mut CycleReport) -> Option<H> {
        match (self.make_http)() {
            Ok(h) => Some(h),
            Err(e) => {
                r.errors.push(format!("http client: {e}"));
                None
            }
        }
    }

    fn disk_ok(&self, r: &mut CycleReport) -> bool {
        let floor = self.archive.config().min_free_bytes;
        match self.archive.free_bytes() {
            Ok(free) => {
                r.free_bytes = Some(free);
                free >= floor
            }
            Err(e) => {
                r.errors.push(format!("free space: {e}"));
                floor == 0
            }
        }
    }

    fn refresh_assets(&self, creds: &AlpacaCredentials, r: &mut CycleReport) {
        let Some(mut http) = self.client(r) else {
            return;
        };
        let log = self.log;
        let assets = match asset_metadata(&mut http, &self.cfg.trading_base, creds, &mut |l| log(l))
        {
            Ok(a) if a.is_empty() => {
                r.errors
                    .push("assets: the endpoint returned none; assets.json unchanged".into());
                return;
            }
            Ok(a) => a,
            Err(e) => {
                r.errors.push(format!("assets: {e}"));
                return;
            }
        };
        r.assets = Some(assets.len());
        if self.archive.read_assets().ok().flatten().as_ref() == Some(&assets) {
            return;
        }
        if let Err(e) = self.archive.write_assets(&assets) {
            r.errors.push(format!("assets: {e}"));
        }
    }

    fn refresh_closes(&self, creds: &AlpacaCredentials, w: &Window, r: &mut CycleReport) {
        // Read first: an unreadable file is reported and left alone, never replaced by just this
        // range (which would drop every other date), as `alpaca-daily-closes` does.
        let mut all = match self.archive.read_daily_closes() {
            Ok(c) => c.unwrap_or_default(),
            Err(e) => {
                r.errors.push(format!("daily closes: {e} (left unchanged)"));
                return;
            }
        };
        let Some(mut http) = self.client(r) else {
            return;
        };
        let log = self.log;
        let from = prev_trading_day(w.first);
        let new = match daily_closes(
            &mut http,
            &self.cfg.trading_base,
            &self.cfg.data_base,
            creds,
            from,
            w.last,
            &mut |l: &str| log(l),
        ) {
            Ok(n) if n.is_empty() => {
                r.errors.push(format!(
                    "daily closes: none for {from}..{}; file unchanged",
                    w.last
                ));
                return;
            }
            Ok(n) => n,
            Err(e) => {
                r.errors.push(format!("daily closes: {e}"));
                return;
            }
        };
        r.closes_changed = merge_daily_closes(&mut all, new) as u64;
        // One day before the earliest day still archived (the window's first, or an older day
        // kept because it is being replayed or because pruning is held back).
        let earliest = self
            .archive
            .days()
            .ok()
            .and_then(|d| d.first().copied())
            .map_or(w.first, |d| d.min(w.first));
        r.closes_trimmed = trim_closes(&mut all, prev_trading_day(earliest));
        r.closes_dates = Some(all.len());
        if r.closes_changed == 0 && r.closes_trimmed == 0 {
            return;
        }
        if let Err(e) = self.archive.write_daily_closes(&all) {
            r.errors.push(format!("daily closes: {e}"));
        }
    }

    fn import(&self, creds: &AlpacaCredentials, w: &Window, r: &mut CycleReport) {
        let Some(mut http) = self.client(r) else {
            return;
        };
        let log = self.log;
        let plan = match plan_universe(
            &mut http,
            &self.cfg.trading_base,
            &self.cfg.data_base,
            creds,
            w.first,
            w.last,
            self.cfg.band,
            &mut |l: &str| log(l),
        ) {
            Ok(p) => p,
            Err(e) => {
                r.errors.push(format!("plan: {e}"));
                return;
            }
        };
        let mut planned: BTreeMap<NaiveDate, usize> = BTreeMap::new();
        for t in &plan.tasks {
            *planned.entry(t.day).or_default() += 1;
        }
        let pending = pending_tasks(&plan, self.archive);
        r.already_complete = pending.done;
        r.occupied = pending.occupied;
        for (day, e) in &pending.unreadable {
            r.errors
                .push(format!("{day}: manifest unreadable ({e}); day skipped"));
        }
        // A day with no manifest yet is hidden before its first symbol lands.
        let new_days: BTreeSet<NaiveDate> = pending
            .tasks
            .iter()
            .map(|t| t.day)
            .filter(|d| !self.archive.manifest_path(*d).exists())
            .collect();
        for d in &new_days {
            if let Err(e) = self.archive.mark_importing(*d) {
                r.errors.push(format!("{d}: {e}"));
            }
        }
        if !pending.tasks.is_empty() {
            (self.log)(&format!(
                "importing {} symbol-days over {} day(s) ({} already complete), {} workers, at most {}/min",
                pending.tasks.len(),
                pending
                    .tasks
                    .iter()
                    .map(|t| t.day)
                    .collect::<BTreeSet<_>>()
                    .len(),
                pending.done,
                self.cfg.jobs,
                self.cfg.per_minute
            ));
        }
        let by_key: BTreeMap<(NaiveDate, &str), &Task> = pending
            .tasks
            .iter()
            .map(|t| ((t.day, t.symbol.as_str()), t))
            .collect();
        let mut todo = Plan {
            tasks: pending.tasks.clone(),
            ..plan.clone()
        };
        let mut failed: Vec<Failure> = Vec::new();
        let mut attempt = 0u32;
        while !todo.tasks.is_empty() {
            let rep = run_batch_until(
                &todo,
                self.make_http,
                &self.cfg.data_base,
                creds,
                self.archive,
                self.cfg.jobs,
                self.cfg.per_minute,
                Duration::from_secs(60),
                log,
                log,
                self.stop,
            );
            r.imported_symbol_days += rep.imported;
            r.requests += rep.requests;
            for (d, n) in &rep.imported_by_day {
                *r.imported_by_day.entry(*d).or_default() += n;
            }
            failed = rep.failed;
            if rep.stopped || failed.is_empty() || attempt >= self.cfg.retries {
                break;
            }
            let delay = self.cfg.retry_delay.saturating_mul(1u32 << attempt.min(16));
            attempt += 1;
            (self.log)(&format!(
                "{} symbol-days failed; retry pass {attempt}/{} in {}s",
                failed.len(),
                self.cfg.retries,
                delay.as_secs()
            ));
            if !(self.wait)(delay) {
                break;
            }
            todo.tasks = failed
                .iter()
                .filter_map(|f| {
                    by_key
                        .get(&(f.day, f.symbol.as_str()))
                        .map(|t| (*t).clone())
                })
                .collect();
        }
        r.failed = failed.len() as u64;
        r.failure_sample = failed.iter().take(FAILURE_SAMPLE).cloned().collect();
        let failures_path = self.archive.root().join("plans").join(FAILURES_FILE);
        if let Err(e) = serde_json::to_vec_pretty(&failed)
            .map_err(|e| e.to_string())
            .and_then(|j| write_atomic(&failures_path, &j))
        {
            r.errors.push(format!("failures file: {e}"));
        }
        // Markers: every window day that carries one (new now, or left by an interrupted run).
        // Stopped mid-run: keep them all, the next start carries on.
        let mut failed_per_day: BTreeMap<NaiveDate, usize> = BTreeMap::new();
        for f in &failed {
            *failed_per_day.entry(f.day).or_default() += 1;
        }
        for d in w
            .days
            .iter()
            .copied()
            .filter(|d| self.archive.is_importing(*d))
        {
            let total = planned.get(&d).copied().unwrap_or(0);
            let bad = failed_per_day.get(&d).copied().unwrap_or(0);
            let unreadable = pending.unreadable.iter().any(|(u, _)| *u == d);
            if !self.stopped()
                && !unreadable
                && bad * 1000 <= total * MAX_FAILED_PER_MILLE
                && self.archive.manifest_path(d).exists()
            {
                // Clears the marker and retires the day's live recording: from here every
                // reader takes the complete, corrected import (market_archive::live).
                if let Err(e) = self.archive.finish_import(d) {
                    r.errors.push(format!("{d}: {e}"));
                    r.still_importing.push(d);
                }
            } else {
                r.still_importing.push(d);
            }
        }
    }

    fn prune(&self, w: &Window, r: &mut CycleReport) {
        let existing = match self.archive.days() {
            Ok(d) => d,
            Err(e) => {
                r.errors.push(format!("prune: {e}"));
                return;
            }
        };
        if !existing.iter().any(|d| *d < w.first) {
            return;
        }
        let view = match self.client(r) {
            Some(mut h) => {
                probe_server(&mut h, &self.cfg.server_url, self.cfg.api_token.as_deref())
            }
            None => ServerView::Unreachable("no http client".into()),
        };
        match prune_plan(&existing, w.first, &view) {
            Err(reason) => {
                (self.log)(&format!("prune skipped: {reason}"));
                r.prune_skipped = Some(reason);
            }
            Ok((delete, protected)) => {
                r.protected_day = protected;
                for d in delete {
                    if self.stopped() {
                        break;
                    }
                    match self.archive.remove_day(d) {
                        Ok(()) => r.pruned.push(d),
                        Err(e) => r.errors.push(format!("prune {d}: {e}")),
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(s: &str) -> NaiveDate {
        s.parse().unwrap()
    }

    fn utc(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    fn at() -> NaiveTime {
        NaiveTime::from_hms_opt(21, 15, 0).unwrap()
    }

    #[test]
    fn the_latest_completed_day_waits_for_the_run_time_and_skips_closed_days() {
        // Friday 2026-10-16: before 21:15 ET the day is not final yet.
        assert_eq!(
            latest_completed_day(utc("2026-10-16T20:00:00-04:00"), at()),
            d("2026-10-15")
        );
        assert_eq!(
            latest_completed_day(utc("2026-10-16T21:15:00-04:00"), at()),
            d("2026-10-16")
        );
        // The weekend and Monday morning: still Friday.
        assert_eq!(
            latest_completed_day(utc("2026-10-18T12:00:00-04:00"), at()),
            d("2026-10-16")
        );
        assert_eq!(
            latest_completed_day(utc("2026-10-19T09:00:00-04:00"), at()),
            d("2026-10-16")
        );
        // Thanksgiving 2026-11-26 (closed): the Wednesday before.
        assert_eq!(
            latest_completed_day(utc("2026-11-26T23:00:00-05:00"), at()),
            d("2026-11-25")
        );
        // 00:30 ET belongs to the new date, which has not run yet.
        assert_eq!(
            latest_completed_day(utc("2026-10-20T00:30:00-04:00"), at()),
            d("2026-10-19")
        );
    }

    #[test]
    fn the_window_is_the_last_21_trading_days_one_in_one_out() {
        // Sunday 2026-10-18: L = Friday 10-16; the seeded 09-17 is the 22nd day back, so it is out.
        let w = window(d("2026-10-16"), 21);
        assert_eq!(
            (w.first, w.last, w.days.len()),
            (d("2026-09-18"), d("2026-10-16"), 21)
        );
        // Monday's day comes in, exactly one goes out (Friday 09-18), the weekend changes nothing.
        let w = window(d("2026-10-19"), 21);
        assert_eq!((w.first, w.days.len()), (d("2026-09-21"), 21));
        assert!(!w.days.contains(&d("2026-10-17")) && !w.days.contains(&d("2026-10-18")));
        // Across Thanksgiving: 11-26 is skipped and the window reaches one day further back; 11-27
        // (an early close) is a session and counts.
        let w = window(d("2026-12-01"), 21);
        assert!(!w.days.contains(&d("2026-11-26")));
        assert!(w.days.contains(&d("2026-11-27")));
        assert_eq!(w.days.len(), 21);
        // Another length.
        assert_eq!(window(d("2026-10-16"), 5).first, d("2026-10-12"));
        // Every trading day of two years: always exactly 21, and from one trading day to the next
        // exactly one day enters and one leaves.
        let mut prev: Option<Window> = None;
        let mut day = d("2026-01-02");
        while day < d("2027-12-31") {
            if is_trading_day(day) {
                let w = window(day, 21);
                assert_eq!(w.days.len(), 21, "{day}");
                assert!(w.days.iter().all(|x| is_trading_day(*x)));
                if let Some(p) = &prev {
                    assert_eq!(&w.days[..20], &p.days[1..], "{day}: one in, one out");
                }
                prev = Some(w);
            }
            day = day.succ_opt().unwrap();
        }
    }

    #[test]
    fn the_next_run_is_the_et_wall_time_across_dst() {
        // Before today's run: today.
        assert_eq!(
            next_run(utc("2026-10-19T12:00:00-04:00"), at()),
            utc("2026-10-19T21:15:00-04:00")
        );
        // Exactly at the run time: tomorrow (strictly after).
        assert_eq!(
            next_run(utc("2026-10-19T21:15:00-04:00"), at()),
            utc("2026-10-20T21:15:00-04:00")
        );
        // DST ends 2026-11-01 02:00: Saturday's run is EDT, Sunday's EST, 25 hours apart.
        let sat = next_run(utc("2026-10-31T21:00:00-04:00"), at());
        assert_eq!(sat, utc("2026-11-01T01:15:00Z"));
        let sun = next_run(sat, at());
        assert_eq!(sun, utc("2026-11-02T02:15:00Z"));
        assert_eq!((sun - sat).num_hours(), 25);
        // DST starts 2027-03-14: 23 hours.
        let a = next_run(utc("2027-03-13T12:00:00-05:00"), at());
        let b = next_run(a, at());
        assert_eq!((b - a).num_hours(), 23);
        assert_eq!(b, utc("2027-03-14T21:15:00-04:00"));
        // A run time inside the spring-forward gap runs an hour later that day.
        let gap = NaiveTime::from_hms_opt(2, 30, 0).unwrap();
        assert_eq!(
            next_run(utc("2027-03-14T00:00:00-05:00"), gap),
            utc("2027-03-14T03:30:00-04:00")
        );
    }

    #[test]
    fn prune_spares_the_replayed_day_and_deletes_nothing_without_an_answer() {
        let existing = [
            d("2026-09-16"),
            d("2026-09-17"),
            d("2026-09-18"),
            d("2026-10-16"),
        ];
        let first = d("2026-09-18");
        assert_eq!(
            prune_plan(
                &existing,
                first,
                &ServerView::Replaying(Some(d("2026-10-16")))
            ),
            Ok((vec![d("2026-09-16"), d("2026-09-17")], None))
        );
        assert_eq!(
            prune_plan(
                &existing,
                first,
                &ServerView::Replaying(Some(d("2026-09-16")))
            ),
            Ok((vec![d("2026-09-17")], Some(d("2026-09-16"))))
        );
        assert_eq!(
            prune_plan(&existing, first, &ServerView::Replaying(None)),
            Ok((vec![d("2026-09-16"), d("2026-09-17")], None))
        );
        let e = prune_plan(&existing, first, &ServerView::Unreachable("refused".into()));
        assert!(e.unwrap_err().contains("2 old day(s) kept"));
        // Nothing old: no question asked, nothing to refuse.
        assert_eq!(
            prune_plan(&existing[2..], first, &ServerView::Unreachable("x".into())),
            Ok((vec![], None))
        );
    }

    #[test]
    fn trimming_keeps_dates_from_the_cut_on() {
        let mut c = DailyCloses::new();
        for day in ["2026-09-15", "2026-09-16", "2026-09-17"] {
            c.entry(d(day)).or_default().insert("A".into(), 1.0);
        }
        assert_eq!(trim_closes(&mut c, d("2026-09-16")), 1);
        assert_eq!(
            c.keys().copied().collect::<Vec<_>>(),
            [d("2026-09-16"), d("2026-09-17")]
        );
        assert_eq!(trim_closes(&mut c, d("2026-09-16")), 0);
    }

    struct Scripted(Result<HttpResponse, String>);
    impl Http for Scripted {
        fn get(&mut self, url: &str, headers: &[(&str, &str)]) -> Result<HttpResponse, String> {
            assert_eq!(url, "http://server:8790/api/replay/state");
            assert!(headers.contains(&("Authorization", "Bearer tok")));
            self.0.clone()
        }
    }

    #[test]
    fn the_server_probe_reads_the_day_and_never_leaks_the_token() {
        let body = |v: serde_json::Value| {
            Ok(HttpResponse {
                status: 200,
                body: v.to_string().into_bytes(),
                retry_after: None,
            })
        };
        let probe = |r| probe_server(&mut Scripted(r), "http://server:8790/", Some("tok"));
        assert_eq!(
            probe(body(
                serde_json::json!({"mode": "replay", "day": "2026-10-16"})
            )),
            ServerView::Replaying(Some(d("2026-10-16")))
        );
        assert_eq!(
            probe(body(serde_json::json!({"mode": "live", "day": null}))),
            ServerView::Replaying(None)
        );
        for r in [
            Ok(HttpResponse {
                status: 401,
                body: vec![],
                retry_after: None,
            }),
            Err("connection refused (sent tok)".into()),
            body(serde_json::json!({"day": "yesterday"})),
            body(serde_json::json!([1, 2])),
        ] {
            match probe(r) {
                ServerView::Unreachable(why) => assert!(!why.contains("tok"), "{why}"),
                other => panic!("{other:?}"),
            }
        }
    }

    #[test]
    fn config_defaults_and_validation() {
        let c = MaintainConfig::from_lookup(|_| None).unwrap();
        assert_eq!(
            (c.window_days, c.jobs, c.per_minute, c.retries),
            (21, 12, 9000, 3)
        );
        assert_eq!(c.run_at, at());
        assert_eq!(c.server_url, "http://127.0.0.1:8790");
        let c = MaintainConfig::from_lookup(|k| match k {
            "MAINTAIN_AT" => Some("22:05".into()),
            "STOCKWATCHER_SERVER_URL" => Some("http://server:8790/".into()),
            "ARCHIVE_WINDOW_DAYS" => Some("42".into()),
            _ => None,
        })
        .unwrap();
        assert_eq!(c.run_at, NaiveTime::from_hms_opt(22, 5, 0).unwrap());
        assert_eq!(c.server_url, "http://server:8790");
        assert_eq!(c.window_days, 42);
        for (k, v) in [
            ("MAINTAIN_AT", "9pm"),
            ("ARCHIVE_WINDOW_DAYS", "0"),
            ("ARCHIVE_WINDOW_DAYS", "505"),
            ("MAINTAIN_PER_MINUTE", "0"),
            ("PRICE_BAND_MIN", "x"),
        ] {
            assert!(
                MaintainConfig::from_lookup(|q| (q == k).then(|| v.to_string())).is_err(),
                "{k}={v}"
            );
        }
    }
}

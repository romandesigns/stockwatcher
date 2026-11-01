//! The Insights **ticker page** (owner, 2026-10-31, approved design): everything about one symbol
//! on one trading day, computed here and pushed whole (owner rule: Rust computes, clients only
//! render: no outcome math, no aggregation, no detector logic in TypeScript).
//!
//! `GET /api/insights/ticker?symbol=INO&day=2026-10-30` (day absent = today's trading date), and
//! over `/ws` (`insights.ticker.v1` in `welcome.features`; old phone builds never see it):
//!
//! - client `insights.ticker.subscribe {symbol, day?}` -> server `insights.ticker.snapshot
//!   {symbol, day, …package}`; for TODAY then `insights.ticker.update {…package}` whenever it
//!   changes (a notice for the symbol, or prices moving its outcomes; rebuilt at most every
//!   [`REFRESH`], and [`DEBOUNCE`] after a notice). The update is the whole package: it replaces.
//! - `insights.ticker.unsubscribe` stops it. A past day gets its snapshot only.
//!
//! # The package
//!
//! - **Top bar**: `symbol`, `name` (assets.json, when the server has it), `prev_day` / `next_day`
//!   (the server's trading calendar; `archived` says whether the tape is there).
//! - **Stats** (`stats`): regular-session close and change vs the previous close (the daily
//!   closes file, split-adjusted like the chart header), pre / regular / after OHLC with volume
//!   and dollar volume, the day's volume and dollar volume, the day's high and its session; the
//!   float the server HELD as of that day (the newest float evidence retrieved by the end of that
//!   day, with when it was retrieved: FMP gives no as-of date of its own) or `available: false`;
//!   short interest is never collected, so it is always `available: false` (never invented).
//! - **Chart**: `bars` (1-minute, the chart engine's own), `markers` (each SENT alert numbered
//!   oldest first, at its reference price; each missed setup at its arming), and `focus`
//!   (`{from, to, reason}`, epoch ms): the window the chart opens on, the move the alerts were
//!   about rather than the whole day ([`package::focus_window`]); null only without bars.
//!   `chart` (crate::chart_context::day_context): over those bars, by the live chart's rules,
//!   `levels` (a `chart.levels` frame), `zones` / `zonesRth` (`chart.zones` frames, extended
//!   hours on / off) and `sessions` (the session schedule, `sessions.v1`), so the page draws
//!   them as sent instead of reading them off its candles.
//! - **Notification history** (`alerts`, oldest first): the ledger's rows for the symbol with
//!   their status ([`crate::notices::FEATURE_ROWS`]), `label`, `marker` and `outcome`: best price
//!   after (and when, before any stop), when +5% over the alert price was first reached, the stop
//!   (a setup's frozen pullback low, from the live journal or the replayed episode) and whether
//!   and when it was hit. A crossing is measured from its crossing print. A setup alert also says
//!   how it did from its first print through the trigger.
//! - **Scorecard** (`scorecard`): alerts sent / held, +5% reached, the best entry, setups armed,
//!   missed, and the same per notice kind.
//! - **Halts & LULD** (`halts`): the halts the tape recorded (code, reason, resume), whether the
//!   day's high came outside 09:30–16:00 (where LULD does not apply), and the regular session's
//!   closest approach to a modelled band (production's `chart_engine::luld`).
//! - **Missed setups** (`missed`): every micro-pullback setup the production detector ARMED that
//!   day (re-run over the tape: [`replay`]) that no notice went out for, with the exact rule that
//!   held it (`crate::cue_notices::SetupVerdict`: warm-up after a restart, one setup notice per
//!   stock and day, the 30-minute window after the Runners flag, no context) and how it played
//!   out. Depth or coverage rejections never armed, so they are not setups and are not listed.
//!   Where the day's ledger exists, an armed setup it names was notified whatever the replay
//!   says, and one the replay would have notified but the ledger lacks is `not_recorded`.
//!
//! # Cost
//!
//! A build replays the symbol's whole day through the chart and context engines, off the async
//! workers ([`tokio::task::spawn_blocking`]), at most [`MAX_BUILDS`] at a time, never under the
//! market's core lock: the live engine is never in its way. A past day is cached on disk under
//! `DATA_DIR/insights/<day>/<SYMBOL>.json`, keyed by everything it was made from (code commit,
//! policies, the sizes of the tape, ledger and journal files, the boots), so an import replacing
//! the live recording or a new build of the server recomputes it. Today is rebuilt per follower
//! group, shared by every connection watching the same symbol.

pub mod package;
pub mod replay;

#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::{Duration as Days, NaiveDate};
use market_archive::{Archive, ArchiveConfig, PrevCloses};
use market_events::session::{SessionId, SessionKind, is_trading_day, previous_trading_day};
use market_events::{MarketEvent, UnixNanos};
use serde_json::{Value, json};
use tokio::sync::{Semaphore, watch};

use crate::notices::NoticeFeed;
use package::{DayRecords, FloatFact, LiveEpisode};
use replay::ReplayInput;

/// The feature a client must see in `welcome.features` before it sends
/// `insights.ticker.subscribe` or calls the endpoint.
pub const FEATURE: &str = "insights.ticker.v1";

/// The package's schema; part of the cache key. 2: `chart` (levels, zones, sessions) added, so
/// a day cached before it is rebuilt rather than served without them.
pub const SCHEMA: u32 = 2;

/// Builds running at once (each replays a whole symbol-day).
pub const MAX_BUILDS: usize = 2;

/// Today's package is rebuilt at most this often while followed.
pub const REFRESH: Duration = Duration::from_secs(15);

/// After a notice for the symbol, wait this long for its other lines (the phone decision, the
/// push) before rebuilding.
pub const DEBOUNCE: Duration = Duration::from_secs(2);

/// Missed setups listed at most (a day has a handful; the bound keeps a pathological one small).
pub const MAX_MISSED: usize = 200;

/// Where the package's inputs live.
#[derive(Clone, Debug)]
pub struct Sources {
    pub archive: ArchiveConfig,
    /// The cue transition log (`ARCHIVE_ROOT/transitions`).
    pub transitions_dir: PathBuf,
    /// The server log (`DATA_DIR/logs`): boots, when the admission journal has none.
    pub server_logs: Option<PathBuf>,
    /// Past days' packages (`DATA_DIR/insights`); `None` caches nothing.
    pub cache_dir: Option<PathBuf>,
}

impl Sources {
    /// The production layout under `ARCHIVE_ROOT` and `DATA_DIR`.
    pub fn standard(archive: ArchiveConfig, data_dir: &Path) -> Sources {
        Sources {
            transitions_dir: archive.root.join(crate::oplog::TRANSITIONS_DIR),
            archive,
            server_logs: Some(data_dir.join(crate::oplog::SERVER_LOG_DIR)),
            cache_dir: Some(data_dir.join("insights")),
        }
    }
}

type Key = (String, NaiveDate);

struct Inner {
    src: Sources,
    archive: Archive,
    notices: NoticeFeed,
    permits: Arc<Semaphore>,
    /// Followed today's packages (module docs, "Cost").
    watchers: Mutex<HashMap<Key, watch::Sender<Option<Arc<String>>>>>,
    /// The daily closes and assets, re-read only when their file changes.
    closes: Mutex<Option<(FileStamp, Arc<PrevCloses>)>>,
    assets: Mutex<Option<(FileStamp, Arc<market_archive::assets::AssetMap>)>>,
}

/// The ticker page's service. Cheap to clone.
#[derive(Clone)]
pub struct Insights(Arc<Inner>);

impl std::fmt::Debug for Insights {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Insights")
            .field("archive", &self.0.src.archive.root)
            .finish()
    }
}

/// A file's size and modification time, for the caches.
type FileStamp = Option<(u64, std::time::SystemTime)>;

fn stamp(p: &Path) -> FileStamp {
    let m = std::fs::metadata(p).ok()?;
    Some((m.len(), m.modified().ok()?))
}

fn len(p: &Path) -> u64 {
    std::fs::metadata(p).map_or(0, |m| m.len())
}

/// A request checked before it names any file.
pub fn parse_request(symbol: &str, day: Option<&str>) -> Result<(String, NaiveDate), &'static str> {
    let symbol = symbol.trim().to_ascii_uppercase();
    if !market_archive::valid_symbol(&symbol) {
        return Err("symbol must be a ticker like INO");
    }
    let day = match day.map(str::trim).filter(|d| !d.is_empty()) {
        Some(d) => {
            NaiveDate::parse_from_str(d, "%Y-%m-%d").map_err(|_| "day must be YYYY-MM-DD")?
        }
        None => market_events::session::trading_date(UnixNanos(now_ns())),
    };
    Ok((symbol, day))
}

fn now_ns() -> i64 {
    crate::oplog::now_ms() * 1_000_000
}

/// Whether `day` is still running at `now`: a trading day whose after-hours session has not
/// ended (its package changes, so it is followed and never cached). On a Saturday the latest
/// trading day is Friday, which is over.
pub fn is_running(day: NaiveDate, now: i64) -> bool {
    is_trading_day(day)
        && market_events::session::trading_date(UnixNanos(now)) == day
        && SessionId::new(day, SessionKind::After)
            .bounds()
            .is_some_and(|(_, end)| now < end.0)
}

/// The trading day after `day` (the calendar's own rule; bounded walk).
pub fn next_trading_day(day: NaiveDate) -> NaiveDate {
    let mut d = day + Days::days(1);
    for _ in 0..10 {
        if is_trading_day(d) {
            return d;
        }
        d += Days::days(1);
    }
    d
}

/// `rest` (an object) as a frame with `type` first, like the notices' frames: clients route on
/// the prefix (`{"type":"insights.ticker.`) without parsing a big body twice.
pub fn frame(kind: &str, rest: &str) -> String {
    debug_assert!(rest.starts_with('{') && rest.len() > 2);
    format!("{{\"type\":{},{}", json!(kind), &rest[1..])
}

/// The tape of a symbol-day, preferring the receipt-order recording the live engine saw.
fn tape_path(a: &Archive, day: NaiveDate, symbol: &str) -> Option<(&'static str, PathBuf)> {
    let ext = market_archive::format::EXT;
    [
        (
            "live-original",
            a.original_dir(day).join(format!("{symbol}.{ext}")),
        ),
        ("live", a.live_path(day, symbol)),
        ("final", a.day_dir(day).join(format!("{symbol}.{ext}"))),
    ]
    .into_iter()
    .find(|(_, p)| p.is_file())
}

/// Every event of a tape file, in recorded order (a torn last frame ends it, as for any reader).
/// Streamed into the replay rather than collected: a busy day is hundreds of thousands of events,
/// and two builds at once would otherwise each hold the whole tape in memory.
fn read_events(path: &Path) -> impl Iterator<Item = MarketEvent> {
    market_archive::reader::SymbolReader::open(path)
        .ok()
        .into_iter()
        .flatten()
        .map_while(|e| e.ok())
        .map(|e| e.event)
}

/// The boots of `day`: the admission journal's segment headers (written beside the recording),
/// else the server log's `listening` lines.
fn boots(a: &Archive, logs: Option<&Path>, day: NaiveDate) -> Vec<i64> {
    for dir in [a.original_dir(day), a.live_dir(day)] {
        let b = market_archive::admissions::boots(&dir);
        if !b.is_empty() {
            return b;
        }
    }
    logs.map(|d| boots_from_log(&d.join(format!("server-{day}.jsonl"))))
        .unwrap_or_default()
}

/// The `listening` lines of a server log day file (the process logs one per boot).
pub fn boots_from_log(path: &Path) -> Vec<i64> {
    use std::io::BufRead;
    let Ok(f) = std::fs::File::open(path) else {
        return Vec::new();
    };
    let mut out: Vec<i64> = std::io::BufReader::new(f)
        .lines()
        .map_while(Result::ok)
        .filter(|l| l.contains("\"message\":\"listening\""))
        .filter_map(|l| {
            let v: Value = serde_json::from_str(&l).ok()?;
            let ts = v.get("timestamp")?.as_str()?;
            chrono::DateTime::parse_from_rfc3339(ts)
                .ok()?
                .timestamp_nanos_opt()
        })
        .collect();
    out.sort_unstable();
    out
}

/// The symbol's episodes in the day's cue transition log: whether each was warm, its trigger and
/// stop, when it armed. Lines not naming the symbol are skipped before parsing.
pub fn live_episodes(path: &Path, symbol: &str) -> HashMap<String, LiveEpisode> {
    use std::io::BufRead;
    let mut out: HashMap<String, LiveEpisode> = HashMap::new();
    let Ok(f) = std::fs::File::open(path) else {
        return out;
    };
    let needle = format!("\"symbol\":{}", json!(symbol));
    for l in std::io::BufReader::new(f).lines().map_while(Result::ok) {
        if !l.contains(&needle) {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(&l) else {
            continue;
        };
        if v.get("symbol").and_then(Value::as_str) != Some(symbol) {
            continue;
        }
        let Some(id) = v.get("episode_id").and_then(Value::as_str) else {
            continue;
        };
        let e = out.entry(id.to_string()).or_default();
        e.warm |= v.get("warmup").and_then(Value::as_bool).unwrap_or(false);
        if let Some(a) = v.get("anchors").filter(|a| a.is_object()) {
            e.trigger = a.get("h0").and_then(Value::as_i64).or(e.trigger);
            e.stop = a.get("pullback_low").and_then(Value::as_i64).or(e.stop);
        }
        if v.get("to").and_then(Value::as_str) == Some("armed") && e.armed_at.is_none() {
            e.armed_at = v.get("observed_at").and_then(Value::as_i64);
        }
    }
    out
}

/// A notice row's own time (ns): its print, else its decision.
fn row_at(v: &Value) -> Option<i64> {
    let n = |k: &str| {
        v.get(k)
            .and_then(|x| x.as_i64().or_else(|| x.as_f64().map(|f| f as i64)))
    };
    n("exec_ns")
        .or_else(|| n("at_ns"))
        .or_else(|| n("at").map(|a| a * 1_000_000))
}

const RUNNERS_FLAGS: [&str; 8] = [
    "runners.new",
    "runners.burst",
    "runners.fast",
    "runners.run",
    "runners.running",
    "runners.breakout",
    "runners.shelf",
    "runners.followup",
];

impl Insights {
    pub fn new(src: Sources, notices: NoticeFeed) -> Insights {
        Insights(Arc::new(Inner {
            archive: Archive::new(src.archive.clone()),
            src,
            notices,
            permits: Arc::new(Semaphore::new(MAX_BUILDS)),
            watchers: Mutex::new(HashMap::new()),
            closes: Mutex::new(None),
            assets: Mutex::new(None),
        }))
    }

    /// No archive (tests, a server without one): every day is unavailable.
    pub fn idle() -> Insights {
        let root = std::env::temp_dir().join("stockwatcher-insights-idle-none");
        Insights::new(
            Sources {
                archive: ArchiveConfig {
                    root: root.clone(),
                    ..ArchiveConfig::default()
                },
                transitions_dir: root.join("transitions"),
                server_logs: None,
                cache_dir: None,
            },
            NoticeFeed::idle(),
        )
    }

    fn prev_closes(&self) -> Arc<PrevCloses> {
        let a = &self.0.archive;
        let st = stamp(&a.daily_closes_path());
        let mut c = self.0.closes.lock().expect("insights closes");
        if let Some((s, v)) = c.as_ref()
            && *s == st
        {
            return v.clone();
        }
        let v = Arc::new(
            a.read_daily_closes()
                .ok()
                .flatten()
                .map(|d| PrevCloses::new(&d).0)
                .unwrap_or_default(),
        );
        *c = Some((st, v.clone()));
        v
    }

    fn name(&self, symbol: &str) -> Option<String> {
        let a = &self.0.archive;
        let st = stamp(&a.assets_path());
        let mut c = self.0.assets.lock().expect("insights assets");
        let map = match c.as_ref() {
            Some((s, v)) if *s == st => v.clone(),
            _ => {
                let v = Arc::new(a.read_assets().ok().flatten().unwrap_or_default());
                *c = Some((st, v.clone()));
                v
            }
        };
        map.get(symbol)
            .map(|m| m.name.clone())
            .filter(|n| !n.is_empty())
    }

    /// The package of `(symbol, day)` as of `now` (UTC ns). Reads files and replays the day:
    /// call it off the async workers ([`Insights::package`]).
    pub fn package_blocking(&self, symbol: &str, day: NaiveDate, now: i64) -> Value {
        let today = market_events::session::trading_date(UnixNanos(now));
        let a = &self.0.archive;
        let ongoing = is_running(day, now);
        let neighbour = |d: NaiveDate| {
            (d <= today).then(
                || json!({"day": d.to_string(), "archived": tape_path(a, d, symbol).is_some()}),
            )
        };
        let mut head = json!({
            "v": SCHEMA,
            "symbol": symbol,
            "day": day.to_string(),
            "today": ongoing,
            "prev_day": neighbour(previous_trading_day(day)),
            "next_day": neighbour(next_trading_day(day)),
        });
        let unavailable = |mut head: Value, reason: &str| {
            head["available"] = json!(false);
            head["unavailable_reason"] = json!(reason);
            head["as_of"] = json!(now / 1_000_000);
            head
        };
        if !is_trading_day(day) {
            return unavailable(head, "not_a_trading_day");
        }
        if day > today {
            return unavailable(head, "in_the_future");
        }
        let Some((tape, path)) = tape_path(a, day, symbol) else {
            let r = self.0.src.archive.retention_days;
            let expired = r > 0 && day < now_date(now) - Days::days(r as i64);
            return unavailable(
                head,
                if expired {
                    "retention_expired"
                } else {
                    "not_archived"
                },
            );
        };
        head["name"] = json!(self.name(symbol));

        let ledger_file =
            crate::notices::day_file(self.0.notices.dir().unwrap_or(Path::new("")), day);
        let transitions_file = self.0.src.transitions_dir.join(format!("{day}.jsonl"));
        let boots = boots(a, self.0.src.server_logs.as_deref(), day);
        // A past day's cache, keyed by every input (module docs, "Cost"). The daily closes file
        // too: the previous close (and its split adjustment) is in the body, and a day built
        // before that file had the close would otherwise keep a missing change forever.
        let key = format!(
            "{SCHEMA}|{}|{}|{}|{tape}:{}|{}|{}|{}|{boots:?}",
            crate::COMMIT,
            stock_context::Policy::current().tag(),
            stock_context::DetectionPolicy::current().tag(),
            len(&path),
            len(&ledger_file),
            len(&transitions_file),
            len(&a.daily_closes_path()),
        );
        let cache = (!ongoing)
            .then(|| self.0.src.cache_dir.as_ref())
            .flatten()
            .map(|d| d.join(day.to_string()).join(format!("{symbol}.json")));
        if let Some(c) = &cache
            && let Some(v) = read_cache(c, &key)
        {
            return merge(head, v, now);
        }

        let (items, _, _) = self.0.notices.items(day);
        let notices: Vec<Value> = items
            .into_iter()
            .filter(|i| i.get("symbol").and_then(Value::as_str) == Some(symbol))
            .collect();
        let watch_from = notices
            .iter()
            .filter(|i| {
                i.get("notice")
                    .and_then(Value::as_str)
                    .is_some_and(|n| RUNNERS_FLAGS.contains(&n))
            })
            .filter_map(row_at)
            .min();
        let ledger_member = notices.iter().any(|i| {
            i.get("notice")
                .and_then(Value::as_str)
                .is_some_and(|n| n.starts_with("ross."))
        });
        let live = live_episodes(&transitions_file, symbol);
        let input = ReplayInput {
            symbol: symbol.to_string(),
            watch_from,
            boots: boots.clone(),
            live_warm: live.iter().map(|(k, v)| (k.clone(), v.warm)).collect(),
            ledger_member,
            until: ongoing.then_some(now),
            restarts: replay::RestartModel::for_day(day),
        };
        let started = std::time::Instant::now();
        let events = read_events(&path);
        let out = replay::run(events, input);
        let elapsed = started.elapsed();
        let day_end = SessionId::new(day, SessionKind::After)
            .bounds()
            .map_or(now, |(_, e)| e.0);
        let float = a.float_as_of(symbol, day_end.min(now)).and_then(|r| {
            Some(FloatFact {
                shares: r.float_shares?,
                retrieved_at: r.retrieved_at,
                provider: r.provider,
                quality: r.quality,
            })
        });
        let records = DayRecords {
            symbol: symbol.to_string(),
            day,
            name: None,
            prev_close: self
                .prev_closes()
                .before(symbol, day)
                .map(|(d, p)| (d, p.micros())),
            float,
            notices,
            ledger_known: ledger_file.is_file() || ongoing,
            live,
            boots,
            tape,
            ongoing,
        };
        let mut body = package::build(&records, &out);
        if let Some(m) = body.get_mut("missed").and_then(Value::as_array_mut) {
            m.truncate(MAX_MISSED);
        }
        body["source"] = json!({
            "tape": tape,
            "events": out.events,
            "boots": records.boots.iter().map(|b| b / 1_000_000).collect::<Vec<_>>(),
            "journal": !records.live.is_empty(),
            "ledger": records.ledger_known,
            // Membership from the engine without the day's pillar evidence, or a Ross notice in
            // the ledger (`replay` module docs).
            "member": out.engine_member || ledger_member,
            "member_basis": "ledger_or_engine_without_evidence",
            "detection_panel": false,
            "build_ms": elapsed.as_millis() as u64,
        });
        tracing::debug!(symbol, %day, events = out.events, ms = elapsed.as_millis() as u64, "insights: ticker-day built");
        if let Some(c) = &cache {
            write_cache(c, &key, &body);
            if let Some(dir) = &self.0.src.cache_dir {
                prune_cache(dir, self.0.src.archive.retention_days, now_date(now));
            }
        }
        merge(head, body, now)
    }

    /// [`Insights::package_blocking`] off the async workers, at most [`MAX_BUILDS`] at once. The
    /// permit moves into the blocking task: a caller that goes away (an HTTP client hanging up
    /// drops this future) cannot free it while its build is still running, so the bound holds.
    pub async fn package(&self, symbol: String, day: NaiveDate) -> Value {
        let Ok(permit) = self.0.permits.clone().acquire_owned().await else {
            return json!({"available": false, "unavailable_reason": "build_failed"});
        };
        let me = self.clone();
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            me.package_blocking(&symbol, day, now_ns())
        })
        .await
        .unwrap_or_else(|_| json!({"available": false, "unavailable_reason": "build_failed"}))
    }

    /// Today's package of `symbol`, kept current for as long as anyone holds the receiver
    /// (module docs). `None` until the first build. One background task per symbol-day, shared.
    pub fn follow(&self, symbol: String, day: NaiveDate) -> watch::Receiver<Option<Arc<String>>> {
        let key = (symbol, day);
        let mut w = self.0.watchers.lock().expect("insights watchers");
        if let Some(tx) = w.get(&key) {
            return tx.subscribe();
        }
        let (tx, rx) = watch::channel(None);
        w.insert(key.clone(), tx.clone());
        let me = self.clone();
        tokio::spawn(async move { me.watch(key, tx).await });
        rx
    }

    async fn watch(self, key: Key, tx: watch::Sender<Option<Arc<String>>>) {
        let mut appends = self.0.notices.subscribe();
        let mut last: Option<u64> = None;
        loop {
            let mut v = self.package(key.0.clone(), key.1).await;
            // Changed = anything but the build's own clock.
            let as_of = v.as_object_mut().and_then(|m| m.remove("as_of"));
            let mut h = std::collections::hash_map::DefaultHasher::new();
            v.to_string().hash(&mut h);
            let h = h.finish();
            if last != Some(h) {
                last = Some(h);
                if let Some(t) = as_of {
                    v["as_of"] = t;
                }
                tx.send_replace(Some(Arc::new(v.to_string())));
            }
            let symbol = key.0.clone();
            let notice = async {
                loop {
                    match appends.recv().await {
                        Ok(a) if a.symbol.as_deref() == Some(symbol.as_str()) => return,
                        Ok(_) => {}
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => return,
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                            std::future::pending::<()>().await
                        }
                    }
                }
            };
            tokio::select! {
                _ = tokio::time::sleep(REFRESH) => {}
                _ = notice => tokio::time::sleep(DEBOUNCE).await,
                _ = tx.closed() => {}
            }
            // Decided under the map's lock, so a `follow` either finds this task alive (and its
            // receiver keeps it so) or finds no entry and starts a new one.
            let mut w = self.0.watchers.lock().expect("insights watchers");
            if tx.receiver_count() == 0 {
                if w.get(&key).is_some_and(|t| t.same_channel(&tx)) {
                    w.remove(&key);
                }
                return;
            }
        }
    }
}

/// The ET calendar date of an instant (retention is counted in calendar days).
fn now_date(ns: i64) -> NaiveDate {
    crate::oplog::et_date_ms(ns / 1_000_000)
}

fn merge(mut head: Value, body: Value, now: i64) -> Value {
    if let (Some(h), Value::Object(b)) = (head.as_object_mut(), body) {
        for (k, v) in b {
            h.entry(k).or_insert(v);
        }
        h.insert("as_of".into(), json!(now / 1_000_000));
    }
    head
}

/// Removes cached days the archive no longer keeps (`retention_days`; 0 keeps everything): their
/// tape is gone, so the page answers `retention_expired` before it would ever read them, and
/// without this the cache would outgrow the archive it was made from.
fn prune_cache(dir: &Path, retention_days: u32, today: NaiveDate) {
    if retention_days == 0 {
        return;
    }
    let cutoff = today - Days::days(i64::from(retention_days));
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let name = e.file_name();
        let Some(d) = name
            .to_str()
            .and_then(|n| NaiveDate::parse_from_str(n, "%Y-%m-%d").ok())
        else {
            continue;
        };
        if d < cutoff {
            let _ = std::fs::remove_dir_all(e.path());
        }
    }
}

fn read_cache(path: &Path, key: &str) -> Option<Value> {
    let v: Value = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
    (v.get("key")?.as_str()? == key).then(|| v.get("package").cloned())?
}

/// Atomic (temp + rename); a failure only costs a rebuild next time.
fn write_cache(path: &Path, key: &str, body: &Value) {
    let Some(dir) = path.parent() else { return };
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }
    let tmp = path.with_extension("json.tmp");
    let text = json!({"key": key, "package": body}).to_string();
    if std::fs::write(&tmp, text).is_ok() && std::fs::rename(&tmp, path).is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
}

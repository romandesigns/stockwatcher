//! The daily scorecard (owner, 2026-10-26: "carry on with the implementations"; the scorecard had
//! been built by hand, `C:\research\stockwatcher\reports\scorecard`): every night, for each alert
//! kind, how many fired, how many reached a phone or a desk and why the rest were held, how many
//! followed through, how big the move was, how fast the push was and how late in the move it came.
//! The owner sees from it whether each strategy earns its notifications.
//!
//! # Inputs (all on the archive volume, read only)
//!
//! - `rocket-watch/<date>.jsonl` (`crates/server/src/rocket/log.rs`): every Runners entry, by path
//!   (`alert` = R2, `burst`, `running`, `run`, `fast`), since 2026-10-22.
//! - `notices/<date>.jsonl` (`crates/server/src/alerts/ledger.rs`, since 2026-10-26): every notice
//!   the server decided, per path (`phone`, `desk`), sent or held with the limit's code, and the
//!   push latency (`pushed` lines). The other kinds (pullback, Ross / Signals crossing and trigger,
//!   shelf, follow-up, breakout, halt) come only from here.
//! - The day's trades from the imported archive. A day is scored only once the nightly import has
//!   made it ready ([`Archive::ready_days`]): the live recording can be torn or incomplete (it was,
//!   2026-10-26) and the import is the complete tape. Until then the API says "scored after the
//!   nightly import".
//!
//! # The rules (the hand-made scorecard's, so the trend is continuous)
//!
//! - **Follow-through**: from the alert price, a counted print at or above +10 % before one at or
//!   below −5 %, within 30 minutes of the alert, print by print ([`bracket`]). Neither within 30
//!   minutes is a timeout, not a loss. Prints are those that move OHLC (`Trade::effect`, so
//!   canceled / incorrect and restricted conditions are out); corrections are not replayed, which
//!   can differ from the chart on the rare symbol-day that has them.
//! - **Max gain / drawdown**: the highest and lowest counted print in those 30 minutes against the
//!   alert price, in percent.
//! - **How late**: `(alert price − L) / (H − L)` with `L` the lowest print in the 60 minutes before
//!   the alert and `H` the highest in the 60 minutes after (research/post-alert-timing,
//!   `lateness.py`): the share of that hour's move already behind the alert. Hindsight by
//!   construction: it describes the alert, it never fed one.
//! - **Latency**: the ledger's `trade_to_expo_ms` (print to push service) per pushed notice.
//! - **Segments** (ET): `open_print` 04:00:00–04:02:00, `pre` to 09:30, `mkt_open` 09:30–10:00,
//!   `rth` 10:00–16:00, `after` 16:00–20:00, `overnight` the rest.
//! - **Trade-report batch** (research/fast-0800): off-exchange prints executed while FINRA's TRF was
//!   closed arrive in one batch at its open (04:00 ET since 2026-03-30), venue `D` with condition
//!   `T`. An alert whose last 60 s of prints are mostly such prints is counted under `batch` as
//!   well, so the scorecard shows how much of a kind fires on reporting rather than trading.
//!
//! # Output
//!
//! `scorecard/<date>.json`, one object (`"v": 1`, see [`DayCard`]), written atomically. A day is
//! scored again when its input logs changed size since (the file records them) and never
//! otherwise, so re-running is free. Files older than [`RETENTION_DAYS`] are removed.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use chrono::{DateTime, NaiveDate, Timelike, Utc};
use chrono_tz::America::New_York;
use market_archive::Archive;
use market_events::MarketEvent;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Where the cards are written, under the archive root.
pub const DIR: &str = "scorecard";
/// About 19 months of trading days: long enough for a seasonal comparison, a few MB in all.
pub const RETENTION_DAYS: i64 = 400;
/// The bracket (the hand-made scorecard's): +10 % before −5 % within 30 minutes.
pub const TARGET: f64 = 0.10;
pub const STOP: f64 = 0.05;
pub const WINDOW_NS: i64 = 30 * 60 * 1_000_000_000;
/// The lateness hour on each side of the alert.
pub const HOUR_NS: i64 = 60 * 60 * 1_000_000_000;
/// The batch test looks at the minute before the alert; mostly = more than half of its prints.
pub const BATCH_LOOKBACK_NS: i64 = 60 * 1_000_000_000;

/// One alert to score: what fired, when, at what price, and what the ledger says became of it.
#[derive(Clone, Debug, PartialEq)]
pub struct Event {
    /// The data kind clients see (`runners.burst`, `ross.crossing`, `runners.pullback.setup`, …).
    pub kind: String,
    pub symbol: String,
    pub at_ns: i64,
    pub price: f64,
    /// The phone and desk decisions (`sent` or the hold code), when the ledger has them.
    pub phone: Option<String>,
    pub desk: Option<String>,
    pub latency_ms: Option<f64>,
}

/// What the tape said after (and before) one alert.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Outcome {
    /// `Some(true)` follow-through, `Some(false)` stopped, `None` neither within 30 minutes.
    pub hit: Option<bool>,
    pub max_gain_pct: f64,
    pub max_drawdown_pct: f64,
    /// Share of the hour's move before the alert, when there was a move.
    pub share_before: Option<f64>,
    pub batch: bool,
}

/// One print: execution time (ns) and price. `ohlc` = it moves the price (`Trade::effect`; the
/// bracket, gain, drawdown and lateness read only these). `d_t` = venue `D` with condition `T`.
/// Every print counts toward the batch test: the trade-report batch is mostly odd lots, which move
/// no price but are exactly what the batch is made of.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Print {
    pub ns: i64,
    pub price: f64,
    pub ohlc: bool,
    pub d_t: bool,
}

/// The bracket outcome, gain, drawdown, lateness and batch flag of an alert at `t0`, `p0` over
/// `prints` sorted by time. `None` when there is no print after the alert at all (nothing to
/// score: the stock stopped trading, or the archive lacks it).
pub fn score(all: &[Print], t0: i64, p0: f64) -> Option<Outcome> {
    // A missing, zero or NaN alert price has no bracket.
    if p0.is_nan() || p0 <= 0.0 {
        return None;
    }
    let prints: Vec<Print> = all.iter().filter(|p| p.ohlc).copied().collect();
    let after: Vec<&Print> = prints
        .iter()
        .filter(|p| p.ns > t0 && p.ns <= t0 + WINDOW_NS)
        .collect();
    if after.is_empty() {
        return None;
    }
    let (up, down) = (p0 * (1.0 + TARGET), p0 * (1.0 - STOP));
    let mut hit = None;
    let (mut hi, mut lo) = (p0, p0);
    for p in &after {
        hi = hi.max(p.price);
        lo = lo.min(p.price);
        if hit.is_none() {
            if p.price >= up {
                hit = Some(true);
            } else if p.price <= down {
                hit = Some(false);
            }
        }
    }
    let before_lo = prints
        .iter()
        .filter(|p| p.ns >= t0 - HOUR_NS && p.ns <= t0)
        .map(|p| p.price)
        .fold(f64::INFINITY, f64::min);
    let after_hi = prints
        .iter()
        .filter(|p| p.ns > t0 && p.ns <= t0 + HOUR_NS)
        .map(|p| p.price)
        .fold(f64::NEG_INFINITY, f64::max);
    let share_before = (before_lo.is_finite() && after_hi.is_finite() && after_hi > before_lo)
        .then(|| ((p0 - before_lo) / (after_hi - before_lo)).clamp(0.0, 1.0));
    let recent: Vec<&Print> = all
        .iter()
        .filter(|p| p.ns > t0 - BATCH_LOOKBACK_NS && p.ns <= t0)
        .collect();
    let batch = !recent.is_empty() && recent.iter().filter(|p| p.d_t).count() * 2 > recent.len();
    Some(Outcome {
        hit,
        max_gain_pct: (hi / p0 - 1.0) * 100.0,
        max_drawdown_pct: (lo / p0 - 1.0) * 100.0,
        share_before,
        batch,
    })
}

/// The ET session segment of an instant (module docs, "Segments").
pub fn segment(ns: i64) -> &'static str {
    let t = DateTime::<Utc>::from_timestamp_nanos(ns).with_timezone(&New_York);
    let s = t.hour() * 3600 + t.minute() * 60 + t.second();
    match s {
        x if (4 * 3600..4 * 3600 + 120).contains(&x) => "open_print",
        x if (4 * 3600 + 120..9 * 3600 + 1800).contains(&x) => "pre",
        x if (9 * 3600 + 1800..10 * 3600).contains(&x) => "mkt_open",
        x if (10 * 3600..16 * 3600).contains(&x) => "rth",
        x if (16 * 3600..20 * 3600).contains(&x) => "after",
        _ => "overnight",
    }
}

/// The Runners entry kinds the forward log carries, as the data kind clients see.
fn runners_kind(log_kind: &str) -> Option<&'static str> {
    Some(match log_kind {
        "alert" => "runners.new",
        "burst" => "runners.burst",
        "running" => "runners.running",
        "run" => "runners.run",
        "fast" => "runners.fast",
        _ => return None,
    })
}

/// A ledger decision's kind: the data kind, with the pullback's two stages told apart. Both share
/// the data kind `runners.pullback`; a `type` field is used when a ledger line carries one, else
/// the title, which says "Crossing" only for the follow-up (`crate::alerts::runners` text).
fn ledger_kind(line: &Value) -> Option<String> {
    let notice = line.get("notice")?.as_str()?;
    if notice != "runners.pullback" {
        return Some(notice.to_string());
    }
    let crossing = match line.get("type").and_then(Value::as_str) {
        Some(t) => t == "pullback_crossing",
        None => line
            .get("title")
            .and_then(Value::as_str)
            .is_some_and(|t| t.contains("Crossing")),
    };
    Some(
        if crossing {
            "runners.pullback.crossing"
        } else {
            "runners.pullback.setup"
        }
        .to_string(),
    )
}

/// The events of a day from its two logs (module docs, "Inputs"). Runners entries come from the
/// forward log (complete since it began) and take their decisions from the ledger by kind,
/// symbol and time; every other kind is one event per ledger `event_id`.
pub fn events(rocket_watch: &str, ledger: &str) -> Vec<Event> {
    struct Decided {
        kind: String,
        symbol: String,
        at_ns: i64,
        price: f64,
        phone: Option<String>,
        desk: Option<String>,
    }
    let mut by_id: BTreeMap<String, Decided> = BTreeMap::new();
    let mut latency: HashMap<String, f64> = HashMap::new();
    for line in ledger
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
    {
        match line.get("kind").and_then(Value::as_str) {
            Some("decision") => {
                let (Some(id), Some(kind), Some(symbol)) = (
                    line.get("event_id").and_then(Value::as_str),
                    ledger_kind(&line),
                    line.get("symbol").and_then(Value::as_str),
                ) else {
                    continue;
                };
                let word = match line.get("decision").and_then(Value::as_str) {
                    Some("sent") => "sent".to_string(),
                    _ => line
                        .get("held")
                        .and_then(Value::as_str)
                        .unwrap_or("held")
                        .to_string(),
                };
                let d = by_id.entry(id.to_string()).or_insert_with(|| Decided {
                    kind,
                    symbol: symbol.to_string(),
                    at_ns: line.get("at_ns").and_then(Value::as_i64).unwrap_or(0),
                    price: line.get("price").and_then(Value::as_f64).unwrap_or(0.0),
                    phone: None,
                    desk: None,
                });
                match line.get("path").and_then(Value::as_str) {
                    Some("phone") => d.phone = Some(word),
                    Some("desk") => d.desk = Some(word),
                    _ => {}
                }
            }
            Some("pushed") => {
                if let (Some(id), Some(ms)) = (
                    line.get("event_id").and_then(Value::as_str),
                    line.get("trade_to_expo_ms").and_then(Value::as_f64),
                ) {
                    latency.insert(id.to_string(), ms);
                }
            }
            _ => {}
        }
    }
    let mut out = Vec::new();
    let mut used: Vec<String> = Vec::new();
    for line in rocket_watch
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
    {
        let Some(kind) = line
            .get("kind")
            .and_then(Value::as_str)
            .and_then(runners_kind)
        else {
            continue;
        };
        let (Some(symbol), Some(at_ns), Some(price)) = (
            line.get("symbol").and_then(Value::as_str),
            line.get("alert_ns").and_then(Value::as_i64),
            line.get("price").and_then(Value::as_f64),
        ) else {
            continue;
        };
        // The ledger's decision for this entry: same kind and stock, decided within two minutes.
        let matched = by_id
            .iter()
            .filter(|(id, d)| {
                d.kind == kind
                    && d.symbol == symbol
                    && (d.at_ns - at_ns).abs() <= 120_000_000_000
                    && !used.contains(id)
            })
            .min_by_key(|(_, d)| (d.at_ns - at_ns).abs())
            .map(|(id, _)| id.clone());
        let (phone, desk, lat) = match &matched {
            Some(id) => {
                used.push(id.clone());
                let d = &by_id[id];
                (d.phone.clone(), d.desk.clone(), latency.get(id).copied())
            }
            None => (None, None, None),
        };
        out.push(Event {
            kind: kind.to_string(),
            symbol: symbol.to_string(),
            at_ns,
            price,
            phone,
            desk,
            latency_ms: lat,
        });
    }
    for (id, d) in by_id {
        // Matched to a forward-log entry already; a Runners notice the log lacks still counts.
        if used.contains(&id) {
            continue;
        }
        out.push(Event {
            kind: d.kind,
            symbol: d.symbol,
            at_ns: d.at_ns,
            price: d.price,
            phone: d.phone,
            desk: d.desk,
            latency_ms: latency.get(&id).copied(),
        });
    }
    out.sort_by(|a, b| a.at_ns.cmp(&b.at_ns).then(a.symbol.cmp(&b.symbol)));
    out
}

/// Counts for one kind, or one segment of it.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Tally {
    pub fired: u32,
    /// Fired alerts with prints after them (the rest cannot be scored).
    pub scored: u32,
    pub follow_through: u32,
    pub stopped: u32,
}

impl Tally {
    fn add(&mut self, o: Option<&Outcome>) {
        self.fired += 1;
        if let Some(o) = o {
            self.scored += 1;
            match o.hit {
                Some(true) => self.follow_through += 1,
                Some(false) => self.stopped += 1,
                None => {}
            }
        }
    }
}

/// One kind's line on the card.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct KindCard {
    pub kind: String,
    #[serde(flatten)]
    pub tally: Tally,
    /// Follow-through share of the scored alerts (`None` when none was scored).
    pub rate: Option<f64>,
    /// Phone and desk decisions: `sent`, the hold codes, and `unknown` (no ledger line: before
    /// 2026-10-26, or a kind the ledger does not see).
    pub phone: BTreeMap<String, u32>,
    pub desk: BTreeMap<String, u32>,
    pub median_gain_pct: Option<f64>,
    pub median_drawdown_pct: Option<f64>,
    pub median_latency_ms: Option<f64>,
    pub median_share_before: Option<f64>,
    pub segments: BTreeMap<String, Tally>,
    /// The alerts that fired on a trade-report batch (module docs).
    pub batch: Tally,
}

/// The day's card (`scorecard/<date>.json`).
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct DayCard {
    pub v: u32,
    pub date: NaiveDate,
    pub computed_at: String,
    /// What it was scored from: the input logs' sizes, so a changed log is scored again.
    pub inputs: Inputs,
    pub rule: Rule,
    pub kinds: Vec<KindCard>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Inputs {
    pub rocket_watch_bytes: u64,
    pub notices_bytes: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Rule {
    pub target_pct: f64,
    pub stop_pct: f64,
    pub window_min: u32,
}

impl Default for Rule {
    fn default() -> Self {
        Rule {
            target_pct: TARGET * 100.0,
            stop_pct: STOP * 100.0,
            window_min: 30,
        }
    }
}

fn median(mut v: Vec<f64>) -> Option<f64> {
    if v.is_empty() {
        return None;
    }
    v.sort_by(|a, b| a.total_cmp(b));
    let n = v.len();
    Some(if n % 2 == 1 {
        v[n / 2]
    } else {
        (v[n / 2 - 1] + v[n / 2]) / 2.0
    })
}

/// The card from scored events (pure; [`score_day`] supplies the outcomes).
pub fn card(
    date: NaiveDate,
    inputs: Inputs,
    scored: &[(Event, Option<Outcome>)],
    now: DateTime<Utc>,
) -> DayCard {
    // Per kind: its card, then the samples its medians are taken over (gain, drawdown, latency,
    // share of the move before the alert).
    type Acc = (KindCard, Vec<f64>, Vec<f64>, Vec<f64>, Vec<f64>);
    let mut kinds: BTreeMap<String, Acc> = BTreeMap::new();
    for (e, o) in scored {
        let (k, gains, dds, lats, shares) = kinds.entry(e.kind.clone()).or_insert_with(|| {
            (
                KindCard {
                    kind: e.kind.clone(),
                    ..KindCard::default()
                },
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
            )
        });
        k.tally.add(o.as_ref());
        k.segments
            .entry(segment(e.at_ns).to_string())
            .or_default()
            .add(o.as_ref());
        if o.as_ref().is_some_and(|o| o.batch) {
            k.batch.add(o.as_ref());
        }
        *k.phone
            .entry(e.phone.clone().unwrap_or_else(|| "unknown".into()))
            .or_default() += 1;
        *k.desk
            .entry(e.desk.clone().unwrap_or_else(|| "unknown".into()))
            .or_default() += 1;
        if let Some(o) = o {
            gains.push(o.max_gain_pct);
            dds.push(o.max_drawdown_pct);
            if let Some(s) = o.share_before {
                shares.push(s);
            }
        }
        if let Some(l) = e.latency_ms {
            lats.push(l);
        }
    }
    let kinds = kinds
        .into_values()
        .map(|(mut k, g, d, l, s)| {
            k.rate = (k.tally.scored > 0)
                .then(|| f64::from(k.tally.follow_through) / f64::from(k.tally.scored));
            k.median_gain_pct = median(g);
            k.median_drawdown_pct = median(d);
            k.median_latency_ms = median(l);
            k.median_share_before = median(s);
            k
        })
        .collect();
    DayCard {
        v: 1,
        date,
        computed_at: now.to_rfc3339(),
        inputs,
        rule: Rule::default(),
        kinds,
    }
}

/// The prints of one symbol-day from the imported archive, within `[from, to]`: those that move
/// the price, and the rest (odd lots, average-price reports) for the batch test only. Canceled
/// and incorrect prints are left out entirely.
fn prints_of(archive: &Archive, day: NaiveDate, symbol: &str, from: i64, to: i64) -> Vec<Print> {
    let Ok(reader) = archive.reader(day, symbol) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for rec in reader {
        let Ok(rec) = rec else { break };
        if let MarketEvent::Trade(t) = rec.event {
            let ns = t.event_ts.nanos();
            if ns < from || ns > to || matches!(t.update.as_deref(), Some("canceled" | "incorrect"))
            {
                continue;
            }
            let d_t = t.exchange == "D" && t.conditions.iter().any(|c| c == "T");
            out.push(Print {
                ns,
                price: t.price.to_f64(),
                ohlc: t.effect().updates_ohlc,
                d_t,
            });
        }
    }
    out.sort_by_key(|p| p.ns);
    out
}

fn log_path(root: &Path, dir: &str, day: NaiveDate) -> PathBuf {
    root.join(dir).join(format!("{day}.jsonl"))
}

fn read_log(root: &Path, dir: &str, day: NaiveDate) -> (String, u64) {
    let p = log_path(root, dir, day);
    match std::fs::read_to_string(&p) {
        Ok(s) => {
            let n = s.len() as u64;
            (s, n)
        }
        Err(_) => (String::new(), 0),
    }
}

/// The path of a day's card.
pub fn card_path(root: &Path, day: NaiveDate) -> PathBuf {
    root.join(DIR).join(format!("{day}.json"))
}

/// Reads a day's card, if one was written.
pub fn read_card(root: &Path, day: NaiveDate) -> Option<DayCard> {
    serde_json::from_slice(&std::fs::read(card_path(root, day)).ok()?).ok()
}

/// What [`run`] did, for the maintainer's status line.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RunReport {
    pub written: Vec<NaiveDate>,
    pub unchanged: usize,
    pub pruned: usize,
    pub errors: Vec<String>,
}

/// Scores one ready day from its logs and trades. Reads each symbol once, over the span its
/// alerts need (an hour either side), so memory is bounded by one symbol's prints at a time.
pub fn score_day(
    archive: &Archive,
    day: NaiveDate,
    rocket_watch: &str,
    ledger: &str,
) -> Vec<(Event, Option<Outcome>)> {
    let evs = events(rocket_watch, ledger);
    let mut by_symbol: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
    for (i, e) in evs.iter().enumerate() {
        by_symbol.entry(e.symbol.as_str()).or_default().push(i);
    }
    let mut outcomes: Vec<Option<Outcome>> = vec![None; evs.len()];
    for (symbol, idx) in &by_symbol {
        let from = idx.iter().map(|&i| evs[i].at_ns).min().unwrap_or(0) - HOUR_NS;
        let to = idx.iter().map(|&i| evs[i].at_ns).max().unwrap_or(0) + HOUR_NS;
        let prints = prints_of(archive, day, symbol, from, to);
        for &i in idx {
            outcomes[i] = score(&prints, evs[i].at_ns, evs[i].price);
        }
    }
    evs.into_iter().zip(outcomes).collect()
}

/// Writes the card of every ready day that has logs and no up-to-date card, at most `max` a run
/// (newest first, so tonight's day is never queued behind a backfill), then prunes old cards.
pub fn run(
    archive: &Archive,
    now: DateTime<Utc>,
    max: usize,
    stop: &dyn Fn() -> bool,
) -> RunReport {
    let root = archive.root().to_path_buf();
    let mut r = RunReport::default();
    let mut days = match archive.ready_days() {
        Ok(d) => d,
        Err(e) => {
            r.errors.push(format!("scorecard: {e}"));
            return r;
        }
    };
    days.sort_unstable_by(|a, b| b.cmp(a));
    for day in days {
        if r.written.len() >= max || stop() {
            break;
        }
        let (rw, rw_n) = read_log(&root, "rocket-watch", day);
        let (nl, nl_n) = read_log(&root, "notices", day);
        if rw_n == 0 && nl_n == 0 {
            continue;
        }
        let inputs = Inputs {
            rocket_watch_bytes: rw_n,
            notices_bytes: nl_n,
        };
        if read_card(&root, day).is_some_and(|c| c.inputs == inputs) {
            r.unchanged += 1;
            continue;
        }
        let c = card(day, inputs, &score_day(archive, day, &rw, &nl), now);
        match write_atomic(&card_path(&root, day), &c) {
            Ok(()) => r.written.push(day),
            Err(e) => r.errors.push(format!("scorecard {day}: {e}")),
        }
    }
    r.pruned = prune(&root, now.date_naive());
    r
}

fn write_atomic(path: &Path, c: &DayCard) -> Result<(), String> {
    let dir = path.parent().ok_or("no parent")?;
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let tmp = path.with_extension("json.partial");
    let json = serde_json::to_vec_pretty(c).map_err(|e| e.to_string())?;
    std::fs::write(&tmp, json).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, path).map_err(|e| e.to_string())
}

/// Removes cards older than [`RETENTION_DAYS`] before `today`; returns how many.
pub fn prune(root: &Path, today: NaiveDate) -> usize {
    let Ok(rd) = std::fs::read_dir(root.join(DIR)) else {
        return 0;
    };
    let cut = today - chrono::Duration::days(RETENTION_DAYS);
    let mut n = 0;
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        let Some(stem) = name.strip_suffix(".json") else {
            continue;
        };
        if NaiveDate::parse_from_str(stem, "%Y-%m-%d").is_ok_and(|d| d < cut)
            && std::fs::remove_file(e.path()).is_ok()
        {
            n += 1;
        }
    }
    n
}

/// The newest `days` cards under `root`, newest first (the API's read; skips unreadable files).
pub fn recent(root: &Path, days: usize) -> Vec<DayCard> {
    let Ok(rd) = std::fs::read_dir(root.join(DIR)) else {
        return Vec::new();
    };
    let mut dates: Vec<NaiveDate> = rd
        .flatten()
        .filter_map(|e| {
            e.file_name()
                .to_str()?
                .strip_suffix(".json")
                .and_then(|s| NaiveDate::parse_from_str(s, "%Y-%m-%d").ok())
        })
        .collect();
    dates.sort_unstable_by(|a, b| b.cmp(a));
    dates
        .into_iter()
        .take(days)
        .filter_map(|d| read_card(root, d))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const S: i64 = 1_000_000_000;
    /// 2026-10-26 10:30:00 ET (EDT, UTC−4).
    const T0: i64 = 1_793_025_000 * S;

    fn p(sec: i64, price: f64) -> Print {
        Print {
            ns: T0 + sec * S,
            price,
            ohlc: true,
            d_t: false,
        }
    }

    #[test]
    fn follow_through_is_plus_ten_before_minus_five_within_thirty_minutes() {
        let up = [p(-30, 1.0), p(60, 1.05), p(120, 1.11), p(180, 0.90)];
        let o = score(&up, T0, 1.0).unwrap();
        assert_eq!(o.hit, Some(true));
        assert!((o.max_gain_pct - 11.0).abs() < 1e-9 && (o.max_drawdown_pct + 10.0).abs() < 1e-9);
        let down = [p(60, 0.94), p(120, 1.20)];
        assert_eq!(score(&down, T0, 1.0).unwrap().hit, Some(false));
        // Neither within 30 minutes: a timeout, and a move after 30 minutes does not count.
        let flat = [p(60, 1.02), p(31 * 60, 1.5)];
        assert_eq!(score(&flat, T0, 1.0).unwrap().hit, None);
        // No print after the alert (a halt to the end of the window): nothing to score.
        assert_eq!(score(&[p(-10, 1.0)], T0, 1.0), None);
    }

    #[test]
    fn lateness_is_the_share_of_the_hours_move_before_the_alert_and_batches_are_flagged() {
        let prints = [p(-50 * 60, 1.0), p(-10, 1.6), p(600, 2.0)];
        let o = score(&prints, T0, 1.6).unwrap();
        assert!((o.share_before.unwrap() - 0.6).abs() < 1e-9);
        assert!(!o.batch);
        let mut batch: Vec<Print> = (0..10)
            .map(|i| Print {
                ns: T0 - i * S,
                price: 1.0,
                ohlc: i >= 7,
                d_t: i < 7,
            })
            .collect();
        batch.sort_by_key(|p| p.ns);
        batch.push(p(30, 1.0));
        assert!(score(&batch, T0, 1.0).unwrap().batch);
    }

    #[test]
    fn segments_follow_eastern_time() {
        // 2026-10-26 04:01 ET = 08:01 UTC; 09:45 ET; 15:59 ET; 17:00 ET.
        let at = |h: i64, m: i64| (1_793_000_000 / 86_400 * 86_400 + (h + 4) * 3600 + m * 60) * S;
        assert_eq!(segment(at(4, 1)), "open_print");
        assert_eq!(segment(at(7, 0)), "pre");
        assert_eq!(segment(at(9, 45)), "mkt_open");
        assert_eq!(segment(at(15, 59)), "rth");
        assert_eq!(segment(at(17, 0)), "after");
    }

    #[test]
    fn events_join_runners_entries_to_their_ledger_decisions_and_keep_other_kinds() {
        let rw = format!(
            "{}\n{}\n{{\"kind\":\"later\",\"symbol\":\"SRTM\"}}\n",
            serde_json::json!({"kind":"run","v":1,"symbol":"SRTM","alert_ns":T0,"price":0.55}),
            serde_json::json!({"kind":"burst","v":1,"symbol":"SRTM","alert_ns":T0 + 178 * S,"price":0.60}),
        );
        let ledger = [
            serde_json::json!({"kind":"decision","event_id":"r1","notice":"runners.run","symbol":"SRTM","price":0.55,"at_ns":T0 + 1,"path":"phone","decision":"sent","held":null}),
            serde_json::json!({"kind":"decision","event_id":"r1","notice":"runners.run","symbol":"SRTM","price":0.55,"at_ns":T0 + 1,"path":"desk","decision":"sent","held":null}),
            serde_json::json!({"kind":"pushed","event_id":"r1","trade_to_expo_ms":1044.6}),
            serde_json::json!({"kind":"decision","event_id":"p1","notice":"runners.pullback","symbol":"SRTM","price":0.56,"at_ns":T0 + 223 * S,"path":"phone","decision":"held","held":"symbol_limit","title":"SRTM · Pullback setting up · $0.56"}),
            serde_json::json!({"kind":"decision","event_id":"p2","notice":"runners.pullback","type":"pullback_crossing","symbol":"SRTM","price":0.58,"at_ns":T0 + 240 * S,"path":"phone","decision":"sent","held":null}),
        ]
        .map(|v| v.to_string())
        .join("\n");
        let e = events(&rw, &ledger);
        let kinds: Vec<_> = e
            .iter()
            .map(|x| (x.kind.as_str(), x.phone.as_deref()))
            .collect();
        assert_eq!(
            kinds,
            vec![
                ("runners.run", Some("sent")),
                ("runners.burst", None),
                ("runners.pullback.setup", Some("symbol_limit")),
                ("runners.pullback.crossing", Some("sent")),
            ]
        );
        assert_eq!(e[0].latency_ms, Some(1044.6));
    }

    #[test]
    fn a_card_counts_tallies_rates_segments_and_unknown_decisions() {
        let ev = |k: &str, phone: Option<&str>| Event {
            kind: k.into(),
            symbol: "A".into(),
            at_ns: T0,
            price: 1.0,
            phone: phone.map(Into::into),
            desk: None,
            latency_ms: Some(900.0),
        };
        let won = Outcome {
            hit: Some(true),
            max_gain_pct: 12.0,
            max_drawdown_pct: -1.0,
            share_before: Some(0.5),
            batch: false,
        };
        let lost = Outcome {
            hit: Some(false),
            max_gain_pct: 2.0,
            max_drawdown_pct: -6.0,
            share_before: Some(0.9),
            batch: true,
        };
        let c = card(
            NaiveDate::from_ymd_opt(2026, 10, 26).unwrap(),
            Inputs::default(),
            &[
                (ev("runners.burst", Some("sent")), Some(won)),
                (ev("runners.burst", None), Some(lost)),
                (ev("runners.burst", Some("symbol_limit")), None),
            ],
            Utc::now(),
        );
        let k = &c.kinds[0];
        assert_eq!(
            (
                k.tally.fired,
                k.tally.scored,
                k.tally.follow_through,
                k.tally.stopped
            ),
            (3, 2, 1, 1)
        );
        assert_eq!(k.rate, Some(0.5));
        assert_eq!(k.phone.get("sent"), Some(&1));
        assert_eq!(k.phone.get("symbol_limit"), Some(&1));
        assert_eq!(k.phone.get("unknown"), Some(&1));
        assert_eq!(k.batch.fired, 1);
        assert_eq!(k.segments.get("rth").map(|t| t.fired), Some(3));
        assert_eq!(k.median_share_before, Some(0.7));
        assert_eq!(k.median_latency_ms, Some(900.0));
    }

    #[test]
    fn cards_are_written_atomically_skipped_when_unchanged_and_pruned_by_age() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let day = NaiveDate::from_ymd_opt(2026, 10, 26).unwrap();
        let c = card(
            day,
            Inputs {
                rocket_watch_bytes: 10,
                notices_bytes: 0,
            },
            &[],
            Utc::now(),
        );
        write_atomic(&card_path(root, day), &c).unwrap();
        assert_eq!(read_card(root, day).unwrap().inputs, c.inputs);
        let old = NaiveDate::from_ymd_opt(2025, 1, 2).unwrap();
        write_atomic(
            &card_path(root, old),
            &card(old, Inputs::default(), &[], Utc::now()),
        )
        .unwrap();
        assert_eq!(prune(root, day), 1);
        assert_eq!(recent(root, 30).len(), 1);
        assert!(!card_path(root, day).with_extension("json.partial").exists());
    }
}

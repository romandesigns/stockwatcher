//! The forward log: the live track record of every Rocket watch alert.
//!
//! `ARCHIVE_ROOT/rocket-watch/<trading date>.jsonl`, append-only, one JSON object per line, ten
//! kinds (all with `"v": 1`):
//!
//! - `alert` — `{kind, v, id, date, symbol, alert_ns, price, score, threshold, mode, reasons,
//!   strategy, version}` when the alert fires. `symbol`/`alert_ns`/`score` are the bake-off's alert
//!   file fields, so the judge can score the live record exactly like a candidate's files.
//! - `outcome` — `{kind, v, id, date, symbol, alert_ns, settled_ns, rocket10, rocket30, start_ns,
//!   mark_ns, start_price, max_gain_pct, max_drawdown_pct, partial}` once the outcome is known
//!   (45 minutes after the alert, or at 20:00 ET with `partial: true`). `rocket10` is the
//!   bake-off's HIT (a canonical rocket starting 60–600 s after the alert), `rocket30` the same
//!   out to 1,800 s; gain and drawdown are the max high / min low over the 30 minutes after the
//!   alert against the alert price, in percent. Computed causally after the fact from the same
//!   15 s bars (`super::features::mark_at`), never from anything the alert could have seen.
//! - `later` — `{kind, v, id, date, symbol, alert_ns, at_ns, final, max_gain_2h_pct,
//!   max_drawdown_2h_pct, max_gain_session_pct, rocket_later, later_start_ns, later_mark_ns,
//!   complete_2h, resumed}`, the later-move outcomes (`super::track`): written when the 2 hours
//!   after the alert are up (`final: false`) and again at 20:00 ET (`final: true`, the session
//!   max final). Gains and drawdown are percent against the alert price; `rocket_later` = a
//!   canonical +30 % mark any time after the alert that day (a strict hit included);
//!   `complete_2h: false` when the day ended first; `resumed: true` when the process restarted
//!   after the alert (the values then rest on the logged lines plus the bars still in memory, so
//!   a move during the outage can be missing). The last `later` line per id wins.
//! - `state` — `{kind, v, id, date, symbol, alert_ns, at_ns, from, to, price, high, vwap,
//!   from_flag_pct, from_high_pct}` on each momentum-state transition of a flagged row
//!   (`super::track`, "Momentum state"), at most `MAX_TRANSITIONS` (1,000) per alert a day.
//! - `burst` — `{kind, v, id, date, symbol, alert_ns, boundary_ns, price, score, threshold,
//!   per_day, start_ns, start_price, gain_pct, move_pct, rate_x, new_highs, sweeps, buy_pct,
//!   reasons, features, strategy, version}` when a buying burst alerts (`super::burst`):
//!   `boundary_ns` is the 5 s boundary scored (`alert_ns` = it + 1 s), `features` the 34 research
//!   features by name (`null` = missing), `score` the model probability. Burst rows also get
//!   `later` and `state` lines (they are tracked like a flag) but never an `outcome` line, and do
//!   not count in the `day` line or the history.
//! - `burst_outcome` — `{kind, v, id, date, symbol, alert_ns, settled_ns, price, cont10, cont20,
//!   cont30, stopped, state, t20_s, mfe_pct, mae_pct, partial}` 30 minutes after a burst alert
//!   (protocol §3: +10/20/30 % before −5 %, print by print; MFE and the MAE before it, in
//!   percent), or at 20:00 ET / after a restart with `partial: true`.
//! - `running` — `{kind, v, id, date, symbol, alert_ns, price, score, reasons, strategy, version,
//!   base_low, gain, req, dollars5, rvol30}` when a running entry enters (`super::running`).
//! - `run` — `{kind, v, id, date, symbol, alert_ns, price, score, reasons, strategy, version,
//!   start_ns, start_price, plus20_ns, early, exec_ns}` when a "Run +20%" entry enters
//!   (`super::run`): `alert_ns` = `plus20_ns`, the second an early fire held (`early: true`) or
//!   the mark slot's end, and `price` the last price or the close; `exec_ns` the triggering print.
//! - `fast` — `{kind, v, id, date, symbol, alert_ns, price, score, reasons, strategy, version,
//!   sec, secs, held, low, gain, trades, dollars}` when a "Fast move" entry enters
//!   (`super::fast`): `sec` the triggering second (epoch s), `alert_ns` its end, `price` the last
//!   price; `secs` the arm's window and what the rule read in it.
//!   Running, run and fast rows also get `later` and `state` lines, never an `outcome` line, and
//!   do not count in the `day` line's `alerts` or the history.
//! - `day` — `{kind, v, date, alerts, hits10, hits30, ran_later, rows, ideal_threshold, strategy,
//!   version}` at
//!   20:00 ET (or at the next day's first barrier when the process was down at 20:00):
//!   `ideal_threshold` is the score that would have given exactly the budget that day, the
//!   adaptive threshold's prior for the next days; `rows` = `{runner, burst, running, run, fast}`, the
//!   day's rows by source (the owner's row budget across the five entry paths).
//!
//! Size: ~450 B per alert line, ~330 B per outcome, ~350 B per later line (two per alert),
//! ~280 B per state line (typically 5–20 per alert), ~200 B per day line: about 30–70 KB a day at
//! 10 alerts. Bounded anyway: a file stops growing at [`MAX_FILE_BYTES`] (logged),
//! and the alert policy caps alerts per day. Files older than the archive's retention window
//! (`ARCHIVE_RETENTION_DAYS`) are deleted at startup and at every day roll, like the days they
//! were recorded beside.
//!
//! Writes are a few small appends a day, done on the quotes task after the core lock is released
//! (never on the ingest path).

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use chrono::NaiveDate;
use serde_json::Value;

/// A day's file stops growing here (a runaway would otherwise fill the archive volume).
///
/// 8 MiB, was 2 MiB: busy days outgrew it. 2026-11-23 filled 2 MiB by 12:45 ET (from 04:00) and
/// lost the afternoon's forward record; 11-12 and 11-13 hit it in the evening. At that day's
/// rate a full 04:00-20:00 session is ~3.7 MiB, so this is twice the busiest day seen.
pub const MAX_FILE_BYTES: u64 = 8 * 1024 * 1024;
/// Days of track record kept in memory and sent to the panel.
pub const HISTORY_DAYS: usize = 20;
/// Exact tracking is replaceable state, separate from the capped append-only event journal.
const MAX_CHECKPOINT_BYTES: u64 = 64 * 1024;
const CHECKPOINT_PREFIX: &str = "breakout-state-";

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct DaySummary {
    pub date: NaiveDate,
    pub alerts: u32,
    pub hits10: u32,
    pub hits30: u32,
}

/// What a restart reads back.
#[derive(Debug, Default)]
pub struct Loaded {
    /// Today's alert lines, oldest first, each with its outcome line when one was written.
    pub today: Vec<(Value, Option<Value>)>,
    /// Today's `burst` lines, oldest first, each with its `burst_outcome` line when written.
    pub bursts: Vec<(Value, Option<Value>)>,
    /// Today's `running` lines (`super::running`), oldest first.
    pub running: Vec<Value>,
    /// Today's `run` lines (`super::run`), oldest first.
    pub runs: Vec<Value>,
    /// Today's `fast` lines (`super::fast`), oldest first.
    pub fasts: Vec<Value>,
    /// Up to [`HISTORY_DAYS`] earlier days, oldest first.
    pub days: Vec<DaySummary>,
    /// `(date, strategy, version, ideal_threshold)` from the earlier days' `day` lines.
    pub thresholds: Vec<(NaiveDate, String, String, f64)>,
    /// Today's last `later` line per alert id.
    pub later: BTreeMap<String, Value>,
    /// Today's last `state` line per alert id, with the number of `state` lines for it.
    pub states: BTreeMap<String, (Value, u32)>,
    /// Today's `also` lines (a row's follow-ups, `super::RocketBoard` "Follow-ups") per alert id,
    /// oldest first.
    pub also: BTreeMap<String, Vec<Value>>,
    /// Today's `crossing` lines (`super::crossing`): symbol → the row's crossing text.
    pub crossings: BTreeMap<String, String>,
}

#[derive(Debug)]
pub struct RocketLog {
    dir: PathBuf,
    retention_days: u32,
    checkpoint_writer: Mutex<()>,
}

fn date_of(path: &Path) -> Option<NaiveDate> {
    let name = path.file_name()?.to_str()?;
    let stem = name.strip_suffix(".jsonl")?;
    NaiveDate::parse_from_str(stem, "%Y-%m-%d").ok()
}

fn checkpoint_date(path: &Path) -> Option<NaiveDate> {
    let name = path.file_name()?.to_str()?;
    let name = name.strip_prefix(".pending-").unwrap_or(name);
    let rest = name
        .strip_prefix(CHECKPOINT_PREFIX)?
        .strip_suffix(".json")?;
    if rest.as_bytes().get(10) != Some(&b'-') {
        return None;
    }
    NaiveDate::parse_from_str(rest.get(..10)?, "%Y-%m-%d").ok()
}

fn checkpoint_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id.as_bytes()[0].is_ascii_alphanumeric()
        && id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_' | b'.'))
}

fn checkpoint_order(v: &Value) -> (i64, u64, u64) {
    let s = &v["breakout_track"];
    let e = s.get("exact").unwrap_or(s);
    (
        s.get("checkpoint_ns").and_then(Value::as_i64).unwrap_or(0),
        e.get("checked_key").and_then(Value::as_u64).unwrap_or(0),
        e.get("admission_cutoff")
            .and_then(Value::as_u64)
            .unwrap_or(0),
    )
}

impl RocketLog {
    pub fn new(dir: PathBuf, retention_days: u32) -> Self {
        RocketLog {
            dir,
            retention_days,
            checkpoint_writer: Mutex::new(()),
        }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn file(&self, date: NaiveDate) -> PathBuf {
        self.dir.join(format!("{date}.jsonl"))
    }

    fn checkpoint_file(&self, date: NaiveDate, id: &str) -> Option<PathBuf> {
        checkpoint_id(id).then(|| {
            self.dir
                .join(format!("{CHECKPOINT_PREFIX}{date}-{id}.json"))
        })
    }

    fn read_checkpoint(&self, date: NaiveDate, id: &str) -> Option<Value> {
        let path = self.checkpoint_file(date, id)?;
        if std::fs::metadata(&path).ok()?.len() > MAX_CHECKPOINT_BYTES {
            return None;
        }
        let v: Value = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
        (v["kind"] == "breakout_track"
            && v["id"] == id
            && v["date"] == date.to_string()
            && v["breakout_track"].is_object())
        .then_some(v)
    }

    fn write_checkpoint(&self, date: NaiveDate, v: &Value) -> std::io::Result<()> {
        let id = v["id"].as_str().unwrap_or_default();
        let path = self
            .checkpoint_file(date, id)
            .ok_or_else(|| std::io::Error::other("invalid checkpoint id"))?;
        if v["date"] != date.to_string() || !v["breakout_track"].is_object() {
            return Err(std::io::Error::other("missing exact tracking checkpoint"));
        }
        let bytes = serde_json::to_vec(v)?;
        if bytes.len() as u64 > MAX_CHECKPOINT_BYTES {
            return Err(std::io::Error::other(
                "exact tracking checkpoint at its size cap",
            ));
        }
        let _writer = self
            .checkpoint_writer
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        std::fs::create_dir_all(&self.dir)?;
        if self
            .read_checkpoint(date, id)
            .is_some_and(|old| checkpoint_order(&old) > checkpoint_order(v))
        {
            return Ok(());
        }
        let tmp = self.dir.join(format!(
            ".pending-{}",
            path.file_name().expect("checkpoint name").to_string_lossy()
        ));
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(&bytes)?;
        f.sync_all()?;
        drop(f);
        std::fs::rename(tmp, path)
    }

    /// Appends lines to `date`'s file (creating the directory). Errors are logged, not returned:
    /// a full disk must not stop the panel.
    pub fn append(&self, date: NaiveDate, lines: &[String]) {
        if lines.is_empty() {
            return;
        }
        // Replaceable per-row state must not spend the event journal's 2 MiB budget.
        let mut event_lines = Vec::new();
        let mut checkpoints: BTreeMap<String, Value> = BTreeMap::new();
        for line in lines {
            match serde_json::from_str::<Value>(line) {
                Ok(v) if v["kind"] == "breakout_track" => {
                    let id = v["id"].as_str().unwrap_or_default().to_owned();
                    if checkpoints
                        .get(&id)
                        .is_none_or(|old| checkpoint_order(&v) >= checkpoint_order(old))
                    {
                        checkpoints.insert(id, v);
                    }
                }
                _ => event_lines.push(line),
            }
        }
        for v in checkpoints.values() {
            if let Err(e) = self.write_checkpoint(date, v) {
                tracing::warn!(date = %date, error = %e, "rocket watch: exact tracking checkpoint failed");
            }
        }
        if event_lines.is_empty() {
            return;
        }
        let path = self.file(date);
        let res = (|| -> std::io::Result<()> {
            std::fs::create_dir_all(&self.dir)?;
            let len = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
            if len >= MAX_FILE_BYTES {
                return Err(std::io::Error::other("day file at its size cap"));
            }
            let mut f = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)?;
            let mut buf = String::new();
            for l in event_lines {
                buf.push_str(l);
                buf.push('\n');
            }
            f.write_all(buf.as_bytes())?;
            f.flush()
        })();
        if let Err(e) = res {
            tracing::warn!(path = %path.display(), error = %e, "rocket watch: forward log append failed");
        }
    }

    /// Deletes day files older than the retention window (0 = keep everything).
    pub fn prune(&self, today: NaiveDate) -> Vec<NaiveDate> {
        let mut gone = Vec::new();
        if self.retention_days == 0 {
            return gone;
        }
        let cutoff = today - chrono::Duration::days(i64::from(self.retention_days));
        let Ok(rd) = std::fs::read_dir(&self.dir) else {
            return gone;
        };
        for e in rd.flatten() {
            let p = e.path();
            if let Some(d) = date_of(&p)
                && d < cutoff
                && std::fs::remove_file(&p).is_ok()
            {
                gone.push(d);
            } else if checkpoint_date(&p).is_some_and(|d| d < cutoff) {
                let _ = std::fs::remove_file(&p);
            }
        }
        gone.sort();
        gone
    }

    fn read_lines(path: &Path) -> Vec<Value> {
        std::fs::read_to_string(path)
            .unwrap_or_default()
            .lines()
            .filter_map(|l| serde_json::from_str::<Value>(l).ok())
            .collect()
    }

    /// Reads today's alerts and the earlier days' record.
    pub fn load(&self, today: NaiveDate) -> Loaded {
        let mut out = Loaded::default();
        let Ok(rd) = std::fs::read_dir(&self.dir) else {
            return out;
        };
        let mut files: Vec<(NaiveDate, PathBuf)> = rd
            .flatten()
            .filter_map(|e| {
                let p = e.path();
                date_of(&p).map(|d| (d, p))
            })
            .filter(|(d, _)| *d <= today)
            .collect();
        files.sort();
        let earlier: Vec<&(NaiveDate, PathBuf)> =
            files.iter().filter(|(d, _)| *d < today).collect();
        let skip = earlier.len().saturating_sub(HISTORY_DAYS);
        for (date, path) in earlier.into_iter().skip(skip) {
            let lines = Self::read_lines(path);
            let mut s = DaySummary {
                date: *date,
                alerts: 0,
                hits10: 0,
                hits30: 0,
            };
            for v in &lines {
                match v.get("kind").and_then(Value::as_str) {
                    Some("alert") => s.alerts += 1,
                    Some("outcome") => {
                        if v.get("rocket10").and_then(Value::as_bool) == Some(true) {
                            s.hits10 += 1;
                        }
                        if v.get("rocket30").and_then(Value::as_bool) == Some(true) {
                            s.hits30 += 1;
                        }
                    }
                    Some("day") => {
                        if let (Some(st), Some(ver), Some(t)) = (
                            v.get("strategy").and_then(Value::as_str),
                            v.get("version").and_then(Value::as_str),
                            v.get("ideal_threshold").and_then(Value::as_f64),
                        ) {
                            out.thresholds
                                .push((*date, st.to_string(), ver.to_string(), t));
                        }
                    }
                    _ => {}
                }
            }
            out.days.push(s);
        }
        if let Some((_, path)) = files.iter().find(|(d, _)| *d == today) {
            let lines = Self::read_lines(path);
            let mut outcomes: BTreeMap<String, Value> = BTreeMap::new();
            let mut checkpoints: BTreeMap<String, Value> = BTreeMap::new();
            for v in &lines {
                let Some(id) = v.get("id").and_then(Value::as_str) else {
                    continue;
                };
                match v.get("kind").and_then(Value::as_str) {
                    Some("outcome") | Some("burst_outcome") => {
                        outcomes.insert(id.to_string(), v.clone());
                    }
                    Some("later") => {
                        out.later.insert(id.to_string(), v.clone());
                    }
                    Some("state") => {
                        let e = out.states.entry(id.to_string()).or_insert((Value::Null, 0));
                        *e = (v.clone(), e.1 + 1);
                    }
                    Some("also") => {
                        out.also.entry(id.to_string()).or_default().push(v.clone());
                    }
                    Some("crossing") => {
                        if let (Some(s), Some(t)) = (
                            v.get("symbol").and_then(Value::as_str),
                            v.get("text").and_then(Value::as_str),
                        ) {
                            out.crossings.insert(s.to_string(), t.to_string());
                        }
                    }
                    Some("breakout_track") if v["breakout_track"].is_object() => {
                        checkpoints.insert(id.to_owned(), v["breakout_track"].clone());
                    }
                    _ => {}
                }
            }
            for mut v in lines {
                let kind = v.get("kind").and_then(Value::as_str).map(str::to_string);
                if matches!(
                    kind.as_deref(),
                    Some("alert" | "burst" | "running" | "run" | "fast")
                ) {
                    let id = v["id"].as_str().unwrap_or_default();
                    if let Some(snapshot) = self
                        .read_checkpoint(today, id)
                        .map(|c| c["breakout_track"].clone())
                        .or_else(|| checkpoints.get(id).cloned())
                    {
                        v["breakout_track"] = snapshot;
                    }
                }
                if kind.as_deref() == Some("running") {
                    out.running.push(v);
                    continue;
                }
                if kind.as_deref() == Some("run") {
                    out.runs.push(v);
                    continue;
                }
                if kind.as_deref() == Some("fast") {
                    out.fasts.push(v);
                    continue;
                }
                if !matches!(kind.as_deref(), Some("alert") | Some("burst")) {
                    continue;
                }
                let id = v.get("id").and_then(Value::as_str).unwrap_or_default();
                let o = outcomes.remove(id);
                if kind.as_deref() == Some("burst") {
                    out.bursts.push((v, o));
                } else {
                    out.today.push((v, o));
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod checkpoint_tests {
    use super::*;
    use serde_json::json;

    fn checkpoint(day: NaiveDate, id: &str, time: i64) -> String {
        json!({"kind":"breakout_track","v":1,"date":day,"id":id,
            "breakout_track":{"v":1,"checkpoint_ns":time,"exact":{"checked_key":time,"admission_cutoff":time}}}).to_string()
    }

    #[test]
    fn replaceable_exact_state_survives_the_event_journal_cap_and_stale_writers() {
        let dir = tempfile::tempdir().unwrap();
        let log = RocketLog::new(dir.path().to_owned(), 30);
        let day = NaiveDate::from_ymd_opt(2026, 10, 26).unwrap();
        log.append(
            day,
            &[
                json!({"kind":"run","id":"2026-10-26-TDIC-run","price":1.83}).to_string(),
                checkpoint(day, "2026-10-26-TDIC-run", 1),
            ],
        );
        let path = log.file(day);
        let mut event_text = std::fs::read_to_string(&path).unwrap();
        assert!(!event_text.contains("breakout_track"));
        event_text.extend(std::iter::repeat_n(
            '\n',
            MAX_FILE_BYTES as usize - event_text.len(),
        ));
        std::fs::write(&path, event_text).unwrap();
        log.append(day, &[checkpoint(day, "2026-10-26-TDIC-run", 3)]);
        log.append(day, &[checkpoint(day, "2026-10-26-TDIC-run", 2)]);
        assert_eq!(std::fs::metadata(&path).unwrap().len(), MAX_FILE_BYTES);
        let loaded = log.load(day);
        assert_eq!(loaded.runs.len(), 1);
        assert_eq!(loaded.runs[0]["breakout_track"]["checkpoint_ns"], 3);
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 2);
    }

    #[test]
    fn all_entry_paths_receive_exact_state_and_legacy_entries_stay_readable() {
        let dir = tempfile::tempdir().unwrap();
        let log = RocketLog::new(dir.path().to_owned(), 30);
        let day = NaiveDate::from_ymd_opt(2026, 10, 26).unwrap();
        for (i, kind) in ["alert", "burst", "running", "run", "fast"]
            .iter()
            .enumerate()
        {
            let id = format!("row-{i}");
            log.append(
                day,
                &[
                    json!({"kind":kind,"id":id}).to_string(),
                    checkpoint(day, &id, 1),
                ],
            );
        }
        log.append(day, &[json!({"kind":"run","id":"legacy"}).to_string()]);
        let loaded = log.load(day);
        let rows = [
            &loaded.today[0].0,
            &loaded.bursts[0].0,
            &loaded.running[0],
            &loaded.runs[0],
            &loaded.fasts[0],
        ];
        assert!(
            rows.iter()
                .all(|v| v["breakout_track"]["checkpoint_ns"] == 1)
        );
        assert!(loaded.runs[1].get("breakout_track").is_none());
        std::fs::write(log.checkpoint_file(day, "row-3").unwrap(), "torn").unwrap();
        assert!(log.load(day).runs[0].get("breakout_track").is_none());
    }

    #[test]
    fn checkpoint_paths_are_bounded_and_expire_without_recursive_pruning() {
        let dir = tempfile::tempdir().unwrap();
        let log = RocketLog::new(dir.path().to_owned(), 1);
        let day = NaiveDate::from_ymd_opt(2026, 10, 26).unwrap();
        let old = day - chrono::Duration::days(3);
        for bad in ["../escape", "/escape", "..", "bad/name", ""] {
            assert!(log.checkpoint_file(day, bad).is_none());
        }
        log.append(old, &[checkpoint(old, "old-row", 1)]);
        let old_path = log.checkpoint_file(old, "old-row").unwrap();
        let pending = dir.path().join(format!(
            ".pending-{}",
            old_path.file_name().unwrap().to_string_lossy()
        ));
        std::fs::write(&pending, "interrupted").unwrap();
        log.append(day, &[checkpoint(day, "current-row", 1)]);
        log.prune(day);
        assert!(!old_path.exists());
        assert!(!pending.exists());
        assert!(log.checkpoint_file(day, "current-row").unwrap().exists());
    }
}

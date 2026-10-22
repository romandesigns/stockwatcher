//! The forward log: the live track record of every Rocket watch alert.
//!
//! `ARCHIVE_ROOT/rocket-watch/<trading date>.jsonl`, append-only, one JSON object per line, three
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
//! - `day` — `{kind, v, date, alerts, hits10, hits30, ideal_threshold, strategy, version}` at
//!   20:00 ET (or at the next day's first barrier when the process was down at 20:00):
//!   `ideal_threshold` is the score that would have given exactly the budget that day, the
//!   adaptive threshold's prior for the next days.
//!
//! Size: ~450 B per alert line, ~330 B per outcome, ~200 B per day line: about 8 KB a day at
//! the default 10 alerts. Bounded anyway: a file stops growing at [`MAX_FILE_BYTES`] (logged),
//! and the alert policy caps alerts per day. Files older than the archive's retention window
//! (`ARCHIVE_RETENTION_DAYS`) are deleted at startup and at every day roll, like the days they
//! were recorded beside.
//!
//! Writes are a few small appends a day, done on the quotes task after the core lock is released
//! (never on the ingest path).

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use chrono::NaiveDate;
use serde_json::Value;

/// A day's file stops growing here (a runaway would otherwise fill the archive volume).
pub const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;
/// Days of track record kept in memory and sent to the panel.
pub const HISTORY_DAYS: usize = 20;

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
    /// Up to [`HISTORY_DAYS`] earlier days, oldest first.
    pub days: Vec<DaySummary>,
    /// `(date, strategy, version, ideal_threshold)` from the earlier days' `day` lines.
    pub thresholds: Vec<(NaiveDate, String, String, f64)>,
}

#[derive(Debug)]
pub struct RocketLog {
    dir: PathBuf,
    retention_days: u32,
}

fn date_of(path: &Path) -> Option<NaiveDate> {
    let name = path.file_name()?.to_str()?;
    let stem = name.strip_suffix(".jsonl")?;
    NaiveDate::parse_from_str(stem, "%Y-%m-%d").ok()
}

impl RocketLog {
    pub fn new(dir: PathBuf, retention_days: u32) -> Self {
        RocketLog {
            dir,
            retention_days,
        }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn file(&self, date: NaiveDate) -> PathBuf {
        self.dir.join(format!("{date}.jsonl"))
    }

    /// Appends lines to `date`'s file (creating the directory). Errors are logged, not returned:
    /// a full disk must not stop the panel.
    pub fn append(&self, date: NaiveDate, lines: &[String]) {
        if lines.is_empty() {
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
            for l in lines {
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
            for v in &lines {
                if v.get("kind").and_then(Value::as_str) == Some("outcome")
                    && let Some(id) = v.get("id").and_then(Value::as_str)
                {
                    outcomes.insert(id.to_string(), v.clone());
                }
            }
            for v in lines {
                if v.get("kind").and_then(Value::as_str) == Some("alert") {
                    let id = v.get("id").and_then(Value::as_str).unwrap_or_default();
                    let o = outcomes.remove(id);
                    out.today.push((v, o));
                }
            }
        }
        out
    }
}

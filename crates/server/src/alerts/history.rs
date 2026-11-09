//! Activity: every fire and every hold-back, kept 30 days (Strategy board: "fired + held back,
//! why"). Server-side, not a synced namespace: it is written only by the server, grows with the
//! market rather than with the person's edits, and would soon pass a settings document's 256 KiB.
//!
//! On disk it is `DATA_DIR/alerts/history.jsonl`, one event per line, appended as events happen
//! (a crash loses at most the line being written; a torn last line is skipped on load). Pruning
//! (older than [`RETENTION_MS`], or past [`MAX_EVENTS`]) and the rare in-place update (a push
//! that failed after it was recorded as sent) rewrite the file with temp + rename.

use std::collections::VecDeque;
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub const RETENTION_MS: i64 = 30 * 24 * 60 * 60_000;
pub const MAX_EVENTS: usize = 5000;
/// Events in the `alerts.history` frame a connection gets (newest), so it stays small.
pub const SNAPSHOT_EVENTS: usize = 500;
const FILE: &str = "history.jsonl";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EventKind {
    Fired,
    Held,
}

fn is_false(b: &bool) -> bool {
    !b
}

/// One Activity row. Wire shape (camelCase) mirrored by packages/shared/src/alerts.ts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Event {
    pub id: String,
    pub alert_id: String,
    pub symbol: String,
    pub kind: EventKind,
    /// The print's (or status's) own time, epoch ms: a late print shows when it really traded.
    pub t: i64,
    /// When the server decided, epoch ms (wall).
    pub at: i64,
    /// "Crossed above 2.50 at 2.51", "Halted · LUDP", …
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub price: Option<f64>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub late: bool,
    /// A toast was shown in the app (the alert's "In app" chip).
    #[serde(default)]
    pub app: bool,
    /// `sent` (handed to the push service), `held` (see `reason`), `off` (the alert's phone chip
    /// is off), `failed` (the push service refused or was unreachable), `none` (a held event).
    pub phone: String,
    /// Machine reason of a hold-back: `stale`, `halted`, `gap` (the alert was not fired), or for
    /// the phone `no_device`, `quiet_hours`, `symbol_limit`, `budget`, `push_failed`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// The Activity row's second line, e.g. "Phone + in app · cooling until 10:46".
    pub note: String,
    /// A drawing alert's fire: `up` / `down` (which way the price came into the drawing) or
    /// `stop` (a position's stop). The clients pick the in-app tone by it. Absent otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tone: Option<String>,
}

pub struct History {
    dir: Option<PathBuf>,
    events: VecDeque<Event>,
}

fn write_atomic(dir: &Path, events: &VecDeque<Event>) -> io::Result<()> {
    let tmp = dir.join(format!("{FILE}.tmp"));
    {
        let mut f = std::fs::File::create(&tmp)?;
        let mut buf = Vec::new();
        for e in events {
            serde_json::to_writer(&mut buf, e).map_err(io::Error::other)?;
            buf.push(b'\n');
        }
        f.write_all(&buf)?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, dir.join(FILE))
}

impl History {
    pub fn in_memory() -> Self {
        History {
            dir: None,
            events: VecDeque::new(),
        }
    }

    /// Loads `dir/history.jsonl` (missing = empty; unreadable lines skipped) and prunes it.
    pub fn open(dir: &Path, now: i64) -> io::Result<Self> {
        std::fs::create_dir_all(dir)?;
        let mut events = VecDeque::new();
        match std::fs::File::open(dir.join(FILE)) {
            Ok(f) => {
                for line in io::BufReader::new(f).lines() {
                    let Ok(line) = line else { break };
                    if let Ok(e) = serde_json::from_str::<Event>(&line) {
                        events.push_back(e);
                    }
                }
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        let mut h = History {
            dir: Some(dir.to_owned()),
            events,
        };
        h.prune(now);
        Ok(h)
    }

    pub fn events(&self) -> impl DoubleEndedIterator<Item = &Event> {
        self.events.iter()
    }

    /// Drops what is past the retention or the cap; rewrites the file when anything went.
    pub fn prune(&mut self, now: i64) -> bool {
        let before = self.events.len();
        while self
            .events
            .front()
            .is_some_and(|e| e.at < now - RETENTION_MS)
            || self.events.len() > MAX_EVENTS
        {
            self.events.pop_front();
        }
        let pruned = self.events.len() != before;
        if pruned
            && let Some(dir) = &self.dir
            && let Err(e) = write_atomic(dir, &self.events)
        {
            tracing::warn!(error = %e, "alert history: rewrite after pruning failed");
        }
        pruned
    }

    pub fn push(&mut self, e: Event) {
        if let Some(dir) = &self.dir {
            let line = serde_json::to_string(&e).expect("events serialize");
            let res = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(dir.join(FILE))
                .and_then(|mut f| f.write_all(format!("{line}\n").as_bytes()));
            if let Err(err) = res {
                // Activity still shows it until the next restart; the alert itself was delivered.
                tracing::warn!(error = %err, "alert history: append failed");
            }
        }
        self.events.push_back(e);
        if self.events.len() > MAX_EVENTS {
            self.prune(i64::MIN / 2);
        }
    }

    /// Changes one event in place (a push that failed after being recorded as sent).
    pub fn update(&mut self, id: &str, f: impl FnOnce(&mut Event)) -> Option<Event> {
        let e = self.events.iter_mut().rev().find(|e| e.id == id)?;
        f(e);
        let out = e.clone();
        if let Some(dir) = &self.dir
            && let Err(err) = write_atomic(dir, &self.events)
        {
            tracing::warn!(error = %err, "alert history: rewrite after an update failed");
        }
        Some(out)
    }

    /// The newest [`SNAPSHOT_EVENTS`], oldest first.
    pub fn recent(&self) -> Vec<&Event> {
        let skip = self.events.len().saturating_sub(SNAPSHOT_EVENTS);
        self.events.iter().skip(skip).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(id: &str, at: i64) -> Event {
        Event {
            id: id.into(),
            alert_id: "a".into(),
            symbol: "XLAB".into(),
            kind: EventKind::Fired,
            t: at,
            at,
            text: "Crossed above 2.50 at 2.51".into(),
            price: Some(2.51),
            late: false,
            app: true,
            phone: "sent".into(),
            reason: None,
            note: String::new(),
            tone: None,
        }
    }

    #[test]
    fn persists_prunes_and_updates() {
        let dir = tempfile::tempdir().unwrap();
        let day = 24 * 60 * 60_000;
        {
            let mut h = History::open(dir.path(), 0).unwrap();
            h.push(ev("old", 0));
            h.push(ev("new", 29 * day));
        }
        // A torn last line (a crash mid-write) is skipped.
        std::fs::OpenOptions::new()
            .append(true)
            .open(dir.path().join(FILE))
            .unwrap()
            .write_all(b"{\"id\":\"tor")
            .unwrap();
        let h = History::open(dir.path(), 29 * day).unwrap();
        assert_eq!(h.events().count(), 2);
        let mut h = History::open(dir.path(), 31 * day).unwrap();
        assert_eq!(
            h.events().map(|e| e.id.as_str()).collect::<Vec<_>>(),
            vec!["new"],
            "30 days kept"
        );
        h.update("new", |e| e.phone = "failed".into()).unwrap();
        let h = History::open(dir.path(), 31 * day).unwrap();
        assert_eq!(h.events().next().unwrap().phone, "failed");
        assert_eq!(serde_json::to_value(ev("x", 1)).unwrap()["alertId"], "a");
    }
}

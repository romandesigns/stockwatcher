//! Whether an input-driven v2 detector (x-gap-1 / x-ctx-1, rw2-vwapr-1 / rw2-level-1, b-good-1 /
//! b-good-2) will send today, and why not: the line an operator reads to see that it is live.
//!
//! These detectors only send on a day watched whole from 04:00 ET whose inputs (the daily-bar
//! facts, the time-of-day baselines, the HI20 presence files, b-good's 826 dates of history) were
//! loaded before the first bar was decided. Until 2026-11-26 nothing reported that state except a
//! warning on the day it failed, so a detector that never sent looked the same as one with nothing
//! to say. Each now keeps a [`Readiness`], logs once when it changes ([`Tracker`]) and the
//! market publishes the set on `/api/health` (`detectors`).

use chrono::NaiveDate;
use serde_json::{Value, json};

/// The three states a day can be in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    /// The day's inputs are not loaded yet (they are asked for at 03:30 ET).
    Waiting,
    /// Inputs complete and the day watched from 04:00: fires are sent.
    Ready,
    /// The day decides (counted in its summary) but sends nothing; `why` says what is missing.
    Silent,
}

impl State {
    pub fn as_str(self) -> &'static str {
        match self {
            State::Waiting => "waiting",
            State::Ready => "ready",
            State::Silent => "silent",
        }
    }
}

/// One detector's state for one trading date.
#[derive(Clone, Debug, PartialEq)]
pub struct Readiness {
    pub date: Option<NaiveDate>,
    pub state: State,
    pub why: Option<String>,
}

impl Readiness {
    pub fn waiting(date: Option<NaiveDate>, why: impl Into<String>) -> Self {
        Readiness {
            date,
            state: State::Waiting,
            why: Some(why.into()),
        }
    }

    pub fn ready(date: Option<NaiveDate>) -> Self {
        Readiness {
            date,
            state: State::Ready,
            why: None,
        }
    }

    pub fn silent(date: Option<NaiveDate>, why: impl Into<String>) -> Self {
        Readiness {
            date,
            state: State::Silent,
            why: Some(why.into()),
        }
    }

    pub fn is_ready(&self) -> bool {
        self.state == State::Ready
    }

    /// `{"state", "date", "why"}` (the health endpoint's and the cost line's field).
    pub fn to_json(&self) -> Value {
        json!({
            "state": self.state.as_str(),
            "date": self.date.map(|d| d.to_string()),
            "why": self.why,
        })
    }
}

/// Logs a detector's [`Readiness`] once per change: at most a few lines a day (waiting after
/// 20:00, ready or silent once the 03:30 inputs land).
#[derive(Clone, Debug, Default)]
pub struct Tracker {
    last: Option<Readiness>,
}

impl Tracker {
    /// Logs `r` when it differs from the last one noted. Returns whether it logged.
    pub fn note(&mut self, name: &str, detectors: &str, r: &Readiness) -> bool {
        if self.last.as_ref() == Some(r) {
            return false;
        }
        let date = r.date.map(|d| d.to_string());
        match r.state {
            State::Ready => tracing::info!(
                detector = name,
                detectors,
                date,
                state = "ready",
                "{name}: ready: inputs loaded and the day watched from 04:00 ET, fires are sent"
            ),
            State::Waiting => tracing::info!(
                detector = name,
                detectors,
                date,
                state = "waiting",
                why = r.why,
                "{name}: waiting for the day's inputs"
            ),
            State::Silent => tracing::warn!(
                detector = name,
                detectors,
                date,
                state = "silent",
                why = r.why,
                "{name}: deciding but sending nothing today"
            ),
        }
        self.last = Some(r.clone());
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tracker_logs_each_change_once() {
        let d = NaiveDate::from_ymd_opt(2026, 11, 30);
        let mut t = Tracker::default();
        assert!(t.note("x", "a", &Readiness::waiting(d, "not loaded")));
        assert!(!t.note("x", "a", &Readiness::waiting(d, "not loaded")));
        assert!(t.note("x", "a", &Readiness::ready(d)));
        assert!(!t.note("x", "a", &Readiness::ready(d)));
        assert!(t.note(
            "x",
            "a",
            &Readiness::ready(NaiveDate::from_ymd_opt(2026, 12, 1))
        ));
        let v = Readiness::silent(d, "stale").to_json();
        assert_eq!(v["state"], "silent");
        assert_eq!(v["date"], "2026-11-30");
        assert_eq!(v["why"], "stale");
    }
}

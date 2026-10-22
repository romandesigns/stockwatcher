//! Runners phone pushes (owner, 2026-10-22): the Runners board's new flags and breakouts reach
//! the phone while the app is closed. He missed CJMB (+95 % in ~30 minutes at 08:05 ET) because
//! the only Runners signal was on screen, or a local banner while the dashboard was open.
//!
//! The pipeline is the price alerts' own ([`super::push`]): same registered phones, same Expo
//! sender, batching, retries and token clean-up. What is Runners-specific lives here:
//!
//! - **What:** a [`Notice`] from the LIVE market's board (`crate::rocket`, "Phone pushes"): a new
//!   flag, or a flagged row turning `breaking_out`. The market offers it to the alerts task with
//!   `try_send` after the core lock is released, so ingest and the board never wait on a push. A
//!   replay market never offers one.
//! - **Who:** each registered phone opts in per kind, on the device record
//!   (`push-devices.json`, sent in `push.register`): new flags ON by default, breakouts OFF.
//!   Per device rather than synced because it is about which phone makes noise, exactly like the
//!   token itself.
//! - **Limits**, checked in this order (the first that applies holds it back; logged, never
//!   sent later):
//!   1. no phone opted in for this kind → `no_device`;
//!   2. the price alerts' quiet hours (`alerts.prefs`, ET wall time) → `quiet_hours`;
//!   3. the notice is over [`MAX_AGE_MS`] old (a catch-up after a stall) → `stale`;
//!   4. the same stock got a Runners push less than [`SYMBOL_GAP_MS`] ago, whatever the kind →
//!      `symbol_limit`;
//!   5. [`DAILY_CAP`] Runners pushes already went out this ET day → `cap`.
//!
//!   The count and each stock's last push are kept in `runners-push.json` beside the devices, so
//!   a redeploy at 09:00 does not hand out a fresh cap. They are separate from the price alerts'
//!   phone budget: a busy Runners morning must not silence a price alert the owner set himself.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::model::Prefs;
use crate::rocket::{Notice, NoticeKind};

/// At most one Runners push per stock per 10 minutes (new flag and breakout alike).
pub const SYMBOL_GAP_MS: i64 = 10 * 60_000;
/// At most this many Runners pushes per ET calendar day.
pub const DAILY_CAP: u32 = 30;
/// A notice older than this when it reaches the task is not pushed: news about a move minutes
/// ago is noise on a lock screen.
pub const MAX_AGE_MS: i64 = 3 * 60_000;
const FILE: &str = "runners-push.json";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    Send,
    Held(Hold),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hold {
    NoDevice,
    QuietHours,
    Stale,
    SymbolLimit,
    Cap,
}

impl Hold {
    pub fn code(self) -> &'static str {
        match self {
            Hold::NoDevice => "no_device",
            Hold::QuietHours => "quiet_hours",
            Hold::Stale => "stale",
            Hold::SymbolLimit => "symbol_limit",
            Hold::Cap => "cap",
        }
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Saved {
    /// The ET trading day the count belongs to, `YYYY-MM-DD`.
    #[serde(default)]
    day: String,
    #[serde(default)]
    sent: u32,
    #[serde(default)]
    last_by_symbol: HashMap<String, i64>,
}

/// The Runners side's memory: today's count and each stock's last push, persisted.
#[derive(Debug, Default)]
pub struct RunnersDelivery {
    dir: Option<PathBuf>,
    s: Saved,
}

impl RunnersDelivery {
    pub fn in_memory() -> Self {
        Self::default()
    }

    pub fn open(dir: &Path) -> Self {
        let s = std::fs::read(dir.join(FILE))
            .ok()
            .and_then(|b| serde_json::from_slice::<Saved>(&b).ok())
            .unwrap_or_default();
        RunnersDelivery {
            dir: Some(dir.to_owned()),
            s,
        }
    }

    fn save(&self) {
        let Some(dir) = &self.dir else { return };
        let res = (|| -> std::io::Result<()> {
            std::fs::create_dir_all(dir)?;
            let tmp = dir.join(format!("{FILE}.tmp"));
            std::fs::write(
                &tmp,
                serde_json::to_vec(&self.s).map_err(std::io::Error::other)?,
            )?;
            std::fs::rename(&tmp, dir.join(FILE))
        })();
        if let Err(e) = res {
            tracing::warn!(error = %e, "runners push: save failed");
        }
    }

    pub fn sent_today(&self, today: NaiveDate) -> u32 {
        if self.s.day == today.to_string() {
            self.s.sent
        } else {
            0
        }
    }

    /// The decision for one notice (module docs, "Limits"). `devices` = phones opted in for its
    /// kind; `at` = when it happened, `now` = now (epoch ms); `today` / `et_minute` = the ET day
    /// and minute of the day at `now`.
    #[allow(clippy::too_many_arguments)]
    pub fn decide(
        &mut self,
        symbol: &str,
        devices: usize,
        prefs: &Prefs,
        at: i64,
        now: i64,
        today: NaiveDate,
        et_minute: u32,
    ) -> Decision {
        let day = today.to_string();
        if self.s.day != day {
            self.s.day = day;
            self.s.sent = 0;
            // Yesterday's stocks cannot hold anything back today (10 minutes < a night).
            self.s.last_by_symbol.clear();
        }
        if devices == 0 {
            return Decision::Held(Hold::NoDevice);
        }
        if prefs.quiet.is_some_and(|q| q.contains(et_minute)) {
            return Decision::Held(Hold::QuietHours);
        }
        if now - at > MAX_AGE_MS {
            return Decision::Held(Hold::Stale);
        }
        if self
            .s
            .last_by_symbol
            .get(symbol)
            .is_some_and(|t| now - t < SYMBOL_GAP_MS)
        {
            return Decision::Held(Hold::SymbolLimit);
        }
        if self.s.sent >= DAILY_CAP {
            return Decision::Held(Hold::Cap);
        }
        self.s.sent += 1;
        self.s.last_by_symbol.insert(symbol.to_owned(), now);
        self.save();
        Decision::Send
    }
}

/// "$2.45", "$0.4321".
fn dollars(p: f64) -> String {
    if p >= 1.0 {
        format!("${p:.2}")
    } else {
        format!("${p:.4}")
    }
}

/// The `kind` in the push's data (apps/mobile src/notifications/route.ts reads it).
pub fn data_kind(kind: NoticeKind) -> &'static str {
    match kind {
        NoticeKind::New => "runners.new",
        NoticeKind::Breakout => "runners.breakout",
    }
}

/// Title, body and data of the push for one notice. The data's `symbol` is what the phone opens
/// the chart for on a tap.
pub fn message(n: &Notice) -> (String, String, Value) {
    let title = match n.kind {
        NoticeKind::New => format!("Runners: {} {}", n.symbol, dollars(n.price)),
        NoticeKind::Breakout => format!("{} breaking out {}", n.symbol, dollars(n.price)),
    };
    let mut parts: Vec<String> = Vec::new();
    if n.kind == NoticeKind::Breakout
        && let Some(p) = n.from_flag_pct
    {
        parts.push(format!("{p:+.1}% from flag"));
    }
    parts.extend(n.reasons.iter().take(2).cloned());
    if parts.is_empty() {
        parts.push("Flagged (experimental)".into());
    }
    let data = json!({
        "symbol": n.symbol,
        "kind": data_kind(n.kind),
        "alert_id": n.alert_id,
    });
    (title, parts.join(" · "), data)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::alerts::model::QuietHours;

    fn day(d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 10, d).unwrap()
    }

    const MIN: i64 = 60_000;

    #[test]
    fn cooldown_cap_quiet_hours_and_staleness() {
        let mut d = RunnersDelivery::in_memory();
        let prefs = Prefs::default();
        let go = |d: &mut RunnersDelivery, s: &str, now: i64| {
            d.decide(s, 1, &prefs, now, now, day(22), 8 * 60)
        };
        assert_eq!(
            d.decide("CJMB", 0, &prefs, 0, 0, day(22), 480),
            Decision::Held(Hold::NoDevice)
        );
        assert_eq!(go(&mut d, "CJMB", 0), Decision::Send);
        // The same stock within 10 minutes (a breakout right after its flag): held.
        assert_eq!(
            go(&mut d, "CJMB", 9 * MIN),
            Decision::Held(Hold::SymbolLimit)
        );
        assert_eq!(go(&mut d, "CJMB", 10 * MIN), Decision::Send);
        // Old news is not pushed.
        assert_eq!(
            d.decide("OLD", 1, &prefs, 0, 4 * MIN, day(22), 480),
            Decision::Held(Hold::Stale)
        );
        // The daily cap of 30, then a new ET day.
        for i in 0..(DAILY_CAP - 2) {
            assert_eq!(go(&mut d, &format!("S{i}"), 11 * MIN), Decision::Send);
        }
        assert_eq!(d.sent_today(day(22)), DAILY_CAP);
        assert_eq!(go(&mut d, "LATE", 12 * MIN), Decision::Held(Hold::Cap));
        assert_eq!(
            d.decide("LATE", 1, &prefs, 13 * MIN, 13 * MIN, day(23), 480),
            Decision::Send
        );
        assert_eq!(d.sent_today(day(22)), 0);
        // The price alerts' quiet hours hold Runners too.
        let quiet = Prefs {
            quiet: Some(QuietHours {
                start_min: 22 * 60,
                end_min: 7 * 60,
            }),
            ..Prefs::default()
        };
        assert_eq!(
            d.decide("Q", 1, &quiet, 20 * MIN, 20 * MIN, day(23), 6 * 60),
            Decision::Held(Hold::QuietHours)
        );
    }

    #[test]
    fn the_count_survives_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let prefs = Prefs::default();
        let mut d = RunnersDelivery::open(dir.path());
        assert_eq!(d.decide("A", 1, &prefs, 0, 0, day(22), 480), Decision::Send);
        let mut back = RunnersDelivery::open(dir.path());
        assert_eq!(back.sent_today(day(22)), 1);
        assert_eq!(
            back.decide("A", 1, &prefs, MIN, MIN, day(22), 481),
            Decision::Held(Hold::SymbolLimit),
            "a redeploy does not forget the cooldown"
        );
    }

    #[test]
    fn messages() {
        let mut n = Notice {
            kind: NoticeKind::New,
            alert_id: "2026-10-22-CJMB-260".into(),
            symbol: "CJMB".into(),
            at_ns: 0,
            price: 2.4512,
            from_flag_pct: Some(0.0),
            reasons: vec!["volume 9x pace".into(), "new high".into(), "third".into()],
        };
        let (t, b, data) = message(&n);
        assert_eq!(t, "Runners: CJMB $2.45");
        assert_eq!(b, "volume 9x pace · new high");
        assert_eq!(
            data,
            json!({"symbol": "CJMB", "kind": "runners.new", "alert_id": "2026-10-22-CJMB-260"})
        );
        n.kind = NoticeKind::Breakout;
        n.price = 0.3;
        n.from_flag_pct = Some(12.34);
        n.reasons.clear();
        let (t, b, data) = message(&n);
        assert_eq!(t, "CJMB breaking out $0.3000");
        assert_eq!(b, "+12.3% from flag");
        assert_eq!(data["kind"], "runners.breakout");
        n.kind = NoticeKind::New;
        assert_eq!(message(&n).1, "Flagged (experimental)");
    }
}

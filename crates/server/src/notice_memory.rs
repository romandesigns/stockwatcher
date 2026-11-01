//! What a previous process of the live run already announced today, read back from the logs it
//! left on the archive volume, so a restart neither re-notifies nor reissues an id.
//!
//! # Why
//!
//! Replaying INO 2026-10-30: a 16:38 ET deploy restarted the server mid-session. Everything the
//! pullback notices remember lived in memory (`crate::cue_notices::CueNotices`): the stocks whose
//! one setting-up notice of the day already went out, the episodes announced, and the detector's
//! per-symbol episode counters (`stock_context::detect`). The live run id is `live` on every boot,
//! so the new process numbered its first INO micro-pullback `live:2026-10-30:INO:micro_pullback:1`
//! again: its notices got the event id `runners-pullback-live:2026-10-30:INO:micro_pullback:1`,
//! which the delivery ledger had already sent and refused as a repeat. A stock could also have
//! had a second "first of the day" setup announced.
//!
//! # What is read
//!
//! Nothing new is persisted: the logs already hold it.
//!
//! - **The notice ledger** (`ARCHIVE_ROOT/notices/<ET date>.jsonl`, `crate::alerts::ledger`):
//!   every Ross, Signals and pullback notice decided, sent or held. Its `decision` lines give the
//!   `(kind, episode)` pairs announced ([`NoticeMemory::seen`]) and, for each setting-up notice,
//!   the stock and trading day ([`NoticeMemory::setup_days`]) — exactly what `CueNotices` records
//!   when it emits, whatever the delivery decided.
//! - **Episode numbers** ([`NoticeMemory::floors`]): every episode id of this run and trading date
//!   found in the notice ledger, the cue-transition log (`ARCHIVE_ROOT/transitions`) and the
//!   Detection Panel's journals of the last day and a half (`ARCHIVE_ROOT/detection`, one file per
//!   boot). The engine numbers its episodes past the highest per symbol and family
//!   (`stock_context::Engine::set_episode_floors`). The scan matches the id's text, not a record
//!   shape, so it reads all three formats alike.
//!
//! The first-flag times of the 30-minute window need nothing here: they come from the Runners
//! rows, which the forward log restores (`crate::rocket::RocketBoard::take_watch`).
//!
//! # Limits
//!
//! The logs are write-behind (`crate::oplog`): a line still queued when the previous process died
//! is not read back. The delivery ledger's own persisted ids remain the backstop against a repeat
//! push. A missing or unreadable file contributes nothing; the read never fails the startup.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use chrono::NaiveDate;
use market_events::UnixNanos;
use market_events::session::trading_date;
use serde_json::Value;
use stock_context::Family;

use crate::rocket::NoticeKind;

/// The notice kinds `CueNotices` decides (its `seen` keys).
const CUE_KINDS: [NoticeKind; 6] = [
    NoticeKind::RossCrossing,
    NoticeKind::SignalsCrossing,
    NoticeKind::RossTrigger,
    NoticeKind::SignalsTrigger,
    NoticeKind::Pullback,
    NoticeKind::PullbackCrossing,
];

/// How far back a Detection Panel journal may have been written to and still hold today's ids
/// (the files are per boot, not per day).
const DETECTION_JOURNAL_AGE: std::time::Duration = std::time::Duration::from_secs(36 * 3600);

/// What a previous process announced and numbered on [`NoticeMemory::date`] (module docs).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NoticeMemory {
    /// The trading date the floors are for.
    pub date: Option<NaiveDate>,
    /// `(symbol, family, highest episode number used)`.
    pub floors: Vec<(String, Family, u32)>,
    /// `(kind, episode id)` already announced.
    pub seen: Vec<(NoticeKind, String)>,
    /// Stocks whose one setting-up notice of the day went out.
    pub setup_days: Vec<(String, NaiveDate)>,
    /// Episodes announced as setting up (their crossing is the pullback follow-up).
    pub setup_episodes: Vec<String>,
}

impl NoticeMemory {
    pub fn is_empty(&self) -> bool {
        self.floors.is_empty() && self.seen.is_empty() && self.setup_days.is_empty()
    }
}

/// Collects the highest number per `(symbol, family)` of every `<run_id>:<date>:<symbol>:<family>:<n>`
/// in `text` (module docs, "Episode numbers").
pub fn scan_episode_ids(
    text: &str,
    run_id: &str,
    date: NaiveDate,
    floors: &mut HashMap<(String, Family), u32>,
) {
    let prefix = format!("{run_id}:{date}:");
    for (i, _) in text.match_indices(&prefix) {
        // A whole id only: `live:` inside another word (`xlive:`) is not this run's.
        if text[..i]
            .chars()
            .next_back()
            .is_some_and(|c| c.is_ascii_alphanumeric() || c == ':')
        {
            continue;
        }
        let rest = &text[i + prefix.len()..];
        let mut parts = rest.splitn(3, ':');
        let (Some(sym), Some(fam), Some(tail)) = (parts.next(), parts.next(), parts.next()) else {
            continue;
        };
        let symbol_ok = !sym.is_empty()
            && sym.len() <= 16
            && sym
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '/' | '_'));
        let Some(family) = Family::parse(fam).filter(|_| symbol_ok) else {
            continue;
        };
        let digits = tail.bytes().take_while(u8::is_ascii_digit).count();
        let Ok(n) = tail[..digits].parse::<u32>() else {
            continue;
        };
        let e = floors.entry((sym.to_string(), family)).or_default();
        *e = (*e).max(n);
    }
}

/// Reads the notice ledger's `decision` lines into `m` (module docs, "What is read").
pub fn scan_notices(text: &str, m: &mut NoticeMemory) {
    let mut seen: HashSet<(NoticeKind, String)> = m.seen.iter().cloned().collect();
    let mut days: HashSet<(String, NaiveDate)> = m.setup_days.iter().cloned().collect();
    for line in text.lines() {
        // A torn last line (a crash mid-write) is skipped like any malformed one.
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if v.get("kind").and_then(Value::as_str) != Some("decision") {
            continue;
        }
        let Some(id) = v.get("event_id").and_then(Value::as_str) else {
            continue;
        };
        // `runners-<kind>-<episode id>` (`crate::rocket::Notice::event_id`); the trailing `-`
        // keeps `pullback` from matching `pullback_crossing`.
        let Some((kind, episode)) = CUE_KINDS.iter().find_map(|k| {
            id.strip_prefix(&format!("runners-{}-", k.as_str()))
                .map(|ep| (*k, ep))
        }) else {
            continue;
        };
        if !seen.insert((kind, episode.to_string())) {
            continue;
        }
        m.seen.push((kind, episode.to_string()));
        if kind == NoticeKind::Pullback {
            m.setup_episodes.push(episode.to_string());
            let symbol = v.get("symbol").and_then(Value::as_str);
            let at = v.get("at_ns").and_then(Value::as_i64);
            if let (Some(symbol), Some(at)) = (symbol, at) {
                let key = (symbol.to_string(), trading_date(UnixNanos(at)));
                if days.insert(key.clone()) {
                    m.setup_days.push(key);
                }
            }
        }
    }
}

/// Today's memory of run `run_id` from the logs under `archive_root`, as of `now_ns` (module
/// docs). Reads a few files once; call it off the core lock.
pub fn read(archive_root: &Path, run_id: &str, now_ns: i64) -> NoticeMemory {
    let date = trading_date(UnixNanos(now_ns));
    // The files are named by ET calendar date; after 20:00 ET the trading date is the next one,
    // so both names are read.
    let calendar = crate::oplog::et_date_ms(now_ns.div_euclid(1_000_000));
    let mut names = vec![calendar];
    if date != calendar {
        names.push(date);
    }
    let day_files = |dir: &str| {
        names
            .iter()
            .map(|d| {
                archive_root
                    .join(dir)
                    .join(format!("{}.jsonl", d.format("%Y-%m-%d")))
            })
            .collect::<Vec<_>>()
    };
    let mut m = NoticeMemory {
        date: Some(date),
        ..NoticeMemory::default()
    };
    let mut floors = HashMap::new();
    for p in day_files(crate::oplog::NOTICES_DIR) {
        if let Ok(text) = std::fs::read_to_string(&p) {
            scan_notices(&text, &mut m);
            scan_episode_ids(&text, run_id, date, &mut floors);
        }
    }
    for p in day_files(crate::oplog::TRANSITIONS_DIR) {
        if let Ok(text) = std::fs::read_to_string(&p) {
            scan_episode_ids(&text, run_id, date, &mut floors);
        }
    }
    let journals = std::fs::read_dir(archive_root.join("detection"))
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter(|e| {
            e.file_name()
                .to_str()
                .is_some_and(|n| n.starts_with("detection-live-") && n.ends_with(".jsonl"))
        })
        .filter(|e| {
            e.metadata()
                .and_then(|md| md.modified())
                .ok()
                .and_then(|t| t.elapsed().ok())
                .is_some_and(|age| age <= DETECTION_JOURNAL_AGE)
        });
    for e in journals {
        if let Ok(text) = std::fs::read_to_string(e.path()) {
            scan_episode_ids(&text, run_id, date, &mut floors);
        }
    }
    let mut floors: Vec<_> = floors.into_iter().map(|((s, f), n)| (s, f, n)).collect();
    floors.sort_by(|a, b| (&a.0, a.1.as_str()).cmp(&(&b.0, b.1.as_str())));
    m.floors = floors;
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    /// INO 2026-10-30, 13:10 ET: the moment of a setting-up notice.
    const AT: i64 = 1_793_380_200_000_000_000;

    fn decision(event_id: &str, symbol: &str, at_ns: i64) -> String {
        serde_json::json!({
            "kind": "decision", "v": 1, "decided_at": 1, "event_id": event_id,
            "notice": "runners.pullback", "symbol": symbol, "at_ns": at_ns, "path": "phone",
            "decision": "held", "held": "quiet_hours", "devices": 1, "title": "t", "body": "b",
        })
        .to_string()
    }

    /// Ids are found in any record shape; the highest number per symbol and family wins; other
    /// dates, other runs and look-alikes are ignored.
    #[test]
    fn episode_numbers_from_any_log() {
        let text = [
            r#"{"kind":"transition","transition_id":"live:2026-10-30:INO:micro_pullback:2:t3","episode_id":"live:2026-10-30:INO:micro_pullback:2"}"#,
            r#"{"event_id":"runners-pullback-live:2026-10-30:INO:micro_pullback:3"}"#,
            r#"{"episode_id":"live:2026-10-30:BRK.B:ignition:7"}"#,
            r#"{"episode_id":"live:2026-10-29:INO:micro_pullback:9"}"#,
            r#"{"episode_id":"replay:2026-10-30:INO:micro_pullback:8"}"#,
            r#"{"episode_id":"xlive:2026-10-30:INO:micro_pullback:8"}"#,
            r#"{"episode_id":"live:2026-10-30:INO:not_a_family:8"}"#,
            "live:2026-10-30:INO:micro_pullback:",
        ]
        .join("\n");
        let mut f = HashMap::new();
        scan_episode_ids(&text, "live", d("2026-10-30"), &mut f);
        assert_eq!(f.len(), 2, "{f:?}");
        assert_eq!(f[&("INO".to_string(), Family::MicroPullback)], 3);
        assert_eq!(f[&("BRK.B".to_string(), Family::Ignition)], 7);
    }

    /// The ledger's decisions: every cue kind's episode is `seen` once (both paths log the same
    /// event), a setting-up notice also counts as the stock's one for its trading day, other
    /// notices and torn lines are skipped.
    #[test]
    fn the_ledger_gives_what_was_announced() {
        let ep = "live:2026-10-30:INO:micro_pullback:1";
        let text = [
            decision(&format!("runners-pullback-{ep}"), "INO", AT),
            decision(&format!("runners-pullback-{ep}"), "INO", AT),
            decision(&format!("runners-pullback_crossing-{ep}"), "INO", AT + 1),
            decision(
                "runners-ross_crossing-live:2026-10-30:ROSS:micro_pullback:4",
                "ROSS",
                AT,
            ),
            decision("runners-new-INO-1", "INO", AT),
            r#"{"kind":"pushed","event_id":"runners-pullback-live:2026-10-30:X:micro_pullback:1"}"#
                .into(),
            r#"{"kind":"decision","event_id":"runners-pullback-live:2026"#.into(),
        ]
        .join("\n");
        let mut m = NoticeMemory::default();
        scan_notices(&text, &mut m);
        assert_eq!(
            m.seen,
            vec![
                (NoticeKind::Pullback, ep.to_string()),
                (NoticeKind::PullbackCrossing, ep.to_string()),
                (
                    NoticeKind::RossCrossing,
                    "live:2026-10-30:ROSS:micro_pullback:4".to_string()
                ),
            ]
        );
        assert_eq!(m.setup_days, vec![("INO".to_string(), d("2026-10-30"))]);
        assert_eq!(m.setup_episodes, vec![ep.to_string()]);
    }

    /// The whole read over a scratch archive root, as a restart does it: the ledger and the
    /// transition log of today, and a missing Detection journal directory is not an error.
    #[test]
    fn reads_todays_logs_under_the_archive_root() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("notices")).unwrap();
        std::fs::create_dir_all(root.join("transitions")).unwrap();
        let ep = "live:2026-10-30:INO:micro_pullback:2";
        std::fs::write(
            root.join("notices/2026-10-30.jsonl"),
            decision(&format!("runners-pullback-{ep}"), "INO", AT),
        )
        .unwrap();
        std::fs::write(
            root.join("transitions/2026-10-30.jsonl"),
            r#"{"episode_id":"live:2026-10-30:INO:micro_pullback:5"}"#,
        )
        .unwrap();
        // Yesterday's file is not read.
        std::fs::write(
            root.join("transitions/2026-10-29.jsonl"),
            r#"{"episode_id":"live:2026-10-30:INO:micro_pullback:9"}"#,
        )
        .unwrap();
        let m = read(root, "live", AT + 3_600_000_000_000);
        assert_eq!(m.date, Some(d("2026-10-30")));
        assert_eq!(
            m.floors,
            vec![("INO".to_string(), Family::MicroPullback, 5)]
        );
        assert_eq!(m.setup_days, vec![("INO".to_string(), d("2026-10-30"))]);
        assert!(read(&root.join("absent"), "live", AT).is_empty());
    }

    /// A micro-pullback impulse peaking at slot `k0 + 9` and retreating two bars (it arms at
    /// `cutoff(k0 + 11)`).
    fn micro_at(s: &mut stock_context::detect::fixture::Script, sym: &str, k0: i64) {
        for k in 0..6 {
            s.bar(sym, k0 + k, [300, 300, 300, 300], 100);
        }
        let rise = [
            [300, 302, 300, 302],
            [302, 310, 302, 310],
            [310, 325, 310, 325],
            [325, 340, 325, 338],
            [338, 338, 333, 335],
            [335, 336, 332, 334],
        ];
        for (k, b) in rise.into_iter().enumerate() {
            s.bar(sym, k0 + 6 + k as i64, b, 200);
        }
    }

    /// A restart mid-day, end to end: process 1 announces INO's first setup and logs it; process
    /// 2, a fresh engine of the same `live` run, reads the logs back. Its next INO episode is
    /// numbered past process 1's (no id the delivery ledger has sent), its arming is not a second
    /// "first of the day", and another stock's first setup still notifies.
    #[test]
    fn a_restart_neither_renotifies_nor_reuses_an_episode_id() {
        use stock_context::detect::fixture::{self, Script, cutoff};
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("notices")).unwrap();
        std::fs::create_dir_all(root.join("transitions")).unwrap();
        let file = format!("{}.jsonl", fixture::day());
        let member = |_: &str| false;
        let since = |_: &str| Some(fixture::at(9, 30, 0));
        let price = |_: &str, _: i64| None;

        // Process 1.
        let mut s = Script::new();
        micro_at(&mut s, "INO", 0);
        s.advance(cutoff(11));
        let mut e1 = fixture::engine("live");
        let ts = fixture::drive(&mut e1, &s.steps);
        let mut c1 = crate::cue_notices::CueNotices::default();
        c1.transitions_with(&ts, false, member, since, price);
        let sent = c1.take();
        assert_eq!(sent.len(), 1);
        let first_id = format!("live:{}:INO:micro_pullback:1", fixture::day());
        assert_eq!(sent[0].alert_id, first_id);
        // What its ledger and transition log hold (the ledger's decision line, abridged).
        let ledger: Vec<String> = sent
            .iter()
            .map(|n| decision(&n.event_id(), &n.symbol, n.at_ns))
            .collect();
        std::fs::write(root.join("notices").join(&file), ledger.join("\n")).unwrap();
        let log: Vec<String> = ts.iter().map(crate::market::transition_line).collect();
        std::fs::write(root.join("transitions").join(&file), log.join("\n")).unwrap();

        // Process 2: restarted at 09:40, a second INO setup and a first OTHR one after it.
        let m = read(root, "live", fixture::at(9, 40, 0));
        let mut e2 = fixture::engine("live");
        e2.set_episode_floors(m.date.unwrap(), m.floors.iter().cloned());
        let mut c2 = crate::cue_notices::CueNotices::default();
        c2.restore(&m);
        let mut s = Script::new();
        micro_at(&mut s, "INO", 40);
        micro_at(&mut s, "OTHR", 60);
        s.advance(cutoff(71));
        let ts = fixture::drive(&mut e2, &s.steps);
        let armed: Vec<_> = ts
            .iter()
            .filter(|t| t.to == stock_context::State::Armed)
            .collect();
        assert_eq!(armed.len(), 2, "{armed:?}");
        let ino = armed.iter().find(|t| t.symbol == "INO").unwrap();
        assert_eq!(
            ino.episode_id,
            format!("live:{}:INO:micro_pullback:2", fixture::day()),
            "numbered past process 1's episode"
        );
        assert!(!ino.warmup);
        c2.transitions_with(&ts, false, member, since, price);
        let n = c2.take();
        assert_eq!(n.len(), 1, "{n:?}");
        assert_eq!(n[0].symbol, "OTHR");
        assert!(n.iter().all(|x| x.event_id() != sent[0].event_id()));
    }
}

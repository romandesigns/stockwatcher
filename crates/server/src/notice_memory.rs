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
//! - **Inherited setups** ([`NoticeMemory::inherited`]): for every announced micro-pullback, its
//!   symbol and H0 bar end, from the transitions in the same two logs. The restart restores the
//!   day's bars, so a setup that was armed or near its trigger before it is opened again under a
//!   new number (the floors above); `CueNotices` recognizes it by its H0 bar and keys its memory
//!   by the old id, so its "At trigger" or crossing is not announced a second time under a fresh
//!   event id the delivery ledger would not recognize.
//!
//! The transition log and the journals are streamed line by line, never read whole: a Detection
//! journal is one file per boot, and a process that ran for days leaves one holding all of their
//! transitions (the ledger is one calendar day of notices and is read whole).
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
use std::io::BufRead;
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
    /// `(symbol, H0 bar end, episode id)` of every announced micro-pullback whose transitions
    /// were found (module docs, "Inherited setups").
    pub inherited: Vec<(String, i64, String)>,
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

/// Calls `f` with each line of `path` (lossy UTF-8), streaming; a missing file is no lines.
fn each_line(path: &Path, mut f: impl FnMut(&str)) {
    let Ok(file) = std::fs::File::open(path) else {
        return;
    };
    let mut r = std::io::BufReader::with_capacity(1 << 16, file);
    let mut buf = Vec::new();
    loop {
        buf.clear();
        match r.read_until(b'\n', &mut buf) {
            Ok(0) | Err(_) => return,
            Ok(_) => f(&String::from_utf8_lossy(&buf)),
        }
    }
}

/// The `(symbol, H0 bar end, episode id)` of the micro-pullback transition(s) in one log line, of
/// any record shape (a transition-log line is the transition itself; a Detection journal's
/// nests it under `emission.transition`), kept when `announced` holds the id.
pub fn scan_inherited(
    line: &str,
    announced: &HashSet<&str>,
    out: &mut HashMap<String, (String, i64)>,
) {
    // Cheap filters first: almost every journal line is another family or has no anchors.
    if announced.is_empty() || !line.contains("h0_bar_end") || !line.contains("micro_pullback") {
        return;
    }
    let Ok(v) = serde_json::from_str::<Value>(line) else {
        return;
    };
    let mut stack = vec![&v];
    while let Some(v) = stack.pop() {
        match v {
            Value::Object(o) => {
                let id = o.get("episode_id").and_then(Value::as_str);
                let sym = o.get("symbol").and_then(Value::as_str);
                let h0 = o
                    .get("anchors")
                    .and_then(|a| a.get("h0_bar_end"))
                    .and_then(Value::as_i64);
                if let (Some(id), Some(sym), Some(h0)) = (id, sym, h0)
                    && announced.contains(id)
                {
                    out.entry(id.to_string())
                        .or_insert_with(|| (sym.to_string(), h0));
                }
                stack.extend(o.values());
            }
            Value::Array(a) => stack.extend(a.iter()),
            _ => {}
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
    let prefix = format!("{run_id}:{date}:");
    for p in day_files(crate::oplog::NOTICES_DIR) {
        // One calendar day of notices (a few thousand lines at most): read whole, so
        // `scan_notices` builds its sets once rather than per line.
        if let Ok(text) = std::fs::read_to_string(&p) {
            scan_notices(&text, &mut m);
            scan_episode_ids(&text, run_id, date, &mut floors);
        }
    }
    // The ledger is read first, so the announced ids are complete before the transitions.
    let seen = m.seen.clone();
    let announced: HashSet<&str> = seen.iter().map(|(_, id)| id.as_str()).collect();
    let mut inherited = HashMap::new();
    let mut scan_log = |p: &Path| {
        each_line(p, |line| {
            if line.contains(&prefix) {
                scan_episode_ids(line, run_id, date, &mut floors);
                scan_inherited(line, &announced, &mut inherited);
            }
        });
    };
    for p in day_files(crate::oplog::TRANSITIONS_DIR) {
        scan_log(&p);
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
        scan_log(&e.path());
    }
    let mut inherited: Vec<_> = inherited
        .into_iter()
        .map(|(id, (s, h0))| (s, h0, id))
        .collect();
    inherited.sort();
    m.inherited = inherited;
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

    /// The H0 bar of announced micro-pullbacks, from a transition-log line and from a Detection
    /// journal record (nested under `emission.transition`); unannounced episodes, other families
    /// and lines without anchors contribute nothing.
    #[test]
    fn inherited_setups_from_both_log_shapes() {
        let ep = "live:2026-10-30:INO:micro_pullback:1";
        let ross = "live:2026-10-30:ROSS:micro_pullback:4";
        let announced: HashSet<&str> = [ep, ross].into_iter().collect();
        let mut out = HashMap::new();
        let log = serde_json::json!({
            "kind": "transition", "episode_id": ep, "symbol": "INO", "family": "micro_pullback",
            "anchors": {"h0_bar_end": 111, "l0": 1},
        });
        let journal = serde_json::json!({
            "Transition": {"journal_sequence": 3, "emission": {"transition": {
                "episode_id": ross, "symbol": "ROSS", "family": "micro_pullback",
                "anchors": {"h0_bar_end": 222},
            }}},
        });
        let other = serde_json::json!({
            "episode_id": "live:2026-10-30:OTHR:micro_pullback:1", "symbol": "OTHR",
            "family": "micro_pullback", "anchors": {"h0_bar_end": 333},
        });
        let bare =
            serde_json::json!({"episode_id": ep, "symbol": "INO", "family": "micro_pullback"});
        for v in [&log, &journal, &other, &bare] {
            scan_inherited(&v.to_string(), &announced, &mut out);
        }
        assert_eq!(out.len(), 2, "{out:?}");
        assert_eq!(out[ep], ("INO".to_string(), 111));
        assert_eq!(out[ross], ("ROSS".to_string(), 222));
    }

    /// A restart while a setup is armed, end to end: process 1 announces INO's setup; process 2
    /// restores the day (a proven order, so nothing is warm-up), opens the SAME setup again under
    /// the next number, and its arming stays quiet. When the price then goes through the
    /// trigger, the crossing is the follow-up of the setup process 1 announced, sent once,
    /// rather than lost (the new id was never "announced" in this process).
    #[test]
    fn a_setup_open_across_a_restart_is_recognized_by_its_h0() {
        use stock_context::detect::fixture::{self, Script, cutoff, end};
        use stock_context::{Adoption, State};
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("notices")).unwrap();
        std::fs::create_dir_all(root.join("transitions")).unwrap();
        let file = format!("{}.jsonl", fixture::day());
        let member = |_: &str| false;
        let since = |_: &str| Some(fixture::at(9, 30, 0));
        let price = |_: &str, _: i64| None;

        let mut s = Script::new();
        micro_at(&mut s, "INO", 0);
        s.advance(cutoff(11));
        let upto = s.steps.len();
        // After the restart: a quiet bar, then one through the trigger.
        s.bar("INO", 12, [334, 335, 333, 334], 200);
        s.advance(cutoff(12));
        s.bar("INO", 13, [334, 346, 334, 345], 400);
        s.advance(cutoff(13));

        // Process 1, up to the restart.
        let mut e1 = fixture::engine("live");
        let ts = fixture::drive(&mut e1, &s.steps[..upto]);
        let mut c1 = crate::cue_notices::CueNotices::default();
        c1.transitions_with(&ts, false, member, since, price);
        let sent = c1.take();
        assert_eq!(sent.len(), 1, "{sent:?}");
        let ledger: Vec<String> = sent
            .iter()
            .map(|n| decision(&n.event_id(), &n.symbol, n.at_ns))
            .collect();
        std::fs::write(root.join("notices").join(&file), ledger.join("\n")).unwrap();
        let log: Vec<String> = ts.iter().map(crate::market::transition_line).collect();
        std::fs::write(root.join("transitions").join(&file), log.join("\n")).unwrap();

        // Process 2: the restore replays the day to the restart, the live engine adopts its facts.
        let m = read(root, "live", end(11));
        assert_eq!(m.inherited.len(), 1, "{m:?}");
        let mut restore = fixture::engine("live");
        fixture::drive(&mut restore, &s.steps[..upto]);
        let mut e2 = fixture::engine("live");
        e2.set_episode_floors(m.date.unwrap(), m.floors.iter().cloned());
        e2.advance_to(UnixNanos(cutoff(11)));
        assert!(matches!(
            e2.adopt_facts(&mut restore, "INO"),
            Adoption::Adopted { .. }
        ));
        let mut c2 = crate::cue_notices::CueNotices::default();
        c2.restore(&m);
        let ts = fixture::drive(&mut e2, &s.steps[upto..]);
        let reopened: Vec<_> = ts
            .iter()
            .filter(|t| t.family == Family::MicroPullback)
            .collect();
        assert!(
            reopened.iter().all(|t| t.episode_id != sent[0].alert_id),
            "process 2 numbers past process 1: {reopened:?}"
        );
        assert!(
            reopened.iter().any(|t| t.to == State::CrossingProvisional),
            "the fixture crosses the trigger after the restart: {reopened:?}"
        );
        c2.transitions_with(&ts, false, member, since, price);
        let n = c2.take();
        assert_eq!(n.len(), 1, "{n:?}");
        assert_eq!(n[0].kind, NoticeKind::PullbackCrossing);
    }
}

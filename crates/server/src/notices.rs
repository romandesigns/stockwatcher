//! The notification log on the Insights page (owner, 2026-10-26: "create an insight page where we
//! can display a notification log"): the ledger's day file read back and grouped, one item per
//! notice (`GET /api/notices?day=YYYY-MM-DD`).
//!
//! The ledger (`crate::alerts::ledger`) writes several lines per notice: a `decision` per path
//! (desk, phone), a `pushed` when Expo took it, a `failed` when it refused. The page wants one row
//! per notice, so lines are grouped by `event_id`:
//!
//! - `decision` sets the notice's facts (first seen wins: both paths carry the same) and its
//!   `desk` / `phone` outcome `{decision, held, devices}`;
//! - `alert` (a fired price alert) is an item of its own, notice `price.alert`, with `phone`;
//! - `pushed` adds `latency` (the four latency figures it carries);
//! - `failed` adds `failed` (Expo's detail, already token-redacted by the ledger).
//!
//! Items come newest first, at most [`MAX_ITEMS`] (the newest; `truncated` says so). The file is
//! append-only and capped by its writer (32 MB), and a crash can leave a torn last line, which is
//! skipped like any malformed line. A day without a file (before the ledger existed on
//! 2026-10-26, a day the live server was down) is an empty list, not an error.
//!
//! **Pushed** (`notices.v1`, [`NoticeFeed`]; owner rule: realtime is pushed, clients only
//! render). The open log used to refetch the whole day every 15 s on every client. Now:
//!
//! - client `notices.subscribe {day?}` (absent = today, the server's ET date) -> server
//!   `notices.snapshot {day, days, items, truncated}`: the same body as `GET /api/notices`.
//! - Then, for today, `notices.append {day, items: [item]}` each time the ledger writes a line:
//!   that notice's item regrouped from all of its lines so far (a desk decision, then the phone
//!   one, then `pushed` with its latency), which the client upserts by `event_id` (new ones go
//!   on top). Subscribed to today without a `day`, the subscription follows the date: the first
//!   line of the next ET day answers with that day's snapshot.
//! - A past day gets its snapshot and nothing else (its file no longer changes). A lag resends
//!   the snapshot. `notices.unsubscribe` stops it.
//!
//! The feed keeps today's lines in memory, primed from today's file at start, so a snapshot of
//! today never races the ledger's background writer (a line can be broadcast before it is on
//! disk) and a restart mid-day still regroups an event whose first lines came before it. Bounded
//! like the file: past [`crate::oplog::NOTICES_CAP`] bytes in a day, lines stop being kept (the
//! writer stops writing them too). Every append carries a `seq` taken under the same lock as the
//! snapshot, so a connection forwards only appends newer than its snapshot and an item never goes
//! back to an older state.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use chrono::NaiveDate;
use serde_json::{Map, Value, json};
use tokio::sync::broadcast;

/// The feature a client must see in `welcome.features` before it sends `notices.subscribe`.
pub const FEATURE: &str = "notices.v1";

/// Rows returned for one day at most (the newest kept).
pub const MAX_ITEMS: usize = 2_000;

/// The dates that have a ledger file, newest first.
pub fn days(dir: &Path) -> Vec<NaiveDate> {
    let mut out: Vec<NaiveDate> = std::fs::read_dir(dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .filter_map(|e| {
                    let p = e.path();
                    // Exactly `<date>.jsonl`: never a partial copy or another log's file.
                    let name = p.file_name()?.to_str()?;
                    (name.len() == 16).then_some(())?;
                    crate::oplog::day_file_date(&p)
                })
                .collect()
        })
        .unwrap_or_default();
    out.sort_unstable_by(|a, b| b.cmp(a));
    out
}

/// One day's items, grouped and newest first, plus whether older ones were cut.
pub fn read_day(dir: &Path, day: NaiveDate) -> (Vec<Value>, bool) {
    let path = dir.join(format!("{}.jsonl", day.format("%Y-%m-%d")));
    let text = std::fs::read_to_string(path).unwrap_or_default();
    group(&text)
}

fn num(v: &Value, k: &str) -> Option<f64> {
    v.get(k).and_then(Value::as_f64)
}

/// Groups ledger lines into items (module docs). Pure, so it is tested without files.
pub fn group(text: &str) -> (Vec<Value>, bool) {
    let mut order: Vec<String> = Vec::new();
    let mut items: HashMap<String, Map<String, Value>> = HashMap::new();
    for line in text.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let Some(id) = v.get("event_id").and_then(Value::as_str) else {
            continue;
        };
        let kind = v.get("kind").and_then(Value::as_str).unwrap_or("");
        let item = items.entry(id.to_string()).or_insert_with(|| {
            order.push(id.to_string());
            let mut m = Map::new();
            m.insert("event_id".into(), json!(id));
            m
        });
        let set_if_absent = |m: &mut Map<String, Value>, k: &str, val: Option<Value>| {
            if let Some(val) = val.filter(|x| !x.is_null())
                && !m.contains_key(k)
            {
                m.insert(k.into(), val);
            }
        };
        match kind {
            "decision" => {
                for k in [
                    "symbol",
                    "price",
                    "notice",
                    "title",
                    "body",
                    "mp5_wick",
                    "breakout",
                    "entry_context",
                ] {
                    set_if_absent(item, k, v.get(k).cloned());
                }
                if let Some(at) = num(&v, "decided_at") {
                    let earlier = item
                        .get("at")
                        .and_then(Value::as_f64)
                        .is_none_or(|a| at < a);
                    if earlier {
                        item.insert("at".into(), json!(at as i64));
                    }
                }
                if let Some(path @ ("desk" | "phone")) = v.get("path").and_then(Value::as_str) {
                    let mut outcome = json!({
                        "decision": v.get("decision").cloned().unwrap_or(Value::Null),
                        "held": v.get("held").cloned().unwrap_or(Value::Null),
                        "devices": v.get("devices").cloned().unwrap_or(Value::Null),
                    });
                    // A merged Runners path (the merge removed 2026-10-30; old days still have it): what it was folded into.
                    for k in ["merged_into", "merged_devices"] {
                        if let Some(x) = v.get(k) {
                            outcome[k] = x.clone();
                        }
                    }
                    item.insert(path.into(), outcome);
                }
            }
            "alert" => {
                item.insert("notice".into(), json!("price.alert"));
                for k in ["symbol", "price"] {
                    set_if_absent(item, k, v.get(k).cloned());
                }
                set_if_absent(item, "title", v.get("text").cloned());
                if let Some(at) = num(&v, "decided_at") {
                    item.insert("at".into(), json!(at as i64));
                }
                item.insert(
                    "phone".into(),
                    json!({
                        "decision": v.get("phone").cloned().unwrap_or(Value::Null),
                        "held": v.get("held").cloned().unwrap_or(Value::Null),
                        "devices": v.get("devices").cloned().unwrap_or(Value::Null),
                    }),
                );
            }
            "pushed" => {
                for k in ["symbol", "notice"] {
                    set_if_absent(item, k, v.get(k).cloned());
                }
                if !item.contains_key("at")
                    && let Some(ns) = num(&v, "expo_ns")
                {
                    item.insert("at".into(), json!((ns / 1e6) as i64));
                }
                let mut lat = Map::new();
                for k in [
                    "trade_to_board_ms",
                    "board_to_queue_ms",
                    "queue_to_expo_ms",
                    "trade_to_expo_ms",
                ] {
                    if let Some(x) = v.get(k).filter(|x| !x.is_null()) {
                        lat.insert(k.into(), x.clone());
                    }
                }
                item.insert("latency".into(), Value::Object(lat));
            }
            "failed" => {
                item.insert(
                    "failed".into(),
                    v.get("detail").cloned().unwrap_or(Value::Null),
                );
                if !item.contains_key("at")
                    && let Some(at) = num(&v, "at")
                {
                    item.insert("at".into(), json!(at as i64));
                }
            }
            _ => {}
        }
    }
    // An item never decided here (a `pushed` or `failed` whose decision is in another day's file,
    // around midnight) still shows, at the time it has.
    let mut out: Vec<(i64, usize, Value)> = order
        .into_iter()
        .enumerate()
        .filter_map(|(i, id)| {
            let m = items.remove(&id)?;
            let at = m.get("at").and_then(Value::as_i64).unwrap_or(0);
            Some((at, i, Value::Object(m)))
        })
        .collect();
    // Newest first; ties keep file order reversed (the later line first).
    out.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.cmp(&a.1)));
    let truncated = out.len() > MAX_ITEMS;
    out.truncate(MAX_ITEMS);
    (out.into_iter().map(|(_, _, v)| v).collect(), truncated)
}

/// One `notices.append` on the feed's channel.
#[derive(Debug)]
pub struct Append {
    pub day: NaiveDate,
    /// Taken under the feed's lock: compare with [`Snapshot::seq`].
    pub seq: u64,
    pub frame: String,
}

/// A `notices.snapshot` and the append `seq` it already includes.
#[derive(Debug)]
pub struct Snapshot {
    pub day: NaiveDate,
    pub seq: u64,
    pub frame: String,
}

/// Today's ledger lines in memory, per event, in file order (module docs, "Pushed").
#[derive(Default)]
struct DayLines {
    day: Option<NaiveDate>,
    /// Event ids in order of first appearance, and each event's lines in order.
    order: Vec<String>,
    events: HashMap<String, Vec<String>>,
    bytes: u64,
    seq: u64,
}

impl DayLines {
    /// Starts `day` empty. `seq` keeps counting: a connection compares it across days.
    fn reset(&mut self, day: NaiveDate) {
        let seq = self.seq;
        *self = DayLines {
            day: Some(day),
            seq,
            ..Default::default()
        };
    }

    /// Keeps `line` under `id`; false once the day's cap is reached.
    fn keep(&mut self, id: &str, line: String) -> bool {
        if self.bytes + line.len() as u64 > crate::oplog::NOTICES_CAP {
            return false;
        }
        self.bytes += line.len() as u64 + 1;
        match self.events.get_mut(id) {
            Some(v) => v.push(line),
            None => {
                self.order.push(id.to_string());
                self.events.insert(id.to_string(), vec![line]);
            }
        }
        true
    }

    /// The whole day as ledger text, grouped per event. [`group`] reads it exactly as it reads
    /// the file: its result depends only on each event's line order and the order in which
    /// events first appear, both kept here.
    fn text(&self) -> String {
        let mut out = String::with_capacity(self.bytes as usize);
        for id in &self.order {
            for l in &self.events[id] {
                out.push_str(l);
                out.push('\n');
            }
        }
        out
    }
}

struct FeedInner {
    dir: Option<PathBuf>,
    lines: Mutex<DayLines>,
    tx: broadcast::Sender<Arc<Append>>,
}

/// The pushed notification log (module docs, "Pushed"). Cheap to clone. The ledger feeds it
/// every line it writes (`crate::alerts::ledger`); `/ws` sessions subscribe to it.
#[derive(Clone)]
pub struct NoticeFeed(Arc<FeedInner>);

impl std::fmt::Debug for NoticeFeed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NoticeFeed")
            .field("dir", &self.0.dir)
            .finish()
    }
}

impl NoticeFeed {
    /// No ledger (tests, a replay-only server): empty snapshots, never an append.
    pub fn idle() -> Self {
        Self::open(None, crate::oplog::et_date_ms(crate::oplog::now_ms()))
    }

    /// The feed over the ledger directory `dir`, primed with `today`'s file if there is one.
    pub fn open(dir: Option<PathBuf>, today: NaiveDate) -> Self {
        let mut lines = DayLines::default();
        lines.reset(today);
        if let Some(d) = &dir {
            let file = d.join(format!("{}.jsonl", today.format("%Y-%m-%d")));
            for line in std::fs::read_to_string(file).unwrap_or_default().lines() {
                // A torn or malformed line is skipped, as `group` skips it.
                if let Some(id) = event_id(line) {
                    lines.keep(&id, line.to_string());
                }
            }
        }
        NoticeFeed(Arc::new(FeedInner {
            dir,
            lines: Mutex::new(lines),
            tx: broadcast::channel(256).0,
        }))
    }

    pub fn dir(&self) -> Option<&Path> {
        self.0.dir.as_deref()
    }

    /// Subscribe BEFORE taking [`NoticeFeed::snapshot`].
    pub fn subscribe(&self) -> broadcast::Receiver<Arc<Append>> {
        self.0.tx.subscribe()
    }

    /// A ledger line, written for ET date `day`: kept, and its notice's regrouped item broadcast.
    /// A line for an earlier day than the one held (a `pushed` stamped just before midnight,
    /// arriving after it) is in that day's file only; a later day starts a new day.
    pub fn push(&self, day: NaiveDate, line: &Value) {
        let Some(id) = line.get("event_id").and_then(Value::as_str) else {
            return;
        };
        let mut l = self.0.lines.lock().expect("notice feed lock");
        match l.day {
            Some(d) if day < d => return,
            Some(d) if day == d => {}
            _ => l.reset(day),
        }
        if !l.keep(id, line.to_string()) {
            return;
        }
        let (items, _) = group(&l.events[id].join("\n"));
        l.seq += 1;
        let frame = json!({
            "type": "notices.append",
            "day": day.format("%Y-%m-%d").to_string(),
            "items": items,
        })
        .to_string();
        // Under the lock, so appends reach the channel in `seq` order.
        let _ = self.0.tx.send(Arc::new(Append {
            day,
            seq: l.seq,
            frame,
        }));
    }

    /// The `notices.snapshot` of `day` (None = today in ET). Reads files (the day list, and a
    /// day not held in memory), so call it off the async workers.
    pub fn snapshot(&self, day: Option<NaiveDate>) -> Snapshot {
        let day = day.unwrap_or_else(|| crate::oplog::et_date_ms(crate::oplog::now_ms()));
        let days = self.0.dir.as_deref().map(days).unwrap_or_default();
        let (held, seq) = {
            let l = self.0.lines.lock().expect("notice feed lock");
            ((l.day == Some(day)).then(|| l.text()), l.seq)
        };
        let (items, truncated) = match (held, &self.0.dir) {
            (Some(text), _) => group(&text),
            (None, Some(dir)) => read_day(dir, day),
            (None, None) => (Vec::new(), false),
        };
        let frame = json!({
            "type": "notices.snapshot",
            "day": day.format("%Y-%m-%d").to_string(),
            "days": days.iter().map(|d| d.format("%Y-%m-%d").to_string()).collect::<Vec<_>>(),
            "items": items,
            "truncated": truncated,
        })
        .to_string();
        Snapshot { day, seq, frame }
    }
}

fn event_id(line: &str) -> Option<String> {
    serde_json::from_str::<Value>(line)
        .ok()?
        .get("event_id")?
        .as_str()
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(v: Value) -> String {
        v.to_string()
    }

    #[test]
    fn one_item_per_notice_newest_first() {
        let text = [
            line(json!({"kind":"decision","v":1,"decided_at":1000,"event_id":"a","notice":"runners.burst","symbol":"SRTM","price":0.55,"path":"desk","decision":"sent","held":null,"devices":2,"title":"SRTM · Buying burst","body":"b"})),
            line(json!({"kind":"decision","v":1,"decided_at":1001,"event_id":"a","notice":"runners.burst","symbol":"SRTM","price":0.55,"path":"phone","decision":"held","held":"symbol_limit","devices":1,"title":"SRTM · Buying burst","body":"b"})),
            line(json!({"kind":"pushed","v":1,"event_id":"a","notice":"runners.burst","symbol":"SRTM","expo_ns":1_500_000_000i64,"trade_to_expo_ms":1044.6,"board_to_queue_ms":838.4,"queue_to_expo_ms":105.1,"trade_to_board_ms":100.9})),
            line(json!({"kind":"alert","v":1,"decided_at":2000,"event_id":"p","alert_id":"x","symbol":"AAPL","price":200.0,"text":"AAPL above 200","phone":"sent","held":null,"devices":1})),
            line(json!({"kind":"failed","v":1,"at":2500,"event_id":"p","detail":"DeviceNotRegistered"})),
            "{\"kind\":\"decision\",\"event_id\":\"torn".to_string(),
        ]
        .join("\n");
        let (items, truncated) = group(&text);
        assert!(!truncated);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0]["notice"], "price.alert");
        assert_eq!(items[0]["phone"]["decision"], "sent");
        assert_eq!(items[0]["failed"], "DeviceNotRegistered");
        assert_eq!(items[1]["symbol"], "SRTM");
        assert_eq!(items[1]["at"], 1000);
        assert_eq!(items[1]["desk"]["decision"], "sent");
        assert_eq!(items[1]["phone"]["held"], "symbol_limit");
        assert_eq!(items[1]["latency"]["trade_to_expo_ms"], 1044.6);
    }

    /// A merged Runners path (old days, before the 2026-10-30 removal) keeps what it was folded into.
    #[test]
    fn a_merged_path_says_what_it_was_folded_into() {
        let text = line(
            json!({"kind":"decision","v":1,"decided_at":3000,"event_id":"f",
            "notice":"runners.fast","symbol":"WFF","path":"phone","decision":"held","held":"merged",
            "devices":2,"merged_into":{"event_id":"r","notice":"runners.running","secs":1.0},
            "merged_devices":2}),
        );
        let (items, _) = group(&text);
        assert_eq!(items[0]["phone"]["held"], "merged");
        assert_eq!(
            items[0]["phone"]["merged_into"]["notice"],
            "runners.running"
        );
        assert_eq!(items[0]["phone"]["merged_devices"], 2);
        assert!(items[0].get("desk").is_none());
    }

    #[test]
    fn caps_to_the_newest_and_lists_days() {
        let text: String = (0..MAX_ITEMS + 5)
            .map(|i| line(json!({"kind":"decision","decided_at":i,"event_id":format!("e{i}"),"path":"desk","decision":"sent"})))
            .collect::<Vec<_>>()
            .join("\n");
        let (items, truncated) = group(&text);
        assert!(truncated);
        assert_eq!(items.len(), MAX_ITEMS);
        assert_eq!(items[0]["event_id"], format!("e{}", MAX_ITEMS + 4));

        let dir = tempfile::tempdir().unwrap();
        for f in [
            "2026-10-26.jsonl",
            "2026-10-27.jsonl",
            ".incoming-2026-10-28.jsonl",
            "notes.txt",
        ] {
            std::fs::write(dir.path().join(f), "").unwrap();
        }
        let d = days(dir.path());
        assert_eq!(
            d.iter().map(|x| x.to_string()).collect::<Vec<_>>(),
            vec!["2026-10-27", "2026-10-26"]
        );
        // A day without a file is empty, not an error.
        assert_eq!(
            read_day(dir.path(), "2026-10-20".parse().unwrap()).0.len(),
            0
        );
    }

    #[test]
    fn frozen_wick_observations_reach_the_read_api_unchanged() {
        let wick = serde_json::to_value(stock_context::MicroWick::measure(
            Some([105, 120, 100, 108]),
            15,
            32,
        ))
        .unwrap();
        let text = line(json!({"kind":"decision","event_id":"w","decided_at":1000,
            "symbol":"WFF","notice":"runners.pullback","path":"phone","decision":"held",
            "held":"symbol_limit","mp5_wick":wick}));
        let (items, _) = group(&text);
        assert_eq!(items[0]["mp5_wick"], wick);
        assert_eq!(items[0]["phone"]["decision"], "held");
    }

    fn d(s: &str) -> NaiveDate {
        s.parse().unwrap()
    }

    fn take(rx: &mut broadcast::Receiver<Arc<Append>>) -> Vec<Value> {
        let mut out = Vec::new();
        while let Ok(a) = rx.try_recv() {
            out.push(serde_json::from_str(&a.frame).unwrap());
        }
        out
    }

    /// The pushed log: each ledger line re-sends its notice's whole item; the snapshot of today
    /// comes from memory (primed from the file at start) and groups as the file does.
    #[test]
    fn the_feed_appends_regrouped_items_and_snapshots_today_from_memory() {
        let dir = tempfile::tempdir().unwrap();
        let before = line(
            json!({"kind":"decision","decided_at":100,"event_id":"old","notice":"runners.fast",
            "symbol":"AAA","path":"desk","decision":"sent"}),
        );
        std::fs::write(dir.path().join("2026-10-30.jsonl"), format!("{before}\n")).unwrap();
        let feed = NoticeFeed::open(Some(dir.path().to_path_buf()), d("2026-10-30"));
        let mut rx = feed.subscribe();
        let snap = feed.snapshot(Some(d("2026-10-30")));
        let v: Value = serde_json::from_str(&snap.frame).unwrap();
        assert_eq!(v["type"], "notices.snapshot");
        assert_eq!(v["items"][0]["event_id"], "old", "primed from today's file");
        assert_eq!(v["days"], json!(["2026-10-30"]));

        // The restart's earlier event gets its phone decision now: the append carries both paths.
        feed.push(
            d("2026-10-30"),
            &json!({"kind":"decision","decided_at":101,"event_id":"old","notice":"runners.fast",
            "symbol":"AAA","path":"phone","decision":"held","held":"quiet_hours"}),
        );
        feed.push(
            d("2026-10-30"),
            &json!({"kind":"decision","decided_at":200,"event_id":"new","notice":"runners.burst",
            "symbol":"BBB","path":"desk","decision":"sent"}),
        );
        feed.push(d("2026-10-30"), &json!({"no_event_id": true}));
        let got = take(&mut rx);
        assert_eq!(got.len(), 2);
        assert_eq!(got[0]["type"], "notices.append");
        assert_eq!(got[0]["items"][0]["desk"]["decision"], "sent");
        assert_eq!(got[0]["items"][0]["phone"]["held"], "quiet_hours");
        assert_eq!(got[1]["items"][0]["symbol"], "BBB");
        let snap2 = feed.snapshot(Some(d("2026-10-30")));
        assert!(snap2.seq > snap.seq);
        let v: Value = serde_json::from_str(&snap2.frame).unwrap();
        assert_eq!(v["items"][0]["event_id"], "new", "newest first");
        assert_eq!(v["items"][1]["phone"]["decision"], "held");

        // A line for the day before is that day's file's business; the next day starts afresh.
        feed.push(
            d("2026-10-29"),
            &json!({"kind":"failed","at":5,"event_id":"x","detail":"d"}),
        );
        assert!(take(&mut rx).is_empty());
        feed.push(
            d("2026-10-31"),
            &json!({"kind":"decision","decided_at":300,"event_id":"n2","path":"desk","decision":"sent"}),
        );
        assert_eq!(take(&mut rx)[0]["day"], "2026-10-31");
        let v: Value = serde_json::from_str(&feed.snapshot(Some(d("2026-10-31"))).frame).unwrap();
        assert_eq!(v["items"].as_array().unwrap().len(), 1);
        // A day no longer held is read from its file.
        let v: Value = serde_json::from_str(&feed.snapshot(Some(d("2026-10-30"))).frame).unwrap();
        assert_eq!(v["items"][0]["event_id"], "old");
    }

    #[test]
    fn an_idle_feed_is_empty() {
        let feed = NoticeFeed::idle();
        let v: Value = serde_json::from_str(&feed.snapshot(Some(d("2026-10-30"))).frame).unwrap();
        assert_eq!(
            (v["items"].clone(), v["days"].clone()),
            (json!([]), json!([]))
        );
    }
}

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

use std::collections::HashMap;
use std::path::Path;

use chrono::NaiveDate;
use serde_json::{Map, Value, json};

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
                    // A merged Runners path (alerts::runners, "Merging"): what it was folded into.
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

    /// A merged Runners path keeps what it was folded into (alerts::runners, "Merging").
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
}

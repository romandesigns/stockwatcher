//! The replay bar's alert dots: what the Runners panel announced on a replay day, read back from
//! the live market's forward logs (`GET /api/replay/alerts?day=YYYY-MM-DD[&symbol=X]`).
//!
//! The replay bar plots these on its scrubber and its Prev/Next alert buttons seek to them, so a
//! practice session can start exactly where an alert fired (owner, 2026-10-24: "practise on the
//! moments alerts fired"). Read-only and on demand: the replay itself never re-runs the Runners
//! rules, it only shows what the live server logged that day.
//!
//! Sources (both written by the live market under `ARCHIVE_ROOT`, see `crate::rocket::log`):
//!
//! - `rocket-watch/<date>.jsonl`: the ENTRY kinds only, one dot each: `alert` (the precursors-R2
//!   flag), `fast` (Fast move), `run` (Run +20%), `burst`, `running`. `state`, `later`,
//!   `outcome`, `burst_outcome`, `also` and `day` lines describe an entry after the fact (or the
//!   day), are not moments an alert fired, and would leak the outcome into a practice run.
//! - `zones/<date>.jsonl`: `entry` lines with `kind_zone: "volume_shelf"` (a volume-shelf
//!   entry), at `t_ns`. Its `run` and `outcome` lines are skipped for the same reason.
//!
//! A day with no log (before the logs existed, a day the live server was down, a fixture day) is
//! an empty list, not an error. A malformed line is skipped: a file is append-only and a crash can
//! leave a torn last line. Each file is capped by its writer (`MAX_FILE_BYTES`, 8 MiB), so reading
//! it whole per request is bounded; requests come once per day switch.

use std::path::Path;

use chrono::NaiveDate;
use serde::Serialize;
use serde_json::Value;

/// One dot on the scrubber.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ReplayAlert {
    /// When it fired, epoch ms.
    pub t: i64,
    /// `r2` | `fast` | `run` | `burst` | `running` | `shelf` (the clients' colour and label key).
    pub kind: &'static str,
    pub symbol: String,
    /// The price the entry logged, when it did.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub price: Option<f64>,
    /// The entry's first reason ("+23% in 60 s (held)"), for the tooltip.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// The rocket-watch kind → the dot's kind. Anything else is not an entry (see the module comment).
fn rocket_kind(kind: &str) -> Option<&'static str> {
    Some(match kind {
        "alert" => "r2",
        "fast" => "fast",
        "run" => "run",
        "burst" => "burst",
        "running" => "running",
        _ => return None,
    })
}

fn read_lines(path: &Path) -> Vec<Value> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    text.lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .collect()
}

fn ns_to_ms(v: &Value) -> Option<i64> {
    v.as_i64()
        .or_else(|| v.as_u64().and_then(|n| i64::try_from(n).ok()))
        .map(|ns| ns / 1_000_000)
}

/// Every alert dot of `day` (optionally only `symbol`'s), ascending by time.
pub fn read_day(root: &Path, day: NaiveDate, symbol: Option<&str>) -> Vec<ReplayAlert> {
    let file = format!("{day}.jsonl");
    let wanted = |s: &str| symbol.is_none_or(|w| w.eq_ignore_ascii_case(s));
    let mut out = Vec::new();
    for v in read_lines(&root.join("rocket-watch").join(&file)) {
        let Some(kind) = v["kind"].as_str().and_then(rocket_kind) else {
            continue;
        };
        let (Some(sym), Some(t)) = (v["symbol"].as_str(), ns_to_ms(&v["alert_ns"])) else {
            continue;
        };
        if !wanted(sym) {
            continue;
        }
        out.push(ReplayAlert {
            t,
            kind,
            symbol: sym.to_string(),
            price: v["price"].as_f64(),
            reason: v["reasons"][0].as_str().map(str::to_string),
        });
    }
    for v in read_lines(&root.join("zones").join(&file)) {
        if v["kind"] != "entry" || v["kind_zone"] != "volume_shelf" {
            continue;
        }
        let (Some(sym), Some(t)) = (v["symbol"].as_str(), ns_to_ms(&v["t_ns"])) else {
            continue;
        };
        if !wanted(sym) {
            continue;
        }
        out.push(ReplayAlert {
            t,
            kind: "shelf",
            symbol: sym.to_string(),
            price: v["entry_price"].as_f64(),
            reason: None,
        });
    }
    out.sort_by(|a, b| a.t.cmp(&b.t).then_with(|| a.symbol.cmp(&b.symbol)));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(root: &Path, dir: &str, day: &str, lines: &[Value]) {
        let d = root.join(dir);
        std::fs::create_dir_all(&d).unwrap();
        let mut text: String = lines.iter().map(|l| format!("{l}\n")).collect();
        // A torn last line, as a crash mid-append leaves it.
        text.push_str("{\"kind\":\"fast\",\"sym");
        std::fs::write(d.join(format!("{day}.jsonl")), text).unwrap();
    }

    fn day() -> NaiveDate {
        "2026-10-23".parse().unwrap()
    }

    fn fixture() -> tempfile::TempDir {
        use serde_json::json;
        let tmp = tempfile::tempdir().unwrap();
        write(
            tmp.path(),
            "rocket-watch",
            "2026-10-23",
            &[
                json!({"kind":"fast","v":1,"symbol":"LABT","alert_ns":1_792_742_415_000_000_000_i64,"price":0.8877,"reasons":["+23% in 60 s (held)"]}),
                json!({"kind":"state","v":1,"symbol":"LABT","alert_ns":1_792_742_415_000_000_000_i64,"at_ns":1_792_742_500_000_000_000_i64}),
                json!({"kind":"later","v":1,"symbol":"LABT","alert_ns":1_792_742_415_000_000_000_i64}),
                json!({"kind":"alert","v":1,"symbol":"XXII","alert_ns":1_792_747_950_000_000_000_i64,"price":0.5844,"reasons":["+171% on the day"]}),
                json!({"kind":"outcome","v":1,"symbol":"XXII","alert_ns":1_792_747_950_000_000_000_i64}),
                json!({"kind":"run","v":1,"symbol":"labt","alert_ns":1_792_742_300_000_000_000_i64,"price":0.9}),
                json!({"kind":"burst","v":1,"symbol":"HUBC","alert_ns":1_792_742_600_000_000_000_i64}),
                json!({"kind":"running","v":1,"symbol":"HUBC","alert_ns":1_792_742_700_000_000_000_i64}),
                json!({"kind":"burst_outcome","v":1,"symbol":"HUBC","alert_ns":1_792_742_600_000_000_000_i64}),
                json!({"kind":"day","v":1,"date":"2026-10-23"}),
            ],
        );
        write(
            tmp.path(),
            "zones",
            "2026-10-23",
            &[
                json!({"kind":"run","v":1,"symbol":"HUBC","plus20_ns":1_792_742_565_000_000_000_i64}),
                json!({"kind":"entry","v":1,"kind_zone":"volume_shelf","symbol":"HUBC","t_ns":1_792_742_760_000_000_000_i64,"entry_price":0.2774}),
                json!({"kind":"outcome","v":1,"symbol":"HUBC","t_ns":1_792_742_760_000_000_000_i64}),
            ],
        );
        tmp
    }

    #[test]
    fn reads_entry_kinds_only_in_time_order() {
        let tmp = fixture();
        let got = read_day(tmp.path(), day(), None);
        let kinds: Vec<(&str, &str, i64)> = got
            .iter()
            .map(|a| (a.kind, a.symbol.as_str(), a.t))
            .collect();
        assert_eq!(
            kinds,
            vec![
                ("run", "labt", 1_792_742_300_000),
                ("fast", "LABT", 1_792_742_415_000),
                ("burst", "HUBC", 1_792_742_600_000),
                ("running", "HUBC", 1_792_742_700_000),
                ("shelf", "HUBC", 1_792_742_760_000),
                ("r2", "XXII", 1_792_747_950_000),
            ]
        );
        assert_eq!(got[1].price, Some(0.8877));
        assert_eq!(got[1].reason.as_deref(), Some("+23% in 60 s (held)"));
        assert_eq!(got[4].price, Some(0.2774));
    }

    #[test]
    fn filters_by_symbol_ignoring_case() {
        let tmp = fixture();
        let got = read_day(tmp.path(), day(), Some("LABT"));
        assert_eq!(got.len(), 2);
        assert!(got.iter().all(|a| a.symbol.eq_ignore_ascii_case("LABT")));
        assert!(read_day(tmp.path(), day(), Some("NOPE")).is_empty());
    }

    #[test]
    fn a_day_without_logs_is_empty() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(read_day(tmp.path(), day(), None).is_empty());
    }
}

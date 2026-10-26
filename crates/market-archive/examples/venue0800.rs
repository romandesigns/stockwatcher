//! Research (research/fast-0800): what changes in the tape at 08:00 ET.
//!
//! For each day given, reads every symbol's archived trades between 07:50 and 08:10 ET and prints
//! one JSON line: trades per minute per reporting venue (`x`), sale-condition sets in the five
//! minutes before and after 08:00, and how many symbols printed in 07:50–07:59 vs 08:00–08:01.
//! Read-only; `ARCHIVE_ROOT` names the archive.
//!
//!     ARCHIVE_ROOT=C:\research\stockwatcher\archive-past cargo run --release -p market-archive --example venue0800 -- 2025-10-01 2025-10-02

use std::collections::{BTreeMap, BTreeSet};

use chrono::{NaiveDate, Timelike};
use chrono_tz::America::New_York;
use market_archive::{Archive, ArchiveConfig};
use market_events::{MarketEvent, UnixNanos};

fn main() {
    let cfg = ArchiveConfig::from_lookup(|k| std::env::var(k).ok()).expect("archive config");
    let archive = Archive::new(cfg);
    // The boundary minute; the window is [b-10, b+10), "pre" [b-5, b), "post" [b, b+5).
    let b: u32 = std::env::var("VWIN_MIN")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(480);
    for d in std::env::args().skip(1) {
        let day: NaiveDate = d.parse().expect("day YYYY-MM-DD");
        let Ok(Some(m)) = archive.manifest(day) else {
            eprintln!("{day}: no manifest");
            continue;
        };
        // minute of day (ET) → venue → trades
        let mut per_min: BTreeMap<u32, BTreeMap<String, u64>> = BTreeMap::new();
        let mut cond_pre: BTreeMap<String, u64> = BTreeMap::new();
        let mut cond_post: BTreeMap<String, u64> = BTreeMap::new();
        let mut syms_pre: BTreeSet<String> = BTreeSet::new();
        let mut syms_post: BTreeSet<String> = BTreeSet::new();
        for e in m.symbols.values() {
            let Ok(reader) = archive.reader(day, &e.symbol) else {
                continue;
            };
            for ev in reader.flatten() {
                let MarketEvent::Trade(t) = &ev.event else {
                    continue;
                };
                let et = UnixNanos(t.event_ts.nanos())
                    .to_utc()
                    .with_timezone(&New_York);
                let min = et.hour() * 60 + et.minute();
                if !(b - 10..b + 10).contains(&min) {
                    if min >= b + 10 {
                        break;
                    }
                    continue;
                }
                *per_min
                    .entry(min)
                    .or_default()
                    .entry(t.exchange.clone())
                    .or_default() += 1;
                let mut c = t.conditions.clone();
                c.sort();
                let key = if c.is_empty() {
                    "-".to_string()
                } else {
                    c.join("+")
                };
                if (b - 5..b).contains(&min) {
                    *cond_pre
                        .entry(format!("{}|{}", t.exchange, key))
                        .or_default() += 1;
                } else if (b..b + 5).contains(&min) {
                    *cond_post
                        .entry(format!("{}|{}", t.exchange, key))
                        .or_default() += 1;
                }
                if (b - 10..b).contains(&min) {
                    syms_pre.insert(e.symbol.clone());
                } else if (b..b + 2).contains(&min) {
                    syms_post.insert(e.symbol.clone());
                }
            }
        }
        let only_post = syms_post.difference(&syms_pre).count();
        let line = serde_json::json!({
            "day": day.to_string(),
            "per_min": per_min.iter().map(|(k, v)| (format!("{:02}:{:02}", k / 60, k % 60), v)).collect::<BTreeMap<_, _>>(),
            "cond_pre": cond_pre,
            "cond_post": cond_post,
            "symbols_0750_0759": syms_pre.len(),
            "symbols_0800_0801": syms_post.len(),
            "symbols_first_at_0800": only_post,
        });
        println!("{line}");
    }
}

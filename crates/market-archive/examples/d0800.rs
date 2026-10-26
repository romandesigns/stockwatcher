//! Research (research/fast-0800): one symbol-day's prints around 08:00 ET, to tell a batch of
//! late-reported trades from real executions at 08:00.
//!
//! Args: `DAY SYMBOL [DAY SYMBOL …]`. Prints one JSON line per symbol-day with: the exchange
//! (non-`D`) price range and last price 04:00–07:59; every print 07:59:00–08:02:00 summarised by
//! second and venue (count, shares, min/max price, condition sets); the first 08:00 `D` prints.
//! Read-only; `ARCHIVE_ROOT` names the archive. `VWIN_MIN` (default 480 = 08:00) moves the
//! boundary; with 240 (04:00) "pre_exchange" is empty (the archive starts at 04:00) and the
//! range to compare is the exchange prints of the 15 minutes after the boundary.

use std::collections::BTreeMap;

use chrono::{NaiveDate, Timelike};
use chrono_tz::America::New_York;
use market_archive::{Archive, ArchiveConfig};
use market_events::{MarketEvent, UnixNanos};

fn main() {
    let cfg = ArchiveConfig::from_lookup(|k| std::env::var(k).ok()).expect("archive config");
    let archive = Archive::new(cfg);
    let args: Vec<String> = std::env::args().skip(1).collect();
    let b: i64 = std::env::var("VWIN_MIN")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(480);
    for pair in args.chunks(2) {
        let [d, sym] = pair else { continue };
        let day: NaiveDate = d.parse().expect("day");
        let Ok(reader) = archive.reader(day, sym) else {
            eprintln!("{day} {sym}: no file");
            continue;
        };
        let (mut pre_lo, mut pre_hi, mut pre_last, mut pre_n) = (f64::MAX, 0f64, 0f64, 0u64);
        // second offset from 07:59:00 → venue → (n, shares, lo, hi)
        let mut secs: BTreeMap<i64, BTreeMap<String, (u64, u64, f64, f64)>> = BTreeMap::new();
        let mut first_d: Vec<serde_json::Value> = Vec::new();
        for ev in reader.flatten() {
            let MarketEvent::Trade(t) = &ev.event else {
                continue;
            };
            let et = UnixNanos(t.event_ts.nanos())
                .to_utc()
                .with_timezone(&New_York);
            let s = (et.hour() * 3600 + et.minute() * 60 + et.second()) as i64;
            let p = t.price.to_f64();
            let pre = if b == 480 {
                (4 * 3600..8 * 3600).contains(&s)
            } else {
                (b * 60..b * 60 + 900).contains(&s)
            };
            if pre && t.exchange != "D" {
                pre_lo = pre_lo.min(p);
                pre_hi = pre_hi.max(p);
                pre_last = p;
                pre_n += 1;
            }
            let off = s - (b * 60 - 60);
            if (0..180).contains(&off) {
                let e = secs
                    .entry(off)
                    .or_default()
                    .entry(t.exchange.clone())
                    .or_insert((0, 0, f64::MAX, 0.0));
                e.0 += 1;
                e.1 += t.size;
                e.2 = e.2.min(p);
                e.3 = e.3.max(p);
                if t.exchange == "D" && first_d.len() < 25 {
                    first_d.push(serde_json::json!({"t": et.format("%H:%M:%S%.6f").to_string(), "p": p, "s": t.size, "c": t.conditions}));
                }
            }
            if s >= (b * 60 + 900).max(8 * 3600 + 180) {
                break;
            }
        }
        let line = serde_json::json!({
            "day": day.to_string(), "sym": sym,
            "pre_exchange": {"n": pre_n, "lo": pre_lo, "hi": pre_hi, "last": pre_last},
            "by_second": secs.iter().map(|(k, v)| (k.to_string(), v.iter().map(|(x, (n, sh, lo, hi))| (x.clone(), serde_json::json!([n, sh, lo, hi]))).collect::<BTreeMap<_, _>>())).collect::<BTreeMap<_, _>>(),
            "first_d": first_d,
        });
        println!("{line}");
    }
}

//! The live wrapper around [`super::logic`]: prints into bars, halt spans with their reason
//! codes, the facts, the watched-window rule and the notice. The decision itself is tested in
//! `logic` and, against the research's own rows on every Train day, by the parity harness
//! (`C:/wavystack/outputs/stockwatcher-research-v2/50-ship/run-halt/`).

use super::*;
use crate::n1_live::Facts;
use market_events::session::et;
use market_events::{Price, Trade};

const BOTH: Counted = Counted {
    ohlc: true,
    volume: true,
};
const S: i64 = logic::SEC;

fn day() -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 11, 18).unwrap()
}

fn at(h: u32, m: u32, s: u32) -> i64 {
    et(day(), h, m, s, 0).unwrap().0
}

fn tr(sym: &str, ts: i64, micros: i64) -> Trade {
    Trade::new(sym, UnixNanos(ts), Price::from_micros(micros), 100, 1)
}

fn table() -> FactsTable {
    FactsTable {
        date: Some(day()),
        facts: ["ABCD", "ETFX"]
            .iter()
            .map(|s| {
                (
                    s.to_string(),
                    Facts {
                        c1: Some(1.5),
                        dadv20: None,
                    },
                )
            })
            .collect(),
        types: [("ETFX".to_string(), SecType::Excluded)].into(),
        ..Default::default()
    }
}

/// `sym` climbs from $2.00 to $2.20 over 10:00–10:10 (a print every 5 s), an LULD pause
/// (`LUDP`) at 10:10:00, the resume status at 10:15:00 and a reopening print at `reopen_px`
/// at 10:15:00.3; ticks every second until 10:16 and returns the notices.
fn day_with_halt(x: &mut XHaltLive, sym: &str, reason: &str, reopen_px: i64) -> Vec<Notice> {
    let halt = at(10, 10, 0);
    let resume = at(10, 15, 0);
    let mut prints: Vec<(i64, i64)> = (0..120)
        .map(|i| (at(10, 0, 0) + i * 5 * S, 2_000_000 + i * 1_680))
        .collect();
    prints.push((resume + 300_000_000, reopen_px));
    prints.push((resume + 2 * S, reopen_px + 10_000));
    let mut out = Vec::new();
    let mut k = 0;
    let mut now = at(9, 40, 0);
    let mut halted = false;
    let mut resumed = false;
    while now <= at(10, 16, 0) {
        while k < prints.len() && prints[k].0 < now {
            x.apply_trade(&tr(sym, prints[k].0, prints[k].1), BOTH);
            k += 1;
        }
        if !halted && now > halt {
            x.apply_halt(sym, halt, None, reason);
            halted = true;
        }
        if !resumed && now > resume {
            x.apply_halt(sym, halt, Some(resume), reason);
            resumed = true;
        }
        x.tick(now);
        out.extend(x.take_notices());
        now += S;
    }
    out
}

fn live() -> XHaltLive {
    let mut x = XHaltLive::new();
    x.tick(at(3, 30, 0));
    assert_eq!(x.take_facts_request(), Some(day()));
    x.set_facts(Arc::new(table()));
    // Watching from 09:00 (a print of some other stock).
    x.apply_trade(&tr("ZZZZ", at(9, 0, 0), 5_000_000), BOTH);
    x
}

#[test]
fn a_resume_up_after_an_luld_pause_is_one_notice_at_the_research_instant() {
    let mut x = live();
    let n = day_with_halt(&mut x, "ABCD", "LUDP", 2_250_000);
    assert_eq!(n.len(), 1, "{n:?}");
    let n = &n[0];
    assert_eq!(n.kind, NoticeKind::ResumedUp);
    // The reopening second (10:15:00) ends at 10:15:01; the alert is 1 s later.
    assert_eq!(n.at_ns, at(10, 15, 2));
    assert_eq!(n.price, 2.25);
    assert_eq!(
        n.alert_id,
        format!("2026-11-18-ABCD-xh{}", at(10, 10, 0) / S)
    );
    let Some(NoticeDetail::ResumedUp(d)) = &n.detail else {
        panic!("{n:?}")
    };
    assert_eq!(d.reason, "LULD pause");
    assert_eq!(d.halt_ns, at(10, 10, 0));
    assert_eq!(d.resume_ns, Some(at(10, 15, 0)));
    assert_eq!(d.reopen_res_s, 1);
    // The last 15 s close before the halt, and the run-in over the 15-minute low ($2.00).
    assert!((d.halt_px - 2.19992).abs() < 1e-9, "{}", d.halt_px);
    assert!(d.run_in > 0.09 && d.run_in < 0.11, "{}", d.run_in);
    assert_eq!(x.cost()["notified"], 1);
}

#[test]
fn untyped_below_the_halt_price_excluded_or_unwatched_sends_nothing() {
    // A news-released halt (T2) is not one of the two typed reasons.
    assert!(day_with_halt(&mut live(), "ABCD", "T2", 2_250_000).is_empty());
    // News pending (T1) is.
    assert_eq!(day_with_halt(&mut live(), "ABCD", "T1", 2_250_000).len(), 1);
    // Reopened below the halt price.
    assert!(day_with_halt(&mut live(), "ABCD", "LUDP", 2_100_000).is_empty());
    // An excluded type, or a symbol with no previous close.
    assert!(day_with_halt(&mut live(), "ETFX", "LUDP", 2_250_000).is_empty());
    assert!(day_with_halt(&mut live(), "NOC1", "LUDP", 2_250_000).is_empty());
    // Watching only since 09:58 (the 15 minutes before the 10:10 halt were not all seen).
    let mut x = XHaltLive::new();
    x.tick(at(3, 30, 0));
    let _ = x.take_facts_request();
    x.set_facts(Arc::new(table()));
    x.apply_trade(&tr("ZZZZ", at(9, 58, 0), 5_000_000), BOTH);
    assert!(day_with_halt(&mut x, "ABCD", "LUDP", 2_250_000).is_empty());
    assert_eq!(x.cost()["unwatched"], 1);
}

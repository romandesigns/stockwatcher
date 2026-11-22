//! The live wrapper around [`super::logic`]: the day, its security types, completeness and the
//! notices. The trigger itself is tested in `logic` and, against the research's own rows, by the
//! parity harness (`outputs/stockwatcher-research-v2/50-ship/float/`).

use super::*;
use market_events::Price;
use market_events::session::et;

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

fn tr(sym: &str, ts: i64, micros: i64, size: u64) -> Trade {
    let mut t = Trade::new(sym, UnixNanos(ts), Price::from_micros(micros), size, 1);
    t.receive_ts = Some(UnixNanos(ts + 7_000_000));
    t
}

fn table() -> Arc<FactsTable> {
    Arc::new(FactsTable {
        date: Some(day()),
        types: [("ETFX".to_string(), SecType::Excluded)].into(),
        ..Default::default()
    })
}

/// 1 M float, C1 $2.00.
fn inp(_: &str) -> Inputs {
    inputs(Some(1_000_000), Some(2.0))
}

/// A $2.10 print at 09:00 (gain 5 %), then 300,000 shares at $2.30 at 09:01:05 (gain 15 %, 30 %
/// of the float, a new high): one trigger, the bar 09:01:00–09:01:15, decided at 09:01:15.050.
fn run(n: &mut XFloatLive, sym: &str, from: i64) -> Vec<Notice> {
    let prints = [
        (at(9, 0, 0), 2_100_000, 1_000),
        (at(9, 1, 5), 2_300_000, 300_000),
    ];
    let mut out = Vec::new();
    let mut k = 0;
    let mut now = from;
    while now <= at(9, 2, 0) {
        while k < prints.len() && prints[k].0 < now {
            n.apply_trade(&tr(sym, prints[k].0, prints[k].1, prints[k].2), BOTH);
            k += 1;
        }
        n.tick(now, inp);
        out.extend(n.take_notices());
        // The rocket task: 50 ms past each second.
        now += S;
    }
    out
}

fn started(at_ns: i64) -> XFloatLive {
    let mut n = XFloatLive::new();
    n.tick(at_ns, inp);
    n
}

#[test]
fn a_whole_day_sends_one_notice_at_the_research_instant() {
    let mut n = started(at(3, 0, 0));
    assert_eq!(n.take_facts_request(), None, "not before 03:30");
    n.tick(at(3, 30, 0), inp);
    assert_eq!(n.take_facts_request(), Some(day()));
    n.set_facts(table());
    let out = run(&mut n, "LOWF", at(8, 59, 59) + logic::DECIDE_NS);
    assert_eq!(out.len(), 1, "{out:?}");
    let x = &out[0];
    assert_eq!(x.kind, NoticeKind::XFloat);
    assert_eq!(x.at_ns, at(9, 1, 15) + logic::DECIDE_NS);
    assert_eq!(x.price, 2.3);
    assert_eq!(x.exec_ns, Some(at(9, 1, 5)));
    assert_eq!(x.alert_id, "2026-11-18-LOWF-xfloat");
    let Some(NoticeDetail::XFloat(d)) = &x.detail else {
        panic!("detail")
    };
    assert_eq!(d.shares, 301_000);
    assert!((d.rotation - 0.301).abs() < 1e-12);
    assert!((d.gain - 0.15).abs() < 1e-9);
    assert_eq!(d.prior_high, 2.1);
    assert_eq!(d.arrival_ns, Some(at(9, 1, 5) + 7_000_000));
    assert_eq!(d.float_source, "fmp");
    assert_eq!(n.fired().len(), 1);
    assert_eq!(n.cost()["notified"], 1);
}

#[test]
fn a_day_joined_late_or_without_its_types_decides_but_sends_nothing() {
    let mut n = started(at(8, 0, 0));
    n.set_facts(table());
    assert!(!n.complete());
    assert!(run(&mut n, "LOWF", at(8, 59, 59)).is_empty());
    assert_eq!(n.fired().len(), 1, "still decided, for the counts");
    assert!(!n.fired()[0].notified);

    let mut n = started(at(3, 0, 0));
    assert!(run(&mut n, "LOWF", at(8, 59, 59)).is_empty(), "no types");
    assert!(!n.complete());
}

#[test]
fn an_excluded_type_no_float_or_a_big_float_never_fires() {
    let mut n = started(at(3, 0, 0));
    n.set_facts(table());
    assert!(run(&mut n, "ETFX", at(8, 59, 59)).is_empty());
    let mut n = started(at(3, 0, 0));
    n.set_facts(table());
    let mut out = Vec::new();
    for (sym, f) in [("NOFL", None), ("BIGF", Some(60_000_000u64))] {
        n.apply_trade(&tr(sym, at(9, 0, 0), 2_100_000, 1_000), BOTH);
        n.apply_trade(&tr(sym, at(9, 1, 5), 2_300_000, 30_000_000), BOTH);
        n.tick(at(9, 2, 0), |_| inputs(f, Some(2.0)));
        out.extend(n.take_notices());
    }
    assert!(out.is_empty());
    assert!(n.complete());
}

#[test]
fn live_inputs_qualify_like_the_research() {
    assert_eq!(inputs(Some(9_999), Some(1.0)).float, None);
    assert_eq!(inputs(Some(50_000_000), Some(1.0)).float, Some(50e6));
    assert_eq!(inputs(Some(50_000_001), Some(1.0)).float, None);
    assert_eq!(inputs(Some(1_000_000), Some(0.0)).c1_u, None);
    assert_eq!(inputs(Some(1_000_000), Some(1.5)).c1_u, Some(1_500_000));
}

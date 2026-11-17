//! The live wrapper around [`super::logic`]: the day, its facts, completeness and the notices.
//! The trigger itself is tested in `logic` and, against the research's own streams, by the
//! parity harness.

use super::*;
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

fn tr(sym: &str, ts: i64, micros: i64, size: u64) -> Trade {
    Trade::new(sym, UnixNanos(ts), Price::from_micros(micros), size, 1)
}

fn table(syms: &[&str]) -> FactsTable {
    FactsTable {
        date: Some(day()),
        facts: syms
            .iter()
            .map(|s| {
                (
                    s.to_string(),
                    Facts {
                        c1: Some(2.0),
                        dadv20: None,
                    },
                )
            })
            .collect(),
        types: [("ETFX".to_string(), SecType::Excluded)].into(),
        ..Default::default()
    }
}

/// A quiet $2.00 print at 09:00, then a 3-minute climb to about $2.14 ending at `end`, ticking
/// the detector every second from `from` (the market's rocket task) and returning its notices.
fn run(n: &mut N1Live, sym: &str, end: i64) -> Vec<Notice> {
    let mut out = Vec::new();
    let mut prints: Vec<(i64, i64)> = vec![(at(9, 0, 0), 2_000_000)];
    for i in 0..12 {
        let slot_start = end - (12 - i) * logic::SLOT;
        for j in 0..3 {
            prints.push((slot_start + j * S, 2_000_000 + i * 12_727));
        }
    }
    let mut k = 0;
    let mut now = at(8, 59, 59);
    while now <= end + 5 * S {
        while k < prints.len() && prints[k].0 < now {
            n.apply_trade(&tr(sym, prints[k].0, prints[k].1, 1_000), BOTH);
            k += 1;
        }
        n.tick(now);
        out.extend(n.take_notices());
        now += S;
    }
    out
}

fn started(at_ns: i64) -> N1Live {
    let mut n = N1Live::new();
    n.tick(at_ns);
    n
}

#[test]
fn a_whole_day_with_its_facts_sends_one_labelled_notice_at_the_research_instant() {
    let mut n = started(at(3, 0, 0));
    n.tick(at(3, 30, 0));
    assert_eq!(n.take_facts_request(), Some(day()), "asked at 03:30 ET");
    assert_eq!(n.take_facts_request(), None, "once");
    n.set_facts(table(&["ABCD"]));
    let end = at(9, 31, 0);
    let got = run(&mut n, "ABCD", end);
    assert_eq!(got.len(), 1, "{got:?}");
    let x = &got[0];
    assert_eq!(x.kind, NoticeKind::N1);
    // The first bar that passes (the 11th of the climb is +6.4 %), decided 2 s after its end.
    let t = end - logic::SLOT + logic::SETTLE;
    assert_eq!(x.at_ns, t, "the bar end + 2 s");
    assert_eq!(x.alert_id, format!("2026-11-18-ABCD-n1-{}", t / S));
    let Some(NoticeDetail::N1(d)) = &x.detail else {
        panic!("{x:?}")
    };
    assert!(
        d.gain >= logic::GAIN_MIN && d.emitted_ns >= x.at_ns,
        "{d:?}"
    );
    let (title, body, data) = crate::alerts::runners::message(x);
    assert!(title.starts_with("ABCD · N1 · shadow · $"), "{title}");
    assert!(body.contains("research shadow"), "{body}");
    assert_eq!(data["kind"], "shadow.n1");
    assert_eq!(data["shadow"], true);
    assert_eq!(n.fired().len(), 1);
    let f = n.frame().unwrap();
    assert_eq!(f["n1"]["label"], "N1 · shadow");
    assert_eq!(f["n1"]["rows"][0]["s"], "ABCD");
}

#[test]
fn a_day_not_watched_from_four_or_without_facts_in_time_sends_nothing() {
    // Started mid-morning: decided and shown on the board as not notified, never sent.
    let mut n = started(at(9, 0, 0));
    n.set_facts(table(&["ABCD"]));
    assert!(run(&mut n, "ABCD", at(9, 31, 0)).is_empty());
    assert!(!n.complete());
    assert_eq!(n.fired().len(), 1);
    assert!(!n.fired()[0].notified);
    assert_eq!(n.frame().unwrap()["n1"]["rows"][0]["notified"], false);
    // Watched whole, but the facts never came: incomplete from the first bar due.
    let mut n = started(at(3, 0, 0));
    assert!(run(&mut n, "ABCD", at(9, 31, 0)).is_empty());
    assert!(!n.complete());
}

#[test]
fn excluded_types_and_symbols_without_a_previous_close_hold_no_state() {
    let mut n = started(at(3, 0, 0));
    n.set_facts(table(&["ABCD"]));
    for s in ["ETFX", "NOPC"] {
        n.apply_trade(&tr(s, at(9, 0, 0), 2_000_000, 100), BOTH);
    }
    assert_eq!(n.cost()["symbols"], 0);
    n.apply_trade(&tr("ABCD", at(9, 0, 0), 2_000_000, 100), BOTH);
    assert_eq!(n.cost()["symbols"], 1);
}

#[test]
fn the_next_day_starts_clean_and_asks_for_its_own_facts() {
    let mut n = started(at(3, 0, 0));
    n.set_facts(table(&["ABCD"]));
    assert_eq!(run(&mut n, "ABCD", at(9, 31, 0)).len(), 1);
    // 20:00 ET is the next trading day's overnight.
    n.tick(at(20, 0, 1));
    assert!(n.fired().is_empty() && n.frame().is_none());
    assert_eq!(
        n.take_facts_request(),
        None,
        "not before 03:30 of the new day"
    );
    let next = NaiveDate::from_ymd_opt(2026, 11, 19).unwrap();
    n.tick(et(next, 3, 30, 0, 0).unwrap().0);
    assert_eq!(n.take_facts_request(), Some(next));
    assert!(
        n.complete() || n.facts.is_none(),
        "watched since before its 04:00"
    );
}

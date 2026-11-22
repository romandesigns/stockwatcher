//! The live wrapper around [`super::logic`]: the day, its table, completeness, the notices and
//! the ToD file. The triggers themselves are tested in `logic` and, against the research's own
//! rows, by the parity harness (`50-ship/gap-ctx/PARITY.md`).

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

/// GAPR: C1 $1.00, a flat $1,000-a-bucket baseline. DAY2: ran yesterday, H1 $3.00.
fn table() -> Table {
    let base = logic::Baseline {
        present20: 20,
        present5: 5,
        buckets: (0..logic::NBUCKETS as u8)
            .map(|b| (b, (1_000.0, 1_000.0)))
            .collect(),
    };
    let gapr = Facts {
        c1: Some(1.0),
        h1: Some(1.05),
        prev_ran: false,
        prev_run: None,
    };
    let day2 = Facts {
        c1: Some(2.5),
        h1: Some(3.0),
        prev_ran: true,
        prev_run: Some(0.45),
    };
    Table {
        date: Some(day()),
        syms: [
            ("GAPR".to_string(), SymDay::new(&gapr, base.clone())),
            ("DAY2".to_string(), SymDay::new(&day2, base.clone())),
            ("ETFX".to_string(), SymDay::new(&gapr, base)),
        ]
        .into(),
        facts: [("GAPR".to_string(), gapr), ("DAY2".to_string(), day2)].into(),
        types: [("ETFX".to_string(), SecType::Excluded)].into(),
        newest_raw: Some(previous_trading_day(day())),
        tod_sessions: 20,
        ctx_eligible: 1,
        ..Default::default()
    }
}

/// Ticks every second from 03:59:59 to `until`, feeding `prints` (ns, sym, micros, size) as the
/// clock passes them; returns the notices.
fn run(n: &mut GapCtxLive, prints: &[(i64, &str, i64, u64)], until: i64) -> Vec<Notice> {
    let mut out = Vec::new();
    let mut k = 0;
    let mut now = at(3, 59, 59);
    while now <= until {
        while k < prints.len() && prints[k].0 < now {
            let (ts, s, px, size) = prints[k];
            n.apply_trade(&tr(s, ts, px, size), BOTH);
            k += 1;
        }
        n.tick(now);
        out.extend(n.take_notices());
        now += S;
    }
    out
}

fn gapper(sym: &'static str) -> Vec<(i64, &'static str, i64, u64)> {
    // 04:10: $1.05 x 10k ($10.5k), then 04:20:05 $1.20 x 100k ($120k): +20 % gap, a new high,
    // $130k since 04:00, 120x the bucket's usual.
    vec![
        (at(4, 10, 1), sym, 1_050_000, 10_000),
        (at(4, 20, 5), sym, 1_200_000, 100_000),
    ]
}

#[test]
fn a_whole_day_with_its_table_sends_one_x_gap_notice_at_the_research_instant() {
    let mut n = GapCtxLive::new();
    n.tick(at(3, 0, 0));
    n.tick(at(3, 30, 0));
    assert_eq!(n.take_table_request(), Some(day()));
    n.set_table(table());
    let mut p = gapper("GAPR");
    p.extend(gapper("ETFX"));
    p.sort();
    let got = run(&mut n, &p, at(4, 30, 0));
    assert_eq!(got.len(), 1, "{got:?}");
    let x = &got[0];
    assert_eq!(x.kind, NoticeKind::XGap);
    assert_eq!(x.symbol, "GAPR");
    // The bar 04:20:00-04:20:15 decided at its end + 50 ms.
    assert_eq!(x.at_ns, at(4, 20, 15) + logic::DECIDE_NS);
    assert_eq!(x.price, 1.2);
    assert_eq!(x.alert_id, "2026-11-18-GAPR-xgap");
    let Some(NoticeDetail::GapCtx(d)) = &x.detail else {
        panic!("detail")
    };
    assert_eq!((d.detector, d.session), ("x-gap-1", "pre"));
    assert!((d.gap - 0.2).abs() < 1e-12);
    assert_eq!(d.level, 1.05);
    assert!(n.complete());
}

#[test]
fn x_ctx_fires_on_the_first_close_above_yesterdays_high_in_any_session() {
    let mut n = GapCtxLive::new();
    n.tick(at(3, 0, 0));
    n.tick(at(3, 30, 0));
    n.take_table_request();
    n.set_table(table());
    let p = vec![
        (at(10, 0, 1), "DAY2", 2_900_000, 50_000),
        (at(10, 1, 2), "DAY2", 3_010_000, 10_000),
        (at(10, 5, 0), "DAY2", 3_200_000, 10_000),
    ];
    let got = run(&mut n, &p, at(10, 10, 0));
    assert_eq!(got.len(), 1, "{got:?}");
    assert_eq!(got[0].kind, NoticeKind::XCtx);
    assert_eq!(got[0].at_ns, at(10, 1, 15) + logic::DECIDE_NS);
    let Some(NoticeDetail::GapCtx(d)) = &got[0].detail else {
        panic!()
    };
    assert_eq!(
        (d.detector, d.session, d.level),
        ("x-ctx-1", "regular", 3.0)
    );
}

#[test]
fn an_incomplete_day_decides_but_sends_nothing() {
    // Started after 04:00.
    let mut n = GapCtxLive::new();
    n.tick(at(4, 5, 0));
    n.take_table_request();
    n.set_table(table());
    assert!(run(&mut n, &gapper("GAPR"), at(4, 30, 0)).is_empty());
    assert_eq!(n.fired().len(), 1, "it still decided");
    // Stale facts (yesterday's import missing).
    for t in [
        Table {
            stale: true,
            ..table()
        },
        Table {
            tod_sessions: TOD_MIN_SESSIONS - 1,
            ..table()
        },
    ] {
        let mut n = GapCtxLive::new();
        n.tick(at(3, 0, 0));
        n.set_table(t);
        assert!(run(&mut n, &gapper("GAPR"), at(4, 30, 0)).is_empty());
    }
    // No table at all by the first bar.
    let mut n = GapCtxLive::new();
    n.tick(at(3, 0, 0));
    assert!(run(&mut n, &gapper("GAPR"), at(4, 30, 0)).is_empty());
    n.set_table(table());
    assert!(!n.complete(), "late facts keep the day incomplete");
}

#[test]
fn a_print_after_its_bar_was_decided_counts_for_later_bars_only() {
    let mut n = GapCtxLive::new();
    n.tick(at(3, 0, 0));
    n.set_table(table());
    let _ = run(
        &mut n,
        &[(at(4, 10, 1), "GAPR", 1_050_000, 10_000)],
        at(4, 11, 0),
    );
    // A print of the 04:10:00 bar arriving at 04:11.
    n.apply_trade(&tr("GAPR", at(4, 10, 2), 1_300_000, 100_000), BOTH);
    assert_eq!(n.cost()["late_prints"], 1);
    // Its $130k counts: the next new high on any volume passes $100k.
    let got = run(
        &mut n,
        &[(at(4, 12, 1), "GAPR", 1_100_000, 1_000)],
        at(4, 13, 0),
    );
    assert_eq!(got.len(), 1);
    assert_eq!(
        got[0].price, 1.1,
        "the late $1.30 never became the bar's high"
    );
}

#[test]
fn the_tod_file_is_cut_after_20_00_with_the_archive_membership_and_reads_back() {
    let mut n = GapCtxLive::new();
    n.tick(at(3, 0, 0));
    n.set_table(table());
    let p = [
        (at(4, 0, 1), "GAPR", 1_000_000, 100),
        (at(4, 0, 2), "GAPR", 1_000_000, 300),
        (at(9, 31, 0), "GAPR", 1_100_000, 1_000),
        // Over $30 at the day's last print: not in the research archive that day.
        (at(9, 31, 0), "BIGP", 45_000_000, 10),
        (at(9, 31, 0), "ETFX", 2_000_000, 10),
    ];
    let mut k = 0;
    for (now, _) in [(at(9, 32, 0), 0), (at(20, 0, 10), 0)] {
        while k < p.len() && p[k].0 < now {
            n.apply_trade(&tr(p[k].1, p[k].0, p[k].2, p[k].3), BOTH);
            k += 1;
        }
        n.tick(now);
    }
    let (date, v) = n.take_tod().expect("cut");
    assert_eq!(date, day());
    assert!(n.take_tod().is_none(), "once");
    assert_eq!(v["present"], json!(["GAPR"]));
    assert_eq!(
        v["agg"]["GAPR"],
        json!([[0, 2, 400, 400.0], [66, 1, 1000, 1100.0]])
    );
    let dir = tempfile::tempdir().unwrap();
    write_tod(dir.path(), date, &v).unwrap();
    let back = read_tod(dir.path(), date).unwrap();
    assert!(back.present.contains("GAPR") && back.present.len() == 1);
    assert_eq!(back.agg["GAPR"], vec![(0, 400.0), (66, 1100.0)]);
    assert!(read_tod(dir.path(), day().succ_opt().unwrap()).is_none());
}

#[test]
fn a_day_not_watched_whole_writes_no_tod_file() {
    let mut n = GapCtxLive::new();
    n.tick(at(9, 0, 0));
    n.apply_trade(&tr("GAPR", at(9, 0, 1), 1_000_000, 100), BOTH);
    n.tick(at(20, 0, 10));
    assert!(n.take_tod().is_none());
}

#[test]
fn each_kind_names_its_detector() {
    // The switch itself is parsed once for every item (`crate::alerts::detectors`).
    assert_eq!(detector_of(NoticeKind::XGap), Some("x-gap-1"));
    assert_eq!(detector_of(NoticeKind::XCtx), Some("x-ctx-1"));
    assert_eq!(detector_of(NoticeKind::N1), None);
}

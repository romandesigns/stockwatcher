//! The live wrapper around [`super::logic`]: the day, its table, completeness, the clock-decided
//! holds, the notices and the presence file. The triggers themselves are tested in `logic` and,
//! against the research's own rows, by the parity harness (`50-ship/rw2/PARITY.md`).

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

/// VWRC: C1 $1.50 (vwapr). LVL: C1 $1.00, HI20 $1.20 (level). ETFX: an excluded type. A flat
/// $1,000-a-bucket baseline for each.
fn table() -> Table {
    let sd = |c1: f64, hi20: Option<f64>| SymDay {
        c1: Some(c1),
        hi20_u: hi20.map(|h| h * 1e6),
        base: vec![1_000.0; logic::NBUCKETS],
    };
    Table {
        date: Some(day()),
        syms: [
            ("VWRC".to_string(), sd(1.5, None)),
            ("LVL".to_string(), sd(1.0, Some(1.2))),
            ("ETFX".to_string(), sd(1.0, Some(1.2))),
        ]
        .into(),
        types: [("ETFX".to_string(), SecType::Excluded)].into(),
        newest_raw: Some(previous_trading_day(day())),
        tod_sessions: 20,
        level_eligible: 2,
        ..Default::default()
    }
}

/// Ticks every second from 03:59:59 to `until`, feeding `prints` (ns, sym, micros, size) as the
/// clock passes them; returns the notices.
fn run(n: &mut Rw2Live, prints: &[(i64, &str, i64, u64)], until: i64) -> Vec<Notice> {
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

/// 10:00:00 slot is 1440. A reclaim day for `sym` at $2: above, a 3 % flush, the reclaim on
/// rising volume at 10:00:45 (slot 1443), one held bar at 10:01:00 and none at 10:01:15, so the
/// hold is decided by the clock at 10:01:30.05.
fn reclaimer(sym: &'static str) -> Vec<(i64, &'static str, i64, u64)> {
    vec![
        (at(10, 0, 1), sym, 2_000_000, 150_000),
        (at(10, 0, 16), sym, 2_010_000, 1_000),
        (at(10, 0, 31), sym, 1_930_000, 1_000),
        (at(10, 0, 32), sym, 1_990_000, 1_000),
        (at(10, 0, 46), sym, 2_050_000, 20_000),
        (at(10, 1, 1), sym, 2_055_000, 1_000),
    ]
}

/// LVL crosses $1.20 at 11:00:00 (slot 1680) with $150k and holds 4 bars.
fn breaker(sym: &'static str) -> Vec<(i64, &'static str, i64, u64)> {
    let mut v = vec![(at(10, 59, 46), sym, 1_150_000, 1_000)];
    v.push((at(11, 0, 1), sym, 1_250_000, 120_000));
    for k in 1..=4 {
        v.push((at(11, 0, 1) + k * 15 * S, sym, 1_260_000, 100));
    }
    v
}

#[test]
fn a_whole_day_sends_the_vwap_reclaim_at_the_holds_end_and_the_level_break() {
    let mut n = Rw2Live::new();
    n.tick(at(3, 0, 0));
    n.tick(at(3, 30, 0));
    assert_eq!(n.take_table_request(), Some(day()));
    n.set_table(table());
    let mut p = reclaimer("VWRC");
    p.extend(breaker("LVL"));
    p.extend(breaker("ETFX"));
    p.sort();
    let got = run(&mut n, &p, at(11, 5, 0));
    assert_eq!(got.len(), 2, "{got:?}");
    let v = &got[0];
    assert_eq!(
        (v.kind, v.symbol.as_str()),
        (NoticeKind::VwapReclaim, "VWRC")
    );
    // Reclaim slot 1443 + 2 = 1445, decided at 10:01:30.05 although no bar printed there.
    assert_eq!(v.at_ns, at(10, 1, 30) + logic::DECIDE_NS);
    assert_eq!(v.price, 2.055);
    assert_eq!(v.alert_id, "2026-11-18-VWRC-vwapr-r");
    assert_eq!(v.exec_ns, Some(at(10, 1, 1)), "the held bar's close print");
    let Some(NoticeDetail::Rw2(d)) = &v.detail else {
        panic!("detail")
    };
    assert_eq!((d.detector, d.session), ("rw2-vwapr-1", "regular"));
    assert_eq!(d.trigger_ns, at(10, 1, 0));
    let l = &got[1];
    assert_eq!((l.kind, l.symbol.as_str()), (NoticeKind::LevelBreak, "LVL"));
    assert_eq!(l.at_ns, at(11, 1, 15) + logic::DECIDE_NS);
    assert_eq!(l.alert_id, "2026-11-18-LVL-level");
    assert!(n.complete());
    assert_eq!(detector_of(NoticeKind::LevelBreak), Some("rw2-level-1"));
}

#[test]
fn a_day_joined_late_decides_but_sends_nothing() {
    let mut n = Rw2Live::new();
    n.tick(at(9, 0, 0));
    assert_eq!(n.take_table_request(), Some(day()));
    n.set_table(table());
    let got = run(&mut n, &reclaimer("VWRC"), at(10, 5, 0));
    assert!(got.is_empty());
    assert!(!n.complete());
    assert_eq!(n.fired().len(), 1);
    assert!(!n.fired()[0].notified);
}

#[test]
fn too_few_tod_sessions_or_a_stale_table_send_nothing() {
    for t in [
        Table {
            tod_sessions: 4,
            ..table()
        },
        Table {
            stale: true,
            ..table()
        },
    ] {
        let mut n = Rw2Live::new();
        n.tick(at(3, 30, 0));
        n.set_table(t);
        assert!(run(&mut n, &reclaimer("VWRC"), at(10, 5, 0)).is_empty());
    }
}

#[test]
fn the_presence_file_lists_the_symbols_that_closed_in_band() {
    let mut n = Rw2Live::new();
    n.tick(at(3, 30, 0));
    n.set_table(table());
    let mut p = reclaimer("VWRC");
    p.extend(breaker("ETFX"));
    p.push((at(12, 0, 1), "HIGH", 30_000_000, 100));
    p.sort();
    run(&mut n, &p, at(12, 1, 0));
    assert!(n.take_member().is_none());
    // 20:00: the trading date moves on.
    n.tick(at(20, 0, 1));
    let (d, v) = n.take_member().expect("cut at 20:00");
    assert_eq!(d, day());
    assert_eq!(
        v["kept"],
        json!(["VWRC"]),
        "excluded and out-of-band symbols left out"
    );
    let dir = std::env::temp_dir().join(format!("rw2-member-{}", std::process::id()));
    write_member(&dir, d, &v).unwrap();
    assert_eq!(
        read_member(&dir, d),
        Some(["VWRC".to_string()].into_iter().collect())
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_logic_cache_dollars_and_factor_match_gap_ctx() {
    for (c, v, d) in [
        (2_000_000, 1_000, 2_000.04f32),
        (0, 100, 150.0),
        (1_234_567, 777, 959.1),
    ] {
        assert_eq!(
            logic::cache_dollars(c, v, d),
            crate::gap_ctx::logic::cache_dollars(c, v, d)
        );
    }
    let sp = [(3, 10.0), (7, 0.5)];
    assert_eq!(
        logic::factor(&sp, 1, 9),
        crate::gap_ctx::logic::factor(&sp, 1, 9)
    );
}

/// The production race (2026-11-23..25): asked for in the pass that already had the first bar
/// due. The bars and holds wait for the table and are decided in order once it lands; the day's
/// fires are the same as with the table in time.
#[test]
fn a_detector_started_before_its_table_waits_and_fires_once_the_table_lands() {
    let mut p = reclaimer("VWRC");
    p.extend(breaker("LVL"));
    p.sort();
    let mut early = Rw2Live::new();
    early.tick(at(3, 30, 0));
    early.take_table_request();
    early.set_table(table());
    let want = run(&mut early, &p, at(11, 5, 0));
    assert_eq!(want.len(), 2);
    // The race, the clock seen at 03:00 then at 10:00:16 with the 10:00:00 bar due.
    let mut n = Rw2Live::new();
    n.tick(at(3, 0, 0));
    assert_eq!(n.take_table_request(), None);
    n.apply_trade(&tr(p[0].1, p[0].0, p[0].2, p[0].3), BOTH);
    n.tick(at(10, 0, 16));
    assert_eq!(n.take_table_request(), Some(day()));
    assert_eq!(n.cost()["bars"], 0, "the due bar waits for the table");
    assert_eq!(n.readiness().state, crate::readiness::State::Waiting);
    // Feed the rest of the prints while still waiting; the table lands at 10:00:50.
    let mut got = Vec::new();
    let mut k = 1;
    let mut now = at(10, 0, 17);
    let landed = at(10, 0, 50);
    while now <= at(11, 5, 0) {
        while k < p.len() && p[k].0 < now {
            n.apply_trade(&tr(p[k].1, p[k].0, p[k].2, p[k].3), BOTH);
            k += 1;
        }
        if now == landed {
            n.set_table(table());
        }
        n.tick(now);
        got.extend(n.take_notices());
        now += S;
    }
    assert!(n.readiness().is_ready());
    assert_eq!(got.len(), 2, "{got:?}");
    for (g, w) in got.iter().zip(&want) {
        assert_eq!(
            (g.kind, g.symbol.as_str(), g.at_ns, g.price),
            (w.kind, w.symbol.as_str(), w.at_ns, w.price)
        );
    }
    assert_eq!(n.fired().len(), early.fired().len());
}

#[test]
fn readiness_reports_a_stale_table() {
    let mut n = Rw2Live::new();
    n.tick(at(3, 30, 0));
    n.set_table(Table {
        stale: true,
        ..table()
    });
    let r = n.readiness();
    assert_eq!(r.state, crate::readiness::State::Silent);
    assert!(r.why.unwrap().contains("not the previous trading day"));
    assert_eq!(n.cost()["readiness"]["state"], "silent");
}

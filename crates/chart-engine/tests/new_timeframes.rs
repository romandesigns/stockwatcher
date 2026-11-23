//! 10s and 1h (2026-11-22): bucketing, the 1h session clip and ET alignment across DST, the 1h
//! series being the exact merge of the 5m one under every revision, and 10s leaving the engine's
//! policies (settle window, retention, base candles) where they were.
mod common;

use chart_engine::wire::{CandleWire, ChartUpdate};
use chart_engine::{ChartEngine, EngineConfig, Timeframe};
use chrono::NaiveDate;
use common::*;
use market_events::session::et;

fn old_set() -> EngineConfig {
    EngineConfig {
        timeframes: vec![Timeframe::S15, Timeframe::S30, Timeframe::M1, Timeframe::M5],
        ..EngineConfig::default()
    }
}

#[test]
fn serde_names_and_parsing() {
    for (tf, name) in [(Timeframe::S10, "10s"), (Timeframe::H1, "1h")] {
        assert_eq!(serde_json::to_string(&tf).unwrap(), format!("\"{name}\""));
        assert_eq!(
            serde_json::from_str::<Timeframe>(&format!("\"{name}\"")).unwrap(),
            tf
        );
        assert_eq!(name.parse::<Timeframe>().unwrap(), tf);
        assert_eq!(tf.to_string(), name);
    }
    assert_eq!(
        Timeframe::ALL.map(|t| t.label()),
        ["10s", "15s", "30s", "1m", "5m", "1h"]
    );
    assert!("2h".parse::<Timeframe>().is_err());
    assert!(serde_json::from_str::<Timeframe>("\"4h\"").is_err());
}

#[test]
fn ten_second_boundaries() {
    let mut e = ChartEngine::default();
    apply_all(
        &mut e,
        &[
            ev(trade(at(10, 0, 9, 999), "1.00", 1, 1)),
            ev(trade(at(10, 0, 10, 0), "2.00", 1, 2)),
            ev(trade(at(10, 0, 19, 999), "3.00", 1, 3)),
            ev(trade(at(10, 0, 20, 0), "4.00", 1, 4)),
        ],
    );
    let cs = candles(&e, Timeframe::S10);
    assert_eq!(
        cs.iter().map(|c| (c.t, c.o, c.c)).collect::<Vec<_>>(),
        vec![
            (ms(at(10, 0, 0, 0)), 1.0, 1.0),
            (ms(at(10, 0, 10, 0)), 2.0, 3.0),
            (ms(at(10, 0, 20, 0)), 4.0, 4.0),
        ]
    );
    // The same prints on 15s: :00 and :15.
    assert_eq!(candles(&e, Timeframe::S15).len(), 2);
}

#[test]
fn ten_seconds_leaves_the_settle_window_on_the_15s_grid() {
    // 10:00:05 is in the 10:00:00 10s interval (ends :10) and the 15s one (ends :15). With the
    // watermark at :16 a print back at :05 is past 10s + 2 s but inside 15s + 2 s: accepted as
    // before 10s existed, and applied to every timeframe, 10s included.
    for cfg in [EngineConfig::default(), old_set()] {
        let mut e = ChartEngine::new(cfg).unwrap();
        e.apply(&ev(trade(at(10, 0, 16, 0), "3.00", 100, 1)));
        let ups = e.apply(&ev(trade(at(10, 0, 5, 0), "2.90", 100, 2)));
        assert_eq!(tape_counted(&ups), vec!["ohlc_and_volume"]);
        assert_eq!(e.quality(SYM)[0].1.late_applied, 1);
        let late = e.apply(&ev(trade(at(10, 0, 5, 0), "2.80", 100, 3)));
        assert_eq!(tape_counted(&late), vec!["ohlc_and_volume"]);
    }
    let mut e = ChartEngine::default();
    e.apply(&ev(trade(at(10, 0, 18, 0), "3.00", 100, 1)));
    let ups = e.apply(&ev(trade(at(10, 0, 5, 0), "2.90", 100, 2)));
    assert_eq!(tape_counted(&ups), vec!["excluded:late"]);
}

#[test]
fn ten_second_ring_holds_a_whole_day() {
    let cfg = EngineConfig::default();
    assert!(cfg.ring_cap(Timeframe::S10) >= 16 * 360);
    assert_eq!(
        cfg.ring_cap(Timeframe::S15),
        cfg.max_intervals_per_timeframe
    );
}

#[test]
fn hour_buckets_are_et_hours_clipped_at_the_open() {
    let mut e = ChartEngine::default();
    let prints = [
        (at(8, 15, 0, 0), "1.00", 10),
        (at(9, 10, 0, 0), "1.10", 20),
        (at(9, 29, 59, 999), "1.20", 30),
        (at(9, 30, 0, 0), "1.30", 40),
        (at(9, 59, 0, 0), "1.25", 50),
        (at(10, 0, 0, 0), "1.40", 60),
        (at(10, 59, 59, 999), "1.35", 70),
        (at(16, 0, 0, 0), "1.50", 80),
    ];
    let evs: Vec<_> = prints
        .iter()
        .enumerate()
        .map(|(i, (t, p, s))| ev(trade(*t, p, *s, i as u64 + 1)))
        .collect();
    apply_all(&mut e, &evs);
    let cs = candles(&e, Timeframe::H1);
    assert_eq!(
        cs.iter().map(|c| (c.t, c.o, c.c, c.v)).collect::<Vec<_>>(),
        vec![
            (ms(at(8, 0, 0, 0)), 1.00, 1.00, 10),
            (ms(at(9, 0, 0, 0)), 1.10, 1.20, 50),
            (ms(at(9, 30, 0, 0)), 1.30, 1.25, 90),
            (ms(at(10, 0, 0, 0)), 1.40, 1.35, 130),
            (ms(at(16, 0, 0, 0)), 1.50, 1.50, 80),
        ]
    );
    // Regular hours only: the half-hour from the open, then whole hours. 16:00 is after hours.
    let rth = e.chart_candles(SYM, Timeframe::H1, true).unwrap();
    assert_eq!(
        rth.iter().map(|c| c.t).collect::<Vec<_>>(),
        vec![ms(at(9, 30, 0, 0)), ms(at(10, 0, 0, 0))]
    );
    // The bounds the clip gives.
    let h = Timeframe::H1;
    assert_eq!(h.start_of(at(9, 45, 0, 0)), at(9, 30, 0, 0));
    assert_eq!(h.end_of(at(9, 45, 0, 0)), at(10, 0, 0, 0));
    assert_eq!(h.start_of(at(9, 15, 0, 0)), at(9, 0, 0, 0));
    assert_eq!(h.end_of(at(9, 15, 0, 0)), at(9, 30, 0, 0));
    assert_eq!(h.start_of(at(10, 30, 0, 0)), at(10, 0, 0, 0));
    assert_eq!(h.end_of(at(10, 30, 0, 0)), at(11, 0, 0, 0));
}

#[test]
fn hour_buckets_stay_on_the_et_hour_across_dst() {
    let d = |y, m, day| NaiveDate::from_ymd_opt(y, m, day).unwrap();
    // Fall back on Sunday 2026-11-01, spring forward on Sunday 2026-03-08.
    for date in [
        d(2026, 10, 30),
        d(2026, 11, 2),
        d(2026, 3, 6),
        d(2026, 3, 9),
    ] {
        let t = |h, m| et(date, h, m, 0, 0).unwrap();
        let h1 = Timeframe::H1;
        assert_eq!(h1.start_of(t(4, 20)), t(4, 0), "{date}");
        assert_eq!(h1.start_of(t(9, 5)), t(9, 0), "{date}");
        assert_eq!(h1.end_of(t(9, 5)), t(9, 30), "{date}");
        assert_eq!(h1.start_of(t(9, 31)), t(9, 30), "{date}");
        assert_eq!(h1.end_of(t(9, 31)), t(10, 0), "{date}");
        assert_eq!(h1.start_of(t(13, 59)), t(13, 0), "{date}");
        assert_eq!(h1.end_of(t(19, 59)), t(20, 0), "{date}");
    }
    // Candles of a post-change Monday carry the ET hour.
    let date = d(2026, 11, 2);
    let t = |h, m| et(date, h, m, 0, 0).unwrap();
    let mut e = ChartEngine::default();
    let mk = |ts, seq| {
        ev(market_events::Trade::new(SYM, ts, px("2.00"), 10, seq)
            .with_exchange("V")
            .with_tape(market_events::Tape::C)
            .with_source(market_events::Source::Fixture))
    };
    apply_all(&mut e, &[mk(t(11, 10), 1), mk(t(12, 50), 2)]);
    assert_eq!(
        candles(&e, Timeframe::H1)
            .iter()
            .map(|c| c.t)
            .collect::<Vec<_>>(),
        vec![ms(t(11, 0)), ms(t(12, 0))]
    );
}

/// The 1h candles a merge of the engine's own 5m candles gives.
fn merged_from_5m(e: &ChartEngine) -> Vec<(i64, f64, f64, f64, f64, u64, u64)> {
    let mut out: Vec<(i64, f64, f64, f64, f64, u64, u64)> = Vec::new();
    for c in candles(e, Timeframe::M5) {
        let start = ms(Timeframe::H1.start_of(market_events::UnixNanos::from_millis(c.t)));
        match out.last_mut() {
            Some(m) if m.0 == start => {
                m.2 = m.2.max(c.h);
                m.3 = m.3.min(c.l);
                m.4 = c.c;
                m.5 += c.v;
                m.6 += c.n;
            }
            _ => out.push((start, c.o, c.h, c.l, c.c, c.v, c.n)),
        }
    }
    out
}

fn tuple(c: &CandleWire) -> (i64, f64, f64, f64, f64, u64, u64) {
    (c.t, c.o, c.h, c.l, c.c, c.v, c.n)
}

#[test]
fn hour_series_is_the_exact_merge_of_5m_under_revisions() {
    let mut e = ChartEngine::default();
    let mut ups = apply_all(
        &mut e,
        &[
            ev(trade(at(10, 1, 0, 0), "3.00", 100, 1)),
            ev(trade(at(10, 7, 0, 0), "3.40", 100, 2)),
            ev(trade(at(10, 6, 0, 0), "2.90", 100, 3)), // out of order
            ev(trade(at(10, 8, 0, 0), "3.10", 100, 4)),
        ],
    );
    assert_eq!(
        candles(&e, Timeframe::H1)
            .iter()
            .map(tuple)
            .collect::<Vec<_>>(),
        merged_from_5m(&e)
    );
    // A correction of the 10:07 high, then a cancel of 11:00's only print (both inside the
    // trade retention).
    ups.extend(e.apply(&correction(at(10, 8, 30, 0), 2, 9, "3.05", 100, &["@"])));
    ups.extend(e.apply(&ev(trade(at(11, 0, 1, 0), "3.20", 50, 5))));
    assert_eq!(candles(&e, Timeframe::H1).len(), 2);
    ups.extend(e.apply(&cancel(at(11, 0, 3, 0), 5)));
    let h1 = candles(&e, Timeframe::H1);
    assert_eq!(h1.iter().map(tuple).collect::<Vec<_>>(), merged_from_5m(&e));
    assert_eq!(h1.len(), 1);
    assert_eq!(h1[0].h, 3.10, "the corrected high is gone");
    // The emptied hour was removed from clients too.
    assert!(ups.iter().any(|u| matches!(u, ChartUpdate::Remove(r)
        if r.timeframe == Timeframe::H1 && r.t == ms(at(11, 0, 0, 0)))));
    // Incremental 1h updates, upserted by `t`, give the snapshot.
    let mut live: Vec<CandleWire> = Vec::new();
    for u in &ups {
        match u {
            ChartUpdate::Candle(c) if c.timeframe == Timeframe::H1 => {
                live.retain(|x| x.t != c.candle.t);
                live.push(c.candle.clone());
            }
            ChartUpdate::Remove(r) if r.timeframe == Timeframe::H1 => {
                live.retain(|x| x.t != r.t);
            }
            _ => {}
        }
    }
    live.sort_by_key(|c| c.t);
    assert_eq!(
        live.iter().map(tuple).collect::<Vec<_>>(),
        h1.iter().map(tuple).collect::<Vec<_>>()
    );
}

#[test]
fn hour_finalizes_by_the_clock() {
    let mut e = ChartEngine::default();
    e.apply(&ev(trade(at(10, 10, 0, 0), "3.00", 100, 1)));
    assert!(!candles(&e, Timeframe::H1)[0].is_final);
    let ups = e.advance_clock(at(11, 0, 0, 0));
    assert!(ups.iter().any(|u| {
        u.as_candle()
            .is_some_and(|c| c.timeframe == Timeframe::H1 && c.candle.is_final)
    }));
}

#[test]
fn new_timeframes_keep_retention_and_base_candles() {
    let (mut new, mut old) = (ChartEngine::default(), ChartEngine::new(old_set()).unwrap());
    for i in 0..3600i64 {
        let t = ev(trade(
            at(10, 0, 0, 0).add_nanos(i * 1_000_000_000),
            "1.00",
            1,
            i as u64 + 1,
        ));
        new.apply(&t);
        old.apply(&t);
    }
    // 1h is derived, so trade retention still aligns to 5m, not to the hour.
    assert_eq!(new.retained_trades(SYM), old.retained_trades(SYM));
    for tf in [Timeframe::S15, Timeframe::S30, Timeframe::M1, Timeframe::M5] {
        assert_eq!(candles(&new, tf), candles(&old, tf), "{tf}");
    }
}

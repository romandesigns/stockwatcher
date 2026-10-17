//! OHLCV per timeframe, exact boundaries, forming → final, gaps, timeframe switching, bounds.
mod common;

use chart_engine::wire::{ChartUpdate, GapReason};
use chart_engine::{ChartEngine, EngineConfig, Timeframe};
use common::*;
use market_events::UnixNanos;

fn s(v: &str) -> String {
    v.to_string()
}

fn base_fixture() -> Vec<market_events::MarketEvent> {
    vec![
        ev(trade(at(10, 0, 0, 0), "3.18", 100, 1)),
        ev(trade(at(10, 0, 7, 500), "3.25", 200, 2)),
        ev(trade(at(10, 0, 14, 999), "3.10", 50, 3)),
        ev(trade(at(10, 0, 15, 0), "3.20", 100, 4)),
        ev(trade(at(10, 0, 44, 0), "3.30", 300, 5)),
        ev(trade(at(10, 1, 0, 0), "3.05", 100, 6)),
        ev(trade(at(10, 4, 59, 999), "3.40", 10, 7)),
    ]
}

#[test]
fn ohlcv_for_every_timeframe() {
    let mut e = ChartEngine::default();
    apply_all(&mut e, &base_fixture());
    let get = |tf| candles(&e, tf).iter().map(ohlcv).collect::<Vec<_>>();
    assert_eq!(
        get(Timeframe::S15),
        vec![
            (
                ms(at(10, 0, 0, 0)),
                s("3.18"),
                s("3.25"),
                s("3.10"),
                s("3.10"),
                350
            ),
            (
                ms(at(10, 0, 15, 0)),
                s("3.20"),
                s("3.20"),
                s("3.20"),
                s("3.20"),
                100
            ),
            (
                ms(at(10, 0, 30, 0)),
                s("3.30"),
                s("3.30"),
                s("3.30"),
                s("3.30"),
                300
            ),
            (
                ms(at(10, 1, 0, 0)),
                s("3.05"),
                s("3.05"),
                s("3.05"),
                s("3.05"),
                100
            ),
            (
                ms(at(10, 4, 45, 0)),
                s("3.40"),
                s("3.40"),
                s("3.40"),
                s("3.40"),
                10
            ),
        ]
    );
    assert_eq!(
        get(Timeframe::S30),
        vec![
            (
                ms(at(10, 0, 0, 0)),
                s("3.18"),
                s("3.25"),
                s("3.10"),
                s("3.20"),
                450
            ),
            (
                ms(at(10, 0, 30, 0)),
                s("3.30"),
                s("3.30"),
                s("3.30"),
                s("3.30"),
                300
            ),
            (
                ms(at(10, 1, 0, 0)),
                s("3.05"),
                s("3.05"),
                s("3.05"),
                s("3.05"),
                100
            ),
            (
                ms(at(10, 4, 30, 0)),
                s("3.40"),
                s("3.40"),
                s("3.40"),
                s("3.40"),
                10
            ),
        ]
    );
    assert_eq!(
        get(Timeframe::M1),
        vec![
            (
                ms(at(10, 0, 0, 0)),
                s("3.18"),
                s("3.30"),
                s("3.10"),
                s("3.30"),
                750
            ),
            (
                ms(at(10, 1, 0, 0)),
                s("3.05"),
                s("3.05"),
                s("3.05"),
                s("3.05"),
                100
            ),
            (
                ms(at(10, 4, 0, 0)),
                s("3.40"),
                s("3.40"),
                s("3.40"),
                s("3.40"),
                10
            ),
        ]
    );
    assert_eq!(
        get(Timeframe::M5),
        vec![(
            ms(at(10, 0, 0, 0)),
            s("3.18"),
            s("3.40"),
            s("3.05"),
            s("3.40"),
            860
        )]
    );
    // Wicks: the 1m 10:00 candle's body is 3.18→3.30 with a lower wick to 3.10.
    let c = &candles(&e, Timeframe::M1)[0];
    assert!(c.l < c.o.min(c.c) && c.h == c.o.max(c.c));
    assert_eq!(c.n, 5);
}

#[test]
fn exact_boundaries_for_every_timeframe() {
    for tf in Timeframe::ALL {
        let boundary = at(10, 5, 0, 0); // a multiple of every timeframe
        for (before, label) in [
            (boundary.add_nanos(-1_000_000), "-1ms"),
            (boundary.add_nanos(-1), "-1ns"),
        ] {
            let mut e = ChartEngine::default();
            apply_all(
                &mut e,
                &[
                    ev(trade(before, "1.00", 1, 1)),
                    ev(trade(boundary, "2.00", 1, 2)),
                ],
            );
            let cs = candles(&e, tf);
            assert_eq!(cs.len(), 2, "{tf} {label}");
            assert_eq!(cs[0].t, ms(tf.start_of(before)), "{tf} {label}");
            assert_eq!(cs[0].t, ms(boundary) - tf.secs() * 1000, "{tf} {label}");
            assert_eq!(cs[1].t, ms(boundary), "{tf} {label}");
            assert_eq!((cs[0].c, cs[1].o), (1.0, 2.0));
        }
    }
    // :29.999 vs :30.000 splits 15s and 30s candles but not 1m/5m.
    let mut e = ChartEngine::default();
    apply_all(
        &mut e,
        &[
            ev(trade(at(10, 0, 29, 999), "1.00", 1, 1)),
            ev(trade(at(10, 0, 30, 0), "2.00", 1, 2)),
        ],
    );
    assert_eq!(candles(&e, Timeframe::S15).len(), 2);
    assert_eq!(candles(&e, Timeframe::S30).len(), 2);
    assert_eq!(candles(&e, Timeframe::M1).len(), 1);
    assert_eq!(candles(&e, Timeframe::M5).len(), 1);
    // :59.999 vs :00.000 splits every timeframe up to 1m.
    let mut e = ChartEngine::default();
    apply_all(
        &mut e,
        &[
            ev(trade(at(10, 0, 59, 999), "1.00", 1, 1)),
            ev(trade(at(10, 1, 0, 0), "2.00", 1, 2)),
        ],
    );
    assert_eq!(candles(&e, Timeframe::M1).len(), 2);
    assert_eq!(candles(&e, Timeframe::M5).len(), 1);
}

#[test]
fn same_timestamp_trades_follow_source_seq() {
    let ts = at(10, 0, 3, 0);
    let mut e = ChartEngine::default();
    let ups = apply_all(
        &mut e,
        &[
            ev(trade(ts, "3.03", 10, 3)),
            ev(trade(ts, "3.01", 10, 1)),
            ev(trade(ts, "3.02", 10, 2)),
        ],
    );
    for tf in Timeframe::ALL {
        let c = &candles(&e, tf)[0];
        assert_eq!(
            (c.o, c.c, c.h, c.l, c.v, c.n),
            (3.01, 3.03, 3.03, 3.01, 30, 3),
            "{tf}"
        );
    }
    assert_eq!(e.quality(SYM)[0].1.out_of_order_reordered, 2);
    assert_eq!(tape_counted(&ups), vec!["ohlc_and_volume"; 3]);
}

#[test]
fn forming_then_final_by_trade_and_by_clock() {
    let mut e = ChartEngine::default();
    let u1 = e.apply(&ev(trade(at(10, 0, 5, 0), "3.00", 100, 1)));
    let c15: Vec<_> = u1.iter().filter_map(|u| u.as_candle()).collect();
    assert_eq!(c15.len(), 4);
    assert!(c15.iter().all(|u| !u.candle.is_final));

    // A trade in the next 15s interval finalizes the previous 15s candle first.
    let u2 = e.apply(&ev(trade(at(10, 0, 20, 0), "3.10", 100, 2)));
    let finals: Vec<_> = u2
        .iter()
        .filter_map(|u| u.as_candle())
        .filter(|u| u.candle.is_final)
        .collect();
    assert_eq!(finals.len(), 1);
    assert_eq!(
        (finals[0].timeframe, finals[0].candle.t),
        (Timeframe::S15, ms(at(10, 0, 0, 0)))
    );
    assert!(finals[0].profile.is_empty());
    let pos_final = u2
        .iter()
        .position(|u| u.as_candle().is_some_and(|c| c.candle.is_final))
        .unwrap();
    let pos_new = u2
        .iter()
        .position(|u| {
            u.as_candle()
                .is_some_and(|c| c.timeframe == Timeframe::S15 && !c.candle.is_final)
        })
        .unwrap();
    assert!(pos_final < pos_new);

    // Silent symbol: the clock finalizes it.
    let u3 = e.advance_clock(at(10, 0, 30, 0));
    let f3: Vec<_> = u3
        .iter()
        .filter_map(|u| u.as_candle())
        .map(|u| (u.timeframe, u.candle.t))
        .collect();
    assert_eq!(
        f3,
        vec![
            (Timeframe::S15, ms(at(10, 0, 15, 0))),
            (Timeframe::S30, ms(at(10, 0, 0, 0)))
        ]
    );
    assert!(e.advance_clock(at(10, 0, 59, 999)).is_empty());
    let u4 = e.advance_clock(at(10, 5, 0, 0));
    let f4: Vec<_> = u4
        .iter()
        .filter_map(|u| u.as_candle())
        .map(|u| u.timeframe)
        .collect();
    assert_eq!(f4, vec![Timeframe::M1, Timeframe::M5]);
    for tf in Timeframe::ALL {
        assert!(candles(&e, tf).iter().all(|c| c.is_final), "{tf}");
    }
    // Finalization is emitted once.
    assert!(e.advance_clock(at(10, 10, 0, 0)).is_empty());
}

#[test]
fn clock_does_not_go_backwards() {
    let mut e = ChartEngine::default();
    e.apply(&ev(trade(at(10, 0, 5, 0), "3.00", 100, 1)));
    e.advance_clock(at(10, 0, 20, 0));
    e.advance_clock(at(10, 0, 1, 0));
    assert_eq!(e.clock(), Some(at(10, 0, 20, 0)));
}

#[test]
fn gaps_are_marked_never_filled() {
    let mut e = ChartEngine::default();
    let ups = apply_all(&mut e, &base_fixture());
    let snap = e.snapshot(SYM, Timeframe::M1, None).unwrap();
    assert_eq!(snap.candles.len(), 3); // no zero-volume candles for 10:02, 10:03
    assert_eq!(snap.gaps.len(), 1);
    let g = &snap.gaps[0];
    assert_eq!(
        (g.t, g.end, g.reason, g.v, g.intervals),
        (
            ms(at(10, 2, 0, 0)),
            ms(at(10, 4, 0, 0)),
            GapReason::NoTrades,
            0,
            2
        )
    );
    // The same gap was announced incrementally when 10:04 opened.
    let inc: Vec<_> = ups
        .iter()
        .filter_map(|u| match u {
            ChartUpdate::Gap(g) if g.timeframe == Timeframe::M1 => Some(g.gap.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(inc, vec![g.clone()]);
    let s15 = e.snapshot(SYM, Timeframe::S15, None).unwrap();
    let spans: Vec<_> = s15.gaps.iter().map(|g| (g.t, g.intervals)).collect();
    assert_eq!(
        spans,
        vec![(ms(at(10, 0, 45, 0)), 1), (ms(at(10, 1, 15, 0)), 14)]
    );
}

#[test]
fn no_gap_across_session_boundary() {
    let mut e = ChartEngine::default();
    apply_all(
        &mut e,
        &[
            ev(trade(at(9, 29, 50, 0), "3.00", 100, 1)), // pre
            ev(trade(at(9, 31, 10, 0), "3.10", 100, 2)), // regular; 09:30 is empty
            ev(trade(at(9, 35, 0, 0), "3.20", 100, 3)),
        ],
    );
    let snap = e.snapshot(SYM, Timeframe::M1, None).unwrap();
    assert_eq!(snap.candles.len(), 3);
    let gaps: Vec<_> = snap.gaps.iter().map(|g| (g.t, g.end)).collect();
    assert_eq!(gaps, vec![(ms(at(9, 32, 0, 0)), ms(at(9, 35, 0, 0)))]);
    assert_eq!(snap.session.unwrap().id, "2026-10-15/regular");
}

#[test]
fn switching_timeframe_reads_the_same_trades() {
    let mut e = ChartEngine::default();
    let mut events = Vec::new();
    for i in 0..600u64 {
        // One trade every 0.5 s with a deterministic price walk.
        let ts = at(10, 0, 0, 0).add_nanos(i as i64 * 500_000_000);
        let cents = 300 + ((i * 37) % 23) as i64;
        events.push(ev(trade(
            ts,
            &format!("{}.{:02}", cents / 100, cents % 100),
            10 + i % 7,
            i + 1,
        )));
    }
    apply_all(&mut e, &events);
    let tot = |tf| {
        let cs = candles(&e, tf);
        (
            cs.iter().map(|c| c.v).sum::<u64>(),
            cs.iter().map(|c| c.n).sum::<u64>(),
            cs.iter().map(|c| c.h).fold(f64::MIN, f64::max),
            cs.iter().map(|c| c.l).fold(f64::MAX, f64::min),
            cs.first().unwrap().o,
            cs.last().unwrap().c,
        )
    };
    let reference = tot(Timeframe::M5);
    for tf in Timeframe::ALL {
        assert_eq!(tot(tf), reference, "{tf}");
    }
    // Profile is identical whichever timeframe is viewed.
    let p = |tf| e.snapshot(SYM, tf, None).unwrap().profile;
    for tf in Timeframe::ALL {
        assert_eq!(p(tf), p(Timeframe::M5));
    }
}

#[test]
fn ring_buffer_and_retention_are_bounded() {
    let cfg = EngineConfig {
        max_intervals_per_timeframe: 3,
        ..EngineConfig::default()
    };
    let mut e = ChartEngine::new(cfg).unwrap();
    for i in 0..5 {
        e.apply(&ev(trade(
            at(10, 0, 0, 0).add_nanos(i * 15_000_000_000),
            "1.00",
            1,
            i as u64 + 1,
        )));
    }
    let cs = candles(&e, Timeframe::S15);
    assert_eq!(
        cs.iter().map(|c| c.t).collect::<Vec<_>>(),
        vec![
            ms(at(10, 0, 30, 0)),
            ms(at(10, 0, 45, 0)),
            ms(at(10, 1, 0, 0))
        ]
    );

    let mut e = ChartEngine::default();
    for i in 0..3600i64 {
        e.apply(&ev(trade(
            at(10, 0, 0, 0).add_nanos(i * 1_000_000_000),
            "1.00",
            1,
            i as u64 + 1,
        )));
    }
    // 10 min retention aligned down to 5 min: at most 15 min of 1 trade/s.
    assert!(e.retained_trades(SYM) <= 900, "{}", e.retained_trades(SYM));
    assert!(e.retained_trades(SYM) >= 600);
}

#[test]
fn quotes_and_status_do_not_touch_candles() {
    let mut e = ChartEngine::default();
    let q = market_events::MarketEvent::Status(market_events::Status {
        symbol: Some(SYM.into()),
        event_ts: at(10, 0, 0, 0),
        receive_ts: None,
        source: market_events::Source::Fixture,
        kind: market_events::StatusKind::Halt,
        code: "H".into(),
        reason: String::new(),
        tape: market_events::Tape::C,
    });
    // A halt is recorded (chart.halt) but makes no candle, gap or tape row.
    let ups = e.apply(&q);
    assert_eq!(ups.len(), 1);
    assert!(ups[0].as_halt().is_some() && ups[0].timeframe().is_none());
    let snap = e.snapshot(SYM, Timeframe::M1, None).unwrap();
    assert!(snap.candles.is_empty() && snap.gaps.is_empty() && snap.luld.is_empty());
    assert_eq!(snap.halts.len(), 1);
    // A market-wide status changes nothing.
    let mut e = ChartEngine::default();
    let mut wide = q.clone();
    if let market_events::MarketEvent::Status(s) = &mut wide {
        s.symbol = None;
    }
    assert!(e.apply(&wide).is_empty());
    assert!(e.snapshot(SYM, Timeframe::M1, None).is_none());
    let _ = UnixNanos(0);
}

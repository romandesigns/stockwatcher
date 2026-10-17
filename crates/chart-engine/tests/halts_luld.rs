//! Halts and LULD bands through the engine (docs/architecture/halts-luld-fills.md §1–2). The
//! band arithmetic itself is unit-tested next to `luld_bands` (src/luld.rs).
mod common;

use chart_engine::wire::{ChartUpdate, HaltWire, LuldWire};
use chart_engine::{ChartEngine, Timeframe};
use common::*;
use market_events::{MarketEvent, Source, Status, StatusKind, Tape, UnixNanos};

fn status(ts: UnixNanos, kind: StatusKind) -> MarketEvent {
    MarketEvent::Status(Status {
        symbol: Some(SYM.into()),
        event_ts: ts,
        receive_ts: None,
        source: Source::Fixture,
        kind,
        code: if kind == StatusKind::Halt { "H" } else { "T" }.into(),
        reason: "LUDP".into(),
        tape: Tape::C,
    })
}

fn halts(ups: &[ChartUpdate]) -> Vec<HaltWire> {
    ups.iter().filter_map(|u| u.as_halt()).cloned().collect()
}

fn lulds(ups: &[ChartUpdate]) -> Vec<LuldWire> {
    ups.iter().filter_map(|u| u.as_luld()).cloned().collect()
}

fn unmatched(e: &ChartEngine) -> u64 {
    e.quality(SYM).iter().map(|(_, q)| q.halts_unmatched).sum()
}

fn band(t: UnixNanos, r: f64, up: f64, down: f64) -> LuldWire {
    LuldWire {
        t: ms(t),
        reference: Some(r),
        up: Some(up),
        down: Some(down),
    }
}

fn null(t: UnixNanos) -> LuldWire {
    LuldWire {
        t: ms(t),
        reference: None,
        up: None,
        down: None,
    }
}

#[test]
fn halt_and_resume_emit_chart_halt_and_land_in_every_snapshot() {
    let mut e = ChartEngine::default();
    e.apply(&ev(trade(at(10, 0, 0, 0), "3.38", 100, 1)));
    let start = e.apply(&status(at(10, 5, 0, 0), StatusKind::Halt));
    let open = HaltWire {
        start: ms(at(10, 5, 0, 0)),
        end: None,
        reason: "LUDP".into(),
        code: "H".into(),
    };
    assert_eq!(halts(&start), vec![open.clone()]);
    assert!(start.iter().all(|u| u.timeframe().is_none()));
    let end = e.apply(&status(at(10, 6, 30, 0), StatusKind::Resume));
    let closed = HaltWire {
        end: Some(ms(at(10, 6, 30, 0))),
        ..open
    };
    assert_eq!(halts(&end), vec![closed.clone()]);
    for tf in Timeframe::ALL {
        assert_eq!(
            e.snapshot(SYM, tf, None).unwrap().halts,
            vec![closed.clone()]
        );
    }
    assert_eq!(unmatched(&e), 0);
}

#[test]
fn unmatched_statuses_are_ignored_and_counted() {
    let mut e = ChartEngine::default();
    e.apply(&ev(trade(at(10, 0, 0, 0), "3.38", 100, 1)));
    // A resume with no open halt.
    assert!(
        e.apply(&status(at(10, 1, 0, 0), StatusKind::Resume))
            .is_empty()
    );
    e.apply(&status(at(10, 5, 0, 0), StatusKind::Halt));
    // A second halt while halted: the first start stands.
    assert!(
        e.apply(&status(at(10, 5, 30, 0), StatusKind::Halt))
            .is_empty()
    );
    e.apply(&status(at(10, 6, 0, 0), StatusKind::Resume));
    let snap = e.snapshot(SYM, Timeframe::M1, None).unwrap();
    assert_eq!(snap.halts.len(), 1);
    assert_eq!(snap.halts[0].start, ms(at(10, 5, 0, 0)));
    assert_eq!(snap.halts[0].end, Some(ms(at(10, 6, 0, 0))));
    assert_eq!(snap.quality.halts_unmatched, 2);
    // Market open/close and other kinds change nothing.
    for k in [
        StatusKind::MarketOpen,
        StatusKind::MarketClose,
        StatusKind::Other,
    ] {
        assert!(e.apply(&status(at(10, 7, 0, 0), k)).is_empty());
    }
    assert_eq!(unmatched(&e), 2);
}

#[test]
fn luld_bands_follow_the_session_and_stop_at_a_halt_and_the_close() {
    let mut e = ChartEngine::default();
    // Pre-market: no bands.
    let ups = e.apply(&ev(trade(at(9, 0, 0, 0), "3.00", 100, 1)));
    assert!(lulds(&ups).is_empty());
    // The first regular-session trade is the reference; 09:30–09:45 doubles 10% to 20%.
    let ups = e.apply(&ev(trade(at(9, 30, 5, 0), "3.38", 100, 2)));
    assert_eq!(lulds(&ups), vec![band(at(9, 30, 5, 0), 3.38, 4.05, 2.71)]);
    // A volume-only print (odd lot) never moves the reference.
    let ups = e.apply(&ev(
        trade(at(9, 31, 0, 0), "9.00", 50, 3).with_conditions(["@", "I"])
    ));
    assert!(lulds(&ups).is_empty());
    // The clock crosses 09:45: same reference, 10%.
    let ups = e.advance_clock(at(9, 46, 0, 0));
    assert_eq!(lulds(&ups), vec![band(at(9, 45, 0, 0), 3.38, 3.71, 3.05)]);
    // Halt: a null point, and nothing while halted.
    let ups = e.apply(&status(at(10, 5, 0, 0), StatusKind::Halt));
    assert_eq!(lulds(&ups), vec![null(at(10, 5, 0, 0))]);
    let ups = e.apply(&ev(trade(at(10, 5, 10, 0), "3.50", 100, 4)));
    assert!(lulds(&ups).is_empty());
    e.apply(&status(at(10, 6, 30, 0), StatusKind::Resume));
    // The first trade after the resume restarts the bands from its own price.
    let ups = e.apply(&ev(trade(at(10, 6, 31, 0), "3.60", 100, 5)));
    assert_eq!(lulds(&ups), vec![band(at(10, 6, 31, 0), 3.60, 3.96, 3.24)]);
    // 16:00 ends the bands (3.60 ≥ $3: no closing doubling).
    let ups = e.advance_clock(at(16, 0, 0, 0));
    assert_eq!(lulds(&ups), vec![null(at(16, 0, 0, 0))]);
    let snap = e.snapshot(SYM, Timeframe::S15, None).unwrap();
    assert_eq!(
        snap.luld,
        vec![
            band(at(9, 30, 5, 0), 3.38, 4.05, 2.71),
            band(at(9, 45, 0, 0), 3.38, 3.71, 3.05),
            null(at(10, 5, 0, 0)),
            band(at(10, 6, 31, 0), 3.60, 3.96, 3.24),
            null(at(16, 0, 0, 0)),
        ]
    );
    // Every timeframe carries the same list.
    for tf in Timeframe::ALL {
        assert_eq!(e.snapshot(SYM, tf, None).unwrap().luld, snap.luld);
    }
    // After-hours trades change nothing.
    let ups = e.apply(&ev(trade(at(16, 30, 0, 0), "4.50", 100, 6)));
    assert!(lulds(&ups).is_empty());
}

#[test]
fn a_print_after_the_close_ends_the_bands_before_the_clock_does() {
    let mut e = ChartEngine::default();
    e.apply(&ev(trade(at(15, 0, 0, 0), "2.00", 100, 1)));
    // No clock at all: the after-hours print crosses 15:35 and 16:00.
    let ups = e.apply(&ev(trade(at(16, 0, 1, 0), "2.00", 100, 2)));
    assert_eq!(
        lulds(&ups),
        vec![
            band(at(15, 35, 0, 0), 2.00, 2.80, 1.20),
            null(at(16, 0, 0, 0))
        ]
    );
}

#[test]
fn the_luld_list_belongs_to_the_current_trading_date() {
    let mut e = ChartEngine::default();
    e.apply(&ev(trade(at(10, 0, 0, 0), "3.38", 100, 1)));
    e.advance_clock(at(16, 0, 0, 0));
    assert_eq!(e.snapshot(SYM, Timeframe::M1, None).unwrap().luld.len(), 2);
    // 20:00 opens the next trading date's overnight session: yesterday's bands are not shown.
    let night = at(20, 30, 0, 0);
    e.apply(&ev(trade(night, "3.40", 100, 2)));
    assert!(
        e.snapshot(SYM, Timeframe::M1, None)
            .unwrap()
            .luld
            .is_empty()
    );
}

/// The engine is rebuilt from scratch on a seek; the same events give the same halts and bands,
/// whether the clock is advanced in many small steps or only right before each event.
#[test]
fn halts_and_bands_rebuild_identically() {
    let mut events = Vec::new();
    let mut seq = 0;
    let mut t = at(9, 30, 0, 0);
    let mut cents = 300i64;
    while t < at(10, 30, 0, 0) {
        seq += 1;
        cents += [3, -1, 2, -4, 5, 1][seq as usize % 6];
        let p = format!("{}.{:02}", cents / 100, cents % 100);
        events.push(ev(trade(t, &p, 100, seq)));
        if seq == 400 {
            events.push(status(t.add_nanos(1), StatusKind::Halt));
            events.push(status(t.add_nanos(90_000_000_000), StatusKind::Resume));
            t = t.add_nanos(90_000_000_000);
        }
        t = t.add_nanos(7_000_000_000);
    }
    let run = |fine: bool| {
        let mut e = ChartEngine::default();
        let mut log = Vec::new();
        for ev in &events {
            let at_s = UnixNanos(ev.event_ts().0.div_euclid(1_000_000_000) * 1_000_000_000);
            if fine {
                log.extend(e.advance_clock(at_s.add_nanos(-3_000_000_000)));
            }
            log.extend(e.advance_clock(at_s));
            log.extend(e.apply(ev));
        }
        log.extend(e.advance_clock(at(16, 0, 0, 0)));
        let snap = e.snapshot(SYM, Timeframe::M1, None).unwrap();
        let mut marks: Vec<String> = log
            .iter()
            .filter(|u| u.as_halt().is_some() || u.as_luld().is_some())
            .map(|u| serde_json::to_string(u).unwrap())
            .collect();
        marks.dedup();
        (snap.halts, snap.luld, marks)
    };
    let a = run(false);
    assert_eq!(a, run(true));
    assert_eq!(a.0.len(), 1);
    assert!(a.1.len() >= 4, "{:?}", a.1);
    assert_eq!(a.1.last().unwrap().up, None);
    // The incremental messages, applied as upserts by `t`, give the snapshot's list.
    let mut upserted: Vec<LuldWire> = Vec::new();
    for m in &a.2 {
        let u: ChartUpdate = serde_json::from_str(m).unwrap();
        if let Some(p) = u.as_luld() {
            upserted.retain(|q| q.t != p.t);
            upserted.push(p.clone());
        }
    }
    assert_eq!(upserted, a.1);
}

#[test]
fn halts_that_ended_before_the_retained_candles_are_dropped() {
    let cfg = chart_engine::EngineConfig {
        max_intervals_per_timeframe: 8,
        ..Default::default()
    };
    let mut e = ChartEngine::new(cfg).unwrap();
    e.apply(&ev(trade(at(10, 0, 0, 0), "3.38", 100, 1)));
    e.apply(&status(at(10, 0, 1, 0), StatusKind::Halt));
    e.apply(&status(at(10, 0, 2, 0), StatusKind::Resume));
    e.apply(&status(at(10, 1, 0, 0), StatusKind::Halt)); // still open: never dropped
    assert_eq!(e.snapshot(SYM, Timeframe::M5, None).unwrap().halts.len(), 2);
    // An hour of 5-minute candles pushes the 10:00 interval out of every ring.
    for (i, m) in (5..=55).step_by(5).enumerate() {
        e.apply(&ev(trade(at(11, m, 0, 0), "3.38", 100, 10 + i as u64)));
    }
    let snap = e.snapshot(SYM, Timeframe::M5, None).unwrap();
    assert_eq!(snap.halts.len(), 1);
    assert_eq!(snap.halts[0].end, None);
}

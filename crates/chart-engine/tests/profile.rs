//! Session volume profile: bins, cumulative volume, bin width, session separation, POC/VA.
mod common;

use chart_engine::{ChartEngine, Timeframe};
use common::*;
use market_events::session::et;
use market_events::{Price, SessionId};

fn sid(s: &str) -> SessionId {
    s.parse().unwrap()
}

#[test]
fn bins_and_cumulative_volume() {
    let mut e = ChartEngine::default();
    apply_all(
        &mut e,
        &[
            ev(trade(at(10, 0, 1, 0), "3.18", 100, 1)),
            ev(trade(at(10, 0, 2, 0), "3.1850", 50, 2)),
        ],
    );
    let third = e.apply(&ev(trade(at(10, 3, 0, 0), "3.18", 25, 3)));
    e.apply(&ev(trade(at(10, 7, 0, 0), "3.21", 10, 4)));
    let snap = e.snapshot(SYM, Timeframe::M1, None).unwrap();
    let p = snap.profile.unwrap();
    assert_eq!(p.anchor, "2026-10-15/regular");
    assert_eq!(p.bin, 0.01);
    assert_eq!(
        p.rows.iter().map(|r| (r.p, r.v)).collect::<Vec<_>>(),
        vec![(3.18, 175), (3.21, 10)]
    );
    assert_eq!(p.total, 185);
    assert_eq!(p.total, snap.candles.iter().map(|c| c.v).sum::<u64>());
    // Increments carry the absolute bin total: the third print reports 175 for the 3.18 bin.
    let third: Vec<_> = third
        .iter()
        .filter_map(|u| u.as_candle())
        .flat_map(|u| u.profile.iter().map(|r| (r.p, r.v)))
        .collect();
    assert_eq!(third, vec![(3.18, 175); 4], "one row per timeframe update");
}

#[test]
fn configurable_bin_width_is_exact() {
    let mut e = ChartEngine::default();
    let prices = ["0.1501", "0.1549", "0.1550", "0.1599", "0.1600", "0.2234"];
    let evs: Vec<_> = prices
        .iter()
        .enumerate()
        .map(|(i, p)| ev(trade(at(10, 0, 1, 0), p, 100 + i as u64, i as u64 + 1)))
        .collect();
    apply_all(&mut e, &evs);
    let rows = |bin: &str| {
        e.profile_rows(SYM, sid("2026-10-15/regular"), px(bin))
            .unwrap()
    };
    assert_eq!(
        rows("0.01"),
        vec![
            (px("0.15"), 100 + 101 + 102 + 103),
            (px("0.16"), 104),
            (px("0.22"), 105)
        ]
    );
    assert_eq!(
        rows("0.005"),
        vec![
            (px("0.15"), 201),
            (px("0.155"), 205),
            (px("0.16"), 104),
            (px("0.22"), 105)
        ]
    );
    assert_eq!(rows("0.0001").len(), 6);
    assert_eq!(rows("0.05"), vec![(px("0.15"), 510), (px("0.20"), 105)]);
    for bin in ["0.0001", "0.005", "0.01", "0.05", "1"] {
        assert_eq!(rows(bin).iter().map(|r| r.1).sum::<u64>(), 615, "{bin}");
    }
    let snap = e.snapshot(SYM, Timeframe::S15, Some(px("0.05"))).unwrap();
    assert_eq!(snap.profile.unwrap().bin, 0.05);
    // Viewing another bin does not change the engine's bin for increments.
    let ups = e.apply(&ev(trade(at(10, 0, 2, 0), "0.1501", 1, 99)));
    assert_eq!(
        ups.iter().find_map(|u| u.as_candle()).unwrap().profile_bin,
        0.01
    );
}

#[test]
fn sessions_are_separate() {
    let mut e = ChartEngine::default();
    let prev_evening = et(
        chrono::NaiveDate::from_ymd_opt(2026, 10, 14).unwrap(),
        20,
        30,
        0,
        0,
    )
    .unwrap();
    apply_all(
        &mut e,
        &[
            ev(trade(prev_evening, "3.00", 10, 1)),
            ev(trade(at(8, 0, 0, 0), "3.05", 20, 2)),
            ev(trade(at(9, 29, 59, 999), "3.05", 30, 3)),
            ev(trade(at(9, 30, 0, 0), "3.10", 40, 4)),
            ev(trade(at(16, 0, 0, 0), "3.20", 50, 5)),
        ],
    );
    assert_eq!(
        e.sessions(SYM),
        vec![
            sid("2026-10-15/overnight"),
            sid("2026-10-15/pre"),
            sid("2026-10-15/regular"),
            sid("2026-10-15/after")
        ]
    );
    let tot = |s: &str| e.profile(SYM, sid(s), None).unwrap().total;
    assert_eq!(
        (
            tot("2026-10-15/overnight"),
            tot("2026-10-15/pre"),
            tot("2026-10-15/regular"),
            tot("2026-10-15/after")
        ),
        (10, 50, 40, 50)
    );
    // Snapshots show the current (latest) session's profile whatever the timeframe.
    for tf in Timeframe::ALL {
        let s = e.snapshot(SYM, tf, None).unwrap();
        assert_eq!(s.profile.unwrap().anchor, "2026-10-15/after", "{tf}");
        assert_eq!(s.session.unwrap().id, "2026-10-15/after");
    }
}

#[test]
fn poc_and_value_area() {
    let mut e = ChartEngine::default();
    let vols = [
        ("3.00", 10),
        ("3.01", 30),
        ("3.02", 40),
        ("3.03", 15),
        ("3.04", 5),
    ];
    let evs: Vec<_> = vols
        .iter()
        .enumerate()
        .map(|(i, (p, v))| ev(trade(at(10, 0, 1, 0), p, *v, i as u64 + 1)))
        .collect();
    apply_all(&mut e, &evs);
    let p = e.profile(SYM, sid("2026-10-15/regular"), None).unwrap();
    assert_eq!(p.poc, Some(3.02));
    let va = p.value_area.unwrap();
    assert_eq!((va.low, va.high, va.v), (3.01, 3.02, 70));
}

#[test]
fn sessions_retained_is_bounded() {
    let mut e = ChartEngine::new(chart_engine::EngineConfig {
        sessions_retained: 2,
        ..Default::default()
    })
    .unwrap();
    apply_all(
        &mut e,
        &[
            ev(trade(at(8, 0, 0, 0), "3.05", 20, 2)),
            ev(trade(at(9, 30, 0, 0), "3.10", 40, 4)),
            ev(trade(at(16, 0, 0, 0), "3.20", 50, 5)),
        ],
    );
    assert_eq!(
        e.sessions(SYM),
        vec![sid("2026-10-15/regular"), sid("2026-10-15/after")]
    );
    let _ = Price::ZERO;
}

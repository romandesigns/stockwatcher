//! The profile's conditions and the board's morning: matching, the reasons, staying once shown,
//! freezing at 09:30, clearing after, and saying nothing it could not check.

use super::*;
use market_events::{Price, Trade};

const BOTH: Counted = Counted {
    ohlc: true,
    volume: true,
};

fn day() -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 11, 24).unwrap()
}

fn ns(h: u32, m: u32) -> i64 {
    at(day(), h, m).0
}

fn tr(sym: &str, ts: i64, px: f64, size: u64) -> Trade {
    Trade::new(
        sym,
        UnixNanos(ts),
        Price::from_micros((px * 1e6).round() as i64),
        size,
        1,
    )
}

/// AKAN 11-24 as the profile saw it: C1 2.58, float 2.39 M, an 11-18 high +128 %.
fn akan() -> Inputs {
    Inputs {
        float: Some(2_390_000.0),
        c1: Some(2.58),
        big_day: Some((NaiveDate::from_ymd_opt(2026, 11, 18).unwrap(), 1.28)),
    }
}

fn board() -> WatchBoard {
    let mut b = WatchBoard::new();
    b.enable();
    b
}

#[test]
fn akan_matches_profile_b_and_each_condition_fails_on_its_own() {
    assert!(check(3.27, 3_560_000, &akan()).is_empty());
    assert_eq!(check(21.0, 3_560_000, &akan()), vec!["price"]);
    assert_eq!(check(3.27, 1_000_000, &akan()), vec!["volume"]);
    assert_eq!(check(2.90, 3_560_000, &akan()), vec!["gap"]);
    let big = Inputs {
        float: Some(12_000_000.0),
        ..akan()
    };
    assert_eq!(check(3.27, 12_000_000, &big), vec!["float"]);
    let no_run = Inputs {
        big_day: None,
        ..akan()
    };
    assert_eq!(check(3.27, 3_560_000, &no_run), vec!["big_day"]);
    // Nothing to check against: the conditions fail, never pass by default.
    assert_eq!(
        check(3.27, 3_560_000, &Inputs::default()),
        vec!["float", "volume", "gap", "big_day"]
    );
}

#[test]
fn a_card_appears_when_the_profile_holds_stays_and_says_what_stopped_holding() {
    let mut b = board();
    b.apply_trade(&tr("AKAN", ns(4, 0), 2.90, 100), BOTH);
    b.apply_trade(&tr("AKAN", ns(7, 0), 3.30, 1_000_000), BOTH);
    b.tick(ns(7, 0) + 1, true, |_| akan());
    // +28 % but 42 % of the float: no card yet.
    assert!(b.cards().is_empty());
    assert!(
        b.take_update().is_some(),
        "the float source is news on the first tick"
    );
    b.apply_trade(&tr("AKAN", ns(8, 0), 3.27, 2_560_000), BOTH);
    b.tick(ns(8, 0) + 1, true, |_| akan());
    let c = b.cards()[0].clone();
    assert_eq!(
        (c.symbol.as_str(), c.pm_vol, c.fails.len(), c.partial),
        ("AKAN", 3_560_100, 0, false)
    );
    assert!((c.gap - (3.27 / 2.58 - 1.0)).abs() < 1e-9);
    let frame: Value = serde_json::from_str(&b.take_update().expect("a new card")).unwrap();
    assert_eq!(frame["cards"][0]["bigDay"]["date"], "2026-11-18");
    assert_eq!(frame["cards"][0]["pmVolPct"], 1.49);
    assert_eq!(frame["floatSource"], "fmp");
    // The same values again: no frame.
    b.apply_trade(&tr("AKAN", ns(8, 1), 3.27, 0), BOTH);
    b.tick(ns(8, 1) + 1, true, |_| akan());
    assert!(b.take_update().is_none());
    // It fades under +15 %: the card stays and says so.
    b.apply_trade(&tr("AKAN", ns(9, 0), 2.80, 10), BOTH);
    b.tick(ns(9, 0) + 1, true, |_| akan());
    assert_eq!(b.cards()[0].fails, vec!["gap"]);
    assert!(b.take_update().is_some());
}

#[test]
fn the_open_freezes_the_card_and_the_morning_clears_after() {
    let mut b = board();
    b.apply_trade(&tr("AKAN", ns(4, 0), 3.27, 3_560_000), BOTH);
    b.tick(ns(9, 0), true, |_| akan());
    assert_eq!(b.cards().len(), 1);
    // A regular-session print is not pre-market volume, nor the pre-market price.
    b.apply_trade(&tr("AKAN", ns(9, 31), 6.77, 9_000_000), BOTH);
    b.tick(ns(9, 32), true, |_| akan());
    assert_eq!((b.cards()[0].price, b.cards()[0].pm_vol), (3.27, 3_560_000));
    b.take_update();
    b.tick(ns(9, 45), true, |_| akan());
    assert!(b.cards().is_empty());
    assert!(b.take_update().is_some());
}

#[test]
fn a_late_start_is_partial_and_no_float_means_no_card() {
    let mut b = board();
    b.apply_trade(&tr("AKAN", ns(8, 0), 3.27, 3_560_000), BOTH);
    b.tick(ns(8, 0) + 1, true, |_| akan());
    assert!(b.cards()[0].partial);
    let mut b = board();
    b.apply_trade(&tr("AKAN", ns(4, 0), 3.27, 3_560_000), BOTH);
    b.tick(ns(5, 0), false, |_| Inputs {
        float: None,
        ..akan()
    });
    assert!(b.cards().is_empty());
    let frame: Value = serde_json::from_str(&b.snapshot_frame()).unwrap();
    assert_eq!(frame["floatSource"], "none");
    // Out of the band, nothing is even read.
    let mut b = board();
    b.apply_trade(&tr("PENY", ns(5, 0), 0.5, 10_000_000), BOTH);
    b.tick(ns(5, 1), true, |_| {
        panic!("inputs read for a stock out of the band")
    });
}

#[test]
fn a_new_trading_date_starts_empty() {
    let mut b = board();
    b.apply_trade(&tr("AKAN", ns(4, 0), 3.27, 3_560_000), BOTH);
    b.tick(ns(5, 0), true, |_| akan());
    assert_eq!(b.cards().len(), 1);
    let next = NaiveDate::from_ymd_opt(2026, 11, 25).unwrap();
    b.apply_trade(&tr("ZZZ", at(next, 4, 1).0, 5.0, 10), BOTH);
    assert!(b.cards().is_empty());
}

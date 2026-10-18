//! Duplicates, late and out-of-order trades, conditions, corrections, cancels, outside-session.
mod common;

use chart_engine::wire::{ChartUpdate, GapReason};
use chart_engine::{ChartEngine, Timeframe};
use common::*;
use market_events::{SessionId, UnixNanos};

fn regular() -> Option<SessionId> {
    Some("2026-10-15/regular".parse().unwrap())
}

fn q(e: &ChartEngine) -> chart_engine::DataQuality {
    e.quality(SYM)
        .into_iter()
        .find(|(s, _)| *s == regular())
        .map(|(_, q)| q)
        .unwrap_or_default()
}

#[test]
fn duplicates_are_ignored_and_counted() {
    let mut e = ChartEngine::default();
    let t = trade(at(10, 0, 1, 0), "3.18", 100, 1);
    let ups = apply_all(
        &mut e,
        &[ev(t.clone()), ev(t.clone()), ev(t.with_price_hack())],
    );
    assert_eq!(
        tape_counted(&ups),
        vec![
            "ohlc_and_volume",
            "excluded:duplicate",
            "excluded:duplicate"
        ]
    );
    assert_eq!(candles(&e, Timeframe::M1)[0].v, 100);
    assert_eq!(q(&e).duplicates, 2);
    // A different exchange with the same sequence number is a different print.
    e.apply(&ev(
        trade(at(10, 0, 1, 0), "3.18", 100, 1).with_exchange("Q")
    ));
    assert_eq!(candles(&e, Timeframe::M1)[0].v, 200);
}

trait PriceHack {
    fn with_price_hack(self) -> Self;
}

impl PriceHack for market_events::Trade {
    /// Same identity, different payload: still a duplicate (identity is source+exchange+seq).
    fn with_price_hack(mut self) -> Self {
        self.price = px("9.99");
        self
    }
}

#[test]
fn late_trade_within_window_goes_to_its_interval_without_moving_the_forming_candle() {
    let mut e = ChartEngine::default();
    apply_all(
        &mut e,
        &[
            ev(trade(at(10, 0, 10, 0), "3.00", 100, 1)),
            ev(trade(at(10, 0, 16, 0), "3.50", 100, 2)),
        ],
    );
    let forming_before = candles(&e, Timeframe::S15).last().cloned().unwrap();
    // 10:00:14 arrives when the watermark is 10:00:16: its 15s interval ended 1 s ago (< 2 s).
    let ups = e.apply(&ev(trade(at(10, 0, 14, 0), "3.20", 100, 3)));
    assert_eq!(tape_counted(&ups), vec!["ohlc_and_volume"]);
    let s15: Vec<_> = ups
        .iter()
        .filter_map(|u| u.as_candle())
        .filter(|u| u.timeframe == Timeframe::S15)
        .collect();
    assert_eq!(s15.len(), 1);
    assert_eq!(s15[0].candle.t, ms(at(10, 0, 0, 0)));
    assert!(s15[0].candle.is_final);
    assert_eq!(
        (s15[0].candle.o, s15[0].candle.c, s15[0].candle.v),
        (3.00, 3.20, 200)
    );
    let cs = candles(&e, Timeframe::S15);
    assert_eq!(cs.last().unwrap(), &forming_before);
    assert!(!cs.last().unwrap().is_final);
    // On the 1m candle the late print lands between the two others in time order.
    let m1 = &candles(&e, Timeframe::M1)[0];
    assert_eq!((m1.o, m1.c, m1.v), (3.00, 3.50, 300));
    let qq = q(&e);
    assert_eq!(
        (qq.late_applied, qq.out_of_order_reordered, qq.late_excluded),
        (1, 1, 0)
    );
}

#[test]
fn settle_window_edge_is_exact() {
    // Trade at 10:00:14 → 15s interval ends 10:00:15 → accepted while watermark < 10:00:17.
    let mut e = ChartEngine::default();
    e.apply(&ev(trade(at(10, 0, 16, 999), "3.00", 100, 1)));
    let ups = e.apply(&ev(trade(at(10, 0, 14, 0), "3.10", 100, 2)));
    assert_eq!(tape_counted(&ups), vec!["ohlc_and_volume"]);

    let mut e = ChartEngine::default();
    e.apply(&ev(trade(at(10, 0, 17, 0), "3.00", 100, 1)));
    let ups = e.apply(&ev(trade(at(10, 0, 14, 0), "3.10", 100, 2)));
    assert_eq!(tape_counted(&ups), vec!["excluded:late"]);
    assert_eq!(ups.len(), 1, "an excluded trade changes no candle");
    for tf in Timeframe::ALL {
        assert_eq!(
            candles(&e, tf).iter().map(|c| c.v).sum::<u64>(),
            100,
            "{tf}"
        );
    }
    let qq = q(&e);
    assert_eq!((qq.late_excluded, qq.late_applied), (1, 0));
}

#[test]
fn clock_makes_trades_late_too() {
    let mut e = ChartEngine::default();
    e.apply(&ev(trade(at(10, 0, 1, 0), "3.00", 100, 1)));
    e.advance_clock(at(10, 0, 20, 0));
    let ups = e.apply(&ev(trade(at(10, 0, 10, 0), "3.10", 100, 2)));
    assert_eq!(tape_counted(&ups), vec!["excluded:late"]);
}

#[test]
fn out_of_order_within_window_equals_in_order() {
    let trades = [
        trade(at(10, 0, 0, 100), "3.00", 10, 1),
        trade(at(10, 0, 0, 200), "3.40", 20, 2),
        trade(at(10, 0, 0, 900), "2.90", 30, 3),
        trade(at(10, 0, 1, 500), "3.10", 40, 4),
    ];
    let mut a = ChartEngine::default();
    apply_all(&mut a, &trades.iter().cloned().map(ev).collect::<Vec<_>>());
    let mut b = ChartEngine::default();
    apply_all(&mut b, &[3, 1, 0, 2].map(|i| ev(trades[i].clone())));
    for tf in Timeframe::ALL {
        assert_eq!(candles(&a, tf), candles(&b, tf), "{tf}");
    }
    assert_eq!(q(&b).out_of_order_reordered, 3);
}

#[test]
fn outside_session_is_excluded_and_counted() {
    let sat = market_events::session::et(
        chrono::NaiveDate::from_ymd_opt(2026, 10, 17).unwrap(),
        12,
        0,
        0,
        0,
    )
    .unwrap();
    let mut e = ChartEngine::default();
    let ups = e.apply(&ev(trade(sat, "3.00", 100, 1)));
    assert_eq!(tape_counted(&ups), vec!["excluded:outside_session"]);
    assert!(candles(&e, Timeframe::M1).is_empty());
    assert_eq!(
        e.quality(SYM),
        vec![(
            None,
            chart_engine::DataQuality {
                outside_session: 1,
                ..Default::default()
            }
        )]
    );
}

#[test]
fn condition_policy_is_applied() {
    let mut e = ChartEngine::default();
    let ups = apply_all(
        &mut e,
        &[
            ev(trade(at(10, 0, 1, 0), "3.18", 100, 1).with_conditions(["@"])),
            ev(trade(at(10, 0, 2, 0), "3.50", 37, 2).with_conditions(["@", "I"])), // odd lot
            ev(trade(at(10, 0, 3, 0), "2.00", 500, 3).with_conditions(["W"])),     // average price
            ev(trade(at(10, 0, 4, 0), "3.60", 900, 4).with_conditions(["M"])),     // official close
            ev(trade(at(10, 0, 5, 0), "3.20", 100, 5).with_conditions(["@", "S"])), // unlisted code
        ],
    );
    assert_eq!(
        tape_counted(&ups),
        vec![
            "ohlc_and_volume",
            "volume_only",
            "volume_only",
            "excluded:condition_M",
            "ohlc_and_volume"
        ]
    );
    let reasons: Vec<_> = ups
        .iter()
        .filter_map(|u| u.as_tape())
        .map(|t| t.reason.clone())
        .collect();
    assert_eq!(
        reasons,
        vec![
            None,
            Some("odd_lot".into()),
            Some("condition_W".into()),
            Some("condition_M".into()),
            None
        ]
    );
    let c = &candles(&e, Timeframe::M1)[0];
    assert_eq!(
        (c.o, c.h, c.l, c.c, c.v, c.n),
        (3.18, 3.20, 3.18, 3.20, 737, 4)
    );
    // Profile: regular sales and the odd lot; not the average-price print, not the official close.
    let rows = e.profile_rows(SYM, regular().unwrap(), px("0.01")).unwrap();
    assert_eq!(
        rows,
        vec![(px("3.18"), 100), (px("3.20"), 100), (px("3.50"), 37)]
    );
    let qq = q(&e);
    assert_eq!(
        (
            qq.volume_only,
            qq.condition_excluded,
            qq.unrecognized_conditions
        ),
        (2, 1, 1)
    );
}

#[test]
fn volume_only_interval_is_a_gap_not_a_candle() {
    let mut e = ChartEngine::default();
    let ups = apply_all(
        &mut e,
        &[
            ev(trade(at(10, 0, 1, 0), "3.00", 100, 1)),
            ev(trade(at(10, 1, 5, 0), "3.05", 40, 2).with_conditions(["@", "I"])),
            ev(trade(at(10, 2, 5, 0), "3.10", 100, 3)),
        ],
    );
    let snap = e.snapshot(SYM, Timeframe::M1, None).unwrap();
    assert_eq!(snap.candles.len(), 2);
    assert_eq!(snap.gaps.len(), 1);
    let g = &snap.gaps[0];
    assert_eq!(
        (g.t, g.reason, g.v),
        (ms(at(10, 1, 0, 0)), GapReason::VolumeOnlyTrades, 40)
    );
    assert!(ups.iter().any(|u| matches!(u, ChartUpdate::Gap(g) if g.timeframe == Timeframe::M1 && g.gap.reason == GapReason::VolumeOnlyTrades)));
    // A regular print arriving in time for that minute turns it into a candle (open = that print).
    let mut e = ChartEngine::default();
    apply_all(
        &mut e,
        &[
            ev(trade(at(10, 0, 1, 0), "3.00", 100, 1)),
            ev(trade(at(10, 1, 5, 0), "3.05", 40, 2).with_conditions(["@", "I"])),
            ev(trade(at(10, 1, 6, 0), "3.07", 100, 4)),
            ev(trade(at(10, 2, 5, 0), "3.10", 100, 3)),
        ],
    );
    let snap = e.snapshot(SYM, Timeframe::M1, None).unwrap();
    let c = &snap.candles[1];
    assert_eq!((c.o, c.v, c.n), (3.07, 140, 2));
    assert!(snap.gaps.is_empty());
}

#[test]
fn correction_recomputes_interval_and_profile_exactly() {
    let mut e = ChartEngine::default();
    apply_all(
        &mut e,
        &[
            ev(trade(at(10, 0, 1, 0), "3.18", 100, 1)),
            ev(trade(at(10, 0, 2, 0), "3.50", 100, 2)),
            ev(trade(at(10, 0, 20, 0), "3.19", 100, 3)),
        ],
    );
    let ups = e.apply(&correction(at(10, 0, 30, 0), 2, 99, "3.20", 150, &["@"]));
    let c15 = &candles(&e, Timeframe::S15)[0];
    assert_eq!(
        (c15.o, c15.h, c15.l, c15.c, c15.v),
        (3.18, 3.20, 3.18, 3.20, 250)
    );
    assert!(c15.is_final);
    let m1 = &candles(&e, Timeframe::M1)[0];
    assert_eq!((m1.h, m1.c, m1.v), (3.20, 3.19, 350));
    let rows = e.profile_rows(SYM, regular().unwrap(), px("0.01")).unwrap();
    assert_eq!(
        rows,
        vec![(px("3.18"), 100), (px("3.19"), 100), (px("3.20"), 150)]
    );
    // The update carries both touched bins with absolute totals (3.50 now empty).
    let prof = &ups.iter().find_map(|u| u.as_candle()).unwrap().profile;
    let prof: Vec<_> = prof.iter().map(|r| (r.p, r.v)).collect();
    assert_eq!(prof, vec![(3.20, 150), (3.50, 0)]);
    assert_eq!(q(&e).corrections_applied, 1);

    // The corrected print can itself be cancelled by its new id.
    e.apply(&cancel(at(10, 0, 31, 0), 99));
    let c15 = &candles(&e, Timeframe::S15)[0];
    assert_eq!((c15.h, c15.c, c15.v), (3.18, 3.18, 100));
    assert_eq!(q(&e).cancels_applied, 1);
}

#[test]
fn correction_to_an_excluding_condition_removes_the_print() {
    let mut e = ChartEngine::default();
    apply_all(
        &mut e,
        &[
            ev(trade(at(10, 0, 1, 0), "3.18", 100, 1)),
            ev(trade(at(10, 0, 2, 0), "3.50", 100, 2)),
        ],
    );
    e.apply(&correction(at(10, 0, 3, 0), 2, 3, "3.50", 100, &["M"]));
    let c = &candles(&e, Timeframe::S15)[0];
    assert_eq!((c.h, c.v, c.n), (3.18, 100, 1));
}

#[test]
fn cancel_of_the_only_print_removes_the_candle() {
    let mut e = ChartEngine::default();
    apply_all(
        &mut e,
        &[
            ev(trade(at(10, 0, 1, 0), "3.00", 100, 1)),
            ev(trade(at(10, 1, 1, 0), "3.10", 100, 2)),
            ev(trade(at(10, 2, 1, 0), "3.20", 100, 3)),
        ],
    );
    let ups = e.apply(&cancel(at(10, 2, 2, 0), 2));
    let removed: Vec<_> = ups
        .iter()
        .filter_map(|u| match u {
            ChartUpdate::Remove(r) => Some((r.timeframe, r.t)),
            _ => None,
        })
        .collect();
    assert!(removed.contains(&(Timeframe::M1, ms(at(10, 1, 0, 0)))));
    assert!(
        !removed.iter().any(|(tf, _)| *tf == Timeframe::M5),
        "the 5m candle still has other prints"
    );
    let snap = e.snapshot(SYM, Timeframe::M1, None).unwrap();
    assert_eq!(snap.candles.len(), 2);
    assert_eq!(
        snap.gaps
            .iter()
            .map(|g| (g.t, g.reason))
            .collect::<Vec<_>>(),
        vec![(ms(at(10, 1, 0, 0)), GapReason::NoTrades)]
    );
    assert_eq!(candles(&e, Timeframe::M5)[0].v, 200);
}

#[test]
fn unknown_or_expired_targets_are_counted_unapplied() {
    let mut e = ChartEngine::default();
    e.apply(&ev(trade(at(10, 0, 1, 0), "3.00", 100, 1)));
    e.apply(&cancel(at(10, 0, 2, 0), 777));
    // Push the watermark 20 minutes on: seq 1 leaves retention (10 min, aligned to 5 min).
    e.apply(&ev(trade(at(10, 20, 0, 0), "3.10", 100, 2)));
    let ups = e.apply(&correction(at(10, 20, 1, 0), 1, 50, "9.00", 1, &["@"]));
    assert!(ups.is_empty());
    assert_eq!(candles(&e, Timeframe::M1)[0].o, 3.00, "nothing changed");
    let qq = q(&e);
    assert_eq!((qq.cancels_unapplied, qq.corrections_unapplied), (1, 1));
    // An expired original is also no longer a duplicate key; replaying it is simply late.
    let ups = e.apply(&ev(trade(at(10, 0, 1, 0), "3.00", 100, 1)));
    assert_eq!(tape_counted(&ups), vec!["excluded:late"]);
    let _ = UnixNanos(0);
}

#[test]
fn late_correction_may_revise_a_final_candle() {
    let mut e = ChartEngine::default();
    e.apply(&ev(trade(at(10, 0, 1, 0), "3.00", 100, 1)));
    e.advance_clock(at(10, 3, 0, 0));
    let ups = e.apply(&correction(at(10, 3, 0, 0), 1, 2, "3.01", 100, &["@"]));
    let revised: Vec<_> = ups.iter().filter_map(|u| u.as_candle()).collect();
    assert_eq!(revised.len(), 4);
    assert!(revised.iter().all(|u| u.candle.o == 3.01));
    assert!(
        revised
            .iter()
            .filter(|u| u.timeframe != Timeframe::M5)
            .all(|u| u.candle.is_final)
    );
}

#[test]
fn apply_revising_reports_what_the_engine_removed_and_the_days_new_last_print() {
    use chart_engine::{RevisedPrint, Revision};
    let mut e = ChartEngine::default();
    apply_all(
        &mut e,
        &[
            ev(trade(at(10, 0, 1, 0), "3.00", 100, 1)),
            ev(trade(at(10, 0, 2, 0), "3.05", 40, 2).with_conditions(["@", "I"])),
            ev(trade(at(10, 1, 1, 0), "3.10", 100, 3)),
        ],
    );
    let print = |p: &str, size, ohlc, volume| RevisedPrint {
        price: px(p),
        size,
        ohlc,
        volume,
    };
    // A trade is not a revision, and `apply` is `apply_revising` without it.
    let (_, none) = e.apply_revising(&ev(trade(at(10, 1, 2, 0), "3.20", 50, 4)));
    assert_eq!(none, None);

    // Cancel the newest price print: the day's last falls back to the one before.
    let (ups, rev) = e.apply_revising(&cancel(at(10, 1, 3, 0), 4));
    assert!(!ups.is_empty());
    let rev: Revision = rev.expect("applied");
    assert_eq!(rev.session, regular().unwrap());
    assert_eq!(rev.removed, print("3.20", 50, true, true));
    assert_eq!(rev.added, None);
    assert_eq!(rev.last, Some((px("3.10"), at(10, 1, 1, 0))));
    assert_eq!(
        rev.last.unwrap().0.to_f64(),
        candles(&e, Timeframe::M1)[1].c
    );

    // Correct the odd lot into a regular print: it now counts for price too, same time.
    let (_, rev) = e.apply_revising(&correction(at(10, 1, 4, 0), 2, 9, "3.06", 40, &["@"]));
    let rev = rev.unwrap();
    assert_eq!(
        rev.removed,
        print("3.05", 40, false, true),
        "as it had counted"
    );
    assert_eq!(rev.added, Some(print("3.06", 40, true, true)));
    assert_eq!(rev.last, Some((px("3.10"), at(10, 1, 1, 0))));

    // Unapplied (unknown target): no revision, as no candle changed.
    assert_eq!(e.apply_revising(&cancel(at(10, 1, 5, 0), 777)).1, None);
}

//! What the engine keeps across trading dates, and that the compact trade keys (inline exchange
//! codes, the id index) resolve revisions exactly as the string keys did.
mod common;

use chart_engine::{ChartEngine, EngineConfig, Timeframe};
use chrono::NaiveDate;
use common::*;
use market_events::session::et;
use market_events::{Cancel, CancelAction, MarketEvent, Price, Source, Tape, UnixNanos};

/// Friday 2026-10-16, the trading date after [`day`].
fn next_day() -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 10, 16).unwrap()
}

fn next(h: u32, m: u32) -> UnixNanos {
    et(next_day(), h, m, 0, 0).unwrap()
}

/// The trading dates the candles of `tf` belong to (every print here is 04:00–20:00 ET).
fn dates(e: &ChartEngine, tf: Timeframe) -> Vec<&'static str> {
    let cut = ms(next(4, 0));
    let mut d: Vec<&'static str> = candles(e, tf)
        .iter()
        .map(|c| {
            if c.t < cut {
                "2026-10-15"
            } else {
                "2026-10-16"
            }
        })
        .collect();
    d.dedup();
    d
}

fn yesterday_then_premarket(e: &mut ChartEngine) {
    apply_all(
        e,
        &[
            ev(trade(at(10, 0, 0, 0), "3.18", 100, 1)),
            ev(trade(at(15, 0, 0, 0), "3.30", 100, 2)),
            ev(trade(next(8, 0), "3.40", 100, 3)),
        ],
    );
    e.advance_clock(next(9, 0));
}

/// Before today's open the previous session is still on the chart (the pre-market dashboard
/// shows it); from 09:30 it is dropped, the way a server restarted after the open never loads it.
#[test]
fn prior_dates_go_at_the_latest_dates_open() {
    let mut e = ChartEngine::default();
    yesterday_then_premarket(&mut e);
    for tf in Timeframe::ALL {
        assert_eq!(
            dates(&e, tf),
            ["2026-10-15", "2026-10-16"],
            "{tf} before the open"
        );
    }
    e.apply(&ev(trade(next(9, 31), "3.50", 100, 4)));
    for tf in Timeframe::ALL {
        assert_eq!(dates(&e, tf), ["2026-10-16"], "{tf} after the open");
    }
    assert!(
        e.sessions(SYM).iter().all(|s| s.date == next_day()),
        "the previous date's profiles go with its candles: {:?}",
        e.sessions(SYM)
    );
    // Today's candles are untouched.
    let c = candles(&e, Timeframe::M1);
    assert_eq!(c.len(), 2);
    assert_eq!(c.iter().map(|c| c.v).sum::<u64>(), 200);
}

/// The clock alone does it too: a symbol that printed pre-market and not since still loses the
/// previous date once the market has opened.
#[test]
fn the_clock_drops_prior_dates_without_a_print() {
    let mut e = ChartEngine::default();
    yesterday_then_premarket(&mut e);
    e.apply(&ev(trade(next(9, 29), "3.45", 100, 4)));
    e.advance_clock(next(9, 45));
    assert_eq!(dates(&e, Timeframe::M5), ["2026-10-16"]);
}

#[test]
fn prior_dates_can_be_kept() {
    let mut e = ChartEngine::new(EngineConfig {
        drop_prior_dates_at_open: false,
        ..Default::default()
    })
    .unwrap();
    yesterday_then_premarket(&mut e);
    e.apply(&ev(trade(next(9, 31), "3.50", 100, 4)));
    assert_eq!(dates(&e, Timeframe::M5), ["2026-10-15", "2026-10-16"]);
}

fn cancel_on(ts: UnixNanos, exchange: &str, id: u64, price: &str, size: u64) -> MarketEvent {
    MarketEvent::Cancel(Cancel {
        symbol: SYM.into(),
        event_ts: ts,
        receive_ts: None,
        source: Source::Fixture,
        exchange: exchange.into(),
        tape: Tape::C,
        trade_id: id,
        price: px(price),
        size,
        action: CancelAction::Cancel,
    })
}

/// Two TRF prints sharing one provider id (venue `D`), and a venue code longer than the inline
/// space: a cancel resolves to the one whose price and size it names, and a later cancel of the
/// other still finds it once the first has left the index.
#[test]
fn shared_ids_and_long_venue_codes_resolve_exactly() {
    for venue in ["D", "A-VENUE-CODE-LONGER-THAN-INLINE"] {
        let mut e = ChartEngine::default();
        apply_all(
            &mut e,
            &[
                ev(trade(at(10, 0, 1, 0), "3.10", 100, 7).with_exchange(venue)),
                ev(trade(at(10, 0, 2, 0), "3.20", 200, 7).with_exchange(venue)),
            ],
        );
        assert_eq!(e.retained_trades(SYM), 2);
        let (_, rev) = e.apply_revising(&cancel_on(at(10, 0, 3, 0), venue, 7, "3.20", 200));
        let rev = rev.expect("resolved to the 3.20 print");
        assert_eq!(rev.removed.price, Price::from_cents(320), "{venue}");
        assert_eq!(e.retained_trades(SYM), 1);
        let (_, rev) = e.apply_revising(&cancel_on(at(10, 0, 4, 0), venue, 7, "3.10", 100));
        assert_eq!(rev.expect("the other one").removed.size, 100, "{venue}");
        assert_eq!(e.retained_trades(SYM), 0);
        assert!(candles(&e, Timeframe::S15).is_empty(), "{venue}");
    }
}

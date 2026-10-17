use super::*;
use crate::wire::{ModifyCommand, PlaceCommand};
use serde_json::json;

const T0: i64 = 1_760_540_400_000;

fn c(cents: i64) -> Price {
    Price::from_cents(cents)
}

fn known(s: &str) -> bool {
    matches!(s, "SMPL" | "FXLO")
}

fn order(side: Side, order_type: OrderType, qty: i64) -> NewOrder {
    NewOrder {
        client_order_id: None,
        symbol: "SMPL".into(),
        side,
        order_type,
        qty,
        limit_price: None,
        stop_price: None,
    }
}

fn limit(side: Side, qty: i64, l: Price) -> NewOrder {
    NewOrder {
        limit_price: Some(l),
        ..order(side, OrderType::Limit, qty)
    }
}

fn stop(side: Side, qty: i64, s: Price) -> NewOrder {
    NewOrder {
        stop_price: Some(s),
        ..order(side, OrderType::Stop, qty)
    }
}

fn stop_limit(side: Side, qty: i64, s: Price, l: Price) -> NewOrder {
    NewOrder {
        stop_price: Some(s),
        limit_price: Some(l),
        ..order(side, OrderType::StopLimit, qty)
    }
}

fn place(b: &mut Broker, o: NewOrder) -> String {
    b.place(o, T0, known)
}

fn status(b: &Broker, id: &str) -> Status {
    b.order(id).unwrap().status
}

fn reject(b: &mut Broker, o: NewOrder) -> Option<RejectReason> {
    let id = place(b, o);
    assert_eq!(status(b, &id), Status::Rejected);
    b.order(&id).unwrap().reject_reason
}

fn dollars(micros: i128) -> f64 {
    micros as f64 / 1e6
}

// --- matching table ------------------------------------------------------------------------

#[test]
fn market_fills_on_the_first_trade_after_placement_at_that_price() {
    let mut b = Broker::new();
    let id = place(&mut b, order(Side::Buy, OrderType::Market, 100));
    assert_eq!(status(&b, &id), Status::Open);
    // Another symbol's trade does nothing.
    assert!(!b.on_trade("FXLO", c(50), T0 + 1).changed);
    let out = b.on_trade("SMPL", c(338), T0 + 2);
    assert_eq!(
        out.fills,
        vec![Fill {
            order_num: 1,
            symbol: "SMPL".into(),
            side: Side::Buy,
            qty: 100,
            price: c(338),
            t: T0 + 2
        }]
    );
    let o = b.order(&id).unwrap();
    assert_eq!(
        (o.status, o.filled_qty, o.avg_fill_price, o.updated_at),
        (Status::Filled, 100, Some(c(338)), T0 + 2)
    );
}

#[test]
fn buy_limit_fills_only_when_traded_through_at_the_limit() {
    let mut b = Broker::new();
    let id = place(&mut b, limit(Side::Buy, 100, c(338)));
    assert!(b.on_trade("SMPL", c(340), T0).fills.is_empty());
    // A touch at the limit does not fill.
    assert!(b.on_trade("SMPL", c(338), T0).fills.is_empty());
    assert_eq!(status(&b, &id), Status::Open);
    let out = b.on_trade("SMPL", c(337), T0 + 5);
    assert_eq!(out.fills.len(), 1);
    assert_eq!(
        out.fills[0].price,
        c(338),
        "fills at the limit, not the print"
    );
    assert_eq!(status(&b, &id), Status::Filled);
}

#[test]
fn sell_limit_fills_only_above_the_limit() {
    let mut b = Broker::new();
    let id = place(&mut b, limit(Side::Sell, 10, c(400)));
    assert!(b.on_trade("SMPL", c(400), T0).fills.is_empty(), "touch");
    assert!(b.on_trade("SMPL", c(390), T0).fills.is_empty());
    let out = b.on_trade("SMPL", c(401), T0);
    assert_eq!(out.fills[0].price, c(400));
    assert_eq!(status(&b, &id), Status::Filled);
}

#[test]
fn buy_stop_triggers_at_or_above_and_fills_on_that_trade_at_the_print() {
    let mut b = Broker::new();
    let id = place(&mut b, stop(Side::Buy, 100, c(350)));
    assert!(!b.on_trade("SMPL", c(349), T0).changed);
    let out = b.on_trade("SMPL", c(350), T0 + 1);
    assert_eq!(out.fills.len(), 1, "a print at the stop triggers it");
    assert_eq!(out.fills[0].price, c(350));
    assert_eq!(status(&b, &id), Status::Filled);

    let id = place(&mut b, stop(Side::Buy, 100, c(350)));
    let out = b.on_trade("SMPL", c(362), T0 + 2);
    assert_eq!(
        out.fills[0].price,
        c(362),
        "gap through: fills at the print"
    );
    assert_eq!(status(&b, &id), Status::Filled);
}

#[test]
fn sell_stop_triggers_at_or_below() {
    let mut b = Broker::new();
    let id = place(&mut b, stop(Side::Sell, 100, c(300)));
    assert!(!b.on_trade("SMPL", c(301), T0).changed);
    let out = b.on_trade("SMPL", c(295), T0);
    assert_eq!(out.fills[0].price, c(295));
    assert_eq!(status(&b, &id), Status::Filled);
    let id = place(&mut b, stop(Side::Sell, 1, c(300)));
    assert_eq!(b.on_trade("SMPL", c(300), T0).fills.len(), 1);
    assert_eq!(status(&b, &id), Status::Filled);
}

#[test]
fn stop_limit_triggers_then_is_a_limit_checked_from_the_next_trade() {
    let mut b = Broker::new();
    let id = place(&mut b, stop_limit(Side::Buy, 100, c(350), c(360)));
    assert!(!b.on_trade("SMPL", c(340), T0).changed);
    // Crosses the stop. 3.52 < 3.60 would fill a limit, but not on the triggering trade.
    let out = b.on_trade("SMPL", c(352), T0 + 1);
    assert!(out.changed && out.fills.is_empty());
    let o = b.order(&id).unwrap();
    assert_eq!((o.status, o.updated_at), (Status::Triggered, T0 + 1));
    // Now an ordinary buy limit 3.60: touch does not fill, through does, at the limit.
    assert!(b.on_trade("SMPL", c(360), T0 + 2).fills.is_empty());
    let out = b.on_trade("SMPL", c(355), T0 + 3);
    assert_eq!(out.fills[0].price, c(360));
    assert_eq!(status(&b, &id), Status::Filled);
}

#[test]
fn sell_stop_limit_triggers_downward() {
    let mut b = Broker::new();
    let id = place(&mut b, stop_limit(Side::Sell, 100, c(300), c(295)));
    assert!(!b.on_trade("SMPL", c(301), T0).changed);
    assert!(b.on_trade("SMPL", c(299), T0).fills.is_empty());
    assert_eq!(status(&b, &id), Status::Triggered);
    assert!(b.on_trade("SMPL", c(295), T0).fills.is_empty(), "touch");
    assert_eq!(b.on_trade("SMPL", c(296), T0).fills[0].price, c(295));
}

#[test]
fn within_one_trade_orders_fill_in_creation_order() {
    let mut b = Broker::new();
    let a = place(&mut b, limit(Side::Buy, 10, c(340)));
    let m = place(&mut b, order(Side::Sell, OrderType::Market, 5));
    let s = place(&mut b, stop(Side::Sell, 7, c(340)));
    let out = b.on_trade("SMPL", c(330), T0);
    let ids: Vec<String> = out
        .fills
        .iter()
        .map(|f| format!("o-{}", f.order_num))
        .collect();
    assert_eq!(ids, vec![a, m, s]);
    // Net: +10 at 3.40, -5 at 3.30, -7 at 3.30 → short 2.
    assert_eq!(b.position("SMPL").unwrap().qty, -2);
}

#[test]
fn every_order_fills_in_full_on_one_trade() {
    let mut b = Broker::new();
    let id = place(&mut b, limit(Side::Buy, 1_000_000, c(100)));
    let out = b.on_trade("SMPL", c(99), T0);
    assert_eq!(out.fills[0].qty, 1_000_000);
    assert_eq!(b.order(&id).unwrap().filled_qty, 1_000_000);
}

// --- validation ----------------------------------------------------------------------------

#[test]
fn tick_grid_is_cents_from_one_dollar_and_hundredths_of_a_cent_below() {
    assert!(on_tick_grid(c(100)));
    assert!(on_tick_grid(c(338)));
    assert!(!on_tick_grid(Price::from_micros(3_385_000)));
    assert!(!on_tick_grid(Price::from_micros(1_000_100)), "$1.0001");
    assert!(on_tick_grid(Price::from_micros(999_900)), "$0.9999");
    assert!(on_tick_grid(Price::from_micros(152_500)), "$0.1525");
    assert!(!on_tick_grid(Price::from_micros(152_550)), "$0.15255");
}

#[test]
fn sub_penny_is_rejected_above_and_below_a_dollar() {
    let mut b = Broker::new();
    let above = limit(Side::Buy, 1, Price::from_micros(3_385_000));
    assert_eq!(reject(&mut b, above), Some(RejectReason::SubPenny));
    let below = limit(Side::Buy, 1, Price::from_micros(152_550));
    assert_eq!(reject(&mut b, below), Some(RejectReason::SubPenny));
    // The stop of a stop-limit is checked too.
    let sl = stop_limit(Side::Buy, 1, Price::from_micros(3_385_000), c(340));
    assert_eq!(reject(&mut b, sl), Some(RejectReason::SubPenny));
    // On-grid sub-dollar is fine.
    let ok = place(&mut b, limit(Side::Buy, 1, Price::from_micros(152_500)));
    assert_eq!(status(&b, &ok), Status::Open);
}

#[test]
fn bad_qty_unknown_symbol_missing_and_bad_price_are_rejected_into_state() {
    let mut b = Broker::new();
    for q in [0, -5, MAX_QTY + 1] {
        assert_eq!(
            reject(&mut b, order(Side::Buy, OrderType::Market, q)),
            Some(RejectReason::BadQty)
        );
    }
    let unknown = NewOrder {
        symbol: "NOPE".into(),
        ..order(Side::Buy, OrderType::Market, 1)
    };
    assert_eq!(reject(&mut b, unknown), Some(RejectReason::UnknownSymbol));
    assert_eq!(
        reject(&mut b, order(Side::Buy, OrderType::Limit, 1)),
        Some(RejectReason::MissingPrice)
    );
    assert_eq!(
        reject(&mut b, order(Side::Sell, OrderType::Stop, 1)),
        Some(RejectReason::MissingPrice)
    );
    let half = NewOrder {
        limit_price: Some(c(340)),
        ..order(Side::Buy, OrderType::StopLimit, 1)
    };
    assert_eq!(reject(&mut b, half), Some(RejectReason::MissingPrice));
    assert_eq!(
        reject(&mut b, limit(Side::Buy, 1, Price::ZERO)),
        Some(RejectReason::BadPrice)
    );
    // Rejected orders never fill and live in the finished list.
    assert!(!b.on_trade("SMPL", c(1), T0).changed);
    assert!(b.orders().iter().all(|o| o.status == Status::Rejected));
}

#[test]
fn prices_an_order_type_does_not_use_are_dropped() {
    let mut b = Broker::new();
    let o = NewOrder {
        limit_price: Some(c(340)),
        stop_price: Some(Price::from_micros(1)), // would be sub-penny if it counted
        ..order(Side::Buy, OrderType::Market, 1)
    };
    let id = place(&mut b, o);
    let o = b.order(&id).unwrap();
    assert_eq!(
        (o.status, o.limit_price, o.stop_price),
        (Status::Open, None, None)
    );
}

// --- modify / cancel -----------------------------------------------------------------------

#[test]
fn modify_keeps_the_id_and_the_queue_position() {
    let mut b = Broker::new();
    let a = place(&mut b, limit(Side::Buy, 10, c(330)));
    let x = place(&mut b, limit(Side::Buy, 10, c(340)));
    let seq = b.seq();
    b.modify(
        &a,
        Modify {
            qty: Some(20),
            limit_price: Some(c(345)),
            ..Modify::default()
        },
        T0 + 9,
    )
    .unwrap();
    assert_eq!(b.seq(), seq + 1);
    let o = b.order(&a).unwrap();
    assert_eq!(
        (o.qty, o.limit_price, o.updated_at),
        (20, Some(c(345)), T0 + 9)
    );
    let out = b.on_trade("SMPL", c(335), T0 + 10);
    assert_eq!(
        out.fills.iter().map(|f| f.order_num).collect::<Vec<_>>(),
        vec![1, 2],
        "{a} still fills before {x}"
    );
}

#[test]
fn modify_of_a_non_open_order_is_not_open_and_changes_nothing() {
    let mut b = Broker::new();
    let filled = place(&mut b, order(Side::Buy, OrderType::Market, 1));
    b.on_trade("SMPL", c(300), T0);
    let cancelled = place(&mut b, limit(Side::Buy, 1, c(100)));
    b.cancel(&cancelled, T0).unwrap();
    let rejected = place(&mut b, order(Side::Buy, OrderType::Market, 0));
    let before = b.state();
    let m = Modify {
        qty: Some(5),
        ..Modify::default()
    };
    for id in [
        &filled,
        &cancelled,
        &rejected,
        &"o-999".to_string(),
        &"x".to_string(),
    ] {
        assert_eq!(b.modify(id, m.clone(), T0), Err(CommandError::NotOpen));
        assert_eq!(b.cancel(id, T0), Err(CommandError::NotOpen));
    }
    assert_eq!(b.state(), before);
    assert_eq!(CommandError::NotOpen.code(), "not_open");
}

#[test]
fn modify_validates_like_place_and_leaves_the_order_alone_on_error() {
    let mut b = Broker::new();
    let id = place(&mut b, limit(Side::Buy, 10, c(330)));
    let m_order = place(&mut b, order(Side::Buy, OrderType::Market, 10));
    let before = b.state();
    assert_eq!(
        b.modify(
            &id,
            Modify {
                qty: Some(0),
                ..Modify::default()
            },
            T0
        ),
        Err(CommandError::Invalid(RejectReason::BadQty))
    );
    assert_eq!(
        b.modify(
            &id,
            Modify {
                limit_price: Some(Price::from_micros(3_301_000)),
                ..Modify::default()
            },
            T0
        ),
        Err(CommandError::Invalid(RejectReason::SubPenny))
    );
    assert!(matches!(
        b.modify(&id, Modify::default(), T0),
        Err(CommandError::BadOrder(_))
    ));
    assert!(matches!(
        b.modify(
            &id,
            Modify {
                stop_price: Some(c(300)),
                ..Modify::default()
            },
            T0
        ),
        Err(CommandError::BadOrder(_))
    ));
    assert!(matches!(
        b.modify(
            &m_order,
            Modify {
                limit_price: Some(c(300)),
                ..Modify::default()
            },
            T0
        ),
        Err(CommandError::BadOrder(_))
    ));
    assert_eq!(b.state(), before);
}

#[test]
fn a_triggered_stop_limit_can_move_its_limit_but_not_its_stop() {
    let mut b = Broker::new();
    let id = place(&mut b, stop_limit(Side::Buy, 10, c(350), c(355)));
    b.on_trade("SMPL", c(351), T0);
    assert_eq!(status(&b, &id), Status::Triggered);
    let stop_move = Modify {
        stop_price: Some(c(352)),
        ..Modify::default()
    };
    assert!(matches!(
        b.modify(&id, stop_move, T0),
        Err(CommandError::BadOrder(_))
    ));
    let limit_move = Modify {
        limit_price: Some(c(360)),
        ..Modify::default()
    };
    b.modify(&id, limit_move, T0).unwrap();
    assert_eq!(b.on_trade("SMPL", c(358), T0).fills[0].price, c(360));
}

#[test]
fn cancel_finishes_a_working_order() {
    let mut b = Broker::new();
    let id = place(&mut b, stop_limit(Side::Sell, 10, c(300), c(299)));
    b.cancel(&id, T0 + 3).unwrap();
    let o = b.order(&id).unwrap();
    assert_eq!((o.status, o.updated_at), (Status::Cancelled, T0 + 3));
    assert!(!b.on_trade("SMPL", c(100), T0).changed);
}

// --- positions and P&L ---------------------------------------------------------------------

fn fill_market(b: &mut Broker, side: Side, qty: i64, cents: i64) {
    place(b, order(side, OrderType::Market, qty));
    assert_eq!(b.on_trade("SMPL", c(cents), T0).fills.len(), 1);
}

#[test]
fn average_price_and_realized_pnl_on_a_long() {
    let mut b = Broker::new();
    fill_market(&mut b, Side::Buy, 100, 300);
    fill_market(&mut b, Side::Buy, 100, 400);
    let p = b.position("SMPL").unwrap();
    assert_eq!(p.qty, 200);
    assert_eq!(p.avg_price_micros(), 3_500_000.0);
    assert_eq!(dollars(b.cash()), 100_000.0 - 300.0 - 400.0);
    // Sell 50 at 4.00: realizes (4.00 − 3.50) × 50 = 25; the average is unchanged.
    fill_market(&mut b, Side::Sell, 50, 400);
    let p = b.position("SMPL").unwrap();
    assert_eq!((p.qty, p.avg_price_micros()), (150, 3_500_000.0));
    assert_eq!(dollars(p.realized), 25.0);
    assert_eq!(dollars(b.realized()), 25.0);
    assert_eq!(dollars(b.cash()), 100_000.0 - 700.0 + 200.0);
}

#[test]
fn short_positions_are_signed_and_profit_when_covered_lower() {
    let mut b = Broker::new();
    fill_market(&mut b, Side::Sell, 100, 500);
    let p = b.position("SMPL").unwrap();
    assert_eq!((p.qty, p.avg_price_micros()), (-100, 5_000_000.0));
    assert_eq!(dollars(b.cash()), 100_500.0, "a short sale credits cash");
    fill_market(&mut b, Side::Sell, 100, 300);
    assert_eq!(b.position("SMPL").unwrap().avg_price_micros(), 4_000_000.0);
    // Cover 50 at 3.50: realizes (4.00 − 3.50) × 50 = 25.
    fill_market(&mut b, Side::Buy, 50, 350);
    let p = b.position("SMPL").unwrap();
    assert_eq!((p.qty, dollars(p.realized)), (-150, 25.0));
}

#[test]
fn flipping_long_to_short_through_zero() {
    let mut b = Broker::new();
    fill_market(&mut b, Side::Buy, 100, 300);
    // Sell 150 at 3.20: closes 100 for +20, opens short 50 at 3.20.
    fill_market(&mut b, Side::Sell, 150, 320);
    let p = b.position("SMPL").unwrap();
    assert_eq!((p.qty, p.avg_price_micros()), (-50, 3_200_000.0));
    assert_eq!(dollars(p.realized), 20.0);
    // And back: buy 80 at 3.10 closes 50 for +5, opens long 30 at 3.10.
    fill_market(&mut b, Side::Buy, 80, 310);
    let p = b.position("SMPL").unwrap();
    assert_eq!((p.qty, p.avg_price_micros()), (30, 3_100_000.0));
    assert_eq!(dollars(p.realized), 25.0);
    assert_eq!(dollars(b.realized()), 25.0);
    assert_eq!(
        dollars(b.cash()),
        100_000.0 - 300.0 + 480.0 - 248.0,
        "cash moves by the full traded value"
    );
}

#[test]
fn a_flat_position_is_dropped_but_its_pnl_stays_in_the_account() {
    let mut b = Broker::new();
    fill_market(&mut b, Side::Buy, 100, 300);
    fill_market(&mut b, Side::Sell, 100, 290);
    assert!(b.position("SMPL").is_none());
    assert_eq!(b.positions().count(), 0);
    assert_eq!(dollars(b.realized()), -10.0);
    assert_eq!(dollars(b.cash()), 99_990.0);
    // A new position starts its own realized P&L from zero.
    fill_market(&mut b, Side::Buy, 10, 300);
    assert_eq!(b.position("SMPL").unwrap().realized, 0);
    assert_eq!(dollars(b.realized()), -10.0);
}

#[test]
fn proportional_basis_with_an_uneven_average() {
    let mut b = Broker::new();
    fill_market(&mut b, Side::Buy, 1, 100);
    fill_market(&mut b, Side::Buy, 2, 101);
    // avg = 3.02 / 3 = 1.00666…; selling everything leaves no residue.
    fill_market(&mut b, Side::Sell, 1, 102);
    fill_market(&mut b, Side::Sell, 2, 102);
    assert!(b.position("SMPL").is_none());
    assert_eq!(dollars(b.realized()), 0.04);
    assert_eq!(b.cash(), STARTING_CASH_MICROS + 40_000);
}

// --- retention, reset, seq -----------------------------------------------------------------

#[test]
fn only_the_last_fifty_finished_orders_are_kept() {
    let mut b = Broker::new();
    let working = place(&mut b, limit(Side::Buy, 1, c(100)));
    for _ in 0..60 {
        place(&mut b, order(Side::Buy, OrderType::Market, 0));
    }
    let orders = b.orders();
    assert_eq!(orders.len(), 1 + FINISHED_RETAINED);
    assert_eq!(orders[0].id(), working, "working orders are never aged out");
    assert_eq!(orders[1].num, 12, "o-2..o-11 aged out");
    assert!(b.order("o-2").is_none());
    assert_eq!(b.cancel("o-2", T0), Err(CommandError::NotOpen));
}

#[test]
fn reset_is_a_flat_account_with_no_orders_and_ids_keep_counting() {
    let mut b = Broker::new();
    fill_market(&mut b, Side::Buy, 100, 300);
    fill_market(&mut b, Side::Sell, 40, 310);
    place(&mut b, limit(Side::Buy, 1, c(100)));
    let seq = b.seq();
    b.reset();
    assert!(b.seq() > seq);
    assert_eq!(b.cash(), STARTING_CASH_MICROS);
    assert_eq!(b.realized(), 0);
    assert!(b.orders().is_empty());
    assert_eq!(b.positions().count(), 0);
    assert!(!b.on_trade("SMPL", c(1), T0).changed, "old orders are gone");
    assert_eq!(place(&mut b, limit(Side::Buy, 1, c(100))), "o-4");
}

#[test]
fn seq_moves_only_on_change() {
    let mut b = Broker::new();
    assert_eq!(b.seq(), 0);
    place(&mut b, limit(Side::Buy, 1, c(100)));
    assert_eq!(b.seq(), 1);
    b.on_trade("SMPL", c(200), T0);
    b.on_trade("FXLO", c(50), T0);
    assert_eq!(b.seq(), 1);
    let _ = b.cancel("o-9", T0);
    assert_eq!(b.seq(), 1);
    b.on_trade("SMPL", c(99), T0);
    assert_eq!(b.seq(), 2);
}

#[test]
fn deterministic_for_the_same_inputs() {
    let run = || {
        let mut b = Broker::new();
        place(&mut b, stop_limit(Side::Buy, 10, c(310), c(320)));
        place(&mut b, limit(Side::Sell, 5, c(330)));
        for (i, p) in [300, 305, 311, 318, 331, 325].into_iter().enumerate() {
            b.on_trade("SMPL", c(p), T0 + i as i64);
        }
        serde_json::to_string(&b.state()).unwrap()
    };
    assert_eq!(run(), run());
}

// --- wire ----------------------------------------------------------------------------------

#[test]
fn state_json_matches_the_contract() {
    let mut b = Broker::new();
    let cmd: PlaceCommand = serde_json::from_value(json!({
        "clientOrderId": "c-1697", "symbol": " smpl", "side": "buy",
        "orderType": "limit", "qty": 100, "limitPrice": 3.38
    }))
    .unwrap();
    b.place(cmd.into_new_order(), T0, known);
    let v = serde_json::to_value(b.state()).unwrap();
    assert_eq!(
        v,
        json!({
            "type": "paper.state", "simulated": true, "seq": 1,
            "account": {"startingCash": 100000.0, "cash": 100000.0, "realizedPnl": 0.0},
            "orders": [{
                "id": "o-1", "clientOrderId": "c-1697", "symbol": "SMPL", "side": "buy",
                "orderType": "limit", "qty": 100, "filledQty": 0, "avgFillPrice": null,
                "limitPrice": 3.38, "stopPrice": null, "status": "open", "rejectReason": null,
                "createdAt": T0, "updatedAt": T0
            }],
            "positions": []
        })
    );
    let out = b.on_trade("SMPL", c(337), T0 + 12_345);
    let fill = serde_json::to_value(wire::PaperFill::from(&out.fills[0])).unwrap();
    assert_eq!(
        fill,
        json!({"type": "paper.fill", "simulated": true, "orderId": "o-1", "symbol": "SMPL",
               "side": "buy", "qty": 100, "price": 3.38, "t": T0 + 12_345})
    );
    let v = serde_json::to_value(b.state()).unwrap();
    assert_eq!(v["account"]["cash"], 99_662.0);
    assert_eq!(
        v["positions"],
        json!([{"symbol": "SMPL", "qty": 100, "avgPrice": 3.38, "realizedPnl": 0.0}])
    );
    assert_eq!(v["orders"][0]["status"], "filled");
    assert_eq!(v["orders"][0]["avgFillPrice"], 3.38);
}

#[test]
fn wire_conversion_rounds_to_micros_then_the_broker_checks_the_grid() {
    let mut b = Broker::new();
    let place_json = |b: &mut Broker, v: serde_json::Value| {
        let cmd: PlaceCommand = serde_json::from_value(v).unwrap();
        let id = b.place(cmd.into_new_order(), T0, known);
        b.order(&id).unwrap().clone()
    };
    // Float noise around a valid price is not a rejection.
    let o = place_json(
        &mut b,
        json!({"symbol":"SMPL","side":"sell","orderType":"stop_limit","qty":1,
               "stopPrice": 0.1 + 0.2, "limitPrice": 0.29}),
    );
    assert_eq!(o.status, Status::Open);
    assert_eq!(o.stop_price, Some(Price::from_micros(300_000)));
    let o = place_json(
        &mut b,
        json!({"symbol":"SMPL","side":"buy","orderType":"limit","qty":1,"limitPrice":3.385}),
    );
    assert_eq!(o.reject_reason, Some(RejectReason::SubPenny));
    let o = place_json(
        &mut b,
        json!({"symbol":"SMPL","side":"buy","orderType":"market","qty":1.5}),
    );
    assert_eq!(o.reject_reason, Some(RejectReason::BadQty));
    let o = place_json(
        &mut b,
        json!({"symbol":"SMPL","side":"buy","orderType":"limit","qty":1,"limitPrice":1e300}),
    );
    assert_eq!(o.reject_reason, Some(RejectReason::BadPrice));
    // Shape errors are not orders at all.
    for bad in [
        json!({"symbol":"SMPL","side":"hold","orderType":"market","qty":1}),
        json!({"symbol":"SMPL","side":"buy","orderType":"trailing","qty":1}),
        json!({"symbol":"SMPL","side":"buy","orderType":"market"}),
        json!({"symbol":"SMPL","side":"buy","orderType":"market","qty":"1"}),
    ] {
        assert!(serde_json::from_value::<PlaceCommand>(bad).is_err());
    }
    let (id, m) = serde_json::from_value::<ModifyCommand>(json!({"id":"o-7","limitPrice":3.36}))
        .unwrap()
        .into_modify();
    assert_eq!(id, "o-7");
    assert_eq!(
        m,
        Modify {
            limit_price: Some(c(336)),
            ..Modify::default()
        }
    );
}

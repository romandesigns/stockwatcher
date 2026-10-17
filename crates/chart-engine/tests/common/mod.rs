//! Deterministic fixtures. Synthetic prints for software tests only; never market evidence.
#![allow(dead_code)]

use chart_engine::wire::{CandleWire, ChartUpdate};
use chart_engine::{ChartEngine, Timeframe};
use chrono::NaiveDate;
use market_events::session::et;
use market_events::{
    Cancel, CancelAction, Correction, MarketEvent, Price, Source, Tape, Trade, UnixNanos,
};

pub const SYM: &str = "SMPL";

/// Thursday 2026-10-15, a full trading day (EDT).
pub fn day() -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 10, 15).unwrap()
}

/// ET wall clock on [`day`].
pub fn at(h: u32, m: u32, s: u32, ms: u32) -> UnixNanos {
    et(day(), h, m, s, ms).unwrap()
}

pub fn px(s: &str) -> Price {
    s.parse().unwrap()
}

pub fn trade(ts: UnixNanos, price: &str, size: u64, seq: u64) -> Trade {
    Trade::new(SYM, ts, px(price), size, seq)
        .with_exchange("V")
        .with_tape(Tape::C)
        .with_source(Source::Fixture)
}

pub fn ev(t: Trade) -> MarketEvent {
    MarketEvent::Trade(t)
}

pub fn cancel(ts: UnixNanos, trade_id: u64) -> MarketEvent {
    MarketEvent::Cancel(Cancel {
        symbol: SYM.into(),
        event_ts: ts,
        receive_ts: None,
        source: Source::Fixture,
        exchange: "V".into(),
        tape: Tape::C,
        trade_id,
        price: Price::ZERO,
        size: 0,
        action: CancelAction::Cancel,
    })
}

pub fn correction(
    ts: UnixNanos,
    original_id: u64,
    corrected_id: u64,
    price: &str,
    size: u64,
    conds: &[&str],
) -> MarketEvent {
    MarketEvent::Correction(Correction {
        symbol: SYM.into(),
        event_ts: ts,
        receive_ts: None,
        source: Source::Fixture,
        exchange: "V".into(),
        tape: Tape::C,
        original_id,
        original_price: Price::ZERO,
        original_size: 0,
        original_conditions: vec![],
        corrected_id,
        corrected_price: px(price),
        corrected_size: size,
        corrected_conditions: conds.iter().map(|c| c.to_string()).collect(),
    })
}

pub fn apply_all(engine: &mut ChartEngine, events: &[MarketEvent]) -> Vec<ChartUpdate> {
    events.iter().flat_map(|e| engine.apply(e)).collect()
}

pub fn candles(engine: &ChartEngine, tf: Timeframe) -> Vec<CandleWire> {
    engine
        .snapshot(SYM, tf, None)
        .map(|s| s.candles)
        .unwrap_or_default()
}

/// `(t, o, h, l, c, v)` with prices as strings for exact comparison.
pub fn ohlcv(c: &CandleWire) -> (i64, String, String, String, String, u64) {
    let f = |v: f64| Price::from_f64_rounded(v).unwrap().to_string();
    (c.t, f(c.o), f(c.h), f(c.l), f(c.c), c.v)
}

pub fn ms(ts: UnixNanos) -> i64 {
    ts.millis()
}

pub fn tape_counted(updates: &[ChartUpdate]) -> Vec<String> {
    updates
        .iter()
        .filter_map(|u| u.as_tape())
        .map(|t| t.counted.clone())
        .collect()
}

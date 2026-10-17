//! Minute bars built by the real chart engine (same rules as the chart), plus a VWAP book.
//!
//! Alpaca has no 15 s bars; its minute bar is the chart's 1m candle. The `vw` field is computed
//! here from the prints the engine counted toward volume (its tape `counted`), net of cancels and
//! corrections. Alpaca does not document which prints enter `vw`, so this is the simulator's
//! documented interpretation, not a claim of parity.

use chart_engine::wire::{CandleWire, ChartUpdate};
use chart_engine::{ChartEngine, Timeframe};
use market_events::{MarketEvent, TradeId, UnixNanos, trade_effect};
use std::collections::{BTreeMap, HashMap};

const MINUTE_MS: i64 = 60_000;

#[derive(Default)]
pub struct BarBook {
    engine: ChartEngine,
    /// symbol → minute start (ms) → (Σ price_micros × size, Σ size)
    vw: HashMap<String, BTreeMap<i64, (i128, u64)>>,
    /// Counted prints, for cancel/correction adjustments: id → (symbol, minute, price µ$, size)
    counted: HashMap<TradeId, (String, i64, i64, u64)>,
}

impl BarBook {
    pub fn engine(&self) -> &ChartEngine {
        &self.engine
    }

    fn add(&mut self, sym: &str, minute: i64, p: i64, s: u64, sign: i128) {
        let e = self
            .vw
            .entry(sym.to_string())
            .or_default()
            .entry(minute)
            .or_default();
        e.0 += sign * i128::from(p) * i128::from(s);
        if sign > 0 {
            e.1 += s;
        } else {
            e.1 = e.1.saturating_sub(s);
        }
    }

    pub fn apply(&mut self, e: &MarketEvent) -> Vec<ChartUpdate> {
        let updates = self.engine.apply(e);
        match e {
            MarketEvent::Trade(t) => {
                let counted = updates.iter().any(|u| {
                    u.as_tape().is_some_and(|row| {
                        row.counted == "ohlc_and_volume" || row.counted == "volume_only"
                    })
                });
                if counted {
                    let minute = t.event_ts.millis().div_euclid(MINUTE_MS) * MINUTE_MS;
                    self.add(&t.symbol, minute, t.price.micros(), t.size, 1);
                    self.counted
                        .insert(t.id(), (t.symbol.clone(), minute, t.price.micros(), t.size));
                }
            }
            MarketEvent::Cancel(c) => {
                if let Some((sym, m, p, s)) = self.counted.remove(&c.target_trade_id()) {
                    self.add(&sym, m, p, s, -1);
                }
            }
            MarketEvent::Correction(c) => {
                if let Some((sym, m, p, s)) = self.counted.remove(&c.original_trade_id()) {
                    self.add(&sym, m, p, s, -1);
                    if trade_effect(&c.corrected_conditions, c.tape).updates_volume {
                        let p2 = c.corrected_price.micros();
                        self.add(&sym, m, p2, c.corrected_size, 1);
                        let mut id = c.original_trade_id();
                        id.seq = c.corrected_id;
                        self.counted.insert(id, (sym, m, p2, c.corrected_size));
                    }
                }
            }
            _ => {}
        }
        updates
    }

    pub fn advance(&mut self, t: UnixNanos) -> Vec<ChartUpdate> {
        self.engine.advance_clock(t)
    }

    /// VWAP of a symbol's minute, or the close when nothing was counted (e.g. after a cancel).
    pub fn vw(&self, sym: &str, c: &CandleWire) -> f64 {
        match self.vw.get(sym).and_then(|m| m.get(&c.t)) {
            Some((pv, v)) if *v > 0 => (*pv as f64) / (*v as f64) / 1e6,
            _ => c.c,
        }
    }

    /// Drops VWAP state for minutes before `before_ms` (they can no longer change within the
    /// engine's retention window).
    pub fn prune(&mut self, before_ms: i64) {
        for m in self.vw.values_mut() {
            *m = m.split_off(&before_ms);
        }
        self.counted.retain(|_, (_, m, _, _)| *m >= before_ms);
    }

    /// Every retained 1m candle of a symbol.
    pub fn candles(&self, sym: &str) -> Vec<CandleWire> {
        self.engine
            .snapshot(sym, Timeframe::M1, None)
            .map(|s| s.candles)
            .unwrap_or_default()
    }
}

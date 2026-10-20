//! Minute bars built by the real chart engine (same rules as the chart), plus a VWAP book.
//!
//! Alpaca has no 15 s bars; its minute bar is the chart's 1m candle. The `vw` field is computed
//! here from the prints the engine counted toward volume (its tape `counted`), net of cancels and
//! corrections. Alpaca does not document which prints enter `vw`, so this is the simulator's
//! documented interpretation, not a claim of parity.

use chart_engine::wire::{CandleWire, ChartUpdate};
use chart_engine::{ChartEngine, Timeframe};
use market_events::{MarketEvent, UnixNanos};
use std::collections::{BTreeMap, HashMap};

const MINUTE_MS: i64 = 60_000;

#[derive(Default)]
pub struct BarBook {
    engine: ChartEngine,
    /// symbol → minute start (ms) → (Σ price_micros × size, Σ size)
    vw: HashMap<String, BTreeMap<i64, (i128, u64)>>,
}

fn minute_of(t: UnixNanos) -> i64 {
    t.millis().div_euclid(MINUTE_MS) * MINUTE_MS
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

    /// Cancels and corrections follow the engine's own [`chart_engine::Revision`] rather than a
    /// second id lookup: provider ids are not unique on venue `D`
    /// (docs/architecture/trade-identity.md), so only the engine knows which print, if any, a
    /// revision hit.
    pub fn apply(&mut self, e: &MarketEvent) -> Vec<ChartUpdate> {
        let (updates, rev) = self.engine.apply_revising(e);
        if let MarketEvent::Trade(t) = e {
            let counted = updates.iter().any(|u| {
                u.as_tape().is_some_and(|row| {
                    row.counted == "ohlc_and_volume" || row.counted == "volume_only"
                })
            });
            if counted {
                let m = minute_of(t.event_ts);
                self.add(&t.symbol, m, t.price.micros(), t.size, 1);
            }
        }
        if let Some(rev) = rev {
            let m = minute_of(rev.ts);
            if rev.removed.volume {
                let r = rev.removed;
                self.add(&rev.symbol, m, r.price.micros(), r.size, -1);
            }
            if let Some(a) = rev.added.filter(|a| a.volume) {
                self.add(&rev.symbol, m, a.price.micros(), a.size, 1);
            }
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
    }

    /// Every retained 1m candle of a symbol.
    pub fn candles(&self, sym: &str) -> Vec<CandleWire> {
        self.engine
            .snapshot(sym, Timeframe::M1, None)
            .map(|s| s.candles)
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;
    use market_events::session::et;
    use market_events::{Cancel, CancelAction, Price, Source, Tape, Trade};

    fn at(m: u32, s: u32) -> UnixNanos {
        et(NaiveDate::from_ymd_opt(2026, 10, 15).unwrap(), 10, m, s, 0).unwrap()
    }

    fn d(ts: UnixNanos, cents: i64, size: u64) -> MarketEvent {
        MarketEvent::Trade(
            Trade::new("SMPL", ts, Price::from_cents(cents), size, 1)
                .with_exchange("D")
                .with_tape(Tape::C)
                .with_source(Source::AlpacaSip),
        )
    }

    /// Two TRF prints share id 1 on venue D; the cancel names the 10:01 one by price and size.
    /// The 10:00 minute's VWAP must keep its print (an id-keyed book used to forget it).
    #[test]
    fn vwap_follows_the_print_the_engine_cancelled_not_the_last_with_that_id() {
        let mut b = BarBook::default();
        b.apply(&d(at(0, 5), 500, 100));
        b.apply(&d(at(1, 5), 520, 100));
        b.apply(&d(at(1, 6), 530, 100));
        b.apply(&MarketEvent::Cancel(Cancel {
            symbol: "SMPL".into(),
            event_ts: at(1, 30),
            receive_ts: None,
            source: Source::AlpacaSip,
            exchange: "D".into(),
            tape: Tape::C,
            trade_id: 1,
            price: Price::from_cents(520),
            size: 100,
            action: CancelAction::Cancel,
        }));
        let cs = b.candles("SMPL");
        assert_eq!((cs[0].v, b.vw("SMPL", &cs[0])), (100, 5.0));
        assert_eq!((cs[1].v, b.vw("SMPL", &cs[1])), (100, 5.3));
    }
}

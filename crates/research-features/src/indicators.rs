//! A Rust port of the chart's indicators (`packages/chart-core/src/indicators.ts`), **exact**:
//! same inputs (the chart's slots: candles and gap markers), same arithmetic in the same order in
//! `f64`, same gap rule. Every function is named `chart_*` so a research feature can never be
//! mistaken for some other definition of "EMA" or "VWAP". Parity with the TypeScript is tested
//! value by value (`tests/parity.rs`, `parity/`); see `docs/research/foundation/indicators.md`.
//!
//! Definitions (as the chart has them on 2026-10-18, origin/main e14e9ab):
//!
//! - Inputs are candle **closes** per slot; a gap slot is NaN. NaN inputs are **skipped**: they
//!   neither count toward a window nor reset any running state, and the output stays NaN there.
//! - [`chart_sma`]: mean of the last `period` candles (running sum, oldest subtracted).
//! - [`chart_ema`]: seeded with the SMA of the first `period` candles, then
//!   `v * k + prev * (1 - k)`, `k = 2 / (period + 1)`. **No session or day reset.**
//! - [`chart_vwap_hlc3`]: `Σ((h + l + c) / 3 × v) / Σ v` over **candles** (gap slots, including
//!   `volume_only_trades` gaps and their volume, are skipped), reset when the chart's trading day
//!   changes ([`chart_trading_day_of`]: 20:00 ET and later belongs to the next day). A candle with
//!   zero cumulative volume shows its close.
//! - [`chart_bollinger`]: SMA ± `mult` × **population** standard deviation of the window.
//! - [`chart_rsi`]: Wilder's RSI, simple-average seed over the first `period` changes; 100 when the
//!   average loss is 0 (50 when the average gain is 0 too).
//! - [`chart_macd`]: `EMA(fast) − EMA(slow)` where both exist, signal = EMA(signal) of that line
//!   (NaN skipped), histogram = line − signal.
//!
//! The chart computes over whatever slots it holds, **including the forming candle** (its current,
//! not final, values). [`completed_and_forming`] exposes both readings separately for research.

use chart_engine::wire::{CandleWire, GapWire};
use chrono::{Duration, NaiveDate, TimeZone, Timelike};
use chrono_tz::America::New_York;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, VecDeque};

/// One slot of the chart's index-based time axis (chart-core `Slot`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Slot {
    Candle(CandleWire),
    Gap(GapWire),
}

impl Slot {
    pub fn t(&self) -> i64 {
        match self {
            Slot::Candle(c) => c.t,
            Slot::Gap(g) => g.t,
        }
    }

    pub fn candle(&self) -> Option<&CandleWire> {
        match self {
            Slot::Candle(c) => Some(c),
            Slot::Gap(_) => None,
        }
    }
}

/// chart-core `buildSlots`: merges candles and gaps (both sorted by `t`); a candle supersedes a
/// gap at the same `t`.
pub fn chart_build_slots(candles: &[CandleWire], gaps: &[GapWire]) -> Vec<Slot> {
    let mut out = Vec::with_capacity(candles.len() + gaps.len());
    let (mut i, mut j) = (0, 0);
    while i < candles.len() || j < gaps.len() {
        let c = candles.get(i);
        let g = gaps.get(j);
        match (c, g) {
            (Some(c), _) if g.is_none_or(|g| c.t <= g.t) => {
                out.push(Slot::Candle(c.clone()));
                i += 1;
                if g.is_some_and(|g| g.t == c.t) {
                    j += 1;
                }
            }
            (_, Some(g)) => {
                out.push(Slot::Gap(g.clone()));
                j += 1;
            }
            _ => unreachable!(),
        }
    }
    out
}

/// chart-core `tradingDayOf`: the ET calendar date of `t_ms`, plus one day from 20:00 ET on.
pub fn chart_trading_day_of(t_ms: i64) -> NaiveDate {
    let local = New_York
        .timestamp_millis_opt(t_ms)
        .single()
        .expect("every UTC instant maps to one ET wall time");
    let d = local.date_naive();
    if local.hour() * 60 + local.minute() >= 20 * 60 {
        d + Duration::days(1)
    } else {
        d
    }
}

/// chart-core `sessionOf` (display classification by ET wall clock, fixed 16:00 close even on an
/// early-close day).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChartSession {
    Overnight,
    Pre,
    Regular,
    After,
}

pub fn chart_session_of(t_ms: i64) -> ChartSession {
    let local = New_York
        .timestamp_millis_opt(t_ms)
        .single()
        .expect("every UTC instant maps to one ET wall time");
    let m = local.hour() * 60 + local.minute();
    if !(4 * 60..20 * 60).contains(&m) {
        ChartSession::Overnight
    } else if m < 9 * 60 + 30 {
        ChartSession::Pre
    } else if m < 16 * 60 {
        ChartSession::Regular
    } else {
        ChartSession::After
    }
}

/// chart-core `sessionSlots`: with the chart setting "extended hours" off, only regular-session
/// slots are kept, **before** indicators are computed (so EMA/VWAP become regular-hours-only).
pub fn chart_session_slots(slots: &[Slot], extended_hours: bool) -> Vec<Slot> {
    if extended_hours {
        return slots.to_vec();
    }
    slots
        .iter()
        .filter(|s| chart_session_of(s.t()) == ChartSession::Regular)
        .cloned()
        .collect()
}

/// Candle closes per slot, NaN on gap slots.
pub fn chart_closes(slots: &[Slot]) -> Vec<f64> {
    slots
        .iter()
        .map(|s| s.candle().map_or(f64::NAN, |c| c.c))
        .collect()
}

pub fn chart_sma(values: &[f64], period: usize) -> Vec<f64> {
    let mut out = vec![f64::NAN; values.len()];
    let mut win: VecDeque<f64> = VecDeque::new();
    let mut sum = 0.0;
    for (i, &v) in values.iter().enumerate() {
        if v.is_nan() {
            continue;
        }
        win.push_back(v);
        sum += v;
        if win.len() > period {
            sum -= win.pop_front().expect("non-empty");
        }
        if win.len() == period {
            out[i] = sum / period as f64;
        }
    }
    out
}

pub fn chart_ema(values: &[f64], period: usize) -> Vec<f64> {
    let mut out = vec![f64::NAN; values.len()];
    let k = 2.0 / (period as f64 + 1.0);
    let mut seed = 0.0;
    let mut count = 0;
    let mut prev = f64::NAN;
    for (i, &v) in values.iter().enumerate() {
        if v.is_nan() {
            continue;
        }
        if count < period {
            seed += v;
            count += 1;
            if count == period {
                prev = seed / period as f64;
                out[i] = prev;
            }
            continue;
        }
        prev = v * k + prev * (1.0 - k);
        out[i] = prev;
    }
    out
}

pub fn chart_vwap_hlc3(slots: &[Slot]) -> Vec<f64> {
    let mut out = vec![f64::NAN; slots.len()];
    let mut day: Option<NaiveDate> = None;
    let mut pv = 0.0;
    let mut vol = 0.0;
    for (i, s) in slots.iter().enumerate() {
        let Some(c) = s.candle() else { continue };
        let key = chart_trading_day_of(c.t);
        if day != Some(key) {
            day = Some(key);
            pv = 0.0;
            vol = 0.0;
        }
        let v = c.v as f64;
        pv += ((c.h + c.l + c.c) / 3.0) * v;
        vol += v;
        out[i] = if vol > 0.0 { pv / vol } else { c.c };
    }
    out
}

/// `(basis, upper, lower)`.
pub fn chart_bollinger(values: &[f64], period: usize, mult: f64) -> [Vec<f64>; 3] {
    let n = values.len();
    let (mut mid, mut up, mut lo) = (vec![f64::NAN; n], vec![f64::NAN; n], vec![f64::NAN; n]);
    let mut win: VecDeque<f64> = VecDeque::new();
    for (i, &v) in values.iter().enumerate() {
        if v.is_nan() {
            continue;
        }
        win.push_back(v);
        if win.len() > period {
            win.pop_front();
        }
        if win.len() < period {
            continue;
        }
        let mut m = 0.0;
        for x in &win {
            m += x;
        }
        m /= period as f64;
        let mut sd = 0.0;
        for x in &win {
            sd += (x - m) * (x - m);
        }
        sd = (sd / period as f64).sqrt();
        mid[i] = m;
        up[i] = m + mult * sd;
        lo[i] = m - mult * sd;
    }
    [mid, up, lo]
}

pub fn chart_rsi(values: &[f64], period: usize) -> Vec<f64> {
    let mut out = vec![f64::NAN; values.len()];
    let mut prev = f64::NAN;
    let (mut gain, mut loss) = (0.0, 0.0);
    let mut n = 0;
    let p = period as f64;
    for (i, &v) in values.iter().enumerate() {
        if v.is_nan() {
            continue;
        }
        if prev.is_nan() {
            prev = v;
            continue;
        }
        let d = v - prev;
        prev = v;
        let g = if d > 0.0 { d } else { 0.0 };
        let l = if -d > 0.0 { -d } else { 0.0 };
        if n < period {
            gain += g;
            loss += l;
            n += 1;
            if n < period {
                continue;
            }
            gain /= p;
            loss /= p;
        } else {
            gain = (gain * (p - 1.0) + g) / p;
            loss = (loss * (p - 1.0) + l) / p;
        }
        out[i] = if loss == 0.0 {
            if gain == 0.0 { 50.0 } else { 100.0 }
        } else {
            100.0 - 100.0 / (1.0 + gain / loss)
        };
    }
    out
}

/// `(line, signal, histogram)`.
pub fn chart_macd(values: &[f64], fast: usize, slow: usize, signal: usize) -> [Vec<f64>; 3] {
    let f = chart_ema(values, fast);
    let s = chart_ema(values, slow);
    let n = values.len();
    let mut line = vec![f64::NAN; n];
    for i in 0..n {
        if !f[i].is_nan() && !s[i].is_nan() {
            line[i] = f[i] - s[i];
        }
    }
    let sig = chart_ema(&line, signal);
    let mut hist = vec![f64::NAN; n];
    for i in 0..n {
        if !sig[i].is_nan() {
            hist[i] = line[i] - sig[i];
        }
    }
    [line, sig, hist]
}

/// The dashboard's preset catalogue (chart-core `INDICATOR_PRESETS`), every line keyed
/// `"<preset id>/<line name>"` as the chart names them: `vwap/VWAP`, `ema-9/EMA`, `ema-20/EMA`,
/// `sma-50/SMA`, `bb-20/Basis|Upper|Lower`, `rsi-14/RSI`, `macd/MACD|Signal|Hist`.
pub fn chart_presets(slots: &[Slot]) -> BTreeMap<String, Vec<f64>> {
    let c = chart_closes(slots);
    let mut m = BTreeMap::new();
    m.insert("vwap/VWAP".into(), chart_vwap_hlc3(slots));
    m.insert("ema-9/EMA".into(), chart_ema(&c, 9));
    m.insert("ema-20/EMA".into(), chart_ema(&c, 20));
    m.insert("sma-50/SMA".into(), chart_sma(&c, 50));
    let [mid, up, lo] = chart_bollinger(&c, 20, 2.0);
    m.insert("bb-20/Basis".into(), mid);
    m.insert("bb-20/Upper".into(), up);
    m.insert("bb-20/Lower".into(), lo);
    m.insert("rsi-14/RSI".into(), chart_rsi(&c, 14));
    let [l, s, h] = chart_macd(&c, 12, 26, 9);
    m.insert("macd/MACD".into(), l);
    m.insert("macd/Signal".into(), s);
    m.insert("macd/Hist".into(), h);
    m
}

/// An indicator read at decision time, completed and forming kept apart.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Reading {
    /// Interval start (ms) of the last **final** candle, and the indicator there, computed from
    /// final candles only. `None` value = not yet defined (warmup) at that candle.
    pub completed_t: Option<i64>,
    pub completed: Option<f64>,
    /// Interval start of the forming (non-final) candle, and the indicator computed with its
    /// **current** values: what the chart shows right now. It is never the value the interval
    /// will finally have; that is only known once the candle is final.
    pub forming_t: Option<i64>,
    pub forming: Option<f64>,
}

fn opt(x: f64) -> Option<f64> {
    (!x.is_nan()).then_some(x)
}

/// Reads `f` (any `chart_*` series function over slots) at decision time. `slots` must be the
/// state **as of** the decision time (the engine's snapshot then): every candle final except
/// possibly the newest. The completed value is computed on the prefix ending at the last final
/// candle, so nothing from the forming interval can reach it.
pub fn completed_and_forming(slots: &[Slot], f: impl Fn(&[Slot]) -> Vec<f64>) -> Reading {
    let forming_idx = match slots.last() {
        Some(Slot::Candle(c)) if !c.is_final => Some(slots.len() - 1),
        _ => None,
    };
    debug_assert!(
        slots[..forming_idx.unwrap_or(slots.len())]
            .iter()
            .all(|s| s.candle().is_none_or(|c| c.is_final)),
        "only the newest candle may be forming"
    );
    let completed_end = forming_idx.unwrap_or(slots.len());
    let last_final = slots[..completed_end]
        .iter()
        .rposition(|s| matches!(s, Slot::Candle(_)));
    let (completed_t, completed) = match last_final {
        Some(i) => {
            let prefix = &slots[..=i];
            (Some(slots[i].t()), opt(f(prefix)[i]))
        }
        None => (None, None),
    };
    let (forming_t, forming) = match forming_idx {
        Some(i) => (Some(slots[i].t()), opt(f(slots)[i])),
        None => (None, None),
    };
    Reading {
        completed_t,
        completed,
        forming_t,
        forming,
    }
}

/// Start (ms) of the interval containing `t_ms` for a timeframe of `tf_ms`: epoch-aligned, exactly
/// as `chart_engine::Timeframe::start_of`. Because ET offsets are whole hours and every session
/// boundary (04:00, 09:30, 13:00, 16:00, 17:00, 20:00 ET) is a multiple of 5 minutes, this is also
/// session-anchored for 15s/30s/1m/5m (tested).
pub fn interval_start_ms(t_ms: i64, tf_ms: i64) -> i64 {
    t_ms.div_euclid(tf_ms) * tf_ms
}

// ---------------------------------------------------------------------------------------------
// A trade-weighted VWAP: a DIFFERENT indicator from the chart's, never substituted for it.
// ---------------------------------------------------------------------------------------------

/// `vwap_trade`: Σ(price × size) / Σ size over individual prints, exact in integers.
///
/// Eligibility (Alpaca's own bar-VWAP rule, made explicit): a print counts iff the research
/// condition policy lets it update **both** high/low **and** volume (so odd lots, average-price,
/// prior-reference and every unresolved print are out), under the caller's [`UpdateMarkerMode`].
/// Resets with [`chart_trading_day_of`], like the chart's VWAP. Only SIP sessions 04:00–20:00 are
/// expected; the caller filters overnight/BOATS prints if it wants a SIP-only VWAP.
///
/// [`UpdateMarkerMode`]: crate::conditions::UpdateMarkerMode
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct VwapTrade {
    day: Option<NaiveDate>,
    /// Σ price_micros × size.
    pv: i128,
    vol: u128,
}

impl VwapTrade {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a print (`ts_ms`, price in micro-dollars, size) if `effect` qualifies it.
    pub fn observe(
        &mut self,
        ts_ms: i64,
        price_micros: i64,
        size: u64,
        effect: &crate::conditions::ResearchEffect,
    ) {
        let day = chart_trading_day_of(ts_ms);
        if self.day != Some(day) {
            *self = VwapTrade {
                day: Some(day),
                pv: 0,
                vol: 0,
            };
        }
        if effect.updates.high_low && effect.updates.volume && size > 0 {
            self.pv += price_micros as i128 * size as i128;
            self.vol += size as u128;
        }
    }

    /// Dollars, `None` before the day's first qualifying print.
    pub fn value(&self) -> Option<f64> {
        (self.vol > 0).then(|| (self.pv as f64 / self.vol as f64) / 1_000_000.0)
    }

    pub fn day(&self) -> Option<NaiveDate> {
        self.day
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conditions::research_effect;
    use chart_engine::Timeframe;
    use chart_engine::wire::GapReason;
    use market_events::{Tape, UnixNanos, session::sessions_for};

    const T0: i64 = 1_792_072_800_000; // 2026-10-15 10:00 ET

    fn candle(t: i64, c: f64, v: u64, fin: bool) -> CandleWire {
        CandleWire {
            t,
            o: c,
            h: c,
            l: c,
            c,
            v,
            n: 1,
            is_final: fin,
        }
    }

    fn slots(closes: &[f64]) -> Vec<Slot> {
        let cs: Vec<_> = closes
            .iter()
            .enumerate()
            .map(|(i, &c)| candle(T0 + i as i64 * 60_000, c, 100, true))
            .collect();
        chart_build_slots(&cs, &[])
    }

    fn close(a: f64, b: f64) {
        assert!((a - b).abs() < 1e-12, "{a} vs {b}");
    }

    // The TS unit tests (packages/chart-core/src/indicators.test.ts), ported one for one.
    #[test]
    fn sma_and_ema_match_the_ts_unit_tests() {
        let out = chart_sma(&[1.0, 2.0, 3.0, 4.0, 5.0], 3);
        assert!(out[0].is_nan() && out[1].is_nan());
        close(out[2], 2.0);
        close(out[4], 4.0);
        let out = chart_sma(&[1.0, 2.0, f64::NAN, 3.0], 3);
        assert!(out[2].is_nan());
        close(out[3], 2.0);
        let out = chart_ema(&[2.0, 4.0, 6.0, 8.0], 3);
        close(out[2], 4.0);
        close(out[3], 8.0 * 0.5 + 4.0 * 0.5);
    }

    #[test]
    fn rsi_macd_bb_match_the_ts_unit_tests() {
        assert_eq!(chart_rsi(&[1.0, 2.0, 3.0, 4.0], 3)[3], 100.0);
        assert_eq!(chart_rsi(&[5.0, 5.0, 5.0, 5.0], 3)[3], 50.0);
        let r = chart_rsi(&[10.0, 11.0, 10.0, 11.0, 12.0], 2);
        close(r[2], 50.0);
        close(r[3], 75.0);
        let xs: Vec<f64> = (0..60).map(|i| 3.0 + (i as f64 / 5.0).sin()).collect();
        let [line, sig, hist] = chart_macd(&xs, 12, 26, 9);
        assert!(line[24].is_nan());
        assert!(!line[25].is_nan());
        for i in 0..xs.len() {
            if !sig[i].is_nan() {
                close(hist[i], line[i] - sig[i]);
            }
        }
        let [mid, up, lo] = chart_bollinger(&[1.0, 2.0, 3.0], 3, 2.0);
        let sd = (2.0f64 / 3.0).sqrt();
        close(mid[2], 2.0);
        close(up[2], 2.0 + 2.0 * sd);
        close(lo[2], 2.0 - 2.0 * sd);
    }

    #[test]
    fn gaps_are_skipped_and_state_carries_across() {
        let mut s = slots(&[1.0, 2.0, 3.0]);
        s.insert(
            2,
            Slot::Gap(GapWire {
                t: T0 + 90_000,
                end: T0 + 120_000,
                reason: GapReason::VolumeOnlyTrades,
                v: 1_000_000,
                intervals: 1,
            }),
        );
        let e = chart_ema(&chart_closes(&s), 3);
        assert!(e[2].is_nan());
        close(e[3], 2.0);
        // VWAP ignores the gap's volume entirely.
        let v = chart_vwap_hlc3(&s);
        assert!(v[2].is_nan());
        close(v[3], 2.0);
    }

    #[test]
    fn vwap_resets_at_20_et_into_the_next_trading_day() {
        // 2026-10-15 19:59 ET and 20:00 ET (= next day's overnight).
        let a = 1_792_108_740_000; // 19:59 ET
        let b = a + 60_000; // 20:00 ET
        assert_eq!(
            chart_trading_day_of(a),
            NaiveDate::from_ymd_opt(2026, 10, 15).unwrap()
        );
        assert_eq!(
            chart_trading_day_of(b),
            NaiveDate::from_ymd_opt(2026, 10, 16).unwrap()
        );
        let s = chart_build_slots(&[candle(a, 2.0, 100, true), candle(b, 4.0, 100, true)], &[]);
        let v = chart_vwap_hlc3(&s);
        close(v[0], 2.0);
        close(v[1], 4.0);
        // Zero volume shows the close.
        let s = chart_build_slots(&[candle(a, 2.0, 0, true)], &[]);
        close(chart_vwap_hlc3(&s)[0], 2.0);
    }

    #[test]
    fn build_slots_candle_supersedes_gap_at_same_t() {
        let g = GapWire {
            t: T0,
            end: T0 + 60_000,
            reason: GapReason::NoTrades,
            v: 0,
            intervals: 1,
        };
        let s = chart_build_slots(&[candle(T0, 1.0, 1, true)], &[g]);
        assert_eq!(s.len(), 1);
        assert!(matches!(s[0], Slot::Candle(_)));
    }

    #[test]
    fn regular_only_filter_uses_fixed_16_00() {
        // 2026-11-27 is an early-close day (regular ends 13:00), but the chart's display filter
        // keeps 13:00-16:00 slots as "regular" all the same.
        let t = UnixNanos::utc(NaiveDate::from_ymd_opt(2026, 11, 27).unwrap(), 19, 0, 0, 0)
            .unwrap()
            .millis(); // 14:00 EST
        assert_eq!(chart_session_of(t), ChartSession::Regular);
        let s = chart_build_slots(&[candle(t, 1.0, 1, true)], &[]);
        assert_eq!(chart_session_slots(&s, false).len(), 1);
    }

    #[test]
    fn completed_and_forming_are_separate() {
        let mut s = slots(&[1.0, 2.0, 3.0, 4.0]);
        if let Slot::Candle(c) = s.last_mut().unwrap() {
            c.is_final = false;
            c.c = 10.0; // the forming candle's current close
        }
        let f = |x: &[Slot]| chart_ema(&chart_closes(x), 3);
        let r = completed_and_forming(&s, f);
        assert_eq!(r.completed_t, Some(T0 + 2 * 60_000));
        close(r.completed.unwrap(), 2.0); // SMA seed of 1,2,3
        assert_eq!(r.forming_t, Some(T0 + 3 * 60_000));
        close(r.forming.unwrap(), 10.0 * 0.5 + 2.0 * 0.5);
        // The completed value equals the chart's value at that candle (indicators are causal), so
        // computing on the prefix changes nothing but guarantees no forming data leaks in.
        close(f(&s)[2], r.completed.unwrap());
        // No forming candle: forming is None.
        let s = slots(&[1.0, 2.0, 3.0]);
        let r = completed_and_forming(&s, f);
        assert_eq!(r.forming, None);
        close(r.completed.unwrap(), 2.0);
    }

    #[test]
    fn intervals_are_session_anchored_on_every_boundary() {
        let days = [
            (2026, 3, 6),
            (2026, 3, 9),
            (2026, 10, 15),
            (2026, 11, 2),
            (2026, 11, 27),
        ];
        for (y, m, d) in days {
            for (_, start, end) in sessions_for(NaiveDate::from_ymd_opt(y, m, d).unwrap()) {
                for tf in Timeframe::ALL {
                    let tf_ms = tf.nanos() / 1_000_000;
                    for b in [start, end] {
                        assert_eq!(interval_start_ms(b.millis(), tf_ms), b.millis(), "{tf} {b}");
                        // One nanosecond before a boundary belongs to the previous interval.
                        let before = b.add_nanos(-1);
                        assert_eq!(
                            interval_start_ms(before.millis(), tf_ms),
                            b.millis() - tf_ms,
                            "{tf} {b}"
                        );
                        assert_eq!(tf.start_of(before).millis(), b.millis() - tf_ms);
                    }
                }
            }
        }
    }

    #[test]
    fn vwap_trade_is_exact_and_condition_filtered() {
        let mut v = VwapTrade::new();
        let reg = research_effect(&["@"], Tape::C);
        let odd = research_effect(&["@", "I"], Tape::C);
        let unres = research_effect(&["@", "E"], Tape::C);
        assert_eq!(v.value(), None);
        v.observe(T0, 3_000_000, 100, &reg);
        v.observe(T0 + 1, 9_000_000, 50, &odd);
        v.observe(T0 + 2, 9_000_000, 50, &unres);
        v.observe(T0 + 3, 4_000_000, 300, &reg);
        close(v.value().unwrap(), (3.0 * 100.0 + 4.0 * 300.0) / 400.0);
        // New trading day resets.
        v.observe(T0 + 86_400_000, 5_000_000, 10, &reg);
        close(v.value().unwrap(), 5.0);
    }
}

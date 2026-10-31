//! Technical indicators over the engine's candles: SMA, EMA, VWAP, Bollinger Bands, RSI and
//! MACD, as a pure function of the candles with incremental state. No I/O and no per-symbol
//! state here: the server keeps one [`IndicatorSeries`] per (symbol, timeframe, rth, spec) that
//! some chart asked for (crates/server/src/indicators.rs), never one per symbol of the market.
//!
//! # Parity with the client
//!
//! This is a port of `packages/chart-core/src/indicators.ts` (`sma` .. `macd`), which the
//! clients ran over the shown candles until the server took it over (docs plan "Moving all
//! client-side market-data computation into the Rust server", 2.3). The values are meant to be
//! **bit-identical** to that code on the same candles, and `tests/indicator_parity.rs` checks it
//! against fixtures that code generated (`packages/chart-core/src/indicatorFixtures.ts`). Both
//! sides are IEEE-754 f64, Rust never fuses a multiply-add on its own and `sqrt` is correctly
//! rounded on both, so parity only needs the same operations in the same order on the same
//! inputs. Every formula below therefore keeps the TS order even where another order would be
//! more natural; do not "simplify" one without regenerating the fixtures and reading the diff:
//!
//! - inputs are [`CandleWire`]'s `Price::to_f64` prices and `v as f64`, the numbers a client
//!   parses from the same JSON (serde_json prints the shortest round-trip form);
//! - SMA: `sum += v` first, `sum -= shifted` second; the running sum is checkpointed per bar
//!   (it depends on its history), never re-summed;
//! - EMA: seed = Σ first `period` values, `/ period`; then `v*k + prev*(1-k)`, no `mul_add`;
//! - VWAP: reset when the trading day changes; `pv += ((h+l+c)/3)*v; vol += v`;
//!   `vol > 0 ? pv/vol : c`;
//! - Bollinger: the window is re-summed oldest to newest on every bar (population sd), which is
//!   O(period) per update and has to stay that way for parity;
//! - RSI: Wilder smoothing after a simple-average seed; `d > 0 ? d : 0` rather than `max`, so a
//!   -0 can never reach the sums;
//! - MACD: EMA(fast) − EMA(slow) where both exist, the signal EMA over that line skipping the
//!   warm-up, histogram = line − signal.
//!
//! Gaps hold no candle and are skipped, not counted, with the running state carried across, as
//! the TS does with its NaN gap slots. So a [`Bar`] list (candles only) gives the same values as
//! the client's slot list.
//!
//! # Deliberate divergences from what the clients showed
//!
//! 1. **RTH on early-close days.** `rth` keeps the engine's `Regular` session, which ends at
//!    13:00 ET on an early close; the client's display schedule (`sessionOf`) counted 13:00–16:00
//!    as regular too. The engine is right.
//! 2. **History anchor.** A series is anchored at the first bar it was built from and does not
//!    re-seed when the front is trimmed ([`IndicatorSeries::trim_before`]); a client recomputed
//!    from whatever its own ring still held (3000 bars on the phone), so the warm-up of EMA, RSI
//!    and MACD at its left edge moved as bars dropped. Values here never move once computed
//!    unless a candle they depend on changed.
//! 3. **A candle that became volume-only** (a cancel or correction took its last price-qualifying
//!    print) is removed here ([`IndicatorSeries::remove`] on its `chart.gap`); the client's store
//!    kept the stale candle until its next snapshot.
//!
//! # Incremental updates
//!
//! [`IndicatorSeries`] keeps the running state after every bar (a checkpoint). A change at bar
//! `k` recomputes `k..len` from checkpoint `k-1`: the forming bar is O(1) (Bollinger
//! O(period)), a new bar O(1), a revision in the middle (a late trade, a correction or cancel)
//! O(len − k). `tests/indicator_incremental.rs` checks after every step of seeded random edits
//! that the incremental values equal [`batch`] over the same bars bit for bit.

use std::ops::Range;

use chrono::NaiveDate;
use market_events::session::{SessionId, SessionKind};
use serde_json::Value;

use crate::wire::CandleWire;

/// Longest lookback a spec may ask for (`PERIOD_MAX` in packages/chart-core/src/syncedSettings.ts).
pub const PERIOD_MAX: u32 = 500;
/// Widest Bollinger band, in standard deviations (`parseIndicators`: `0 < mult <= 10`).
pub const MULT_MAX: f64 = 10.0;
/// Most specs one list may hold (`parseIndicators` caps the stored list at 16).
pub const MAX_SPECS: usize = 16;
/// Overlays and panes shown at once (`MAX_OVERLAYS`, `MAX_PANES` in indicators.ts): the colour
/// slots are validated for three overlays; anything past the caps is dropped in list order, as
/// `computeIndicators` does.
pub const MAX_OVERLAYS: usize = 3;
pub const MAX_PANES: usize = 2;
/// Longest spec id (`parseIndicators`).
pub const MAX_ID_CHARS: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum IndicatorKind {
    Sma,
    Ema,
    Vwap,
    Bb,
    Rsi,
    Macd,
}

impl IndicatorKind {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "sma" => IndicatorKind::Sma,
            "ema" => IndicatorKind::Ema,
            "vwap" => IndicatorKind::Vwap,
            "bb" => IndicatorKind::Bb,
            "rsi" => IndicatorKind::Rsi,
            "macd" => IndicatorKind::Macd,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            IndicatorKind::Sma => "sma",
            IndicatorKind::Ema => "ema",
            IndicatorKind::Vwap => "vwap",
            IndicatorKind::Bb => "bb",
            IndicatorKind::Rsi => "rsi",
            IndicatorKind::Macd => "macd",
        }
    }

    /// Drawn over the price (`INDICATOR_PLACEMENT` "overlay") rather than in its own pane.
    pub fn is_overlay(self) -> bool {
        !matches!(self, IndicatorKind::Rsi | IndicatorKind::Macd)
    }
}

/// How a line is drawn (`IndicatorLine.style`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineStyle {
    Line,
    Histogram,
}

impl LineStyle {
    pub fn as_str(self) -> &'static str {
        match self {
            LineStyle::Line => "line",
            LineStyle::Histogram => "histogram",
        }
    }
}

/// A study with every parameter resolved (the TS defaults filled in: sma 20, ema 9, bb 20 / 2,
/// rsi 14, macd 12 / 26 / 9). Two specs that compute the same values are equal and have the same
/// [`IndicatorSpec::key`], whatever ids the clients gave them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum IndicatorSpec {
    Sma {
        period: u32,
    },
    Ema {
        period: u32,
    },
    Vwap,
    /// `mult` is kept as the client's f64, never rounded: the band is `m ± mult*sd`, and a
    /// rounded multiplier would break bit parity.
    Bb {
        period: u32,
        mult: f64,
    },
    Rsi {
        period: u32,
    },
    Macd {
        fast: u32,
        slow: u32,
        signal: u32,
    },
}

/// `intIn(v, 1, PERIOD_MAX)` of syncedSettings.ts: a finite integer in range, else absent.
fn period_in(v: Option<&Value>) -> Option<u32> {
    let x = v?.as_f64()?;
    (x.is_finite() && x.fract() == 0.0 && (1.0..=PERIOD_MAX as f64).contains(&x))
        .then_some(x as u32)
}

impl IndicatorSpec {
    pub fn kind(&self) -> IndicatorKind {
        match self {
            IndicatorSpec::Sma { .. } => IndicatorKind::Sma,
            IndicatorSpec::Ema { .. } => IndicatorKind::Ema,
            IndicatorSpec::Vwap => IndicatorKind::Vwap,
            IndicatorSpec::Bb { .. } => IndicatorKind::Bb,
            IndicatorSpec::Rsi { .. } => IndicatorKind::Rsi,
            IndicatorSpec::Macd { .. } => IndicatorKind::Macd,
        }
    }

    /// One entry of a client's list, by `parseIndicators`' rules: `None` for an entry it would
    /// drop (not an object, no usable id, unknown kind); an out-of-range parameter is left out,
    /// so the default applies, exactly as the client's own parse does. Returns the id too.
    pub fn parse(raw: &Value) -> Option<(String, IndicatorSpec)> {
        let o = raw.as_object()?;
        let id = o.get("id")?.as_str()?;
        // `r.id.length > 64` counts UTF-16 code units.
        if id.is_empty() || id.encode_utf16().count() > MAX_ID_CHARS {
            return None;
        }
        let kind = IndicatorKind::parse(o.get("kind")?.as_str()?)?;
        let period = period_in(o.get("period"));
        let mult = o
            .get("mult")
            .and_then(Value::as_f64)
            .filter(|m| m.is_finite() && *m > 0.0 && *m <= MULT_MAX);
        let fast = period_in(o.get("fast"));
        let slow = period_in(o.get("slow"));
        let signal = period_in(o.get("signal"));
        let spec = match kind {
            IndicatorKind::Sma => IndicatorSpec::Sma {
                period: period.unwrap_or(20),
            },
            IndicatorKind::Ema => IndicatorSpec::Ema {
                period: period.unwrap_or(9),
            },
            IndicatorKind::Vwap => IndicatorSpec::Vwap,
            IndicatorKind::Bb => IndicatorSpec::Bb {
                period: period.unwrap_or(20),
                mult: mult.unwrap_or(2.0),
            },
            IndicatorKind::Rsi => IndicatorSpec::Rsi {
                period: period.unwrap_or(14),
            },
            IndicatorKind::Macd => IndicatorSpec::Macd {
                fast: fast.unwrap_or(12),
                slow: slow.unwrap_or(26),
                signal: signal.unwrap_or(9),
            },
        };
        Some((id.to_string(), spec))
    }

    /// A client's whole list, as `parseIndicators` then `computeIndicators` see it: malformed
    /// entries and repeated ids dropped, at most [`MAX_SPECS`] kept, then overlays past
    /// [`MAX_OVERLAYS`] and panes past [`MAX_PANES`] dropped in order. `Err` only when `raw` is
    /// not a list at all (the client sent something no client version sends).
    pub fn parse_list(raw: &Value) -> Result<Vec<(String, IndicatorSpec)>, &'static str> {
        let list = raw.as_array().ok_or("indicators must be a list")?;
        let mut parsed: Vec<(String, IndicatorSpec)> = Vec::new();
        for r in list {
            let Some((id, spec)) = Self::parse(r) else {
                continue;
            };
            if parsed.iter().any(|(i, _)| *i == id) {
                continue;
            }
            parsed.push((id, spec));
            if parsed.len() >= MAX_SPECS {
                break;
            }
        }
        let (mut overlays, mut panes) = (0, 0);
        parsed.retain(|(_, s)| {
            let n = if s.kind().is_overlay() {
                &mut overlays
            } else {
                &mut panes
            };
            let cap = if s.kind().is_overlay() {
                MAX_OVERLAYS
            } else {
                MAX_PANES
            };
            *n += 1;
            *n <= cap
        });
        Ok(parsed)
    }

    /// The canonical key frames carry (`specKey` in indicators.ts, tested against the same
    /// vectors: `tests/fixtures/indicator_spec_keys.json`): `"sma:20"`, `"ema:9"`, `"vwap"`,
    /// `"bb:20:2"`, `"rsi:14"`, `"macd:12:26:9"`. `mult` is printed as JavaScript's
    /// `String(number)` prints it.
    pub fn key(&self) -> String {
        match *self {
            IndicatorSpec::Sma { period } => format!("sma:{period}"),
            IndicatorSpec::Ema { period } => format!("ema:{period}"),
            IndicatorSpec::Vwap => "vwap".into(),
            IndicatorSpec::Bb { period, mult } => format!("bb:{period}:{}", js_number(mult)),
            IndicatorSpec::Rsi { period } => format!("rsi:{period}"),
            IndicatorSpec::Macd { fast, slow, signal } => format!("macd:{fast}:{slow}:{signal}"),
        }
    }

    /// The lines a study draws, in the TS order (`computeIndicators`): the values of
    /// [`IndicatorSeries::values`] follow it.
    pub fn lines(&self) -> &'static [(&'static str, LineStyle)] {
        const L: LineStyle = LineStyle::Line;
        match self {
            IndicatorSpec::Sma { .. } => &[("SMA", L)],
            IndicatorSpec::Ema { .. } => &[("EMA", L)],
            IndicatorSpec::Vwap => &[("VWAP", L)],
            IndicatorSpec::Bb { .. } => &[("Basis", L), ("Upper", L), ("Lower", L)],
            IndicatorSpec::Rsi { .. } => &[("RSI", L)],
            IndicatorSpec::Macd { .. } => {
                &[("Hist", LineStyle::Histogram), ("MACD", L), ("Signal", L)]
            }
        }
    }
}

/// `String(x)` of JavaScript for a positive finite `x`: the shortest round-trip digits, plain
/// below 1e21 and from 1e-6 up, exponent form (`1.5e-7`) outside. Rust's `Display` gives the
/// same shortest digits, always plain, and `LowerExp` the same exponent form.
fn js_number(x: f64) -> String {
    if x != 0.0 && (x.abs() < 1e-6 || x.abs() >= 1e21) {
        let s = format!("{x:e}");
        // JS writes a positive exponent with its sign ("1e+21").
        match s.split_once('e') {
            Some((m, e)) if !e.starts_with('-') => format!("{m}e+{e}"),
            _ => s,
        }
    } else {
        format!("{x}")
    }
}

/// What one engine increment does to an indicator input series.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum BarEdit {
    Upsert(Bar),
    Remove(i64),
}

impl BarEdit {
    /// The edit `u` makes to the series of its (symbol, timeframe) built with `rth`, if any:
    /// - `chart.update`: the candle is upserted (outside the regular session, an rth series
    ///   ignores it; a bucket never changes session);
    /// - `chart.remove`: the bar at `t` goes;
    /// - `chart.gap`: no candle at `t`. For a volume-only bucket that is a candle that lost its
    ///   last price-qualifying print (divergence 3 in the module docs); for a no-trades gap
    ///   there is no bar at `t` and the removal is a no-op.
    ///
    /// Tape, halt and LULD rows change nothing. A finalization re-emits the same candle, which
    /// [`IndicatorSeries::upsert`] recognizes as no change.
    pub fn of(u: &crate::wire::ChartUpdate, rth: bool) -> Option<BarEdit> {
        use crate::wire::ChartUpdate;
        match u {
            ChartUpdate::Candle(c) => {
                let session: SessionId = c.session.parse().ok()?;
                Bar::keep(session, rth).then(|| BarEdit::Upsert(Bar::new(&c.candle, session)))
            }
            ChartUpdate::Remove(r) => Some(BarEdit::Remove(r.t)),
            ChartUpdate::Gap(g) => Some(BarEdit::Remove(g.gap.t)),
            ChartUpdate::Tape(_) | ChartUpdate::Halt(_) | ChartUpdate::Luld(_) => None,
        }
    }
}

/// One candle as the indicators read it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bar {
    /// Interval start, epoch ms (`CandleWire.t`).
    pub t_ms: i64,
    /// The trading day VWAP resets on: the bucket's session date, where the overnight session
    /// that starts at 20:00 ET belongs to the next date. That is the TS
    /// `formatEtDateKey(tradingDayOf(t))` for every candle the engine can emit (there are no
    /// candles outside sessions), without wall-clock arithmetic here.
    pub day: NaiveDate,
    pub h: f64,
    pub l: f64,
    pub c: f64,
    /// `CandleWire.v as f64`: exact below 2^53, as the client's JSON number is.
    pub v: f64,
}

impl Bar {
    pub fn new(candle: &CandleWire, session: SessionId) -> Self {
        Bar {
            t_ms: candle.t,
            day: session.date,
            h: candle.h,
            l: candle.l,
            c: candle.c,
            v: candle.v as f64,
        }
    }

    /// Whether the bar belongs in a series built with `rth` (regular session only).
    pub fn keep(session: SessionId, rth: bool) -> bool {
        !rth || session.kind == SessionKind::Regular
    }

    fn same(&self, o: &Bar) -> bool {
        self.t_ms == o.t_ms
            && self.day == o.day
            && self.h.to_bits() == o.h.to_bits()
            && self.l.to_bits() == o.l.to_bits()
            && self.c.to_bits() == o.c.to_bits()
            && self.v.to_bits() == o.v.to_bits()
    }
}

// ---------------------------------------------------------------------------------------------
// Reference batch functions: the TS functions line for line (NaN = no value; NaN inputs are gap
// slots and are skipped). The parity test runs these against the TS fixtures, and the
// incremental series is tested equal to them.

fn nan_vec(n: usize) -> Vec<f64> {
    vec![f64::NAN; n]
}

/// Simple moving average over the last `period` values (NaN inputs skipped, not counted).
pub fn sma(values: &[f64], period: u32) -> Vec<f64> {
    let p = period as usize;
    let mut out = nan_vec(values.len());
    let mut win: std::collections::VecDeque<f64> = std::collections::VecDeque::new();
    let mut sum = 0.0;
    for (i, &v) in values.iter().enumerate() {
        if v.is_nan() {
            continue;
        }
        win.push_back(v);
        sum += v;
        if win.len() > p {
            sum -= win.pop_front().expect("longer than period");
        }
        if win.len() == p {
            out[i] = sum / period as f64;
        }
    }
    out
}

/// Exponential moving average seeded with the SMA of the first `period` values.
pub fn ema(values: &[f64], period: u32) -> Vec<f64> {
    let mut out = nan_vec(values.len());
    let mut st = EmaState::default();
    for (i, &v) in values.iter().enumerate() {
        if v.is_nan() {
            continue;
        }
        out[i] = st.push(v, period);
    }
    out
}

/// Session VWAP from candles, reset when [`Bar::day`] changes.
pub fn vwap(bars: &[Bar]) -> Vec<f64> {
    let mut st = VwapState::default();
    bars.iter().map(|b| st.push(b)).collect()
}

/// Bollinger Bands: `[basis, upper, lower]`, population standard deviation.
pub fn bollinger(values: &[f64], period: u32, mult: f64) -> [Vec<f64>; 3] {
    let p = period as usize;
    let n = values.len();
    let (mut mid, mut up, mut lo) = (nan_vec(n), nan_vec(n), nan_vec(n));
    let mut win: Vec<f64> = Vec::new();
    for (i, &v) in values.iter().enumerate() {
        if v.is_nan() {
            continue;
        }
        win.push(v);
        if win.len() > p {
            win.remove(0);
        }
        if win.len() < p {
            continue;
        }
        let [m, u, l] = bb_window(win.iter().copied(), period, mult);
        mid[i] = m;
        up[i] = u;
        lo[i] = l;
    }
    [mid, up, lo]
}

/// Wilder's RSI: 100 when there were no losses, 50 when nothing moved.
pub fn rsi(values: &[f64], period: u32) -> Vec<f64> {
    let mut out = nan_vec(values.len());
    let mut st = RsiState::default();
    for (i, &v) in values.iter().enumerate() {
        if v.is_nan() {
            continue;
        }
        out[i] = st.push(v, period);
    }
    out
}

/// MACD: `[line, signal, histogram]` (the TS return order).
pub fn macd(values: &[f64], fast: u32, slow: u32, signal: u32) -> [Vec<f64>; 3] {
    let n = values.len();
    let f = ema(values, fast);
    let s = ema(values, slow);
    let mut line = nan_vec(n);
    for i in 0..n {
        if !f[i].is_nan() && !s[i].is_nan() {
            line[i] = f[i] - s[i];
        }
    }
    let sig = ema(&line, signal);
    let mut hist = nan_vec(n);
    for i in 0..n {
        if !sig[i].is_nan() {
            hist[i] = line[i] - sig[i];
        }
    }
    [line, sig, hist]
}

/// Every line of `spec` over `bars`, per bar in [`IndicatorSpec::lines`] order (unused slots
/// NaN), through the batch functions above.
pub fn batch(spec: &IndicatorSpec, bars: &[Bar]) -> Vec<[f64; 3]> {
    let closes: Vec<f64> = bars.iter().map(|b| b.c).collect();
    let one = |v: Vec<f64>| v.into_iter().map(|x| [x, f64::NAN, f64::NAN]).collect();
    let three =
        |a: Vec<f64>, b: Vec<f64>, c: Vec<f64>| (0..a.len()).map(|i| [a[i], b[i], c[i]]).collect();
    match *spec {
        IndicatorSpec::Sma { period } => one(sma(&closes, period)),
        IndicatorSpec::Ema { period } => one(ema(&closes, period)),
        IndicatorSpec::Vwap => one(vwap(bars)),
        IndicatorSpec::Bb { period, mult } => {
            let [m, u, l] = bollinger(&closes, period, mult);
            three(m, u, l)
        }
        IndicatorSpec::Rsi { period } => one(rsi(&closes, period)),
        IndicatorSpec::Macd { fast, slow, signal } => {
            let [line, sig, hist] = macd(&closes, fast, slow, signal);
            three(hist, line, sig)
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Running state, shared by the batch functions and the incremental series so there is one copy
// of each formula.

#[derive(Clone, Copy, Debug)]
struct EmaState {
    seed: f64,
    count: u32,
    prev: f64,
}

impl Default for EmaState {
    fn default() -> Self {
        EmaState {
            seed: 0.0,
            count: 0,
            prev: f64::NAN,
        }
    }
}

impl EmaState {
    /// Folds one value; returns the EMA at it (NaN before the seed).
    fn push(&mut self, v: f64, period: u32) -> f64 {
        if self.count < period {
            self.seed += v;
            self.count += 1;
            if self.count == period {
                self.prev = self.seed / period as f64;
                return self.prev;
            }
            return f64::NAN;
        }
        // `2 / (period + 1)` is recomputed rather than stored: it is the same f64 every time,
        // exactly as the TS computes it once.
        let k = 2.0 / (period as f64 + 1.0);
        self.prev = v * k + self.prev * (1.0 - k);
        self.prev
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct VwapState {
    day: Option<NaiveDate>,
    pv: f64,
    vol: f64,
}

impl VwapState {
    fn push(&mut self, b: &Bar) -> f64 {
        if self.day != Some(b.day) {
            self.day = Some(b.day);
            self.pv = 0.0;
            self.vol = 0.0;
        }
        self.pv += ((b.h + b.l + b.c) / 3.0) * b.v;
        self.vol += b.v;
        if self.vol > 0.0 {
            self.pv / self.vol
        } else {
            b.c
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct RsiState {
    prev: f64,
    gain: f64,
    loss: f64,
    n: u32,
}

impl Default for RsiState {
    fn default() -> Self {
        RsiState {
            prev: f64::NAN,
            gain: 0.0,
            loss: 0.0,
            n: 0,
        }
    }
}

impl RsiState {
    fn push(&mut self, v: f64, period: u32) -> f64 {
        if self.prev.is_nan() {
            self.prev = v;
            return f64::NAN;
        }
        let d = v - self.prev;
        self.prev = v;
        // `Math.max(d, 0)` / `Math.max(-d, 0)`: never -0 (JS max ranks +0 above -0).
        let g = if d > 0.0 { d } else { 0.0 };
        let l = if -d > 0.0 { -d } else { 0.0 };
        let p = period as f64;
        if self.n < period {
            self.gain += g;
            self.loss += l;
            self.n += 1;
            if self.n < period {
                return f64::NAN;
            }
            self.gain /= p;
            self.loss /= p;
        } else {
            self.gain = (self.gain * (p - 1.0) + g) / p;
            self.loss = (self.loss * (p - 1.0) + l) / p;
        }
        if self.loss == 0.0 {
            if self.gain == 0.0 { 50.0 } else { 100.0 }
        } else {
            100.0 - 100.0 / (1.0 + self.gain / self.loss)
        }
    }
}

/// One Bollinger point from a full window, oldest first (walked twice, as the TS does).
fn bb_window(win: impl Iterator<Item = f64> + Clone, period: u32, mult: f64) -> [f64; 3] {
    let p = period as f64;
    let mut m = 0.0;
    for x in win.clone() {
        m += x;
    }
    m /= p;
    let mut sd = 0.0;
    for x in win {
        sd += (x - m) * (x - m);
    }
    sd = (sd / p).sqrt();
    [m, m + mult * sd, m - mult * sd]
}

/// Checkpoint after one bar.
#[derive(Clone, Copy, Debug)]
enum State {
    /// The running sum and the window length (at most `period`); the value leaving the window
    /// is read back from the bars.
    Sma {
        sum: f64,
        count: u32,
    },
    Ema(EmaState),
    Vwap(VwapState),
    /// Stateless: every point re-sums its window from the bars.
    Bb,
    Rsi(RsiState),
    Macd {
        fast: EmaState,
        slow: EmaState,
        signal: EmaState,
    },
}

impl State {
    fn initial(spec: &IndicatorSpec) -> State {
        match spec {
            IndicatorSpec::Sma { .. } => State::Sma { sum: 0.0, count: 0 },
            IndicatorSpec::Ema { .. } => State::Ema(EmaState::default()),
            IndicatorSpec::Vwap => State::Vwap(VwapState::default()),
            IndicatorSpec::Bb { .. } => State::Bb,
            IndicatorSpec::Rsi { .. } => State::Rsi(RsiState::default()),
            IndicatorSpec::Macd { .. } => State::Macd {
                fast: EmaState::default(),
                slow: EmaState::default(),
                signal: EmaState::default(),
            },
        }
    }
}

/// Bar `i`'s point from the checkpoint before it.
fn step(spec: &IndicatorSpec, prev: State, bars: &[Bar], i: usize) -> (State, [f64; 3]) {
    const N: f64 = f64::NAN;
    let c = bars[i].c;
    match (*spec, prev) {
        (IndicatorSpec::Sma { period }, State::Sma { mut sum, mut count }) => {
            sum += c;
            count += 1;
            if count > period {
                sum -= bars[i - period as usize].c;
                count = period;
            }
            let v = if count == period {
                sum / period as f64
            } else {
                N
            };
            (State::Sma { sum, count }, [v, N, N])
        }
        (IndicatorSpec::Ema { period }, State::Ema(mut e)) => {
            let v = e.push(c, period);
            (State::Ema(e), [v, N, N])
        }
        (IndicatorSpec::Vwap, State::Vwap(mut w)) => {
            let v = w.push(&bars[i]);
            (State::Vwap(w), [v, N, N])
        }
        (IndicatorSpec::Bb { period, mult }, State::Bb) => {
            let p = period as usize;
            if i + 1 < p {
                return (State::Bb, [N, N, N]);
            }
            // The window as the TS holds it, oldest first.
            let win = bars[i + 1 - p..=i].iter().map(|b| b.c);
            (State::Bb, bb_window(win, period, mult))
        }
        (IndicatorSpec::Rsi { period }, State::Rsi(mut r)) => {
            let v = r.push(c, period);
            (State::Rsi(r), [v, N, N])
        }
        (
            IndicatorSpec::Macd { fast, slow, signal },
            State::Macd {
                fast: mut f,
                slow: mut s,
                signal: mut g,
            },
        ) => {
            let fv = f.push(c, fast);
            let sv = s.push(c, slow);
            let (line, sig) = if !fv.is_nan() && !sv.is_nan() {
                let line = fv - sv;
                (line, g.push(line, signal))
            } else {
                (N, N)
            };
            let hist = if sig.is_nan() { N } else { line - sig };
            (
                State::Macd {
                    fast: f,
                    slow: s,
                    signal: g,
                },
                [hist, line, sig],
            )
        }
        _ => unreachable!("a checkpoint always matches its spec"),
    }
}

/// One study over one input series (symbol, timeframe, rth): the bars, the checkpoint after
/// each, and the values at each.
#[derive(Clone, Debug)]
pub struct IndicatorSeries {
    spec: IndicatorSpec,
    bars: Vec<Bar>,
    ckpt: Vec<State>,
    out: Vec<[f64; 3]>,
    /// Most bars kept: the engine's ring (`max_intervals_per_timeframe`). Past it the oldest are
    /// dropped without recomputing anything ([`IndicatorSeries::trim_before`]).
    cap: usize,
}

/// Bars past `cap` that are tolerated before the front is drained, so a series at its cap
/// shifts its vectors once per this many new bars rather than on every one.
const DRAIN_SLACK: usize = 64;

impl IndicatorSeries {
    /// Builds a series over `bars` (ascending by `t_ms`, candles only). O(n).
    pub fn build(spec: IndicatorSpec, bars: Vec<Bar>, cap: usize) -> Self {
        let mut s = IndicatorSeries {
            spec,
            ckpt: Vec::with_capacity(bars.len()),
            out: Vec::with_capacity(bars.len()),
            bars,
            cap: cap.max(1),
        };
        s.recompute(0);
        s.enforce_cap();
        s
    }

    pub fn spec(&self) -> &IndicatorSpec {
        &self.spec
    }

    pub fn len(&self) -> usize {
        self.bars.len()
    }

    pub fn is_empty(&self) -> bool {
        self.bars.is_empty()
    }

    pub fn bars(&self) -> &[Bar] {
        &self.bars
    }

    /// Bar `i`'s start, epoch ms.
    pub fn t(&self, i: usize) -> i64 {
        self.bars[i].t_ms
    }

    /// Bar `i`'s values, one per [`IndicatorSpec::lines`] entry (NaN = no value yet).
    pub fn values(&self, i: usize) -> &[f64] {
        &self.out[i][..self.spec.lines().len()]
    }

    /// Where bar `t_ms` is (Ok) or would go (Err).
    fn find(&self, t_ms: i64) -> Result<usize, usize> {
        self.bars.binary_search_by_key(&t_ms, |b| b.t_ms)
    }

    /// Recomputes checkpoints and values from bar `k` to the end.
    fn recompute(&mut self, k: usize) {
        self.ckpt.truncate(k);
        self.out.truncate(k);
        let mut st = if k == 0 {
            State::initial(&self.spec)
        } else {
            self.ckpt[k - 1]
        };
        for i in k..self.bars.len() {
            let (next, v) = step(&self.spec, st, &self.bars, i);
            self.ckpt.push(next);
            self.out.push(v);
            st = next;
        }
    }

    fn enforce_cap(&mut self) -> usize {
        if self.bars.len() <= self.cap + DRAIN_SLACK {
            return 0;
        }
        let n = self.bars.len() - self.cap;
        self.drain_front(n);
        n
    }

    fn drain_front(&mut self, n: usize) {
        self.bars.drain(..n);
        self.ckpt.drain(..n);
        self.out.drain(..n);
    }

    /// Inserts or replaces the bar at `bar.t_ms`. Returns the indices whose values changed
    /// (after any front drain), or `None` when nothing did: the identical candle again (the
    /// engine re-emits it when it turns final), or a change no value depends on (only the volume
    /// of a candle, for a study that reads closes).
    pub fn upsert(&mut self, bar: Bar) -> Option<Range<usize>> {
        match self.find(bar.t_ms) {
            Ok(k) => {
                if self.bars[k].same(&bar) {
                    return None;
                }
                self.bars[k] = bar;
                let old: Vec<[f64; 3]> = self.out[k..].to_vec();
                self.recompute(k);
                // Values that came out bit-identical are not news.
                let changed = |i: usize| {
                    let (a, b) = (&old[i - k], &self.out[i]);
                    (0..3).any(|j| a[j].to_bits() != b[j].to_bits())
                };
                let first = (k..self.out.len()).find(|&i| changed(i))?;
                let last = (first..self.out.len()).rev().find(|&i| changed(i))?;
                Some(first..last + 1)
            }
            Err(k) => {
                self.bars.insert(k, bar);
                self.recompute(k);
                let dropped = self.enforce_cap();
                Some(k.saturating_sub(dropped)..self.bars.len())
            }
        }
    }

    /// Removes the bar at `t_ms`. Returns the indices from the removal point to the end (their
    /// values may all have moved), or `None` when there was no such bar.
    pub fn remove(&mut self, t_ms: i64) -> Option<Range<usize>> {
        let k = self.find(t_ms).ok()?;
        self.bars.remove(k);
        self.recompute(k);
        Some(k..self.bars.len())
    }

    /// [`IndicatorSeries::upsert`] or [`IndicatorSeries::remove`].
    pub fn apply(&mut self, edit: BarEdit) -> Option<Range<usize>> {
        match edit {
            BarEdit::Upsert(b) => self.upsert(b),
            BarEdit::Remove(t) => self.remove(t),
        }
    }

    /// Drops every bar before `t_ms` (the engine no longer holds them). The remaining values do
    /// not change: they keep the anchor they were computed from. Returns how many were dropped.
    pub fn trim_before(&mut self, t_ms: i64) -> usize {
        let n = self.find(t_ms).unwrap_or_else(|i| i);
        if n > 0 {
            self.drain_front(n);
        }
        n
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn day() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 10, 15).unwrap()
    }

    fn bar(i: i64, c: f64) -> Bar {
        Bar {
            t_ms: i * 60_000,
            day: day(),
            h: c + 0.1,
            l: c - 0.1,
            c,
            v: 100.0,
        }
    }

    #[test]
    fn defaults_and_keys_follow_the_client() {
        let k = |v: Value| IndicatorSpec::parse(&v).map(|(_, s)| s.key());
        assert_eq!(
            k(json!({"id": "a", "kind": "sma"})).as_deref(),
            Some("sma:20")
        );
        assert_eq!(
            k(json!({"id": "a", "kind": "bb", "mult": 0.5})).as_deref(),
            Some("bb:20:0.5")
        );
        assert_eq!(
            k(json!({"id": "a", "kind": "rsi", "period": 501})).as_deref(),
            Some("rsi:14")
        );
        assert_eq!(
            k(json!({"id": "a", "kind": "ema", "period": 9.5})).as_deref(),
            Some("ema:9")
        );
        assert_eq!(k(json!({"id": "a", "kind": "nope"})), None);
        assert_eq!(js_number(1.5e-7), "1.5e-7");
        assert_eq!(js_number(2.0), "2");
        assert_eq!(js_number(0.1 + 0.2), "0.30000000000000004");
    }

    #[test]
    fn a_list_keeps_three_overlays_and_two_panes() {
        let raw = json!([
            {"id": "a", "kind": "sma"}, {"id": "b", "kind": "ema"}, {"id": "a", "kind": "vwap"},
            {"id": "c", "kind": "rsi"}, {"id": "d", "kind": "vwap"}, {"id": "e", "kind": "bb"},
            {"id": "f", "kind": "macd"}, {"id": "g", "kind": "rsi", "period": 7}
        ]);
        let ids: Vec<String> = IndicatorSpec::parse_list(&raw)
            .unwrap()
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        assert_eq!(ids, ["a", "b", "c", "d", "f"]);
        assert!(IndicatorSpec::parse_list(&json!({"id": "a"})).is_err());
    }

    #[test]
    fn identical_candle_and_close_only_studies_report_no_change() {
        let bars: Vec<Bar> = (0..30).map(|i| bar(i, 10.0 + i as f64 * 0.01)).collect();
        let mut s = IndicatorSeries::build(IndicatorSpec::Sma { period: 5 }, bars.clone(), 4000);
        assert_eq!(s.upsert(bars[29]), None);
        let mut louder = bars[29];
        louder.v = 900.0;
        assert_eq!(s.upsert(louder), None, "SMA does not read volume");
        let mut w = IndicatorSeries::build(IndicatorSpec::Vwap, bars, 4000);
        assert_eq!(w.upsert(louder), Some(29..30));
    }

    #[test]
    fn trimming_keeps_the_remaining_values() {
        let bars: Vec<Bar> = (0..300)
            .map(|i| bar(i, 10.0 + (i % 17) as f64 * 0.03))
            .collect();
        let mut s = IndicatorSeries::build(IndicatorSpec::Ema { period: 20 }, bars, 4000);
        let before: Vec<u64> = (100..300).map(|i| s.values(i)[0].to_bits()).collect();
        assert_eq!(s.trim_before(100 * 60_000), 100);
        let after: Vec<u64> = (0..200).map(|i| s.values(i)[0].to_bits()).collect();
        assert_eq!(before, after);
        // A new bar continues from the old anchor, not from a re-seed.
        let r = s.upsert(bar(300, 11.0)).unwrap();
        assert_eq!(r, 200..201);
    }

    #[test]
    fn the_cap_drops_the_oldest_in_batches() {
        let bars: Vec<Bar> = (0..10).map(|i| bar(i, 10.0)).collect();
        let mut s = IndicatorSeries::build(IndicatorSpec::Vwap, bars, 10);
        for i in 10..10 + DRAIN_SLACK as i64 {
            s.upsert(bar(i, 10.0));
        }
        assert_eq!(s.len(), 10 + DRAIN_SLACK);
        let r = s.upsert(bar(10 + DRAIN_SLACK as i64, 10.0)).unwrap();
        assert_eq!(s.len(), 10);
        assert_eq!(r, 9..10);
    }
}

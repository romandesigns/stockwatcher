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
//! # MACD styles (`indicators.macd.v2`, owner-approved 2026-11-07)
//!
//! A MACD spec carries a `style` ([`MacdStyle`]). Classic is the formula above, unchanged and
//! still under the parity fixtures. The client's "4-color histogram" draws Classic's values in
//! other shades, so it parses to Classic here and shares its key and computation. The other
//! four are new (no client code ever computed them, so there is nothing to be bit-identical
//! to; `tests` below pin each with small hand-checked series):
//!
//! - **PPO**: `(EMA(fast) − EMA(slow)) / EMA(slow) × 100` (MACD as a percentage of price), the
//!   signal EMA over it, histogram = line − signal.
//! - **MACD-V** (Spiroglou): `(EMA(fast) − EMA(slow)) / ATR(26) × 100`, ATR with Wilder
//!   smoothing: true range `max(h − l, |h − c₋₁|, |l − c₋₁|)` (`h − l` on the first bar), seeded
//!   by the mean of the first 26 ranges, then `(atr × 25 + tr) / 26`. No value while ATR is 0
//!   (a flat stretch: the ratio is undefined, not infinite). ATR's period is fixed at
//!   [`MACD_V_ATR`].
//! - **Zero-lag**: MACD over zero-lag EMAs. `ZLEMA(n)` is the EMA of period `n` over the
//!   de-lagged price `2·cᵢ − cᵢ₋ₗ`, `lag = floor((n − 1) / 2)`: adding back the difference to the
//!   price `lag` bars ago cancels most of the EMA's own delay, so it turns sooner and crosses
//!   more often. The first `lag` bars have no de-lagged price and are skipped; then line =
//!   ZLEMA(fast) − ZLEMA(slow), the signal is a plain EMA of that line, histogram = line − signal.
//! - **Signal area**: Classic's line, with the signal a SIMPLE average of the last `signal` line
//!   values (the client draws it as bars from zero, the line over them); lines `[MACD, Signal]`,
//!   no histogram.
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
//!    unless a candle they depend on changed. One exception: SMA and Bollinger read back from
//!    the bars, so a change to one of the oldest retained bars after a trim re-anchors those
//!    two at the current front ([`IndicatorSeries`] `restart_at`).
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

/// MACD-V's ATR period (fixed: the published indicator pairs it with the slow EMA's 26).
pub const MACD_V_ATR: u32 = 26;

/// What a MACD spec computes (module docs, "MACD styles"). Absent on the wire = Classic, and so
/// is every name this server does not know (a newer client's style draws as Classic here rather
/// than dropping the study).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum MacdStyle {
    #[default]
    Classic,
    Ppo,
    MacdV,
    ZeroLag,
    SignalArea,
}

impl MacdStyle {
    /// Every style the server computes (tests walk it).
    pub const ALL: [MacdStyle; 5] = [
        MacdStyle::Classic,
        MacdStyle::Ppo,
        MacdStyle::MacdV,
        MacdStyle::ZeroLag,
        MacdStyle::SignalArea,
    ];

    /// The client's `style` field (chart-core `MacdStyle`). "four-color" is Classic's values in
    /// other shades, a drawing choice only, so it computes (and keys) as Classic.
    pub fn parse(s: &str) -> MacdStyle {
        match s {
            "ppo" => MacdStyle::Ppo,
            "macd-v" => MacdStyle::MacdV,
            "zero-lag" => MacdStyle::ZeroLag,
            "signal-area" => MacdStyle::SignalArea,
            _ => MacdStyle::Classic,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            MacdStyle::Classic => "classic",
            MacdStyle::Ppo => "ppo",
            MacdStyle::MacdV => "macd-v",
            MacdStyle::ZeroLag => "zero-lag",
            MacdStyle::SignalArea => "signal-area",
        }
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
        style: MacdStyle,
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
                style: o
                    .get("style")
                    .and_then(Value::as_str)
                    .map(MacdStyle::parse)
                    .unwrap_or_default(),
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
    /// `"bb:20:2"`, `"rsi:14"`, `"macd:12:26:9"`, and a non-Classic MACD style appended
    /// (`"macd:12:26:9:ppo"`), so Classic keeps the key every older client looks up. `mult` is
    /// printed as JavaScript's `String(number)` prints it.
    pub fn key(&self) -> String {
        match *self {
            IndicatorSpec::Sma { period } => format!("sma:{period}"),
            IndicatorSpec::Ema { period } => format!("ema:{period}"),
            IndicatorSpec::Vwap => "vwap".into(),
            IndicatorSpec::Bb { period, mult } => format!("bb:{period}:{}", js_number(mult)),
            IndicatorSpec::Rsi { period } => format!("rsi:{period}"),
            IndicatorSpec::Macd {
                fast,
                slow,
                signal,
                style: MacdStyle::Classic,
            } => format!("macd:{fast}:{slow}:{signal}"),
            IndicatorSpec::Macd {
                fast,
                slow,
                signal,
                style,
            } => format!("macd:{fast}:{slow}:{signal}:{}", style.as_str()),
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
            // Signal area draws its signal as bars from zero and has no histogram.
            IndicatorSpec::Macd {
                style: MacdStyle::SignalArea,
                ..
            } => &[("MACD", L), ("Signal", L)],
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

/// The signal EMA and histogram over a finished line (`[line, signal, histogram]`).
fn with_signal(line: Vec<f64>, signal: u32) -> [Vec<f64>; 3] {
    let sig = ema(&line, signal);
    let hist = (0..line.len())
        .map(|i| {
            if sig[i].is_nan() {
                f64::NAN
            } else {
                line[i] - sig[i]
            }
        })
        .collect();
    [line, sig, hist]
}

/// PPO: `[line, signal, histogram]`, line = `(EMA(fast) − EMA(slow)) / EMA(slow) × 100`.
pub fn ppo(values: &[f64], fast: u32, slow: u32, signal: u32) -> [Vec<f64>; 3] {
    let f = ema(values, fast);
    let s = ema(values, slow);
    let line = (0..values.len())
        .map(|i| ppo_point(f[i], s[i]))
        .collect::<Vec<_>>();
    with_signal(line, signal)
}

fn ppo_point(f: f64, s: f64) -> f64 {
    if f.is_nan() || s.is_nan() || s == 0.0 {
        f64::NAN
    } else {
        (f - s) / s * 100.0
    }
}

/// Wilder's ATR over `bars` (module docs, MACD-V).
pub fn atr(bars: &[Bar], period: u32) -> Vec<f64> {
    let mut st = AtrState::default();
    bars.iter().map(|b| st.push(b, period)).collect()
}

/// MACD-V: `[line, signal, histogram]`, line = `(EMA(fast) − EMA(slow)) / ATR(26) × 100`.
pub fn macd_v(bars: &[Bar], fast: u32, slow: u32, signal: u32) -> [Vec<f64>; 3] {
    let closes: Vec<f64> = bars.iter().map(|b| b.c).collect();
    let f = ema(&closes, fast);
    let s = ema(&closes, slow);
    let a = atr(bars, MACD_V_ATR);
    let line = (0..bars.len())
        .map(|i| macd_v_point(f[i], s[i], a[i]))
        .collect::<Vec<_>>();
    with_signal(line, signal)
}

fn macd_v_point(f: f64, s: f64, atr: f64) -> f64 {
    // `!(atr > 0)` is also true for a NaN ATR (not seeded yet).
    if f.is_nan() || s.is_nan() || atr.is_nan() || atr <= 0.0 {
        f64::NAN
    } else {
        (f - s) / atr * 100.0
    }
}

/// How far back a zero-lag EMA of period `n` reads the price: `floor((n − 1) / 2)`.
pub fn zero_lag_lag(n: u32) -> usize {
    (n.saturating_sub(1) / 2) as usize
}

/// The zero-lag EMA (module docs): the EMA of `2·vᵢ − vᵢ₋ₗ` over dense values (one per bar).
pub fn zlema(values: &[f64], period: u32) -> Vec<f64> {
    let lag = zero_lag_lag(period);
    let x: Vec<f64> = (0..values.len())
        .map(|i| {
            if i >= lag {
                2.0 * values[i] - values[i - lag]
            } else {
                f64::NAN
            }
        })
        .collect();
    ema(&x, period)
}

/// Zero-lag MACD: `[line, signal, histogram]`, line = ZLEMA(fast) − ZLEMA(slow), signal a plain
/// EMA of it.
pub fn zero_lag_macd(values: &[f64], fast: u32, slow: u32, signal: u32) -> [Vec<f64>; 3] {
    let f = zlema(values, fast);
    let s = zlema(values, slow);
    let line = (0..values.len())
        .map(|i| {
            if f[i].is_nan() || s[i].is_nan() {
                f64::NAN
            } else {
                f[i] - s[i]
            }
        })
        .collect::<Vec<_>>();
    with_signal(line, signal)
}

/// Signal area: `[line, signal]`, Classic's line and a SIMPLE `signal`-bar average of it.
pub fn macd_sma_signal(values: &[f64], fast: u32, slow: u32, signal: u32) -> [Vec<f64>; 2] {
    let [line, _, _] = macd(values, fast, slow, signal);
    let sig = sma(&line, signal);
    [line, sig]
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
        IndicatorSpec::Macd {
            fast,
            slow,
            signal,
            style,
        } => {
            let [line, sig, hist] = match style {
                MacdStyle::Classic => macd(&closes, fast, slow, signal),
                MacdStyle::Ppo => ppo(&closes, fast, slow, signal),
                MacdStyle::MacdV => macd_v(bars, fast, slow, signal),
                MacdStyle::ZeroLag => zero_lag_macd(&closes, fast, slow, signal),
                MacdStyle::SignalArea => {
                    let [line, sig] = macd_sma_signal(&closes, fast, slow, signal);
                    return three(line, sig, nan_vec(bars.len()));
                }
            };
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

/// Wilder's average true range (MACD-V).
#[derive(Clone, Copy, Debug)]
struct AtrState {
    prev_c: f64,
    sum: f64,
    n: u32,
    atr: f64,
}

impl Default for AtrState {
    fn default() -> Self {
        AtrState {
            prev_c: f64::NAN,
            sum: 0.0,
            n: 0,
            atr: f64::NAN,
        }
    }
}

impl AtrState {
    fn push(&mut self, b: &Bar, period: u32) -> f64 {
        let tr = if self.prev_c.is_nan() {
            b.h - b.l
        } else {
            (b.h - b.l)
                .max((b.h - self.prev_c).abs())
                .max((b.l - self.prev_c).abs())
        };
        self.prev_c = b.c;
        let p = period as f64;
        if self.n < period {
            self.sum += tr;
            self.n += 1;
            if self.n < period {
                return f64::NAN;
            }
            self.atr = self.sum / p;
        } else {
            self.atr = (self.atr * (p - 1.0) + tr) / p;
        }
        self.atr
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
    /// Classic, PPO and zero-lag (the zero-lag EMAs read their lagged price back from the bars).
    Macd {
        fast: EmaState,
        slow: EmaState,
        signal: EmaState,
    },
    MacdV {
        fast: EmaState,
        slow: EmaState,
        atr: AtrState,
        signal: EmaState,
    },
    /// The simple signal's running sum and window length; the line value leaving the window is
    /// read back from the outputs.
    SignalArea {
        fast: EmaState,
        slow: EmaState,
        sum: f64,
        count: u32,
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
            IndicatorSpec::Macd {
                style: MacdStyle::MacdV,
                ..
            } => State::MacdV {
                fast: EmaState::default(),
                slow: EmaState::default(),
                atr: AtrState::default(),
                signal: EmaState::default(),
            },
            IndicatorSpec::Macd {
                style: MacdStyle::SignalArea,
                ..
            } => State::SignalArea {
                fast: EmaState::default(),
                slow: EmaState::default(),
                sum: 0.0,
                count: 0,
            },
            IndicatorSpec::Macd { .. } => State::Macd {
                fast: EmaState::default(),
                slow: EmaState::default(),
                signal: EmaState::default(),
            },
        }
    }
}

/// Bar `i`'s point from the checkpoint before it. `out` is the points of bars `..i` (signal
/// area reads the line value leaving its window there).
fn step(
    spec: &IndicatorSpec,
    prev: State,
    bars: &[Bar],
    out: &[[f64; 3]],
    i: usize,
) -> (State, [f64; 3]) {
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
            IndicatorSpec::Macd {
                fast,
                slow,
                signal,
                style: MacdStyle::MacdV,
            },
            State::MacdV {
                fast: mut f,
                slow: mut s,
                atr: mut a,
                signal: mut g,
            },
        ) => {
            let fv = f.push(c, fast);
            let sv = s.push(c, slow);
            let av = a.push(&bars[i], MACD_V_ATR);
            let line = macd_v_point(fv, sv, av);
            let sig = if line.is_nan() {
                N
            } else {
                g.push(line, signal)
            };
            let hist = if sig.is_nan() { N } else { line - sig };
            (
                State::MacdV {
                    fast: f,
                    slow: s,
                    atr: a,
                    signal: g,
                },
                [hist, line, sig],
            )
        }
        (
            IndicatorSpec::Macd {
                fast,
                slow,
                signal,
                style: MacdStyle::SignalArea,
            },
            State::SignalArea {
                fast: mut f,
                slow: mut s,
                mut sum,
                mut count,
            },
        ) => {
            let fv = f.push(c, fast);
            let sv = s.push(c, slow);
            let (line, sig) = if !fv.is_nan() && !sv.is_nan() {
                let line = fv - sv;
                // `sma`'s order: add first, then drop the value leaving the window (the line is
                // defined on every bar from its first, so that value is `signal` bars back).
                sum += line;
                count += 1;
                if count > signal {
                    sum -= out[i - signal as usize][0];
                    count = signal;
                }
                let sig = if count == signal {
                    sum / signal as f64
                } else {
                    N
                };
                (line, sig)
            } else {
                (N, N)
            };
            (
                State::SignalArea {
                    fast: f,
                    slow: s,
                    sum,
                    count,
                },
                [line, sig, N],
            )
        }
        (
            IndicatorSpec::Macd {
                fast,
                slow,
                signal,
                style,
            },
            State::Macd {
                fast: mut f,
                slow: mut s,
                signal: mut g,
            },
        ) => {
            let (fv, sv) = match style {
                MacdStyle::ZeroLag => {
                    let (lf, ls) = (zero_lag_lag(fast), zero_lag_lag(slow));
                    let fv = if i >= lf {
                        f.push(2.0 * c - bars[i - lf].c, fast)
                    } else {
                        N
                    };
                    let sv = if i >= ls {
                        s.push(2.0 * c - bars[i - ls].c, slow)
                    } else {
                        N
                    };
                    (fv, sv)
                }
                _ => (f.push(c, fast), s.push(c, slow)),
            };
            let line = match style {
                MacdStyle::Ppo => ppo_point(fv, sv),
                _ if !fv.is_nan() && !sv.is_nan() => fv - sv,
                _ => N,
            };
            let sig = if line.is_nan() {
                N
            } else {
                g.push(line, signal)
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
    /// Bars were dropped from the front (the cap or [`IndicatorSeries::trim_before`]), so the
    /// first [`IndicatorSeries::lookback`] bars can no longer be recomputed from their
    /// checkpoints: see [`IndicatorSeries::restart_at`].
    trimmed: bool,
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
            trimmed: false,
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
            let (next, v) = step(&self.spec, st, &self.bars, &self.out, i);
            self.ckpt.push(next);
            self.out.push(v);
            st = next;
        }
    }

    /// How many bars before bar `i` its step reads back from `bars` (not from the checkpoint):
    /// SMA reads the value leaving its window, `bars[i - period]`; Bollinger re-sums
    /// `bars[i + 1 - period..=i]`; zero-lag MACD reads each EMA's lagged price
    /// `bars[i - lag]`; signal area reads the line value leaving its window, `out[i - signal]`.
    /// The other studies carry everything in the checkpoint.
    fn lookback(&self) -> usize {
        match self.spec {
            IndicatorSpec::Sma { period } => period as usize,
            IndicatorSpec::Bb { period, .. } => (period as usize).saturating_sub(1),
            IndicatorSpec::Macd {
                fast,
                slow,
                style: MacdStyle::ZeroLag,
                ..
            } => zero_lag_lag(fast).max(zero_lag_lag(slow)),
            IndicatorSpec::Macd {
                signal,
                style: MacdStyle::SignalArea,
                ..
            } => signal as usize,
            _ => 0,
        }
    }

    /// Where a change at bar `k` has to recompute from. Normally `k`. But once the front was
    /// dropped, a change within the first [`IndicatorSeries::lookback`] bars needs bars that are
    /// gone: SMA's step would index `bars[k - period]` below 0 (a panic, under the market core
    /// lock, which poisons it and takes the whole server down), and Bollinger would quietly turn
    /// those points into NaN next to untouched neighbours. So such a change recomputes the whole
    /// series from the bars it still holds (a re-anchor at the current front, the same values a
    /// fresh build over these bars gives). Rare: a revision of one of the oldest retained bars.
    fn restart_at(&self, k: usize) -> usize {
        if self.trimmed && k < self.lookback() {
            0
        } else {
            k
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
        self.trimmed = true;
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
                let k = self.restart_at(k);
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
                let k = self.restart_at(k);
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
        let k = self.restart_at(k);
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

    /// A revision of one of the oldest retained bars after the front was dropped. SMA used to
    /// index below 0 here (a panic under the market core lock); Bollinger turned points NaN.
    /// Studies that read back from the bars re-anchor at the current front (= a fresh build);
    /// the others keep their anchor (= the batch over the whole history, dropped part included).
    #[test]
    fn a_revision_at_the_trimmed_front_never_reads_dropped_bars() {
        let specs = [
            IndicatorSpec::Sma { period: 20 },
            IndicatorSpec::Bb {
                period: 20,
                mult: 2.0,
            },
            IndicatorSpec::Ema { period: 20 },
            IndicatorSpec::Rsi { period: 14 },
            IndicatorSpec::Vwap,
        ]
        .into_iter()
        .chain(MacdStyle::ALL.map(|style| IndicatorSpec::Macd {
            fast: 12,
            slow: 26,
            signal: 9,
            style,
        }));
        for spec in specs {
            let mut full: Vec<Bar> = (0..200)
                .map(|i| bar(i, 10.0 + (i % 13) as f64 * 0.07))
                .collect();
            let mut s = IndicatorSeries::build(spec, full.clone(), 4000);
            assert_eq!(s.trim_before(100 * 60_000), 100);
            let reads_back = s.lookback() > 0;
            // An edit, an insert-free removal and a re-add, each within the lookback.
            let edits = [
                BarEdit::Upsert(bar(103, 42.0)),
                BarEdit::Remove(101 * 60_000),
                BarEdit::Upsert(bar(101, 9.0)),
            ];
            for e in edits {
                s.apply(e);
                match e {
                    BarEdit::Upsert(b) => match full.binary_search_by_key(&b.t_ms, |x| x.t_ms) {
                        Ok(i) => full[i] = b,
                        Err(i) => full.insert(i, b),
                    },
                    BarEdit::Remove(t) => full.retain(|x| x.t_ms != t),
                }
                let want = if reads_back {
                    batch(&spec, s.bars())
                } else {
                    batch(&spec, &full)[full.len() - s.len()..].to_vec()
                };
                for (i, w) in want.iter().enumerate() {
                    let got = s.values(i);
                    for j in 0..got.len() {
                        assert_eq!(got[j].to_bits(), w[j].to_bits(), "{} bar {i}", spec.key());
                    }
                }
            }
        }
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

    // --- MACD styles: small synthetic series with values worked out by hand -------------------

    fn macd_spec(fast: u32, slow: u32, signal: u32, style: MacdStyle) -> IndicatorSpec {
        IndicatorSpec::Macd {
            fast,
            slow,
            signal,
            style,
        }
    }

    fn close_to(a: f64, b: f64) -> bool {
        (a - b).abs() <= 1e-9 * b.abs().max(1.0)
    }

    /// The values of `spec` over `bars`, built incrementally (the server's path), by line.
    fn built(spec: IndicatorSpec, bars: &[Bar]) -> Vec<Vec<f64>> {
        let s = IndicatorSeries::build(spec, bars.to_vec(), 4000);
        (0..spec.lines().len())
            .map(|j| (0..s.len()).map(|i| s.values(i)[j]).collect())
            .collect()
    }

    #[test]
    fn macd_styles_parse_key_and_name_their_lines() {
        let key = |style: Value| {
            IndicatorSpec::parse(&json!({"id": "m", "kind": "macd", "style": style}))
                .map(|(_, s)| s.key())
                .unwrap()
        };
        // Classic keeps the key every older client looks up; 4-color is Classic's values.
        assert_eq!(key(json!("classic")), "macd:12:26:9");
        assert_eq!(key(json!("four-color")), "macd:12:26:9");
        assert_eq!(key(json!("ppo")), "macd:12:26:9:ppo");
        assert_eq!(key(json!("macd-v")), "macd:12:26:9:macd-v");
        assert_eq!(key(json!("zero-lag")), "macd:12:26:9:zero-lag");
        assert_eq!(key(json!("signal-area")), "macd:12:26:9:signal-area");
        // A style from a newer client, or junk, is Classic rather than a dropped study.
        assert_eq!(key(json!("next-year")), "macd:12:26:9");
        assert_eq!(key(json!(7)), "macd:12:26:9");
        let lines = |s: MacdStyle| -> Vec<&str> {
            macd_spec(12, 26, 9, s)
                .lines()
                .iter()
                .map(|l| l.0)
                .collect()
        };
        assert_eq!(lines(MacdStyle::Ppo), ["Hist", "MACD", "Signal"]);
        assert_eq!(lines(MacdStyle::SignalArea), ["MACD", "Signal"]);
    }

    #[test]
    fn ppo_is_the_macd_as_a_percentage_of_the_slow_average() {
        // EMA(1) = the close; EMA(2) seeds at bar 1 with (10 + 30) / 2 = 20.
        let bars: Vec<Bar> = [10.0, 30.0, 30.0]
            .iter()
            .enumerate()
            .map(|(i, &c)| bar(i as i64, c))
            .collect();
        let v = built(macd_spec(1, 2, 1, MacdStyle::Ppo), &bars);
        let (hist, line, sig) = (&v[0], &v[1], &v[2]);
        assert!(line[0].is_nan());
        assert_eq!(line[1], 50.0, "(30 - 20) / 20 x 100");
        // Bar 2: EMA(2) = 30*2/3 + 20/3 = 80/3, so (30 - 80/3) / (80/3) x 100 = 12.5.
        assert!(close_to(line[2], 12.5), "{}", line[2]);
        assert_eq!(sig[1], line[1], "a 1-bar signal is the line");
        assert_eq!(hist[2], 0.0);
        // The same move on a stock ten times the price reads the same, unlike Classic's.
        let tenfold: Vec<Bar> = bars
            .iter()
            .map(|b| bar(b.t_ms / 60_000, b.c * 10.0))
            .collect();
        let p10 = built(macd_spec(1, 2, 1, MacdStyle::Ppo), &tenfold);
        assert!(close_to(p10[1][2], line[2]));
    }

    #[test]
    fn macd_v_divides_by_wilder_atr_26() {
        let b = |i: i64, c: f64| Bar {
            h: c + 0.5,
            l: c - 0.5,
            ..bar(i, c)
        };
        // 26 calm bars: every true range is 1, so ATR seeds at 1 on bar 25.
        let mut bars: Vec<Bar> = (0..26).map(|i| b(i, 10.0)).collect();
        // Bar 26 jumps to 12: TR = max(1, |12.5 - 10|, |11.5 - 10|) = 2.5,
        // ATR = (1*25 + 2.5) / 26; EMA(1) = 12, EMA(2) = 12*2/3 + 10/3 = 34/3.
        bars.push(b(26, 12.0));
        let a = atr(&bars, MACD_V_ATR);
        assert!(a[24].is_nan());
        assert_eq!(a[25], 1.0);
        assert!(close_to(a[26], 27.5 / 26.0));
        let v = built(macd_spec(1, 2, 1, MacdStyle::MacdV), &bars);
        let line = &v[1];
        assert!(line[24].is_nan(), "no value before ATR is seeded");
        assert_eq!(line[25], 0.0);
        let want = (12.0 - 34.0 / 3.0) / (27.5 / 26.0) * 100.0;
        assert!(close_to(line[26], want), "{} vs {want}", line[26]);
        // A flat tape (ATR 0) has no MACD-V at all rather than an infinite one.
        let flat: Vec<Bar> = (0..40)
            .map(|i| Bar {
                h: 10.0,
                l: 10.0,
                ..bar(i, 10.0)
            })
            .collect();
        let v = built(macd_spec(12, 26, 9, MacdStyle::MacdV), &flat);
        assert!(v.iter().flatten().all(|x| x.is_nan()));
    }

    #[test]
    fn zero_lag_emas_have_no_lag_on_a_ramp() {
        // On a straight ramp an EMA(n) seeded by its SMA trails by exactly (n - 1) / 2 bars; the
        // zero-lag EMA's de-lagged input (2*c[i] - c[i - lag], lag = (n - 1) / 2) cancels that
        // exactly for odd n. So ZLEMA(5) of a ramp is the ramp, and the zero-lag MACD(3, 5) is 0
        // where Classic's EMA(3) - EMA(5) is a constant (5 - 3) / 2 = 1.
        let ramp: Vec<f64> = (0..60).map(|i| 10.0 + i as f64).collect();
        let z = zlema(&ramp, 5);
        assert!(z[5].is_nan(), "lag 2 + 5 values before the seed");
        for i in 6..60 {
            assert!(close_to(z[i], ramp[i]), "bar {i}: {}", z[i]);
        }
        let bars: Vec<Bar> = ramp
            .iter()
            .enumerate()
            .map(|(i, &c)| bar(i as i64, c))
            .collect();
        let zl = built(macd_spec(3, 5, 4, MacdStyle::ZeroLag), &bars);
        let cl = built(macd_spec(3, 5, 4, MacdStyle::Classic), &bars);
        for i in 20..60 {
            assert!(zl[1][i].abs() < 1e-9, "zero-lag line at {i}: {}", zl[1][i]);
            assert!(close_to(cl[1][i], 1.0), "classic line at {i}: {}", cl[1][i]);
        }
        assert_eq!(zero_lag_lag(1), 0);
        assert_eq!(zero_lag_lag(12), 5);
        assert_eq!(zero_lag_lag(26), 12);
    }

    #[test]
    fn signal_area_signal_is_a_simple_average_of_the_line() {
        let bars: Vec<Bar> = (0..80)
            .map(|i| bar(i, 10.0 + ((i * 7) % 11) as f64 * 0.13))
            .collect();
        let v = built(macd_spec(12, 26, 9, MacdStyle::SignalArea), &bars);
        let (line, sig) = (&v[0], &v[1]);
        let closes: Vec<f64> = bars.iter().map(|b| b.c).collect();
        let [classic, _, _] = macd(&closes, 12, 26, 9);
        // The line is Classic's; the signal waits for 9 line values (bar 25 + 8).
        for i in 0..80 {
            assert_eq!(line[i].to_bits(), classic[i].to_bits(), "bar {i}");
        }
        assert!(sig[32].is_nan());
        for i in 33..80 {
            let mean = line[i - 8..=i].iter().sum::<f64>() / 9.0;
            assert!(close_to(sig[i], mean), "bar {i}: {} vs {mean}", sig[i]);
        }
    }

    #[test]
    fn every_macd_style_builds_incrementally_as_its_batch() {
        let bars: Vec<Bar> = (0..120)
            .map(|i| {
                let c = 10.0 + ((i * 13) % 17) as f64 * 0.09;
                Bar {
                    h: c + 0.05 + (i % 3) as f64 * 0.04,
                    l: c - 0.07,
                    ..bar(i, c)
                }
            })
            .collect();
        for style in MacdStyle::ALL {
            let spec = macd_spec(5, 11, 4, style);
            let want = batch(&spec, &bars);
            let mut s = IndicatorSeries::build(spec, bars[..60].to_vec(), 4000);
            for b in &bars[60..] {
                s.upsert(*b);
            }
            for (i, w) in want.iter().enumerate() {
                for (j, got) in s.values(i).iter().enumerate() {
                    assert_eq!(
                        got.to_bits(),
                        w[j].to_bits(),
                        "{} bar {i} line {j}",
                        spec.key()
                    );
                }
            }
        }
    }
}

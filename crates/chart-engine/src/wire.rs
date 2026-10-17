//! Output types: the server → client wire format of `docs/architecture/simulator-and-chart.md`.
//!
//! JSON, camelCase, times in **epoch milliseconds UTC**, prices as JSON numbers in dollars
//! (converted from exact micro-dollars only here, for display). Messages carry `"type"`:
//! `chart.snapshot`, `chart.update`, `chart.remove`, `chart.gap`, `tape.trade`.
//!
//! Client rules:
//! - a snapshot is authoritative and replaces everything for its symbol/timeframe;
//! - `chart.update` upserts the candle at `candle.t` (it may be an older, already-final candle that
//!   a late trade, correction or cancel revised); a candle at `t` supersedes any gap covering `t`;
//! - `chart.remove` deletes whatever is at `t` (a cancel/correction emptied that interval);
//! - `chart.gap` adds a gap marker; `profile` rows carry **absolute** bin totals, not deltas.

use crate::quality::DataQuality;
use crate::timeframe::Timeframe;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CandleWire {
    /// Interval start, epoch ms.
    pub t: i64,
    pub o: f64,
    pub h: f64,
    pub l: f64,
    pub c: f64,
    /// Sum of volume-qualifying sizes (includes volume-only prints such as odd lots).
    pub v: u64,
    /// Number of volume-qualifying trades.
    pub n: u64,
    /// `true` once the symbol's watermark (or the clock) has passed the interval end.
    #[serde(rename = "final")]
    pub is_final: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GapReason {
    /// No trade at all in the interval(s).
    NoTrades,
    /// Only volume-only prints (e.g. odd lots, average-price): there is volume but no qualifying
    /// price, so no candle is drawn (Alpaca emits no bar either).
    VolumeOnlyTrades,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GapWire {
    /// First missing interval start, epoch ms.
    pub t: i64,
    /// Exclusive end (start of the next interval that has data), epoch ms.
    pub end: i64,
    pub reason: GapReason,
    /// Volume inside the gap (non-zero only for `volume_only_trades`).
    pub v: u64,
    /// Number of timeframe intervals the gap spans.
    pub intervals: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileRow {
    /// Bin start (`floor(price / bin) * bin`), dollars.
    pub p: f64,
    /// Absolute volume in the bin.
    pub v: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ValueArea {
    /// Lowest bin start inside the value area.
    pub low: f64,
    /// Highest bin start inside the value area.
    pub high: f64,
    pub v: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileWire {
    pub bin: f64,
    /// The session id the profile is anchored to, e.g. `"2026-10-15/regular"`.
    pub anchor: String,
    /// Non-empty bins, ascending by price.
    pub rows: Vec<ProfileRow>,
    pub total: u64,
    /// Point of control: the bin with the most volume (lowest price on a tie).
    pub poc: Option<f64>,
    /// 70% value area around the POC.
    pub value_area: Option<ValueArea>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionInfo {
    pub id: String,
    pub start: i64,
    pub end: i64,
}

/// Replay/live clock, filled in by the server (the engine does not know the mode).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClockInfo {
    pub mode: String,
    pub now: i64,
    pub speed: f64,
    pub playing: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChartSnapshot {
    /// Always `"chart.snapshot"`.
    #[serde(rename = "type")]
    pub kind: String,
    pub symbol: String,
    pub timeframe: Timeframe,
    /// The symbol's current session (the latest session it traded in).
    pub session: Option<SessionInfo>,
    /// Every retained candle of this timeframe (all sessions, ascending).
    pub candles: Vec<CandleWire>,
    pub gaps: Vec<GapWire>,
    /// Profile of the current session.
    pub profile: Option<ProfileWire>,
    /// Data-quality counters of the current session.
    pub quality: DataQuality,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clock: Option<ClockInfo>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CandleUpdate {
    pub symbol: String,
    pub timeframe: Timeframe,
    pub session: String,
    pub candle: CandleWire,
    pub profile_bin: f64,
    /// Profile bins this event changed (absolute totals), in `session`'s profile.
    pub profile: Vec<ProfileRow>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CandleRemove {
    pub symbol: String,
    pub timeframe: Timeframe,
    pub session: String,
    /// Interval start, epoch ms.
    pub t: i64,
    pub profile_bin: f64,
    pub profile: Vec<ProfileRow>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GapUpdate {
    pub symbol: String,
    pub timeframe: Timeframe,
    pub session: String,
    pub gap: GapWire,
    pub profile_bin: f64,
    pub profile: Vec<ProfileRow>,
}

/// One row of the trade tape, showing how the engine counted the print.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TapeTrade {
    pub symbol: String,
    /// Execution time, epoch ms.
    pub t: i64,
    /// Execution time, epoch ns (the ordering key; ms loses ties).
    pub ts_ns: i64,
    pub p: f64,
    pub s: u64,
    pub exchange: String,
    pub seq: u64,
    pub conditions: Vec<String>,
    /// `ohlc_and_volume`, `volume_only` or `excluded:<reason>` where reason is `duplicate`,
    /// `late`, `outside_session` or `condition_<code>`.
    pub counted: String,
    /// Why it was restricted: `odd_lot`, `condition_<code>`; `None` for a regular sale.
    pub reason: Option<String>,
    pub session: Option<String>,
}

/// One increment produced by [`crate::ChartEngine::apply`] / [`crate::ChartEngine::advance_clock`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ChartUpdate {
    #[serde(rename = "chart.update")]
    Candle(CandleUpdate),
    #[serde(rename = "chart.remove")]
    Remove(CandleRemove),
    #[serde(rename = "chart.gap")]
    Gap(GapUpdate),
    #[serde(rename = "tape.trade")]
    Tape(TapeTrade),
}

impl ChartUpdate {
    pub fn timeframe(&self) -> Option<Timeframe> {
        match self {
            ChartUpdate::Candle(u) => Some(u.timeframe),
            ChartUpdate::Remove(u) => Some(u.timeframe),
            ChartUpdate::Gap(u) => Some(u.timeframe),
            ChartUpdate::Tape(_) => None,
        }
    }

    pub fn as_candle(&self) -> Option<&CandleUpdate> {
        match self {
            ChartUpdate::Candle(u) => Some(u),
            _ => None,
        }
    }

    pub fn as_tape(&self) -> Option<&TapeTrade> {
        match self {
            ChartUpdate::Tape(u) => Some(u),
            _ => None,
        }
    }
}

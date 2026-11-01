//! Output types: the server → client wire format of `docs/architecture/simulator-and-chart.md`.
//!
//! JSON, camelCase, times in **epoch milliseconds UTC**, prices as JSON numbers in dollars
//! (converted from exact micro-dollars only here, for display). Messages carry `"type"`:
//! `chart.snapshot`, `chart.update`, `chart.remove`, `chart.gap`, `tape.trade`, `chart.halt`,
//! `chart.luld`.
//!
//! Client rules:
//! - a snapshot is authoritative and replaces everything for its symbol/timeframe;
//! - `chart.update` upserts the candle at `candle.t` (it may be an older, already-final candle that
//!   a late trade, correction or cancel revised); a candle at `t` supersedes any gap covering `t`;
//! - `chart.remove` deletes whatever is at `t` (a cancel/correction emptied that interval);
//! - `chart.gap` adds a gap marker; `profile` rows carry **absolute** bin totals, not deltas;
//! - an increment whose `profile` rows are non-empty may also carry `total`, `poc` and
//!   `valueArea` ([`ProfileDerivedWire`]): the whole session profile's, at the frame's
//!   `profileBin`, read when the frame was sent. Present = take them as sent (as from a snapshot);
//!   absent (the engine never sets them, an older server) = recompute from the rows;
//! - `chart.halt` and `chart.luld` do not depend on the timeframe (docs/architecture/
//!   halts-luld-fills.md): a client upserts a halt by `start` and a LULD point by `t`.
//!
//! **Exchange-time ns twins** (`times.ns.v1` in the server's `welcome.features`): every time
//! below also goes out in the engine's own resolution, as `tNs` beside `t`, `endNs` beside `end`,
//! `startNs` beside `start` (and `nowNs` on [`ClockInfo`]). Rust is the only clock and it counts
//! exchange ns; the ms field is that instant floored (`UnixNanos::millis`), kept because every
//! phone build installed before the twins reads only it. A client prefers the twin when present
//! and falls back to the ms field when absent (an older server). The twin is an exact integer in
//! Rust but **not in JS** (an epoch-ns number is past 2^53, so JSON.parse rounds it to the
//! nearest 256 ns): clients turn it back into ms with a slack that absorbs that rounding
//! (chart-core `wireMs`), never key on the raw number. Deserializing JSON without the twins
//! (an older golden file) gives 0 / `None`.

use crate::quality::DataQuality;
use crate::timeframe::Timeframe;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CandleWire {
    /// Interval start, epoch ms.
    pub t: i64,
    /// Interval start, exchange-time ns (module docs).
    #[serde(default)]
    pub t_ns: i64,
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
    /// `t` in exchange-time ns (module docs).
    #[serde(default)]
    pub t_ns: i64,
    /// Exclusive end (start of the next interval that has data), epoch ms.
    pub end: i64,
    /// `end` in exchange-time ns.
    #[serde(default)]
    pub end_ns: i64,
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

/// The profile's derived figures on an increment (module docs). `total` is always there when the
/// struct is; `poc` and `valueArea` are `null` once cancels have emptied the session's profile.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileDerivedWire {
    pub total: u64,
    pub poc: Option<f64>,
    pub value_area: Option<ValueArea>,
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
    /// Epoch ms.
    pub start: i64,
    /// `start` in exchange-time ns (module docs).
    #[serde(default)]
    pub start_ns: i64,
    /// Epoch ms.
    pub end: i64,
    /// `end` in exchange-time ns.
    #[serde(default)]
    pub end_ns: i64,
}

/// Replay/live clock, filled in by the server (the engine does not know the mode).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClockInfo {
    pub mode: String,
    /// Epoch ms (what every client read before `nowNs`).
    pub now: i64,
    /// The same instant in exchange-time ns, the server's own resolution: the clock clients
    /// measure ages against (crates/server/src/time_marks.rs). Additive; absent from older
    /// servers' frames.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub now_ns: Option<i64>,
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
    /// Every retained halt of the symbol, ascending by `start` (the same for every timeframe).
    #[serde(default)]
    pub halts: Vec<HaltWire>,
    /// The LULD band changes of the current trading date's regular session, ascending.
    #[serde(default)]
    pub luld: Vec<LuldWire>,
    /// The official close of the trading day before the current session's date, in dollars, when
    /// the server has one (`daily-closes.json`, market_archive::closes); omitted otherwise. The
    /// engine never sets it: it sees only the tape, and a single-day replay holds no prior day.
    /// Clients measure change % against it and draw it as the "Prev close" level.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prev_close: Option<f64>,
}

/// A trading halt. `end` is `None` while the halt is still on.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HaltWire {
    /// The halt's `event_ts`, epoch ms.
    pub start: i64,
    /// The halt's `event_ts` as received, exchange-time ns (module docs): unlike a bar time it
    /// may fall inside a millisecond.
    #[serde(default)]
    pub start_ns: i64,
    /// The resume's `event_ts`, epoch ms.
    pub end: Option<i64>,
    /// The resume's `event_ts`, exchange-time ns; `null` with `end`.
    #[serde(default)]
    pub end_ns: Option<i64>,
    /// Provider reason (Alpaca `rc`), e.g. `"LUDP"`.
    pub reason: String,
    /// Provider status code of the halt (Alpaca `sc`), e.g. `"H"`.
    pub code: String,
}

/// LULD bands in effect from `t` until the next point. `up`/`down` (and `ref`) `None` means no
/// bands from `t` (a halt, or the close). Modelled, not the exchange's numbers.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LuldWire {
    /// Epoch ms.
    pub t: i64,
    /// `t` in exchange-time ns (module docs).
    #[serde(default)]
    pub t_ns: i64,
    /// The reference price (5-minute mean), dollars.
    #[serde(rename = "ref")]
    pub reference: Option<f64>,
    pub up: Option<f64>,
    pub down: Option<f64>,
}

/// `chart.halt`: a halt started, or (same `start`, `end` now set) resumed.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HaltUpdate {
    pub symbol: String,
    pub halt: HaltWire,
}

/// `chart.luld`: a LULD band change.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LuldUpdate {
    pub symbol: String,
    pub point: LuldWire,
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
    /// Set by the server (never the engine) when `profile` is non-empty: module docs.
    #[serde(flatten, default, skip_serializing_if = "Option::is_none")]
    pub derived: Option<ProfileDerivedWire>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CandleRemove {
    pub symbol: String,
    pub timeframe: Timeframe,
    pub session: String,
    /// Interval start, epoch ms.
    pub t: i64,
    /// `t` in exchange-time ns (module docs).
    #[serde(default)]
    pub t_ns: i64,
    pub profile_bin: f64,
    pub profile: Vec<ProfileRow>,
    /// Set by the server (never the engine) when `profile` is non-empty: module docs.
    #[serde(flatten, default, skip_serializing_if = "Option::is_none")]
    pub derived: Option<ProfileDerivedWire>,
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
    /// Set by the server (never the engine) when `profile` is non-empty: module docs.
    #[serde(flatten, default, skip_serializing_if = "Option::is_none")]
    pub derived: Option<ProfileDerivedWire>,
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
    #[serde(rename = "chart.halt")]
    Halt(HaltUpdate),
    #[serde(rename = "chart.luld")]
    Luld(LuldUpdate),
}

impl ChartUpdate {
    pub fn timeframe(&self) -> Option<Timeframe> {
        match self {
            ChartUpdate::Candle(u) => Some(u.timeframe),
            ChartUpdate::Remove(u) => Some(u.timeframe),
            ChartUpdate::Gap(u) => Some(u.timeframe),
            ChartUpdate::Tape(_) | ChartUpdate::Halt(_) | ChartUpdate::Luld(_) => None,
        }
    }

    pub fn symbol(&self) -> &str {
        match self {
            ChartUpdate::Candle(u) => &u.symbol,
            ChartUpdate::Remove(u) => &u.symbol,
            ChartUpdate::Gap(u) => &u.symbol,
            ChartUpdate::Tape(u) => &u.symbol,
            ChartUpdate::Halt(u) => &u.symbol,
            ChartUpdate::Luld(u) => &u.symbol,
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

    pub fn as_halt(&self) -> Option<&HaltWire> {
        match self {
            ChartUpdate::Halt(u) => Some(&u.halt),
            _ => None,
        }
    }

    pub fn as_luld(&self) -> Option<&LuldWire> {
        match self {
            ChartUpdate::Luld(u) => Some(&u.point),
            _ => None,
        }
    }
}

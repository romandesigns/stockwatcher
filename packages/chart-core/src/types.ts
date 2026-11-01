// Wire format, server → client. Mirrors crates/chart-engine/src/wire.rs
// (the source of truth; JSON, camelCase) and Appendix A of
// docs/architecture/simulator-and-chart.md. Change both sides together; the
// contract test (wire.contract.test.ts) reads the engine's golden JSON.
//
// These types live in chart-core rather than @stockwatcher/shared because
// only chart consumers need them and chart-core must stay dependency-free;
// shared can re-export them once the server forwards them.
//
// Times are epoch ms UTC. Newer servers send each one's exchange-ns twin beside it
// (`tNs`, `startNs`, `endNs`, `nowNs`, `tsNs`; `times.ns.v1`): parseChartMessage
// rewrites the ms fields from them (wireTime.ts), so the rest of the client reads
// `t` as before. An epoch-ns number LOSES PRECISION in JS: never key on a twin
// directly, only through `wireMs`. Prices are dollars as
// JSON numbers, for display only; the engine keeps exact micro-dollars.
// Sessions and labels are computed in America/New_York by time.ts.

import type { IndicatorSpec } from "./indicators.ts";
import { preferNsTimes } from "./wireTime.ts";

export type Timeframe = "15s" | "30s" | "1m" | "5m";

export const TIMEFRAMES: readonly Timeframe[] = ["15s", "30s", "1m", "5m"];

export const TIMEFRAME_MS: Readonly<Record<Timeframe, number>> = {
  "15s": 15_000,
  "30s": 30_000,
  "1m": 60_000,
  "5m": 300_000,
};

/** One candle over the half-open interval [t, t + timeframe). Built by the server only. */
export interface Candle {
  t: number;
  /** `t` in exchange-time ns (newer servers; crates/chart-engine/src/wire.rs). */
  tNs?: number;
  o: number;
  h: number;
  l: number;
  c: number;
  /** Sum of volume-qualifying sizes (includes volume-only prints such as odd lots). */
  v: number;
  /** Number of volume-qualifying trades. */
  n: number;
  /** True once the symbol's watermark (or the clock) passed the interval end. */
  final: boolean;
}

/**
 * - `no_trades`: no trade at all in the interval(s);
 * - `volume_only_trades`: only volume-only prints, so there is volume (`v`)
 *   but no qualifying price and no candle (Alpaca emits no bar either).
 */
export type GapReason = "no_trades" | "volume_only_trades";

/**
 * A run of empty intervals [t, end) inside one session. Never drawn as fake
 * candles; on the index-based time axis a gap occupies exactly ONE slot
 * however many `intervals` it spans, so a long lull does not squeeze the
 * candles around it.
 */
export interface Gap {
  t: number;
  /** `t` and `end` in exchange-time ns (newer servers; crates/chart-engine/src/wire.rs). */
  tNs?: number;
  end: number;
  endNs?: number;
  reason: GapReason;
  v: number;
  intervals: number;
}

export type SessionKind = "overnight" | "pre" | "regular" | "after";

export interface SessionRef {
  /** e.g. "2026-10-15/regular" */
  id: string;
  start: number;
  /** `start` and `end` in exchange-time ns (newer servers; crates/chart-engine/src/wire.rs). */
  startNs?: number;
  end: number;
  endNs?: number;
}

/** One profile bin: bin start `p` = floor(price / bin) * bin, absolute volume `v`. */
export interface ProfileRow {
  p: number;
  v: number;
}

/** 70 % value area: lowest and highest BIN START inside it, and the volume inside. */
export interface ValueArea {
  low: number;
  high: number;
  v: number;
}

export interface ProfileWire {
  bin: number;
  /** Session id the profile accumulates over; independent of the candle timeframe. */
  anchor: string;
  /** Non-empty bins, ascending by price. */
  rows: ProfileRow[];
  total: number;
  /** Bin with the most volume (lowest price on a tie). */
  poc: number | null;
  valueArea: ValueArea | null;
}

/** Data-quality counters of the current session (crates/chart-engine/src/quality.rs). */
export interface DataQuality {
  duplicates: number;
  lateExcluded: number;
  lateApplied: number;
  outOfOrderReordered: number;
  correctionsApplied: number;
  correctionsUnapplied: number;
  /** A correction whose id fits several retained prints, or none of them by price and size (venue D: FINRA TRFs reuse ids); not applied. docs/architecture/trade-identity.md */
  correctionsAmbiguous: number;
  cancelsApplied: number;
  cancelsUnapplied: number;
  /** As correctionsAmbiguous, for a cancel/error. */
  cancelsAmbiguous: number;
  conditionExcluded: number;
  volumeOnly: number;
  unrecognizedConditions: number;
  outsideSession: number;
  /** A resume with no open halt, or a second halt while halted (docs/architecture/halts-luld-fills.md). */
  haltsUnmatched: number;
}

export type ClockMode = "replay" | "live";

/** Filled in by the server (the engine does not know the mode). */
export interface Clock {
  mode: ClockMode | (string & {});
  /** Epoch ms. */
  now: number;
  /** The same instant in exchange-time ns (newer servers): the server's own clock. */
  nowNs?: number;
  /** Replay speed multiplier; 0 means "max" (client convention, see ReplayControl). */
  speed: number;
  playing: boolean;
}

/** A trading halt (docs/architecture/halts-luld-fills.md). end null = still halted. */
export interface Halt {
  start: number;
  /** `start` and `end` in exchange-time ns (newer servers; crates/chart-engine/src/wire.rs); may fall inside a ms. */
  startNs?: number;
  end: number | null;
  endNs?: number | null;
  reason: string;
  code: string;
}

/** LULD bands in effect from t until the next point; up/down null = no bands from t. */
export interface LuldPoint {
  t: number;
  /** `t` in exchange-time ns (newer servers; crates/chart-engine/src/wire.rs). */
  tNs?: number;
  ref: number | null;
  up: number | null;
  down: number | null;
}

export interface ChartSnapshot {
  type: "chart.snapshot";
  symbol: string;
  timeframe: Timeframe;
  /** The symbol's current session; null before it ever traded. */
  session: SessionRef | null;
  /** Every retained candle of this timeframe, all sessions, ascending. */
  candles: Candle[];
  gaps: Gap[];
  /** Profile of the current session. */
  profile: ProfileWire | null;
  quality: DataQuality;
  clock?: Clock;
  /** Retained halts, ascending (timeframe-independent). Absent from servers that predate halts. */
  halts?: Halt[];
  /** LULD band changes of the current session, ascending (timeframe-independent). */
  luld?: LuldPoint[];
  /**
   * The official close of the trading day before the current session's date,
   * when the server has one (its daily-closes.json); absent otherwise. The
   * change-% reference brokers use, and the "Prev close" level.
   */
  prevClose?: number;
}

/**
 * The whole session profile's figures at the increment's `profileBin`
 * (chart_engine::wire::ProfileDerivedWire): sent by servers since the
 * profile-figures change on every increment whose `profile` rows are
 * non-empty. `total` present = take all three as sent, as from a snapshot
 * (`poc`/`valueArea` null once cancels emptied the profile); absent (an
 * older server, or a frame that changed no rows) = recompute from the rows.
 */
export interface ProfileFigures {
  total?: number;
  poc?: number | null;
  valueArea?: ValueArea | null;
}

/**
 * Upserts the candle at candle.t. It may be an older, already-final candle
 * revised by a late trade, correction or cancel. A candle at t supersedes any
 * gap covering t. `profile` holds the bins this event touched, as ABSOLUTE
 * totals in `session`'s profile (replace, never add).
 */
export interface ChartUpdate extends ProfileFigures {
  type: "chart.update";
  symbol: string;
  timeframe: Timeframe;
  session: string;
  candle: Candle;
  profileBin: number;
  profile: ProfileRow[];
}

/** A cancel or correction emptied the interval at t: delete whatever is there. */
export interface ChartRemove extends ProfileFigures {
  type: "chart.remove";
  symbol: string;
  timeframe: Timeframe;
  session: string;
  t: number;
  /** `t` in exchange-time ns (newer servers; crates/chart-engine/src/wire.rs). */
  tNs?: number;
  profileBin: number;
  profile: ProfileRow[];
}

/** A gap opened. */
export interface ChartGap extends ProfileFigures {
  type: "chart.gap";
  symbol: string;
  timeframe: Timeframe;
  session: string;
  gap: Gap;
  profileBin: number;
  profile: ProfileRow[];
}

/**
 * How the engine counted a print: toward OHLC and volume, volume only, or not
 * at all. Excluded reasons: `duplicate`, `late`, `outside_session`,
 * `condition_<code>`.
 */
export type Counted = "ohlc_and_volume" | "volume_only" | `excluded:${string}`;

export interface TapeTrade {
  type: "tape.trade";
  symbol: string;
  /** Execution time, epoch ms. */
  t: number;
  /** Execution time, epoch ns. Lossy as a JS number: never a key (parseChartMessage turns it into `t` with wireTime.ts `wireMs`). */
  tsNs: number;
  p: number;
  s: number;
  exchange: string;
  seq: number;
  conditions: string[];
  counted: Counted;
  /** Why it was restricted: `odd_lot`, `condition_<code>`; null for a regular sale. */
  reason: string | null;
  session: string | null;
}

/**
 * Replay/live clock tick. NOT produced by chart-engine: the server owns the
 * clock (crates/server/src/market.rs). Sent to every client on change and
 * about 4 Hz while replay plays; the snapshot carries `clock` too.
 */
export interface ClockMessage {
  type: "replay.clock";
  clock: Clock;
}

/** A halt started, or (same start, end set) resumed. Sent to every subscriber of the symbol. */
export interface ChartHalt {
  type: "chart.halt";
  symbol: string;
  halt: Halt;
}

/** A LULD band change. Sent to every subscriber of the symbol. */
export interface ChartLuld {
  type: "chart.luld";
  symbol: string;
  point: LuldPoint;
}

export type ChartIncrement = ChartUpdate | ChartRemove | ChartGap;
export type ChartMessage = ChartSnapshot | ChartIncrement | TapeTrade | ClockMessage | ChartHalt | ChartLuld | ChartIndicatorsSnapshot | ChartIndicatorIncrement | ChartLevels | ChartZones;

// chart.indicators / chart.indicator come only to a client that sent `indicators` (indicators.v1),
// chart.levels / chart.zones only to one that sent `levels` / `zones` (levels.v1, zones.v1).
const CHART_TYPES = new Set(["chart.snapshot", "chart.update", "chart.remove", "chart.gap", "tape.trade", "replay.clock", "chart.halt", "chart.luld", "chart.indicators", "chart.indicator", "chart.levels", "chart.zones"]);

/** Parses one frame (string or already-parsed object); null if it is not a chart message. */
export function parseChartMessage(raw: unknown): ChartMessage | null {
  let value: unknown = raw;
  if (typeof raw === "string") {
    try {
      value = JSON.parse(raw);
    } catch {
      return null;
    }
  }
  if (typeof value !== "object" || value === null) return null;
  const type = (value as { type?: unknown }).type;
  // The exchange-ns twins win over the ms fields when present (wireTime.ts).
  return typeof type === "string" && CHART_TYPES.has(type) ? preferNsTimes(value as ChartMessage) : null;
}

/**
 * Client → server subscription (architecture doc: `chart.subscribe`). A connection holds up to
 * MAX_CHARTS_PER_CONNECTION charts, one per `id` (absent = the single default slot); frames come
 * once per distinct chart and are routed by their symbol, timeframe and profileBin, not by slot
 * (crates/server/src/ws.rs).
 */
export interface ChartSubscribe {
  type: "chart.subscribe";
  symbol: string;
  timeframe: Timeframe;
  profileBin: number;
  /** The chart slot (≤ 32 chars). */
  id?: string;
  /**
   * The slot's effective indicator list, sent ONLY when the welcome advertised `indicators.v1`
   * (shared `INDICATORS_FEATURE`): the server then pushes `chart.indicators` /
   * `chart.indicator` for it. Absent = no indicator frames (the client computes its own).
   */
  indicators?: IndicatorSpec[];
  /** Extended hours off: the server computes the indicators (and the zones) over the regular session only. */
  rth?: boolean;
  /** `chart.levels` for this slot, sent ONLY when the welcome advertised `levels.v1`. */
  levels?: boolean;
  /** `chart.zones` for this slot (the zones layer on), sent ONLY with `zones.v1`. */
  zones?: boolean;
}

/**
 * Client → server (`indicators.v1`): change a held slot's studies without a new candle snapshot
 * (a study toggled, extended hours flipped, indicators hidden = `[]`). The answer is a
 * `chart.indicators`.
 */
export interface ChartIndicatorsSet {
  type: "chart.indicators.set";
  id?: string;
  indicators: IndicatorSpec[];
  rth?: boolean;
  /** The zones layer on or off (`zones.v1` only); absent = unchanged. */
  zones?: boolean;
}

/** One server-computed line: `values[i]` belongs to the snapshot's `t[from + i]`; null = no value. */
export interface IndicatorLineWire {
  /** As `IndicatorLine.name` ("EMA", "Basis", "Hist", …), in `computeIndicators`' order. */
  name: string;
  style: "line" | "histogram";
  from: number;
  values: (number | null)[];
}

/**
 * Server → client (`indicators.v1`, crates/server/src/indicators.rs): the values of the studies
 * it names (by `specKey`, never by client id) over every input bar `t` of the chart key (symbol,
 * timeframe, rth). Replaces the client's values of those keys. Follows the slot's
 * `chart.snapshot` at the same seq; also answers `chart.indicators.set`, comes again on every
 * resync, and replaces an increment that would have been too large.
 */
export interface ChartIndicatorsSnapshot {
  type: "chart.indicators";
  symbol: string;
  timeframe: Timeframe;
  rth: boolean;
  t: number[];
  /** `t` in exchange-time ns, index for index (newer servers; crates/chart-engine/src/wire.rs). */
  tNs?: number[];
  studies: { key: string; lines: IndicatorLineWire[] }[];
}

/**
 * Server → client (`indicators.v1`): what one engine apply moved. Drop the bars in `removed`
 * first, then upsert each point's values per key (one value per line; a point names only the
 * keys that moved there; ignore keys the slot did not ask for).
 */
export interface ChartIndicatorIncrement {
  type: "chart.indicator";
  symbol: string;
  timeframe: Timeframe;
  rth: boolean;
  points: { t: number; tNs?: number; v: Record<string, (number | null)[]> }[];
  removed?: number[];
  /** `removed` in exchange-time ns, index for index (newer servers). */
  removedNs?: number[];
}

/**
 * Server → client (`levels.v1`, crates/server/src/chart_context.rs): the chart's key levels
 * (the symbol's, repeated per timeframe) and its timeframe's opening ranges, the whole state,
 * replacing what the client held. Sent after the slot's `chart.snapshot`, on every resync, and
 * whenever its content changes. `day` is null, and everything empty, before the symbol's first
 * candle. Same shapes as levels.ts `keyLevels` / `OpeningRange` read off the candles, which stay
 * only as the fallback for an older server (`serverKeyLevels`).
 */
export interface ChartLevels {
  type: "chart.levels";
  symbol: string;
  timeframe: Timeframe;
  day: string | null;
  pmHigh: number | null;
  pmLow: number | null;
  hod: number | null;
  lod: number | null;
  open: number | null;
  /** The official previous close when the server has it, else the latest earlier regular close. */
  prevClose: number | null;
  /** `tNs` / `endNs`: the exchange-time ns twins (newer servers; crates/chart-engine/src/wire.rs). */
  openingRanges: { day: string; t: number; tNs?: number; end: number; endNs?: number; high: number; low: number; forming: boolean }[];
}

/**
 * Server → client (`zones.v1`): the chart key's automatic supply/demand zones (autoZones.ts
 * `AutoZone`, the same rule computed by the server), the whole set. Sent when the slot asks
 * (`zones: true`), on every resync, and whenever the set changes.
 */
export interface ChartZones {
  type: "chart.zones";
  symbol: string;
  timeframe: Timeframe;
  rth: boolean;
  zones: { kind: "supply" | "demand"; lo: number; hi: number; t: number; tNs?: number; tested: boolean }[];
}

/** Client → server: free a chart slot (`id` absent = the default slot). */
export interface ChartUnsubscribe {
  type: "chart.unsubscribe";
  id?: string;
}

/** The server's limit on charts per connection (crates/server/src/ws.rs MAX_CHARTS). */
export const MAX_CHARTS_PER_CONNECTION = 4;

/**
 * Client → server replay control (crates/server/src/market.rs `parse_control`).
 * Honoured in replay mode only; live mode answers `error not_replay`. There is
 * no direct reply: the resulting `replay.clock` is the answer.
 *
 * `value` by action:
 * - `play`, `pause`, `step` (exactly one event): none;
 * - `stepInterval`: a timeframe label ("15s" | "30s" | "1m" | "5m"), default "1m";
 * - `seek`: epoch ms (or an RFC 3339 string); the server rebuilds from the
 *   session start and resends `chart.snapshot`;
 * - `speed`: multiplier (1, 2, 10, 60, …), 0 = max;
 * - `day`: "YYYY-MM-DD", one of GET /api/replay/state's `days`
 *   (replayDays.ts). Rebuilds like a seek on that day at the server's
 *   REPLAY_START time of day: the paper account resets and every
 *   subscription gets a fresh snapshot. `error unknown_day` /
 *   `day_unavailable` mean nothing changed.
 */
export interface ReplayControl {
  type: "replay.control";
  action: "play" | "pause" | "step" | "stepInterval" | "seek" | "speed" | "day";
  value?: number | string;
}

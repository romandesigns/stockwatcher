import { formatChange, formatPrice, formatVolume } from "./format.ts";
import type { Slot } from "./scale.ts";
import { etParts, etToUtc, formatEtTime, sessionOf } from "./time.ts";
import { TIMEFRAME_MS, type TapeTrade, type Timeframe } from "./types.ts";

// View-models for the inspector and the tape, kept here so every client
// words them identically.

export interface InspectionRow {
  k: string;
  v: string;
}

export interface Inspection {
  kind: "candle" | "gap";
  /** "10:41:00 – 10:42:00" in ET */
  timeLabel: string;
  rows: InspectionRow[];
  state: "Forming" | "Final" | "No trades" | "Volume only";
  /** Gap explanation, e.g. "2 intervals, no trades". */
  note?: string;
  /** One sentence for an aria-live region. */
  summary: string;
  /**
   * The compact readout every inspector line shows (owner: simpler, easier
   * to read): the start time only (seconds only for sub-minute candles),
   * O/H/L/C, the change since the open with its direction, and volume.
   */
  time: string;
  ohlc: InspectionRow[];
  change: { text: string; dir: "up" | "down" | "flat" } | null;
  volume: string | null;
  forming: boolean;
}

/**
 * Exclusive end of the candle starting at `t`. `t + TIMEFRAME_MS` except for
 * 1h, which the server clips to the session so that no candle mixes two
 * (crates/chart-engine/src/timeframe.rs): the 09:00 candle ends at the 09:30
 * open and the 09:30 one at 10:00. ET offsets are whole hours, so the epoch
 * hour is the ET hour.
 */
export function intervalEnd(t: number, timeframe: Timeframe): number {
  const tf = TIMEFRAME_MS[timeframe];
  if (timeframe !== "1h") return t + tf;
  const end = Math.floor(t / tf) * tf + tf;
  if (sessionOf(t) !== "pre") return end;
  const p = etParts(t);
  const open = etToUtc(p.year, p.month, p.day, 9, 30);
  return open > t && open < end ? open : end;
}

export function describeSlot(slot: Slot, timeframe: Timeframe): Inspection {
  const tf = TIMEFRAME_MS[timeframe];
  const time = formatEtTime(slot.t, tf < 60_000);
  if (slot.kind === "gap") {
    const g = slot.gap;
    const timeLabel = `${formatEtTime(g.t, true)} – ${formatEtTime(g.end, true)}`;
    const span = `${g.intervals} interval${g.intervals === 1 ? "" : "s"}`;
    if (g.reason === "volume_only_trades") {
      const note = `${span}, volume-only prints (${formatVolume(g.v)}), no qualifying price`;
      return { kind: "gap", timeLabel, rows: [], state: "Volume only", note, summary: `${timeLabel} Eastern: ${note}; no candle.`, time, ohlc: [], change: null, volume: formatVolume(g.v), forming: false };
    }
    const note = `${span}, no trades`;
    return { kind: "gap", timeLabel, rows: [], state: "No trades", note, summary: `${timeLabel} Eastern: ${note}; no candle.`, time, ohlc: [], change: null, volume: null, forming: false };
  }
  const c = slot.candle;
  const timeLabel = `${formatEtTime(c.t, true)} – ${formatEtTime(intervalEnd(c.t, timeframe), true)}`;
  const rows = [
    { k: "O", v: formatPrice(c.o) },
    { k: "H", v: formatPrice(c.h) },
    { k: "L", v: formatPrice(c.l) },
    { k: "C", v: formatPrice(c.c) },
    { k: "V", v: formatVolume(c.v) },
    { k: "Δ", v: formatChange(c.o, c.c) },
  ];
  const state = c.final ? "Final" : "Forming";
  const summary = `${state} ${timeframe} candle ${timeLabel} Eastern: open ${rows[0]!.v}, high ${rows[1]!.v}, low ${rows[2]!.v}, close ${rows[3]!.v}, volume ${rows[4]!.v}, change ${rows[5]!.v}.`;
  const pct = c.o ? (c.c / c.o - 1) * 100 : 0;
  const dir = pct > 0.0049 ? "up" : pct < -0.0049 ? "down" : "flat";
  // A real minus sign: a hyphen reads as a dash beside the other figures.
  const change = { text: `${dir === "up" ? "+" : dir === "down" ? "−" : ""}${Math.abs(pct).toFixed(2)}%`, dir } as const;
  return { kind: "candle", timeLabel, rows, state, summary, time, ohlc: rows.slice(0, 4), change, volume: rows[4]!.v, forming: !c.final };
}

/**
 * Display names of the trade conditions the engine restricts, from Alpaca's
 * table as encoded in crates/market-events/src/conditions.rs. Display only:
 * the engine decides how a print counts, the client only words it.
 * [long, short]
 */
export const CONDITION_NAMES: Readonly<Record<string, [string, string]>> = {
  B: ["Average price / bunched", "avg/bunched"],
  C: ["Cash sale", "cash"],
  G: ["Bunched sold", "bunched sold"],
  H: ["Price variation", "price var"],
  I: ["Odd lot", "odd lot"],
  M: ["Official close", "official close"],
  N: ["Next day", "next day"],
  P: ["Prior reference price", "prior ref"],
  Q: ["Official open", "official open"],
  R: ["Seller", "seller"],
  U: ["Extended trading hours", "ext hours"],
  V: ["Contingent", "contingent"],
  W: ["Average price", "avg price"],
  Z: ["Sold out of sequence", "out of seq"],
  "4": ["Derivatively priced", "deriv priced"],
  "7": ["Qualified contingent", "QCT"],
  "9": ["Corrected close", "corr close"],
};

/** "odd_lot" / "condition_W" → [long, short] */
function reasonName(reason: string | null | undefined): [string, string] | null {
  if (!reason) return null;
  if (reason === "odd_lot") return ["Odd lot", "odd lot"];
  const m = /^condition_(.+)$/.exec(reason);
  if (m) return CONDITION_NAMES[m[1]!] ?? [`Condition ${m[1]}`, `cond ${m[1]}`];
  const words = reason.replace(/_/g, " ");
  return [words.charAt(0).toUpperCase() + words.slice(1), words];
}

/** counted: the row reads normally / dimmed (volume only) / faint (excluded) / struck (ignored duplicate). */
export type CountedTone = "counted" | "partial" | "excluded" | "ignored";

export interface CountedLabel {
  /** "Odd lot · volume only" */
  label: string;
  /** "odd lot · vol only" (phone) */
  short: string;
  tone: CountedTone;
}

/** Human wording for how the engine counted a print (the tape's "counted as" column). */
export function describeCounted(trade: Pick<TapeTrade, "counted" | "reason">): CountedLabel {
  if (trade.counted === "ohlc_and_volume") return { label: "Regular sale", short: "counted", tone: "counted" };
  if (trade.counted === "volume_only") {
    const n = reasonName(trade.reason) ?? ["Restricted", "restricted"];
    return { label: `${n[0]} · volume only`, short: `${n[1]} · vol only`, tone: "partial" };
  }
  const why = trade.counted.slice("excluded:".length);
  if (why === "duplicate") return { label: "Duplicate · ignored", short: "duplicate", tone: "ignored" };
  if (why === "late") return { label: "Late (after settle window) · excluded", short: "late · excluded", tone: "excluded" };
  if (why === "outside_session") return { label: "Outside any session · excluded", short: "no session", tone: "excluded" };
  const n = reasonName(why) ?? ["Excluded", "excluded"];
  return { label: `${n[0]} · excluded`, short: `${n[1]} · excluded`, tone: "excluded" };
}

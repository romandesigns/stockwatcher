// The Halt watcher (owner, 2026-10-19; build-ups since 2026-10-20): the client side of the server's
// build-up board (protocol.ts `pressure.*`, crates/server/src/pressure.rs), its cue lines, and the gauge's
// geometry and colours, so the web and the phone draw the same card from the same numbers.

import type { PressureCardWire, PressureHaltWire, PressureMessage } from "./protocol.ts";
import { quoteChangePct } from "./tickers/quotes.ts";

export interface PressureCard {
  symbol: string;
  price: number;
  /** Percent, against the previous close when known, else the open (the quote board's rule). */
  changePct: number;
  /** The halt risk 0–100, what the gauge fills with; frozen at the halt while `halt` is set. */
  risk: number;
  /** The side the build-up pushes: `bull` toward limit up, `bear` toward limit down. */
  side: "bull" | "bear";
  /** That side's pressure now minus `riseSecs` earlier, points. */
  rise: number;
  riseSecs: number;
  /** The modelled LULD band on that side, dollars, and the distance to it in % of the price. */
  band: number;
  toBand: number;
  /** The flow pressure 0–100 and its inputs (tooltip). */
  pressure: number;
  buyShare: number;
  pace: number;
  halt?: PressureHaltWire;
}

export type PressureBoard = readonly PressureCard[];

export const EMPTY_PRESSURE_BOARD: PressureBoard = [];

/** The empty board's line: no card is the normal state (owner, 2026-10-20). */
export const NO_BUILDUPS = "No build-ups right now";

const clamp100 = (v: number) => Math.max(0, Math.min(100, Number.isFinite(v) ? v : 0));

function fromWire(c: PressureCardWire): PressureCard {
  return {
    symbol: c.s,
    price: c.p,
    changePct: quoteChangePct(c.p, c.pc, c.o),
    risk: clamp100(c.risk),
    side: c.side === "bear" ? "bear" : "bull",
    rise: c.rise,
    riseSecs: c.riseSecs,
    band: c.band,
    toBand: Math.max(0, c.toBand),
    pressure: clamp100(c.pressure),
    buyShare: c.buyShare,
    pace: c.pace,
    ...(c.halt ? { halt: c.halt } : {}),
  };
}

/** The board a frame describes: snapshot and update alike carry the whole board. */
export function pressureBoardFrom(m: PressureMessage): PressureCard[] {
  return m.board.map(fromWire);
}

/**
 * A feed's pressure board, subscribed on demand like the quote channel: the first listener opts
 * the connection in, the last one leaving opts it out and empties the board (a board from minutes
 * ago would show halts that ended).
 */
export interface PressureChannel {
  get(): PressureBoard;
  /** `fn` runs after every frame. Returns the unsubscribe. */
  subscribe(fn: () => void): () => void;
}

export function createPressureChannel(io: { subscribe(): void; unsubscribe(): void }): PressureChannel & { apply(m: PressureMessage): void } {
  let board: PressureBoard = EMPTY_PRESSURE_BOARD;
  const listeners = new Set<() => void>();
  return {
    get: () => board,
    subscribe(fn) {
      listeners.add(fn);
      if (listeners.size === 1) io.subscribe();
      return () => {
        if (!listeners.delete(fn)) return;
        if (listeners.size > 0) return;
        io.unsubscribe();
        board = EMPTY_PRESSURE_BOARD;
      };
    },
    apply(m) {
      board = pressureBoardFrom(m);
      for (const l of listeners) l();
    },
  };
}

// --- The build-up cue -----------------------------------------------------------------------

type Cue = Pick<PressureCard, "side" | "rise" | "riseSecs" | "toBand">;

function riseText(c: Cue): string {
  return `${c.rise >= 0 ? "+" : "−"}${Math.abs(Math.round(c.rise))}`;
}

function bandName(side: "bull" | "bear"): string {
  return side === "bear" ? "LULD down" : "LULD up";
}

function distance(c: Cue): string {
  return c.toBand < 0.005 ? `at ${bandName(c.side)}` : `${c.toBand.toFixed(1)}% to ${bandName(c.side)}`;
}

/**
 * The line under the ticker (owner's "minimal, compact UI"): how fast the pressure built and how
 * far the band is, "+45 in 30s · 1.2% to LULD up" ("… · at LULD up" once there).
 */
export function buildupCue(c: Cue): string {
  return `${riseText(c)} in ${c.riseSecs}s · ${distance(c)}`;
}

/** The same for the phone's mini cell, ~56 px wide: "+45 · 1.2%↑" ("+45 · at↑" at the band). */
export function buildupCueShort(c: Cue): string {
  const arrow = c.side === "bear" ? "↓" : "↑";
  return `${riseText(c)} · ${c.toBand < 0.005 ? "at" : `${c.toBand.toFixed(1)}%`}${arrow}`;
}

/** For a screen reader or a tooltip, spelled out. */
export function buildupSentence(c: Cue & Pick<PressureCard, "band">, formatPrice: (p: number) => string): string {
  const who = c.side === "bear" ? "selling" : "buying";
  const band = `the ${c.side === "bear" ? "lower" : "upper"} LULD band at ${formatPrice(c.band)}`;
  const where = c.toBand < 0.005 ? `at ${band}` : `${c.toBand.toFixed(1)}% from ${band}`;
  return `${who} pressure ${c.rise >= 0 ? "up" : "down"} ${Math.abs(Math.round(c.rise))} points in ${c.riseSecs} seconds, ${where}`;
}

// --- Halts --------------------------------------------------------------------------------

/** A LULD trading pause lasts 5 minutes (it can be extended), so its reopening is an estimate. */
export const LULD_PAUSE_MS = 5 * 60_000;

/**
 * The card's amber line in place of the change %: "Halted ~10:47" for a LULD pause (its expected
 * reopening, approximate because a pause can be extended), "Halted 10:42" otherwise (the halt's
 * time: a news or regulatory halt has no expected end). `formatTime` renders epoch ms as ET HH:MM.
 */
export function haltLine(h: PressureHaltWire, formatTime: (ms: number) => string): string {
  return h.reason === "LUDP" ? `Halted ~${formatTime(h.start + LULD_PAUSE_MS)}` : `Halted ${formatTime(h.start)}`;
}

/** The same for a screen reader or a tooltip, spelled out. */
export function haltSentence(h: PressureHaltWire, formatTime: (ms: number) => string): string {
  return h.reason === "LUDP"
    ? `halted at ${formatTime(h.start)} by a LULD pause, reopening about ${formatTime(h.start + LULD_PAUSE_MS)}`
    : `halted at ${formatTime(h.start)}${h.reason ? ` (${h.reason})` : ""}`;
}

// --- The gauge ------------------------------------------------------------------------------

/**
 * A shadcn-style radial half ring in a 200 × 116 box (owner, 2026-10-19, design canvas "Halt
 * Watcher", option A "fill"; it replaced a 240° speedometer with a needle): a thick round-capped
 * track from 180° (left) clockwise over the top to 360° (right), filled in proportion to the 0–100
 * reading (the halt risk since 2026-10-20), with the reading inside the ring.
 */
export const GAUGE = { width: 200, height: 116, cx: 100, cy: 100, r: 74, start: 180, sweep: 180, stroke: 24 } as const;

function point(deg: number): [number, number] {
  const a = (deg * Math.PI) / 180;
  return [GAUGE.cx + GAUGE.r * Math.cos(a), GAUGE.cy + GAUGE.r * Math.sin(a)];
}

/** An SVG arc path on the gauge's radius from `a0` to `a1` degrees, clockwise (≤ 180°). */
export function gaugeArc(a0: number, a1: number): string {
  const [x0, y0] = point(a0);
  const [x1, y1] = point(a1);
  const f = (n: number) => n.toFixed(2);
  return `M ${f(x0)} ${f(y0)} A ${GAUGE.r} ${GAUGE.r} 0 0 1 ${f(x1)} ${f(y1)}`;
}

export interface GaugeGeometry {
  track: string;
  /** The filled arc; null at (near) zero, where a round cap would draw a dot. */
  value: string | null;
  /** The rounded 0–100 value shown. */
  label: number;
}

export function gaugeGeometry(reading: number): GaugeGeometry {
  const v = Math.max(0, Math.min(100, Number.isFinite(reading) ? reading : 0)) / 100;
  return {
    track: gaugeArc(GAUGE.start, GAUGE.start + GAUGE.sweep),
    value: v > 0.005 ? gaugeArc(GAUGE.start, GAUGE.start + GAUGE.sweep * v) : null,
    label: Math.round(v * 100),
  };
}

/**
 * Which colour fills the ring: the side the build-up pushes (the server's `side`), green toward
 * limit up, red toward limit down, the platform's status good/critical (the candles' colours);
 * `halted` is the neutral grey. Each app maps the tone to its own tokens.
 */
export function gaugeTone(side: "bull" | "bear", halted: boolean): "good" | "critical" | "halted" {
  return halted ? "halted" : side === "bear" ? "critical" : "good";
}

/** "limit-up risk" / "limit-down risk", the caption under the reading on a full-size card. */
export function gaugeCaption(side: "bull" | "bear", halted: boolean): string {
  return halted ? "halted" : side === "bear" ? "limit-down risk" : "limit-up risk";
}

/** "limit-up halt risk 81 of 100", for the gauge's accessible name. */
export function riskPhrase(card: Pick<PressureCard, "side" | "risk">): string {
  return `${card.side === "bear" ? "limit-down" : "limit-up"} halt risk ${Math.round(card.risk)} of 100`;
}

// The Halt watcher (owner, 2026-10-19): the client side of the server's pressure board
// (protocol.ts `pressure.*`, crates/server/src/pressure.rs), and the gauge's geometry and colours,
// so the web and the phone draw the same gauge from the same numbers.

import type { PressureCardWire, PressureHaltWire, PressureMessage } from "./protocol.ts";
import { quoteChangePct } from "./tickers/quotes.ts";

export interface PressureCard {
  symbol: string;
  price: number;
  /** Percent, against the previous close when known, else the open (the quote board's rule). */
  changePct: number;
  /** 0–100; frozen at the halt while `halt` is set. */
  pressure: number;
  side: "bull" | "bear";
  buyShare: number;
  pace: number;
  halt?: PressureHaltWire;
}

export type PressureBoard = readonly PressureCard[];

export const EMPTY_PRESSURE_BOARD: PressureBoard = [];

function fromWire(c: PressureCardWire): PressureCard {
  return {
    symbol: c.s,
    price: c.p,
    changePct: quoteChangePct(c.p, c.pc, c.o),
    pressure: Math.max(0, Math.min(100, c.pressure)),
    side: c.side === "bear" ? "bear" : "bull",
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
 * A 240° speedometer in a 200 × 136 box (the design canvas's Gauge): the arc opens downward,
 * from 150° (lower left) clockwise over the top to 30° (lower right); the value sits under the
 * hub.
 */
export const GAUGE = { width: 200, height: 136, cx: 100, cy: 100, r: 78, start: 150, sweep: 240, needle: 58, stroke: 12 } as const;

function point(deg: number, radius: number): [number, number] {
  const a = (deg * Math.PI) / 180;
  return [GAUGE.cx + radius * Math.cos(a), GAUGE.cy + radius * Math.sin(a)];
}

/** An SVG arc path on the gauge's radius from `a0` to `a1` degrees, clockwise. */
export function gaugeArc(a0: number, a1: number): string {
  const [x0, y0] = point(a0, GAUGE.r);
  const [x1, y1] = point(a1, GAUGE.r);
  const f = (n: number) => n.toFixed(2);
  return `M ${f(x0)} ${f(y0)} A ${GAUGE.r} ${GAUGE.r} 0 ${a1 - a0 > 180 ? 1 : 0} 1 ${f(x1)} ${f(y1)}`;
}

export interface GaugeGeometry {
  track: string;
  /** The value arc; null at (near) zero, where a round cap would draw a dot. */
  value: string | null;
  /** The needle's tip. */
  needle: { x: number; y: number };
  /** The rounded 0–100 value shown. */
  label: number;
}

export function gaugeGeometry(pressure: number): GaugeGeometry {
  const v = Math.max(0, Math.min(100, Number.isFinite(pressure) ? pressure : 0)) / 100;
  const end = gaugeAngle(pressure);
  const [x, y] = point(end, GAUGE.needle);
  return {
    track: gaugeArc(GAUGE.start, GAUGE.start + GAUGE.sweep),
    value: v > 0.005 ? gaugeArc(GAUGE.start, end) : null,
    needle: { x, y },
    label: Math.round(v * 100),
  };
}

/**
 * Direction is the hue, strength the lightness (owner, 2026-10-19): bullish green, bearish red,
 * each starting dark and getting lighter as the pressure builds. These are data colours (like the
 * chart's candles), not the UI palette: the accent is not spent on them.
 */
export const GAUGE_COLORS = {
  bull: { from: "#1f9d5c", to: "#a8f0c6" },
  bear: { from: "#d9433b", to: "#ffbcb6" },
} as const;

/**
 * The value arc's gradient, dark to light, laid across the WHOLE dial left to right (user space
 * `GAUGE_GRADIENT`, not the arc's own box): the arc reveals as much of it as the pressure
 * reaches, so a weak reading stays dark and only a strong one ends light. (A gradient over the
 * arc's own box would end every reading light.) Null while halted: the arc is then the neutral
 * grey of the platform's theme.
 */
export function gaugeColors(side: "bull" | "bear", halted: boolean): { from: string; to: string } | null {
  return halted ? null : GAUGE_COLORS[side];
}

/** The gradient's span in the gauge's coordinates: the dial's left and right edges. */
export const GAUGE_GRADIENT = { x1: GAUGE.cx - GAUGE.r, x2: GAUGE.cx + GAUGE.r } as const;

/** The needle's angle (degrees, SVG's clockwise from +x) for a pressure. */
export function gaugeAngle(pressure: number): number {
  const v = Math.max(0, Math.min(100, Number.isFinite(pressure) ? pressure : 0)) / 100;
  return GAUGE.start + GAUGE.sweep * v;
}

/** "buying pressure 91 of 100", for the gauge's accessible name. */
export function pressurePhrase(card: Pick<PressureCard, "side" | "pressure">): string {
  return `${card.side === "bear" ? "selling" : "buying"} pressure ${Math.round(card.pressure)} of 100`;
}

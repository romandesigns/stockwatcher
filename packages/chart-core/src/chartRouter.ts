import type { ChartMessage, Timeframe } from "./types.ts";

// Routes one connection's chart frames to the charts that asked for them (2026-10-18, the
// dashboard's multi-chart panel). The server streams up to MAX_CHARTS_PER_CONNECTION charts on
// one socket and sends each frame once, however many slots show that chart
// (crates/server/src/ws.rs), so frames carry no slot id: they are routed here by what they
// already say.
// - chart.snapshot / chart.update / chart.remove / chart.gap: by (symbol, timeframe, bin);
// - tape.trade / chart.halt / chart.luld: by symbol, whatever the slot's timeframe;
// - chart.indicators / chart.indicator (indicators.v1): by (symbol, timeframe), whatever the bin
//   (they do not depend on it); the frame's `rth` keys the values in the slot's store, and the
//   chart draws the set matching its own extended-hours setting;
// - replay.clock: to every slot.
// Two slots showing the same chart both get every frame, from the one copy the server sent.
// Shared by the web and phone feeds so both route alike.

export interface RoutedSubscription {
  symbol: string;
  timeframe: Timeframe;
  profileBin: number;
}

export interface ChartRouter {
  /** Starts routing to `emit` for slot `id` (replacing that slot's previous subscription). */
  add(id: string, sub: RoutedSubscription, emit: (m: ChartMessage) => void): void;
  remove(id: string): void;
  /** Delivers a frame to every slot it belongs to. */
  dispatch(m: ChartMessage): void;
  /** The slots held, for resubscribing after a reconnect. */
  entries(): [string, RoutedSubscription][];
  clear(): void;
}

/** Same bin? Bins are decimals on the wire (0.01), compared with a tolerance below a micro-dollar. */
const sameBin = (a: number, b: number) => Math.abs(a - b) < 5e-7;

export function createChartRouter(): ChartRouter {
  const slots = new Map<string, { sub: RoutedSubscription; emit: (m: ChartMessage) => void }>();
  const sym = (s: string) => s.trim().toUpperCase();

  return {
    add(id, sub, emit) {
      slots.set(id, { sub: { ...sub, symbol: sym(sub.symbol) }, emit });
    },
    remove(id) {
      slots.delete(id);
    },
    dispatch(m) {
      for (const { sub, emit } of [...slots.values()]) {
        if (wants(sub, m)) emit(m);
      }
    },
    entries: () => [...slots.entries()].map(([id, s]) => [id, s.sub]),
    clear: () => slots.clear(),
  };
}

function wants(sub: RoutedSubscription, m: ChartMessage): boolean {
  switch (m.type) {
    case "replay.clock":
      return true;
    case "tape.trade":
    case "chart.halt":
    case "chart.luld":
      return m.symbol === sub.symbol;
    case "chart.snapshot":
      // A snapshot without a profile (no session yet) names no bin: symbol and timeframe decide.
      return m.symbol === sub.symbol && m.timeframe === sub.timeframe && (!m.profile || sameBin(m.profile.bin, sub.profileBin));
    case "chart.update":
    case "chart.remove":
    case "chart.gap":
      return m.symbol === sub.symbol && m.timeframe === sub.timeframe && sameBin(m.profileBin, sub.profileBin);
    case "chart.indicators":
    case "chart.indicator":
      return m.symbol === sub.symbol && m.timeframe === sub.timeframe;
  }
}

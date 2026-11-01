// Exchange-ns twins on the wire (`times.ns.v1`; crates/chart-engine/src/wire.rs module docs,
// crates/server/src/time_marks.rs). Rust is the only clock and it counts exchange ns: every ms
// time a newer server sends also comes as `tNs` / `startNs` / `endNs` (`nowNs` on the clock,
// `tsNs` on the tape). The client prefers the twin and falls back to the ms field when it is
// absent (an older server). Every phone build installed before the twins reads only the ms.
//
// The twin is exact in Rust but not here: an epoch-ns number is past 2^53, so JSON.parse rounds
// it to the nearest 256 ns, and a plain floor of a bar start (a whole second, so a whole ms)
// would land a millisecond early whenever the rounding went down. `wireMs` floors with a slack
// that absorbs the rounding and the division, so a whole-ms time comes back exactly as the ms
// field the server floored it to. The price: a time within SLACK_NS of the NEXT millisecond (a
// halt's event time can fall anywhere in one) reads 1 ms late, about 1 in 2000 of those and
// never a bar time. Keys stay consistent because every frame goes through the same function.
//
// Mirrored in packages/shared/src/wireTime.ts for the board frames (chart-core stays free of
// dependencies, and shared does not depend on chart-core).

import type { ChartMessage } from "./types.ts";

const NS_PER_MS = 1_000_000;
/** Above the worst rounding of an epoch-ns double (128 ns) plus the division's (~200 ns). */
export const WIRE_NS_SLACK = 512;

/**
 * Epoch ms of a wire time: from its exchange-ns twin when the server sent one, else the ms field.
 * A twin that is not a positive finite number (absent, null, or the 0 a Rust reader fills in for
 * JSON from before the twins) counts as absent.
 */
export function wireMs(ms: number, ns: unknown): number {
  return typeof ns === "number" && Number.isFinite(ns) && ns > 0 ? Math.floor((ns + WIRE_NS_SLACK) / NS_PER_MS) : ms;
}

/** As `wireMs` for a field that may be null (a halt's `end`, a board frame's `t` before any clock). */
export function wireMsOrNull(ms: number | null, ns: unknown): number | null {
  return typeof ns === "number" && Number.isFinite(ns) && ns > 0 ? wireMs(0, ns) : ms;
}

type Timed = { t: number; tNs?: unknown };
const atT = (x: Timed) => {
  x.t = wireMs(x.t, x.tNs);
};
const span = (x: { t: number; tNs?: unknown; end: number; endNs?: unknown }) => {
  atT(x);
  x.end = wireMs(x.end, x.endNs);
};
const halt = (h: { start: number; startNs?: unknown; end: number | null; endNs?: unknown }) => {
  h.start = wireMs(h.start, h.startNs);
  h.end = wireMsOrNull(h.end, h.endNs);
};
/** `t[i]` from `tNs[i]` when the twin array is there and lines up. */
const column = (t: number[], ns: unknown) => {
  if (!Array.isArray(ns) || ns.length !== t.length) return;
  for (let i = 0; i < t.length; i++) t[i] = wireMs(t[i]!, ns[i]);
};

/**
 * Rewrites a parsed chart frame's ms times from their ns twins, in place (parseChartMessage calls
 * it, so the store, the router and the charts see one time for every frame whatever the server's
 * age). Without twins every value stays as sent.
 */
export function preferNsTimes(m: ChartMessage): ChartMessage {
  switch (m.type) {
    case "chart.snapshot":
      if (m.session) {
        m.session.start = wireMs(m.session.start, m.session.startNs);
        m.session.end = wireMs(m.session.end, m.session.endNs);
      }
      for (const c of m.candles) atT(c);
      for (const g of m.gaps) span(g);
      for (const h of m.halts ?? []) halt(h);
      for (const p of m.luld ?? []) atT(p);
      if (m.clock) m.clock.now = wireMs(m.clock.now, m.clock.nowNs);
      break;
    case "chart.update":
      atT(m.candle);
      break;
    case "chart.remove":
      atT(m);
      break;
    case "chart.gap":
      span(m.gap);
      break;
    case "chart.halt":
      halt(m.halt);
      break;
    case "chart.luld":
      atT(m.point);
      break;
    case "replay.clock":
      m.clock.now = wireMs(m.clock.now, m.clock.nowNs);
      break;
    case "tape.trade":
      m.t = wireMs(m.t, m.tsNs);
      break;
    case "chart.indicators":
      column(m.t, m.tNs);
      break;
    case "chart.indicator":
      for (const p of m.points) atT(p);
      if (m.removed) column(m.removed, m.removedNs);
      break;
    case "chart.levels":
      for (const r of m.openingRanges) span(r);
      break;
    case "chart.zones":
      for (const z of m.zones) atT(z);
      break;
  }
  return m;
}

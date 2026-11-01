// Exchange-ns twins on the wire (`times.ns.v1`; crates/server/src/time_marks.rs): every board
// frame a newer server stamps with the market clock (`rocket.*`, `movers.*`, `favorites.*`,
// `quotes.*`, `quotes.search.*`, `pressure.*`) carries `tNs`, the same instant in the server's own
// exchange-time ns, beside its ms `t`. parseServerMessage rewrites `t` from it, so every reducer
// keeps reading `t`; an older server's frame (no twin) keeps its ms. Phone builds installed before
// the twins read only `t`.
//
// The rounding problem and its slack are explained in packages/chart-core/src/wireTime.ts, which
// does the same for the chart frames; this copy exists because shared does not depend on
// chart-core. Keep the two in step.

/** Advertised in `welcome.features` by servers that send the twins. Their presence is the gate (they are additive), so nothing checks this; it names the feature for logs and tests. */
export const TIMES_NS_FEATURE = "times.ns.v1";

const NS_PER_MS = 1_000_000;
const WIRE_NS_SLACK = 512;

/** Epoch ms of a wire time: from its exchange-ns twin when it is a positive finite number, else the ms field. */
export function wireMs(ms: number, ns: unknown): number {
  return typeof ns === "number" && Number.isFinite(ns) && ns > 0 ? Math.floor((ns + WIRE_NS_SLACK) / NS_PER_MS) : ms;
}

/** As `wireMs` for a nullable field (a board frame's `t` before any clock). */
export function wireMsOrNull(ms: number | null, ns: unknown): number | null {
  return typeof ns === "number" && Number.isFinite(ns) && ns > 0 ? wireMs(0, ns) : ms;
}

/** A parsed frame's top-level `t` from its `tNs` twin, in place (no-op without one). */
export function preferNsClock<T extends object>(frame: T): T {
  const f = frame as { t?: unknown; tNs?: unknown };
  if ("tNs" in f && (typeof f.t === "number" || f.t === null)) f.t = wireMsOrNull(f.t, f.tNs);
  return frame;
}

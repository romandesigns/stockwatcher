import type { ProfileRow, ValueArea } from "./types.ts";

// Session volume profile helpers. Rows come from the server (actual traded
// volume per price bin). POC and value area replicate
// crates/chart-engine/src/profile.rs exactly, because the client has to
// recompute them after incremental updates (only snapshots carry them) and
// every client must agree with the server.

const keyOf = (p: number, bin: number) => Math.round(p / bin);

function decimalsOf(bin: number): number {
  return Math.max(0, Math.ceil(-Math.log10(bin) - 1e-9)) + 1;
}

/** Bin start for a key, rounded so 318 × 0.01 prints as 3.18, not 3.1799999. */
export function binPrice(key: number, bin: number): number {
  return Number((key * bin).toFixed(decimalsOf(bin)));
}

/** Non-empty rows, ascending, merged by bin (float noise in `p` maps to one bin). */
export function normalizeRows(rows: readonly ProfileRow[], bin: number): ProfileRow[] {
  const byKey = new Map<number, number>();
  for (const r of rows) {
    if (r.v <= 0) continue;
    const k = keyOf(r.p, bin);
    byKey.set(k, (byKey.get(k) ?? 0) + r.v);
  }
  return [...byKey.entries()].sort((a, b) => a[0] - b[0]).map(([k, v]) => ({ p: binPrice(k, bin), v }));
}

/** Point of control: the bin with the most volume; the LOWEST price on a tie (engine rule). */
export function pointOfControl(rows: readonly ProfileRow[]): number | null {
  let best: ProfileRow | null = null;
  for (const r of rows) {
    if (r.v <= 0) continue;
    if (!best || r.v > best.v || (r.v === best.v && r.p < best.p)) best = r;
  }
  return best ? best.p : null;
}

/**
 * Value area holding at least `percent` % of the volume (target rounded UP):
 * start at the POC and repeatedly add the adjacent NON-EMPTY bin with more
 * volume, the one above on a tie. Empty bins inside the range are skipped,
 * not counted. Returns bin starts, as the engine does.
 */
export function valueArea(rows: readonly ProfileRow[], bin: number, percent = 70): ValueArea | null {
  const r = normalizeRows(rows, bin);
  const pocPrice = pointOfControl(r);
  if (pocPrice === null) return null;
  const poc = r.findIndex((x) => x.p === pocPrice);
  const total = r.reduce((a, x) => a + x.v, 0);
  const target = Math.ceil((total * percent) / 100);
  let lo = poc;
  let hi = poc;
  let acc = r[poc]!.v;
  while (acc < target) {
    const up = r[hi + 1]?.v;
    const down = lo > 0 ? r[lo - 1]!.v : undefined;
    if (up === undefined && down === undefined) break;
    if (up !== undefined && (down === undefined || up >= down)) acc += r[++hi]!.v;
    else acc += r[--lo]!.v;
  }
  return { low: r[lo]!.p, high: r[hi]!.p, v: acc };
}

export function profileTotal(rows: readonly ProfileRow[]): number {
  return rows.reduce((a, r) => a + r.v, 0);
}

/**
 * Regroups server rows into a COARSER bin for display, only when `toBin` is
 * an exact multiple of `fromBin` (anything else would split a row's volume
 * across bins, i.e. invent a distribution). Returns null otherwise.
 *
 * Preferred path: ask the server for the new width (chart.subscribe
 * `profileBin`); the engine stores volume per exact price, so its re-bin is
 * exact for ANY width. This is only a display fallback.
 */
export function rebinProfile(rows: readonly ProfileRow[], fromBin: number, toBin: number): ProfileRow[] | null {
  const ratio = toBin / fromBin;
  if (!(ratio >= 1) || Math.abs(ratio - Math.round(ratio)) > 1e-6) return null;
  const n = Math.round(ratio);
  const out = new Map<number, number>();
  for (const r of rows) {
    if (r.v <= 0) continue;
    const k = Math.floor(keyOf(r.p, fromBin) / n);
    out.set(k, (out.get(k) ?? 0) + r.v);
  }
  return [...out.entries()].sort((a, b) => a[0] - b[0]).map(([k, v]) => ({ p: binPrice(k, toBin), v }));
}

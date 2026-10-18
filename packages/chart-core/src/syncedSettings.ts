import { DRAWING_KINDS, type Anchor, type Drawing, type DrawingKind } from "./drawings.ts";
import { INDICATOR_PLACEMENT, type IndicatorKind, type IndicatorSpec } from "./indicators.ts";

// What the clients sync through the server (owner request 2026-10-18: "any setting change takes
// place in all clients"; docs/architecture/settings-sync.md has the full synced vs device-local
// table). The namespaces and the validation of everything read back live here, next to the
// models, so the web app and the phone agree byte for byte on what a namespace holds.
//
// Synced (the same person's preferences, whatever the screen):
// - `indicators`      IndicatorSpec[] (parseIndicators)
// - `layers`          ChartLayers (levels.ts parseLayers)
// - `chartSettings`   ChartSettings (chartSettings.ts parseChartSettings)
// - `drawingSettings` DrawingSettings (drawingSettings.ts parseDrawingSettings)
// - `drawings:<SYMBOL>` one symbol's drawings, as a MAP of drawing id → {z, d} (below), written
//   with entry-level patches so two devices editing different drawings never overwrite each other.
// Device-local (each screen's own view): ticker, timeframe, zoom/pan, the toolbox's place and
// orientation, pane heights, tools shown, open dialogs and tabs, ticket size, search recents.
//
// Remote JSON is never trusted: another device may run an older or newer build, so every value
// goes through these parsers (unknown fields dropped, junk falling back to defaults).

export const SYNC_NS = {
  indicators: "indicators",
  layers: "layers",
  chartSettings: "chartSettings",
  drawingSettings: "drawingSettings",
} as const;

const DRAWINGS_PREFIX = "drawings:";

/** The namespace of one symbol's drawings. Symbols are upper case on the wire everywhere. */
export const drawingsNs = (symbol: string) => `${DRAWINGS_PREFIX}${symbol.trim().toUpperCase()}`;

/** The symbol a `drawings:<SYMBOL>` namespace belongs to, else null. */
export const drawingsNsSymbol = (ns: string): string | null => (ns.startsWith(DRAWINGS_PREFIX) ? ns.slice(DRAWINGS_PREFIX.length) || null : null);

const finite = (v: unknown): v is number => typeof v === "number" && Number.isFinite(v);
const isObj = (v: unknown): v is Record<string, unknown> => typeof v === "object" && v !== null && !Array.isArray(v);

const PERIOD_MAX = 500;
const KINDS = new Set(Object.keys(INDICATOR_PLACEMENT));
const intIn = (v: unknown, lo: number, hi: number): number | undefined => (finite(v) && Number.isInteger(v) && v >= lo && v <= hi ? v : undefined);

/**
 * A stored indicator list → valid specs. Unknown kinds and malformed entries are dropped, a
 * repeated id keeps its first entry, out-of-range parameters are left out (the indicator then
 * uses its default), and the list is capped well above what the menus allow.
 */
export function parseIndicators(raw: unknown): IndicatorSpec[] {
  if (!Array.isArray(raw)) return [];
  const out: IndicatorSpec[] = [];
  const ids = new Set<string>();
  for (const r of raw) {
    if (!isObj(r) || typeof r.id !== "string" || !r.id || r.id.length > 64 || ids.has(r.id)) continue;
    if (typeof r.kind !== "string" || !KINDS.has(r.kind)) continue;
    const spec: IndicatorSpec = { id: r.id, kind: r.kind as IndicatorKind };
    const period = intIn(r.period, 1, PERIOD_MAX);
    if (period !== undefined) spec.period = period;
    if (finite(r.mult) && r.mult > 0 && r.mult <= 10) spec.mult = r.mult;
    for (const k of ["fast", "slow", "signal"] as const) {
      const v = intIn(r[k], 1, PERIOD_MAX);
      if (v !== undefined) spec[k] = v;
    }
    ids.add(r.id);
    out.push(spec);
    if (out.length >= 16) break;
  }
  return out;
}

const KIND_SET = new Set<string>(DRAWING_KINDS);
const anchor = (v: unknown): Anchor | null => (isObj(v) && finite(v.t) && finite(v.p) ? { t: v.t, p: v.p } : null);

/** One stored drawing → a valid drawing, or null. Only known fields survive. */
export function parseDrawing(raw: unknown): Drawing | null {
  if (!isObj(raw) || typeof raw.id !== "string" || !raw.id || raw.id.length > 128) return null;
  if (typeof raw.kind !== "string" || !KIND_SET.has(raw.kind)) return null;
  const a = anchor(raw.a);
  const b = anchor(raw.b);
  if (!a || !b) return null;
  const d: Drawing = { id: raw.id, kind: raw.kind as DrawingKind, a, b };
  const c = anchor(raw.c);
  if (c) d.c = c;
  if (finite(raw.risk) && raw.risk > 0) d.risk = raw.risk;
  if (raw.qty === null) d.qty = null;
  else if (finite(raw.qty) && raw.qty > 0) d.qty = raw.qty;
  if (typeof raw.color === "string" && /^#[0-9a-f]{6}$/i.test(raw.color)) d.color = raw.color;
  if (finite(raw.angle)) d.angle = raw.angle;
  return d;
}

/**
 * One entry of a `drawings:<SYMBOL>` map. `z` is the drawing's place in the list (paint and
 * hit-test order): the server keeps object keys sorted, not in insertion order, so the order
 * travels as data.
 */
export interface DrawingEntry {
  z: number;
  d: Drawing;
}

/** A symbol's stored drawing map → its drawings in order. Malformed entries are dropped. */
export function drawingsFromDoc(raw: unknown): Drawing[] {
  if (!isObj(raw)) return [];
  const rows: { z: number; d: Drawing }[] = [];
  for (const [id, e] of Object.entries(raw)) {
    if (!isObj(e)) continue;
    const d = parseDrawing(e.d);
    // The key is the identity: an entry whose drawing claims another id is not trusted.
    if (!d || d.id !== id) continue;
    rows.push({ z: finite(e.z) ? e.z : 0, d });
  }
  rows.sort((x, y) => x.z - y.z || (x.d.id < y.d.id ? -1 : x.d.id > y.d.id ? 1 : 0));
  return rows.map((r) => r.d);
}

/** A whole drawing list as a map (the one-time migration of a symbol's local drawings). */
export function drawingsToDoc(list: readonly Drawing[]): Record<string, DrawingEntry> {
  const out: Record<string, DrawingEntry> = {};
  list.forEach((d, z) => (out[d.id] = { z, d }));
  return out;
}

/** Same drawing? Compared after parsing both, so key order (the server sorts keys) and unknown fields do not count as a change. */
const same = (x: unknown, y: Drawing) => JSON.stringify(parseDrawing(x)) === JSON.stringify(parseDrawing(y));

/**
 * The patch that turns the stored map `doc` into the list `next`: only the drawings that were
 * added or changed are set, only the missing ones removed, so an edit never rewrites (and can
 * never undo) another device's edit of a different drawing. A drawing keeps its `z`; a new one
 * goes on top. Null when nothing changed.
 */
export function diffDrawings(doc: unknown, next: readonly Drawing[]): { set: Record<string, DrawingEntry>; remove: string[] } | null {
  const prev = new Map<string, { z: number; d: unknown }>();
  let top = -1;
  if (isObj(doc)) {
    for (const [id, e] of Object.entries(doc)) {
      if (!isObj(e)) continue;
      const z = finite(e.z) ? e.z : 0;
      prev.set(id, { z, d: e.d });
      top = Math.max(top, z);
    }
  }
  const set: Record<string, DrawingEntry> = {};
  const keep = new Set<string>();
  for (const d of next) {
    keep.add(d.id);
    const old = prev.get(d.id);
    if (old && same(old.d, d)) continue;
    set[d.id] = { z: old ? old.z : ++top, d };
  }
  const remove = [...prev.keys()].filter((id) => !keep.has(id));
  return Object.keys(set).length || remove.length ? { set, remove } : null;
}

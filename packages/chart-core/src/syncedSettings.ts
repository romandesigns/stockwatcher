import { parseChartSettings } from "./chartSettings.ts";
import { parseDrawingSettings } from "./drawingSettings.ts";
import { DRAWING_KINDS, type Anchor, type Drawing, type DrawingKind } from "./drawings.ts";
import { INDICATOR_PLACEMENT, isColorRole, isHexColor, isMacdStyle, isPresetSpec, presetOf, sameIndicatorSettings, type ColorRole, type IndicatorKind, type IndicatorSpec } from "./indicators.ts";
import { parseLayers } from "./levels.ts";

// What the clients sync through the server (owner request 2026-10-18: "any setting change takes
// place in all clients"; docs/architecture/settings-sync.md has the full synced vs device-local
// table). The namespaces and the validation of everything read back live here, next to the
// models, so the web app and the phone agree byte for byte on what a namespace holds.
//
// Synced (the same person's preferences, whatever the screen):
// - `indicators`      IndicatorSpec[] (parseIndicators)
// - `indicatorMemory` the settings of indicators turned OFF, as a MAP of id → IndicatorSpec
//                     (parseIndicatorMemory, rememberIndicators), written with entry-level
//                     patches. Its own namespace, not a field of `indicators`, so a build from
//                     before it (which rewrites `indicators` whole and drops unknown fields)
//                     never wipes it, and its absence just means nothing is remembered.
// - `tickerIndicators` the Insights ticker page chart's own IndicatorSpec[] (until 2026-11-07 a
//                     per-device preference; seeded once from it), apart from the main list.
// - `indicatorColors` every indicator line's colour, GLOBAL across charts and devices, as a flat
//                     MAP "<identity>/<role>" → "#rrggbb" (indicatorColors.ts), entry patches;
//                     seeded once from the per-spec colours, which are ignored from then on.
// - `layers`          ChartLayers (levels.ts parseLayers)
// - `chartSettings`   ChartSettings (chartSettings.ts parseChartSettings)
// - `drawingSettings` DrawingSettings (drawingSettings.ts parseDrawingSettings)
// - `drawings:<SYMBOL>` one symbol's drawings, as a MAP of drawing id → {z, d} (below), written
//   with entry-level patches so two devices editing different drawings never overwrite each other.
// The app-level namespaces (ticker/timeframe/bin `view`, `tradeQty`, `searchRecents`) live in
// @stockwatcher/app-state (namespaces.ts), next to the store that reads them.
// Device-local (each screen's own layout): zoom/pan, the toolbox's place and orientation, pane
// heights, tools shown, open dialogs and tabs.
//
// Remote JSON is never trusted: another device may run an older or newer build, so every value
// goes through these parsers (unknown fields dropped, junk falling back to defaults).

export const SYNC_NS = {
  indicators: "indicators",
  layers: "layers",
  chartSettings: "chartSettings",
  drawingSettings: "drawingSettings",
  indicatorMemory: "indicatorMemory",
  tickerIndicators: "tickerIndicators",
  indicatorColors: "indicatorColors",
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
    if (spec.kind === "macd") {
      // A style this build does not know (a newer device's) is dropped: the MACD draws as classic.
      if (isMacdStyle(r.style) && r.style !== "classic") spec.style = r.style;
      if (typeof r.markers === "boolean") spec.markers = r.markers;
    }
    if (isObj(r.colors)) {
      const colors: Partial<Record<ColorRole, string>> = {};
      for (const [role, c] of Object.entries(r.colors)) if (isColorRole(role) && isHexColor(c)) colors[role] = c.toLowerCase();
      if (Object.keys(colors).length) spec.colors = colors;
    }
    ids.add(r.id);
    out.push(spec);
    if (out.length >= 16) break;
  }
  return out;
}

/** How many turned-off indicators are remembered (well above the catalogue). */
const MEMORY_MAX = 32;

/**
 * The stored `indicatorMemory` map → id → valid spec. Each entry goes through parseIndicators, and
 * one whose spec claims another id than its key is dropped (the key is the identity, as with
 * drawings). Anything else (absent, a list, junk) is an empty memory.
 */
export function parseIndicatorMemory(raw: unknown): Record<string, IndicatorSpec> {
  const out: Record<string, IndicatorSpec> = {};
  if (!isObj(raw)) return out;
  let n = 0;
  for (const [id, e] of Object.entries(raw)) {
    if (!isObj(e) || (e.id !== undefined && e.id !== id)) continue;
    const [spec] = parseIndicators([{ ...e, id }]);
    if (!spec) continue;
    out[id] = spec;
    if (++n >= MEMORY_MAX) break;
  }
  return out;
}

/**
 * What an indicator list change does to the remembered settings (owner, 2026-11-07: turning an
 * indicator off and on lost its style, colours, periods and markers; "the state should persist").
 * `prev` is the stored list, `next` the one a menu produced, `memory` the stored map:
 * - an indicator turned OFF is remembered with every setting it had (`set`), unless it was still
 *   its preset, which needs no memory (`remove`);
 * - an indicator turned ON as its bare preset (the menus add the preset) comes back with its
 *   remembered settings (`specs`); one added with settings of its own keeps them;
 * - an indicator that is ON and equal to its preset ("Reset to default", resetIndicator) clears
 *   what was remembered for it, so a later off and on does not bring the old settings back.
 * The entry of an indicator restored stays until it is turned off again (and overwritten), so two
 * devices toggling at once still restore the same settings. `set`/`remove` are a patch for the
 * memory namespace; both empty when nothing changes there.
 */
export function rememberIndicators(
  prev: readonly IndicatorSpec[],
  next: readonly IndicatorSpec[],
  memory: Readonly<Record<string, IndicatorSpec>>,
): { specs: IndicatorSpec[]; set: Record<string, IndicatorSpec>; remove: string[] } {
  const was = new Map(prev.map((s) => [s.id, s]));
  const now = new Set(next.map((s) => s.id));
  const set: Record<string, IndicatorSpec> = {};
  const remove = new Set<string>();
  const specs = next.map((s) => {
    if (was.has(s.id)) {
      if (memory[s.id] && isPresetSpec(s)) remove.add(s.id);
      return s;
    }
    const kept = memory[s.id];
    const preset = presetOf(s.id);
    if (kept && kept.kind === s.kind && preset && sameIndicatorSettings(s, preset)) return { ...kept, id: s.id };
    return s;
  });
  for (const [id, s] of was) {
    if (now.has(id)) continue;
    if (isPresetSpec(s)) {
      if (memory[id]) remove.add(id);
    } else if (!memory[id] || !sameIndicatorSettings(memory[id]!, s)) set[id] = s;
  }
  return { specs, set, remove: [...remove] };
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

/** A client's settings from before sync, as read from its old storage keys (absent = never saved). */
export interface LegacySettings {
  indicators?: unknown;
  layers?: unknown;
  chartSettings?: unknown;
  drawingSettings?: unknown;
  /** `{ [symbol]: Drawing[] }`, the shape both apps kept. */
  drawings?: unknown;
}

/**
 * The one-time migration of a client's pre-sync settings, shared by the web app and the phone:
 * each value that was ever saved is validated and handed to `seed` (settingsSync's `seed`,
 * which queues an `ifAbsent` write). So the first client to connect makes its setup the shared
 * one; on a later client the server's value wins and replaces the local one. Nothing saved,
 * nothing seeded: a fresh install simply adopts the server's settings.
 */
export function migrateLegacySettings(legacy: LegacySettings, seed: (ns: string, value: unknown) => void): void {
  const one = <T,>(ns: string, raw: unknown, parse: (raw: unknown) => T) => {
    if (raw !== undefined && raw !== null) seed(ns, parse(raw));
  };
  one(SYNC_NS.indicators, legacy.indicators, parseIndicators);
  one(SYNC_NS.layers, legacy.layers, parseLayers);
  one(SYNC_NS.chartSettings, legacy.chartSettings, parseChartSettings);
  one(SYNC_NS.drawingSettings, legacy.drawingSettings, parseDrawingSettings);
  if (isObj(legacy.drawings)) {
    for (const [symbol, list] of Object.entries(legacy.drawings)) {
      if (!symbol.trim() || !Array.isArray(list)) continue;
      const valid = list.map(parseDrawing).filter((d): d is Drawing => d !== null);
      if (valid.length) seed(drawingsNs(symbol), drawingsToDoc(valid));
    }
  }
}

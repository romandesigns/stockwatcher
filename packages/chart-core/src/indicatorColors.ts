import { indicatorColorRoles, isColorRole, isHexColor, type ColorRole, type IndicatorSpec } from "./indicators.ts";

// Global indicator line colours (owner, 2026-11-07: "The colors change should propagate globally
// and not independently per chart"). A line's colour is the person's preference for THAT line,
// wherever it is drawn: the main chart, every dashboard cell, the Insights ticker page, on the
// web, the desktop and the phone. Styles, periods and markers stay per chart (IndicatorSpec).
//
// Stored in the synced namespace `indicatorColors` (SYNC_NS.indicatorColors) as a FLAT map
//   "<identity>/<role>" → "#rrggbb"      e.g. "ema:9/line", "bb/fill", "macd/histUp"
// written with entry patches, one entry per role: two devices changing different roles (or
// different indicators) at once both land, and only two edits of the same role resolve
// last-writer-wins. An absent entry is the theme default.
//
// Identity (colorIdentity): the indicator TYPE, plus the period where the period makes a different
// line. EMA 9 and EMA 20 are two lines a trader tells apart by colour, so they key as "ema:9" and
// "ema:20" (SMA likewise). VWAP has no period; Bollinger, RSI and MACD are one study each in the
// catalogue, whose periods are a tuning of the same study, so they key by type alone: a MACD
// recoloured once keeps that colour on every chart whatever its periods or style (the roles a
// style does not draw are simply unused there).
//
// Migration (migrateSpecColors): until 2026-11-07 a colour lived on each spec (`colors`). The
// first build with global colours seeds the namespace ONCE (an `ifAbsent` seed, so the first
// device to connect wins and the others adopt it) from every per-spec colour it knows. With no
// timestamps per spec, "latest" is approximated by source order: the turned-off memory first,
// then the dashboard's own lists, the ticker page, and the main chart last, the one most likely
// edited most recently, so its colour wins a conflict. From then on per-spec colours are ignored
// (applyIndicatorColors replaces them) and dropped the next time a list is written
// (stripIndicatorColors). The other namespaces keep their shapes, so an older build still loads
// every setting; it simply shows its own per-spec colours.

/** The `indicatorColors` map: "<identity>/<role>" → "#rrggbb". */
export type IndicatorColors = Record<string, string>;

const DEFAULT_PERIOD: Partial<Record<IndicatorSpec["kind"], number>> = { ema: 9, sma: 20 };

/** The colour identity of a spec's lines (see above): "ema:9", "sma:50", "vwap", "bb", "rsi", "macd". */
export function colorIdentity(spec: Pick<IndicatorSpec, "kind" | "period">): string {
  const d = DEFAULT_PERIOD[spec.kind];
  return d === undefined ? spec.kind : `${spec.kind}:${spec.period ?? d}`;
}

/** The map key of one role of a spec. */
export const colorKey = (spec: Pick<IndicatorSpec, "kind" | "period">, role: ColorRole): string => `${colorIdentity(spec)}/${role}`;

const KEY = /^(sma|ema):([1-9][0-9]{0,2})\/([A-Za-z]+)$|^(vwap|bb|rsi|macd)\/([A-Za-z]+)$/;
const MAX_ENTRIES = 256;

/**
 * The stored map → valid entries (a known identity and role, a hex colour, lower-cased); null when
 * the namespace was never written (the migration has not run: per-spec colours still apply).
 */
export function parseIndicatorColors(raw: unknown): IndicatorColors | null {
  if (raw === undefined || raw === null) return null;
  const out: IndicatorColors = {};
  if (typeof raw !== "object" || Array.isArray(raw)) return out;
  let n = 0;
  for (const [k, v] of Object.entries(raw as Record<string, unknown>)) {
    const m = KEY.exec(k);
    const role = m?.[3] ?? m?.[5];
    if (!m || !role || !isColorRole(role) || !isHexColor(v)) continue;
    out[k] = v.toLowerCase();
    if (++n >= MAX_ENTRIES) break;
  }
  return out;
}

/** Every role colour stored for a spec's identity (all roles, also those its current style does not draw). */
export function colorsFor(spec: IndicatorSpec, colors: Readonly<IndicatorColors>): Partial<Record<ColorRole, string>> {
  const prefix = `${colorIdentity(spec)}/`;
  const out: Partial<Record<ColorRole, string>> = {};
  for (const [k, v] of Object.entries(colors)) {
    if (!k.startsWith(prefix)) continue;
    const role = k.slice(prefix.length);
    if (isColorRole(role)) out[role] = v;
  }
  return out;
}

/** A spec without its own colours. */
function bare(spec: IndicatorSpec): IndicatorSpec {
  if (!spec.colors) return spec;
  const { colors: _, ...rest } = spec;
  return rest;
}

/**
 * The specs as every chart draws them: each spec's own `colors` replaced by the global ones for its
 * identity. `colors` null (namespace never written, before the migration) leaves the specs as they
 * are, so nothing changes colour while the seed is in flight. The same array back when nothing
 * changes (stable props for memoised charts).
 */
export function applyIndicatorColors(specs: readonly IndicatorSpec[], colors: Readonly<IndicatorColors> | null): IndicatorSpec[] {
  if (!colors) return specs as IndicatorSpec[];
  let changed = false;
  const out = specs.map((s) => {
    const c = colorsFor(s, colors);
    const next = Object.keys(c).length ? { ...bare(s), colors: c } : bare(s);
    if (sameColors(s.colors, next.colors)) return s;
    changed = true;
    return next;
  });
  return changed ? out : (specs as IndicatorSpec[]);
}

function sameColors(a: IndicatorSpec["colors"], b: IndicatorSpec["colors"]): boolean {
  const ea = Object.entries(a ?? {}).filter(([, v]) => !!v);
  const eb = Object.entries(b ?? {}).filter(([, v]) => !!v);
  return ea.length === eb.length && ea.every(([k, v]) => (b as Record<string, string | undefined>)?.[k] === v);
}

/** The specs without per-spec colours, as every list is now written (the colours live globally). */
export function stripIndicatorColors(specs: readonly IndicatorSpec[]): IndicatorSpec[] {
  return specs.some((s) => s.colors) ? specs.map(bare) : (specs as IndicatorSpec[]);
}

/** The patch for one role's colour (null or the default = back to the theme default). */
export function indicatorColorPatch(spec: IndicatorSpec, role: ColorRole, color: string | null): { set: IndicatorColors; remove: string[] } {
  const key = colorKey(spec, role);
  return color && isHexColor(color) ? { set: { [key]: color.toLowerCase() }, remove: [] } : { set: {}, remove: [key] };
}

/**
 * "Reset to default" for colours: removes every stored role of the spec's identity (the roles its
 * other styles use too), so the indicator draws in its theme colours on every chart.
 */
export function resetIndicatorColorsPatch(spec: IndicatorSpec, colors: Readonly<IndicatorColors> | null): string[] {
  const prefix = `${colorIdentity(spec)}/`;
  const stored = Object.keys(colors ?? {}).filter((k) => k.startsWith(prefix));
  // Also every role the spec offers, in case an entry is still in flight from this device.
  const offered = indicatorColorRoles(spec).map((r) => colorKey(spec, r.role));
  return [...new Set([...stored, ...offered])];
}

/**
 * The one-time migration: every per-spec colour in `sources` as a global map, later sources (and
 * later specs within one) overriding earlier ones (see the header for the order). Empty when no
 * spec had a colour.
 */
export function migrateSpecColors(sources: readonly (readonly IndicatorSpec[])[]): IndicatorColors {
  const out: IndicatorColors = {};
  for (const list of sources) {
    for (const s of list) {
      for (const [role, c] of Object.entries(s.colors ?? {})) {
        if (isColorRole(role) && isHexColor(c)) out[colorKey(s, role)] = c.toLowerCase();
      }
    }
  }
  return out;
}

/**
 * What a chart's indicator settings call to change colours globally (the app store's
 * `indicatorColorActions`). A chart without it writes colours onto its own specs, as before.
 */
export interface IndicatorColorActions {
  /** One role's colour (null = the theme default), on every chart and device. */
  set(spec: IndicatorSpec, role: ColorRole, color: string | null): void;
  /** Every stored colour of the spec's identity back to the theme default. */
  reset(spec: IndicatorSpec): void;
}

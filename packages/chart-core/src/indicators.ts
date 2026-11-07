import type { ChartTheme } from "./displayList.ts";
import type { IconPath } from "./icons.ts";
import type { Slot } from "./scale.ts";

// Technical indicators: the settings (specs, presets, caps, labels, glyphs) and the presentation
// of the values. The values themselves are computed on the server (`indicators.v1`,
// crates/chart-engine/src/indicators.rs) and joined onto the chart's slots here
// (serverIndicatorResults), so every value lines up with the candle it belongs to. A gap slot
// (no candle) holds NaN and breaks the drawn line rather than bridging it: the chart never invents
// a price where nothing traded, and the indicators follow the same rule.
//
// The client's own math (sma, ema, vwap, bollinger, rsi, macd, computeIndicators) left the
// runtime on 2026-10-31, once the server, the web and the phone OTA all spoke indicators.v1: it
// lives on in indicatorMath.ts only for the parity fixtures the Rust port is held to and for
// tests. A server without the feature gets no lines and a "needs a server update" note.
//
// Scope (owner decision 2026-10-17): drawings, indicators and simulated
// chart trading move into this phase. The set here is the day-trading core,
// kept small on purpose; every one is a pure function of the candles.
//
// Colour: overlay indicators take categorical slots in a FIXED order
// (theme.series1..3), never cycled. Three slots validate all-pairs on the dark
// surface (dataviz validator: worst CVD ΔE 13.2, normal 19.3), so at most
// MAX_OVERLAYS overlays can be on at once; an oscillator lives in its own
// pane, where its lines use the same slots from the start.

export type IndicatorKind = "sma" | "ema" | "vwap" | "bb" | "rsi" | "macd";

export interface IndicatorSpec {
  /** Stable id, unique per chart (e.g. "ema-9"). */
  id: string;
  kind: IndicatorKind;
  /** sma/ema/bb/rsi: lookback in candles. */
  period?: number;
  /** bb: band width in standard deviations. */
  mult?: number;
  /** macd: fast, slow and signal periods. */
  fast?: number;
  slow?: number;
  signal?: number;
  /** macd: how it is computed and drawn (MACD_STYLES); absent = classic, today's MACD. */
  style?: MacdStyle;
  /** macd: the crossover markers (macdMarkersOn); absent = on for every style but classic. */
  markers?: boolean;
  /**
   * The person's line colours by role (indicatorColorRoles: "line", "upper", "macd", …), `#rrggbb`
   * each; a role left out keeps its theme colour. Display only: never part of specKey, so a
   * colour change asks the server for nothing.
   */
  colors?: Partial<Record<ColorRole, string>>;
}

/**
 * The MACD styles (owner-approved 2026-11-07). Computed on the server (`indicators.macd.v2`,
 * crates/chart-engine/src/indicators.rs `MacdStyle`); "four-color" is classic's values in other
 * shades, so it shares classic's server key. Against a server without the feature the computed
 * styles draw as classic (classicMacd).
 */
export type MacdStyle = "classic" | "four-color" | "ppo" | "macd-v" | "zero-lag" | "signal-area";

export const MACD_STYLES: readonly { id: MacdStyle; name: string; hint: string }[] = [
  { id: "classic", name: "Classic", hint: "MACD, signal and histogram, as today" },
  { id: "four-color", name: "4-color histogram", hint: "Shades show momentum building or fading" },
  { id: "ppo", name: "PPO", hint: "MACD as % of price, same scale on every stock" },
  { id: "macd-v", name: "MACD-V", hint: "Volatility-adjusted, stretched beyond ±150" },
  { id: "zero-lag", name: "Zero-lag", hint: "Turns sooner, more false crosses" },
  { id: "signal-area", name: "Signal area", hint: "Green MACD line over a red signal area" },
];

const MACD_STYLE_IDS = new Set<string>(MACD_STYLES.map((s) => s.id));
export const isMacdStyle = (v: unknown): v is MacdStyle => typeof v === "string" && MACD_STYLE_IDS.has(v);

/** A spec's MACD style (classic for anything else). */
export function macdStyleOf(s: IndicatorSpec): MacdStyle {
  return s.kind === "macd" && s.style ? s.style : "classic";
}

/** Whether a MACD draws its crossover markers: as set, else off for classic (current charts look the same) and on for the rest. */
export function macdMarkersOn(s: IndicatorSpec): boolean {
  return s.kind === "macd" && (s.markers ?? macdStyleOf(s) !== "classic");
}

/** MACD-V's stretched levels, drawn dotted in the warning hue. */
export const MACD_V_LEVELS = [150, -150] as const;

/** Styles the server computes differently from classic (four-color is classic's values). */
const computedStyle = (s: IndicatorSpec) => s.kind === "macd" && !!s.style && s.style !== "classic" && s.style !== "four-color";

/**
 * The specs as a server without `indicators.macd.v2` can serve them: every computed MACD style
 * back to classic (such a server ignores `style` and sends classic under the classic key).
 * Markers, colours and the 4-color shading stay: they are drawn here, from any server's values.
 * The same array back when nothing changes.
 */
export function classicMacd(specs: readonly IndicatorSpec[]): readonly IndicatorSpec[] {
  if (!specs.some(computedStyle)) return specs;
  return specs.map((s) => {
    if (!computedStyle(s)) return s;
    const { style: _, ...rest } = s;
    return { ...rest, markers: macdMarkersOn(s) };
  });
}

export type IndicatorPlacement = "overlay" | "pane";

export const INDICATOR_PLACEMENT: Readonly<Record<IndicatorKind, IndicatorPlacement>> = {
  sma: "overlay",
  ema: "overlay",
  vwap: "overlay",
  bb: "overlay",
  rsi: "pane",
  macd: "pane",
};

export const MAX_OVERLAYS = 3;
export const MAX_PANES = 2;

/** The catalogue the indicator menus offer, with day-trading defaults. */
export const INDICATOR_PRESETS: readonly IndicatorSpec[] = [
  { id: "vwap", kind: "vwap" },
  { id: "ema-9", kind: "ema", period: 9 },
  { id: "ema-20", kind: "ema", period: 20 },
  { id: "sma-50", kind: "sma", period: 50 },
  { id: "bb-20", kind: "bb", period: 20, mult: 2 },
  { id: "rsi-14", kind: "rsi", period: 14 },
  { id: "macd", kind: "macd", fast: 12, slow: 26, signal: 9 },
];

export function indicatorLabel(s: IndicatorSpec): string {
  switch (s.kind) {
    case "sma":
      return `SMA ${s.period ?? 20}`;
    case "ema":
      return `EMA ${s.period ?? 9}`;
    case "vwap":
      return "VWAP";
    case "bb":
      return `BB ${s.period ?? 20} ${s.mult ?? 2}`;
    case "rsi":
      return `RSI ${s.period ?? 14}`;
    case "macd":
      return macdLabel(s);
  }
}

/** A MACD's legend name, by style (the periods always shown: they stay editable). */
function macdLabel(s: IndicatorSpec): string {
  const p = `${s.fast ?? 12} ${s.slow ?? 26} ${s.signal ?? 9}`;
  switch (macdStyleOf(s)) {
    case "ppo":
      return `PPO ${p}`;
    case "macd-v":
      return `MACD-V ${p}`;
    case "zero-lag":
      return `ZL MACD ${p}`;
    case "signal-area":
      return `MACD ${s.fast ?? 12} ${s.slow ?? 26} · signal SMA ${s.signal ?? 9}`;
    default:
      return `MACD ${p}`;
  }
}

/**
 * The canonical key the server's indicator frames carry for a spec (`chart.indicators`,
 * `chart.indicator`; crates/chart-engine/src/indicators.rs `IndicatorSpec::key`): the kind and
 * its RESOLVED parameters, so two specs that compute the same values share one server
 * computation whatever ids the clients gave them. Both sides are tested against the same
 * vectors (crates/chart-engine/tests/fixtures/indicator_spec_keys.json); change one only with
 * the other.
 */
export function specKey(s: IndicatorSpec): string {
  switch (s.kind) {
    case "sma":
      return `sma:${s.period ?? 20}`;
    case "ema":
      return `ema:${s.period ?? 9}`;
    case "vwap":
      return "vwap";
    case "bb":
      return `bb:${s.period ?? 20}:${String(s.mult ?? 2)}`;
    case "rsi":
      return `rsi:${s.period ?? 14}`;
    case "macd": {
      // A computed style is its own study (`macd:12:26:9:ppo`); classic and 4-color share classic's key.
      const base = `macd:${s.fast ?? 12}:${s.slow ?? 26}:${s.signal ?? 9}`;
      return computedStyle(s) ? `${base}:${s.style}` : base;
    }
  }
}

/** One drawn series, aligned to the slots (NaN = nothing at that slot). */
export interface IndicatorLine {
  name: string;
  values: Float64Array;
  /** "line" draws a polyline; "histogram" draws bars from zero (MACD). */
  style: "line" | "histogram";
}

export interface IndicatorResult {
  spec: IndicatorSpec;
  placement: IndicatorPlacement;
  lines: IndicatorLine[];
  /** bb: fill between lines[1] (upper) and lines[2] (lower). */
  band?: [number, number];
  /** pane: a fixed value range (RSI 0–100); absent = fit the visible values. */
  fixedRange?: { lo: number; hi: number };
  /** pane: horizontal guide values (RSI 30/70, MACD 0). */
  guides?: number[];
  /** pane: a shaded zone between two values (RSI 30–70) and an unlabelled faint midline (RSI 50). */
  zone?: { lo: number; hi: number };
  midline?: number;
  /** pane: dotted reference levels in the warning hue, labelled (MACD-V +/-150). */
  levels?: number[];
  /** Categorical colour slot of the first line (overlays only; panes start at 0). */
  colorSlot: number;
}

const nanArray = (n: number) => new Float64Array(n).fill(Number.NaN);

/** The line names and styles of a spec, in drawing order (the server's frames use the same names). */
export function indicatorLineNames(kind: IndicatorKind): readonly { name: string; style: IndicatorLine["style"] }[] {
  return LINE_NAMES[kind];
}

const LINE: IndicatorLine["style"] = "line";
const LINE_NAMES: Readonly<Record<IndicatorKind, readonly { name: string; style: IndicatorLine["style"] }[]>> = {
  sma: [{ name: "SMA", style: LINE }],
  ema: [{ name: "EMA", style: LINE }],
  vwap: [{ name: "VWAP", style: LINE }],
  bb: [
    { name: "Basis", style: LINE },
    { name: "Upper", style: LINE },
    { name: "Lower", style: LINE },
  ],
  rsi: [{ name: "RSI", style: LINE }],
  macd: [
    { name: "Hist", style: "histogram" },
    { name: "MACD", style: LINE },
    { name: "Signal", style: LINE },
  ],
};

const SIGNAL_AREA_LINES: readonly { name: string; style: IndicatorLine["style"] }[] = [
  { name: "MACD", style: LINE },
  { name: "Signal", style: LINE },
];

/** A spec's lines (the server's frames use the same names): signal area has no histogram. */
export function specLineNames(spec: IndicatorSpec): readonly { name: string; style: IndicatorLine["style"] }[] {
  return spec.kind === "macd" && spec.style === "signal-area" ? SIGNAL_AREA_LINES : LINE_NAMES[spec.kind];
}

/**
 * The results for `specs`, values from `valuesOf` (one array per line, in indicatorLineNames
 * order) and everything else (placement, colour slot, band, RSI range and guides) decided here
 * once, so the server's values (serverIndicatorResults) draw exactly as the former local math did
 * (indicatorMath.ts, kept for the fixtures and tests only). Overlays beyond MAX_OVERLAYS and panes beyond MAX_PANES are dropped (the
 * menus never offer more; the server drops the same ones), so the colour slots stay inside the
 * validated set.
 */
export function assembleIndicators(specs: readonly IndicatorSpec[], valuesOf: (spec: IndicatorSpec) => Float64Array[]): IndicatorResult[] {
  const out: IndicatorResult[] = [];
  let overlays = 0;
  let panes = 0;
  for (const spec of specs) {
    const placement = INDICATOR_PLACEMENT[spec.kind];
    if (placement === "overlay" ? overlays >= MAX_OVERLAYS : panes >= MAX_PANES) continue;
    const colorSlot = placement === "overlay" ? overlays++ : (panes++, 0);
    const values = valuesOf(spec);
    const lines = specLineNames(spec).map((l, k): IndicatorLine => ({ name: l.name, style: l.style, values: values[k]! }));
    const r: IndicatorResult = { spec, placement, colorSlot, lines };
    if (spec.kind === "bb") r.band = [1, 2];
    if (spec.kind === "rsi") Object.assign(r, { fixedRange: { lo: 0, hi: 100 }, guides: [30, 70], zone: { lo: 30, hi: 70 }, midline: 50 });
    if (spec.kind === "macd") r.guides = [0];
    if (spec.kind === "macd" && spec.style === "macd-v") r.levels = [...MACD_V_LEVELS];
    out.push(r);
  }
  return out;
}

/**
 * The server's values (ChartStore `getIndicators`, `indicators.v1`) as results over the shown
 * slots: each line joined onto the slots by bar time (both ascending, one merge pass), NaN on
 * gap slots and wherever the server has no value. A spec whose key the server has not sent yet
 * (just toggled on: the `chart.indicators` answer is on its way) draws nothing rather than
 * falling back to local math, so a line never jumps between two sources.
 */
export function serverIndicatorResults(slots: readonly Slot[], specs: readonly IndicatorSpec[], server: IndicatorValuesLike | null): IndicatorResult[] {
  // Slot i's index in server.t, -1 = none.
  const at = new Int32Array(slots.length).fill(-1);
  if (server) {
    const t = server.t;
    let j = 0;
    for (let i = 0; i < slots.length; i++) {
      const s = slots[i]!;
      if (s.kind !== "candle") continue;
      while (j < t.length && t[j]! < s.t) j++;
      if (j < t.length && t[j] === s.t) at[i] = j;
    }
  }
  return assembleIndicators(specs, (spec) => {
    const study = server?.studies.get(specKey(spec));
    return specLineNames(spec).map((l) => {
      const out = nanArray(slots.length);
      const src = study?.lines.find((x) => x.name === l.name)?.values;
      if (src) for (let i = 0; i < at.length; i++) if (at[i]! >= 0) out[i] = src[at[i]!]!;
      return out;
    });
  });
}

/**
 * Whether a chart showing slots from index `from` on reaches past the server's values: they were
 * cut to a `limit` (`truncated`) and the first visible candle is older than their first bar. The
 * host then asks again without the limit (the lines would otherwise stop short of the screen's
 * left edge).
 */
export function needsIndicatorHistory(slots: readonly Slot[], from: number, server: IndicatorValuesLike | null | undefined): boolean {
  if (!server?.truncated || server.t.length === 0) return false;
  for (let i = Math.max(0, from); i < slots.length; i++) {
    const s = slots[i]!;
    if (s.kind === "candle") return s.t < server.t[0]!;
  }
  return false;
}

/** What serverIndicatorResults reads: ChartStore's `IndicatorValues` (store.ts). */
export interface IndicatorValuesLike {
  /** Bar times, ascending. */
  readonly t: readonly number[];
  /** The server left older bars out (`limit`): its values start at `t[0]`, not at the chart's first candle. */
  readonly truncated?: boolean;
  /** By spec key; each line's values parallel to `t`, NaN = none. */
  readonly studies: ReadonlyMap<string, { readonly lines: readonly { readonly name: string; readonly values: ArrayLike<number> }[] }>;
}

/**
 * What a chart asks the server for (`indicators.v1`): its effective spec list (none while the
 * indicators layer is hidden, so the server computes nothing nobody sees) and whether it shows
 * the regular session only (extended hours off). The host sends it on `chart.subscribe` and
 * `chart.indicators.set`, and the same `rth` picks the values to draw (ChartStore `getIndicators`).
 */
export interface IndicatorDemand {
  indicators: IndicatorSpec[];
  rth: boolean;
  /**
   * The Levels menu's zones layer is on (`zones.v1`): the server computes the chart's supply/demand
   * zones over the same `rth` and pushes `chart.zones`. Absent = off.
   */
  zones?: boolean;
  /**
   * The most bars the indicator snapshot should carry, the newest (`indicators.limit.v1`): the
   * phone's cellular budget, dropped once the chart is zoomed out past them
   * (`needsIndicatorHistory`). Absent = every bar.
   */
  limit?: number;
}

export function indicatorDemand(specs: readonly IndicatorSpec[], settings: { extendedHours: boolean; visibility: { indicators: boolean } }, layers?: { autoZones: boolean }): IndicatorDemand {
  return { indicators: settings.visibility.indicators ? specs.map(serverSpec) : [], rth: !settings.extendedHours, zones: layers?.autoZones ?? false };
}

/** A spec as the server needs it: the drawing-only fields (colours, MACD markers) left off the wire. */
export function serverSpec(s: IndicatorSpec): IndicatorSpec {
  if (!s.colors && s.markers === undefined) return s;
  const { colors: _c, markers: _m, ...rest } = s;
  return rest;
}

/** Same demand (to skip a `chart.indicators.set` that would change nothing)? */
export function sameIndicatorDemand(a: IndicatorDemand, b: IndicatorDemand): boolean {
  return a.rth === b.rth && (a.zones ?? false) === (b.zones ?? false) && a.limit === b.limit && a.indicators.length === b.indicators.length && a.indicators.every((s, i) => s.id === b.indicators[i]!.id && specKey(s) === specKey(b.indicators[i]!));
}

/**
 * Colour of a result's k-th line: the person's colour for its role (IndicatorSpec.colors) when
 * set, else the default, its categorical slot (overlays: one slot for all their lines; panes:
 * the slot of the line, MACD first and signal second).
 */
export function indicatorColor(theme: ChartTheme, r: IndicatorResult, k: number): string {
  return roleColor(theme, r.spec, r.colorSlot, lineRole(r.spec, k));
}

// --- Line colours (owner, 2026-11-07: "change the color of indicator lines from each indicator's
// settings"). A colour belongs to a ROLE of the study (its MACD line, a band's fill), not to a
// line index, so it survives a style switch that reorders the lines. The picker shows and stores
// a plain `#rrggbb` hue; where a role draws translucent (a band's fill, a histogram's faded bars,
// the signal area), layout.ts applies the same alpha to the picked hue as to the default.

export type ColorRole = "line" | "basis" | "upper" | "lower" | "fill" | "macd" | "signal" | "histUp" | "histDown" | "upStrong" | "upSoft" | "downStrong" | "downSoft" | "area";

const ROLE_NAMES: Readonly<Record<ColorRole, string>> = {
  line: "Line",
  basis: "Middle line",
  upper: "Upper band",
  lower: "Lower band",
  fill: "Band fill",
  macd: "MACD line",
  signal: "Signal line",
  histUp: "Histogram up",
  histDown: "Histogram down",
  upStrong: "Up, building",
  upSoft: "Up, fading",
  downStrong: "Down, building",
  downSoft: "Down, fading",
  area: "Signal area",
};

const ALL_ROLES = new Set(Object.keys(ROLE_NAMES));
export const isColorRole = (v: string): v is ColorRole => ALL_ROLES.has(v);
export const isHexColor = (v: unknown): v is string => typeof v === "string" && /^#[0-9a-f]{6}$/i.test(v);

/** The colours a spec's settings offer, in the order its settings list them. */
export function indicatorColorRoles(spec: IndicatorSpec): readonly { role: ColorRole; name: string }[] {
  const roles = ((): ColorRole[] => {
    switch (spec.kind) {
      case "bb":
        return ["upper", "basis", "lower", "fill"];
      case "macd":
        switch (macdStyleOf(spec)) {
          case "four-color":
            return ["macd", "signal", "upStrong", "upSoft", "downStrong", "downSoft"];
          case "signal-area":
            return ["macd", "area"];
          default:
            return ["macd", "signal", "histUp", "histDown"];
        }
      default:
        return ["line"];
    }
  })();
  return roles.map((role) => ({ role, name: ROLE_NAMES[role] }));
}

/** The role of a result's k-th line (specLineNames order). */
function lineRole(spec: IndicatorSpec, k: number): ColorRole {
  if (spec.kind === "bb") return (["basis", "upper", "lower"] as const)[k] ?? "basis";
  if (spec.kind !== "macd") return "line";
  const name = specLineNames(spec)[k]?.name;
  return name === "Signal" ? "signal" : name === "Hist" ? "histUp" : "macd";
}

/** A role's default hue, as the picker shows it (the theme's colour; the alpha a role draws at is layout's). */
export function defaultRoleColor(theme: ChartTheme, spec: IndicatorSpec, colorSlot: number, role: ColorRole): string {
  const series = [theme.series1, theme.series2, theme.series3];
  const slot = series[colorSlot % series.length]!;
  switch (role) {
    case "macd":
      return macdStyleOf(spec) === "signal-area" ? theme.up : theme.series1;
    case "signal":
      return theme.series2;
    case "histUp":
    case "upStrong":
    case "upSoft":
      return theme.up;
    case "histDown":
    case "downStrong":
    case "downSoft":
    case "area":
      return theme.down;
    default:
      return spec.kind === "rsi" ? theme.series1 : slot;
  }
}

/** The hue a role draws in: the person's when set, else the default. */
export function roleColor(theme: ChartTheme, spec: IndicatorSpec, colorSlot: number, role: ColorRole): string {
  const picked = spec.colors?.[role];
  return picked && isHexColor(picked) ? picked : defaultRoleColor(theme, spec, colorSlot, role);
}

/** The picker's preset row: the palette's indicator hues (categorical slots, up/down, warning, accent, neutrals). */
export function indicatorSwatches(theme: ChartTheme): string[] {
  return [...new Set([theme.series1, theme.series2, theme.series3, theme.up, theme.down, theme.luldText, theme.alert, theme.drawing, theme.textDim, theme.text].filter(isHexColor).map((c) => c.toLowerCase()))];
}

/** The spec with one role's colour set (null = back to the default). */
export function withIndicatorColor(spec: IndicatorSpec, role: ColorRole, color: string | null): IndicatorSpec {
  const colors = { ...spec.colors };
  if (color && isHexColor(color)) colors[role] = color.toLowerCase();
  else delete colors[role];
  const { colors: _, ...rest } = spec;
  return Object.keys(colors).length ? { ...rest, colors } : rest;
}

/** The spec with every colour back to its default ("Reset to default"). */
export function resetIndicatorColors(spec: IndicatorSpec): IndicatorSpec {
  const { colors: _, ...rest } = spec;
  return rest;
}

/** Replaces the spec with `id` in a list (a settings change), keeping its place. */
export function replaceIndicator(specs: readonly IndicatorSpec[], next: IndicatorSpec): IndicatorSpec[] {
  return specs.map((s) => (s.id === next.id ? next : s));
}

/**
 * The menus' glyph tint for a spec: the person's colour for its main role when set (its line, a
 * band's upper edge, the MACD line), else `fallback` (the slot colour the menu computed).
 */
export function indicatorMenuColor(spec: IndicatorSpec, fallback: string): string {
  const main = indicatorColorRoles(spec)[0]?.role;
  const picked = main ? spec.colors?.[main] : undefined;
  return picked && isHexColor(picked) ? picked : fallback;
}

/** Toggles a preset on or off, respecting the overlay and pane caps. Returns the specs unchanged when the cap blocks it. */
export function toggleIndicator(specs: readonly IndicatorSpec[], preset: IndicatorSpec): IndicatorSpec[] {
  if (specs.some((s) => s.id === preset.id)) return specs.filter((s) => s.id !== preset.id);
  const placement = INDICATOR_PLACEMENT[preset.kind];
  const used = specs.filter((s) => INDICATOR_PLACEMENT[s.kind] === placement).length;
  if (used >= (placement === "overlay" ? MAX_OVERLAYS : MAX_PANES)) return [...specs];
  return [...specs, preset];
}

/** Whether a preset can be switched on now (false when its placement is full). */
export function canAddIndicator(specs: readonly IndicatorSpec[], preset: IndicatorSpec): boolean {
  if (specs.some((s) => s.id === preset.id)) return true;
  const placement = INDICATOR_PLACEMENT[preset.kind];
  return specs.filter((s) => INDICATOR_PLACEMENT[s.kind] === placement).length < (placement === "overlay" ? MAX_OVERLAYS : MAX_PANES);
}

/** What a menu shows for an indicator: its short name, its settings, and one line on what it measures. */
export interface IndicatorInfo {
  name: string;
  params: string;
  description: string;
}

export function indicatorInfo(s: IndicatorSpec): IndicatorInfo {
  switch (s.kind) {
    case "vwap":
      return { name: "VWAP", params: "", description: "Volume-weighted average price, resets daily" };
    case "ema":
      return { name: "EMA", params: String(s.period ?? 9), description: (s.period ?? 9) <= 10 ? "Fast exponential average" : "Exponential moving average" };
    case "sma":
      return { name: "SMA", params: String(s.period ?? 20), description: "Simple moving average" };
    case "bb":
      return { name: "Bollinger", params: `${s.period ?? 20} · ${s.mult ?? 2}σ`, description: "Volatility bands around the average" };
    case "rsi":
      return { name: "RSI", params: String(s.period ?? 14), description: "Momentum from 0 to 100, lines at 30 and 70" };
    case "macd":
      return macdStyleOf(s) === "classic"
        ? { name: "MACD", params: `${s.fast ?? 12} ${s.slow ?? 26} ${s.signal ?? 9}`, description: "Trend momentum, with signal and histogram" }
        : { name: MACD_STYLES.find((x) => x.id === macdStyleOf(s))!.name, params: `${s.fast ?? 12} ${s.slow ?? 26} ${s.signal ?? 9}`, description: MACD_STYLES.find((x) => x.id === macdStyleOf(s))!.hint };
  }
}

/**
 * Icon glyphs per indicator kind, as duotone path data on a 16 px grid
 * (icons.ts), so the web (inline <svg>) and the phone (react-native-svg)
 * draw the same icon. The study's own line is primary; the price or guides
 * it is read against, and a band's area, are secondary. Each menu tints the
 * glyph with the line's colour, so both tones follow it.
 */
export const INDICATOR_GLYPHS: Readonly<Record<IndicatorKind, readonly IconPath[]>> = {
  vwap: [
    { d: "M2 14v-2.5M5 14v-4M8 14v-1.5M11 14v-3M14 14v-2", role: "secondary" },
    { d: "M1.5 8.5c2-1.5 4-3.5 6.5-3.5s4.5 1.5 6.5 0.5", role: "primary" },
  ],
  ema: [
    { d: "M1.5 12 4.5 8.5l2.5 2 3.5-6 4 3", role: "secondary" },
    { d: "M1.5 11c3-1 4.5-4.5 7-5.5s4 0 6-1.5", role: "primary" },
  ],
  sma: [
    { d: "M1.5 12 4.5 8.5l2.5 2 3.5-6 4 3", role: "secondary" },
    { d: "M1.5 12c3.5-.5 5-3 7-4s4-.5 6-1.5", role: "primary" },
  ],
  bb: [
    { d: "M1.5 6c3-2 5-2.5 7-1.5s4 1 6-1v6c-2 1.5-4 2-6 1s-4-.5-7 1.5z", paint: "fill", role: "secondary" },
    { d: "M1.5 6c3-2 5-2.5 7-1.5s4 1 6-1M1.5 12c3-2 5-2.5 7-1.5s4 1 6-1", role: "primary" },
  ],
  rsi: [
    { d: "M1.5 4.5h13M1.5 11.5h13", role: "secondary" },
    { d: "M1.5 9c1.5-5 3-5 4.5-1s3 5 4.5 1 2.5-4.5 4-3", role: "primary" },
  ],
  macd: [
    { d: "M3 8v3M5.5 8v4.5M8 8v2M10.5 8V6M13 8V4.5", role: "secondary" },
    { d: "M1.5 10c3-1 5-5 8-5.5s4 .5 5 1", role: "primary" },
  ],
};

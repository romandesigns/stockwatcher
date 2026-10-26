// Every icon in the product, defined once as SVG path data, so the web
// (inline <svg>, chart-web Icon) and the phone (react-native-svg,
// chart-native Icon) draw the same marks in the same two tones.
//
// Duotone (owner request 2026-10-18: "make them two complementary tones"):
// each path has a ROLE, never a colour.
//   primary    the object the icon is about (the tool's line, the arrow, the
//              lens ring), in the icon's current colour: the state's text
//              colour, so idle icons stay neutral and only an active state
//              that spends accent (an armed drawing tool, a menu check, the
//              Trade and Play buttons) turns accent.
//   secondary  context and fill (price behind a level, a drawing's handles, a
//              soft body inside an outline), the SAME colour at
//              ICON_SECONDARY_OPACITY. Derived rather than a second hue, so
//              the pair is complementary on any surface and in any state, and
//              the accent never leaks into idle icons (60/30/10).
// A path may carry a status `hue` (good / critical / warning) where the hue
// IS the meaning (a long position's profit zone, a halt): it keeps its role,
// so a hued path is still split into a full and a soft tone.
//
// Renderers map roles to their theme tokens: the web's `--icon-primary` /
// `--icon-secondary` CSS variables (apps/web index.css), the phone's
// `color` + ICON_SECONDARY_OPACITY. Strokes are round-capped and
// round-joined; widths are in viewBox units.

export type IconRole = "primary" | "secondary";
export type IconHue = "good" | "critical" | "warning";

export interface IconPath {
  d: string;
  role: IconRole;
  /** Default "stroke". */
  paint?: "stroke" | "fill";
  /** A status hue in place of the icon's current colour (still split by role). */
  hue?: IconHue;
  dash?: readonly number[];
  /** Stroke width override in viewBox units (default: the icon's strokeWidth). */
  width?: number;
}

export interface IconDef {
  /** viewBox width and height (16 × 16 unless said otherwise). */
  w: number;
  h: number;
  /** Default stroke width in viewBox units. */
  strokeWidth: number;
  paths: readonly IconPath[];
}

/**
 * The secondary tone's opacity. 0.4 is the long-standing duotone convention
 * (Font Awesome's duotone default): soft enough to read as the second voice,
 * strong enough that a 1.5 px secondary stroke survives on the 14 px toolbar
 * icons. apps/web index.css spells the same number as `--icon-secondary`'s
 * color-mix percentage; a test there keeps the two in step.
 */
export const ICON_SECONDARY_OPACITY = 0.4;

/** A circle as path data (SVG <circle> is not shared by every renderer's path API). */
export function circlePath(cx: number, cy: number, r: number): string {
  return `M${cx - r} ${cy}a${r} ${r} 0 1 0 ${2 * r} 0a${r} ${r} 0 1 0 ${-2 * r} 0z`;
}

const P = (d: string, extra: Partial<IconPath> = {}): IconPath => ({ d, role: "primary", ...extra });
const S = (d: string, extra: Partial<IconPath> = {}): IconPath => ({ d, role: "secondary", ...extra });
const icon = (paths: readonly IconPath[], strokeWidth = 1.5): IconDef => ({ w: 16, h: 16, strokeWidth, paths });

const CURSOR = "M4 2.5v10l3-3 2 4.5 1.6-.7-2-4.4H13z";
const TRASH_BODY = "M4.5 4.5l.6 8.5h5.8l.6-8.5";
const PENCIL = "M10.5 2.5 13.5 5.5 5.5 13.5H2.5v-3z";
const PLAY = "M4.8 3.4v9.2l7.6-4.6z";
const STEP = "M4 3.5 9.5 8 4 12.5z";
const DROP = "M8 2.2c2.6 3 4.2 5.2 4.2 7.2a4.2 4.2 0 0 1-8.4 0c0-2 1.6-4.2 4.2-7.2z";
const STAR = "M8 2 9.59 6.22 14.09 6.42 10.57 9.23 11.76 13.58 8 11.1 4.24 13.58 5.43 9.23 1.91 6.42 6.41 6.22z";

export type IconName =
  | "cursor"
  | "trend"
  | "ray"
  | "hline"
  | "vline"
  | "rect"
  | "supply"
  | "demand"
  | "measure"
  | "long"
  | "short"
  | "trash"
  | "undo"
  | "redo"
  | "rotate"
  | "grip"
  | "indicators"
  | "levels"
  | "trades"
  | "trade"
  | "pencil"
  | "search"
  | "clear"
  | "live"
  | "replay"
  | "play"
  | "pause"
  | "step"
  | "alertPrev"
  | "alertNext"
  | "toLatest"
  | "chevronDown"
  | "chevronUp"
  | "chevronRight"
  | "star"
  | "starOn"
  | "chevronLeft"
  | "check"
  | "menu"
  | "chart"
  | "chartCandles"
  | "chartHollow"
  | "chartBars"
  | "chartHeikinAshi"
  | "chartLine"
  | "chartArea"
  | "chartBaseline"
  | "settings"
  | "color"
  | "dashboard"
  | "sidebarCollapse"
  | "sidebarExpand"
  | "screen"
  | "gauge"
  | "bell"
  | "plus"
  | "more"
  | "calendar";

export const ICONS: Readonly<Record<IconName, IconDef>> = {
  // Drawing tools: the drawn object is primary, its handles and fills secondary.
  cursor: icon([S(CURSOR, { paint: "fill" }), P(CURSOR, { width: 1.1 })]),
  trend: icon([P("M3 12.5 13 3.5"), S(circlePath(3, 12.5, 1.8), { paint: "fill" }), S(circlePath(13, 3.5, 1.8), { paint: "fill" })]),
  ray: icon([P("M3 12.5 15 2"), S(circlePath(3, 12.5, 1.8), { paint: "fill" })]),
  hline: icon([P("M1.5 8h13"), S(circlePath(8, 8, 1.8), { paint: "fill" })]),
  vline: icon([P("M8 1.5v13"), S(circlePath(8, 8, 1.8), { paint: "fill" })]),
  rect: icon([S("M4 4h8a1 1 0 0 1 1 1v6a1 1 0 0 1-1 1H4a1 1 0 0 1-1-1V5a1 1 0 0 1 1-1z", { paint: "fill" }), P(circlePath(3, 4, 1.3), { paint: "fill" }), P(circlePath(13, 12, 1.3), { paint: "fill" })]),
  // Zones: the band in the hue that is its meaning (supply red above, demand green below), the price
  // path that meets it primary.
  supply: icon([S("M3 2.5h10a1 1 0 0 1 1 1v3H2v-3a1 1 0 0 1 1-1z", { paint: "fill", hue: "critical" }), P("M2 13.5 5 10l2 2 3-4.5 2 1.5")]),
  demand: icon([S("M2 9.5h12v3a1 1 0 0 1-1 1H3a1 1 0 0 1-1-1z", { paint: "fill", hue: "good" }), P("M2 2.5 5 6l2-2 3 4.5 2-1.5")]),
  // The ruler: a measure is not stored, it reads between two points.
  measure: icon([P("M2 11.5 11.5 2l2.5 2.5L4.5 14z"), S("M5 9.5l1 1M7 7.5l1.5 1.5M9 5.5l1 1")]),
  // Position tools: the green zone on the side the trade profits, the red one on the side it loses; the entry line is the object.
  long: icon([
    S("M4 2h8a1 1 0 0 1 1 1v5H3V3a1 1 0 0 1 1-1z", { paint: "fill", hue: "good" }),
    S("M3 8h10v4a1 1 0 0 1-1 1H4a1 1 0 0 1-1-1z", { paint: "fill", hue: "critical" }),
    P("M2 8h12"),
  ]),
  short: icon([
    S("M4 3h8a1 1 0 0 1 1 1v4H3V4a1 1 0 0 1 1-1z", { paint: "fill", hue: "critical" }),
    S("M3 8h10v5a1 1 0 0 1-1 1H4a1 1 0 0 1-1-1z", { paint: "fill", hue: "good" }),
    P("M2 8h12"),
  ]),

  // Edit actions.
  trash: icon([S(`${TRASH_BODY}z`, { paint: "fill" }), P(TRASH_BODY), P("M3 4.5h10M6.5 4.5V3h3v1.5")]),
  undo: icon([P("M5.5 3 2.5 6l3 3M2.5 6h7"), S("M9.5 6a4 4 0 0 1 0 8h-2")]),
  redo: icon([P("M10.5 3l3 3-3 3M13.5 6h-7"), S("M6.5 6a4 4 0 0 0 0 8h2")]),
  // The toolbox's orientation flip: the turn it makes primary, the way back secondary.
  rotate: icon([P("M3 8a5 5 0 0 1 8.5-3.5L13 6M13 3v3h-3"), S("M13 8a5 5 0 0 1-8.5 3.5L3 10M3 13v-3h3")]),
  grip: icon([
    ...[4, 8, 12].map((y) => P(circlePath(6, y, 1.1), { paint: "fill" })),
    ...[4, 8, 12].map((y) => S(circlePath(10, y, 1.1), { paint: "fill" })),
  ]),

  // Chart toolbar.
  indicators: icon([S("M1.5 12.5 4.5 9l2.5 2 3.5-5 4 3"), P("M1.5 9.5c2.5-4 4.5-5 6.5-3s4 1.5 6.5-3")]),
  levels: icon([S("M1.5 4h13", { dash: [2, 2] }), P("M1.5 8h13"), S("M1.5 12h13")]),
  trades: icon([S("M6.5 4h8M6.5 8h8M6.5 12h8"), P("M2 5.5 3.5 3 5 5.5z", { paint: "fill" }), P("M2 10.5 3.5 13 5 10.5z", { paint: "fill" })]),
  // Trade: buy is the primary arrow, sell its secondary twin. Sits on the accent button.
  trade: icon([P("M5 13V3M2.5 5.5 5 3l2.5 2.5"), S("M11 3v10M8.5 10.5 11 13l2.5-2.5")]),
  pencil: icon([S(PENCIL, { paint: "fill" }), P(PENCIL), P("M9 4l3 3")]),

  // App chrome.
  search: icon([S(circlePath(7, 7, 4.5), { paint: "fill" }), P(circlePath(7, 7, 4.5)), P("M10.5 10.5 14 14")]),
  clear: icon([S(circlePath(8, 8, 6.5), { paint: "fill" }), P("M5.75 5.75l4.5 4.5M10.25 5.75l-4.5 4.5")], 1.6),
  // Data source: live radiates (the near waves primary, the far ones secondary); replay is a clock turning back.
  live: icon([P(circlePath(8, 8, 1.6), { paint: "fill" }), P("M5.2 5.2a4 4 0 0 0 0 5.6M10.8 5.2a4 4 0 0 1 0 5.6"), S("M3.1 3.1a7 7 0 0 0 0 9.8M12.9 3.1a7 7 0 0 1 0 9.8")], 1.6),
  replay: icon([P("M2.5 8a5.5 5.5 0 1 0 1.6-3.9"), P("M2.5 2.5v2.2h2.2"), S("M8 5.2V8l1.9 1.3")], 1.6),

  // Replay transport: soft bodies inside crisp outlines, so play/pause/step read as one family.
  play: icon([S(PLAY, { paint: "fill" }), P(PLAY)], 1.4),
  pause: icon([S("M4.5 3.5h2v9h-2zM9.5 3.5h2v9h-2z", { paint: "fill" }), P("M4.5 3.5h2v9h-2zM9.5 3.5h2v9h-2z")], 1.4),
  step: icon([S(STEP, { paint: "fill" }), P(STEP), P("M12 3.5v9")], 1.4),
  // The compact replay bar's Prev/Next alert jumps: a chevron (primary) toward the alert it lands
  // on, drawn as the scrubber draws the day's alerts: a dot (secondary) at the far end.
  alertPrev: icon([S(circlePath(3.5, 8, 2), { paint: "fill" }), P("M12 3.5 7.5 8l4.5 4.5")], 1.7),
  alertNext: icon([P("M4 3.5 8.5 8 4 12.5"), S(circlePath(12.5, 8, 2), { paint: "fill" })], 1.7),
  // Back to the latest candle: the arrowhead and the "now" bar primary, the way there secondary.
  toLatest: icon([S("M2.5 8h9"), P("M8 4.5 11.5 8 8 11.5M13.5 3.5v9")], 1.6),

  // Affordances: a caret with a soft wedge inside it; the check's long stroke primary, its short leg secondary.
  chevronDown: icon([S("M3.2 5.6 8 10.4 12.8 5.6z", { paint: "fill" }), P("M3.2 5.6 8 10.4 12.8 5.6")], 2.4),
  // chevronDown turned over: the Favorites panel's move-up.
  chevronUp: icon([S("M3.2 10.4 8 5.6 12.8 10.4z", { paint: "fill" }), P("M3.2 10.4 8 5.6 12.8 10.4")], 2.4),
  // Favorites (owner, 2026-10-26): off is an outline round a soft star; on fills the star inside
  // a soft halo, so the state reads at a glance without leaning on colour alone.
  star: icon([S(STAR, { paint: "fill" }), P(STAR, { width: 1.3 })]),
  starOn: icon([S(STAR, { width: 2.8 }), P(STAR, { paint: "fill" })]),
  chevronRight: icon([S("M5.6 3.2 10.4 8 5.6 12.8z", { paint: "fill" }), P("M5.6 3.2 10.4 8 5.6 12.8")], 1.8),
  // The single-column Settings' back button: chevronRight mirrored, so "in" and "back" read as one mark.
  chevronLeft: icon([S("M10.4 3.2 5.6 8 10.4 12.8z", { paint: "fill" }), P("M10.4 3.2 5.6 8 10.4 12.8")], 1.8),
  check: icon([S("M2.7 8.3 6 11.6"), P("M6 11.6 13.3 4.3")], 1.6),
  // The phone's tape button: the rows of prints, the newest two primary.
  menu: icon([P("M3 4h10M3 8h10"), S("M3 12h6")]),

  // Chart settings (owner request 2026-10-18), the toolbar button left of Indicators: two candles,
  // Robinhood's chart-icon idea. The near one is the object (primary outline), the far one context.
  chart: icon([S("M9.5 6h3v5h-3z", { paint: "fill" }), S("M11 3.5v10"), P("M3.5 4.5h3v6h-3zM5 2v2.5M5 10.5V14")]),
  // Chart types, one mark each, for the settings panel's type picker.
  chartCandles: icon([P("M3.5 4.5h3v6h-3z", { paint: "fill" }), P("M5 2v12"), S("M9.5 6h3v5h-3z", { paint: "fill" }), S("M11 3.5v10")]),
  chartHollow: icon([P("M3.5 4.5h3v6h-3zM5 2v2.5M5 10.5V14"), S("M9.5 6h3v5h-3z", { paint: "fill" }), S("M11 3.5v10")]),
  chartBars: icon([P("M5 2v12M3 4.5h2M5 11h2"), S("M11 3.5v10M9 11.5h2M11 6h2")]),
  // Heikin-Ashi: a smooth run of candles stepping up, the latest primary.
  chartHeikinAshi: icon([S("M2 9h2.5v4H2zM6 7h2.5v4H6z", { paint: "fill" }), P("M10 3.5h2.5v5H10z", { paint: "fill" }), P("M11.25 2v8.5")]),
  chartLine: icon([S("M2 13.5h12", { dash: [1.5, 2] }), P("M2 11 5.5 7l3 2.5L14 3.5")]),
  chartArea: icon([S("M2 11 5.5 7l3 2.5L14 3.5V14H2z", { paint: "fill" }), P("M2 11 5.5 7l3 2.5L14 3.5")]),
  chartBaseline: icon([S("M1.5 8.5h13", { dash: [1.5, 2] }), P("M2 12 5 9.5 8 11l3-6 3 1.5")]),

  // Settings (owner request 2026-10-18), the header's gear: the toothed ring is the object, its body
  // and the hub context. The teeth are short wide strokes so they still read at 14 px.
  settings: icon([
    S(circlePath(8, 8, 4.4), { paint: "fill" }),
    P(circlePath(8, 8, 4.4)),
    P("M8 1.6v1.6M8 12.8v1.6M1.6 8h1.6M12.8 8h1.6M3.5 3.5l1.1 1.1M11.4 11.4l1.1 1.1M3.5 12.5l1.1-1.1M11.4 4.6l1.1-1.1", { width: 2 }),
    S(circlePath(8, 8, 1.5)),
  ]),
  // A drawing's colour, for the Drawing settings row (the toolbox itself shows the live swatch):
  // a drop, its body soft, its outline crisp.
  color: icon([S(DROP, { paint: "fill" }), P(DROP)]),

  // Main navigation (owner request 2026-10-18). Dashboard: four tiles, the tall one (the main view)
  // and the wide one primary, the two small ones context.
  dashboard: icon([
    P("M3.5 2.5h3a1 1 0 0 1 1 1v5a1 1 0 0 1-1 1h-3a1 1 0 0 1-1-1v-5a1 1 0 0 1 1-1z"),
    S("M9.5 2.5h3a1 1 0 0 1 1 1v1a1 1 0 0 1-1 1h-3a1 1 0 0 1-1-1v-1a1 1 0 0 1 1-1z", { paint: "fill" }),
    P("M9.5 7.5h3a1 1 0 0 1 1 1v4a1 1 0 0 1-1 1h-3a1 1 0 0 1-1-1v-4a1 1 0 0 1 1-1z"),
    S("M3.5 11.5h3a1 1 0 0 1 1 1v0a1 1 0 0 1-1 1h-3a1 1 0 0 1-1-1v0a1 1 0 0 1 1-1z", { paint: "fill" }),
  ]),
  // The nav rail's own toggle: a panel with its rail (secondary) and the way it will move (primary).
  sidebarCollapse: icon([S("M2.5 3.5h11v9h-11z", { paint: "fill" }), P("M2.5 3.5h11v9h-11zM6 3.5v9"), P("M10.5 6.5 9 8l1.5 1.5")]),
  sidebarExpand: icon([S("M2.5 3.5h11v9h-11z", { paint: "fill" }), P("M2.5 3.5h11v9h-11zM6 3.5v9"), P("M9 6.5 10.5 8 9 9.5")]),
  // The dashboard's multi-view control ("screen × n"): a monitor, its glass soft, frame and stand crisp.
  screen: icon([S("M2.5 3h11v7.5h-11z", { paint: "fill" }), P("M3.5 2.5h9a1 1 0 0 1 1 1v6.5a1 1 0 0 1-1 1h-9a1 1 0 0 1-1-1V3.5a1 1 0 0 1 1-1zM8 11v2.5M5.5 13.5h5")]),
  // The Halt watcher (dashboard pressure gauges): the dial soft, the needle and its hub crisp.
  gauge: icon([S("M2.5 11.5a5.5 5.5 0 0 1 11 0"), P("M8 11.5 11 6.5", { width: 1.6 }), P(circlePath(8, 11.5, 1.3), { paint: "fill" })]),
  // Price alerts (2026-10-19, the approved design's bell): its body soft, outline and clapper crisp.
  bell: icon([S("M4 11.5V7.5a4 4 0 0 1 8 0v4l1 1.5H3z", { paint: "fill" }), P("M4 11.5V7.5a4 4 0 0 1 8 0v4l1 1.5H3z"), P("M6.5 14.5h3")], 1.4),
  // "New": the cross crisp on a soft disc (the alerts header's add button).
  plus: icon([S(circlePath(8, 8, 6.5), { paint: "fill" }), P("M8 4.5v7M4.5 8h7")], 1.6),
  // Row menus (an alert's options): three dots, the middle one primary.
  more: icon([S(circlePath(3, 8, 1.4), { paint: "fill" }), P(circlePath(8, 8, 1.4), { paint: "fill" }), S(circlePath(13, 8, 1.4), { paint: "fill" })]),
  // The replay bar's day picker (2026-10-24, the approved two-tier design): the page soft; its frame, header rule and rings crisp.
  calendar: icon([S("M2.5 4.5h11v9h-11z", { paint: "fill" }), P("M3.5 3.5h9a1 1 0 0 1 1 1v8a1 1 0 0 1-1 1h-9a1 1 0 0 1-1-1v-8a1 1 0 0 1 1-1zM2.5 6.5h11M5.5 2v3M10.5 2v3")]),
};

export const ICON_NAMES = Object.keys(ICONS) as IconName[];

/**
 * How one path paints for a given primary colour, as a colour and an
 * opacity: the shared rule both renderers apply (the web reaches the same
 * result through its CSS variables). `hues` maps status hues to the
 * renderer's palette.
 */
export function iconPaint(path: IconPath, primary: string, hues: Readonly<Record<IconHue, string>>): { color: string; opacity: number } {
  return { color: path.hue ? hues[path.hue] : primary, opacity: path.role === "secondary" ? ICON_SECONDARY_OPACITY : 1 };
}

// Volume shelves (2026-10-22; EXPERIMENTAL): the client side of the optional `zones` key on the
// Runners channel's `rocket.snapshot` / `rocket.update` frames (crates/server/src/rocket/).
//
// A shelf is a high-volume node of today's volume-at-price profile since the symbol's first +20 %
// run, in the lower half of the current run (its "launch base"). The server sends the qualifying
// shelves (`bands`, drawn as faint bands on the chart) and, when price comes down from above into
// one, an event it resolves after the fact: held (+10 % before −5 %), lost (the opposite) or
// expired (30 minutes with neither).
//
// What the clients may say is bounded by the research: about 1 in 3 entries held (31.9 %) and
// about 1 in 2 were lost (~52 %). So an entry is a NOTE with that base rate beside it, never a
// prediction, and never in a success colour; a resolution is stated plainly, not celebrated.
// One view model here so the web and the phone say exactly the same thing.

import type { RocketPanelWire, RocketRowView, RocketSectionsView } from "./rocket.ts";

/** A qualifying shelf. Prices in dollars, `formed` in epoch ms. */
export interface ZoneBandWire {
  lo: number;
  hi: number;
  formed: number;
  armed: boolean;
}

export type ZoneEventState = "in_shelf" | "held" | "lost" | "expired";

/**
 * Price came down from above into the shelf `lo`–`hi` at `entry`, at `at` (epoch ms); `resolved`
 * is when the server decided its state (null while `in_shelf`). A resolved event stays in the
 * frame about 10 minutes, then the server sends `event: null`.
 */
export interface ZoneEventWire {
  state: ZoneEventState;
  lo: number;
  hi: number;
  entry: number;
  at: number;
  resolved: number | null;
}

export interface ZoneSymbolWire {
  bands: ZoneBandWire[];
  event: ZoneEventWire | null;
}

/** The frame's `zones`: only symbols with a band or an event; a missing symbol has nothing to show. */
export type ZonesWire = Record<string, ZoneSymbolWire>;

const EVENT_STATES: ReadonlySet<string> = new Set<ZoneEventState>(["in_shelf", "held", "lost", "expired"]);

const isObj = (x: unknown): x is Record<string, unknown> => typeof x === "object" && x !== null && !Array.isArray(x);
const price = (x: unknown): x is number => typeof x === "number" && Number.isFinite(x) && x > 0;
const time = (x: unknown): x is number => typeof x === "number" && Number.isFinite(x);

function parseBand(x: unknown): ZoneBandWire | null {
  if (!isObj(x) || !price(x.lo) || !price(x.hi) || x.hi < x.lo || !time(x.formed)) return null;
  return { lo: x.lo, hi: x.hi, formed: x.formed, armed: x.armed === true };
}

function parseEvent(x: unknown): ZoneEventWire | null {
  if (!isObj(x) || typeof x.state !== "string" || !EVENT_STATES.has(x.state)) return null;
  if (!price(x.lo) || !price(x.hi) || x.hi < x.lo || !time(x.at)) return null;
  return {
    state: x.state as ZoneEventState,
    lo: x.lo,
    hi: x.hi,
    entry: price(x.entry) ? x.entry : x.lo,
    at: x.at,
    resolved: time(x.resolved) ? x.resolved : null,
  };
}

/** One symbol's entry, tolerant: bad bands are dropped; null when nothing usable is left. */
export function parseZoneSymbol(x: unknown): ZoneSymbolWire | null {
  if (!isObj(x)) return null;
  const bands = Array.isArray(x.bands) ? x.bands.map(parseBand).filter((b): b is ZoneBandWire => b !== null) : [];
  const event = parseEvent(x.event);
  return bands.length > 0 || event ? { bands, event } : null;
}

/** A frame's `zones`, tolerant of absence (older servers) and garbage: {} when nothing usable. */
export function parseZones(x: unknown): ZonesWire {
  const out: ZonesWire = {};
  if (!isObj(x)) return out;
  for (const [s, v] of Object.entries(x)) {
    const z = parseZoneSymbol(v);
    if (z) out[s] = z;
  }
  return out;
}

/** The shelves for one symbol from the latest Runners frame; null when it has none. */
export function zonesFor(panel: Pick<RocketPanelWire, "zones"> | null | undefined, symbol: string): ZoneSymbolWire | null {
  const all = panel?.zones;
  if (!symbol || !isObj(all) || !Object.prototype.hasOwnProperty.call(all, symbol)) return null;
  return parseZoneSymbol(all[symbol]);
}

/** The research base rate, shown muted beside an entry: 31.9 % held, ~52 % lost. */
export const ZONE_BASE_RATE = "about 1 in 3 hold, 1 in 2 fail";

/** The chart's band label. */
export const ZONE_BAND_LABEL = "Volume shelf";

/** Prices as the Runners rows show them, with a dollar sign. */
function dollars(x: number): string {
  return `$${x >= 1 ? x.toFixed(2) : x.toFixed(4).replace(/0{1,2}$/, "")}`;
}

/**
 * The note for a row or a chart caption. `tone` is never a success colour: "text" for an entry
 * (plain, it is information), "muted" once resolved. `baseRate` is "" once resolved.
 */
export interface ZoneNoteView {
  state: "in_shelf" | "held" | "lost";
  /** "In volume shelf $2.41–$2.55", "Shelf held", "Shelf lost". */
  label: string;
  /** {@link ZONE_BASE_RATE} while in the shelf, else "". */
  baseRate: string;
  tone: "text" | "muted";
  aria: string;
}

/** The note for a symbol's event; null with no event, or an expired one (nothing to say). */
export function zoneNoteView(z: ZoneSymbolWire | null | undefined): ZoneNoteView | null {
  const e = z?.event;
  if (!e) return null;
  switch (e.state) {
    case "in_shelf": {
      const label = `In volume shelf ${dollars(e.lo)}–${dollars(e.hi)}`;
      return { state: "in_shelf", label, baseRate: ZONE_BASE_RATE, tone: "text", aria: `${label}; historically ${ZONE_BASE_RATE}` };
    }
    case "held":
      return { state: "held", label: "Shelf held", baseRate: "", tone: "muted", aria: "Volume shelf held" };
    case "lost":
      return { state: "lost", label: "Shelf lost", baseRate: "", tone: "muted", aria: "Volume shelf lost" };
    default:
      return null;
  }
}

/**
 * Runners rows with their symbol's note (`zone`) and the note appended to the row's aria, so a
 * screen reader hears it on the row button. Rows without one are returned as they are.
 */
export function withZoneNotes(rows: RocketRowView[], zones: ZonesWire | undefined): RocketRowView[] {
  if (!zones) return rows;
  return rows.map((r) => {
    const zone = zoneNoteView(zonesFor({ zones }, r.symbol));
    return zone ? { ...r, zone, aria: `${r.aria}. ${zone.aria}` } : r;
  });
}

/** The Runners panel's sections with every row's shelf note: what both panels render. */
export function withZoneSections(s: RocketSectionsView, zones: ZonesWire | undefined): RocketSectionsView {
  if (!zones) return s;
  return { bursts: s.bursts ? { ...s.bursts, rows: withZoneNotes(s.bursts.rows, zones) } : null, runners: withZoneNotes(s.runners, zones) };
}

/** A band for the chart (chart-core `VolumeShelf`): prices in dollars, `formed` in epoch ms. */
export interface ZoneBandView {
  lo: number;
  hi: number;
  formed: number;
}

/** The bands to draw for a symbol, in the latest frame only; [] when none. */
export function zoneBandsView(z: ZoneSymbolWire | null | undefined): ZoneBandView[] {
  return z ? z.bands.map((b) => ({ lo: b.lo, hi: b.hi, formed: b.formed })) : [];
}

/** What a chart shows for its symbol: the bands to draw and the caption note. */
export interface ZoneView {
  bands: ZoneBandView[];
  note: ZoneNoteView | null;
}

export const EMPTY_ZONE_VIEW: ZoneView = { bands: [], note: null };

/**
 * A memoised reader for one chart: returns the SAME object until the symbol's bands or note
 * change, so a host can use it as a `useSyncExternalStore` snapshot and a chart is not re-laid
 * out on every Runners frame (each frame is a new object).
 */
export function zoneSelector(): (panel: Pick<RocketPanelWire, "zones"> | null | undefined, symbol: string) => ZoneView {
  let last = { symbol: "", key: "", view: EMPTY_ZONE_VIEW };
  return (panel, symbol) => {
    const z = zonesFor(panel, symbol);
    const key = zoneKey(z);
    if (symbol === last.symbol && key === last.key) return last.view;
    last = { symbol, key, view: key ? { bands: zoneBandsView(z), note: zoneNoteView(z) } : EMPTY_ZONE_VIEW };
    return last.view;
  };
}

/**
 * A key that changes only when what a chart draws or a caption says changes, so a host can hold
 * one array across Runners frames (each frame is a new object) and not re-lay out the chart.
 */
export function zoneKey(z: ZoneSymbolWire | null | undefined): string {
  if (!z) return "";
  const bands = z.bands.map((b) => `${b.lo}:${b.hi}:${b.formed}`).join(",");
  const e = z.event;
  return `${bands}|${e ? `${e.state}:${e.lo}:${e.hi}` : ""}`;
}

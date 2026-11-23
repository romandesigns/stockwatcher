// The HOD Momentum scanner (owner, 2026-11-22): the client side of the server's `hod.*` channel
// (crates/server/src/hod/). Every time a stock in the band prints a new high of day and matches
// one of the policy's categories, the server adds a row tagged with every category it matches;
// this module folds the frames, filters the rows by the categories a person picked (synced
// settings `hodFilter`), and formats the columns. Not an alert: nothing here notifies.

/** The welcome feature a server with the scanner lists (an older one would reject the subscribe). */
export const HOD_FEATURE = "hod.v1";
/** The synced settings namespace of the category filter (per user, every device). */
export const HOD_FILTER_NS = "hodFilter";

export interface HodRowWire {
  id: number;
  /** Epoch ms of the new high. */
  t: number;
  sym: string;
  price: number;
  /** Shares since 04:00 ET. */
  vol: number;
  float: number | null;
  /** Rel vol, daily rate (multiple). */
  rvol: number | null;
  /** Rel vol, last 5 minutes vs the normal for this time of day (multiple; shown as %). */
  rvol5: number | null;
  /** Fractions: 0.12 = +12 %. */
  gap: number | null;
  chg: number;
  chg5: number | null;
  chg10: number | null;
  cats: string[];
}

export interface HodCategoryWire {
  id: string;
  name: string;
  default_on: boolean;
  needs_history: boolean;
}

export interface HodPolicyWire {
  id: string;
  version: number;
  band: [number, number];
  cooldown_secs: number;
  float_low_max: number;
  float_medium_max: number;
  rvol_high_min: number;
  rvol_medium_min: number;
  rvol_sessions: number;
  rvol_base_min: number;
  price_high_min: number;
  momo_run_min: number;
  momo_sessions: number;
  categories: HodCategoryWire[];
}

interface HodHead {
  enabled: boolean;
  /** The `detectors` runtime switch `hod-1`. */
  on: boolean;
  date: string | null;
  gen: number;
  complete: boolean;
  since_ns: number | null;
  hi52_available: boolean;
  facts_loaded: boolean;
}

export interface HodSnapshotMessage extends HodHead {
  type: "hod.snapshot";
  policy: HodPolicyWire;
  rows: HodRowWire[];
}

export interface HodUpdateMessage extends HodHead {
  type: "hod.update";
  prev_gen: number;
  rows: HodRowWire[];
}

export type HodMessage = HodSnapshotMessage | HodUpdateMessage;
export type HodClientMessage = { type: "hod.subscribe" } | { type: "hod.unsubscribe" };

export interface HodState {
  enabled: boolean;
  on: boolean;
  date: string | null;
  gen: number;
  complete: boolean;
  sinceMs: number | null;
  hi52Available: boolean;
  factsLoaded: boolean;
  policy: HodPolicyWire | null;
  /** Newest first. */
  rows: readonly HodRowWire[];
  /** A frame has arrived on the connection open now. */
  connected: boolean;
  /** The server has the scanner (welcome `hod.v1`); false for an older server. */
  available: boolean;
}

export const EMPTY_HOD: HodState = { enabled: false, on: true, date: null, gen: 0, complete: true, sinceMs: null, hi52Available: false, factsLoaded: false, policy: null, rows: [], connected: false, available: true };

/** Rows kept on the client (the server keeps 500). */
export const HOD_MAX_ROWS = 500;

const head = (m: HodHead): Pick<HodState, "enabled" | "on" | "date" | "gen" | "complete" | "sinceMs" | "hi52Available" | "factsLoaded"> => ({
  enabled: m.enabled === true,
  on: m.on !== false,
  date: typeof m.date === "string" ? m.date : null,
  gen: typeof m.gen === "number" ? m.gen : 0,
  complete: m.complete !== false,
  sinceMs: typeof m.since_ns === "number" ? Math.floor(m.since_ns / 1e6) : null,
  hi52Available: m.hi52_available === true,
  factsLoaded: m.facts_loaded === true,
});

const validRows = (v: unknown): HodRowWire[] => (Array.isArray(v) ? v.filter((r): r is HodRowWire => !!r && typeof r === "object" && typeof (r as HodRowWire).sym === "string" && typeof (r as HodRowWire).price === "number" && Array.isArray((r as HodRowWire).cats)) : []);

/**
 * One frame folded in. A snapshot replaces everything; an update prepends its rows when it
 * continues this state (same date, `prev_gen` = our `gen`), else asks for a snapshot (`resync`).
 */
export function applyHodMessage(state: HodState, m: HodMessage): { state: HodState; resync: boolean } {
  if (m.type === "hod.snapshot") {
    return { state: { ...state, ...head(m), policy: m.policy ?? state.policy, rows: validRows(m.rows).slice(0, HOD_MAX_ROWS), connected: true }, resync: false };
  }
  if (!state.policy || m.date !== state.date || m.prev_gen !== state.gen) {
    return { state: { ...state, connected: true }, resync: true };
  }
  return { state: { ...state, ...head(m), rows: [...validRows(m.rows), ...state.rows].slice(0, HOD_MAX_ROWS), connected: true }, resync: false };
}

export interface HodChannel {
  get(): HodState;
  subscribe(fn: () => void): () => void;
}

/** Subscribed on demand like the movers: the first listener opts in, the last opts out. */
export function createHodChannel(io: { subscribe(): void; unsubscribe(): void }): HodChannel & { apply(m: HodMessage): void; setConnected(open: boolean): void; setAvailable(on: boolean): void } {
  let state: HodState = EMPTY_HOD;
  const listeners = new Set<() => void>();
  const emit = () => {
    for (const l of listeners) l();
  };
  return {
    get: () => state,
    subscribe(fn) {
      listeners.add(fn);
      if (listeners.size === 1) io.subscribe();
      return () => {
        if (!listeners.delete(fn) || listeners.size > 0) return;
        io.unsubscribe();
        state = { ...EMPTY_HOD, available: state.available };
      };
    },
    apply(m) {
      const next = applyHodMessage(state, m);
      state = next.state;
      if (next.resync && listeners.size > 0) io.subscribe();
      emit();
    },
    setConnected(open) {
      if (open || !state.connected) return;
      state = { ...state, connected: false };
      emit();
    },
    setAvailable(on) {
      if (state.available === on) return;
      state = { ...state, available: on };
      emit();
    },
  };
}

/** The categories a person shows: the stored list, else each category's default. */
export function parseHodFilter(raw: unknown, policy: HodPolicyWire | null): Set<string> {
  const ids = policy?.categories.map((c) => c.id) ?? [];
  if (raw && typeof raw === "object" && !Array.isArray(raw) && Array.isArray((raw as { on?: unknown }).on)) {
    const on = (raw as { on: unknown[] }).on.filter((s): s is string => typeof s === "string");
    return new Set(policy ? on.filter((s) => ids.includes(s)) : on);
  }
  return new Set(policy?.categories.filter((c) => c.default_on).map((c) => c.id) ?? []);
}

/** The stored filter after flipping `id`. */
export function toggleHodFilter(cur: ReadonlySet<string>, id: string): { on: string[] } {
  const next = new Set(cur);
  if (next.has(id)) next.delete(id);
  else next.add(id);
  return { on: [...next].sort() };
}

/** The rows with at least one shown category, each with only its shown tags. */
export function filterHodRows(rows: readonly HodRowWire[], on: ReadonlySet<string>): { row: HodRowWire; cats: string[] }[] {
  const out: { row: HodRowWire; cats: string[] }[] = [];
  for (const row of rows) {
    const cats = row.cats.filter((c) => on.has(c));
    if (cats.length) out.push({ row, cats });
  }
  return out;
}

/** The tint family of a category (subtle row tint and tag colour, from the palette tokens). */
export type HodTint = "lowFloat" | "medFloat" | "squeeze";
export function hodTint(cat: string): HodTint {
  return cat.startsWith("lf_") ? "lowFloat" : cat.startsWith("mf_") ? "medFloat" : "squeeze";
}

/** A short tag for a category (the full name is the tag's title). */
export const HOD_SHORT: Readonly<Record<string, string>> = {
  lf_momo: "LF · Former momo",
  lf_hrv: "LF · High RV",
  lf_hrv_20: "LF · High RV · $20+",
  lf_mrv: "LF · Med RV",
  mf_hrv_20: "MF · High RV · $20+",
  mf_hrv_u20: "MF · High RV · <$20",
  mf_mrv_20: "MF · Med RV · $20+",
  sq_10_10: "Squeeze 10%/10m",
  sq_5_5: "Squeeze 5%/5m",
  sq_52wk: "52wk breakout",
};

export const hodShort = (cat: string): string => HOD_SHORT[cat] ?? cat;

export function fmtHodPct(x: number | null | undefined, digits = 1): string {
  if (x === null || x === undefined || !Number.isFinite(x)) return "—";
  const v = x * 100;
  return `${v > 0 ? "+" : ""}${v.toFixed(digits)}%`;
}

export function fmtHodShares(n: number | null | undefined): string {
  if (n === null || n === undefined || !Number.isFinite(n)) return "—";
  if (n >= 1e9) return `${(n / 1e9).toFixed(2)}B`;
  if (n >= 1e6) return `${(n / 1e6).toFixed(n >= 1e8 ? 0 : 1)}M`;
  if (n >= 1e3) return `${(n / 1e3).toFixed(n >= 1e5 ? 0 : 1)}K`;
  return String(Math.round(n));
}

export function fmtHodMultiple(x: number | null | undefined): string {
  if (x === null || x === undefined || !Number.isFinite(x)) return "—";
  return `${x >= 100 ? x.toFixed(0) : x >= 10 ? x.toFixed(1) : x.toFixed(2)}x`;
}

export function fmtHodPrice(p: number): string {
  return p >= 1 ? p.toFixed(2) : p.toFixed(4);
}

/** HH:MM:SS in New York time. */
export function fmtHodTime(ms: number): string {
  return new Date(ms).toLocaleTimeString("en-US", { hour12: false, timeZone: "America/New_York" });
}

/** The policy in one line for the panel's footer ("Low float < 10M, medium 10–50M; …"). */
export function hodPolicyLine(p: HodPolicyWire): string {
  const m = (n: number) => fmtHodShares(n);
  return `Low float < ${m(p.float_low_max)}, medium ${m(p.float_low_max)}–${m(p.float_medium_max)}. High rel vol ≥ ${p.rvol_high_min}x, medium ${p.rvol_medium_min}–${p.rvol_high_min}x (daily rate, ${p.rvol_sessions} sessions). Former momo: a +${Math.round(p.momo_run_min * 100)}% day in ${p.momo_sessions} sessions. Momo and squeezes need ≥ ${p.rvol_base_min}x. One row per stock per ${p.cooldown_secs} s. Policy ${p.id} v${p.version}.`;
}

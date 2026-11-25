// The pre-open "Watch" card (owner, 2026-11-25, after AKAN 11-24): 04:00-09:30 ET, a quiet list
// of stocks that look like AKAN did before its open, with why each matched. No sound, no push,
// never a notification (server crates/server/src/prewatch). The profile was fixed before testing
// (research v2 70-akan/PROFILE-A-PRIORI.md, profile B); on Train about 1 in 5 matches ran +30 %
// after the open (twice the plain gappers' rate) and about 2 in 3 were below the open by 11:00,
// which the card says, so it never reads as a call to buy.
//
// Wire contract: `prewatch.subscribe` -> `prewatch.snapshot` (every card), again on every change.
// Gated on the welcome's `prewatch.v1`.

/** The welcome feature a server with the card lists. */
export const PREWATCH_FEATURE = "prewatch.v1";

/** A condition that stopped holding after the card appeared. */
export type WatchFail = "price" | "float" | "volume" | "gap" | "big_day";

export interface WatchCardWire {
  symbol: string;
  /** Epoch ms it first matched. */
  since: number;
  price: number;
  c1: number;
  /** Fraction: 0.27 = +27 % over the prior close. */
  gap: number;
  pmVol: number;
  float: number;
  /** Fraction of the float traded since 04:00. */
  pmVolPct: number;
  /** The recent +50 % day: ISO date and its run (fraction). */
  bigDay: { date: string; run: number };
  fails: WatchFail[];
  /** The server started after 04:00: the volume is short (the card can only be late). */
  partial: boolean;
}

export interface PrewatchSnapshotMessage {
  type: "prewatch.snapshot";
  enabled: boolean;
  date: string | null;
  /** Where the float comes from: "fmp", or "none" (no stock context: no float, no card). */
  floatSource: "fmp" | "none";
  cards: WatchCardWire[];
}

export type PrewatchMessage = PrewatchSnapshotMessage;
export type PrewatchClientMessage = { type: "prewatch.subscribe" | "prewatch.unsubscribe" };

export interface PrewatchState {
  /** The server lists `prewatch.v1`. */
  available: boolean;
  /** A snapshot arrived. */
  snapshot: boolean;
  connected: boolean;
  enabled: boolean;
  floatSource: "fmp" | "none";
  cards: WatchCardWire[];
}

export const EMPTY_PREWATCH: PrewatchState = { available: false, snapshot: false, connected: false, enabled: false, floatSource: "none", cards: [] };

const FAILS = new Set<WatchFail>(["price", "float", "volume", "gap", "big_day"]);
const fin = (v: unknown): v is number => typeof v === "number" && Number.isFinite(v);

function validCard(v: unknown): WatchCardWire | null {
  if (!v || typeof v !== "object") return null;
  const o = v as Record<string, unknown>;
  const bd = o.bigDay as Record<string, unknown> | undefined;
  if (typeof o.symbol !== "string" || !o.symbol || !fin(o.since) || !fin(o.price) || !fin(o.c1) || !fin(o.gap) || !fin(o.pmVol) || !fin(o.float) || !fin(o.pmVolPct)) return null;
  if (!bd || typeof bd.date !== "string" || !fin(bd.run)) return null;
  const fails = Array.isArray(o.fails) ? o.fails.filter((f): f is WatchFail => FAILS.has(f as WatchFail)) : [];
  return { symbol: o.symbol, since: o.since, price: o.price, c1: o.c1, gap: o.gap, pmVol: o.pmVol, float: o.float, pmVolPct: o.pmVolPct, bigDay: { date: bd.date, run: bd.run }, fails, partial: o.partial === true };
}

/** A snapshot folded in: it replaces everything. Matching cards first, then the newest match. */
export function applyPrewatchMessage(state: PrewatchState, m: PrewatchMessage): PrewatchState {
  const cards = (Array.isArray(m.cards) ? m.cards.map(validCard).filter((c): c is WatchCardWire => c !== null) : []).sort((a, b) => Number(a.fails.length > 0) - Number(b.fails.length > 0) || b.since - a.since || a.symbol.localeCompare(b.symbol));
  return { ...state, snapshot: true, connected: true, enabled: m.enabled === true, floatSource: m.floatSource === "fmp" ? "fmp" : "none", cards };
}

export interface PrewatchChannel {
  get(): PrewatchState;
  subscribe(fn: () => void): () => void;
}

/** Subscribed on demand like the News pending card: the first listener opts in, the last opts out. */
export function createPrewatchChannel(io: { subscribe(): void; unsubscribe(): void }): PrewatchChannel & { apply(m: PrewatchMessage): void; setConnected(open: boolean): void; setAvailable(on: boolean): void } {
  let state: PrewatchState = EMPTY_PREWATCH;
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
        state = { ...EMPTY_PREWATCH, available: state.available };
      };
    },
    apply(m) {
      state = applyPrewatchMessage(state, m);
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

/** The panel's item count (0 hides it, like every dashboard panel). */
export function prewatchItemCount(s: PrewatchState): number {
  return s.enabled ? s.cards.length : 0;
}

const pct = (f: number) => `${f >= 0 ? "+" : ""}${Math.round(f * 100)}%`;
/** "2.39M", "850K". */
export function fmtShares(n: number): string {
  if (n >= 1e9) return `${(n / 1e9).toFixed(2)}B`;
  if (n >= 1e6) return `${(n / 1e6).toFixed(2)}M`;
  if (n >= 1e3) return `${Math.round(n / 1e3)}K`;
  return String(Math.round(n));
}

const FAIL_TEXT: Record<WatchFail, string> = {
  price: "price left $1-$20",
  float: "float over 10M",
  volume: "pre-market volume under 50% of float",
  gap: "gap under +15%",
  big_day: "no +50% day in 10 sessions",
};

export interface WatchCardView {
  symbol: string;
  price: string;
  /** "+27% gap". */
  gap: string;
  /** The reasons it matched, each a short fact. */
  reasons: { label: string; value: string }[];
  /** "No longer: gap under +15%", or null while it still matches. */
  faded: string | null;
  /** "Since 07:12", plus the partial note. */
  since: string;
}

const ET_TIME = new Intl.DateTimeFormat("en-US", { hour: "2-digit", minute: "2-digit", hour12: false, timeZone: "America/New_York" });
const shortDate = (iso: string) => {
  const [, m, d] = iso.split("-");
  return m && d ? `${Number(m)}/${Number(d)}` : iso;
};

/** A card in words: what every client shows. */
export function watchCardView(c: WatchCardWire): WatchCardView {
  return {
    symbol: c.symbol,
    price: `$${c.price.toFixed(2)}`,
    gap: `${pct(c.gap)} gap`,
    reasons: [
      { label: "Gap", value: `${pct(c.gap)} over ${c.c1.toFixed(2)}` },
      { label: "Pre-market vol", value: `${fmtShares(c.pmVol)} · ${Math.round(c.pmVolPct * 100)}% of float` },
      { label: "Float", value: fmtShares(c.float) },
      { label: "Big day", value: `${pct(c.bigDay.run)} high on ${shortDate(c.bigDay.date)}` },
    ],
    faded: c.fails.length ? `No longer: ${c.fails.map((f) => FAIL_TEXT[f]).join(", ")}` : null,
    since: `Since ${ET_TIME.format(new Date(c.since))}${c.partial ? " · volume counted from the server's start" : ""}`,
  };
}

/** The card's honest footer (Train, profile B; never a call to act). */
export const WATCH_BASE_RATE = "On past data about 1 in 5 like this ran +30% after the open; about 2 in 3 were below the open by 11:00.";
/** Said when the server has no float source (no stock context). */
export const WATCH_NO_FLOAT = "No live float source on this server, so no stock can be checked.";

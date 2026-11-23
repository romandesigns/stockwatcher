// The "News pending" card (owner, 2026-11-22): one card per stock the exchange halted because news
// is pending (Nasdaq T1, NYSE "news pending"), walking through halted -> news found -> resumption
// set -> resumed. The server (crates/server/src/pending) sends one frame per state change; every
// timer here is computed from the card's timestamps, so nothing is broadcast per tick.
//
// Wire contract: `pending.subscribe` -> `pending.snapshot`, then `pending.update` (changed cards,
// each a full replacement by id, plus removed ids). Gated on the welcome's `pending.v1`.

/** The welcome feature a server with the card lists (an older one would reject the subscribe). */
export const PENDING_FEATURE = "pending.v1";

export type PendingPhase = "halted" | "news" | "resuming" | "resumed";
export type PendingNewsKind = "catalyst" | "offering";

export interface PendingNewsWire {
  id: string;
  /** Raw "provider/sub": "alpaca/benzinga", "sec_edgar/8-K", "press_wire/globenewswire". */
  source: string;
  /** Display label from the server ("Benzinga", "SEC 8-K", "GlobeNewswire"). */
  label: string;
  headline: string | null;
  url: string | null;
  /** Epoch ms published. */
  at: number;
  /** Epoch ms we first saw it. */
  seen: number;
  kind: PendingNewsKind;
}

export interface PendingCardWire {
  id: string;
  sym: string;
  /** Epoch ms of the halt's start (feed clock). */
  halted_at: number;
  code: string;
  reason: string;
  price: number | null;
  prev_close: number | null;
  /** Percent units: 45.2 = +45.2 % into the halt. */
  chg_pct: number | null;
  phase: PendingPhase;
  /** The halt start's session: an after-hours T1 gets a small tag (T1 study: they reopened up
   * less often). Older servers send none: "regular". */
  session: "pre" | "regular" | "after" | "overnight" | "closed";
  released_at: number | null;
  /** Newest first, at most 3. */
  news: PendingNewsWire[];
  offering: boolean;
  resume_at: number | null;
  resume_est: boolean;
  resumed_at: number | null;
  reopen_price: number | null;
  updated: number;
}

export interface PendingSnapshotMessage {
  type: "pending.snapshot";
  enabled: boolean;
  gen: number;
  grace_secs: number;
  band: [number, number];
  cards: PendingCardWire[];
}

export interface PendingUpdateMessage {
  type: "pending.update";
  enabled: boolean;
  prev_gen: number;
  gen: number;
  cards: PendingCardWire[];
  removed: string[];
}

export type PendingMessage = PendingSnapshotMessage | PendingUpdateMessage;
export type PendingClientMessage = { type: "pending.subscribe" } | { type: "pending.unsubscribe" };

export interface PendingState {
  enabled: boolean;
  gen: number;
  graceSecs: number;
  band: [number, number] | null;
  /** In display order: see {@link sortPendingCards}. */
  cards: readonly PendingCardWire[];
  /** A snapshot has arrived on this connection. */
  snapshot: boolean;
  /** A frame has arrived on the connection open now. */
  connected: boolean;
  /** The server has the card (welcome `pending.v1`); false for an older server. */
  available: boolean;
}

export const EMPTY_PENDING: PendingState = { enabled: false, gen: 0, graceSecs: 120, band: null, cards: [], snapshot: false, connected: false, available: true };

const PHASES: readonly PendingPhase[] = ["halted", "news", "resuming", "resumed"];
const num = (v: unknown): number | null => (typeof v === "number" && Number.isFinite(v) ? v : null);
const str = (v: unknown): string | null => (typeof v === "string" ? v : null);

function validNews(v: unknown): PendingNewsWire[] {
  if (!Array.isArray(v)) return [];
  const out: PendingNewsWire[] = [];
  for (const n of v) {
    if (!n || typeof n !== "object") continue;
    const o = n as Record<string, unknown>;
    const id = str(o.id);
    const at = num(o.at);
    if (!id || at === null) continue;
    const source = str(o.source) ?? "";
    out.push({ id, source, label: str(o.label) || pendingSourceLabel(source), headline: str(o.headline), url: safeUrl(str(o.url)), at, seen: num(o.seen) ?? at, kind: o.kind === "offering" ? "offering" : "catalyst" });
  }
  return out.slice(0, 3);
}

/** Only http(s) links are followed (a headline link is the one thing here a click opens). */
function safeUrl(u: string | null): string | null {
  return u && /^https?:\/\//i.test(u) ? u : null;
}

/** One card from the wire, or null when it lacks the fields a card cannot do without. */
export function validPendingCard(v: unknown): PendingCardWire | null {
  if (!v || typeof v !== "object") return null;
  const o = v as Record<string, unknown>;
  const id = str(o.id);
  const sym = str(o.sym);
  const halted_at = num(o.halted_at);
  if (!id || !sym || halted_at === null) return null;
  const news = validNews(o.news);
  return {
    id,
    sym,
    halted_at,
    code: str(o.code) ?? "",
    reason: str(o.reason) || "News pending",
    price: num(o.price),
    prev_close: num(o.prev_close),
    chg_pct: num(o.chg_pct),
    phase: PHASES.includes(o.phase as PendingPhase) ? (o.phase as PendingPhase) : "halted",
    session: SESSIONS.includes(o.session as PendingCardWire["session"]) ? (o.session as PendingCardWire["session"]) : "regular",
    released_at: num(o.released_at),
    news,
    offering: o.offering === true || news.some((n) => n.kind === "offering"),
    resume_at: num(o.resume_at),
    resume_est: o.resume_est === true,
    resumed_at: num(o.resumed_at),
    reopen_price: num(o.reopen_price),
    updated: num(o.updated) ?? halted_at,
  };
}

const SESSIONS: readonly PendingCardWire["session"][] = ["pre", "regular", "after", "overnight", "closed"];

/** The small session tag a card shows, or null in the regular session (and pre-market). */
export function pendingSessionTag(c: PendingCardWire): string | null {
  return c.session === "after" || c.session === "overnight" ? "After hours" : null;
}

const cardsOf = (v: unknown): PendingCardWire[] => (Array.isArray(v) ? v.map(validPendingCard).filter((c): c is PendingCardWire => c !== null) : []);

/**
 * Display order: live halts first (the ones still to act on), the soonest reopen first, then the
 * newest halt; resumed cards last, on their way out.
 */
export function sortPendingCards(cards: readonly PendingCardWire[]): PendingCardWire[] {
  const rank = (c: PendingCardWire) => (c.phase === "resumed" ? 1 : 0);
  return [...cards].sort((a, b) => rank(a) - rank(b) || (a.resume_at ?? Infinity) - (b.resume_at ?? Infinity) || b.halted_at - a.halted_at || a.id.localeCompare(b.id));
}

/**
 * One frame folded in. A snapshot replaces everything; an update replaces its cards by id and
 * drops the removed ids when it continues this state (`prev_gen` = our `gen`), else asks for a
 * snapshot (`resync`).
 */
export function applyPendingMessage(state: PendingState, m: PendingMessage): { state: PendingState; resync: boolean } {
  if (m.type === "pending.snapshot") {
    const band = Array.isArray(m.band) && m.band.length === 2 && m.band.every((x) => typeof x === "number") ? ([m.band[0], m.band[1]] as [number, number]) : state.band;
    return {
      state: { ...state, enabled: m.enabled === true, gen: num(m.gen) ?? 0, graceSecs: num(m.grace_secs) ?? state.graceSecs, band, cards: sortPendingCards(cardsOf(m.cards)), snapshot: true, connected: true },
      resync: false,
    };
  }
  if (!state.snapshot || m.prev_gen !== state.gen) return { state: { ...state, connected: true }, resync: true };
  const removed = new Set(Array.isArray(m.removed) ? m.removed.filter((s): s is string => typeof s === "string") : []);
  const changed = cardsOf(m.cards);
  const byId = new Map(state.cards.filter((c) => !removed.has(c.id)).map((c) => [c.id, c]));
  for (const c of changed) if (!removed.has(c.id)) byId.set(c.id, c);
  return { state: { ...state, enabled: m.enabled === true, gen: num(m.gen) ?? state.gen, cards: sortPendingCards([...byId.values()]), connected: true }, resync: false };
}

export interface PendingChannel {
  get(): PendingState;
  subscribe(fn: () => void): () => void;
}

/** Subscribed on demand like the HOD scanner: the first listener opts in, the last opts out. */
export function createPendingChannel(io: { subscribe(): void; unsubscribe(): void }): PendingChannel & { apply(m: PendingMessage): void; setConnected(open: boolean): void; setAvailable(on: boolean): void } {
  let state: PendingState = EMPTY_PENDING;
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
        state = { ...EMPTY_PENDING, available: state.available };
      };
    },
    apply(m) {
      const next = applyPendingMessage(state, m);
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

/** Cards to show: the server's, less resumed ones whose grace ran out (the server removes them too; this covers a lost update). */
export function visiblePendingCards(s: PendingState, now: number): PendingCardWire[] {
  if (!s.enabled) return [];
  return s.cards.filter((c) => c.phase !== "resumed" || c.resumed_at === null || now < c.resumed_at + s.graceSecs * 1000);
}

/** The panel's item count (0 hides it, like every dashboard panel). */
export function pendingItemCount(s: PendingState, now: number = Date.now()): number {
  return visiblePendingCards(s, now).length;
}

/** "4:32", "1:02:05": a duration in ms, floored to the second, never negative. */
export function fmtPendingClock(ms: number): string {
  const t = Math.max(0, Math.floor(ms / 1000));
  const h = Math.floor(t / 3600);
  const m = Math.floor((t % 3600) / 60);
  const s = t % 60;
  const ss = String(s).padStart(2, "0");
  return h > 0 ? `${h}:${String(m).padStart(2, "0")}:${ss}` : `${m}:${ss}`;
}

/** HH:MM:SS in New York time. */
export function fmtPendingTime(ms: number): string {
  return new Date(ms).toLocaleTimeString("en-US", { hour12: false, timeZone: "America/New_York" });
}

/** HH:MM in New York time. */
export function fmtPendingHm(ms: number): string {
  return new Date(ms).toLocaleTimeString("en-US", { hour12: false, hour: "2-digit", minute: "2-digit", timeZone: "America/New_York" });
}

export function fmtPendingPrice(p: number | null): string {
  if (p === null || !Number.isFinite(p)) return "—";
  return p >= 1 ? p.toFixed(2) : p.toFixed(4);
}

/** "+45.2%" from percent units. */
export function fmtPendingPct(p: number | null | undefined, digits = 1): string {
  if (p === null || p === undefined || !Number.isFinite(p)) return "—";
  return `${p > 0 ? "+" : ""}${p.toFixed(digits)}%`;
}

const WIRES: Readonly<Record<string, string>> = {
  benzinga: "Benzinga",
  globenewswire: "GlobeNewswire",
  prnewswire: "PR Newswire",
  businesswire: "Business Wire",
  accesswire: "ACCESSWIRE",
  newsfile: "Newsfile",
};

/** The source as a reader names it, from the raw "provider/sub" (the server's label is preferred). */
export function pendingSourceLabel(source: string): string {
  const [provider = "", sub = ""] = source.split("/");
  if (provider === "sec_edgar") return sub ? `SEC ${sub}` : "SEC filing";
  if (provider === "alpaca") return WIRES[sub.toLowerCase()] ?? "Benzinga";
  const k = (sub || provider).toLowerCase();
  if (WIRES[k]) return WIRES[k];
  if (provider === "press_wire") return "Press wire";
  return sub || provider || "News";
}

/** What the card says on its status line, by phase (the timers are separate). */
export function pendingStatus(c: PendingCardWire): string {
  switch (c.phase) {
    case "resumed":
      return "Resumed";
    case "resuming":
      return "Resumption set";
    case "news":
      return c.offering ? "Offering filed" : "News out";
    default:
      return c.reason || "News pending";
  }
}

/** The reopen's change from the halt price, percent units. */
export function pendingReopenPct(c: PendingCardWire): number | null {
  return c.reopen_price !== null && c.price !== null && c.price > 0 ? (c.reopen_price / c.price - 1) * 100 : null;
}

/** The card in one sentence for assistive tech. */
export function pendingSentence(c: PendingCardWire, now: number): string {
  const parts = [`${c.sym} halted, ${c.reason || "news pending"}, at ${fmtPendingTime(c.halted_at)}${pendingSessionTag(c) ? `, ${pendingSessionTag(c)!.toLowerCase()}` : ""}`];
  if (c.price !== null) parts.push(`halt price ${fmtPendingPrice(c.price)}${c.chg_pct !== null ? `, ${fmtPendingPct(c.chg_pct)} into the halt` : ""}`);
  if (c.offering) parts.push("offering filed");
  const n = c.news[0];
  if (n) parts.push(`${n.label}: ${n.headline ?? "news item"}`);
  else if (c.released_at !== null) parts.push("the exchange says news is released");
  if (c.phase === "resumed" && c.resumed_at !== null) parts.push(`resumed at ${fmtPendingTime(c.resumed_at)}`);
  else if (c.resume_at !== null) parts.push(`${c.resume_est ? "about " : ""}${fmtPendingClock(c.resume_at - now)} to the reopen at ${fmtPendingHm(c.resume_at)}`);
  else parts.push(`halted for ${fmtPendingClock(now - c.halted_at)}`);
  return parts.join(". ");
}

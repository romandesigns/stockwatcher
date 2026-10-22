// Rocket watch (owner, 2026-10-21; EXPERIMENTAL): the client side of the server's `rocket.*`
// channel (crates/server/src/rocket/, ws.rs) and ONE row/header view model, so the web side
// column and the phone card show an alert identically.
//
// A "rocket" is the research bake-off's event: a +30 % leg within 15 minutes from $0.15–$25. A
// row is a symbol the server's strategy flagged as likely to start one soon; its outcome is filled
// in after the fact by the server (a hit = a rocket started 1–10 minutes after the flag).

/** `pending` until known; `rocketed` = the bake-off's hit; `late` = started 10–30 min after;
 * `moved` = +30 % on a leg that started too early to count; `faded` = 45 min without one. */
export type RocketOutcomeState = "pending" | "rocketed" | "late" | "moved" | "faded";

export interface RocketOutcomeWire {
  state: RocketOutcomeState;
  /** Max gain / drawdown (%) over the 30 minutes after the flag, against the flag price. */
  gain: number | null;
  drawdown: number | null;
  /** Settled at 20:00 ET before its 45 minutes were up. */
  partial: boolean;
}

export interface RocketRowWire {
  id: string;
  s: string;
  /** Flag time, epoch ms (the market clock: a replay's own day). */
  at: number;
  /** Price at the flag, last price now, previous close (dollars; null when unknown). */
  p: number;
  last: number | null;
  pc: number | null;
  score: number;
  reasons: string[];
  outcome: RocketOutcomeWire;
}

export interface RocketStrategyWire {
  id: string;
  version: string;
  /** A stand-in, not a research-selected strategy. */
  placeholder: boolean;
}

export interface RocketDayWire {
  date: string;
  alerts: number;
  hits10: number;
  hits30: number;
}

export interface RocketPanelWire {
  /** The market clock at the frame, epoch ms. */
  t: number | null;
  enabled: boolean;
  strategy: RocketStrategyWire | null;
  mode: "budget" | "fixed";
  budget: number;
  threshold: number | null;
  today: { date: string | null; alerts: number; hits: number; hits30: number; known: number };
  /** The last 20 trading days before today, oldest first. */
  days: RocketDayWire[];
  /** Newest first, at most 50. */
  rows: RocketRowWire[];
}

export interface RocketSnapshotMessage extends RocketPanelWire {
  type: "rocket.snapshot";
}

export interface RocketUpdateMessage extends RocketPanelWire {
  type: "rocket.update";
}

export type RocketMessage = RocketSnapshotMessage | RocketUpdateMessage;

export interface RocketClientMessage {
  type: "rocket.subscribe" | "rocket.unsubscribe";
}

export interface RocketState {
  /** null until the first snapshot; false when the server runs without Rocket watch. */
  enabled: boolean | null;
  panel: RocketPanelWire | null;
  /** Wall-clock ms when the last frame arrived: ages tick between frames from here. */
  receivedAt: number;
  /** A frame arrived on the connection that is open now (ross.ts `RossState.connected`). */
  connected: boolean;
}

export const EMPTY_ROCKET: RocketState = { enabled: null, panel: null, receivedAt: 0, connected: false };

export interface RocketChannel {
  get(): RocketState;
  /** `fn` runs after every frame. Returns the unsubscribe. */
  subscribe(fn: () => void): () => void;
}

/** Subscribed on demand like the Ross channel: the first listener opts in, the last opts out. */
export function createRocketChannel(
  io: { subscribe(): void; unsubscribe(): void },
  now: () => number = Date.now,
): RocketChannel & { apply(m: RocketMessage): void; setConnected(open: boolean): void } {
  let state: RocketState = EMPTY_ROCKET;
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
        if (!listeners.delete(fn)) return;
        if (listeners.size > 0) return;
        io.unsubscribe();
        state = EMPTY_ROCKET;
      };
    },
    apply(m) {
      const { type: _type, ...panel } = m;
      state = { enabled: m.enabled, panel: m.enabled ? panel : null, receivedAt: now(), connected: true };
      emit();
    },
    /** A drop keeps the last rows and marks them not live; a frame on the new socket clears it. */
    setConnected(open: boolean) {
      if (open || !state.connected) return;
      state = { ...state, connected: false };
      emit();
    },
  };
}

/** Semantic colours; each app maps them to its tokens. `good` only ever marks an outcome. */
export type RocketTone = "good" | "accent" | "text" | "muted" | "dim";

export interface RocketRowView {
  key: string;
  symbol: string;
  price: string;
  /** Day gain vs the previous close ("+12.4%"), "" when unknown. */
  gain: string;
  /** "now" under a minute, else "3m", "1h 05m". */
  age: string;
  /** Flagged under a minute ago: the one place the accent is spent. */
  fresh: boolean;
  /** One or two reason chips. */
  reasons: string[];
  /** The score, shown subtly ("2.31"). */
  score: string;
  /** The outcome tag once known ("+34% ✓", "faded"), else null. */
  outcome: { label: string; tone: RocketTone } | null;
  aria: string;
}

const money = (x: number) => (x >= 10 ? x.toFixed(2) : x >= 1 ? x.toFixed(2) : x.toFixed(4).replace(/0{1,2}$/, ""));

function pct(x: number): string {
  const r = Math.round(x * 10) / 10;
  return `${r > 0 ? "+" : ""}${Math.abs(r) >= 100 ? Math.round(r) : r.toFixed(1)}%`;
}

/** A flag's age in market time: "now" under a minute. */
export function rocketAge(ms: number): string {
  if (ms < 60_000) return "now";
  const m = Math.floor(ms / 60_000);
  if (m < 60) return `${m}m`;
  return `${Math.floor(m / 60)}h ${String(m % 60).padStart(2, "0")}m`;
}

function outcomeTag(o: RocketOutcomeWire): RocketRowView["outcome"] {
  const g = o.gain !== null ? `+${Math.max(0, Math.round(o.gain))}%` : "";
  switch (o.state) {
    case "rocketed":
      return { label: `${g} ✓`.trim(), tone: "good" };
    case "late":
      return { label: `${g} late`.trim(), tone: "muted" };
    case "moved":
      return { label: g || "moved", tone: "muted" };
    case "faded":
      return { label: "faded", tone: "dim" };
    default:
      return null;
  }
}

/**
 * One row. `nowMs` is the market clock now: the frame's `t` plus the wall time since it arrived
 * ({@link rocketNow}), so an age keeps ticking between frames.
 */
export function rocketRowView(r: RocketRowWire, nowMs: number): RocketRowView {
  const last = r.last ?? r.p;
  const gain = r.pc && r.pc > 0 ? pct(((last - r.pc) / r.pc) * 100) : "";
  const ageMs = Math.max(0, nowMs - r.at);
  const outcome = outcomeTag(r.outcome);
  const reasons = r.reasons.slice(0, 2);
  const age = rocketAge(ageMs);
  return {
    key: r.id,
    symbol: r.s,
    price: money(last),
    gain,
    age,
    fresh: ageMs < 60_000,
    reasons,
    score: Number.isFinite(r.score) ? r.score.toFixed(2) : "",
    outcome,
    aria: `${r.s} ${money(last)}${gain ? ` ${gain} on the day` : ""}, flagged ${age === "now" ? "just now" : `${age} ago`}${reasons.length ? `: ${reasons.join(", ")}` : ""}${outcome ? `. Outcome ${outcome.label}` : ""}`,
  };
}

/** The market clock now, for ages: the frame's `t` advanced by the wall time since it arrived. */
export function rocketNow(state: RocketState, wallNow: number = Date.now()): number {
  const t = state.panel?.t;
  if (t === null || t === undefined) return wallNow;
  return t + Math.max(0, Math.min(wallNow - state.receivedAt, 5 * 60_000));
}

export interface RocketHeaderView {
  /** "4 of 9 rocketed today", "Nothing flagged today". */
  record: string;
  /** "last 20 days: 31 of 180 (17%)", "" with no history. */
  history: string;
  /** "placeholder-b2 · budget 10/day" — what is running, for the subtle line. */
  strategy: string;
  placeholder: boolean;
}

export function rocketHeaderView(p: RocketPanelWire): RocketHeaderView {
  const n = p.today.alerts;
  const record = n === 0 ? "Nothing flagged today" : `${p.today.hits} of ${n} rocketed today`;
  const a = p.days.reduce((s, d) => s + d.alerts, 0);
  const h = p.days.reduce((s, d) => s + d.hits10, 0);
  const history = a > 0 ? `last ${p.days.length} day${p.days.length === 1 ? "" : "s"}: ${h} of ${a} (${Math.round((h / a) * 100)}%)` : "";
  const s = p.strategy;
  const strategy = s ? `${s.id}${s.version ? ` v${s.version}` : ""} · ${p.mode === "budget" ? `budget ${p.budget}/day` : "fixed threshold"}` : "";
  return { record, history, strategy, placeholder: s?.placeholder ?? false };
}

/** Rows flagged under a minute ago (the tab's count). */
export function rocketFreshCount(state: RocketState, wallNow: number = Date.now()): number {
  const now = rocketNow(state, wallNow);
  return state.panel?.rows.filter((r) => now - r.at < 60_000).length ?? 0;
}

/**
 * The newest flags not seen before, for an optional local notification: ids are remembered in
 * `seen` (the caller keeps the set). The first call only primes it, so opening the app does not
 * announce the day's earlier flags.
 */
export function newRocketFlags(state: RocketState, seen: Set<string>, primed: boolean): RocketRowWire[] {
  const rows = state.panel?.rows ?? [];
  const fresh = primed ? rows.filter((r) => !seen.has(r.id)) : [];
  for (const r of rows) seen.add(r.id);
  return fresh;
}

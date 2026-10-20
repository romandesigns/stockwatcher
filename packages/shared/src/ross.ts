// The Ross Panel (owner, 2026-10-19/20; design canvas "Ross Panel"): the client side of the
// server's `ross.*` channel (protocol.ts, crates/server/src/context.rs) and ONE row view model,
// so the web side panel and the phone card format a member identically.

import type { RossMessage, RossPanelWire, RossRowWire, RossSetupState } from "./protocol.ts";

export interface RossState {
  /** null until the first snapshot; false when the server does not run stock context. */
  enabled: boolean | null;
  panel: RossPanelWire | null;
}

export const EMPTY_ROSS: RossState = { enabled: null, panel: null };

export interface RossChannel {
  get(): RossState;
  /** `fn` runs after every frame. Returns the unsubscribe. */
  subscribe(fn: () => void): () => void;
}

/** Subscribed on demand like the pressure channel: the first listener opts in, the last opts out. */
export function createRossChannel(io: { subscribe(): void; unsubscribe(): void }): RossChannel & { apply(m: RossMessage): void } {
  let state: RossState = EMPTY_ROSS;
  const listeners = new Set<() => void>();
  return {
    get: () => state,
    subscribe(fn) {
      listeners.add(fn);
      if (listeners.size === 1) io.subscribe();
      return () => {
        if (!listeners.delete(fn)) return;
        if (listeners.size > 0) return;
        io.unsubscribe();
        state = EMPTY_ROSS;
      };
    },
    apply(m) {
      state = { enabled: m.enabled, panel: m.panel };
      for (const l of listeners) l();
    },
  };
}

/** Semantic colours; each app maps them to its tokens (good/critical/warning/accent/text/muted/dim). */
export type RossTone = "good" | "critical" | "warning" | "accent" | "text" | "muted" | "dim";

export interface RossRowView {
  symbol: string;
  price: string;
  gain: string;
  /** ▲ / ▼ when the rank moved since the last panel, else "". */
  move: "▲" | "▼" | "";
  moveTone: RossTone;
  badge: { label: "NEW" | "HALTED"; tone: RossTone } | null;
  /** The last minute's move ("+3.0%", "—" when unranked). */
  m1: string;
  m1Tone: RossTone;
  /** Distance below the day's high ("−0.4%"), bright within 1 %. */
  fromHigh: string;
  fromHighTone: RossTone;
  /** Daily-denominator relative volume ("7.4×"), "—" when unknown. */
  rvol: string;
  /** The missing pillar ("Float 30M ✗", "News ?"), else the float when known ("Float 5.0M"). */
  tag: string;
  tagTone: RossTone;
  setup: { state: RossSetupState; label: string; note: string; tone: RossTone; pulse: boolean; attention: boolean; meter: number | null };
  /** Dimmed row (an invalidated setup). */
  dimmed: boolean;
  spark: number[];
  /** Spoken summary for the row's accessible name. */
  aria: string;
}

/** Micro-dollars as the app prints prices: four decimals below $1, two from $1. */
export function rossPrice(micros: number): string {
  const p = micros / 1_000_000;
  return p < 1 ? p.toFixed(4) : p.toFixed(2);
}

function pct(bp: number, digits = 1): string {
  const v = bp / 100;
  const s = Math.abs(v).toFixed(digits);
  return v > 0 ? `+${s}%` : v < 0 ? `−${s}%` : `${(0).toFixed(digits)}%`;
}

function shares(n: number): string {
  return n >= 1_000_000 ? `${(n / 1_000_000).toFixed(n >= 10_000_000 ? 0 : 1)}M` : n >= 1_000 ? `${Math.round(n / 1_000)}K` : String(n);
}

const PILLAR_LABEL: Record<string, string> = { price: "Price", gain: "Gain", rvol: "Volume", float: "Float", news: "News" };

const SETUP: Record<RossSetupState, { label: string; tone: RossTone; pulse: boolean; attention: boolean }> = {
  crossing: { label: "Crossing", tone: "accent", pulse: true, attention: true },
  near: { label: "Near", tone: "warning", pulse: false, attention: true },
  triggered: { label: "Triggered", tone: "good", pulse: false, attention: true },
  forming: { label: "Flag forming", tone: "muted", pulse: false, attention: false },
  invalidated: { label: "Invalidated", tone: "dim", pulse: false, attention: false },
  halted: { label: "Halted", tone: "dim", pulse: false, attention: false },
  watching: { label: "Watching", tone: "dim", pulse: false, attention: false },
};

/** A member admitted within this long before the panel's cutoff is NEW. */
export const ROSS_NEW_NS = 5 * 60 * 1_000_000_000;

/** The row's display, from the engine's row and the panel's cutoff (ns). */
export function rossRowView(r: RossRowWire, cutoff: number, formatTime: (epochNs: number) => string): RossRowView {
  const setupWire = r.setup ?? null;
  const state: RossSetupState = setupWire?.state ?? "watching";
  const look = SETUP[state];
  const m = r.momentum;
  const m1 = m.status === "ready" && m.r_primary_bp !== null ? m.r_primary_bp : null;
  const fromHighBp = r.from_high_bp ?? m.below_high_bp;
  const missing = r.membership.nonpassing[0];
  let tag = "";
  let tagTone: RossTone = "muted";
  if (missing) {
    const [pillar, st] = missing;
    const name = PILLAR_LABEL[pillar] ?? pillar;
    if (st === "FAIL") {
      tag = pillar === "float" && r.float_shares ? `Float ${shares(r.float_shares)} ✗` : `${name} ✗`;
      tagTone = "critical";
    } else {
      tag = `${name} ?`;
      tagTone = "warning";
    }
  } else if (r.float_shares) {
    tag = `Float ${shares(r.float_shares)}`;
  }
  const trigger = setupWire?.trigger?.price ?? null;
  const inval = setupWire?.invalidation?.price ?? null;
  const risk = setupWire?.risk_to_stop_bp ?? null;
  const riskText = risk !== null ? ` · ${(risk / 100).toFixed(1)}% to stop` : "";
  let note = "";
  if (state === "crossing" && trigger !== null) note = `${rossPrice(trigger)} · close pending${riskText}`;
  else if (state === "near" && trigger !== null) note = `${rossPrice(trigger)} · ${((setupWire?.distance_to_trigger_bp ?? 0) / 100).toFixed(1)}% away${riskText}`;
  else if (state === "triggered" && trigger !== null) note = `${rossPrice(trigger)}${setupWire?.state_since ? ` · ${formatTime(setupWire.state_since)}` : ""}`;
  else if (state === "invalidated" && inval !== null) note = `below ${rossPrice(inval)}`;
  const admitted = r.membership.episodes.at(-1)?.admitted_at;
  const badge = state === "halted" ? { label: "HALTED" as const, tone: "warning" as RossTone } : admitted !== undefined && cutoff - admitted <= ROSS_NEW_NS ? { label: "NEW" as const, tone: "accent" as RossTone } : null;
  const move = r.rank !== null && r.prev_rank !== null && r.rank !== r.prev_rank ? (r.rank < r.prev_rank ? "▲" : "▼") : "";
  const view: RossRowView = {
    symbol: r.symbol,
    price: r.price !== null ? rossPrice(r.price) : "—",
    gain: r.gain_bp !== null ? pct(r.gain_bp) : "—",
    move,
    moveTone: move === "▲" ? "good" : move === "▼" ? "critical" : "dim",
    badge,
    m1: m1 !== null ? pct(m1) : "—",
    m1Tone: m1 === null ? "dim" : m1 > 0 ? "good" : m1 < 0 ? "critical" : "muted",
    fromHigh: fromHighBp !== null && fromHighBp !== undefined ? pct(-fromHighBp) : "—",
    fromHighTone: fromHighBp !== null && fromHighBp !== undefined && fromHighBp <= 100 ? "text" : "muted",
    rvol: r.rvol_milli ? `${(r.rvol_milli / 1000).toFixed(1)}×` : "—",
    tag,
    tagTone,
    setup: { state, label: look.label, note, tone: look.tone, pulse: look.pulse, attention: look.attention, meter: state === "watching" || state === "halted" || state === "invalidated" ? null : (setupWire?.meter ?? null) },
    dimmed: state === "invalidated",
    spark: r.spark ?? [],
    aria: "",
  };
  view.aria = `${view.symbol} ${view.price} ${view.gain}, last minute ${view.m1}, ${look.label}${note ? ` ${note}` : ""}${tag ? `, ${tag}` : ""}`;
  return view;
}

/** Rows needing attention (near, crossing, triggered): the panel's count, or derived from the rows. */
export function rossAttention(panel: RossPanelWire): number {
  return panel.attention ?? panel.rows.filter((r) => r.setup && SETUP[r.setup.state].attention).length;
}

/**
 * A fixed synthetic panel for the labelled demo mode (dev builds only, `?rossDemo`) and tests:
 * never shown as live data. The members mirror the design canvas.
 */
export function rossDemoPanel(cutoff: number): RossPanelWire {
  const MIN = 60 * 1_000_000_000;
  const ep = (agoMin: number) => [{ id: "demo", admitted_at: cutoff - agoMin * MIN, retained_since: null, exited_at: null }];
  const mom = (m1: number | null, high: number) => ({ status: m1 === null ? ("unavailable" as const) : ("ready" as const), reason: m1 === null ? "halted" : "ready", r_short_bp: null, r_primary_bp: m1, r_context_bp: null, accel_bp: null, below_high_bp: high });
  const spark = (from: number, to: number, wobble = 0) => Array.from({ length: 20 }, (_, i) => Math.round(from + ((to - from) * i) / 19 + (i % 3 === 1 ? wobble : 0)));
  const row = (o: Partial<RossRowWire> & Pick<RossRowWire, "symbol" | "price" | "gain_bp" | "momentum">): RossRowWire => ({
    membership: { status: "qualified", episodes: ep(20), nonpassing: [] },
    counts: { pass: 5, fail: 0, unknown: 0 },
    rank: null,
    prev_rank: null,
    ...o,
  });
  return {
    revision: 1,
    bar_end: cutoff - 2_000_000_000,
    cutoff,
    attention: 2,
    rows: [
      row({ symbol: "BBBB", price: 6_080_000, gain_bp: 2150, momentum: mom(300, 40), rank: 1, prev_rank: 3, rvol_milli: 7400, float_shares: 30_100_000, membership: { status: "retained", episodes: ep(11), nonpassing: [["float", "FAIL", "at_or_above_max"]] }, counts: { pass: 4, fail: 1, unknown: 0 }, setup: { state: "crossing", trigger: { price: 6_050_000, version: "demo" }, invalidation: { price: 5_860_000, version: "demo" }, meter: 1, risk_to_stop_bp: 362 }, spark: spark(5_700_000, 6_080_000, 20_000) }),
      row({ symbol: "EEEE", price: 2_470_000, gain_bp: 3100, momentum: mom(140, 80), rank: 2, prev_rank: 2, rvol_milli: 11_200, float_shares: 3_100_000, membership: { status: "qualified", episodes: ep(2), nonpassing: [] }, setup: { state: "near", trigger: { price: 2_490_000, version: "demo" }, invalidation: { price: 2_360_000, version: "demo" }, distance_to_trigger_bp: 80, meter: 0.82, risk_to_stop_bp: 530 }, spark: spark(2_380_000, 2_470_000, 8_000) }),
      row({ symbol: "FFFF", price: 421_000, gain_bp: 6400, momentum: mom(90, 120), rank: 3, prev_rank: 3, rvol_milli: 22_000, float_shares: 1_800_000, setup: { state: "forming", trigger: { price: 429_000, version: "demo" }, invalidation: { price: 398_000, version: "demo" }, meter: 0.55 }, spark: spark(405_000, 421_000, 2_000) }),
      row({ symbol: "AAAA", price: 5_680_000, gain_bp: 4200, momentum: mom(-200, 630), rank: 4, prev_rank: 2, rvol_milli: 9100, float_shares: 5_000_000, setup: { state: "forming", trigger: { price: 5_900_000, version: "demo" }, invalidation: { price: 5_500_000, version: "demo" }, meter: 0.35 }, spark: spark(5_900_000, 5_680_000, 30_000) }),
      row({ symbol: "CCCC", price: 3_410_000, gain_bp: 1280, momentum: mom(-260, 980), rank: 5, prev_rank: 4, rvol_milli: 5600, membership: { status: "retained", episodes: ep(30), nonpassing: [["news", "UNKNOWN", "news_coverage_unknown"]] }, counts: { pass: 4, fail: 0, unknown: 1 }, setup: { state: "invalidated", invalidation: { price: 3_440_000, version: "demo" } }, spark: spark(3_550_000, 3_410_000) }),
      row({ symbol: "DDDD", price: 8_120_000, gain_bp: 1520, momentum: mom(null, 210), rvol_milli: 6000, float_shares: 14_000_000, setup: { state: "halted" }, spark: spark(8_050_000, 8_120_000) }),
    ],
  };
}

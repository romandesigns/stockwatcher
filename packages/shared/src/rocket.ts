// Rocket watch (owner, 2026-10-21; EXPERIMENTAL): the client side of the server's `rocket.*`
// channel (crates/server/src/rocket/, ws.rs) and ONE row/header view model, so the web side
// column and the phone card show an alert identically.
//
// A "rocket" is the research bake-off's event: a +30 % leg within 15 minutes from $0.15–$25. A
// row is a symbol the server's strategy flagged as likely to start one soon; its outcome is filled
// in after the fact by the server (a hit = a rocket started 1–10 minutes after the flag).
//
// Since 2026-10-22 every row is also watched for bullish momentum until 20:00 ET (server
// crates/server/src/rocket/track.rs): a state per row, colour-coded in the Ross row language,
// the micro-pullback levels when the stock-context engine runs, the day line, and the later-move
// outcome ("ran later +43%") beside the strict one.
//
// Since 2026-10-22 the channel also carries "buying burst" rows (`kind: "burst"`): a research
// detector (5 s windows + LightGBM) that fires once heavy buying is confirmed on the tape,
// typically 5–9 minutes into a move, about halfway. They are tracked like a flag (`at` = the
// alert, `p` = its price) and shown in their own section at the top of the panel
// ({@link rocketSectionsView}) with their strict 30-minute outcome (+10/20/30 % before −5 %).
//
// And "running" rows (`kind: "running"`, owner 2026-10-22): every stock in $0.10–$25 that IS
// running, by a price-relative rule on the tape (crates/server/src/rocket/running.rs), not a
// forecast. They sit with the flags, in the same momentum order, each labelled with its source
// ({@link RocketRowView.source}: "R2 flag", "Burst", "Running") so an early pick stays
// distinguishable from a move already under way. `p` is the entry price.
//
// And "Run +20%" rows (`kind: "run"`, owner 2026-10-22): the research labeller's +20 % leg in
// $0.10–$25 (crates/server/src/rocket/run.rs, the volume-shelf module's run trigger), entered at
// the mark slot's close. Like running rows they sit with the flags, measure from their entry and
// carry a quiet source chip ("Run +20%").
//
// And "Fast move" rows (`kind: "fast"`, owner 2026-10-22): a stock in $0.10–$25 that jumps +8 %
// in 60 s or +12 % in 15 s at a held price with real trading behind it, read every second
// (crates/server/src/rocket/fast.rs). Entered at the end of the second that fired; like run rows
// they sit with the flags, measure from their entry and carry a quiet chip ("Fast move").

import { rossLinePaths, type RossLineView } from "./ross.ts";
import type { ZoneNoteView, ZonesWire } from "./zones.ts";

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
  /** Later-move outcomes (newer servers): max gain / drawdown over 2 hours, max gain to 20:00 ET. */
  gain_2h?: number | null;
  drawdown_2h?: number | null;
  gain_session?: number | null;
  /** A canonical +30 % mark came any time after the flag that day. */
  rocket_later?: boolean;
}

/** The server's momentum state for a flagged row (crates/server/src/rocket/track.rs). */
export type RocketMomentumState = "watching" | "building" | "breaking_out" | "running" | "pullback" | "fading" | "halted";

/** The micro-pullback (MP5) levels of the stock-context engine, as on a Ross row (prices in micros). */
export interface RocketLevelsWire {
  state: string;
  trigger: number | null;
  invalidation: number | null;
  meter_milli: number | null;
  distance_to_trigger_bp?: number | null;
  to_invalidation_bp?: number | null;
  near_trigger?: boolean | null;
  episode_id?: string | null;
}

/**
 * A burst's strict outcome, 30 minutes after the alert: the highest of +10/20/30 % reached BEFORE
 * −5 %; `stopped` = −5 % came first; `faded` = 30 minutes with neither.
 */
export type BurstOutcomeState = "pending" | "cont30" | "cont20" | "cont10" | "stopped" | "faded";

export interface BurstOutcomeWire {
  state: BurstOutcomeState;
  c10: boolean;
  c20: boolean;
  c30: boolean;
  stopped: boolean;
  /** Max gain % within 30 minutes (so far while pending). */
  mfe: number | null;
  /** Max drawdown % before that high. */
  mae: number | null;
  settled: boolean;
}

/** What the burst detector saw when it fired (a `kind: "burst"` row). */
export interface RocketBurstWire {
  /** Epoch ms: the burst's start (its first gate pass). */
  start: number;
  /** Dollars: the price just before the burst started (null when unknown). */
  start_price: number | null;
  /** % from `start_price` to the alert price (already gained in the burst); null when unknown. */
  gain_pct: number | null;
  /** % over the 30 s before the alert; null when nothing traded 30 s before. */
  move_pct: number | null;
  /** Dollar rate in the last 15 s as a multiple of its 30-min base. */
  rate_x: number;
  /** Prints that set a new day high in the last 15 s. */
  new_highs: number;
  /** Multi-venue up-tick "sweep" seconds in the last 15 s. */
  sweeps: number;
  /** % of the last 15 s volume on up-ticks. */
  buy_pct: number;
  outcome: BurstOutcomeWire;
}

/** One follow-up path on a row ({@link RocketRowWire.also}). */
export interface RocketAlsoWire {
  kind: "run" | "running" | "fast";
  /** When it fired, epoch ms. */
  at: number;
}

export interface RocketRowWire {
  id: string;
  /** Absent = "runner" (older servers). */
  kind?: "runner" | "burst" | "running" | "run" | "fast";
  /**
   * Later paths that fired on this stock after its row was made (owner, 2026-10-23: new
   * evidence, so it notifies, but the stock keeps one row): oldest first, at most one per path.
   * Absent from older servers and on rows without any.
   */
  also?: RocketAlsoWire[];
  /** A burst row's detector read-out and strict outcome. */
  burst?: RocketBurstWire;
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
  /** Momentum (newer servers; absent = watching). */
  state?: RocketMomentumState;
  /** Epoch ms the state took effect. */
  state_since?: number;
  /** A breakout seen in the bar still forming (or a halt not yet at a barrier). */
  provisional?: boolean;
  from_flag_pct?: number | null;
  /** ≤ 0: below the high since the flag. */
  from_high_pct?: number | null;
  move_1m_pct?: number | null;
  pullback_depth_pct?: number | null;
  levels?: RocketLevelsWire | null;
  /** The day line (crates/server/src/day_line.rs): 30 s closes in micros from `day_line_from`. */
  day_line_start?: number;
  day_line_step_s?: number;
  day_line_from?: number;
  day_line?: number[];
  prev_close?: number;
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
  today: { date: string | null; alerts: number; hits: number; hits30: number; known: number; ran_later?: number };
  /** The last 20 trading days before today, oldest first. */
  days: RocketDayWire[];
  /** Newest first, at most 80: past that the server keeps the active rows, then the newest. */
  rows: RocketRowWire[];
  /** The buying-burst detector (newer servers). `today` above counts Runners flags only. */
  burst?: RocketBurstPanelWire;
  /**
   * Volume shelves by symbol (newer servers; zones.ts). Untrusted wire data: read it through
   * `zonesFor`, which drops anything malformed.
   */
  zones?: ZonesWire;
}

export interface RocketBurstPanelWire {
  enabled: boolean;
  /** The model probability threshold. */
  threshold: number;
  /** The target alerts/day of that threshold. */
  per_day: number;
  /** Burst alerts today. */
  today: number;
  /** The research's typical outcome from the alert (35 unseen days). */
  typical: { mfe30_med_pct: number; mae_med_pct: number; cont10_pct: number; cont20_pct: number; fires_after_min: [number, number] };
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

/**
 * Applies a frame's day-line slices to the lines already held (the `mergeRossDayLines` rule,
 * keyed by the row's symbol `s`), so every row holds its WHOLE line with `day_line_from: 0`.
 */
export function mergeRocketDayLines(prev: RocketPanelWire | null, next: RocketPanelWire): { panel: RocketPanelWire; resync: boolean } {
  if (!next.rows.some((r) => r.day_line && (r.day_line_from ?? 0) > 0)) return { panel: next, resync: false };
  let resync = false;
  const held = new Map(prev?.rows.map((r) => [r.s, r]) ?? []);
  const rows = next.rows.map((r) => {
    const from = r.day_line_from ?? 0;
    if (!r.day_line || from === 0) return r;
    const old = held.get(r.s);
    if (old?.day_line && old.day_line_start === r.day_line_start && old.day_line.length >= from) {
      return { ...r, day_line: old.day_line.slice(0, from).concat(r.day_line), day_line_from: 0 };
    }
    resync = true;
    const { day_line: _line, day_line_from: _from, day_line_start: _start, ...rest } = r;
    return rest;
  });
  return { panel: { ...next, rows }, resync };
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
      const { type: _type, ...next } = m;
      const merged = m.enabled ? mergeRocketDayLines(state.panel, next) : { panel: null, resync: false };
      state = { enabled: m.enabled, panel: merged.panel, receivedAt: now(), connected: true };
      // A slice this client cannot place: the server answers every `rocket.subscribe` with a
      // whole snapshot, so ask again rather than draw a broken line.
      if (merged.resync && listeners.size > 0) io.subscribe();
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

/**
 * Semantic colours; each app maps them to its tokens. `good` only ever marks a strict outcome,
 * `loss` only a burst stopped out (−5 % first).
 */
export type RocketTone = "good" | "loss" | "warning" | "accent" | "text" | "muted" | "dim";

/** How a momentum state looks (the Ross row language): label, tone, pulse, inset edge. */
export interface RocketStateLook {
  label: string;
  tone: RocketTone;
  /** A breathing dot (still with reduced motion). */
  pulse: boolean;
  /** The inset edge bar's tone, or null for none (never a border). */
  edge: RocketTone | null;
  /** Counted in the header's attention count. */
  attention: boolean;
  /** The whole row dimmed. */
  dimmed: boolean;
}

/**
 * The momentum states (owner, 2026-10-22). Accent is spent only on a breakout (pulsing) and a
 * run; amber on building pressure and on a halt; a pullback is muted with its depth and trigger
 * meter; a fade dims the row. Never a success green: that stays the strict outcome's.
 */
export const ROCKET_STATES: Record<RocketMomentumState, RocketStateLook> = {
  breaking_out: { label: "Breaking out", tone: "accent", pulse: true, edge: "accent", attention: true, dimmed: false },
  running: { label: "Running", tone: "accent", pulse: false, edge: "accent", attention: false, dimmed: false },
  halted: { label: "Halted", tone: "warning", pulse: false, edge: null, attention: false, dimmed: false },
  building: { label: "Building", tone: "warning", pulse: false, edge: "warning", attention: true, dimmed: false },
  pullback: { label: "Pullback", tone: "muted", pulse: false, edge: null, attention: false, dimmed: false },
  watching: { label: "Watching", tone: "muted", pulse: false, edge: null, attention: false, dimmed: false },
  fading: { label: "Fading", tone: "dim", pulse: false, edge: null, attention: false, dimmed: true },
};

/** Row order by state: what is moving first, what is fading last. */
export const ROCKET_STATE_ORDER: RocketMomentumState[] = ["breaking_out", "running", "halted", "building", "pullback", "watching", "fading"];

function momentumOf(r: RocketRowWire): RocketMomentumState {
  return r.state && r.state in ROCKET_STATES ? r.state : "watching";
}

export interface RocketRowView {
  key: string;
  symbol: string;
  /**
   * The symbol's volume-shelf note (zones.ts: `withZoneSections` over {@link rocketSectionsView},
   * which the panels use); absent or null when it has none to show.
   */
  zone?: ZoneNoteView | null;
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
  /** The outcome tag once known ("+34% ✓", "ran later +43%", "faded"), else null. */
  outcome: { label: string; tone: RocketTone } | null;
  /** The momentum state: label, look, since when, and its note (pullback depth, trigger). */
  momentum: RocketStateLook & { state: RocketMomentumState; since: string; provisional: boolean; note: string };
  /** The halted badge (amber). */
  halted: boolean;
  /** From the flag price, off the high since the flag, the last minute ("+12.4%", "—"). */
  fromFlag: string;
  fromFlagTone: RocketTone;
  fromHigh: string;
  fromHighTone: RocketTone;
  move1m: string;
  move1mTone: RocketTone;
  /** The trigger meter (0–1) in a pullback with micro-pullback levels, else null. */
  meter: number | null;
  /** The row chart: the day line with the live price as its end; null with < 2 points. */
  line: RossLineView | null;
  /** A buying-burst row's own lines, null for a Runners flag. */
  burst: RocketBurstView | null;
  /** Where the row came from: the strategy's flag ("R2 flag"), a buying burst, a running entry or a +20 % run ("Run +20%"). */
  source: { kind: "runner" | "burst" | "running" | "run" | "fast"; label: string };
  /**
   * Secondary source chips: later paths that fired on this stock after its row was made
   * ("+ Run +20% · 3m"), oldest first. Quiet, never accent: the row's own source leads.
   */
  also: { kind: RocketAlsoWire["kind"]; label: string; age: string }[];
  /**
   * The MP5 pullback setup on this stock (owner, 2026-10-25, "Pullback setting up"): "Pullback ·
   * trigger $3.42" while armed, "Pullback · crossing $3.42" while the trigger is being crossed,
   * else null. From the row's `levels`, which exist only once the episode has armed.
   */
  setup: { label: string; crossing: boolean } | null;
  /** What `fromFlag` is measured from: "flag", "alert" (a burst) or "entry" (a running or run row). */
  fromWhat: "flag" | "alert" | "entry";
  aria: string;
}

/** A running-entry row (owner, 2026-10-22). */
export const isRunningRow = (r: RocketRowWire): boolean => r.kind === "running";

/** A "Run +20%" row (owner, 2026-10-22). */
export const isRunRow = (r: RocketRowWire): boolean => r.kind === "run";

/** A "Fast move" row (owner, 2026-10-22). */
export const isFastRow = (r: RocketRowWire): boolean => r.kind === "fast";

/** A row entered on a move already under way (a running entry, a +20 % run or a fast move): measured from its entry. */
export const isEntryRow = (r: RocketRowWire): boolean => isRunningRow(r) || isRunRow(r) || isFastRow(r);

/** The source chip of a row that is not a burst. */
function sourceOf(r: RocketRowWire, strategyId: string | undefined | null): RocketRowView["source"] {
  if (isRunningRow(r)) return { kind: "running", label: "Running" };
  if (isRunRow(r)) return { kind: "run", label: "Run +20%" };
  if (isFastRow(r)) return { kind: "fast", label: "Fast move" };
  return { kind: "runner", label: flagLabel(strategyId) };
}

const ALSO_LABEL: Record<RocketAlsoWire["kind"], string> = { running: "Running", run: "Run +20%", fast: "Fast move" };

/**
 * The setup chip ({@link RocketRowView.setup}). The cue's states (stock_context `CueState`):
 * "forming" with a trigger (armed, not yet near) and "near" are the armed setup; "crossing" is the
 * trigger being crossed. Triggered, invalidated, halted and watching show no chip: the push
 * already told of the crossing, and a broken setup is not news on the row.
 */
export function rocketSetupView(lv: RocketLevelsWire | null | undefined): RocketRowView["setup"] {
  if (!lv || lv.trigger == null || !(lv.trigger > 0)) return null;
  const at = `$${money(lv.trigger / 1_000_000)}`;
  if (lv.state === "crossing") return { label: `Pullback · crossing ${at}`, crossing: true };
  if (lv.state === "forming" || lv.state === "near") return { label: `Pullback · trigger ${at}`, crossing: false };
  return null;
}

/** A row's follow-up chips ({@link RocketRowView.also}): known paths only, one per path, oldest first. */
export function rocketAlsoView(r: Pick<RocketRowWire, "also">, nowMs: number): RocketRowView["also"] {
  const seen = new Set<string>();
  return (Array.isArray(r.also) ? r.also : [])
    .filter((a) => a && typeof a === "object" && a.kind in ALSO_LABEL && Number.isFinite(a.at))
    .sort((x, y) => x.at - y.at)
    .filter((a) => (seen.has(a.kind) ? false : (seen.add(a.kind), true)))
    .map((a) => ({ kind: a.kind, label: ALSO_LABEL[a.kind], age: rocketAge(Math.max(0, nowMs - a.at)) }));
}

/** "R2 flag" for `precursors-R2`, "R0 flag" for `precursors-R0`, else "Flag". */
export function flagLabel(strategyId: string | undefined | null): string {
  const m = strategyId ? /-R(\d+)$/i.exec(strategyId) : null;
  return m ? `R${m[1]} flag` : "Flag";
}

export interface RocketBurstView {
  /** "Buying burst". */
  headline: string;
  /** "+18.0% since burst start · 6m ago". */
  since: string;
  /** "+4.2% last 30 s". */
  move: string;
  /** Pending and alerted under {@link BURST_LIVE_MS} ago: the row's accent marker. */
  live: boolean;
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

/**
 * The outcome tag (owner, 2026-10-22): "+34% ✓" when it rocketed in the strict window (the only
 * green); else "ran later +43%" once the strict outcome is known and the 2-hour max gain reached
 * +15 % (neutral text: a later move, not the strict success); else the strict tags.
 */
function outcomeTag(o: RocketOutcomeWire): RocketRowView["outcome"] {
  const g = o.gain !== null ? `+${Math.max(0, Math.round(o.gain))}%` : "";
  if (o.state !== "rocketed" && o.state !== "pending" && o.gain_2h != null && o.gain_2h >= RAN_LATER_PCT) {
    return { label: `ran later +${Math.round(o.gain_2h)}%`, tone: "text" };
  }
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

/** "Ran later": the 2-hour max gain at least this (percent; the server's `RAN_LATER_PCT`). */
export const RAN_LATER_PCT = 15;

const signed = (x: number | null | undefined): string => (x === null || x === undefined || !Number.isFinite(x) ? "—" : pct(x).replace("-", "−"));
/** Positive moves in plain text, the rest muted: never green (that is the outcome's). */
const moveTone = (x: number | null | undefined): RocketTone => (x === null || x === undefined ? "dim" : x > 0 ? "text" : "muted");

/** The row chart's line: the day's 30 s closes, then the live price (micros), drawn neutral. */
export function rocketLine(r: RocketRowWire): RossLineView | null {
  const day = r.day_line;
  if (!day || day.length === 0) return null;
  const live = r.last ?? null;
  const values = live !== null ? [...day, Math.round(live * 1_000_000)] : day.slice();
  if (values.length < 2) return null;
  return { kind: "day", values, prevClose: r.prev_close ?? null, tone: values[values.length - 1]! >= values[0]! ? "good" : "critical" };
}

export { rossLinePaths as rocketLinePaths };

/** A burst row's marker stays accent this long after the alert while its outcome is pending. */
export const BURST_LIVE_MS = 5 * 60_000;

/** A buying-burst row (absent `kind` = a Runners flag, from older servers too). */
export const isBurstRow = (r: RocketRowWire): r is RocketRowWire & { burst: RocketBurstWire } => r.kind === "burst" && r.burst !== undefined;

/**
 * A burst's strict outcome tag: "+20% before −5% ✓" (the only green), "−5% first" (loss),
 * "faded" (dim); null while pending (the row is still live).
 */
function burstOutcomeTag(o: BurstOutcomeWire): RocketRowView["outcome"] {
  switch (o.state) {
    case "cont30":
      return { label: "+30% before −5% ✓", tone: "good" };
    case "cont20":
      return { label: "+20% before −5% ✓", tone: "good" };
    case "cont10":
      return { label: "+10% before −5% ✓", tone: "good" };
    case "stopped":
      return { label: "−5% first", tone: "loss" };
    case "faded":
      return { label: o.mfe != null ? `faded · max +${Math.max(0, Math.round(o.mfe))}%` : "faded", tone: "dim" };
    default:
      return null;
  }
}

/** "about halfway in": no "buy" wording, only what the tape showed and how long ago. */
function burstView(b: RocketBurstWire, at: number, nowMs: number): RocketBurstView {
  const ago = rocketAge(Math.max(0, nowMs - b.start));
  return {
    headline: "Buying burst",
    since: `${b.gain_pct != null && Number.isFinite(b.gain_pct) ? `${pct(b.gain_pct)} since burst start` : "Burst started"} · ${ago === "now" ? "just now" : `${ago} ago`}`,
    move: b.move_pct != null && Number.isFinite(b.move_pct) ? `${pct(b.move_pct)} last 30 s` : "",
    live: b.outcome.state === "pending" && nowMs - at < BURST_LIVE_MS,
  };
}

/**
 * One row. `nowMs` is the market clock now: the frame's `t` plus the wall time since it arrived
 * ({@link rocketNow}), so an age keeps ticking between frames.
 */
export function rocketRowView(r: RocketRowWire, nowMs: number, strategyId?: string | null): RocketRowView {
  const last = r.last ?? r.p;
  const gain = r.pc && r.pc > 0 ? pct(((last - r.pc) / r.pc) * 100) : "";
  const ageMs = Math.max(0, nowMs - r.at);
  const b = isBurstRow(r) ? r.burst : null;
  const outcome = b ? burstOutcomeTag(b.outcome) : outcomeTag(r.outcome);
  const reasons = r.reasons.slice(0, 2);
  const age = rocketAge(ageMs);
  const state = momentumOf(r);
  const look = ROCKET_STATES[state];
  const lv = r.levels ?? null;
  let note = "";
  if (state === "pullback") {
    const depth = r.pullback_depth_pct ?? (r.from_high_pct != null ? -r.from_high_pct : null);
    const parts = [depth != null ? `−${Math.abs(depth).toFixed(1)}% off high` : ""];
    if (lv?.trigger != null) parts.push(`trigger ${money(lv.trigger / 1_000_000)}${lv.distance_to_trigger_bp != null && lv.distance_to_trigger_bp > 0 ? ` · ${(lv.distance_to_trigger_bp / 100).toFixed(1)}% away` : ""}`);
    note = parts.filter(Boolean).join(" · ");
  } else if (state === "running" && r.from_flag_pct != null) {
    note = `${pct(r.from_flag_pct)} from ${isEntryRow(r) ? "entry" : "flag"}`;
  }
  const since = r.state_since !== undefined && state !== "watching" ? rocketAge(Math.max(0, nowMs - r.state_since)) : "";
  const meter = state === "pullback" && lv?.meter_milli != null ? lv.meter_milli / 1000 : null;
  const view: RocketRowView = {
    key: r.id,
    symbol: r.s,
    price: money(last),
    gain,
    age,
    fresh: ageMs < 60_000,
    reasons,
    score: Number.isFinite(r.score) ? r.score.toFixed(2) : "",
    outcome,
    momentum: { ...look, state, since, provisional: r.provisional ?? false, note },
    halted: state === "halted",
    fromFlag: signed(r.from_flag_pct),
    fromFlagTone: moveTone(r.from_flag_pct),
    fromHigh: signed(r.from_high_pct),
    fromHighTone: r.from_high_pct != null && r.from_high_pct > -1 ? "text" : "muted",
    move1m: signed(r.move_1m_pct),
    move1mTone: moveTone(r.move_1m_pct),
    meter,
    line: rocketLine(r),
    burst: b ? burstView(b, r.at, nowMs) : null,
    source: b ? { kind: "burst", label: "Burst" } : sourceOf(r, strategyId),
    also: rocketAlsoView(r, nowMs),
    setup: rocketSetupView(lv),
    fromWhat: b ? "alert" : isEntryRow(r) ? "entry" : "flag",
    aria: "",
  };
  const alsoAria = (view.also.length ? `. Then ${view.also.map((a) => `${a.label} ${a.age === "now" ? "just now" : `${a.age} ago`}`).join(", ")}` : "") + (view.setup ? `. ${view.setup.label}` : "");
  if (view.burst) {
    const bv = view.burst;
    view.aria = `${r.s} ${money(last)}${gain ? ` ${gain} on the day` : ""}. ${bv.headline} alerted ${age === "now" ? "just now" : `${age} ago`}, ${bv.since}, ${bv.move}, ${look.label}${note ? ` ${note}` : ""}${reasons.length ? `: ${reasons.join(", ")}` : ""}${outcome ? `. Outcome ${outcome.label}` : ""}${alsoAria}`;
    return view;
  }
  const how = isRunningRow(r) ? "running, entered" : isRunRow(r) ? "run +20%, entered" : isFastRow(r) ? "fast move, entered" : `${view.source.label}, flagged`;
  view.aria = `${r.s} ${money(last)}${gain ? ` ${gain} on the day` : ""}, ${how} ${age === "now" ? "just now" : `${age} ago`}, ${look.label}${view.momentum.provisional ? " (provisional)" : ""}${note ? ` ${note}` : ""}, ${view.fromFlag} from the ${view.fromWhat}${reasons.length ? `: ${reasons.join(", ")}` : ""}${outcome ? `. Outcome ${outcome.label}` : ""}${alsoAria}`;
  return view;
}

/**
 * The rows in panel order, strongest first: by momentum state (breaking out and running first,
 * fading last, dimmed), newest first within one. Flags, running entries and +20 % runs share
 * the order.
 */
export function rocketRowsView(p: RocketPanelWire, nowMs: number): RocketRowView[] {
  const rank = (s: RocketMomentumState) => ROCKET_STATE_ORDER.indexOf(s);
  return p.rows
    .map((r, i) => ({ v: rocketRowView(r, nowMs, p.strategy?.id), i }))
    .sort((a, b) => rank(a.v.momentum.state) - rank(b.v.momentum.state) || a.i - b.i)
    .map((x) => x.v);
}

export interface RocketBurstSectionView {
  /** "3 today". */
  count: string;
  /** "Heavy buying confirmed, typically 5–9 min into a move (about halfway)". */
  what: string;
  /** "Typical from here: +13% max in 30 min (median), 1 in 4 reach +20% before −5%"; "" unknown. */
  typical: string;
}

/** "1 in 4" for 23 %: the odds in words. */
function oneIn(p: number): string {
  return p > 0 ? `1 in ${Math.max(1, Math.round(100 / p))}` : "none";
}

/** The Bursts section's header, from the panel's detector block (older servers: the row count). */
export function rocketBurstSectionView(b: RocketBurstPanelWire | undefined, rows: number): RocketBurstSectionView {
  const t = b?.typical;
  const [lo, hi] = t?.fires_after_min ?? [5, 9];
  return {
    count: `${b?.today ?? rows} today`,
    what: `Heavy buying confirmed, typically ${lo}–${hi} min into a move (about halfway)`,
    typical: t ? `Typical from here: +${Math.round(t.mfe30_med_pct)}% max in 30 min (median), ${oneIn(t.cont20_pct)} reach +20% before −5%` : "",
  };
}

export interface RocketSectionsView {
  /** The Bursts section (top), newest first; null when the server has no burst detector and sent none. */
  bursts: { head: RocketBurstSectionView; rows: RocketRowView[] } | null;
  /** The Runners flags below, in momentum order ({@link rocketRowsView}). */
  runners: RocketRowView[];
}

/** The panel split in two: buying bursts first (newest alert first), then the Runners flags. */
export function rocketSectionsView(p: RocketPanelWire, nowMs: number): RocketSectionsView {
  const burstRows = p.rows.filter(isBurstRow);
  const runners = rocketRowsView({ ...p, rows: p.rows.filter((r) => !isBurstRow(r)) }, nowMs);
  if (burstRows.length === 0 && p.burst?.enabled !== true) return { bursts: null, runners };
  const rows = burstRows
    .map((r, i) => ({ r, i }))
    .sort((a, b) => b.r.at - a.r.at || a.i - b.i)
    .map((x) => rocketRowView(x.r, nowMs, p.strategy?.id));
  return { bursts: { head: rocketBurstSectionView(p.burst, burstRows.length), rows }, runners };
}

/** Rows that need attention now: building or breaking out (the header count). */
export function rocketAttention(p: RocketPanelWire, current = true): number {
  return current ? p.rows.filter((r) => ROCKET_STATES[momentumOf(r)].attention).length : 0;
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
  /** "2 ran ≥ +15% later", "" when none (or an older server). */
  ranLater: string;
  /** Rows building or breaking out now. */
  attention: number;
  /** "last 20 days: 31 of 180 (17%)", "" with no history. */
  history: string;
  /** "placeholder-b2 · budget 10/day" — what is running, for the subtle line. */
  strategy: string;
  placeholder: boolean;
}

export function rocketHeaderView(p: RocketPanelWire, current = true): RocketHeaderView {
  const n = p.today.alerts;
  const record = n === 0 ? "Nothing flagged today" : `${p.today.hits} of ${n} rocketed today`;
  const a = p.days.reduce((s, d) => s + d.alerts, 0);
  const h = p.days.reduce((s, d) => s + d.hits10, 0);
  const history = a > 0 ? `last ${p.days.length} day${p.days.length === 1 ? "" : "s"}: ${h} of ${a} (${Math.round((h / a) * 100)}%)` : "";
  const s = p.strategy;
  const strategy = s ? `${s.id}${s.version ? ` v${s.version}` : ""} · ${p.mode === "budget" ? `budget ${p.budget}/day` : "fixed threshold"}` : "";
  const k = p.today.ran_later ?? 0;
  const ranLater = k > 0 ? `${k} ran ≥ +${RAN_LATER_PCT}% later` : "";
  return { record, ranLater, attention: rocketAttention(p, current), history, strategy, placeholder: s?.placeholder ?? false };
}

/** Rows flagged under a minute ago (the tab's count). */
export function rocketFreshCount(state: RocketState, wallNow: number = Date.now()): number {
  if (!state.connected) return 0;
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

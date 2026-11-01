// Wire types shared by web, desktop and mobile. The Rust side mirrors these
// in crates/server/src/{api,ws,market}.rs; change both together.
import { isDetectionMessage } from "./detection.ts";
import { parseRunnersNotice } from "./runnersNotify.ts";

/** GET /api/health (never requires auth). */
export interface Health {
  status: "ok";
  version: string;
  commit: string;
}

/** GET /api/version (requires auth when the server has a token). */
export interface VersionInfo {
  version: string;
  commit: string;
}

// --- WebSocket /ws ---------------------------------------------------------
// Browsers cannot set headers on a WebSocket, so the token travels in the
// first message (`hello`), never in the URL.

/**
 * Which of the server's two markets a connection reads (owner, 2026-10-18): `live` is the main
 * page's (the dashboard), `replay` the practice feature's (the chart page). The server runs both
 * side by side; a client that names neither gets the server's `MARKET_DATA_MODE`. The REST market
 * routes take the same value as `?source=`.
 */
export type MarketSource = "live" | "replay";

/** `path` with `?source=` (or `&source=`) appended, for the REST market routes. */
export function withMarketSource(path: string, source: MarketSource | undefined): string {
  return source ? `${path}${path.includes("?") ? "&" : "?"}source=${source}` : path;
}

export interface HelloMessage {
  type: "hello";
  token?: string;
  /** Free-form client identifier for server logs, e.g. "web". */
  client?: string;
  /** The market this connection reads for its whole life; absent = the server's default. */
  source?: MarketSource;
}

export interface WelcomeMessage {
  type: "welcome";
  version: string;
  commit: string;
  heartbeatSecs: number;
  /**
   * What the server does beyond the base protocol, for the client to opt into. Absent from
   * servers older than the field: treat that as empty. Known: {@link INDICATORS_FEATURE},
   * `SYMBOLS_FEATURE` (symbols.ts), `NOTICES_FEATURE` (noticeFeed.ts).
   */
  features?: string[];
}

/**
 * The server computes chart indicators (crates/server/src/indicators.rs): a client that sees it
 * may send its slot's `indicators` and `rth` on `chart.subscribe` (and `chart.indicators.set`
 * when they change) and draw the pushed values (chart-core types.ts `ChartIndicatorsSnapshot`,
 * `ChartIndicatorIncrement`) instead of computing them. Without it, keep the local math.
 */
export const INDICATORS_FEATURE = "indicators.v1";

/** Whether a welcome advertises `feature` (false for a server older than `features`). */
export function hasFeature(welcome: Pick<WelcomeMessage, "features">, feature: string): boolean {
  return Array.isArray(welcome.features) && welcome.features.includes(feature);
}

/** Either side may ping; the other answers with a pong echoing `ts`. */
export interface PingMessage {
  type: "ping";
  ts: number;
}

export interface PongMessage {
  type: "pong";
  ts: number;
}

/**
 * Error codes. The handshake ones are acted on here (ws.ts); the rest answer
 * chart/replay/paper commands and are listed so a client can tell them apart:
 * `not_replay` (a replay.control in live mode), `unknown_symbol` /
 * `outside_price_band` (chart.subscribe, and the resync after a replay day
 * switch when the new day lacks the symbol), `unknown_day` (replay.control
 * `day` not in GET /api/replay/state's `days`) and `day_unavailable` (offered,
 * but the server could not open it); after either day error nothing changed.
 * The paper broker adds `bad_order` and its own refusal codes (`not_open`, …;
 * docs/architecture/chart-trading-sim.md). `too_many_charts`: a chart.subscribe
 * for a new slot while the connection already holds MAX_CHARTS (4).
 */
export type ErrorCode =
  | "unauthorized"
  | "bad_request"
  | "hello_timeout"
  | "shutting_down"
  | "not_replay"
  | "replay_unavailable"
  | "unknown_symbol"
  | "outside_price_band"
  | "unknown_day"
  | "day_unavailable"
  | "too_many_charts"
  | (string & {});

export interface ErrorMessage {
  type: "error";
  code: ErrorCode;
  message: string;
  /** Chart errors (`unknown_symbol`, `outside_price_band`): the symbol refused. */
  symbol?: string;
  /** Chart errors for a named slot: which chart (chart.subscribe `id`). */
  id?: string;
}

// --- Quote board (after `welcome`; crates/server/src/ws.rs, crates/server/src/quotes.rs) -----
// The ticker search's live rows. Push, not poll: the client opts in with
// `quotes.subscribe`, gets one `quotes.snapshot` at once, then a
// `quotes.update` with only the changed rows at most once per second of
// wall time. Only price-band candidates are ever sent; `gone` lists those
// that left the band. A seek or a lagged connection gets a fresh snapshot,
// which REPLACES the client's board. Subscribing again after a reconnect
// is the client's job (createWsClient does it).

/**
 * One row, compact keys (a snapshot carries thousands). Prices are dollars.
 * Change % = (p - o) / o.
 */
export interface QuoteRowWire {
  /** Symbol. */
  s: string;
  /** Last price (OHLC-qualifying prints only, the candles' rule). */
  p: number;
  /**
   * The previous trading day's official close: the change-% reference
   * whenever present. Static for the day, so it is sent in snapshots, for rows
   * entering the band, and in an update only when it changes (the row's
   * trading date moved on); a client keeps the last one it got for the row.
   */
  pc?: number;
  /**
   * How `pc` relates to the closes file (crates/server/src/quotes.rs `PcBasis`): `split` = put on
   * today's share basis by a known split; `guard` = withheld because the day's first print is a
   * split factor away from it and no split is known (measure against `o`, and drop any `pc` held
   * for the row). Absent for a plain close.
   */
  pcb?: "split" | "guard";
  /** Fallback change reference when there is no `pc`: the regular open, or the day's first price while `pre` is set. */
  o: number;
  /** Day volume (shares) and prints counted, by the candles' volume rule. */
  v: number;
  n: number;
  /** Present (true) only while the symbol has no print at/after 09:30 yet (it describes `o`). */
  pre?: true;
  /** Company name and listing exchange (e.g. "NASDAQ"): snapshots, and update rows entering the band; absent when the server has no metadata. */
  name?: string;
  x?: string;
}

export interface QuotesSnapshotMessage {
  type: "quotes.snapshot";
  /** Replay/live clock (epoch ms) when the rows were taken; null before any clock. */
  t: number | null;
  quotes: QuoteRowWire[];
}

export interface QuotesUpdateMessage {
  type: "quotes.update";
  t: number | null;
  quotes: QuoteRowWire[];
  gone?: string[];
}

export type QuotesMessage = QuotesSnapshotMessage | QuotesUpdateMessage;

/**
 * One movers row (crates/server/src/movers.rs): Top Gainers, Hot stocks Bull and Bear are ranked
 * on the server once a tick and pushed as these. Prices are micros.
 */
export interface MoverRowWire {
  symbol: string;
  /** Last price (micros), the quote board's. */
  price: number;
  /** Previous close (micros), on today's share basis; absent when unknown or withheld (`pcb`). */
  pc?: number;
  /** `split`: `pc` was split-adjusted; `guard`: the close was withheld (the day trades on another basis). */
  pcb?: "split" | "guard";
  /** The fallback reference (micros): the regular open, else the day's first price. */
  ref: number;
  /** Change %, against `pc`, else `ref` (3 decimals). */
  chg: number;
  /** Dollar volume (last price × day volume), whole dollars. */
  dv: number;
  volume: number;
  trades: number;
  halted?: boolean;
  /** The day line, as `ross.*` rows carry it (crates/server/src/day_line.rs): whole in a snapshot, a slice in an update. */
  day_line_start?: number;
  day_line_step_s?: number;
  day_line_from?: number;
  day_line?: number[];
  prev_close?: number;
}

export interface MoversSnapshotMessage {
  type: "movers.snapshot";
  /** The market clock, epoch ms. */
  t: number | null;
  /** Symbols in rank order, at most 25 each. */
  gainers: string[];
  bull: string[];
  bear: string[];
  /** One row per symbol in any list. */
  rows: MoverRowWire[];
}

/** Only what changed: a list only when it changed (whole), rows that changed, entered or gained day-line points, `gone` symbols in no list. */
export interface MoversUpdateMessage {
  type: "movers.update";
  t: number | null;
  gainers?: string[];
  bull?: string[];
  bear?: string[];
  rows: MoverRowWire[];
  gone?: string[];
}

export type MoversMessage = MoversSnapshotMessage | MoversUpdateMessage;

export type MoversClientMessage = { type: "movers.subscribe" } | { type: "movers.unsubscribe" };

export interface QuotesSubscribeMessage {
  type: "quotes.subscribe";
}

export interface QuotesUnsubscribeMessage {
  type: "quotes.unsubscribe";
}

// --- Halt watcher (after `welcome`; crates/server/src/pressure.rs, ws.rs) -----------------
// The dashboard's build-ups (owner, 2026-10-20): the symbols whose one-sided pressure is rising
// fast while the price runs at its LULD band, ranked on the server once per second with
// hysteresis. At most 6, usually fewer, often none (an empty board is the normal state). Opt in
// with `pressure.subscribe`: a `pressure.snapshot` at once, then a `pressure.update` whenever the
// board changed. Both carry the WHOLE board in display order and replace the client's (six cards
// at most: no patching). A seek or a lag gets a fresh snapshot; resubscribing after a reconnect is
// the client's job (createWsClient does it).

/** A halt in progress. */
export interface PressureHaltWire {
  /** Epoch ms of the halt. */
  start: number;
  /** Provider reason, e.g. "LUDP" (a LULD trading pause, normally 5 minutes). */
  reason: string;
}

/** One card. Prices and the change-% rule are the quote row's (`p`, `pc`, `o`). */
export interface PressureCardWire {
  s: string;
  p: number;
  pc?: number;
  o: number;
  /**
   * The halt risk, 0–100, what the gauge fills with (crates/server/src/pressure.rs "Halt risk"):
   * `round(100 · (0.4 · build-up + 0.35 · urgency + 0.25 · closeness))` from the rise below, the
   * time to the band at the current pace and how much of the way to it is covered. Frozen at the
   * halt while `halt` is set.
   */
  risk: number;
  /** The side the build-up pushes: `bull` toward the upper LULD band (limit up), `bear` the lower. */
  side: "bull" | "bear";
  /** That side's pressure now minus `riseSecs` seconds earlier, in points (−100…100). */
  rise: number;
  /** The horizon of `rise`, seconds (30). */
  riseSecs: number;
  /** The modelled LULD band on that side, dollars. */
  band: number;
  /** Distance from the last price to `band`, percent of the price (two decimals; 0 at the band). */
  toBand: number;
  /**
   * The one-sided flow pressure itself, 0–100: over the last minute of OHLC-counted prints, sides
   * by the tick rule, `round(100 · |2·buyShare − 1| · (0.4 + 0.6 · min(1, ln(1 + pace) / ln 101)))`.
   */
  pressure: number;
  /** Buy volume / volume with a side in the window, 0–1 (two decimals). */
  buyShare: number;
  /** Window volume as a multiple of the symbol's average volume per minute so far this session (premarket, regular, after hours each on its own; one decimal). */
  pace: number;
  halt?: PressureHaltWire;
}

export interface PressureSnapshotMessage {
  type: "pressure.snapshot";
  /** Market clock (epoch ms) at the ranking; null before any clock. */
  t: number | null;
  board: PressureCardWire[];
}

export interface PressureUpdateMessage {
  type: "pressure.update";
  t: number | null;
  board: PressureCardWire[];
}

export type PressureMessage = PressureSnapshotMessage | PressureUpdateMessage;

export interface PressureSubscribeMessage {
  type: "pressure.subscribe";
}

export interface PressureUnsubscribeMessage {
  type: "pressure.unsubscribe";
}

// --- Ross Panel (after `welcome`; crates/server/src/context.rs, crates/stock-context) -------
// The five-pillar watchlist (enter 5/5, stay 4/5) ordered by last-minute momentum, with each
// member's experimental pullback cue (a stage, not an entry signal). Opt in with `ross.subscribe`: a `ross.snapshot` at once,
// then a `ross.update` with the WHOLE panel when it changed (at most once a second). Both
// replace the client's panel, except each row's day line, which an update sends as a slice to
// merge (`RossRowWire.day_line`; `createRossChannel` does it). `enabled: false` means the server does not run stock context
// (STOCK_CONTEXT off): "not enabled", never "nothing qualifies". Field names inside `panel` are
// the engine's snake_case (stock_context::PanelSnapshot); prices are integer micro-dollars,
// returns basis points, times epoch nanoseconds.

/**
 * The coarse cue state. Since 2026-10-21 (detection-v2) it maps the micro-pullback's stages:
 * forming = `pullback_forming` / `armed`, near = armed within 1 % below the trigger, crossing =
 * `crossing_provisional`, triggered = `trigger_observed` (held), invalidated (held). The tokens are
 * the earlier `bull_flag_v0` cue's, kept so an older client still decodes a row. A cue describes a
 * stage, not an entry signal.
 */
export type RossSetupState = "watching" | "forming" | "near" | "crossing" | "triggered" | "invalidated" | "halted";

/** The detector state behind a cue (stock_context::State of the micro-pullback episode). */
export type RossCueStage = "pullback_forming" | "armed" | "crossing_provisional" | "trigger_observed" | "invalidated" | "expired" | "superseded" | "interrupted";

export interface RossLevelWire {
  price: number;
  /** `<episode id>:trigger:v<n>` (`<setup id>:trigger:<n>` from servers before 2026-10-21). */
  version: string;
  set_at?: number;
  reason?: string;
}

/**
 * stock_context::RossCue: the row's pullback cue, read from the engine's `micro_pullback` episodes
 * under `detection-v2` (variant MP5), the same episodes the Detection Panel shows. EXPERIMENTAL.
 * Every field past `state` is optional so a frame from an older server (the `bull_flag_v0`
 * SetupView) still decodes.
 */
export interface RossSetupWire {
  /** `micro_pullback` (`bull_flag_v0` from older servers). */
  family?: string;
  /** `micro_pullback_mp5`. */
  variant?: string;
  /** `detection-v2@<hash>`. */
  policy?: string;
  experimental?: boolean;
  state: RossSetupState;
  stage?: RossCueStage | null;
  state_since?: number | null;
  /** The Detection Panel's episode id for the same pullback. */
  episode_id?: string | null;
  trigger?: RossLevelWire | null;
  invalidation?: RossLevelWire | null;
  impulse_bp?: number | null;
  /** Fraction of the impulse retraced, thousandths. */
  depth_milli?: number | null;
  pullback_bars?: number | null;
  price?: number | null;
  /** The price the cue was judged on is older than the policy's freshness limit. */
  stale?: boolean;
  near_trigger?: boolean | null;
  distance_to_trigger_bp?: number | null;
  /** Integer thousandths (0..1000) from invalidation to trigger, never a 0..1 float. */
  meter_milli?: number | null;
  /** `(price − invalidation) / price`, bp. */
  to_invalidation_bp?: number | null;
  /** Only while crossing: what a completed bar still has to show. */
  pending?: { condition: "close_above_trigger" | "volume"; state: "PASS" | "FAIL" | "UNKNOWN" }[];
  extended?: boolean;
  reason?: string | null;
  /** How a held `triggered` / `invalidated` episode resolved. */
  resolution?: { state: RossCueStage; reason: string; at: number; bar_end: number | null; transition_id: string } | null;
  /** Older servers only (`bull_flag_v0`): the same distance as `to_invalidation_bp`. */
  risk_to_stop_bp?: number | null;
  /** Older servers only (`bull_flag_v0`). */
  transition_id?: string | null;
  setup_id?: string | null;
  flag_bars?: number;
  crossed_version?: string | null;
}

export interface RossMomentumWire {
  status: "ready" | "unavailable";
  reason: string;
  r_short_bp: number | null;
  r_primary_bp: number | null;
  r_context_bp: number | null;
  accel_bp: number | null;
  below_high_bp: number | null;
}

export interface RossEpisodeWire {
  id: string;
  admitted_at: number;
  retained_since: number | null;
  exited_at: number | null;
}

export interface RossMembershipWire {
  status: "not_member" | "qualified" | "retained" | "exited" | null;
  episodes: RossEpisodeWire[];
  /** [pillar, state, reason] of each pillar not passing. */
  nonpassing: [string, "PASS" | "FAIL" | "UNKNOWN", string][];
}

export interface RossRowWire {
  symbol: string;
  membership: RossMembershipWire;
  counts: { pass: number; fail: number; unknown: number };
  price: number | null;
  gain_bp: number | null;
  momentum: RossMomentumWire;
  rank: number | null;
  prev_rank: number | null;
  setup?: RossSetupWire | null;
  /** The one-minute move (= momentum.r_primary_bp). */
  move_1m_bp?: number | null;
  /** The pillars not passing, as objects (the same facts as membership.nonpassing). */
  missing?: { pillar: string; state: "FAIL" | "UNKNOWN"; reason: string }[];
  from_high_bp?: number | null;
  rvol_milli?: number | null;
  float_shares?: number | null;
  /** When that float figure was observed (epoch ns); pillars-v4 keeps one for 4 days. */
  float_as_of?: number | null;
  news_first_seen?: number | null;
  /** That story's headline class (`financing` = offering / dilution; stock-context `NewsItem::event_type`). */
  news_kind?: string | null;
  /** Closes (micros) of the last ≤20 completed 15 s bars that traded, oldest first; no timestamps. */
  spark?: number[];
  /**
   * The day line (crates/server/src/day_line.rs, owner 2026-10-21): 30 s closes (micros) of the
   * member's trading date from 04:00 ET, one per interval from `day_line_start` (epoch ns), the
   * first at the day's first price, the last at the last COMPLETED interval (`price` is the live
   * end). On the wire `day_line` holds the points from index `day_line_from`: 0 and the whole
   * line in every snapshot; in an update only what changed since the previous update.
   * `createRossChannel` merges the slices, so a row a client reads always holds the WHOLE line
   * with `day_line_from: 0`. Absent: no line yet (or an older server).
   */
  day_line_start?: number;
  day_line_step_s?: number;
  day_line_from?: number;
  day_line?: number[];
  /** The previous regular close (micros), when the closes file has it: the line's baseline. */
  prev_close?: number;
}

export interface RossPanelWire {
  revision: number;
  bar_end: number;
  cutoff: number;
  /** `live` or `replay`: only a live run's cutoff is comparable with a wall clock. */
  source?: string;
  /** The policy the cues come from: `detection-v2@<hash>` (`setups-v0@<hash>` from older servers). */
  setup_policy?: string;
  /** Members whose cue is near or crossing (older servers also counted triggered). */
  attention?: number;
  rows: RossRowWire[];
}

/** One pillar's counts over every symbol the engine decided today. */
export interface RossPillarReadinessWire {
  pillar: string;
  pass: number;
  fail: number;
  /** Could not be decided: the evidence is missing, stale or expired. */
  unknown: number;
}

export interface RossReasonCountWire {
  pillar: string;
  reason: string;
  symbols: number;
}

/**
 * Why the panel looks the way it does (crates/server/src/context.rs `Readiness`).
 *
 * An empty panel is not the same statement as "nothing qualifies today": it can also mean a
 * pillar had no evidence to decide on. These counts let a client say which, instead of showing
 * "No stock at 5/5 yet" over a missing-evidence state.
 *
 * Taken on the server's evidence task, so `observed_at` (UTC ns) is a snapshot time, not now.
 */
export interface RossReadinessWire {
  symbols: number;
  qualified: number;
  members: number;
  pillars: RossPillarReadinessWire[];
  /**
   * Symbols no pillar FAILS that still have a pillar UNKNOWN: the ones evidence could still put
   * on the panel. Absent from servers before 2026-10-30.
   */
  contenders?: number;
  /**
   * The commonest reasons a pillar could not decide, most symbols first, bounded. Counted over
   * the contenders only: a symbol another pillar already fails is not blocked by evidence.
   */
  blocking: RossReasonCountWire[];
  evidence_records: number;
  warmups?: {
    candidates: number;
    queued: number;
    known: number;
    full: number;
    busy: number;
    filed: number;
    flush_errors: number;
    pending: number;
    tracked: number;
    rolled: number;
  };
  observed_at: number;
}

export interface RossSnapshotMessage {
  type: "ross.snapshot";
  enabled: boolean;
  panelPolicy?: string;
  pillarPolicy?: string;
  panel: RossPanelWire | null;
  readiness?: RossReadinessWire | null;
}

export interface RossUpdateMessage {
  type: "ross.update";
  enabled: boolean;
  panelPolicy?: string;
  pillarPolicy?: string;
  panel: RossPanelWire | null;
  readiness?: RossReadinessWire | null;
}

export type RossMessage = RossSnapshotMessage | RossUpdateMessage;

export interface RossSubscribeMessage {
  type: "ross.subscribe";
}

export interface RossUnsubscribeMessage {
  type: "ross.unsubscribe";
}

// --- Synced settings (after `welcome`; crates/server/src/settings.rs, ws.rs) ---------------
// The user's chart preferences and drawings, shared live by every client and kept on the
// server (docs/architecture/settings-sync.md). Values are opaque to the server: each client
// validates what it reads with chart-core's parse functions. settingsSync.ts is the client.

/** One namespace (`layers`, `chartSettings`, `drawings:SMPL`, …) as the server holds it. */
export interface SettingsDocWire {
  ns: string;
  /** 1, 2, 3, … per namespace, bumped by every applied write. */
  rev: number;
  /** Wall-clock epoch ms of the last write. */
  updatedAt: number;
  value: unknown;
}

/** Right after `welcome`, again to a lagged connection: every namespace, the whole truth. */
export interface SettingsSnapshotMessage {
  type: "settings.snapshot";
  docs: SettingsDocWire[];
}

/** After every applied write, to every connection (the writer included: it matches `opId`). */
export interface SettingsChangedMessage extends SettingsDocWire {
  type: "settings.changed";
  opId: string;
}

/** To the writer alone; nothing changed. `settings_unavailable` is worth retrying, the rest are not. */
export interface SettingsRejectedMessage {
  type: "settings.rejected";
  ns: string;
  opId: string;
  code: "bad_request" | "too_large" | "store_full" | "not_a_map" | "settings_unavailable" | (string & {});
  message: string;
}

export type SettingsServerMessage = SettingsSnapshotMessage | SettingsChangedMessage | SettingsRejectedMessage;

/** Replaces a namespace's value; `ifAbsent` only seeds one that was never written (migration). */
export interface SettingsSetMessage {
  type: "settings.set";
  ns: string;
  value: unknown;
  opId: string;
  ifAbsent?: true;
}

/** Sets and removes entries of an object-valued namespace (a symbol's drawings by id). */
export interface SettingsPatchMessage {
  type: "settings.patch";
  ns: string;
  set?: Record<string, unknown>;
  remove?: string[];
  opId: string;
}

export type SettingsClientMessage = SettingsSetMessage | SettingsPatchMessage;

// --- Price alerts (after `welcome`; crates/server/src/alerts, ws.rs) ------------------------
// The rules are the synced `alerts` namespace (app-state alerts.ts); the server evaluates them
// on the live market and reports here. Activity (fires and hold-backs) is server-side, kept 30
// days; a connection gets the newest 500 events on connect.

/** One Activity row (crates/server/src/alerts/history.rs Event). */
export interface AlertEventWire {
  id: string;
  alertId: string;
  symbol: string;
  kind: "fired" | "held";
  /** The print's (or status's) own time, epoch ms. */
  t: number;
  /** When the server decided, epoch ms. */
  at: number;
  /** "Crossed above 2.50 at 2.51", "Halted · LUDP", … */
  text: string;
  price?: number;
  /** A late or corrected print. */
  late?: true;
  /** A toast was shown in the app (the alert's "In app" chip). */
  app: boolean;
  /** `sent` | `held` | `off` | `failed` | `none` (a held event). */
  phone: "sent" | "held" | "off" | "failed" | "none" | (string & {});
  /** `stale`, `halted`, `gap` (not fired) or, for the phone, `no_device`, `quiet_hours`, `symbol_limit`, `budget`, `push_failed`. */
  reason?: string;
  /** The row's second line ("Phone + in app · cooling until 10:46"). */
  note: string;
}

export interface AlertsStatusWire {
  /** Phone notifications sent today (ET day) and the budget. */
  phoneToday: number;
  phoneBudget: number;
  /** Phones registered for push. */
  devices: number;
  /** Market inputs the evaluator had to drop because it fell behind (should stay 0). */
  dropped: number;
}

/** Right after `settings.snapshot`, and to a lagged connection: Activity (oldest first) and the status. */
export interface AlertsHistoryMessage extends AlertsStatusWire {
  type: "alerts.history";
  events: AlertEventWire[];
}

export interface AlertFiredMessage {
  type: "alert.fired";
  event: AlertEventWire;
  phoneToday?: number;
}

export interface AlertHeldMessage {
  type: "alert.held";
  event: AlertEventWire;
}

/** A recorded event changed (a phone push that failed after it was recorded as sent). */
export interface AlertUpdatedMessage {
  type: "alert.updated";
  event: AlertEventWire;
}

export interface AlertsStatusMessage extends AlertsStatusWire {
  type: "alerts.status";
}

export type AlertsServerMessage = AlertsHistoryMessage | AlertFiredMessage | AlertHeldMessage | AlertUpdatedMessage | AlertsStatusMessage;

/** This phone's Expo push token (per device; the server never sends it to other clients). */
export interface PushRegisterMessage {
  type: "push.register";
  token: string;
  platform?: string;
  name?: string;
  /** Runners pushes for new flags on this phone (server default on; absent = unchanged). */
  runnersNew?: boolean;
  /** Runners pushes for breakouts on this phone (server default off; absent = unchanged). */
  runnersBreakout?: boolean;
  /** Runners pushes for buying bursts on this phone (server default on; absent = unchanged). */
  runnersBurst?: boolean;
  /** Runners pushes for running entries on this phone (server default off; absent = unchanged). */
  runnersRunning?: boolean;
  /** Runners pushes for "Run +20%" entries on this phone (server default off; absent = unchanged). */
  runnersRun?: boolean;
  /** Runners pushes for "Fast move" entries on this phone (server default on; absent = unchanged). */
  runnersFast?: boolean;
  /** Pushes when a Runners stock comes down into a volume shelf (server default on; absent = unchanged). */
  runnersShelf?: boolean;
  /** Pushes when a later Runners path fires on a stock already listed (server default on). */
  runnersFollowUp?: boolean;
  /** Pushes when a Ross stock's pullback trigger is being crossed (server default on). */
  rossCrossing?: boolean;
  /** The same for the Signals panel's episodes (server default on). */
  signalsCrossing?: boolean;
  /** Pushes when a Ross stock's price reaches its trigger (server default off). */
  rossTrigger?: boolean;
  /** The same for the Signals panel (server default off). */
  signalsTrigger?: boolean;
  /** Pushes when the Halt watcher's gauge appears for a stock (server default off). */
  haltWatch?: boolean;
  /** Pushes when a Runners or Ross stock's pullback sets up, then crosses its trigger (server default on). */
  runnersPullback?: boolean;
}

export interface PushUnregisterMessage {
  type: "push.unregister";
  token: string;
}

export type ClientMessage = HelloMessage | PingMessage | PongMessage | QuotesSubscribeMessage | QuotesUnsubscribeMessage | PressureSubscribeMessage | PressureUnsubscribeMessage | RossSubscribeMessage | RossUnsubscribeMessage | import("./favorites.ts").FavoritesClientMessage | MoversClientMessage | import("./symbols.ts").SymbolsClientMessage | import("./noticeFeed.ts").NoticesClientMessage | import("./insightsTicker.ts").TickerClientMessage | { type: "detection.subscribe" | "detection.unsubscribe" } | import("./rocket.ts").RocketClientMessage | SettingsClientMessage | PushRegisterMessage | PushUnregisterMessage | import("./runnersNotify.ts").RunnersNotifyMessage | import("./tickers/quoteSearch.ts").QuoteSearchClientMessage;
export type ServerMessage = WelcomeMessage | PingMessage | PongMessage | ErrorMessage | QuotesMessage | PressureMessage | RossMessage | import("./favorites.ts").FavoritesMessage | MoversMessage | import("./symbols.ts").SymbolsMessage | import("./noticeFeed.ts").NoticesMessage | import("./insightsTicker.ts").TickerSnapshotMessage | import("./detection.ts").DetectionMessage | import("./rocket.ts").RocketMessage | SettingsServerMessage | AlertsServerMessage | import("./runnersNotify.ts").RunnersNoticeMessage | import("./tickers/quoteSearch.ts").QuoteSearchMessage | import("./rocket.ts").RunnersEventMessage;

const ALERT_TYPES = new Set(["alerts.history", "alert.fired", "alert.held", "alert.updated", "alerts.status"]);

/** True for the alert frames (routed to the app's store, whatever page is open). */
export function isAlertsMessage(m: ServerMessage): m is AlertsServerMessage {
  return ALERT_TYPES.has(m.type);
}

const SERVER_TYPES = new Set(["welcome", "ping", "pong", "error", "quotes.snapshot", "quotes.update", "quotes.search.snapshot", "quotes.search.update", "quotes.directory", "runners.event", "pressure.snapshot", "pressure.update", "ross.snapshot", "ross.update", "favorites.snapshot", "favorites.update", "movers.snapshot", "movers.update", "symbols.snapshot", "symbols.update", "notices.snapshot", "notices.append", "insights.ticker.snapshot", "insights.ticker.update", "detection.snapshot", "detection.update", "rocket.snapshot", "rocket.update", "settings.snapshot", "settings.changed", "settings.rejected", ...ALERT_TYPES]);

/** Parses one server frame; returns null for anything malformed or unknown. */
export function parseServerMessage(raw: unknown): ServerMessage | null {
  if (typeof raw !== "string") return null;
  let value: unknown;
  try {
    value = JSON.parse(raw);
  } catch {
    return null;
  }
  if (typeof value !== "object" || value === null) return null;
  const type = (value as { type?: unknown }).type;
  if (type === "detection.snapshot" || type === "detection.update") return isDetectionMessage(value) ? value : null;
  if (type === "runners.notice") return parseRunnersNotice(value);
  return typeof type === "string" && SERVER_TYPES.has(type) ? (value as ServerMessage) : null;
}

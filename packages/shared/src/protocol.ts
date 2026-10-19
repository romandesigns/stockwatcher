// Wire types shared by web, desktop and mobile. The Rust side mirrors these
// in crates/server/src/{api,ws,market}.rs; change both together.

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

export interface QuotesSubscribeMessage {
  type: "quotes.subscribe";
}

export interface QuotesUnsubscribeMessage {
  type: "quotes.unsubscribe";
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

export type ClientMessage = HelloMessage | PingMessage | PongMessage | QuotesSubscribeMessage | QuotesUnsubscribeMessage | SettingsClientMessage;
export type ServerMessage = WelcomeMessage | PingMessage | PongMessage | ErrorMessage | QuotesMessage | SettingsServerMessage;

const SERVER_TYPES = new Set(["welcome", "ping", "pong", "error", "quotes.snapshot", "quotes.update", "settings.snapshot", "settings.changed", "settings.rejected"]);

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
  return typeof type === "string" && SERVER_TYPES.has(type) ? (value as ServerMessage) : null;
}

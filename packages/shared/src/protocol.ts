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

export interface HelloMessage {
  type: "hello";
  token?: string;
  /** Free-form client identifier for server logs, e.g. "web". */
  client?: string;
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

export type ErrorCode = "unauthorized" | "bad_request" | "hello_timeout" | "shutting_down";

export interface ErrorMessage {
  type: "error";
  code: ErrorCode;
  message: string;
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

export type ClientMessage = HelloMessage | PingMessage | PongMessage | QuotesSubscribeMessage | QuotesUnsubscribeMessage;
export type ServerMessage = WelcomeMessage | PingMessage | PongMessage | ErrorMessage | QuotesMessage;

const SERVER_TYPES = new Set(["welcome", "ping", "pong", "error", "quotes.snapshot", "quotes.update"]);

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

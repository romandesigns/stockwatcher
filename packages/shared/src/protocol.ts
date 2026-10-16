// Wire types shared by web, desktop and mobile. The Rust side mirrors these
// in crates/server/src/{routes,ws}.rs; change both together.

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

export type ClientMessage = HelloMessage | PingMessage | PongMessage;
export type ServerMessage = WelcomeMessage | PingMessage | PongMessage | ErrorMessage;

const SERVER_TYPES = new Set(["welcome", "ping", "pong", "error"]);

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

// Which account the LIVE market's chart orders go to (owner, 2026-10-29; contract:
// docs/architecture/alpaca-paper.md, server: crates/server/src/trading.rs).
//
// - `tradingAccount` (synced, written by clients): `{ live: "simulator" | "alpaca_paper" }`.
//   Anything else reads as the simulator, as on the server. Replay is always the simulator.
// - `broker.status` (written by the server only): the Alpaca paper route's health, so Settings
//   can say "not configured" and why.

export const TRADING_NS = {
  account: "tradingAccount",
  status: "broker.status",
} as const;

export type LiveAccount = "simulator" | "alpaca_paper";

export interface TradingAccount {
  live: LiveAccount;
}

export const DEFAULT_TRADING_ACCOUNT: TradingAccount = { live: "simulator" };

export function parseTradingAccount(raw: unknown): TradingAccount {
  const live = typeof raw === "object" && raw !== null ? (raw as { live?: unknown }).live : undefined;
  return { live: live === "alpaca_paper" ? "alpaca_paper" : "simulator" };
}

/** The Alpaca paper route as the server reports it. */
export interface BrokerStatus {
  /** True once Alpaca paper accepted the server's keys. */
  configured: boolean;
  /** `connecting`, `live`, `down`, `unauthorized`, `not_configured`, or `unknown` before the server said. */
  state: string;
  message: string;
}

export const UNKNOWN_BROKER_STATUS: BrokerStatus = { configured: false, state: "unknown", message: "" };

export function parseBrokerStatus(raw: unknown): BrokerStatus {
  const a = typeof raw === "object" && raw !== null ? (raw as { alpacaPaper?: unknown }).alpacaPaper : undefined;
  if (typeof a !== "object" || a === null) return UNKNOWN_BROKER_STATUS;
  const v = a as Record<string, unknown>;
  return {
    configured: v.configured === true,
    state: typeof v.state === "string" ? v.state : "unknown",
    message: typeof v.message === "string" ? v.message : "",
  };
}

/** True when choosing Alpaca paper cannot work right now (the server has no working keys). */
export function alpacaUnavailable(s: BrokerStatus): boolean {
  return s.state === "not_configured" || s.state === "unauthorized";
}

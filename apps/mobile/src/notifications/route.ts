// Where a tapped phone notification goes (pure, so bun tests it without a native runtime).
//
// - A Runners push (crates/server/src/alerts/runners.rs, data `{symbol, kind: "runners.new" |
//   "runners.breakout" | "runners.burst" | "runners.running", alert_id}`) opens that ticker's chart modal: the point of the push is to
//   see the move now.
// - Anything else (a price alert's push, data `{alertId, symbol, eventId, url}`; an unknown or
//   older payload) opens the Alerts tab, as before.

export type NotificationTarget = { to: "chart"; symbol: string } | { to: "alerts" };

/** A ticker as the wire spells it (app-state parseSymbol's rule). */
const SYMBOL = /^[A-Z][A-Z0-9.-]{0,11}$/;

export function notificationTarget(data: unknown): NotificationTarget {
  if (data && typeof data === "object") {
    const d = data as Record<string, unknown>;
    const kind = typeof d.kind === "string" ? d.kind : "";
    const symbol = typeof d.symbol === "string" ? d.symbol.trim().toUpperCase() : "";
    if (kind.startsWith("runners.") && SYMBOL.test(symbol)) return { to: "chart", symbol };
  }
  return { to: "alerts" };
}

/**
 * Whether the dashboard should ring its own local banner for a new Runners flag: only while this
 * phone has no push token. With one, the server's push already rings (in the foreground too), and
 * a second banner for the same flag would be noise.
 */
export function ringRunnersLocally(pushNew: boolean, token: string | null): boolean {
  return pushNew && !token;
}

// Runners in-app toasts on the web dashboard (owner, 2026-10-22): the phone gets a push for each
// new flag and, opted in, each breakout (crates/server/src/alerts/runners.rs); the web shows the
// same moments as a toast whether or not the Runners panel is on screen, the price alerts' toast with a Chart action.
//
// The server decides each moment on the market clock and sends it as `runners.event` (clock.v1,
// crates/server/src/time_marks.rs: only news, fresh by the market clock, running / run / fast
// entries never): the toast is shown as sent. The web always ships with its server, so the
// wall-clock diff of panels an older server needed is gone (the phone keeps its own until its
// installed builds are current).
import type { RocketChannel, RunnersEventMessage } from "@stockwatcher/shared";
import { useEffect, useRef } from "react";

import { showToast } from "../lib/toastStore.ts";
import { getRunnersNotifier, type RunnersNotifier } from "../notify/runnersNotify.ts";

export interface RunnersToast {
  kind: "new" | "breakout";
  symbol: string;
  text: string;
}

const dollars = (p: number) => `$${p.toFixed(p >= 1 ? 2 : 4)}`;

/** A `runners.event` as a toast. */
export function runnersEventToast(m: RunnersEventMessage): RunnersToast | null {
  if (typeof m.symbol !== "string" || !m.symbol) return null;
  if (m.kind === "new_flag") return { kind: "new", symbol: m.symbol, text: [`Runners: ${m.symbol}${m.price ? ` ${dollars(m.price)}` : ""}`, ...(m.reason ? [m.reason] : [])].join(" · ") };
  if (m.kind === "breakout") {
    const pct = m.from_flag_pct;
    return { kind: "breakout", symbol: m.symbol, text: [`${m.symbol} breaking out${m.price ? ` ${dollars(m.price)}` : ""}`, pct == null ? null : `${pct >= 0 ? "+" : ""}${pct.toFixed(1)}% from flag`].filter(Boolean).join(" · ") };
  }
  return null;
}

/**
 * One event, one signal (owner, 2026-10-23): while this device shows Runners system
 * notifications for the kind (notify/runnersNotify.ts), the toast stands down, focused window
 * or not; the notification carries the phone's text and its own click to the chart. A kind
 * switched off there still toasts as before.
 */
export function toastStandsDown(kind: RunnersToast["kind"], notifier: Pick<RunnersNotifier, "active" | "prefs">): boolean {
  return notifier.active() && notifier.prefs().kinds[kind];
}

/** While the dashboard is mounted: a toast with "Chart" for each new flag and breakout. */
export function useRunnersToasts(channel: RocketChannel | undefined, onPick: (symbol: string) => void): void {
  // The latest callback, without resubscribing.
  const pick = useRef(onPick);
  useEffect(() => {
    pick.current = onPick;
  }, [onPick]);
  useEffect(() => {
    if (!channel) return;
    // Subscribing keeps the connection opted in to rocket.* (and so to `runners.event`).
    const offRows = channel.subscribe(() => {});
    const offEvents = channel.onEvent?.((m) => {
      const t = runnersEventToast(m);
      if (!t || toastStandsDown(t.kind, getRunnersNotifier())) return;
      showToast(t.text, { icon: "bell", ms: 8000, action: { label: "Chart", run: () => pick.current(t.symbol) } });
    });
    return () => {
      offRows();
      offEvents?.();
    };
  }, [channel]);
}

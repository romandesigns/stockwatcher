// Runners in-app toasts on the web dashboard (owner, 2026-10-22): the phone gets a push for each
// new flag and, opted in, each breakout (crates/server/src/alerts/runners.rs); the web shows the
// same moments as a toast whether or not the Runners panel is on screen, the price alerts' toast with a Chart action.
//
// A server that decides time (clock.v1) sends each moment as `runners.event`, decided on the
// market clock (crates/server/src/time_marks.rs): the toast is shown as sent. Against an older
// server the rows are diffed here: only news, the first frame primes (no burst on open), and a
// flag or breakout older than FRESH_MS by the wall clock is not shown, so a reconnect or a
// replay's own day (its times are in the past) never toasts, as a replay never pushes.
import type { RocketChannel, RocketMomentumState, RocketRowWire, RunnersEventMessage } from "@stockwatcher/shared";
import { useEffect, useRef } from "react";

import { showToast } from "../lib/toastStore.ts";
import { getRunnersNotifier, type RunnersNotifier } from "../notify/runnersNotify.ts";

/** The server's MAX_AGE_MS for a push. */
export const FRESH_MS = 3 * 60_000;
/** At most this many toasts from one frame (a toast replaces the last one anyway). */
const MAX_PER_FRAME = 3;

export interface RunnersToastMemory {
  /** Each row seen, with its momentum state then. */
  seen: Map<string, RocketMomentumState | undefined>;
  primed: boolean;
}

export interface RunnersToast {
  kind: "new" | "breakout";
  symbol: string;
  text: string;
}

const dollars = (p: number) => `$${p.toFixed(p >= 1 ? 2 : 4)}`;

/** The toasts one frame's rows call for, updating `mem`. Pure, for the test. */
export function runnersToasts(rows: RocketRowWire[], mem: RunnersToastMemory, wallNow: number): RunnersToast[] {
  const out: RunnersToast[] = [];
  const fresh = (t: number | undefined) => t !== undefined && wallNow - t <= FRESH_MS;
  for (const r of rows) {
    const had = mem.seen.has(r.id);
    const before = mem.seen.get(r.id);
    const state = r.provisional ? before : r.state;
    mem.seen.set(r.id, state);
    if (!mem.primed) continue;
    if (!had) {
      // Running, "Run +20%" and "Fast move" entries never toast (owner, 2026-10-22): many a day,
      // and a move under way rather than a forecast; the phone push is their per-device switch.
      if (fresh(r.at) && r.kind !== "running" && r.kind !== "run" && r.kind !== "fast") out.push({ kind: "new", symbol: r.s, text: [`Runners: ${r.s} ${dollars(r.p)}`, ...r.reasons.slice(0, 1)].join(" · ") });
    } else if (state === "breaking_out" && before !== "breaking_out" && fresh(r.state_since)) {
      const pct = r.from_flag_pct;
      const price = r.last ?? r.p;
      out.push({ kind: "breakout", symbol: r.s, text: [`${r.s} breaking out ${dollars(price)}`, pct == null ? null : `${pct >= 0 ? "+" : ""}${pct.toFixed(1)}% from flag`].filter(Boolean).join(" · ") });
    }
  }
  mem.primed = true;
  return out.slice(0, MAX_PER_FRAME);
}

/** A `runners.event` as a toast (the same words the diff below writes). */
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
  // The latest callback, without resubscribing (a resubscribe would prime again and miss news).
  const pick = useRef(onPick);
  useEffect(() => {
    pick.current = onPick;
  }, [onPick]);
  useEffect(() => {
    if (!channel) return;
    const mem: RunnersToastMemory = { seen: new Map(), primed: false };
    const show = (t: RunnersToast) => {
      if (toastStandsDown(t.kind, getRunnersNotifier())) return;
      showToast(t.text, { icon: "bell", ms: 8000, action: { label: "Chart", run: () => pick.current(t.symbol) } });
    };
    // Rows already on the panel (another view subscribed first) are not news.
    const now = channel.get().panel?.rows;
    if (now) runnersToasts(now, mem, Date.now());
    // Subscribing keeps the connection opted in to rocket.* (and so to `runners.event`).
    const offRows = channel.subscribe(() => {
      // The server decides the moments (clock.v1): nothing to diff, and no wall clock to ask.
      if (channel.get().serverEvents) return;
      const rows = channel.get().panel?.rows;
      if (!rows) return;
      for (const t of runnersToasts(rows, mem, Date.now())) show(t);
    });
    const offEvents = channel.onEvent?.((m) => {
      const t = runnersEventToast(m);
      if (t) show(t);
    });
    return () => {
      offRows();
      offEvents?.();
    };
  }, [channel]);
}

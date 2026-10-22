// Runners in-app toasts on the web dashboard (owner, 2026-10-22): the phone gets a push for each
// new flag and, opted in, each breakout (crates/server/src/alerts/runners.rs); the web shows the
// same moments as a toast whatever side tab is open, the price alerts' toast with a Chart action.
//
// Only news: the first frame primes (no burst on open), and a flag or breakout older than
// FRESH_MS by the wall clock is not shown, so a reconnect or a replay's own day (its times are
// in the past) never toasts, as a replay never pushes.
import type { RocketChannel, RocketMomentumState, RocketRowWire } from "@stockwatcher/shared";
import { useEffect, useRef } from "react";

import { showToast } from "../lib/toastStore.ts";

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
      if (fresh(r.at)) out.push({ kind: "new", symbol: r.s, text: [`Runners: ${r.s} ${dollars(r.p)}`, ...r.reasons.slice(0, 1)].join(" · ") });
    } else if (state === "breaking_out" && before !== "breaking_out" && fresh(r.state_since)) {
      const pct = r.from_flag_pct;
      const price = r.last ?? r.p;
      out.push({ kind: "breakout", symbol: r.s, text: [`${r.s} breaking out ${dollars(price)}`, pct == null ? null : `${pct >= 0 ? "+" : ""}${pct.toFixed(1)}% from flag`].filter(Boolean).join(" · ") });
    }
  }
  mem.primed = true;
  return out.slice(0, MAX_PER_FRAME);
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
    // Rows already on the panel (another view subscribed first) are not news.
    const now = channel.get().panel?.rows;
    if (now) runnersToasts(now, mem, Date.now());
    return channel.subscribe(() => {
      const rows = channel.get().panel?.rows;
      if (!rows) return;
      for (const t of runnersToasts(rows, mem, Date.now())) {
        showToast(t.text, { icon: "bell", ms: 8000, action: { label: "Chart", run: () => pick.current(t.symbol) } });
      }
    });
  }, [channel]);
}

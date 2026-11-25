// The practice page's alert toasts (owner, 2026-11-23; crates/server/src/practice.rs; the
// wiring is shared's `attachPracticeToasts`, the phone uses the same):
//
// - **Practice:** what the replay market's detectors decided (`practice.notice`, replay
//   connections only), as an in-app toast with a "Practice" chip. Never a system notification,
//   never a sound: this hook is the only reader of the feed's `practiceNotices`, and it only ever
//   calls showToast. Its "Chart" opens the ticker on the practice chart.
// - **Live:** a real alert arriving while practising (`runners.notice`: the server sends those to
//   every socket), as an in-app toast with a "Live" chip, so a real alert is never mistaken for a
//   practice one. While the page is open that toast is the only signal on this device: the live
//   notifier holds back its system/desktop notification (`quiet()`, owner 2026-11-25; a halt
//   still plays its sound). The phone's push is unaffected. Its "Chart" opens the LIVE
//   dashboard, the only page that shows live data. A grouped repeat never toasts.
//
// Both follow this device's Settings → Alerts switches (the same kinds the live notifications
// use), and each id toasts once.

import { attachPracticeToasts, type PracticeToast } from "@stockwatcher/shared";
import { useEffect, useRef } from "react";

import type { ChartFeed } from "../feed/types.ts";
import { showToast } from "../lib/toastStore.ts";
import { openTickerChart } from "../nav/openTicker.ts";
import { getRunnersNotifier } from "../notify/runnersNotify.ts";

/** How long an alert toast stays fully visible (the dashboard's Runners toasts' time). */
export const PRACTICE_TOAST_MS = 8000;

/** One practice-page toast as the web shows it: a practice "Chart" opens the practice chart, a live one the live dashboard. */
export function showPracticeToast(t: PracticeToast, openPractice: (symbol: string) => void, openLive: (symbol: string) => void = openTickerChart, show: typeof showToast = showToast): void {
  const open = t.label === "live" ? openLive : openPractice;
  show(t.text, { icon: "bell", tag: t.label, ms: PRACTICE_TOAST_MS, action: { label: "Chart", run: () => open(t.symbol) } });
}

/** While the practice page is mounted: its Practice and Live toasts (module docs). */
export function usePracticeToasts(feed: ChartFeed, openPractice: (symbol: string) => void): void {
  const open = useRef(openPractice);
  useEffect(() => {
    open.current = openPractice;
  }, [openPractice]);
  useEffect(
    () =>
      attachPracticeToasts(feed, {
        kinds: () => getRunnersNotifier().prefs().kinds,
        show: (t) => showPracticeToast(t, (s) => open.current(s)),
      }),
    [feed],
  );
  // No system notification for a live alert while practising: the Live toast says it (module docs).
  useEffect(() => getRunnersNotifier().quiet(), []);
}

// The practice screen's alert toasts (the Chart tab; owner, 2026-11-23; the web's
// practice/practiceToasts.ts, through the same shared `attachPracticeToasts`):
//
// - **Practice:** what the replay market's detectors decided (`practice.notice`), as an in-app
//   toast with a "Practice" chip. Never a local notification (`scheduleNotificationAsync`), a push
//   or a sound: this hook is the only reader of the feed's `practiceNotices`, and it only ever
//   calls showToast. Its "Chart" loads the ticker into the practice chart.
// - **Live:** a real alert arriving while practising (`runners.notice`, which the server sends to
//   every socket), with a "Live" chip; the phone's own push rings for it as always. Its "Chart"
//   opens the live chart modal (the dashboard's feed), never the practice chart.
//
// Both follow this phone's Settings → Alerts switches, and each id toasts once.

import { attachPracticeToasts } from "@stockwatcher/shared";
import { useEffect, useRef } from "react";

import type { MobileFeed } from "../feed/serverFeed";
import { openTickerChart } from "../navigation/ref";
import { showToast } from "../ui/toast";
import { phoneNotifyKinds } from "./runnersNoticeModel";
import { runnersPushPrefs } from "./runnersPush";

/** How long an alert toast stays fully visible. */
const PRACTICE_TOAST_MS = 8000;

export function usePracticeToasts(feed: MobileFeed, openPractice: (symbol: string) => void): void {
  const open = useRef(openPractice);
  useEffect(() => {
    open.current = openPractice;
  }, [openPractice]);
  useEffect(
    () =>
      attachPracticeToasts(feed, {
        kinds: () => phoneNotifyKinds(runnersPushPrefs()),
        show: (t) => showToast(t.text, { icon: "bell", tag: t.label, ms: PRACTICE_TOAST_MS, action: { label: "Chart", run: () => (t.label === "live" ? openTickerChart(t.symbol) : open.current(t.symbol)) } }),
      }),
    [feed],
  );
}

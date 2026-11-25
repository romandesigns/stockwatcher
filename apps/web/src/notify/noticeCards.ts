import { useSyncExternalStore } from "react";

import { createNoticeCards, noticeCardLabel, type NoticeCard } from "@stockwatcher/shared";

// The page's per-stock notice cards ("Group repeat alerts", owner 2026-11-25; shared
// noticeCounts.ts): every `runners.notice` and silent `runners.grouped` frame lands here
// (feed/serverFeed.ts), and a Runners row shows its stock's "12 alerts · 10:37".

export const webNoticeCards = createNoticeCards();

/** The stock's card today, re-rendering when any card changes. */
export function useNoticeCard(symbol: string): NoticeCard | null {
  return useSyncExternalStore(
    (cb) => webNoticeCards.subscribe(cb),
    () => webNoticeCards.get(symbol),
    () => null,
  );
}

/** The row chip's text, or null for a single alert. */
export function useNoticeCardLabel(symbol: string): string | null {
  return noticeCardLabel(useNoticeCard(symbol));
}

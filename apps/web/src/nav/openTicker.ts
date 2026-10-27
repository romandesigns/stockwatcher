// The one "open this ticker" action of the web app and the desktop shell (owner, 2026-10-23: the
// chart must always be replaced by the ticker just clicked, wherever it was presented). Every
// entry point goes through it: a system notification's click (notify/runnersNotify.ts, and the
// desktop shell's `window.__stockwatcherOpenChart`, apps/desktop/src-tauri/src/notify.rs), a
// Runners toast's "Chart", the dashboard's panel rows (Ross, Runners, Signals, Halt watcher), its
// ticker search and the alerts' "Open chart".
//
// What it does, in this order:
// 1. loads the ticker into the dashboard's selected chart (app-state `loadIntoSelected`), which
//    also records it as this device's explicit pick (`picked`), so the "left the price band,
//    show the first candidate" fallback (app-state `shownTicker`) can never put another ticker
//    in its place. That fallback was the bug: a Runners ticker the polled candidate list did not
//    hold yet loaded into the slot, and the chart showed the first candidate instead;
// 2. closes the search panel (a click from a notification can arrive while it is open);
// 3. only then switches to the dashboard if another page is open, so the dashboard mounts with
//    the new ticker already in its slot and never paints the old one first.
//
// The slot is a synced document, but the write is local-first (settingsSync: a pending edit stays
// on top of whatever the server confirms meanwhile), so a snapshot or another device's older
// write arriving after the click cannot put the previous ticker back.

import { parseSymbol, type AppStore } from "@stockwatcher/app-state";
import { parseNoticeChartContext, type NoticeChartContext } from "@stockwatcher/shared";
import { getAppStore } from "../state/store.ts";
import { pageOf } from "./routes.ts";

export interface OpenTickerEnv {
  store: Pick<AppStore, "getState">;
  /** The page's location hash (the open page, nav/routes.ts). */
  hash: { get(): string; set(hash: string): void };
}

/** Opens `raw` in `env`; returns the ticker opened, or null when `raw` is not a ticker. */
export function openTickerIn(env: OpenTickerEnv, raw: string, noticeContext?: NoticeChartContext): string | null {
  const symbol = parseSymbol(raw);
  if (!symbol) return null;
  const app = env.store.getState();
  app.loadIntoSelected(symbol);
  app.selectNotice(parseNoticeChartContext(noticeContext, symbol));
  if (app.searchOpen) app.setSearchOpen(false);
  if (pageOf(env.hash.get()) !== "dashboard") env.hash.set("#/");
  return symbol;
}

function browserEnv(): OpenTickerEnv {
  return {
    store: getAppStore(),
    hash: {
      get: () => (typeof window === "undefined" ? "" : window.location.hash),
      set: (h) => {
        if (typeof window !== "undefined") window.location.hash = h;
      },
    },
  };
}

/** The page's "open this ticker": see the file comment. */
export function openTickerChart(raw: string, noticeContext?: NoticeChartContext): void {
  openTickerIn(browserEnv(), raw, noticeContext);
}

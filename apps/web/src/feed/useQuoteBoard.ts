// The server's live quote board, only while `enabled`: subscribing opts the connection in to
// quotes.* frames, the last unsubscribe opts it out, so the server builds and sends nothing nobody
// is looking at. The dashboard holds it for as long as it is open (Top Gainers and Hot stocks are
// ranked from it, 2026-10-30); elsewhere only the open ticker search does.
import { EMPTY_QUOTE_BOARD, type QuoteBoard, type QuoteChannel } from "@stockwatcher/shared";
import { useCallback, useSyncExternalStore } from "react";

const NOOP = () => {};

export function useQuoteBoard(channel: QuoteChannel | undefined, enabled: boolean): QuoteBoard {
  const live = enabled ? channel : undefined;
  const subscribe = useCallback((cb: () => void) => (live ? live.subscribe(cb) : NOOP), [live]);
  const get = useCallback(() => (live ? live.get() : EMPTY_QUOTE_BOARD), [live]);
  // The same snapshot server-side, so the dashboard layout also renders in tests (renderToStaticMarkup).
  return useSyncExternalStore(subscribe, get, get);
}

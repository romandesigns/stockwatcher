// The server's live quote board, only while `enabled`: subscribing opts the connection in to
// quotes.* frames, the last unsubscribe opts it out, so the server builds and sends nothing nobody
// is looking at. Only the open ticker search holds it: the dashboard's Top Gainers and Hot stocks
// come ranked from the server's movers channel (useMovers), so no page keeps the whole board open.
import { EMPTY_QUOTE_BOARD, type QuoteBoard, type QuoteChannel } from "@stockwatcher/shared";
import { useCallback, useSyncExternalStore } from "react";

const NOOP = () => {};

export function useQuoteBoard(channel: QuoteChannel | undefined, enabled: boolean): QuoteBoard {
  const live = enabled ? channel : undefined;
  const subscribe = useCallback((cb: () => void) => (live ? live.subscribe(cb) : NOOP), [live]);
  const get = useCallback(() => (live ? live.get() : EMPTY_QUOTE_BOARD), [live]);
  // The same snapshot server-side, so a component holding it also renders in tests (renderToStaticMarkup).
  return useSyncExternalStore(subscribe, get, get);
}

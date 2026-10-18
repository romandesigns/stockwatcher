// The server's live quote board, only while `enabled` (the ticker search is open): subscribing
// opts the connection in to quotes.* frames, unsubscribing (closing the search) opts it out, so
// the server builds and sends nothing for a search nobody is looking at.
import { EMPTY_QUOTE_BOARD, type QuoteBoard, type QuoteChannel } from "@stockwatcher/shared";
import { useCallback, useSyncExternalStore } from "react";

const NOOP = () => {};

export function useQuoteBoard(channel: QuoteChannel | undefined, enabled: boolean): QuoteBoard {
  const live = enabled ? channel : undefined;
  const subscribe = useCallback((cb: () => void) => (live ? live.subscribe(cb) : NOOP), [live]);
  const get = useCallback(() => (live ? live.get() : EMPTY_QUOTE_BOARD), [live]);
  return useSyncExternalStore(subscribe, get);
}

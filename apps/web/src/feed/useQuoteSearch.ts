// The ticker search ranked on the server (quotes.search.v1, shared tickers/quoteSearch.ts): whether
// the connected server does it, and one search's live rows while `enabled`. Opening the search
// sends `quotes.search`; closing it sends `quotes.search.stop`, so the server ranks nothing nobody
// is looking at.
import { EMPTY_SEARCH_STATE, type QuoteSearchChannel, type QuoteSearchHandle, type QuoteSearchMeta, type QuoteSearchQuery, type QuoteSearchState } from "@stockwatcher/shared";
import { useCallback, useEffect, useRef, useState, useSyncExternalStore } from "react";

const NOOP = () => {};
const NO_META: QuoteSearchMeta = { available: false, directory: null };

export function useQuoteSearchMeta(channel: QuoteSearchChannel | undefined): QuoteSearchMeta {
  const subscribe = useCallback((cb: () => void) => (channel ? channel.watch(cb) : NOOP), [channel]);
  const get = useCallback(() => channel?.meta() ?? NO_META, [channel]);
  return useSyncExternalStore(subscribe, get, get);
}

/** One search's state; `query` may change freely (the channel ignores an unchanged one). */
export function useQuoteSearch(channel: QuoteSearchChannel | undefined, enabled: boolean, id: string, query: QuoteSearchQuery): QuoteSearchState {
  const [handle, setHandle] = useState<QuoteSearchHandle | null>(null);
  const latest = useRef(query);
  useEffect(() => {
    latest.current = query;
    handle?.set(query);
  }, [handle, query]);
  useEffect(() => {
    if (!channel || !enabled) return;
    const h = channel.open(id, latest.current);
    setHandle(h);
    return () => {
      h.close();
      setHandle(null);
    };
  }, [channel, enabled, id]);
  const subscribe = useCallback((cb: () => void) => (handle ? handle.subscribe(cb) : NOOP), [handle]);
  const get = useCallback(() => handle?.get() ?? EMPTY_SEARCH_STATE, [handle]);
  return useSyncExternalStore(subscribe, get, get);
}

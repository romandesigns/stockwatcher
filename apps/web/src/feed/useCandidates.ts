// Keeps the server's candidate list current. The Rust server screens every stock against its
// price band ($0.10–$25 by default) at the replay/live clock, so candidates come and go as
// prices move; the symbol picker must follow, not show the list from page load.
import { getToken } from "./token.ts";
import { EMPTY_SYMBOLS, type SymbolsChannel, type SymbolsState } from "@stockwatcher/shared";
import { useCallback, useEffect, useMemo, useState, useSyncExternalStore } from "react";
import { probeServer, type ServerSymbols, type SourceChoice } from "./source.ts";

/** The poll period, only for a server that does not push the list (no `symbols.v1`). */
export const CANDIDATE_REFRESH_MS = 10_000;

export interface Candidates {
  symbols: string[];
  band: ServerSymbols["priceBand"];
  screened: ServerSymbols["screened"];
  /** The latest /api/symbols answer or pushed list (day, source, fixture flag): see source.ts `followServer`. */
  server?: ServerSymbols;
}

const NOOP = () => {};

/**
 * `day` is the replay's trading day ("YYYY-MM-DD"): when it changes (a day switch, from this client
 * or another) the list is fetched at once, so a symbol the new day lacks is replaced right away
 * rather than leaving the old day's chart on screen.
 *
 * `channel` is the feed's `symbols` (crates/server/src/symbols.rs): once the server's welcome says
 * it pushes the list, the pushed list is the answer and nothing polls; `/api/symbols` is then only
 * the first probe. Against an older server (or before the first welcome) it polls every 10 s as
 * it always did.
 */
export function useCandidates(choice: SourceChoice, day: string | null = null, channel?: SymbolsChannel): Candidates {
  const [state, setState] = useState<Candidates>({ symbols: choice.symbols, band: undefined, screened: undefined });
  const subscribe = useCallback((cb: () => void) => (channel ? channel.subscribe(cb) : NOOP), [channel]);
  const get = useCallback((): SymbolsState => (channel ? channel.get() : EMPTY_SYMBOLS), [channel]);
  const pushed = useSyncExternalStore(subscribe, get, get);
  const live = pushed.available === true;

  const refresh = useCallback(
    async (alive: () => boolean) => {
      const probe = await probeServer(fetch, 2500, getToken(), choice.market);
      if (!alive() || !probe.ok) return;
      const d = probe.data;
      setState({ symbols: Array.isArray(d.symbols) ? d.symbols : [], band: d.priceBand, screened: d.screened, server: d });
    },
    [choice.market],
  );
  // The first probe, and again on a day change: always, pushed or not.
  useEffect(() => {
    let alive = true;
    void refresh(() => alive);
    return () => {
      alive = false;
    };
  }, [refresh, day]);
  // The poll, only while the list is not pushed.
  useEffect(() => {
    if (live) return;
    let alive = true;
    const id = setInterval(() => void refresh(() => alive), CANDIDATE_REFRESH_MS);
    return () => {
      alive = false;
      clearInterval(id);
    };
  }, [refresh, live]);

  const latest = live ? pushed.latest : null;
  const fromPush = useMemo<Candidates | null>(() => (latest ? { symbols: latest.symbols, band: latest.priceBand, screened: latest.screened, server: latest } : null), [latest]);
  return fromPush ?? state;
}

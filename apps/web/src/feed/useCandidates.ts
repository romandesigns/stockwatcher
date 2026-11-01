// Keeps the server's candidate list current. The Rust server screens every stock against its
// price band ($0.10–$25 by default) at the replay/live clock, so candidates come and go as
// prices move; the symbol picker must follow, not show the list from page load.
import { getToken } from "./token.ts";
import { EMPTY_SYMBOLS, type SymbolsChannel, type SymbolsState } from "@stockwatcher/shared";
import { useCallback, useEffect, useMemo, useState, useSyncExternalStore } from "react";
import { probeServer, type ServerSymbols, type SourceChoice } from "./source.ts";

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
 * `channel` is the feed's `symbols` (crates/server/src/symbols.rs): the server pushes the list
 * (`symbols.v1`) and the pushed list is the answer; `/api/symbols` is only the first probe (and a
 * day change's). Nothing polls: the web always ships with its server, so the 10 s poll an older
 * server needed is gone (the phone keeps its own for installed builds).
 */
export function useCandidates(choice: SourceChoice, day: string | null = null, channel?: SymbolsChannel): Candidates {
  const [state, setState] = useState<Candidates>({ symbols: choice.symbols, band: undefined, screened: undefined });
  const subscribe = useCallback((cb: () => void) => (channel ? channel.subscribe(cb) : NOOP), [channel]);
  const get = useCallback((): SymbolsState => (channel ? channel.get() : EMPTY_SYMBOLS), [channel]);
  const pushed = useSyncExternalStore(subscribe, get, get);
  const live = pushed.available === true;

  // The probe's answer, or null; the state is set from the promise below (only while mounted),
  // never synchronously inside an effect.
  const load = useCallback(async (): Promise<Candidates | null> => {
    const probe = await probeServer(fetch, 2500, getToken(), choice.market);
    if (!probe.ok) return null;
    const d = probe.data;
    return { symbols: Array.isArray(d.symbols) ? d.symbols : [], band: d.priceBand, screened: d.screened, server: d };
  }, [choice.market]);
  const refresh = useCallback(
    (alive: () => boolean) =>
      void load().then((c) => {
        if (c && alive()) setState(c);
      }),
    [load],
  );
  // The first probe, and again on a day change: always, pushed or not.
  useEffect(() => {
    let alive = true;
    refresh(() => alive);
    return () => {
      alive = false;
    };
  }, [refresh, day]);
  const latest = live ? pushed.latest : null;
  const fromPush = useMemo<Candidates | null>(() => (latest ? { symbols: latest.symbols, band: latest.priceBand, screened: latest.screened, server: latest } : null), [latest]);
  return fromPush ?? state;
}

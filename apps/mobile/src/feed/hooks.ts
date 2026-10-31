import { ChartStore, parseReplayAlerts, parseReplayDayState, sameIndicatorDemand, type ChartMessage, type Clock, type IndicatorDemand, type IndicatorValues, type PaperState, type ReplayAlert, type ReplayDayState, type Series, type TapeTrade, type Timeframe } from "@stockwatcher/chart-core";
import type { ChartTrading } from "@stockwatcher/chart-native";
import { authenticatedFetch, EMPTY_QUOTE_BOARD, parseLiveSession, withMarketSource, type DetectionChannel, type DetectionState, type FavoritesChannel, type FavoritesState, type LiveSession, type MarketSource, type MoversChannel, type MoversState, type PressureBoard, type PressureChannel, type PressureStatus, type QuoteBoard, type QuoteChannel, type RocketChannel, type RocketState, type RossChannel, type RossState, zoneSelector, type ZoneView } from "@stockwatcher/shared";
import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, useSyncExternalStore } from "react";

import { SERVER_URL } from "./server";
import type { MobileFeed } from "./serverFeed";

const TAPE_MAX = 50;

/** Feed subscription → ChartStore + a short tape (the web's useChartFeed, trimmed for the phone). */
/**
 * `slot`: the chart's slot on a connection carrying several (the dashboard); omitted = the default slot.
 *
 * `demand`: what the chart shows of its indicators (chart-core `indicatorDemand`). As on the web
 * (apps/web/src/feed/useChartFeed.ts): with a server that computes them (`indicators.v1`) it goes
 * on the subscription, a change goes out as `chart.indicators.set`, and `indicators` is the
 * server's values for the chart to draw; otherwise `indicators` is undefined and the chart
 * computes its own.
 */
export function useChartFeed(feed: MobileFeed, symbol: string, timeframe: Timeframe, profileBin = 0.01, slot?: string, demand?: IndicatorDemand) {
  const store = useMemo(() => new ChartStore({ maxCandles: 3000 }), []);
  const [tape, setTape] = useState<TapeTrade[]>([]);
  // Read at subscribe time without re-subscribing on a toggle; `sent` is what the server last heard.
  const demandRef = useRef(demand);
  // A layout effect runs before every passive effect of the commit, so the subscribe below reads this render's demand.
  useLayoutEffect(() => {
    demandRef.current = demand;
  });
  const sent = useRef<IndicatorDemand | undefined>(undefined);
  useEffect(() => {
    let pending: TapeTrade[] = [];
    let frame = 0;
    // Prints arrive in bursts at high replay speed: batch per frame.
    const flush = () => {
      frame = 0;
      const batch = pending;
      pending = [];
      setTape((prev) => [...batch.reverse(), ...prev].slice(0, TAPE_MAX));
    };
    const onMessage = (m: ChartMessage) => {
      if (m.type === "chart.snapshot") {
        pending = [];
        setTape([]);
        store.applySnapshot(m);
      } else if (m.type === "chart.update" || m.type === "chart.remove" || m.type === "chart.gap") store.applyIncrement(m);
      else if (m.type === "replay.clock") store.applyClock(m.clock);
      else if (m.type === "chart.halt" || m.type === "chart.luld") store.applySymbolEvent(m);
      else if (m.type === "chart.indicators") store.applyIndicators(m);
      else if (m.type === "chart.indicator") store.applyIndicatorIncrement(m);
      else if (m.symbol === symbol) {
        pending.push(m);
        if (!frame) frame = requestAnimationFrame(flush);
      }
    };
    sent.current = demandRef.current;
    const off = feed.subscribe({ symbol, timeframe, profileBin, ...(slot ? { id: slot } : {}), ...(demandRef.current ? { indicators: demandRef.current } : {}) }, onMessage);
    return () => {
      off();
      cancelAnimationFrame(frame);
    };
  }, [feed, store, symbol, timeframe, profileBin, slot]);
  useEffect(() => {
    if (!demand || (sent.current && sameIndicatorDemand(sent.current, demand))) return;
    // Extended hours flipped: the other rth's values are frozen from when this chart last showed
    // them (ChartStore `dropIndicators`); no lines until the answer, which on cellular takes a while.
    if (sent.current && sent.current.rth !== demand.rth) store.dropIndicators(symbol, timeframe, demand.rth);
    sent.current = demand;
    feed.indicators.set(slot, demand);
  }, [feed, store, symbol, timeframe, slot, demand]);
  const indicatorsOn = useSyncExternalStore(
    useCallback((cb: () => void) => feed.indicators.subscribe(cb), [feed]),
    useCallback(() => feed.indicators.available(), [feed]),
  );
  const subscribe = useCallback((cb: () => void) => store.subscribe(cb), [store]);
  const series = useSyncExternalStore(subscribe, useCallback((): Series | undefined => store.getSeries(symbol, timeframe), [store, symbol, timeframe]));
  const clock = useSyncExternalStore(subscribe, useCallback((): Clock | null => store.getClock(), [store]));
  const rth = demand?.rth ?? false;
  const serverValues = useSyncExternalStore(subscribe, useCallback((): IndicatorValues | undefined => store.getIndicators(symbol, timeframe, rth), [store, symbol, timeframe, rth]));
  // null while the server computes them but has not answered yet: no lines, not a flash of local ones.
  const indicators = demand && indicatorsOn ? (serverValues ?? null) : undefined;
  return { series, tape, clock, indicators };
}

/** The feed's paper broker as the chart's `trading` prop. */
export function usePaperTrading(feed: MobileFeed): ChartTrading {
  const subscribe = useCallback((cb: () => void) => feed.onPaper(cb), [feed]);
  const state = useSyncExternalStore(subscribe, useCallback((): PaperState | null => feed.paperLatest(), [feed]));
  return useMemo(
    () => ({
      state,
      onPlace: (o) => feed.sendPaper({ type: "order.place", ...o }),
      onModify: (c) => feed.sendPaper({ type: "order.modify", ...c }),
      onCancel: (id) => feed.sendPaper({ type: "order.cancel", id }),
    }),
    [feed, state],
  );
}

const NOOP = () => {};

/**
 * The server's live quote board, only while `enabled` (the ticker search is open): subscribing
 * opts the connection in to quotes.* frames and closing the search opts it out, so the server
 * builds and sends nothing for a search nobody is looking at (the web's useQuoteBoard).
 */
export function useQuoteBoard(channel: QuoteChannel, enabled: boolean): QuoteBoard {
  const subscribe = useCallback((cb: () => void) => (enabled ? channel.subscribe(cb) : NOOP), [channel, enabled]);
  const get = useCallback(() => (enabled ? channel.get() : EMPTY_QUOTE_BOARD), [channel, enabled]);
  return useSyncExternalStore(subscribe, get);
}

/** The server's Ross Panel while mounted (the web's useRoss). */
export function useRoss(channel: RossChannel): RossState {
  const subscribe = useCallback((cb: () => void) => channel.subscribe(cb), [channel]);
  return useSyncExternalStore(subscribe, useCallback(() => channel.get(), [channel]));
}

/** The Favorites card's rows while mounted (the web's useFavorites). */
export function useFavorites(channel: FavoritesChannel): FavoritesState {
  const subscribe = useCallback((cb: () => void) => channel.subscribe(cb), [channel]);
  return useSyncExternalStore(subscribe, useCallback(() => channel.get(), [channel]));
}

/**
 * Top Gainers and Hot stocks Bull / Bear while the dashboard shows them (the web's useMovers):
 * the first listener opts the connection in to movers.* frames, the last opts it out.
 */
export function useMovers(channel: MoversChannel): MoversState {
  const subscribe = useCallback((cb: () => void) => channel.subscribe(cb), [channel]);
  return useSyncExternalStore(subscribe, useCallback(() => channel.get(), [channel]));
}

/** Rocket watch while mounted (the web's useRocket). */
export function useRocket(channel: RocketChannel): RocketState {
  const subscribe = useCallback((cb: () => void) => channel.subscribe(cb), [channel]);
  return useSyncExternalStore(subscribe, useCallback(() => channel.get(), [channel]));
}

/**
 * A chart's volume shelves (shared zones.ts) from the Runners channel's latest frame, the web's
 * useZones. It subscribes itself, so a chart gets its bands and caption whatever else is mounted
 * (the channel is ref-counted), and its snapshot changes only when this symbol's bands or note do,
 * so a Runners frame does not re-render the chart.
 */
export function useZones(channel: RocketChannel, symbol: string): ZoneView {
  const [pick] = useState(zoneSelector);
  const subscribe = useCallback((cb: () => void) => channel.subscribe(cb), [channel]);
  return useSyncExternalStore(subscribe, useCallback(() => pick(channel.get().panel, symbol), [channel, pick, symbol]));
}

/** Shares one authoritative channel between the dashboard, Ross links and chart modal. */
export function useDetection(channel: DetectionChannel): DetectionState {
  const subscribe = useCallback((cb: () => void) => channel.subscribe(cb), [channel]);
  return useSyncExternalStore(subscribe, useCallback(() => channel.get(), [channel]));
}

/** The server's Halt watcher board while mounted (the web's usePressureBoard). */
export function usePressureBoard(channel: PressureChannel): PressureBoard {
  const subscribe = useCallback((cb: () => void) => channel.subscribe(cb), [channel]);
  const get = useCallback(() => channel.get(), [channel]);
  return useSyncExternalStore(subscribe, get);
}

/** Status changes notify independently of the retained board, just like the web panel. */
export function usePressureStatus(channel: PressureChannel): PressureStatus {
  const subscribe = useCallback((cb: () => void) => channel.subscribe(cb), [channel]);
  const get = useCallback(() => channel.status(), [channel]);
  return useSyncExternalStore(subscribe, get);
}

export interface Candidates {
  symbols: string[];
  /** The server replays a fixture archive (not market data): the search may list mock rows. */
  fixture: boolean;
  /** Live: the trading day shown (the last session while the market is closed). */
  session: LiveSession | null;
}

/**
 * The server's price-band candidates, refreshed every 10 s (they change as prices move), and at
 * once when the replay's trading day (`day`, "YYYY-MM-DD") changes: after a day switch a symbol the
 * new day lacks is replaced right away instead of leaving the old day's chart up.
 */
export function useCandidates(token: string | null, day: string | null = null, source?: MarketSource): Candidates {
  const [state, setState] = useState<Candidates>({ symbols: [], fixture: false, session: null });
  useEffect(() => {
    let alive = true;
    const refresh = async () => {
      try {
        const res = await authenticatedFetch(`${SERVER_URL}${withMarketSource("/api/symbols", source)}`, token);
        if (!res.ok) return;
        const d = (await res.json()) as { symbols?: unknown; fixture?: unknown; session?: unknown };
        if (alive && Array.isArray(d.symbols)) setState({ symbols: d.symbols.filter((x): x is string => typeof x === "string"), fixture: d.fixture === true, session: parseLiveSession(d.session) });
      } catch {
        // Offline or server down: keep the last list; the socket status says so.
      }
    };
    void refresh();
    const id = setInterval(() => void refresh(), 10_000);
    return () => {
      alive = false;
      clearInterval(id);
    };
  }, [token, day, source]);
  return state;
}

/**
 * The replay day picker's data: GET /api/replay/state's `day` and `days` (the web's
 * useReplayDays). Fetched, not pushed (the list changes a few times a month): on start, when the
 * sheet opens (`refresh`), and when the clock's trading day changes, which is how a switch made
 * on another device reaches this one's label.
 */
export function useReplayDays(token: string | null, clockDay: string | null): { state: ReplayDayState | null; refresh: () => void } {
  const [state, setState] = useState<ReplayDayState | null>(null);
  // Only the latest request may set the state: answers can arrive out of order.
  const latest = useRef(0);
  const refresh = useCallback(() => {
    const id = ++latest.current;
    // The replay market's days: only the chart tab (the practice feature) has a day picker.
    void authenticatedFetch(`${SERVER_URL}/api/replay/state?source=replay`, token)
      .then((res) => (res.ok ? res.json() : null))
      .then((body: unknown) => {
        if (id === latest.current && body) setState(parseReplayDayState(body));
      })
      .catch(() => {
        // Offline: keep the last list; the socket status says so.
      });
  }, [token]);
  useEffect(() => refresh(), [refresh, clockDay]);
  return { state, refresh };
}

const NO_ALERTS: readonly ReplayAlert[] = [];

/**
 * The replay bar's alert dots: GET /api/replay/alerts?day= (the web's useReplayAlerts), every
 * Runners entry the live server logged that day, all tickers. Once per day: a past day's log is fixed.
 */
export function useReplayAlerts(token: string | null, day: string | null): readonly ReplayAlert[] {
  const [got, setGot] = useState<{ day: string; alerts: readonly ReplayAlert[] } | null>(null);
  useEffect(() => {
    if (!day) return;
    let live = true;
    void authenticatedFetch(`${SERVER_URL}/api/replay/alerts?day=${encodeURIComponent(day)}`, token)
      .then((res) => (res.ok ? res.json() : null))
      .then((body: unknown) => {
        const parsed = parseReplayAlerts(body);
        if (live && parsed) setGot(parsed);
      })
      .catch(() => {
        // Offline or an older server: no dots, the alert jumps disable.
      });
    return () => {
      live = false;
    };
  }, [token, day]);
  return got && got.day === day ? got.alerts : NO_ALERTS;
}

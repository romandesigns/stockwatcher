import { ChartStore, frameScheduler, sameIndicatorDemand, type ChartLevels, type ChartMessage, type ChartZones, type Clock, type IndicatorDemand, type IndicatorValues, type Series, type TapeTrade, type Timeframe } from "@stockwatcher/chart-core";
import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, useSyncExternalStore } from "react";
import type { ChartFeed } from "./types.ts";

const TAPE_MAX = 200;

/**
 * Binds a feed subscription to a chart-core ChartStore and a tape buffer.
 * Changing the timeframe or bin re-subscribes; each timeframe keeps its own
 * series in the store, so switching never mixes candle sets.
 */
/**
 * `slot` names the chart on a connection that carries several (the dashboard's panel: `d0`…`d3`);
 * omitted, it is the single default slot (the chart page).
 *
 * `demand` is what the chart shows of its indicators (chart-core `indicatorDemand`). When the
 * server computes them (`indicators.v1`) it goes on the subscription and a change goes out as
 * `chart.indicators.set` (no new candle snapshot), and `indicators` is the server's values for
 * the chart to draw. Against a server from before the feature `indicators` is undefined: the chart
 * draws no lines and says the server needs an update (the client's own math is gone). Before the
 * first welcome it is null, like a pending answer, so the note never flashes while connecting.
 *
 * `levels` and `zones` follow the same rule (`levels.v1`, `zones.v1`): the server's `chart.levels`
 * and, while `demand.zones` (the zones layer) is on, its `chart.zones` for the demand's rth; null
 * while asked and not answered yet; undefined when the chart must read them off its own candles.
 */
export function useChartFeed(feed: ChartFeed, symbol: string, timeframe: Timeframe, profileBin: number, slot?: string, demand?: IndicatorDemand) {
  // One notification per display frame (chart-core `frameScheduler`): a candle frame and the
  // indicator, levels and zones frames that follow it render together, once, instead of once per
  // socket message with the candle drawn over the previous values in between.
  const store = useMemo(() => new ChartStore({ maxCandles: 5000, schedule: frameScheduler }), []);
  useEffect(() => () => store.dispose(), [store]);
  const [tape, setTape] = useState<TapeTrade[]>([]);
  // Read by the subscribe effect without re-subscribing on every toggle; `sent` is what the
  // server last heard for this slot, so the demand effect only sends a real change.
  const demandRef = useRef(demand);
  // A layout effect runs before every passive effect of the commit, so the subscribe below reads this render's demand.
  useLayoutEffect(() => {
    demandRef.current = demand;
  });
  const sent = useRef<IndicatorDemand | undefined>(undefined);

  useEffect(() => {
    let pending: TapeTrade[] = [];
    let frame = 0;
    // Tape prints arrive in bursts at high replay speed: batch per frame.
    const flush = () => {
      frame = 0;
      const batch = pending;
      pending = [];
      setTape((prev) => [...batch.reverse(), ...prev].slice(0, TAPE_MAX));
    };
    const onMessage = (m: ChartMessage) => {
      if (m.type === "chart.snapshot") {
        // A snapshot restarts the subscription, tape history included.
        pending = [];
        setTape([]);
        store.applySnapshot(m);
      } else if (m.type === "chart.update" || m.type === "chart.remove" || m.type === "chart.gap") {
        store.applyIncrement(m);
      } else if (m.type === "replay.clock") {
        store.applyClock(m.clock);
      } else if (m.type === "chart.halt" || m.type === "chart.luld") {
        store.applySymbolEvent(m);
      } else if (m.type === "chart.indicators") {
        store.applyIndicators(m);
      } else if (m.type === "chart.indicator") {
        store.applyIndicatorIncrement(m);
      } else if (m.type === "chart.levels") {
        store.applyLevels(m);
      } else if (m.type === "chart.zones") {
        store.applyZones(m);
      } else if (m.symbol === symbol) {
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
    // Extended hours flipped: the store's values for the other rth are the ones frozen when this
    // chart last showed them (ChartStore `dropIndicators`); draw nothing until the answer instead.
    if (sent.current && sent.current.rth !== demand.rth) store.dropIndicators(symbol, timeframe, demand.rth);
    // Likewise the zones, when the layer comes back on or their rth flips.
    if (demand.zones && sent.current && (!sent.current.zones || sent.current.rth !== demand.rth)) store.dropZones(symbol, timeframe, demand.rth);
    sent.current = demand;
    feed.indicators?.set(slot, demand);
  }, [feed, store, symbol, timeframe, slot, demand]);

  const indicatorsOn = useSyncExternalStore(
    useCallback((cb: () => void) => feed.indicators?.subscribe(cb) ?? (() => {}), [feed]),
    useCallback(() => feed.indicators?.available() ?? false, [feed]),
  );
  const indicatorsKnown = useSyncExternalStore(
    useCallback((cb: () => void) => feed.indicators?.subscribe(cb) ?? (() => {}), [feed]),
    useCallback(() => feed.indicators?.known() ?? true, [feed]),
  );

  const subscribe = useCallback((cb: () => void) => store.subscribe(cb), [store]);
  const getSeries = useCallback((): Series | undefined => store.getSeries(symbol, timeframe), [store, symbol, timeframe]);
  const series = useSyncExternalStore(subscribe, getSeries);
  // A new ticker's tape is empty until its snapshot clears the buffer: the previous ticker's prints
  // never show under the new one meanwhile (the newest print says whose the buffer is).
  const shownTape = useMemo(() => (tape[0] && tape[0].symbol !== symbol ? [] : tape), [tape, symbol]);
  const clock = useSyncExternalStore(subscribe, useCallback((): Clock | null => store.getClock(), [store]));
  const rth = demand?.rth ?? false;
  const serverValues = useSyncExternalStore(subscribe, useCallback((): IndicatorValues | undefined => store.getIndicators(symbol, timeframe, rth), [store, symbol, timeframe, rth]));
  // null (not undefined) while the server computes them but has not answered yet, or before the
  // first welcome says whether it does: no lines and no note. undefined = it computes none.
  const indicators = !demand ? undefined : indicatorsOn ? (serverValues ?? null) : indicatorsKnown ? undefined : null;
  const levelsOn = useSyncExternalStore(
    useCallback((cb: () => void) => feed.indicators?.subscribe(cb) ?? (() => {}), [feed]),
    useCallback(() => feed.indicators?.levels() ?? false, [feed]),
  );
  const zonesOn = useSyncExternalStore(
    useCallback((cb: () => void) => feed.indicators?.subscribe(cb) ?? (() => {}), [feed]),
    useCallback(() => feed.indicators?.zones() ?? false, [feed]),
  );
  const serverLevels = useSyncExternalStore(subscribe, useCallback((): ChartLevels | undefined => store.getLevels(symbol, timeframe), [store, symbol, timeframe]));
  const serverZones = useSyncExternalStore(subscribe, useCallback((): ChartZones | undefined => store.getZones(symbol, timeframe, rth), [store, symbol, timeframe, rth]));
  const levels = levelsOn ? (serverLevels ?? null) : undefined;
  const zones = zonesOn && demand?.zones ? (serverZones?.zones ?? null) : undefined;
  return { series, tape: shownTape, clock, indicators, levels, zones };
}

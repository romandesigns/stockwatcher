import { sameIndicatorDemand, type IndicatorDemand } from "./indicators.ts";
import type { ChartIndicatorsSet, ChartSubscribe } from "./types.ts";

// The feed side of server-computed indicators (`indicators.v1`, crates/server/src/indicators.rs),
// shared by the web and phone feeds so both speak it alike. It holds each slot's latest demand
// whether or not the server offers the feature, so a resubscribe (a reconnect, maybe to a newer
// server) carries it; only a welcome that lists the feature lets it onto the wire. An older
// server would ignore the fields anyway, but then the client must keep drawing its own lines,
// which `available()` false tells the chart.

/** What a chart host reads and drives. */
export interface ServerIndicatorsChannel {
  /** Whether the server of the latest welcome computes them (false before the first welcome). */
  available(): boolean;
  /** Called when `available()` changes (a reconnect may meet another server version). */
  subscribe(fn: () => void): () => void;
  /** A held slot's demand changed: `chart.indicators.set` when available; kept for the next resubscribe either way. */
  set(id: string | undefined, demand: IndicatorDemand): void;
}

export interface ServerIndicators {
  readonly channel: ServerIndicatorsChannel;
  /** From the feed's welcome handler, BEFORE it resubscribes. */
  setAvailable(on: boolean): void;
  /** The optional `chart.subscribe` fields for slot `id` ("" = the default slot). */
  subscribeFields(id: string): Pick<ChartSubscribe, "indicators" | "rth">;
  /** A slot (re)subscribed with this demand (undefined = the host computes its own). */
  held(id: string, demand: IndicatorDemand | undefined): void;
  released(id: string): void;
  clear(): void;
}

/** `send` writes a frame when the socket is open (and drops it otherwise: the resubscribe heals it); `isHeld` says whether slot `id` is subscribed. */
export function createServerIndicators(send: (frame: ChartIndicatorsSet) => void, isHeld: (id: string) => boolean): ServerIndicators {
  let on = false;
  const listeners = new Set<() => void>();
  const demands = new Map<string, IndicatorDemand>();
  return {
    channel: {
      available: () => on,
      subscribe(fn) {
        listeners.add(fn);
        return () => {
          listeners.delete(fn);
        };
      },
      set(id, demand) {
        const key = id ?? "";
        const prev = demands.get(key);
        demands.set(key, demand);
        if (prev && sameIndicatorDemand(prev, demand)) return;
        // The server answers an id it does not hold with an error: a slot not subscribed yet
        // sends its demand on chart.subscribe instead.
        if (on && isHeld(key)) send({ type: "chart.indicators.set", ...(key ? { id: key } : {}), indicators: demand.indicators, rth: demand.rth });
      },
    },
    setAvailable(next) {
      if (next === on) return;
      on = next;
      for (const l of listeners) l();
    },
    subscribeFields(id) {
      const d = demands.get(id);
      return on && d ? { indicators: d.indicators, rth: d.rth } : {};
    },
    held(id, demand) {
      if (demand) demands.set(id, demand);
      else demands.delete(id);
    },
    released(id) {
      demands.delete(id);
    },
    clear() {
      demands.clear();
      listeners.clear();
    },
  };
}

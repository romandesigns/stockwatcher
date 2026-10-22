// A chart's volume shelves (shared zones.ts), read from the Runners channel's latest frame. It
// subscribes to the channel itself, so a chart gets its bands and caption whether or not the
// Runners tab is open (the channel is ref-counted: the first listener opts the socket in, the last
// opts it out). The snapshot is memoised per chart, so a Runners frame that changes nothing for
// this symbol does not re-render the chart.
import { EMPTY_ZONE_VIEW, zoneSelector, type RocketChannel, type ZoneView } from "@stockwatcher/shared";
import { useCallback, useState, useSyncExternalStore } from "react";

const NOOP = () => {};

export function useZones(channel: RocketChannel | undefined, symbol: string): ZoneView {
  const [pick] = useState(zoneSelector);
  const subscribe = useCallback((cb: () => void) => (channel ? channel.subscribe(cb) : NOOP), [channel]);
  const get = useCallback(() => (channel ? pick(channel.get().panel, symbol) : EMPTY_ZONE_VIEW), [channel, pick, symbol]);
  return useSyncExternalStore(subscribe, get, get);
}

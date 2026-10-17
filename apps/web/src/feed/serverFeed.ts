import { parseChartMessage, type ChartMessage, type ChartSubscribe, type ReplayControl } from "@stockwatcher/chart-core";
import { createWsClient, sameOriginWsUrl, type ConnectionStatus, type WebSocketLike } from "@stockwatcher/shared";
import type { ChartFeed, ChartSubscription, ReplayCommand } from "./types.ts";

export interface ServerFeedOptions {
  url?: string;
  token?: string | null;
  onStatus?: (s: ConnectionStatus) => void;
}

/**
 * The real feed and the default source: the server's `/ws` (chart-engine
 * output from the replay or live provider), via @stockwatcher/shared's client
 * (hello + token, heartbeat, backoff). Replay controls go to the server as
 * `replay.control {action, value}`; the server answers with `replay.clock`.
 *
 * shared's parseServerMessage only knows hello/welcome/ping/pong/error and
 * drops everything else, so this wraps the socket and tees raw frames to
 * chart-core's parser. When the protocol is frozen, chart.* types belong in
 * shared's ServerMessage union and this tee can go.
 */
export function createServerFeed(opts: ServerFeedOptions = {}): ChartFeed {
  const listeners = new Set<(m: ChartMessage) => void>();
  const active = new Set<ChartSubscription>();
  let raw: WebSocketLike | null = null;

  const sendRaw = (msg: ChartSubscribe | ReplayControl) => {
    if (raw && raw.readyState === 1) raw.send(JSON.stringify(msg));
  };
  const resubscribe = () => {
    for (const s of active) sendRaw({ type: "chart.subscribe", symbol: s.symbol, timeframe: s.timeframe, profileBin: s.profileBin });
  };

  const client = createWsClient({
    url: opts.url ?? sameOriginWsUrl(window.location),
    token: opts.token ?? null,
    client: "web",
    onStatus: (s) => {
      opts.onStatus?.(s);
      if (s === "open") resubscribe(); // subscriptions do not survive a reconnect
    },
    createSocket: (url) => {
      const socket = new WebSocket(url);
      // Chart frames go to chart listeners; everything else to whatever
      // handler the shared client assigns to onmessage. The own-property
      // accessor shadows the native one, so the browser never calls it.
      let inner: WebSocketLike["onmessage"] = null;
      socket.addEventListener("message", (ev) => {
        const chart = parseChartMessage(ev.data);
        if (chart) for (const l of listeners) l(chart);
        else inner?.({ data: ev.data });
      });
      Object.defineProperty(socket, "onmessage", { configurable: true, get: () => inner, set: (fn: WebSocketLike["onmessage"]) => (inner = fn) });
      raw = socket as unknown as WebSocketLike;
      return raw;
    },
  });

  return {
    kind: "server",
    subscribe(sub, emit) {
      listeners.add(emit);
      active.add(sub);
      sendRaw({ type: "chart.subscribe", symbol: sub.symbol, timeframe: sub.timeframe, profileBin: sub.profileBin });
      return () => {
        listeners.delete(emit);
        active.delete(sub);
      };
    },
    control(cmd: ReplayCommand) {
      sendRaw({ type: "replay.control", ...cmd });
    },
    close() {
      client.close();
      listeners.clear();
      active.clear();
    },
  };
}

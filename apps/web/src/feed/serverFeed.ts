import { parseChartMessage, parsePaperMessage, type ChartMessage, type ChartSubscribe, type PaperMessage, type PaperState, type ReplayControl } from "@stockwatcher/chart-core";
import { createQuoteChannel, createWsClient, sameOriginWsUrl, type ConnectionStatus, type SettingsSync, type WebSocketLike } from "@stockwatcher/shared";
import type { ChartFeed, ChartSubscription, PaperCommand, ReplayCommand } from "./types.ts";

export interface ServerFeedOptions {
  url?: string;
  token?: string | null;
  onStatus?: (s: ConnectionStatus) => void;
  /** The page's synced settings (settings/sync.ts): attached on every welcome by the shared client. */
  settings?: SettingsSync;
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
 * shared's ServerMessage union and this tee can go. quotes.* frames are
 * shared's own and go straight through to it (opted in only while a ticker
 * search is open: see `quotes`).
 */
export function createServerFeed(opts: ServerFeedOptions = {}): ChartFeed {
  const listeners = new Set<(m: ChartMessage) => void>();
  const active = new Set<ChartSubscription>();
  let raw: WebSocketLike | null = null;
  const paperListeners = new Set<(m: PaperMessage) => void>();
  let paperState: PaperState | null = null;

  // Late-bound: the channel is built before the client it drives.
  const quotes = createQuoteChannel({ subscribe: () => client.subscribeQuotes(), unsubscribe: () => client.unsubscribeQuotes() });

  const sendRaw = (msg: ChartSubscribe | ReplayControl | PaperCommand) => {
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
    onQuotes: (m) => quotes.apply(m),
    settings: opts.settings,
    createSocket: (url) => {
      const socket = new WebSocket(url);
      // Chart frames go to chart listeners; everything else to whatever
      // handler the shared client assigns to onmessage. The own-property
      // accessor shadows the native one, so the browser never calls it.
      let inner: WebSocketLike["onmessage"] = null;
      socket.addEventListener("message", (ev) => {
        // A quotes.snapshot is hundreds of KB: hand it to shared's parser without first
        // parsing it twice as a chart or paper frame. The server writes `type` first. The
        // settings.* frames (shared's own too) take the same shortcut.
        if (typeof ev.data === "string" && (ev.data.startsWith('{"type":"quotes.') || ev.data.startsWith('{"type":"settings.'))) {
          inner?.({ data: ev.data });
          return;
        }
        const chart = parseChartMessage(ev.data);
        if (chart) {
          for (const l of listeners) l(chart);
          return;
        }
        const paper = parsePaperMessage(ev.data);
        if (paper) {
          if (paper.type === "paper.state") paperState = paper;
          for (const l of paperListeners) l(paper);
          return;
        }
        inner?.({ data: ev.data });
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
    paper: {
      latest: () => paperState,
      subscribe(fn) {
        paperListeners.add(fn);
        return () => paperListeners.delete(fn);
      },
      send: (cmd) => sendRaw(cmd),
    },
    quotes,
    close() {
      client.close();
      listeners.clear();
      active.clear();
      paperListeners.clear();
    },
  };
}

import {
  parseChartMessage,
  parsePaperMessage,
  type ChartMessage,
  type ChartSubscribe,
  type OrderCancel,
  type OrderModify,
  type OrderPlace,
  type PaperMessage,
  type PaperState,
  type ReplayControl,
  type Timeframe,
} from "@stockwatcher/chart-core";
import { createQuoteChannel, createWsClient, type ConnectionStatus, type QuoteChannel, type WebSocketLike } from "@stockwatcher/shared";

import { wsUrl } from "./server";

export interface ChartSubscription {
  symbol: string;
  timeframe: Timeframe;
  profileBin: number;
}

export type PaperCommand = OrderPlace | OrderModify | OrderCancel | { type: "paper.reset" };

export interface MobileFeed {
  subscribe(sub: ChartSubscription, emit: (m: ChartMessage) => void): () => void;
  control(cmd: Omit<ReplayControl, "type">): void;
  paperLatest(): PaperState | null;
  onPaper(fn: (m: PaperMessage) => void): () => void;
  sendPaper(cmd: PaperCommand): void;
  /** The server's live quote board for the ticker search; subscribing opts the connection in. */
  quotes: QuoteChannel;
  close(): void;
}

/**
 * The phone's connection to the server's /ws: the same protocol as the web
 * app's feed (apps/web/src/feed/serverFeed.ts). shared's client does hello +
 * token, heartbeat and backoff; chart.* and paper.* frames are teed off the
 * raw socket because shared's parser only knows the handshake messages;
 * quotes.* frames are shared's own and pass straight through to it.
 */
export function createMobileFeed(token: string | null, onStatus?: (s: ConnectionStatus) => void): MobileFeed {
  const listeners = new Set<(m: ChartMessage) => void>();
  const paperListeners = new Set<(m: PaperMessage) => void>();
  const active = new Set<ChartSubscription>();
  let raw: WebSocketLike | null = null;
  let paper: PaperState | null = null;

  // Late-bound: the channel is built before the client it drives.
  const quotes = createQuoteChannel({ subscribe: () => client.subscribeQuotes(), unsubscribe: () => client.unsubscribeQuotes() });

  const sendRaw = (msg: ChartSubscribe | ReplayControl | PaperCommand) => {
    if (raw && raw.readyState === 1) raw.send(JSON.stringify(msg));
  };

  const client = createWsClient({
    url: wsUrl(),
    token,
    client: "mobile",
    onStatus: (s) => {
      onStatus?.(s);
      // Subscriptions do not survive a reconnect.
      if (s === "open") for (const a of active) sendRaw({ type: "chart.subscribe", ...a });
    },
    onQuotes: (m) => quotes.apply(m),
    createSocket: (url) => {
      const socket = new WebSocket(url);
      let inner: WebSocketLike["onmessage"] = null;
      socket.addEventListener("message", (ev: { data: unknown }) => {
        // A quotes.snapshot is hundreds of KB: skip the chart/paper parsers (the server writes
        // `type` first), which would each JSON.parse it again on the phone's JS thread.
        if (typeof ev.data === "string" && ev.data.startsWith('{"type":"quotes.')) {
          inner?.({ data: ev.data });
          return;
        }
        const chart = parseChartMessage(ev.data);
        if (chart) {
          for (const l of listeners) l(chart);
          return;
        }
        const pm = parsePaperMessage(ev.data);
        if (pm) {
          if (pm.type === "paper.state") paper = pm;
          for (const l of paperListeners) l(pm);
          return;
        }
        inner?.({ data: ev.data as string });
      });
      // The shared client assigns onmessage; this accessor keeps that
      // handler for non-chart frames instead of letting it replace the tee.
      Object.defineProperty(socket, "onmessage", { configurable: true, get: () => inner, set: (fn: WebSocketLike["onmessage"]) => (inner = fn) });
      raw = socket as unknown as WebSocketLike;
      return raw;
    },
  });

  return {
    subscribe(sub, emit) {
      listeners.add(emit);
      active.add(sub);
      sendRaw({ type: "chart.subscribe", ...sub });
      return () => {
        listeners.delete(emit);
        active.delete(sub);
      };
    },
    control: (cmd) => sendRaw({ type: "replay.control", ...cmd }),
    paperLatest: () => paper,
    onPaper(fn) {
      paperListeners.add(fn);
      return () => {
        paperListeners.delete(fn);
      };
    },
    sendPaper: (cmd) => sendRaw(cmd),
    quotes,
    close() {
      client.close();
      listeners.clear();
      paperListeners.clear();
      active.clear();
    },
  };
}

import {
  createChartRouter,
  parseChartMessage,
  parsePaperMessage,
  type ChartMessage,
  type ChartSubscribe,
  type ChartUnsubscribe,
  type OrderCancel,
  type OrderModify,
  type OrderPlace,
  type PaperMessage,
  type PaperState,
  type ReplayControl,
  type Timeframe,
} from "@stockwatcher/chart-core";
import { createPressureChannel, createQuoteChannel, createWsClient, type AlertsServerMessage, type ConnectionStatus, type MarketSource, type PressureChannel, type QuoteChannel, type SettingsSync, type WebSocketLike } from "@stockwatcher/shared";

import { deviceInfo, pushToken } from "../notifications/push";
import { wsUrl } from "./server";

export interface ChartSubscription {
  symbol: string;
  timeframe: Timeframe;
  profileBin: number;
  /** The chart slot (the dashboard's panel, up to 4 per connection); absent = the default slot. */
  id?: string;
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
  /** The server's Halt watcher board (the dashboard's gauges); subscribing opts the connection in. */
  pressure: PressureChannel;
  close(): void;
}

/**
 * The phone's connection to the server's /ws: the same protocol as the web
 * app's feed (apps/web/src/feed/serverFeed.ts). shared's client does hello +
 * token, heartbeat and backoff; chart.* and paper.* frames are teed off the
 * raw socket because shared's parser only knows the handshake messages;
 * quotes.* frames are shared's own and pass straight through to it.
 */
export function createMobileFeed(token: string | null, onStatus?: (s: ConnectionStatus) => void, settings?: SettingsSync, clientName = "mobile", source?: MarketSource, onAlerts?: (m: AlertsServerMessage) => void): MobileFeed {
  // Chart frames go only to the slots they belong to (chart-core chartRouter.ts), as on the web.
  const router = createChartRouter();
  const current = new Map<string, (m: ChartMessage) => void>();
  const paperListeners = new Set<(m: PaperMessage) => void>();
  let raw: WebSocketLike | null = null;
  let paper: PaperState | null = null;

  // Late-bound: the channel is built before the client it drives.
  const quotes = createQuoteChannel({ subscribe: () => client.subscribeQuotes(), unsubscribe: () => client.unsubscribeQuotes() });
  const pressure = createPressureChannel({ subscribe: () => client.subscribePressure(), unsubscribe: () => client.unsubscribePressure() });

  const sendRaw = (msg: ChartSubscribe | ChartUnsubscribe | ReplayControl | PaperCommand) => {
    if (raw && raw.readyState === 1) raw.send(JSON.stringify(msg));
  };
  // The default slot ("") goes without an id: exactly the single-chart protocol.
  const subscribeFrame = (id: string, s: Omit<ChartSubscription, "id">): ChartSubscribe => ({ type: "chart.subscribe", symbol: s.symbol, timeframe: s.timeframe, profileBin: s.profileBin, ...(id ? { id } : {}) });

  const client = createWsClient({
    url: wsUrl(),
    token,
    // Which screen's socket this is, in the server's connect log ("mobile", "mobile-dashboard").
    client: clientName,
    // Which server market: the dashboard tab reads live, the chart tab replay.
    source,
    onStatus: (s) => {
      onStatus?.(s);
      // Subscriptions do not survive a reconnect.
      if (s === "open") {
        for (const [id, a] of router.entries()) sendRaw(subscribeFrame(id, a));
        // This phone's push token, on every connection (idempotent; heals a lost or rotated one).
        const t = pushToken();
        if (t) client.send({ type: "push.register", token: t, ...deviceInfo() });
      }
    },
    onQuotes: (m) => quotes.apply(m),
    onPressure: (m) => pressure.apply(m),
    // Price alerts (Activity, fires): the app store takes them whatever tab is open.
    onAlerts,
    // Synced settings (src/settings/sync.ts): attached on every welcome by the shared client.
    settings,
    createSocket: (url) => {
      const socket = new WebSocket(url);
      let inner: WebSocketLike["onmessage"] = null;
      socket.addEventListener("message", (ev: { data: unknown }) => {
        // A quotes.snapshot is hundreds of KB: skip the chart/paper parsers (the server writes
        // `type` first), which would each JSON.parse it again on the phone's JS thread. The
        // settings.* and pressure.* frames are shared's own too and take the same shortcut.
        if (typeof ev.data === "string" && (ev.data.startsWith('{"type":"quotes.') || ev.data.startsWith('{"type":"settings.') || ev.data.startsWith('{"type":"pressure.') || ev.data.startsWith('{"type":"alert'))) {
          inner?.({ data: ev.data });
          return;
        }
        const chart = parseChartMessage(ev.data);
        if (chart) {
          router.dispatch(chart);
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
      const id = sub.id ?? "";
      // A token per subscription, so a stale cleanup never removes the slot's newer subscription.
      const token = (m: ChartMessage) => emit(m);
      router.add(id, sub, token);
      current.set(id, token);
      sendRaw(subscribeFrame(id, sub));
      return () => {
        if (current.get(id) !== token) return;
        current.delete(id);
        router.remove(id);
        if (id) sendRaw({ type: "chart.unsubscribe", id });
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
    pressure,
    close() {
      client.close();
      router.clear();
      current.clear();
      paperListeners.clear();
    },
  };
}

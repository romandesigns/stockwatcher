import {
  createChartRouter,
  createServerIndicators,
  parseChartMessage,
  parsePaperMessage,
  type ChartIndicatorsSet,
  type ChartMessage,
  type ChartSubscribe,
  type ChartUnsubscribe,
  type IndicatorDemand,
  type OrderCancel,
  type OrderModify,
  type OrderPlace,
  type PaperMessage,
  type PaperState,
  type ReplayControl,
  type ServerIndicatorsChannel,
  type Timeframe,
} from "@stockwatcher/chart-core";
import { createDetectionChannel, createFavoritesChannel, createMoversChannel, createPressureChannel, createQuoteChannel, createRocketChannel, createRossChannel, createNoticesChannel, createSymbolsChannel, createWsClient, hasFeature, INDICATORS_FEATURE, NOTICES_FEATURE, SYMBOLS_FEATURE, type AlertsServerMessage, type ConnectionStatus, type DetectionChannel, type FavoritesChannel, type MarketSource, type MoversChannel, type PressureChannel, type QuoteChannel, type RocketChannel, type RossChannel, type NoticesChannel, type SettingsSync, type SymbolsChannel, type WebSocketLike } from "@stockwatcher/shared";

import { deviceInfo, pushToken, subscribePushToken } from "../notifications/push";
import { runnersRegisterFields, subscribeRunnersPush } from "../notifications/runnersPush";
import { wsUrl } from "./server";

export interface ChartSubscription {
  symbol: string;
  timeframe: Timeframe;
  profileBin: number;
  /** The chart slot (the dashboard's panel, up to 4 per connection); absent = the default slot. */
  id?: string;
  /**
   * What the chart wants the server to compute (`indicators.v1`; chart-core `indicatorDemand`),
   * sent on `chart.subscribe` only when the server advertised it. Pure JS on purpose: it reaches
   * phones as an OTA update, so it must need no native module.
   */
  indicators?: IndicatorDemand;
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
  ross: RossChannel;
  /** The Favorites card's rows (the list is the synced `favorites` setting); subscribing opts in. */
  favorites: FavoritesChannel;
  /**
   * Top Gainers and Hot stocks Bull / Bear, ranked on the server once a tick (`movers.*`,
   * crates/server/src/movers.rs); subscribing opts in. Never the whole quote board for them.
   */
  movers: MoversChannel;
  /**
   * The price-band candidate list, pushed by a server with `symbols.v1` (crates/server/src/
   * symbols.rs); `useCandidates` polls `/api/symbols` without it, as every older APK does.
   */
  symbols: SymbolsChannel;
  /**
   * Today's notification log, pushed by a server with `notices.v1` (crates/server/src/notices.rs);
   * the Insights tab polls `/api/notices` without it, as every older APK does.
   */
  notices: NoticesChannel;
  /** Rocket watch (experimental): flags of a likely +30 % leg; subscribing opts the connection in. */
  rocket: RocketChannel;
  /** Independent authoritative event discovery; it does not subscribe to chart bars. */
  detection: DetectionChannel;
  /**
   * Chart indicators computed by the server (`indicators.v1`). Against a server without it the
   * chart keeps computing them itself, as every APK installed before it still does.
   */
  indicators: ServerIndicatorsChannel;
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
  const ross = createRossChannel({ subscribe: () => client.subscribeRoss(), unsubscribe: () => client.unsubscribeRoss() });
  const favorites = createFavoritesChannel({ subscribe: () => client.subscribeFavorites(), unsubscribe: () => client.unsubscribeFavorites() });
  const movers = createMoversChannel({ subscribe: () => client.subscribeMovers(), unsubscribe: () => client.unsubscribeMovers() });
  const symbols = createSymbolsChannel({ subscribe: () => client.subscribeSymbols(), unsubscribe: () => client.unsubscribeSymbols() });
  const notices = createNoticesChannel({ subscribe: () => client.subscribeNotices(), unsubscribe: () => client.unsubscribeNotices() });
  const rocket = createRocketChannel({ subscribe: () => client.subscribeRocket(), unsubscribe: () => client.unsubscribeRocket() });
  const detection = createDetectionChannel({ subscribe: () => client.subscribeDetection(), unsubscribe: () => client.unsubscribeDetection(), resync: () => client.subscribeDetection(), source });

  const sendRaw = (msg: ChartSubscribe | ChartUnsubscribe | ChartIndicatorsSet | ReplayControl | PaperCommand) => {
    if (raw && raw.readyState === 1) raw.send(JSON.stringify(msg));
  };
  // Server-computed indicators (indicators.v1): chart-core serverIndicators.ts, as on the web.
  const serverIndicators = createServerIndicators(sendRaw, (id) => current.has(id));
  // The default slot ("") goes without an id: exactly the single-chart protocol.
  const subscribeFrame = (id: string, s: Pick<ChartSubscription, "symbol" | "timeframe" | "profileBin">): ChartSubscribe => ({ type: "chart.subscribe", symbol: s.symbol, timeframe: s.timeframe, profileBin: s.profileBin, ...(id ? { id } : {}), ...serverIndicators.subscribeFields(id) });

  // This phone's push token and its Runners opt-ins (src/notifications/runnersPush.ts): on every
  // connection, and again when either changes while connected (a switch in Settings → Runners,
  // phone alerts turned on), so the server's copy follows at once.
  let open = false;
  const registerPush = () => {
    const t = pushToken();
    if (open && t) client.send({ type: "push.register", token: t, ...deviceInfo(), ...runnersRegisterFields() });
  };
  const unsubPush = [subscribePushToken(registerPush), subscribeRunnersPush(registerPush)];

  const client = createWsClient({
    url: wsUrl(),
    token,
    // Which screen's socket this is, in the server's connect log ("mobile", "mobile-dashboard").
    client: clientName,
    // Which server market: the dashboard tab reads live, the chart tab replay.
    source,
    // Runs before onStatus("open") and its resubscribe, so those carry the indicators or not.
    onWelcome: (w) => {
      serverIndicators.setAvailable(hasFeature(w, INDICATORS_FEATURE));
      symbols.setAvailable(hasFeature(w, SYMBOLS_FEATURE));
      notices.setAvailable(hasFeature(w, NOTICES_FEATURE));
    },
    onStatus: (s) => {
      onStatus?.(s);
      open = s === "open";
      // The Ross Panel keeps its last rows while the socket is down, so it has to be told: a
      // panel nobody is updating must not read as live (shared/ross.ts `rossFreshness`).
      ross.setConnected(s === "open");
      favorites.setConnected(s === "open");
      movers.setConnected(s === "open");
      symbols.setConnected(s === "open");
      notices.setConnected(s === "open");
      pressure.setConnected(s === "open");
      rocket.setConnected(s === "open");
      detection.setConnected(s === "open");
      // Subscriptions do not survive a reconnect.
      if (s === "open") {
        for (const [id, a] of router.entries()) sendRaw(subscribeFrame(id, a));
        // This phone's push token, on every connection (idempotent; heals a lost or rotated one).
        registerPush();
      }
    },
    onQuotes: (m) => quotes.apply(m),
    onPressure: (m) => pressure.apply(m),
    onRoss: (m) => ross.apply(m),
    onFavorites: (m) => favorites.apply(m),
    onMovers: (m) => movers.apply(m),
    onSymbols: (m) => symbols.apply(m),
    onNotices: (m) => notices.apply(m),
    onRocket: (m) => rocket.apply(m),
    onDetection: (m) => detection.apply(m),
    onDetectionError: (reason) => detection.reject(reason),
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
        if (typeof ev.data === "string" && (ev.data.startsWith('{"type":"quotes.') || ev.data.startsWith('{"type":"settings.') || ev.data.startsWith('{"type":"pressure.') || ev.data.startsWith('{"type":"ross.') || ev.data.startsWith('{"type":"favorites.') || ev.data.startsWith('{"type":"movers.') || ev.data.startsWith('{"type":"symbols.') || ev.data.startsWith('{"type":"notices.') || ev.data.startsWith('{"type":"rocket.') || ev.data.startsWith('{"type":"detection.') || ev.data.startsWith('{"type":"alert'))) {
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
      serverIndicators.held(id, sub.indicators);
      sendRaw(subscribeFrame(id, sub));
      return () => {
        if (current.get(id) !== token) return;
        current.delete(id);
        router.remove(id);
        serverIndicators.released(id);
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
    ross,
    favorites,
    movers,
    symbols,
    notices,
    rocket,
    detection,
    indicators: serverIndicators.channel,
    close() {
      for (const u of unsubPush) u();
      client.close();
      router.clear();
      current.clear();
      serverIndicators.clear();
      paperListeners.clear();
    },
  };
}

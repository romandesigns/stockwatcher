import { createChartRouter, createSnapshotWatch, parseChartMessage, parsePaperMessage, createServerIndicators, type ChartIndicatorsSet, type ChartSubscribe, type ChartUnsubscribe, type PaperMessage, type PaperState, type ReplayControl } from "@stockwatcher/chart-core";
import { createDetectionChannel, createFavoritesChannel, createHodChannel, HOD_FEATURE, createPendingChannel, PENDING_FEATURE, createMoversChannel, createPressureChannel, createQuoteSearchChannel, createRocketChannel, createRossChannel, createNoticesChannel, createSymbolsChannel, createTickerChannel, createWsClient, hasFeature, CLOCK_FEATURE, INDICATORS_FEATURE, INSIGHTS_TICKER_FEATURE, INSIGHTS_TICKER_INDICATORS_FEATURE, INDICATORS_LIMIT_FEATURE, LEVELS_FEATURE, MACD_STYLES_FEATURE, NOTICES_FEATURE, QUOTE_SEARCH_FEATURE, serverWsUrl, SYMBOLS_FEATURE, ZONES_FEATURE, type AlertsServerMessage, type ConnectionStatus, type MarketSource, type SettingsSync, type WebSocketLike } from "@stockwatcher/shared";
import { webNoticeCards } from "../notify/noticeCards.ts";
import type { RunnersNotifier } from "../notify/runnersNotify.ts";
import type { ChartFeed, PaperCommand, ReplayCommand } from "./types.ts";

export interface ServerFeedOptions {
  url?: string;
  token?: string | null;
  onStatus?: (s: ConnectionStatus) => void;
  /** The page's synced settings (settings/sync.ts): attached on every welcome by the shared client. */
  settings?: SettingsSync;
  /** The price alerts' frames (Activity, fires), whatever this page shows: the app store takes them. */
  onAlerts?: (m: AlertsServerMessage) => void;
  /** The market this page reads: `live` (dashboard) or `replay` (chart page). */
  source?: MarketSource;
  /** Runners system notifications: the opt-in this connection sends, and where notices go. */
  notifier?: Pick<RunnersNotifier, "notifyMessage" | "handle" | "attach">;
  /** shared ws.ts `PROBE_TIMEOUT_MS` / chart-core `SNAPSHOT_TIMEOUT_MS`, shortened by tests. */
  probeTimeoutMs?: number;
  snapshotTimeoutMs?: number;
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
 * shared's ServerMessage union and this tee can go. quotes.search.* frames
 * are shared's own and go straight through to it.
 */
export function createServerFeed(opts: ServerFeedOptions = {}): ChartFeed {
  // Chart frames go only to the slots they belong to (chart-core chartRouter.ts): one socket
  // carries up to 4 charts on the dashboard, the chart page's single default slot otherwise.
  const router = createChartRouter();
  const current = new Map<string, (m: Parameters<Parameters<ChartFeed["subscribe"]>[1]>[0]) => void>();
  let raw: WebSocketLike | null = null;
  const paperListeners = new Set<(m: PaperMessage) => void>();
  let paperState: PaperState | null = null;

  // Late-bound: the channel is built before the client it drives.
  // The ticker search ranked on the server (quotes.search.v1): only the top rows and the pinned
  // matches come down. The whole quote board an older server needed is never opened by the web.
  const quoteSearch = createQuoteSearchChannel({ send: (m) => client.send(m) });
  // The dashboard's Halt watcher, opted in while its panel is mounted.
  const pressure = createPressureChannel({ subscribe: () => client.subscribePressure(), unsubscribe: () => client.unsubscribePressure() });
  const ross = createRossChannel({ subscribe: () => client.subscribeRoss(), unsubscribe: () => client.unsubscribeRoss() });
  // The Favorites panel's rows (the list itself is a synced setting), opted in while it is mounted.
  const favorites = createFavoritesChannel({ subscribe: () => client.subscribeFavorites(), unsubscribe: () => client.unsubscribeFavorites() });
  // Top Gainers and Hot stocks Bull / Bear, ranked on the server (movers.*), opted in while the
  // dashboard shows them. This replaced holding the whole quote board open for them.
  const movers = createMoversChannel({ subscribe: () => client.subscribeMovers(), unsubscribe: () => client.unsubscribeMovers() });
  // The price-band candidates, pushed by servers with symbols.v1 (useCandidates polls otherwise).
  const symbols = createSymbolsChannel({ subscribe: () => client.subscribeSymbols(), unsubscribe: () => client.unsubscribeSymbols() });
  // Today's notification log, pushed by servers with notices.v1 (useNoticeLog polls otherwise).
  const notices = createNoticesChannel({ subscribe: () => client.subscribeNotices(), unsubscribe: () => client.unsubscribeNotices() });
  // The Insights ticker page's package (insights.ticker.v1), one symbol-day while the page is open.
  const ticker = createTickerChannel({ subscribe: (w) => client.subscribeTicker(w), unsubscribe: () => client.unsubscribeTicker(), setIndicators: (d) => client.setTickerIndicators(d) });
  // Rocket watch (experimental), opted in while its tab is open.
  const rocket = createRocketChannel({ subscribe: () => client.subscribeRocket(), unsubscribe: () => client.unsubscribeRocket() });
  // The HOD Momentum scanner (hod.*), opted in while its panel is mounted.
  const hod = createHodChannel({ subscribe: () => client.subscribeHod(), unsubscribe: () => client.unsubscribeHod() });
  // The News pending card (pending.*), opted in while its panel is mounted.
  const pending = createPendingChannel({ subscribe: () => client.subscribePending(), unsubscribe: () => client.unsubscribePending() });
  const detection = createDetectionChannel({ source: opts.source, subscribe: () => client.subscribeDetection(), unsubscribe: () => client.unsubscribeDetection(), resync: () => client.subscribeDetection() });

  const sendRaw = (msg: ChartSubscribe | ChartUnsubscribe | ChartIndicatorsSet | ReplayControl | PaperCommand) => {
    if (raw && raw.readyState === 1) raw.send(JSON.stringify(msg));
  };
  // Server-computed indicators (indicators.v1): chart-core serverIndicators.ts.
  const serverIndicators = createServerIndicators(sendRaw, (id) => current.has(id));
  // The default slot ("") is sent without an id: exactly the single-chart protocol.
  const subscribeFrame = (id: string, s: { symbol: string; timeframe: ChartSubscribe["timeframe"]; profileBin: number }): ChartSubscribe => ({ type: "chart.subscribe", symbol: s.symbol, timeframe: s.timeframe, profileBin: s.profileBin, ...(id ? { id } : {}), ...serverIndicators.subscribeFields(id) });
  // A chart whose snapshot is overdue was most likely subscribed into a connection that died
  // without closing: check it (chart-core snapshotWatch.ts; the empty chart, 2026-11-15).
  const watch = createSnapshotWatch(() => client.checkConnection(), opts.snapshotTimeoutMs);
  const resubscribe = () => {
    for (const [id, s] of router.entries()) {
      sendRaw(subscribeFrame(id, s));
      watch.expect(id);
    }
  };

  const client = createWsClient({
    url: opts.url ?? serverWsUrl(window.location),
    token: opts.token ?? null,
    client: "web",
    source: opts.source,
    probeTimeoutMs: opts.probeTimeoutMs,
    // Runs before onStatus("open") and its resubscribe, so those carry the indicators or not.
    onWelcome: (w) => {
      serverIndicators.setAvailable(hasFeature(w, INDICATORS_FEATURE));
      // Key levels and zones computed on the server (levels.v1, zones.v1): asked on the resubscribe.
      serverIndicators.setContext(hasFeature(w, LEVELS_FEATURE), hasFeature(w, ZONES_FEATURE));
      serverIndicators.setLimitAvailable(hasFeature(w, INDICATORS_LIMIT_FEATURE));
      serverIndicators.setMacdStyles(hasFeature(w, MACD_STYLES_FEATURE));
      symbols.setAvailable(hasFeature(w, SYMBOLS_FEATURE));
      notices.setAvailable(hasFeature(w, NOTICES_FEATURE));
      ticker.setAvailable(hasFeature(w, INSIGHTS_TICKER_FEATURE), hasFeature(w, INSIGHTS_TICKER_INDICATORS_FEATURE), hasFeature(w, MACD_STYLES_FEATURE));
      quoteSearch.setAvailable(hasFeature(w, QUOTE_SEARCH_FEATURE));
      hod.setAvailable(hasFeature(w, HOD_FEATURE));
      pending.setAvailable(hasFeature(w, PENDING_FEATURE));
      // The server decides time (clock.v1): the Runners panel's ages follow its clock frames and
      // its toasts come as `runners.event`.
      rocket.setServerClock(hasFeature(w, CLOCK_FEATURE));
    },
    onStatus: (s) => {
      opts.onStatus?.(s);
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
      hod.setConnected(s === "open");
      pending.setConnected(s === "open");
      if (s === "open") {
        resubscribe(); // subscriptions do not survive a reconnect
        quoteSearch.resend();
      }
    },
    onMessage: (m) => {
      if (m.type === "quotes.search.snapshot" || m.type === "quotes.search.update" || m.type === "quotes.directory") quoteSearch.apply(m);
      else if (m.type === "runners.event") rocket.applyEvent(m);
    },
    onPressure: (m) => pressure.apply(m),
    onRoss: (m) => ross.apply(m),
    onFavorites: (m) => favorites.apply(m),
    onMovers: (m) => movers.apply(m),
    onSymbols: (m) => symbols.apply(m),
    onNotices: (m) => notices.apply(m),
    onTicker: (m) => ticker.apply(m),
    onTickerIndicators: (m) => ticker.applyIndicators(m),
    onRocket: (m) => rocket.apply(m),
    onDetection: (m) => detection.apply(m),
    onHod: (m) => hod.apply(m),
    onPending: (m) => pending.apply(m),
    onDetectionError: (reason) => detection.reject(reason),
    onAlerts: opts.onAlerts,
    runnersNotify: () => opts.notifier?.notifyMessage() ?? null,
    onRunnersNotice: (m) => {
      // Every notice, sent or grouped, updates its stock's card first (notify/noticeCards.ts); a
      // grouped one never notifies (shared shouldShowNotice).
      webNoticeCards.record(m);
      return opts.notifier?.handle(m);
    },
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
        // settings.*, pressure.* and alert frames (shared's own too) take the same shortcut.
        if (typeof ev.data === "string" && (ev.data.startsWith('{"type":"quotes.') || ev.data.startsWith('{"type":"settings.') || ev.data.startsWith('{"type":"pressure.') || ev.data.startsWith('{"type":"ross.') || ev.data.startsWith('{"type":"favorites.') || ev.data.startsWith('{"type":"movers.') || ev.data.startsWith('{"type":"symbols.') || ev.data.startsWith('{"type":"notices.') || ev.data.startsWith('{"type":"insights.') || ev.data.startsWith('{"type":"rocket.') || ev.data.startsWith('{"type":"hod.') || ev.data.startsWith('{"type":"pending.') || ev.data.startsWith('{"type":"alert') || ev.data.startsWith('{"type":"runners.') || /"type"\s*:\s*"detection\.(snapshot|update)"/.test(ev.data))) {
          inner?.({ data: ev.data });
          return;
        }
        const chart = parseChartMessage(ev.data);
        if (chart) {
          if (chart.type === "replay.clock") rocket.applyClock(chart.clock.now);
          router.dispatch(chart);
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

  // The switches changed (Settings → Runners): tell the server on this connection.
  const detachNotifier = opts.notifier?.attach(
    () => client.refreshRunnersNotify(),
    (m) => client.send(m),
  );

  // Back to the page (tab shown again, laptop awake) or the network back: the connection may have
  // died meanwhile without closing, so check it now instead of 37.5 s from now (shared ws.ts
  // `checkConnection`). Otherwise a chart opened then would sit empty until the liveness timeout.
  const recheck = () => client.checkConnection();
  const onVisibility = () => {
    if (document.visibilityState === "visible") recheck();
  };
  const hasDocument = typeof document !== "undefined";
  const hasWindow = typeof window !== "undefined" && typeof window.addEventListener === "function";
  if (hasDocument) document.addEventListener("visibilitychange", onVisibility);
  if (hasWindow) window.addEventListener("online", recheck);

  return {
    kind: "server",
    subscribe(sub, emit) {
      const id = sub.id ?? "";
      // A token per subscription, so a stale cleanup never removes the slot's newer subscription.
      const token = (m: Parameters<typeof emit>[0]) => {
        if (m.type === "chart.snapshot") watch.seen(id);
        emit(m);
      };
      router.add(id, sub, token);
      current.set(id, token);
      serverIndicators.held(id, sub.indicators);
      sendRaw(subscribeFrame(id, sub));
      watch.expect(id);
      return () => {
        if (current.get(id) !== token) return;
        current.delete(id);
        watch.forget(id);
        router.remove(id);
        serverIndicators.released(id);
        // A named slot is freed on the server; the default slot is simply replaced by the next subscribe.
        if (id) sendRaw({ type: "chart.unsubscribe", id });
      };
    },
    checkConnection: recheck,
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
    quoteSearch,
    pressure,
    ross,
    favorites,
    movers,
    symbols,
    notices,
    ticker,
    rocket,
    detection,
    hod,
    pending,
    indicators: serverIndicators.channel,
    close() {
      detachNotifier?.();
      if (hasDocument) document.removeEventListener("visibilitychange", onVisibility);
      if (hasWindow) window.removeEventListener("online", recheck);
      watch.clear();
      client.close();
      router.clear();
      current.clear();
      serverIndicators.clear();
      paperListeners.clear();
    },
  };
}

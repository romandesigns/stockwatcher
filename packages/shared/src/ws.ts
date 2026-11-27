import { isAlertsMessage, parseServerMessage, type AlertsServerMessage, type ClientMessage, type HelloMessage, type MarketSource, type PressureMessage, type QuotesMessage, type RossMessage, type ServerMessage, type WelcomeMessage } from "./protocol.ts";
import { serverUrl } from "./http.ts";
import type { SettingsSync } from "./settingsSync.ts";
import type { DetectionMessage } from "./detection.ts";
import type { RocketMessage } from "./rocket.ts";
import { HOD_FEATURE, type HodMessage } from "./hod.ts";
import { PENDING_FEATURE, type PendingMessage } from "./pending.ts";
import { PREWATCH_FEATURE, type PrewatchMessage } from "./prewatch.ts";
import type { RunnersNoticeMessage, RunnersNotifyMessage } from "./runnersNotify.ts";
import { SYMBOLS_FEATURE, type SymbolsMessage } from "./symbols.ts";
import { NOTICES_FEATURE, type NoticesMessage } from "./noticeFeed.ts";
import { INSIGHTS_TICKER_FEATURE, INSIGHTS_TICKER_INDICATORS_FEATURE, type TickerIndicatorDemand, type TickerIndicatorsMessage, type TickerRequest, type TickerSnapshotMessage } from "./insightsTicker.ts";

export type ConnectionStatus = "connecting" | "open" | "reconnecting" | "unauthorized" | "closed";

/** Minimal WebSocket surface, so tests (and React Native) can supply their own. */
export interface WebSocketLike {
  readyState: number;
  onopen: ((ev: unknown) => void) | null;
  onmessage: ((ev: { data: unknown }) => void) | null;
  onclose: ((ev: unknown) => void) | null;
  onerror: ((ev: unknown) => void) | null;
  send(data: string): void;
  close(code?: number, reason?: string): void;
}

export interface WsClientOptions {
  url: string;
  token?: string | null;
  client?: string;
  /** Which market the connection reads (`hello.source`); absent = the server's default. */
  source?: MarketSource;
  onStatus?: (status: ConnectionStatus) => void;
  onWelcome?: (welcome: WelcomeMessage) => void;
  onMessage?: (message: ServerMessage) => void;
  /** `quotes.snapshot` / `quotes.update` frames, while subscribed (see `subscribeQuotes`). */
  onQuotes?: (message: QuotesMessage) => void;
  /** `pressure.snapshot` / `pressure.update` frames, while subscribed (see `subscribePressure`). */
  onPressure?: (message: PressureMessage) => void;
  /** `ross.snapshot` / `ross.update` frames, while subscribed (see `subscribeRoss`). */
  onRoss?: (message: RossMessage) => void;
  /** `rocket.snapshot` / `rocket.update` frames, while subscribed (see `subscribeRocket`). */
  onRocket?: (message: RocketMessage) => void;
  /** `favorites.snapshot` / `favorites.update` frames, while subscribed (see `subscribeFavorites`). */
  onFavorites?: (message: import("./favorites.ts").FavoritesMessage) => void;
  /** `movers.snapshot` / `movers.update` frames, while subscribed (see `subscribeMovers`). */
  onMovers?: (message: import("./protocol.ts").MoversMessage) => void;
  /** `symbols.snapshot` / `symbols.update` frames, while subscribed (see `subscribeSymbols`). */
  onSymbols?: (message: SymbolsMessage) => void;
  /** `notices.snapshot` / `notices.append` frames, while subscribed (see `subscribeNotices`). */
  onNotices?: (message: NoticesMessage) => void;
  /** `insights.ticker.snapshot` / `.update` frames, while a ticker page is open (see `subscribeTicker`). */
  onTicker?: (message: TickerSnapshotMessage) => void;
  /** `insights.ticker.indicators` frames (`insights.ticker.indicators.v1`), while a ticker page is open. */
  onTickerIndicators?: (message: TickerIndicatorsMessage) => void;
  onDetection?: (message: DetectionMessage) => void;
  /** `hod.snapshot` / `hod.update` frames, while subscribed (see `subscribeHod`). */
  onHod?: (message: HodMessage) => void;
  /** `pending.snapshot` / `pending.update` frames (the News pending card), while subscribed (see `subscribePending`). */
  onPending?: (message: PendingMessage) => void;
  /** `prewatch.snapshot` frames (the pre-open Watch card), while subscribed (see `subscribePrewatch`). */
  onPrewatch?: (message: PrewatchMessage) => void;
  onDetectionError?: (reason: string) => void;
  /** The price alerts' frames (`alerts.history`, `alert.fired`, …): every connection gets them. */
  onAlerts?: (message: AlertsServerMessage) => void;
  /** `runners.notice` frames (runnersNotify.ts): every connection gets them; show per `runnersNotify`. */
  onRunnersNotice?: (message: RunnersNoticeMessage) => void;
  /**
   * This client's Runners notification opt-in, read on every `welcome` and by
   * `refreshRunnersNotify` (null = this client shows none; nothing is sent on welcome).
   */
  runnersNotify?: () => RunnersNotifyMessage | null;
  /**
   * The synced settings (settingsSync.ts). Attached on every `welcome` (the server's
   * `settings.snapshot` follows it), fed every `settings.*` frame, and detached when the
   * connection drops, so a reconnect resyncs and resends unconfirmed edits by itself.
   */
  settings?: SettingsSync;
  createSocket?: (url: string) => WebSocketLike;
  baseDelayMs?: number;
  maxDelayMs?: number;
  random?: () => number;
  /**
   * How long `checkConnection` waits for any frame after its ping before it gives the connection
   * up (default PROBE_TIMEOUT_MS). Tests shorten it.
   */
  probeTimeoutMs?: number;
}

/**
 * `checkConnection`'s wait for an answer. The server answers a ping at once, so a few seconds
 * covers a slow cellular round trip; past that the connection is treated as dead.
 */
export const PROBE_TIMEOUT_MS = 4_000;

/**
 * A timer that fires this much past its time means this thread was busy (a long task, a page
 * buried under frames): the socket's frames may be waiting, unread, behind the timer itself. The
 * check then waits this long once more for them instead of calling the connection dead. Before
 * (the Practice-page freeze, 2026-11-26), a page too busy to read its socket gave the socket up,
 * reconnected, and got every snapshot again, which kept it busy.
 */
export const STALLED_TIMER_MS = 1_000;

/** setTimeout that, if it fires more than STALLED_TIMER_MS late, waits `grace` once more first. */
function patientTimeout(fn: () => void, ms: number, grace: number, onTimer: (t: ReturnType<typeof setTimeout>) => void): void {
  const due = Date.now() + ms;
  onTimer(
    setTimeout(() => {
      if (Date.now() - due > STALLED_TIMER_MS) onTimer(setTimeout(fn, grace));
      else fn();
    }, ms),
  );
}

export interface WsClient {
  send(message: ClientMessage): boolean;
  /**
   * Checks that the connection is really alive, now, and replaces it at once when it is not.
   * Call it when the app comes back (page visible again, phone back in the foreground, network
   * back) and when a subscription's first frame is overdue.
   *
   * Why (the empty chart, 2026-11-15): a connection that died without a close (sleep, a wifi or
   * cellular handover, a phone back from the background) keeps `readyState` OPEN, so every
   * subscribe sent into it is lost, and the liveness timer only notices 2.5 heartbeats (37.5 s)
   * later, longer on a phone where JS timers stop in the background. Here: a ping, and any frame
   * within `probeTimeoutMs` keeps the connection; silence drops it without waiting for a close
   * event (to a dead peer that can take minutes) and reconnects immediately. With no connection
   * up (between retries), it skips the rest of the backoff.
   */
  checkConnection(): void;
  /**
   * Opts in to the quote board: the server answers with a `quotes.snapshot`
   * and then sends `quotes.update`s. The opt-in is remembered and sent again
   * after every reconnect (the server forgets it with the connection).
   */
  subscribeQuotes(): void;
  unsubscribeQuotes(): void;
  /** The Halt watcher's board (`pressure.*`), remembered across reconnects like the quotes opt-in. */
  subscribePressure(): void;
  unsubscribePressure(): void;
  /** The Ross Panel (`ross.*`), remembered across reconnects like the pressure opt-in. */
  subscribeRoss(): void;
  unsubscribeRoss(): void;
  /** Rocket watch (`rocket.*`), remembered across reconnects like the Ross opt-in. */
  subscribeRocket(): void;
  unsubscribeRocket(): void;
  /** The Favorites panel (`favorites.*`), remembered across reconnects like the Ross opt-in. */
  subscribeFavorites(): void;
  unsubscribeFavorites(): void;
  /** Top Gainers and Hot stocks Bull / Bear (`movers.*`), remembered across reconnects like the Favorites opt-in. */
  subscribeMovers(): void;
  unsubscribeMovers(): void;
  /**
   * The price-band candidate list (`symbols.*`), remembered across reconnects like the movers
   * opt-in, but sent only to a server whose welcome lists `symbols.v1`: an older one would
   * answer the unknown frame with an error. Without it nothing is sent; the caller polls.
   */
  subscribeSymbols(): void;
  unsubscribeSymbols(): void;
  /** Today's notification log (`notices.*`), gated on `notices.v1` like `subscribeSymbols`. */
  subscribeNotices(): void;
  unsubscribeNotices(): void;
  /**
   * The Insights ticker page's package for one symbol-day (`insights.ticker.*`), gated on
   * `insights.ticker.v1` like `subscribeNotices` and sent again after every reconnect. A new call
   * replaces the open one (the server keeps one per connection).
   */
  subscribeTicker(want: TickerRequest): void;
  unsubscribeTicker(): void;
  /**
   * The ticker page chart's indicators (`insights.ticker.indicators.v1`): kept, carried on every
   * `insights.ticker.subscribe` (reconnects included), and sent now as
   * `insights.ticker.indicators.set` while a page is open. Never sent to a server without it.
   */
  setTickerIndicators(demand: TickerIndicatorDemand): void;
  subscribeDetection(): void;
  unsubscribeDetection(): void;
  /** The HOD Momentum scanner (`hod.*`), gated on `hod.v1` like `subscribeSymbols` and sent again after every reconnect. */
  subscribeHod(): void;
  unsubscribeHod(): void;
  /** The News pending card (`pending.*`), gated on `pending.v1` and sent again after every reconnect. */
  subscribePending(): void;
  unsubscribePending(): void;
  /** The pre-open Watch card (`prewatch.*`), gated on `prewatch.v1` and sent again after every reconnect. */
  subscribePrewatch(): void;
  unsubscribePrewatch(): void;
  /** Sends `runnersNotify()` now (or `enabled: false` when it is null): the switches changed. */
  refreshRunnersNotify(): void;
  close(): void;
}

const OPEN = 1;

/** Exponential backoff with 50-100 % jitter, so clients don't reconnect in lockstep. */
export function backoffDelay(attempt: number, baseMs = 500, maxMs = 30_000, random = Math.random): number {
  const ceiling = Math.min(maxMs, baseMs * 2 ** Math.max(0, attempt));
  return Math.round(ceiling * (0.5 + random() * 0.5));
}

/** Builds a ws:// or wss:// URL for `path` on the page's own origin. */
export function sameOriginWsUrl(location: { protocol: string; host: string }, path = "/ws"): string {
  return `${location.protocol === "https:" ? "wss:" : "ws:"}//${location.host}${path}`;
}

/** The server's socket URL: the page's own origin unless one was named (`setServerOrigin`, desktop). */
export function serverWsUrl(location: { protocol: string; host: string }, path = "/ws"): string {
  const origin = serverUrl("");
  return origin ? `${origin.replace(/^http/, "ws")}${path}` : sameOriginWsUrl(location, path);
}

/**
 * Connects, sends `hello` with the token, answers server pings, and
 * reconnects with backoff. "open" is reported only after `welcome`, i.e.
 * once the server has accepted the token. An `unauthorized` error stops
 * reconnecting: retrying a rejected token only hammers the server.
 */
export function createWsClient(opts: WsClientOptions): WsClient {
  const createSocket = opts.createSocket ?? ((url: string) => new WebSocket(url) as unknown as WebSocketLike);
  let socket: WebSocketLike | null = null;
  let attempt = 0;
  let stopped = false;
  let reconnectTimer: ReturnType<typeof setTimeout> | null = null;
  let livenessTimer: ReturnType<typeof setTimeout> | null = null;
  let heartbeatMs = 15_000;
  let quotesWanted = false;
  let pressureWanted = false;
  let rossWanted = false;
  let rocketWanted = false;
  let favoritesWanted = false;
  let moversWanted = false;
  let symbolsWanted = false;
  let noticesWanted = false;
  let tickerWanted: TickerRequest | null = null;
  let tickerIndicators: TickerIndicatorDemand | null = null;
  // The subscribe, with the indicator demand when the server computes it.
  const tickerSubscribe = (want: TickerRequest) => {
    if (!features.includes(INSIGHTS_TICKER_FEATURE)) return;
    const withIndicators = tickerIndicators && features.includes(INSIGHTS_TICKER_INDICATORS_FEATURE) ? tickerIndicators : null;
    send({ type: "insights.ticker.subscribe", ...want, ...(withIndicators ? { indicators: withIndicators.indicators, rth: withIndicators.rth } : {}) });
  };
  // The current connection's welcome.features (empty before a welcome and for older servers).
  let features: readonly string[] = [];
  let detectionWanted = false;
  let hodWanted = false;
  let pendingWanted = false;
  let prewatchWanted = false;

  const setStatus = (s: ConnectionStatus) => opts.onStatus?.(s);

  // `checkConnection`'s pending wait for an answer (cleared by any frame from the socket).
  let probeTimer: ReturnType<typeof setTimeout> | null = null;
  const probeMs = opts.probeTimeoutMs ?? PROBE_TIMEOUT_MS;
  const clearProbe = () => {
    if (probeTimer) clearTimeout(probeTimer);
    probeTimer = null;
  };

  // Gives up a connection that has gone silent and reconnects at once. It does not wait for the
  // socket's close event: closing a socket whose peer is gone leaves it CLOSING until TCP gives up,
  // which can take minutes, and every subscribe meanwhile would go nowhere. The old socket's
  // handlers ignore it from here on (`socket !== ws`).
  const abandon = (ws: WebSocketLike, code: number, reason: string) => {
    if (stopped || socket !== ws) return;
    if (livenessTimer) clearTimeout(livenessTimer);
    clearProbe();
    socket = null;
    features = [];
    opts.settings?.detach();
    try {
      ws.close(code, reason);
    } catch {
      // Already closing; nothing else to do with it.
    }
    connect();
  };

  // A half-open TCP connection never fires onclose. If nothing (not even a
  // heartbeat ping) arrives for 2.5 heartbeats, drop it and reconnect.
  const armLiveness = () => {
    if (livenessTimer) clearTimeout(livenessTimer);
    const ws = socket;
    patientTimeout(
      () => {
        if (ws) abandon(ws, 4000, "heartbeat timeout");
      },
      heartbeatMs * 2.5,
      probeMs,
      (t) => (livenessTimer = t),
    );
  };

  const send = (message: ClientMessage): boolean => {
    if (!socket || socket.readyState !== OPEN) return false;
    socket.send(JSON.stringify(message));
    return true;
  };

  const connect = () => {
    if (stopped) return;
    setStatus(attempt === 0 ? "connecting" : "reconnecting");
    const ws = createSocket(opts.url);
    socket = ws;
    let unauthorized = false;

    ws.onopen = () => {
      if (stopped || socket !== ws) return;
      const hello: HelloMessage = { type: "hello" };
      if (opts.client) hello.client = opts.client;
      if (opts.token) hello.token = opts.token;
      if (opts.source) hello.source = opts.source;
      send(hello);
      armLiveness();
    };
    ws.onmessage = (ev) => {
      // A callback already queued on a retired socket must not overwrite a new source/epoch.
      if (stopped || socket !== ws) return;
      // Anything at all from the server answers a pending `checkConnection`.
      clearProbe();
      const msg = parseServerMessage(ev.data);
      if (!msg) {
        if (detectionWanted && typeof ev.data === "string" && /"type"\s*:\s*"detection\.(snapshot|update)"/.test(ev.data)) opts.onDetectionError?.("invalid_detection_frame");
        return;
      }
      armLiveness();
      if (msg.type === "welcome") {
        attempt = 0;
        heartbeatMs = msg.heartbeatSecs * 1000;
        features = Array.isArray(msg.features) ? msg.features : [];
        // Before "open": the chart feeds resubscribe on "open" and read the welcome's `features`
        // (indicators.v1) to decide what their chart.subscribe carries.
        opts.onWelcome?.(msg);
        setStatus("open");
        opts.settings?.attach(send);
        if (quotesWanted) send({ type: "quotes.subscribe" });
        if (pressureWanted) send({ type: "pressure.subscribe" });
        if (rossWanted) send({ type: "ross.subscribe" });
        if (rocketWanted) send({ type: "rocket.subscribe" });
        if (favoritesWanted) send({ type: "favorites.subscribe" });
        if (moversWanted) send({ type: "movers.subscribe" });
        if (symbolsWanted && features.includes(SYMBOLS_FEATURE)) send({ type: "symbols.subscribe" });
        if (noticesWanted && features.includes(NOTICES_FEATURE)) send({ type: "notices.subscribe" });
        if (tickerWanted) tickerSubscribe(tickerWanted);
        if (detectionWanted) send({ type: "detection.subscribe" });
        if (hodWanted && features.includes(HOD_FEATURE)) send({ type: "hod.subscribe" });
        if (pendingWanted && features.includes(PENDING_FEATURE)) send({ type: "pending.subscribe" });
        if (prewatchWanted && features.includes(PREWATCH_FEATURE)) send({ type: "prewatch.subscribe" });
        const notify = opts.runnersNotify?.();
        if (notify) send(notify);
      } else if (msg.type === "runners.notice") {
        opts.onRunnersNotice?.(msg);
      } else if (msg.type === "ping") {
        send({ type: "pong", ts: msg.ts });
      } else if (msg.type === "error" && msg.code === "unauthorized") {
        unauthorized = true;
      } else if ((msg.type === "quotes.snapshot" || msg.type === "quotes.update") && quotesWanted) {
        opts.onQuotes?.(msg);
      } else if ((msg.type === "pressure.snapshot" || msg.type === "pressure.update") && pressureWanted) {
        opts.onPressure?.(msg);
      } else if ((msg.type === "ross.snapshot" || msg.type === "ross.update") && rossWanted) {
        opts.onRoss?.(msg);
      } else if ((msg.type === "rocket.snapshot" || msg.type === "rocket.update") && rocketWanted) {
        opts.onRocket?.(msg);
      } else if ((msg.type === "favorites.snapshot" || msg.type === "favorites.update") && favoritesWanted) {
        opts.onFavorites?.(msg);
      } else if ((msg.type === "movers.snapshot" || msg.type === "movers.update") && moversWanted) {
        opts.onMovers?.(msg);
      } else if ((msg.type === "symbols.snapshot" || msg.type === "symbols.update") && symbolsWanted) {
        opts.onSymbols?.(msg);
      } else if ((msg.type === "notices.snapshot" || msg.type === "notices.append") && noticesWanted) {
        opts.onNotices?.(msg);
      } else if ((msg.type === "insights.ticker.snapshot" || msg.type === "insights.ticker.update") && tickerWanted) {
        opts.onTicker?.(msg);
      } else if (msg.type === "insights.ticker.indicators" && tickerWanted) {
        opts.onTickerIndicators?.(msg);
      } else if ((msg.type === "detection.snapshot" || msg.type === "detection.update") && detectionWanted) {
        opts.onDetection?.(msg);
      } else if ((msg.type === "hod.snapshot" || msg.type === "hod.update") && hodWanted) {
        opts.onHod?.(msg);
      } else if ((msg.type === "pending.snapshot" || msg.type === "pending.update") && pendingWanted) {
        opts.onPending?.(msg);
      } else if (msg.type === "prewatch.snapshot" && prewatchWanted) {
        opts.onPrewatch?.(msg);
      } else if (msg.type === "settings.snapshot" || msg.type === "settings.changed" || msg.type === "settings.rejected") {
        opts.settings?.handle(msg);
      } else if (isAlertsMessage(msg)) {
        opts.onAlerts?.(msg);
      }
      opts.onMessage?.(msg);
    };
    // onclose always follows onerror; reconnecting is handled there.
    ws.onerror = () => {};
    ws.onclose = () => {
      if (socket !== ws) return;
      if (livenessTimer) clearTimeout(livenessTimer);
      clearProbe();
      if (socket === ws) {
        socket = null;
        features = [];
        opts.settings?.detach();
      }
      if (stopped) return setStatus("closed");
      if (unauthorized) {
        stopped = true;
        return setStatus("unauthorized");
      }
      setStatus("reconnecting");
      reconnectTimer = setTimeout(connect, backoffDelay(attempt++, opts.baseDelayMs, opts.maxDelayMs, opts.random));
    };
  };

  connect();

  return {
    send,
    checkConnection() {
      if (stopped) return;
      const ws = socket;
      if (ws && ws.readyState === OPEN) {
        if (probeTimer) return; // a check is already waiting for its answer
        send({ type: "ping", ts: Date.now() });
        patientTimeout(
          () => {
            probeTimer = null;
            abandon(ws, 4001, "no answer to ping");
          },
          probeMs,
          probeMs,
          (t) => (probeTimer = t),
        );
      } else if (!ws && reconnectTimer) {
        // Between retries (the backoff can be up to 30 s): the app is back, so try now.
        clearTimeout(reconnectTimer);
        reconnectTimer = null;
        connect();
      }
      // A socket still connecting will either open or close on its own.
    },
    subscribeQuotes() {
      quotesWanted = true;
      // Not open yet: the welcome handler sends it.
      send({ type: "quotes.subscribe" });
    },
    unsubscribeQuotes() {
      quotesWanted = false;
      send({ type: "quotes.unsubscribe" });
    },
    subscribePressure() {
      pressureWanted = true;
      send({ type: "pressure.subscribe" });
    },
    subscribeRoss() {
      rossWanted = true;
      send({ type: "ross.subscribe" });
    },
    unsubscribeRoss() {
      rossWanted = false;
      send({ type: "ross.unsubscribe" });
    },
    subscribeRocket() {
      rocketWanted = true;
      send({ type: "rocket.subscribe" });
    },
    unsubscribeRocket() {
      rocketWanted = false;
      send({ type: "rocket.unsubscribe" });
    },
    subscribeFavorites() {
      favoritesWanted = true;
      send({ type: "favorites.subscribe" });
    },
    unsubscribeFavorites() {
      favoritesWanted = false;
      send({ type: "favorites.unsubscribe" });
    },
    subscribeMovers() {
      moversWanted = true;
      send({ type: "movers.subscribe" });
    },
    unsubscribeMovers() {
      moversWanted = false;
      send({ type: "movers.unsubscribe" });
    },
    subscribeSymbols() {
      symbolsWanted = true;
      // Not open yet: the welcome handler sends it, if the server has the feature.
      if (features.includes(SYMBOLS_FEATURE)) send({ type: "symbols.subscribe" });
    },
    unsubscribeSymbols() {
      symbolsWanted = false;
      if (features.includes(SYMBOLS_FEATURE)) send({ type: "symbols.unsubscribe" });
    },
    subscribeNotices() {
      noticesWanted = true;
      if (features.includes(NOTICES_FEATURE)) send({ type: "notices.subscribe" });
    },
    unsubscribeNotices() {
      noticesWanted = false;
      if (features.includes(NOTICES_FEATURE)) send({ type: "notices.unsubscribe" });
    },
    subscribeTicker(want) {
      tickerWanted = { symbol: want.symbol, day: want.day };
      tickerSubscribe(tickerWanted);
    },
    setTickerIndicators(demand) {
      tickerIndicators = { indicators: [...demand.indicators], rth: demand.rth };
      if (tickerWanted && features.includes(INSIGHTS_TICKER_FEATURE) && features.includes(INSIGHTS_TICKER_INDICATORS_FEATURE)) send({ type: "insights.ticker.indicators.set", ...tickerIndicators });
    },
    unsubscribeTicker() {
      if (!tickerWanted) return;
      tickerWanted = null;
      if (features.includes(INSIGHTS_TICKER_FEATURE)) send({ type: "insights.ticker.unsubscribe" });
    },
    subscribeDetection() {
      detectionWanted = true;
      send({ type: "detection.subscribe" });
    },
    unsubscribeDetection() {
      detectionWanted = false;
      send({ type: "detection.unsubscribe" });
    },
    subscribeHod() {
      hodWanted = true;
      if (features.includes(HOD_FEATURE)) send({ type: "hod.subscribe" });
    },
    unsubscribeHod() {
      hodWanted = false;
      if (features.includes(HOD_FEATURE)) send({ type: "hod.unsubscribe" });
    },
    subscribePending() {
      pendingWanted = true;
      if (features.includes(PENDING_FEATURE)) send({ type: "pending.subscribe" });
    },
    unsubscribePending() {
      pendingWanted = false;
      if (features.includes(PENDING_FEATURE)) send({ type: "pending.unsubscribe" });
    },
    subscribePrewatch() {
      prewatchWanted = true;
      if (features.includes(PREWATCH_FEATURE)) send({ type: "prewatch.subscribe" });
    },
    unsubscribePrewatch() {
      prewatchWanted = false;
      if (features.includes(PREWATCH_FEATURE)) send({ type: "prewatch.unsubscribe" });
    },
    unsubscribePressure() {
      pressureWanted = false;
      send({ type: "pressure.unsubscribe" });
    },
    refreshRunnersNotify() {
      // Not open yet: the welcome handler sends it.
      send(opts.runnersNotify?.() ?? { type: "runners.notify", enabled: false });
    },
    close() {
      stopped = true;
      if (reconnectTimer) clearTimeout(reconnectTimer);
      if (livenessTimer) clearTimeout(livenessTimer);
      clearProbe();
      if (socket) socket.close(1000, "client closed");
      else setStatus("closed");
    },
  };
}

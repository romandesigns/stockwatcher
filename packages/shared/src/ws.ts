import { isAlertsMessage, parseServerMessage, type AlertsServerMessage, type ClientMessage, type HelloMessage, type MarketSource, type PressureMessage, type QuotesMessage, type RossMessage, type ServerMessage, type WelcomeMessage } from "./protocol.ts";
import { serverUrl } from "./http.ts";
import type { SettingsSync } from "./settingsSync.ts";
import type { DetectionMessage } from "./detection.ts";
import type { RocketMessage } from "./rocket.ts";
import type { RunnersNoticeMessage, RunnersNotifyMessage } from "./runnersNotify.ts";
import { SYMBOLS_FEATURE, type SymbolsMessage } from "./symbols.ts";

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
  onDetection?: (message: DetectionMessage) => void;
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
}

export interface WsClient {
  send(message: ClientMessage): boolean;
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
  subscribeDetection(): void;
  unsubscribeDetection(): void;
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
  // The current connection's welcome.features (empty before a welcome and for older servers).
  let features: readonly string[] = [];
  let detectionWanted = false;

  const setStatus = (s: ConnectionStatus) => opts.onStatus?.(s);

  // A half-open TCP connection never fires onclose. If nothing (not even a
  // heartbeat ping) arrives for 2.5 heartbeats, drop it and reconnect.
  const armLiveness = () => {
    if (livenessTimer) clearTimeout(livenessTimer);
    livenessTimer = setTimeout(() => socket?.close(4000, "heartbeat timeout"), heartbeatMs * 2.5);
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
        if (detectionWanted) send({ type: "detection.subscribe" });
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
      } else if ((msg.type === "detection.snapshot" || msg.type === "detection.update") && detectionWanted) {
        opts.onDetection?.(msg);
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
    subscribeDetection() {
      detectionWanted = true;
      send({ type: "detection.subscribe" });
    },
    unsubscribeDetection() {
      detectionWanted = false;
      send({ type: "detection.unsubscribe" });
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
      if (socket) socket.close(1000, "client closed");
      else setStatus("closed");
    },
  };
}

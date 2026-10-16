import { parseServerMessage, type ClientMessage, type HelloMessage, type ServerMessage, type WelcomeMessage } from "./protocol.ts";

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
  onStatus?: (status: ConnectionStatus) => void;
  onWelcome?: (welcome: WelcomeMessage) => void;
  onMessage?: (message: ServerMessage) => void;
  createSocket?: (url: string) => WebSocketLike;
  baseDelayMs?: number;
  maxDelayMs?: number;
  random?: () => number;
}

export interface WsClient {
  send(message: ClientMessage): boolean;
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
      const hello: HelloMessage = { type: "hello" };
      if (opts.client) hello.client = opts.client;
      if (opts.token) hello.token = opts.token;
      send(hello);
      armLiveness();
    };
    ws.onmessage = (ev) => {
      const msg = parseServerMessage(ev.data);
      if (!msg) return;
      armLiveness();
      if (msg.type === "welcome") {
        attempt = 0;
        heartbeatMs = msg.heartbeatSecs * 1000;
        setStatus("open");
        opts.onWelcome?.(msg);
      } else if (msg.type === "ping") {
        send({ type: "pong", ts: msg.ts });
      } else if (msg.type === "error" && msg.code === "unauthorized") {
        unauthorized = true;
      }
      opts.onMessage?.(msg);
    };
    // onclose always follows onerror; reconnecting is handled there.
    ws.onerror = () => {};
    ws.onclose = () => {
      if (livenessTimer) clearTimeout(livenessTimer);
      if (socket === ws) socket = null;
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
    close() {
      stopped = true;
      if (reconnectTimer) clearTimeout(reconnectTimer);
      if (livenessTimer) clearTimeout(livenessTimer);
      if (socket) socket.close(1000, "client closed");
      else setStatus("closed");
    },
  };
}

import { afterEach, describe, expect, test } from "bun:test";
import { authenticatedFetch, UnauthorizedError } from "./http.ts";
import { parseServerMessage } from "./protocol.ts";
import { backoffDelay, createWsClient, sameOriginWsUrl, type ConnectionStatus, type WebSocketLike } from "./ws.ts";

class FakeSocket implements WebSocketLike {
  static instances: FakeSocket[] = [];
  readyState = 0;
  sent: string[] = [];
  onopen: ((ev: unknown) => void) | null = null;
  onmessage: ((ev: { data: unknown }) => void) | null = null;
  onclose: ((ev: unknown) => void) | null = null;
  onerror: ((ev: unknown) => void) | null = null;
  constructor(public url: string) {
    FakeSocket.instances.push(this);
  }
  send(data: string) {
    this.sent.push(data);
  }
  close() {
    this.readyState = 3;
    this.onclose?.({});
  }
  open() {
    this.readyState = 1;
    this.onopen?.({});
  }
  receive(msg: object) {
    this.onmessage?.({ data: JSON.stringify(msg) });
  }
}

const tick = (ms = 5) => new Promise((r) => setTimeout(r, ms));
const welcome = { type: "welcome", version: "0.1.0", commit: "abc", heartbeatSecs: 15 };

afterEach(() => {
  FakeSocket.instances = [];
});

describe("parseServerMessage", () => {
  test("accepts known types", () => {
    expect(parseServerMessage(JSON.stringify(welcome))).toEqual(welcome as never);
    expect(parseServerMessage('{"type":"ping","ts":1}')).toEqual({ type: "ping", ts: 1 });
  });
  test("rejects malformed or unknown frames", () => {
    expect(parseServerMessage("not json")).toBeNull();
    expect(parseServerMessage('{"type":"bogus"}')).toBeNull();
    expect(parseServerMessage("null")).toBeNull();
    expect(parseServerMessage(42)).toBeNull();
  });
});

describe("backoffDelay", () => {
  test("grows exponentially and caps", () => {
    const max = () => 1;
    expect(backoffDelay(0, 500, 30_000, max)).toBe(500);
    expect(backoffDelay(3, 500, 30_000, max)).toBe(4000);
    expect(backoffDelay(20, 500, 30_000, max)).toBe(30_000);
  });
  test("jitters down to half", () => {
    expect(backoffDelay(3, 500, 30_000, () => 0)).toBe(2000);
  });
});

test("sameOriginWsUrl follows the page scheme", () => {
  expect(sameOriginWsUrl({ protocol: "https:", host: "a.example" })).toBe("wss://a.example/ws");
  expect(sameOriginWsUrl({ protocol: "http:", host: "localhost:5173" })).toBe("ws://localhost:5173/ws");
});

describe("authenticatedFetch", () => {
  const realFetch = globalThis.fetch;
  afterEach(() => {
    globalThis.fetch = realFetch;
  });

  test("sets the bearer header only when a token exists", async () => {
    const seen: (string | null)[] = [];
    globalThis.fetch = (async (_input: unknown, init?: RequestInit) => {
      seen.push(new Headers(init?.headers).get("authorization"));
      return new Response("{}");
    }) as typeof fetch;
    await authenticatedFetch("/api/version", "t0ken");
    await authenticatedFetch("/api/version", null);
    expect(seen).toEqual(["Bearer t0ken", null]);
  });

  test("throws UnauthorizedError on 401", async () => {
    globalThis.fetch = (async () => new Response("", { status: 401 })) as unknown as typeof fetch;
    await expect(authenticatedFetch("/api/version", "bad")).rejects.toBeInstanceOf(UnauthorizedError);
  });
});

describe("createWsClient", () => {
  function start(token: string | null = "secret") {
    const statuses: ConnectionStatus[] = [];
    const client = createWsClient({
      url: "ws://x/ws",
      token,
      client: "test",
      createSocket: (url) => new FakeSocket(url),
      onStatus: (s) => statuses.push(s),
      baseDelayMs: 1,
      maxDelayMs: 2,
    });
    return { client, statuses };
  }

  test("sends hello with token and reports open after welcome", () => {
    const { client, statuses } = start();
    const sock = FakeSocket.instances[0]!;
    sock.open();
    expect(JSON.parse(sock.sent[0]!)).toEqual({ type: "hello", client: "test", token: "secret" });
    expect(statuses).toEqual(["connecting"]);
    sock.receive(welcome);
    expect(statuses).toEqual(["connecting", "open"]);
    client.close();
    expect(statuses.at(-1)).toBe("closed");
  });

  test("omits the token when there is none", () => {
    const { client } = start(null);
    const sock = FakeSocket.instances[0]!;
    sock.open();
    expect(JSON.parse(sock.sent[0]!)).toEqual({ type: "hello", client: "test" });
    client.close();
  });

  test("answers server pings with pongs", () => {
    const { client } = start();
    const sock = FakeSocket.instances[0]!;
    sock.open();
    sock.receive({ type: "ping", ts: 123 });
    expect(JSON.parse(sock.sent.at(-1)!)).toEqual({ type: "pong", ts: 123 });
    client.close();
  });

  test("reconnects after an unexpected close", async () => {
    const { client, statuses } = start();
    FakeSocket.instances[0]!.open();
    FakeSocket.instances[0]!.close();
    expect(statuses.at(-1)).toBe("reconnecting");
    await tick();
    expect(FakeSocket.instances.length).toBe(2);
    client.close();
  });

  test("stops reconnecting after an unauthorized error", async () => {
    const { statuses } = start("wrong");
    const sock = FakeSocket.instances[0]!;
    sock.open();
    sock.receive({ type: "error", code: "unauthorized", message: "bad token" });
    sock.close();
    await tick();
    expect(statuses.at(-1)).toBe("unauthorized");
    expect(FakeSocket.instances.length).toBe(1);
  });
});

describe("quote board opt-in", () => {
  test("parseServerMessage knows the quotes frames", () => {
    const snap = { type: "quotes.snapshot", t: 1, quotes: [] };
    expect(parseServerMessage(JSON.stringify(snap))).toEqual(snap as never);
    expect(parseServerMessage('{"type":"quotes.update","t":2,"quotes":[]}')?.type).toBe("quotes.update");
  });

  test("subscribes after welcome, delivers frames, re-subscribes after a reconnect, stops on unsubscribe", async () => {
    const got: string[] = [];
    const client = createWsClient({
      url: "ws://x/ws",
      createSocket: (url) => new FakeSocket(url),
      onQuotes: (m) => got.push(m.type),
      baseDelayMs: 1,
      maxDelayMs: 2,
    });
    // Asked for before the socket is open: sent once the server has welcomed us.
    client.subscribeQuotes();
    const first = FakeSocket.instances[0]!;
    first.open();
    expect(first.sent.map((s) => JSON.parse(s).type)).toEqual(["hello"]);
    first.receive(welcome);
    expect(JSON.parse(first.sent.at(-1)!)).toEqual({ type: "quotes.subscribe" });
    first.receive({ type: "quotes.snapshot", t: 1, quotes: [] });
    first.receive({ type: "quotes.update", t: 2, quotes: [] });
    expect(got).toEqual(["quotes.snapshot", "quotes.update"]);

    // The server forgets the opt-in with the connection; the client does not.
    first.close();
    await tick();
    const second = FakeSocket.instances[1]!;
    second.open();
    second.receive(welcome);
    expect(JSON.parse(second.sent.at(-1)!)).toEqual({ type: "quotes.subscribe" });

    client.unsubscribeQuotes();
    expect(JSON.parse(second.sent.at(-1)!)).toEqual({ type: "quotes.unsubscribe" });
    // A frame already in flight after the unsubscribe is not delivered.
    second.receive({ type: "quotes.update", t: 3, quotes: [] });
    expect(got.length).toBe(2);
    client.close();
  });
});

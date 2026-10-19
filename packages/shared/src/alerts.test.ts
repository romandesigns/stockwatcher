import { expect, test } from "bun:test";
import { isAlertsMessage, parseServerMessage } from "./protocol.ts";
import { createWsClient, type WebSocketLike } from "./ws.ts";

test("alert frames parse and reach onAlerts; push.register goes out as given", () => {
  const fired = JSON.stringify({ type: "alert.fired", event: { id: "h1", alertId: "a", symbol: "XLAB", kind: "fired", t: 1, at: 1, text: "Crossed above 2.50 at 2.51", app: true, phone: "sent", note: "" }, phoneToday: 1 });
  const m = parseServerMessage(fired);
  expect(m && isAlertsMessage(m)).toBe(true);
  expect(parseServerMessage(JSON.stringify({ type: "alerts.history", events: [], phoneToday: 0, phoneBudget: 20, devices: 0, dropped: 0 }))?.type).toBe("alerts.history");

  const sent: string[] = [];
  let sock: WebSocketLike | null = null;
  const got: string[] = [];
  const client = createWsClient({
    url: "ws://x",
    createSocket: () => {
      sock = { readyState: 1, onopen: null, onmessage: null, onclose: null, onerror: null, send: (d) => sent.push(d), close: () => {} };
      return sock;
    },
    onAlerts: (a) => got.push(a.type),
  });
  sock!.onopen?.({});
  sock!.onmessage?.({ data: JSON.stringify({ type: "welcome", version: "1", commit: "x", heartbeatSecs: 15 }) });
  sock!.onmessage?.({ data: fired });
  expect(got).toEqual(["alert.fired"]);
  client.send({ type: "push.register", token: "ExponentPushToken[x]", platform: "android" });
  expect(JSON.parse(sent.at(-1)!)).toEqual({ type: "push.register", token: "ExponentPushToken[x]", platform: "android" });
  client.close();
});

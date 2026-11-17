import { expect, test } from "bun:test";
import { createRocketChannel, type RocketMessage } from "@stockwatcher/shared";

import { runnersEventToast, toastStandsDown } from "./runnersToasts.ts";

test("a server's runners.event is the toast, in the same words", () => {
  expect(runnersEventToast({ type: "runners.event", kind: "new_flag", id: "a", symbol: "CJMB", at_ns: 1, path: "runner", price: 2.45, reason: "volume 12x" })).toEqual({ kind: "new", symbol: "CJMB", text: "Runners: CJMB $2.45 · volume 12x" });
  expect(runnersEventToast({ type: "runners.event", kind: "breakout", id: "a", symbol: "CJMB", at_ns: 1, path: "runner", price: 3.1, from_flag_pct: 26.53 })).toEqual({ kind: "breakout", symbol: "CJMB", text: "CJMB breaking out $3.10 · +26.5% from flag" });
  expect(runnersEventToast({ type: "runners.event", kind: "new_flag", id: "a", symbol: "", at_ns: 1, path: "runner", price: null })).toBeNull();
});

test("a system notification for the kind stands the toast down; a kind switched off still toasts", () => {
  const notifier = (active: boolean, breakout: boolean) => ({ active: () => active, prefs: () => ({ enabled: active, kinds: { breakout, new: true } as never, haltSounds: { halted: "siren" as const, resumed: "chime" as const } }) });
  expect(toastStandsDown("new", notifier(true, false))).toBe(true);
  expect(toastStandsDown("breakout", notifier(true, false))).toBe(false);
  expect(toastStandsDown("new", notifier(false, true))).toBe(false);
});

test("a panel frame alone is never a toast: only the server's events are (no wall-clock diff)", () => {
  const ch = createRocketChannel({ subscribe: () => {}, unsubscribe: () => {} });
  const events: string[] = [];
  const off = ch.subscribe(() => {});
  const offE = ch.onEvent!((m) => events.push(m.id));
  ch.apply({ type: "rocket.update", enabled: true, t: 1, rows: [], today: { alerts: 0, hits: 0 }, days: [] } as unknown as RocketMessage);
  expect(events).toEqual([]);
  ch.applyEvent({ type: "runners.event", kind: "new_flag", id: "x", symbol: "CJMB", at_ns: 1, path: "runner", price: 1 });
  expect(events).toEqual(["x"]);
  off();
  offE();
});

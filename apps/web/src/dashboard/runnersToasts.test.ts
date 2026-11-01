import { expect, test } from "bun:test";
import type { RocketRowWire } from "@stockwatcher/shared";

import { FRESH_MS, runnersEventToast, runnersToasts, type RunnersToastMemory } from "./runnersToasts.ts";

const NOW = 1_792_670_400_000;
const row = (id: string, s: string, extra: Partial<RocketRowWire> = {}): RocketRowWire => ({
  id,
  s,
  at: NOW - 30_000,
  p: 2.45,
  last: 2.5,
  pc: 2,
  score: 1,
  reasons: ["volume 9x pace", "new high"],
  outcome: { state: "pending", gain: null, drawdown: null, partial: false },
  state: "watching",
  ...extra,
});

test("the first frame primes; a new fresh flag and a turn into breaking out toast once", () => {
  const mem: RunnersToastMemory = { seen: new Map(), primed: false };
  expect(runnersToasts([row("a", "OLD")], mem, NOW)).toEqual([]);
  const flag = runnersToasts([row("b", "CJMB"), row("a", "OLD")], mem, NOW);
  expect(flag).toEqual([{ kind: "new", symbol: "CJMB", text: "Runners: CJMB $2.45 · volume 9x pace" }]);
  // The same frame again: nothing.
  expect(runnersToasts([row("b", "CJMB"), row("a", "OLD")], mem, NOW)).toEqual([]);
  // A provisional breakout is not news yet; a settled one is.
  expect(runnersToasts([row("b", "CJMB", { state: "breaking_out", provisional: true, state_since: NOW })], mem, NOW)).toEqual([]);
  const out = runnersToasts([row("b", "CJMB", { state: "breaking_out", state_since: NOW - 1000, last: 3.1, from_flag_pct: 26.53 })], mem, NOW);
  expect(out).toEqual([{ kind: "breakout", symbol: "CJMB", text: "CJMB breaking out $3.10 · +26.5% from flag" }]);
  expect(runnersToasts([row("b", "CJMB", { state: "breaking_out", state_since: NOW - 1000 })], mem, NOW)).toEqual([]);
});

test("a running entry never toasts as news (opt-in push on the phone only, 2026-10-22)", () => {
  const mem: RunnersToastMemory = { seen: new Map(), primed: true };
  expect(runnersToasts([row("r", "TANH", { kind: "running" })], mem, NOW)).toEqual([]);
  // Nor a "Run +20%" entry.
  expect(runnersToasts([row("g", "LEGS", { kind: "run" })], mem, NOW)).toEqual([]);
  // Nor a "Fast move" entry.
  expect(runnersToasts([row("f", "ZAP", { kind: "fast" })], mem, NOW)).toEqual([]);
});

test("old news never toasts (a reconnect, a replay's own day)", () => {
  const mem: RunnersToastMemory = { seen: new Map(), primed: true };
  expect(runnersToasts([row("r", "REPL", { at: NOW - FRESH_MS - 1 })], mem, NOW)).toEqual([]);
  expect(runnersToasts([row("r", "REPL", { state: "breaking_out", state_since: NOW - FRESH_MS - 1 })], mem, NOW)).toEqual([]);
});

test("a server's runners.event is the toast, in the same words", () => {
  expect(runnersEventToast({ type: "runners.event", kind: "new_flag", id: "a", symbol: "CJMB", at_ns: 1, path: "runner", price: 2.45, reason: "volume 12x" })).toEqual({ kind: "new", symbol: "CJMB", text: "Runners: CJMB $2.45 · volume 12x" });
  expect(runnersEventToast({ type: "runners.event", kind: "breakout", id: "a", symbol: "CJMB", at_ns: 1, path: "runner", price: 3.1, from_flag_pct: 26.53 })).toEqual({ kind: "breakout", symbol: "CJMB", text: "CJMB breaking out $3.10 · +26.5% from flag" });
});

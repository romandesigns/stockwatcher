import { describe, expect, test } from "bun:test";
import type { SettingsClientMessage, SettingsServerMessage } from "./protocol.ts";
import { applyPatch, createSettingsSync, type SettingsSyncOptions } from "./settingsSync.ts";
import { createWsClient, type WebSocketLike } from "./ws.ts";

/** A fake clock and timer queue, so throttling is tested without sleeping. */
function fakeTime() {
  let t = 1_000;
  const timers: { at: number; fn: () => void; id: number }[] = [];
  let id = 0;
  return {
    now: () => t,
    setTimer: (fn: () => void, ms: number) => {
      timers.push({ at: t + ms, fn, id: ++id });
      return id;
    },
    clearTimer: (h: unknown) => {
      const i = timers.findIndex((x) => x.id === h);
      if (i >= 0) timers.splice(i, 1);
    },
    advance(ms: number) {
      t += ms;
      for (;;) {
        const due = timers.filter((x) => x.at <= t).sort((a, b) => a.at - b.at)[0];
        if (!due) break;
        timers.splice(timers.indexOf(due), 1);
        due.fn();
      }
    },
  };
}

function rig(extra: Partial<SettingsSyncOptions> = {}) {
  const time = fakeTime();
  const saved: string[] = [];
  const rejects: string[] = [];
  let n = 0;
  const sync = createSettingsSync({
    initial: null,
    save: (raw) => saved.push(raw),
    now: time.now,
    setTimer: time.setTimer,
    clearTimer: time.clearTimer,
    newOpId: () => `op-${++n}`,
    onReject: (ns, code) => rejects.push(`${ns}:${code}`),
    ...extra,
  });
  const sent: SettingsClientMessage[] = [];
  const changes: string[] = [];
  sync.subscribe((ns) => changes.push(`${ns}:${sync.origin(ns)}`));
  const connect = (docs: { ns: string; rev: number; value: unknown }[] = []) => {
    sync.attach((m) => {
      sent.push(m);
      return true;
    });
    sync.handle({ type: "settings.snapshot", docs: docs.map((d) => ({ ...d, updatedAt: 1 })) });
  };
  const changed = (ns: string, rev: number, value: unknown, opId = "remote"): SettingsServerMessage => ({ type: "settings.changed", ns, rev, value, opId, updatedAt: 1 });
  return { sync, sent, changes, saved, rejects, time, connect, changed };
}

describe("settings sync: local edits", () => {
  test("an edit shows at once, is cached, and is sent only once the snapshot has arrived", () => {
    const r = rig();
    r.sync.set("layers", { halts: false });
    expect(r.sync.get("layers")).toEqual({ halts: false });
    expect(r.changes).toEqual(["layers:local"]);
    expect(JSON.parse(r.saved.at(-1)!).pending.layers).toHaveLength(1);
    // Attached but no snapshot yet: nothing goes out (seeds may still be moot).
    r.sync.attach((m) => (r.sent.push(m), true));
    r.sync.set("layers", { halts: true });
    expect(r.sent).toEqual([]);
    r.sync.handle({ type: "settings.snapshot", docs: [] });
    // Coalesced while unsent: one write with the latest value.
    expect(r.sent).toEqual([{ type: "settings.set", ns: "layers", value: { halts: true }, opId: "op-2" }]);
  });

  test("the first change goes at once, a burst is coalesced and the final value is flushed", () => {
    const r = rig();
    r.connect();
    r.sync.set("chartSettings", { zoom: 25 });
    expect(r.sent).toHaveLength(1);
    r.sync.set("chartSettings", { zoom: 50 });
    r.time.advance(50);
    r.sync.set("chartSettings", { zoom: 100 });
    expect(r.sent).toHaveLength(1);
    r.time.advance(150);
    expect(r.sent).toHaveLength(2);
    expect(r.sent[1]).toMatchObject({ type: "settings.set", value: { zoom: 100 } });
    // Quiet for a while: the next change goes immediately again.
    r.time.advance(1000);
    r.sync.set("chartSettings", { zoom: 25 });
    expect(r.sent).toHaveLength(3);
  });

  test("patches fold over the value and coalesce entry by entry", () => {
    expect(applyPatch({ a: 1, b: 2 }, { c: 3 }, ["a"])).toEqual({ b: 2, c: 3 });
    expect(applyPatch([1], { a: 1 }, [])).toEqual({ a: 1 });
    const r = rig();
    r.sync.patch("drawings:SMPL", { d1: 1, d2: 2 });
    r.sync.patch("drawings:SMPL", { d3: 3 }, ["d1"]);
    r.sync.patch("drawings:SMPL", { d1: 9 });
    r.sync.patch("drawings:SMPL", {}, []);
    expect(r.sync.get("drawings:SMPL")).toEqual({ d2: 2, d3: 3, d1: 9 });
    r.connect();
    expect(r.sent).toEqual([{ type: "settings.patch", ns: "drawings:SMPL", opId: "op-3", set: { d1: 9, d2: 2, d3: 3 } }]);
  });
});

describe("settings sync: the server's answers", () => {
  test("our own echo changes nothing; another device's change shows as remote", () => {
    const r = rig();
    r.connect();
    r.sync.set("indicators", ["vwap"]);
    r.changes.length = 0;
    r.sync.handle(r.changed("indicators", 1, ["vwap"], "op-1"));
    expect(r.changes).toEqual([]);
    expect(r.sync.pendingCount()).toBe(0);
    r.sync.handle(r.changed("indicators", 2, ["ema-9"]));
    expect(r.sync.get("indicators")).toEqual(["ema-9"]);
    expect(r.changes).toEqual(["indicators:remote"]);
    // A stale or repeated rev is ignored.
    r.sync.handle(r.changed("indicators", 1, ["old"]));
    expect(r.sync.get("indicators")).toEqual(["ema-9"]);
    // Nothing was ever written back in response.
    expect(r.sent).toHaveLength(1);
  });

  test("a remote change while our edit is in flight never flickers, and both sides converge", () => {
    const r = rig();
    r.connect([{ ns: "layers", rev: 1, value: { halts: true } }]);
    r.sync.set("layers", { halts: false });
    r.changes.length = 0;
    // The server applied another device's write first (rev 2), then ours (rev 3).
    r.sync.handle(r.changed("layers", 2, { luld: false }));
    expect(r.sync.get("layers")).toEqual({ halts: false });
    r.sync.handle(r.changed("layers", 3, { halts: false }, "op-1"));
    expect(r.sync.get("layers")).toEqual({ halts: false });
    expect(r.changes).toEqual([]);
  });

  test("a refused edit is dropped and the UI falls back; an unavailable store keeps it for later", () => {
    const r = rig();
    r.connect([{ ns: "layers", rev: 4, value: { halts: true } }]);
    r.sync.set("layers", { halts: false });
    r.sync.handle({ type: "settings.rejected", ns: "layers", opId: "op-1", code: "too_large", message: "" });
    expect(r.sync.get("layers")).toEqual({ halts: true });
    expect(r.rejects).toEqual(["layers:too_large"]);

    r.sync.set("layers", { open: true });
    r.sync.handle({ type: "settings.rejected", ns: "layers", opId: "op-2", code: "settings_unavailable", message: "" });
    expect(r.sync.get("layers")).toEqual({ open: true });
    expect(r.sync.pendingCount()).toBe(1);
    r.sync.detach();
    r.connect([{ ns: "layers", rev: 4, value: { halts: true } }]);
    expect(r.sent.at(-1)).toMatchObject({ opId: "op-2", value: { open: true } });
  });

  test("a value too large for one frame is refused before it is sent", () => {
    const r = rig();
    r.connect();
    r.sync.set("big", "x".repeat(70_000));
    expect(r.sent).toEqual([]);
    expect(r.sync.get("big")).toBeUndefined();
    expect(r.rejects).toEqual(["big:too_large"]);
  });
});

describe("settings sync: migration of existing local settings", () => {
  test("the first client seeds a namespace the server has never had", () => {
    const r = rig();
    r.sync.seed("indicators", ["vwap", "ema-9"]);
    expect(r.sync.get("indicators")).toEqual(["vwap", "ema-9"]);
    r.connect([]);
    expect(r.sent).toEqual([{ type: "settings.set", ns: "indicators", value: ["vwap", "ema-9"], opId: "op-1", ifAbsent: true }]);
  });

  test("a later client adopts the server's value over its own local one", () => {
    const r = rig();
    r.sync.seed("indicators", ["rsi-14"]);
    r.changes.length = 0;
    r.connect([{ ns: "indicators", rev: 3, value: ["vwap"] }]);
    expect(r.sent).toEqual([]);
    expect(r.sync.get("indicators")).toEqual(["vwap"]);
    expect(r.changes).toEqual(["indicators:remote"]);
  });

  test("losing the seeding race: the server answers with the shared value and it is adopted", () => {
    const r = rig();
    r.sync.seed("layers", { halts: false });
    r.connect([]);
    r.sync.handle(r.changed("layers", 1, { halts: true }, "op-1"));
    expect(r.sync.get("layers")).toEqual({ halts: true });
    expect(r.sync.pendingCount()).toBe(0);
  });

  test("an edit made on top of a seed survives the seed being dropped", () => {
    const r = rig();
    r.sync.seed("drawings:SMPL", { d1: 1 });
    r.sync.patch("drawings:SMPL", { d2: 2 });
    r.connect([{ ns: "drawings:SMPL", rev: 1, value: { d9: 9 } }]);
    expect(r.sync.get("drawings:SMPL")).toEqual({ d9: 9, d2: 2 });
    expect(r.sent).toEqual([{ type: "settings.patch", ns: "drawings:SMPL", opId: "op-2", set: { d2: 2 } }]);
  });

  test("seed is a no-op once the namespace is known here", () => {
    const r = rig();
    r.connect([{ ns: "layers", rev: 1, value: { a: 1 } }]);
    r.sync.seed("layers", { a: 2 });
    r.sync.seed("other", undefined);
    expect(r.sync.get("layers")).toEqual({ a: 1 });
    expect(r.sync.pendingCount()).toBe(0);
  });

  test("a server that lost its data is re-seeded from what this device last knew", () => {
    const r = rig();
    r.connect([{ ns: "layers", rev: 7, value: { halts: false } }]);
    r.sync.detach();
    r.connect([]);
    expect(r.sent).toEqual([{ type: "settings.set", ns: "layers", value: { halts: false }, opId: "op-1", ifAbsent: true }]);
    expect(r.sync.get("layers")).toEqual({ halts: false });
  });
});

describe("settings sync: offline and restarts", () => {
  test("edits made offline survive a restart (coalesced) and go out after the next snapshot", () => {
    const a = rig();
    a.connect([{ ns: "layers", rev: 2, value: { halts: true } }]);
    a.sync.detach();
    a.sync.set("layers", { halts: false });
    a.sync.set("layers", { halts: false, luld: false });
    expect(a.sent).toEqual([]);

    // The app restarts from the cache, still offline: the last known value shows at once.
    const b = rig({ initial: a.saved.at(-1)! });
    expect(b.sync.get("layers")).toEqual({ halts: false, luld: false });
    b.connect([{ ns: "layers", rev: 3, value: { other: "device" } }]);
    // Last writer wins: the offline edit is the latest action, so it is sent.
    expect(b.sent).toEqual([{ type: "settings.set", ns: "layers", value: { halts: false, luld: false }, opId: "op-2" }]);
    expect(b.sync.get("layers")).toEqual({ halts: false, luld: false });
  });

  test("an edit in flight when the connection drops is sent again after reconnecting", () => {
    const r = rig();
    r.connect();
    r.sync.set("chartSettings", { grid: false });
    expect(r.sent).toHaveLength(1);
    r.sync.detach();
    r.connect([]);
    expect(r.sent).toHaveLength(2);
    expect(r.sent[1]).toMatchObject({ opId: "op-1", value: { grid: false } });
  });

  test("a corrupt cache is ignored rather than breaking startup", () => {
    for (const initial of ["not json", '{"v":2}', '{"v":1,"docs":[],"pending":{}}', "null"]) {
      const r = rig({ initial });
      expect(r.sync.namespaces()).toEqual([]);
    }
    const r = rig({ initial: JSON.stringify({ v: 1, docs: { a: { rev: 1, value: 1 }, b: "junk" }, pending: { a: [{ kind: "nope" }, { opId: "x", kind: "set", value: 2 }] } }) });
    expect(r.sync.get("a")).toBe(2);
    expect(r.sync.get("b")).toBeUndefined();
  });
});

describe("settings sync: views", () => {
  test("every value goes through the app's parser, and the parsed object is stable", () => {
    const r = rig();
    const parse = (raw: unknown) => ({ halts: typeof raw === "object" && raw !== null && (raw as { halts?: unknown }).halts === true });
    const view = r.sync.view("layers", parse);
    let calls = 0;
    const off = view.subscribe(() => calls++);
    expect(view.get()).toEqual({ halts: false });
    expect(view.get()).toBe(view.get());
    r.connect([{ ns: "layers", rev: 1, value: "garbage from an old build" }]);
    expect(view.get()).toEqual({ halts: false });
    r.sync.handle(r.changed("layers", 2, { halts: true }));
    expect(view.get()).toEqual({ halts: true });
    expect(view.origin()).toBe("remote");
    expect(calls).toBe(2);
    // A second parser on the same namespace does not evict the first one's object.
    const first = view.get();
    const other = r.sync.view("layers", (raw) => raw);
    other.get();
    expect(view.get()).toBe(first);
    off();
    r.sync.handle(r.changed("layers", 3, { halts: false }));
    expect(calls).toBe(2);
  });
});

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

describe("settings sync over the shared ws client", () => {
  test("attached on welcome, fed settings frames, resynced after a reconnect", async () => {
    FakeSocket.instances = [];
    const sync = createSettingsSync({ initial: null, newOpId: () => "op-x" });
    const client = createWsClient({ url: "ws://x/ws", settings: sync, createSocket: (u) => new FakeSocket(u), baseDelayMs: 1, maxDelayMs: 1, random: () => 0 });
    const s1 = FakeSocket.instances[0]!;
    s1.open();
    s1.receive({ type: "welcome", version: "0", commit: "c", heartbeatSecs: 15 });
    s1.receive({ type: "settings.snapshot", docs: [{ ns: "layers", rev: 1, updatedAt: 1, value: { halts: false } }] });
    expect(sync.get("layers")).toEqual({ halts: false });
    sync.set("layers", { halts: true });
    expect(JSON.parse(s1.sent.at(-1)!)).toEqual({ type: "settings.set", ns: "layers", value: { halts: true }, opId: "op-x" });

    // The connection drops before the echo: the edit is resent on the next connection.
    s1.close();
    await new Promise((r) => setTimeout(r, 10));
    const s2 = FakeSocket.instances[1]!;
    s2.open();
    s2.receive({ type: "welcome", version: "0", commit: "c", heartbeatSecs: 15 });
    expect(s2.sent.filter((m) => m.includes("settings.set"))).toEqual([]);
    s2.receive({ type: "settings.snapshot", docs: [{ ns: "layers", rev: 1, updatedAt: 1, value: { halts: false } }] });
    expect(sync.get("layers")).toEqual({ halts: true });
    expect(s2.sent.filter((m) => m.includes("settings.set"))).toHaveLength(1);
    s2.receive({ type: "settings.changed", ns: "layers", rev: 2, updatedAt: 2, value: { halts: true }, opId: "op-x" });
    expect(sync.pendingCount()).toBe(0);
    client.close();
  });
});

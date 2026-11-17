import { expect, test } from "bun:test";
import { DEFAULT_RUNNERS_NOTIFY, type RunnersNoticeMessage } from "@stockwatcher/shared";

import { toastStandsDown } from "../dashboard/runnersToasts.ts";
import { createRunnersNotifier, NOTIFY_ICON, type NotifyEnv } from "./runnersNotify.ts";
import { breakoutTestFixture } from "../../../../packages/shared/src/breakoutTestFixture.ts";

// The server's frame (crates/server/src/alerts/runners.rs desk_frame): the phone push's text.
const notice: RunnersNoticeMessage = {
  type: "runners.notice",
  id: "runners-burst-2026-10-23-CJMB-7",
  alertId: "2026-10-23-CJMB-7",
  symbol: "CJMB",
  kind: "runners.burst",
  title: "CJMB · Buying burst · $2.10",
  body: "+18.2% in 6 min · typ. +13% max in 30 min, 1 in 4 reach +20% before −5%",
  at: 1,
};

function memoryStorage() {
  const m = new Map<string, string>();
  return { getItem: (k: string) => m.get(k) ?? null, setItem: (k: string, v: string) => void m.set(k, v) };
}

class FakeNotification {
  static permission: NotificationPermission = "default";
  static shown: FakeNotification[] = [];
  static requestPermission = async () => (FakeNotification.permission = "granted");
  onclick: ((e: { preventDefault(): void }) => void) | null = null;
  closed = false;
  title: string;
  opts: NotificationOptions;
  constructor(title: string, opts: NotificationOptions) {
    this.title = title;
    this.opts = opts;
    FakeNotification.shown.push(this);
  }
  close() {
    this.closed = true;
  }
}

function webEnv(extra: Partial<NotifyEnv> = {}) {
  FakeNotification.permission = "default";
  FakeNotification.shown = [];
  const calls: string[] = [];
  const env: NotifyEnv = {
    storage: memoryStorage(),
    invoke: null,
    Notification: FakeNotification as unknown as NotifyEnv["Notification"],
    focus: () => calls.push("focus"),
    openChart: (s) => calls.push(`chart ${s}`),
    ...extra,
  };
  return { env, calls };
}

test("web: off until the switch asks for permission; the phone's defaults after", async () => {
  const { env } = webEnv();
  const n = createRunnersNotifier(env);
  expect(n.desktop).toBe(false);
  expect(n.prefs()).toEqual({ enabled: false, kinds: { ...DEFAULT_RUNNERS_NOTIFY }, haltSounds: { halted: "siren", resumed: "chime" } });
  expect(n.prefs().kinds.breakout).toBe(false);
  // Without system notifications, only the exchange halts (their sound and an in-app toast).
  expect(n.notifyMessage()).toEqual({ type: "runners.notify", enabled: true, new: false, breakout: false, burst: false, running: false, run: false, fast: false, shelf: false, followUp: false, rossCrossing: false, signalsCrossing: false, rossTrigger: false, signalsTrigger: false, halt: false, pullback: false, halted: true, resumed: true });
  expect(n.handle(notice)).toBe(false);
  expect(await n.enable()).toBe(true);
  expect(n.notifyMessage()).toEqual({ type: "runners.notify", enabled: true, new: true, breakout: false, burst: true, running: true, run: true, fast: true, shelf: true, followUp: true, rossCrossing: true, signalsCrossing: true, rossTrigger: false, signalsTrigger: false, halt: false, pullback: true, halted: true, resumed: true });
  // Remembered on this device.
  expect(createRunnersNotifier(env).prefs().enabled).toBe(true);
});

test("web: a denied permission stays off", async () => {
  const { env } = webEnv();
  FakeNotification.requestPermission = async () => (FakeNotification.permission = "denied");
  const n = createRunnersNotifier(env);
  expect(await n.enable()).toBe(false);
  expect(n.active()).toBe(false);
  FakeNotification.requestPermission = async () => (FakeNotification.permission = "granted");
});

test("web: the server's text as is, once per id, and a click opens that chart", async () => {
  const { env, calls } = webEnv();
  const n = createRunnersNotifier(env);
  await n.enable();
  expect(n.handle(notice)).toBe(true);
  // A reconnect or a second socket delivering it again; then a reload on the same storage.
  expect(n.handle(notice)).toBe(false);
  expect(createRunnersNotifier(env).handle(notice)).toBe(false);
  expect(FakeNotification.shown).toHaveLength(1);
  const shown = FakeNotification.shown[0]!;
  expect(shown.title).toBe(notice.title);
  expect(shown.opts).toMatchObject({ body: notice.body, icon: NOTIFY_ICON, tag: notice.id });
  shown.onclick?.({ preventDefault() {} });
  expect(calls).toEqual(["focus", "chart CJMB"]);
  expect(shown.closed).toBe(true);
});

test("browser and native commands carry optional frozen event context; legacy stays ticker-only", async () => {
  const opened: unknown[][] = [];
  const { env } = webEnv({ openChart: (...args) => opened.push(args) });
  const n = createRunnersNotifier(env);
  await n.enable();
  n.setPrefs({ ...n.prefs(), kinds: { ...n.prefs().kinds, breakout: true } });
  const frame: RunnersNoticeMessage = { ...notice, id: "runners-breakout-fixture", kind: "runners.breakout", data: { breakout: breakoutTestFixture } };
  const context = { event_id: frame.id, symbol: frame.symbol, breakout: breakoutTestFixture };
  expect(n.handle(frame)).toBe(true);
  FakeNotification.shown[0]!.onclick?.({ preventDefault() {} });
  expect(opened).toEqual([["CJMB", context]]);
  const commands: Record<string, unknown>[] = [];
  const desktop = createRunnersNotifier(webEnv({ invoke: async (_cmd, args) => void commands.push(args) }).env);
  desktop.setPrefs({ ...desktop.prefs(), kinds: { ...desktop.prefs().kinds, breakout: true } });
  expect(desktop.handle(frame)).toBe(true);
  expect(commands[0]?.noticeContext).toEqual(context);
});

test("a kind switched off is not shown, and its toast keeps toasting", async () => {
  const { env } = webEnv();
  const n = createRunnersNotifier(env);
  await n.enable();
  expect(n.handle({ ...notice, id: "runners-breakout-x", kind: "runners.breakout" })).toBe(false);
  expect(toastStandsDown("breakout", n)).toBe(false);
  expect(toastStandsDown("new", n)).toBe(true);
  n.setPrefs({ ...n.prefs(), kinds: { ...n.prefs().kinds, burst: false } });
  expect(n.handle({ ...notice, id: "runners-burst-y" })).toBe(false);
  // Off altogether: the dashboard toasts as before.
  n.setPrefs({ ...n.prefs(), enabled: false });
  expect(toastStandsDown("new", n)).toBe(false);
});

test("Ross, Signals, shelf, follow-up and Halt notices follow their switches; a Ross crossing in Signals too shows with either", async () => {
  const { env, calls } = webEnv();
  const n = createRunnersNotifier(env);
  await n.enable();
  const ross = { ...notice, id: "ross.crossing-e1", kind: "ross.crossing" as const, title: "CJMB · Ross crossing · $2.10", alsoSignals: true };
  n.setPrefs({ ...n.prefs(), kinds: { ...n.prefs().kinds, rossCrossing: false } });
  expect(n.handle(ross)).toBe(true);
  expect(n.handle({ ...ross, id: "ross.crossing-e2", alsoSignals: undefined })).toBe(false);
  expect(n.handle({ ...notice, id: "runners.shelf-a", kind: "runners.shelf" })).toBe(true);
  expect(n.handle({ ...notice, id: "runners.followup-a", kind: "runners.followup" })).toBe(true);
  // Opt-in: off by default.
  expect(n.handle({ ...notice, id: "halt.watch-a", kind: "halt.watch" })).toBe(false);
  expect(n.handle({ ...notice, id: "signals.trigger-a", kind: "signals.trigger" })).toBe(false);
  n.setPrefs({ ...n.prefs(), kinds: { ...n.prefs().kinds, halt: true } });
  expect(n.handle({ ...notice, id: "halt.watch-b", kind: "halt.watch" })).toBe(true);
  expect(n.notifyMessage()).toMatchObject({ rossCrossing: false, halt: true });
  // Every one opens its ticker's chart on a click.
  FakeNotification.shown[0]!.onclick?.({ preventDefault() {} });
  expect(calls).toEqual(["focus", "chart CJMB"]);
});

test("desktop shell: on by default, shown through its command, switches resent to the server", () => {
  const invoked: unknown[] = [];
  const { env } = webEnv({ Notification: null, invoke: async (cmd, args) => invoked.push([cmd, args]) });
  const n = createRunnersNotifier(env);
  expect(n.desktop).toBe(true);
  expect(n.permission()).toBe("granted");
  expect(n.prefs().enabled).toBe(true);
  let refreshed = 0;
  const detach = n.attach(() => refreshed++);
  expect(n.handle(notice)).toBe(true);
  expect(invoked).toEqual([["runners_notify", { id: notice.id, title: notice.title, body: notice.body, symbol: "CJMB" }]]);
  n.setPrefs({ ...n.prefs(), kinds: { ...n.prefs().kinds, fast: false } });
  expect(refreshed).toBe(1);
  expect(n.notifyMessage()).toMatchObject({ fast: false });
  detach();
  n.setPrefs({ ...n.prefs(), enabled: false });
  expect(refreshed).toBe(1);
  expect(n.notifyMessage()).toMatchObject({ new: false, fast: false, halted: true, resumed: true });
  n.setPrefs({ ...n.prefs(), kinds: { ...n.prefs().kinds, halted: false, resumed: false } });
  expect(n.notifyMessage()).toBeNull();
});

/** Exchange halts (plan "halt sounds", 2026-11-16): the device's sound, with or without system notifications. */
test("halts play this device's sound; without notifications they toast, and a locked page says how to allow sound", async () => {
  const halted: RunnersNoticeMessage = { ...notice, id: "runners-halted-XLAB-halt-1", alertId: "XLAB-halt-1", symbol: "XLAB", kind: "halt.halted", title: "XLAB · Halted · $2.76", body: "LULD pause · +42.3% on the day", data: { symbol: "XLAB", kind: "halt.halted", sound: "siren" } };
  const resumed: RunnersNoticeMessage = { ...halted, id: "runners-resumed-XLAB-halt-1", kind: "halt.resumed", title: "XLAB · Resumed · $3.10", body: "Halted 2 min · +12.3% from $2.76", data: { symbol: "XLAB", kind: "halt.resumed", sound: "chime" } };
  const played: string[] = [];
  const toasts: [string, boolean][] = [];
  let unlocked = true;
  const { env } = webEnv({ playHalt: (k) => (unlocked && k !== "none" ? (played.push(k), true) : false), soundUnlocked: () => unlocked, toast: (m, locked) => void toasts.push([m.id, locked]) });
  const n = createRunnersNotifier(env);
  // Notifications off: the sound and a toast, once per event.
  expect(n.handle(halted)).toBe(true);
  expect(n.handle(halted)).toBe(false);
  expect(played).toEqual(["siren"]);
  expect(toasts).toEqual([[halted.id, false]]);
  expect(FakeNotification.shown).toHaveLength(0);
  // The device's own pick for Resumed; the page has not had its click: the toast says so.
  n.setPrefs({ ...n.prefs(), haltSounds: { halted: "siren", resumed: "ding" } });
  expect(createRunnersNotifier(env).prefs().haltSounds).toEqual({ halted: "siren", resumed: "ding" });
  unlocked = false;
  expect(n.handle(resumed)).toBe(true);
  expect(toasts.at(-1)).toEqual([resumed.id, true]);
  unlocked = true;
  // With notifications on: a silent system notification (the sound is ours), no toast.
  await n.enable();
  expect(n.handle({ ...halted, id: "runners-halted-XLAB-halt-2" })).toBe(true);
  expect(played).toEqual(["siren", "siren"]);
  expect(FakeNotification.shown.at(-1)!.opts.silent).toBe(true);
  expect(toasts).toHaveLength(2);
  // "None" plays nothing and is not a locked sound.
  n.setPrefs({ ...n.prefs(), haltSounds: { halted: "none", resumed: "ding" } });
  expect(n.handle({ ...halted, id: "runners-halted-XLAB-halt-3" })).toBe(true);
  expect(played).toEqual(["siren", "siren"]);
  // A switched-off event is not shown; the Settings test always is, and opens no chart.
  n.setPrefs({ ...n.prefs(), kinds: { ...n.prefs().kinds, halted: false } });
  expect(n.handle({ ...halted, id: "runners-halted-XLAB-halt-4" })).toBe(false);
  const test1: RunnersNoticeMessage = { ...halted, id: "runners-halted-test-1", symbol: "TEST", data: { symbol: "TEST", kind: "halt.halted", sound: "siren", test: true } };
  expect(n.handle(test1)).toBe(true);
  FakeNotification.shown.at(-1)!.onclick?.({ preventDefault() {} });
});

test("Send test goes out on an attached connection", () => {
  const { env } = webEnv();
  const n = createRunnersNotifier(env);
  expect(n.sendHaltTest("halted")).toBe(false);
  const sent: unknown[] = [];
  const detach = n.attach(
    () => {},
    (m) => (sent.push(m), true),
  );
  expect(n.sendHaltTest("resumed")).toBe(true);
  expect(sent).toEqual([{ type: "halts.test", event: "resumed" }]);
  detach();
  expect(n.sendHaltTest("halted")).toBe(false);
});

/**
 * WFF 2026-10-26: Running, then Fast move 1 s later. Since 2026-10-30 every distinct event is its
 * own notification (server alerts/runners.rs): two notifications, each tagged by its own id, on
 * the browser and the desktop shell alike.
 */
test("two notices on one stock moments apart are two notifications", async () => {
  const running: RunnersNoticeMessage = { ...notice, id: "runners-running-WFF-1", kind: "runners.running", symbol: "WFF", title: "WFF · Running · $0.1019", body: "+32.2% in 60 min\nAlert $0.1019 · +5% = $0.1070" };
  const fast: RunnersNoticeMessage = { ...notice, id: "runners-fast-WFF-1", kind: "runners.fast", symbol: "WFF", title: "WFF · Fast move · $0.1057", body: "b" };
  const { env } = webEnv();
  const n = createRunnersNotifier(env);
  await n.enable();
  expect(n.handle(running)).toBe(true);
  expect(n.handle(fast)).toBe(true);
  expect(FakeNotification.shown.map((x) => x.opts.tag)).toEqual([running.id, fast.id]);
  expect(FakeNotification.shown[1]!.title).toBe(fast.title);
  // Delivered again (a reconnect): once only.
  expect(n.handle(fast)).toBe(false);
  const invoked: unknown[] = [];
  const desk = createRunnersNotifier(webEnv({ Notification: null, invoke: async (cmd, args) => void invoked.push([cmd, args]) }).env);
  expect(desk.handle(running)).toBe(true);
  expect(desk.handle(fast)).toBe(true);
  expect(invoked).toHaveLength(2);
});

test("storage that throws (private mode) never breaks it", async () => {
  const boom = () => {
    throw new Error("denied");
  };
  const { env } = webEnv({ storage: { getItem: boom, setItem: boom } });
  const n = createRunnersNotifier(env);
  await n.enable();
  expect(n.handle(notice)).toBe(true);
  expect(n.handle(notice)).toBe(false);
});

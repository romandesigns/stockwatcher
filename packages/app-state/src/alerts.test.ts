import { describe, expect, test } from "bun:test";
import { etToUtc } from "@stockwatcher/chart-core";
import { createSettingsSync, type AlertEventWire, type SettingsClientMessage } from "@stockwatcher/shared";
import {
  alertDisplay,
  ALERT_TONES,
  alertsOnDrawing,
  alertSoundFor,
  ALERTS_NS,
  conditionText,
  crossDirFor,
  DEFAULT_ALERT_PREFS,
  editedAlert,
  expiresAtFor,
  inQuietHours,
  isActiveAlert,
  lineLabel,
  mergeActivity,
  metaText,
  newAlert,
  newDrawingAlert,
  parseAlertPrefs,
  parseAlertRules,
  parseAlertStates,
  shortConditionText,
} from "./alerts.ts";
import { createAppStore } from "./store.ts";

// Monday 2026-10-19, ET.
const at = (day: number, h: number, m = 0) => etToUtc(2026, 10, day, h, m);

describe("alert model", () => {
  test("rules parse defensively, junk skipped, fields defaulted", () => {
    const rules = parseAlertRules({
      a: { symbol: "xlab", rule: { kind: "price", dir: "below", price: 2.1 }, fire: "every", gapMin: 90, sessions: ["regular", "x"], createdAt: 2 },
      b: { symbol: "SAFX", rule: { kind: "move", dir: "sideways", pct: 5, window: "2h" }, createdAt: 1 },
      p: { symbol: "OPENW", rule: { kind: "pressure", side: "buy", min: 80 }, createdAt: 3 },
      bad: { symbol: "SAFX", rule: { kind: "pressure", min: 0 } },
      worse: "junk",
    });
    expect(rules.map((r) => r.id)).toEqual(["b", "a", "p"]);
    expect(conditionText(rules[2]!.rule)).toBe("Buy pressure 80 or more (Halt watcher)");
    expect(rules[1]).toMatchObject({ symbol: "XLAB", fire: "every", gapMin: 60, sessions: ["regular"], expiry: "week", notify: { app: true, phone: true }, editedAt: 2 });
    expect(rules[0]!.rule).toEqual({ kind: "move", dir: "up", pct: 5, window: "5m" });
    expect(parseAlertRules([1])).toEqual([]);
  });

  test("states and prefs", () => {
    expect(parseAlertStates({ a: { status: "cooling", until: 5, basis: 1, count: 2 }, b: 7 })).toEqual({ a: { status: "cooling", until: 5, basis: 1, count: 2 } });
    expect(parseAlertPrefs(null)).toEqual(DEFAULT_ALERT_PREFS);
    expect(parseAlertPrefs({ phoneBudget: 50, quietHours: { on: true, start: "23:00", end: "6:00" }, sessions: [] })).toEqual({
      phoneBudget: 50,
      quietHours: { on: true, start: "23:00", end: "07:00" },
      sessions: ["pre", "regular", "after"],
      sound: { on: true, volume: 0.6 },
    });
    expect(parseAlertPrefs({ sound: { on: false, volume: 3 } }).sound).toEqual({ on: false, volume: 1 });
  });

  test("drawing alerts: parse, defaults, text, orphaned, sounds", () => {
    const rules = parseAlertRules({
      d: { symbol: "XLAB", rule: { kind: "drawing", drawingId: "dr-1", part: "stop", tol: { unit: "pct", value: 0.5 }, tf: "5m" }, sound: false, createdAt: 1 },
      e: { symbol: "XLAB", rule: { kind: "drawing", drawingId: "dr-2", part: "nope", tol: "junk", tf: "1h" }, createdAt: 2 },
      bad: { symbol: "XLAB", rule: { kind: "drawing", drawingId: "" } },
    });
    expect(rules.map((r) => r.id)).toEqual(["d", "e"]);
    expect(rules[0]!.rule).toEqual({ kind: "drawing", drawingId: "dr-1", part: "stop", tol: { unit: "pct", value: 0.5 }, tf: "5m" });
    expect(rules[0]!.sound).toBe(false);
    expect(rules[1]!.rule).toEqual({ kind: "drawing", drawingId: "dr-2", part: "line", tol: { unit: "ticks", value: 1 }, tf: "1m" });
    expect(rules[1]!.sound).toBe(true);
    expect(conditionText(rules[0]!.rule)).toBe("Touches the drawing's stop (±0.5%, 5m chart)");
    expect(shortConditionText(rules[1]!.rule)).toBe("Touches drawing");
    expect(metaText(rules[0]!, at(19, 10))).toContain("sound off");

    // Defaults: one tick, once, never expiring, After added to the sessions picked last.
    const prefs = { ...DEFAULT_ALERT_PREFS, sessions: ["regular" as const] };
    const n = newDrawingAlert("xlab", { id: "dr-9" }, "stop", "1m", prefs, at(19, 17), "n1");
    expect(n).toMatchObject({ id: "n1", symbol: "XLAB", fire: "once", expiry: "never", expiresAt: null, sessions: ["regular", "after"], sound: true, notify: { app: true, phone: true } });
    expect(n.rule).toEqual({ kind: "drawing", drawingId: "dr-9", part: "stop", tol: { unit: "ticks", value: 1 }, tf: "1m" });
    expect(alertsOnDrawing([n, ...rules], "XLAB", "dr-9").map((r) => r.id)).toEqual(["n1"]);

    // A deleted drawing: the server's word, shown as such, no longer active.
    const st = parseAlertStates({ n1: { status: "armed", basis: at(19, 17), count: 0, snoozed: false, orphaned: true } });
    expect(st.n1!.orphaned).toBe(true);
    expect(alertDisplay(n, st.n1, at(19, 18))).toEqual({ kind: "orphaned", label: "Drawing deleted" });
    expect(isActiveAlert(n, st.n1, at(19, 18))).toBe(false);

    // Sounds: drawing fires only, the alert's chip, the global switch, quiet hours.
    const fire = { tone: "stop" as const, alertId: "n1" };
    expect(alertSoundFor(fire, n, DEFAULT_ALERT_PREFS, at(19, 17))).toBe("stop");
    expect(alertSoundFor({ alertId: "x" }, n, DEFAULT_ALERT_PREFS, at(19, 17))).toBeNull();
    expect(alertSoundFor(fire, { sound: false }, DEFAULT_ALERT_PREFS, at(19, 17))).toBeNull();
    expect(alertSoundFor(fire, n, { ...DEFAULT_ALERT_PREFS, sound: { on: false, volume: 0.6 } }, at(19, 17))).toBeNull();
    const quiet = { ...DEFAULT_ALERT_PREFS, quietHours: { on: true, start: "22:00", end: "07:00" } };
    expect(inQuietHours(quiet, at(19, 23))).toBe(true);
    expect(inQuietHours(quiet, at(19, 6))).toBe(true);
    expect(inQuietHours(quiet, at(19, 17))).toBe(false);
    expect(alertSoundFor(fire, n, quiet, at(19, 23))).toBeNull();
    expect(ALERT_TONES.up.map((x) => x.hz)).toEqual([660, 880]);
  });

  test("expiry: end of day is 20:00 ET (next weekday after it), end of week Friday 20:00", () => {
    expect(expiresAtFor("day", at(19, 10))).toBe(at(19, 20));
    expect(expiresAtFor("day", at(19, 21))).toBe(at(20, 20));
    // Friday evening → Monday.
    expect(expiresAtFor("day", at(23, 21))).toBe(at(26, 20));
    expect(expiresAtFor("week", at(19, 10))).toBe(at(23, 20));
    expect(expiresAtFor("week", at(23, 21))).toBe(at(30, 20));
    expect(expiresAtFor("never", at(19, 10))).toBeNull();
  });

  test("defaults, edits and text", () => {
    const now = at(19, 10);
    expect(crossDirFor(2.6, 2.5)).toBe("above");
    expect(crossDirFor(2.4, 2.5)).toBe("below");
    const a = newAlert("xlab", { kind: "price", dir: "above", price: 2.5 }, DEFAULT_ALERT_PREFS, now, "a1");
    expect(a).toMatchObject({ symbol: "XLAB", fire: "once", expiry: "week", expiresAt: at(23, 20), sessions: ["pre", "regular", "after"] });
    expect(lineLabel(a)).toBe("above 2.50 · once");
    expect(conditionText(a.rule)).toBe("Price crosses above 2.50");
    expect(metaText(a, now)).toBe("Once · pre, regular, after · expires Fri");
    expect(shortConditionText({ kind: "move", dir: "up", pct: 5, window: "5m" })).toBe("Up 5% in 5 min");
    expect(conditionText({ kind: "luld", side: "up", pct: 2 })).toBe("Within 2% of the limit-up band (modelled)");
    const moved = editedAlert(a, { rule: { kind: "price", dir: "above", price: 2.55 } }, now + 1000);
    expect(moved.editedAt).toBe(now + 1000);
    expect(lineLabel({ ...moved, fire: "every" })).toBe("above 2.55 · every time");
  });

  test("display state follows the server's state for this version of the rule", () => {
    const now = at(19, 10);
    const a = newAlert("XLAB", { kind: "price", dir: "above", price: 2.5 }, DEFAULT_ALERT_PREFS, now, "a1");
    expect(alertDisplay(a, undefined, now)).toEqual({ kind: "armed", label: "Armed" });
    expect(alertDisplay(a, { status: "cooling", until: now + 9 * 60_000 - 1, basis: now, count: 1 }, now).label).toBe("Cooling · 9 min");
    expect(alertDisplay(a, { status: "cooling", until: now - 1, basis: now, count: 1 }, now).label).toBe("Waiting for a 0.5% move back");
    expect(alertDisplay(a, { status: "done", firedAt: at(19, 10, 42), basis: now, count: 1 }, now)).toEqual({ kind: "fired", label: "Fired 10:42" });
    // A state for an older version (the line was dragged since) does not count.
    expect(alertDisplay(a, { status: "done", basis: now - 1, count: 1 }, now).kind).toBe("armed");
    expect(alertDisplay({ ...a, snoozeUntil: now + 1 }, undefined, now).kind).toBe("snoozed");
    expect(isActiveAlert(a, { status: "expired", basis: now, count: 0 }, now)).toBe(false);
  });

  test("a state the server decided on its clock is shown as written (snoozed present)", () => {
    const now = at(19, 10);
    const a = { ...newAlert("XLAB", { kind: "price", dir: "above", price: 2.5 }, DEFAULT_ALERT_PREFS, now, "a1"), expiresAt: now - 1, snoozeUntil: now + 60_000 };
    // This device's clock says expired and snoozed; the server says armed: armed it is.
    expect(alertDisplay(a, { status: "armed", basis: now, count: 0, snoozed: false }, now).kind).toBe("armed");
    expect(alertDisplay(a, { status: "armed", basis: now, count: 0, snoozed: true }, now).kind).toBe("snoozed");
    expect(alertDisplay(a, { status: "expired", basis: now, count: 0, snoozed: false }, now).kind).toBe("expired");
    expect(isActiveAlert(a, { status: "armed", basis: now, count: 0, snoozed: false }, now)).toBe(true);
    expect(parseAlertStates({ a1: { status: "armed", basis: 1, count: 0, snoozed: true } }).a1?.snoozed).toBe(true);
    expect(parseAlertStates({ a1: { status: "armed", basis: 1, count: 0 } }).a1?.snoozed).toBeUndefined();
  });

  test("activity merges by id, newest first", () => {
    const e = (id: string, at: number): AlertEventWire => ({ id, alertId: "a", symbol: "X", kind: "fired", t: at, at, text: "", app: true, phone: "sent", note: "" });
    const m = mergeActivity([e("1", 1), e("2", 2)], [e("2", 2), e("3", 3)], false);
    expect(m.map((x) => x.id)).toEqual(["3", "2", "1"]);
    expect(mergeActivity(m, [e("9", 9)], true).map((x) => x.id)).toEqual(["9"]);
  });
});

describe("alerts in the store", () => {
  test("save, edit, delete are per-entry patches; frames update Activity and status", () => {
    let n = 0;
    const sync = createSettingsSync({ initial: null, newOpId: () => `op-${++n}`, setTimer: () => 0, clearTimer: () => {} });
    const store = createAppStore({ sync });
    const sent: SettingsClientMessage[] = [];
    sync.attach((m) => (sent.push(m), true));
    sync.handle({ type: "settings.snapshot", docs: [] });
    const s = () => store.getState();
    const a = newAlert("XLAB", { kind: "price", dir: "above", price: 2.5 }, DEFAULT_ALERT_PREFS, 1000, "a1");
    s().saveAlert({ ...a, sessions: ["regular"] });
    expect(s().alerts.map((x) => x.id)).toEqual(["a1"]);
    expect(s().alertPrefs.sessions).toEqual(["regular"]);
    expect(sent[0]).toMatchObject({ type: "settings.patch", ns: ALERTS_NS.rules });
    s().updateAlert("a1", { rule: { kind: "price", dir: "above", price: 2.6 } });
    expect(s().alerts[0]!.rule).toEqual({ kind: "price", dir: "above", price: 2.6 });
    expect(s().alerts[0]!.editedAt).toBeGreaterThan(1000);
    s().snoozeAlert("a1", 5000);
    expect(s().alerts[0]!.snoozeUntil).toBe(5000);
    // The server's state arrives like any other device's write.
    sync.handle({ type: "settings.changed", ns: ALERTS_NS.state, rev: 7, updatedAt: 1, value: { a1: { status: "done", basis: 99, count: 1 } }, opId: "srv-alerts-1" });
    expect(s().alertStates.a1?.status).toBe("done");
    s().removeAlert("a1");
    expect(s().alerts).toEqual([]);

    const ev: AlertEventWire = { id: "h1", alertId: "a1", symbol: "XLAB", kind: "fired", t: 1, at: 1, text: "Crossed above 2.50 at 2.51", app: true, phone: "sent", note: "Phone + in app" };
    s().applyAlertsMessage({ type: "alerts.history", events: [ev], phoneToday: 3, phoneBudget: 20, devices: 1, dropped: 0 });
    expect(s().alertActivity).toHaveLength(1);
    expect(s().lastFired).toBeNull();
    s().applyAlertsMessage({ type: "alert.fired", event: { ...ev, id: "h2", at: 2 }, phoneToday: 4 });
    expect(s().lastFired?.id).toBe("h2");
    expect(s().alertStatus?.phoneToday).toBe(4);
    s().applyAlertsMessage({ type: "alert.updated", event: { ...ev, id: "h2", at: 2, phone: "failed" } });
    expect(s().alertActivity[0]!.phone).toBe("failed");
    // A price alert's fire marks no drawing.
    expect(s().drawingFires).toEqual({});
    // A drawing alert's fire stays marked until acknowledged.
    s().applyAlertsMessage({ type: "alert.fired", event: { ...ev, id: "h3", alertId: "d1", at: 3, t: 30, price: 1.85, tone: "stop" }, phoneToday: 4 });
    expect(s().drawingFires).toEqual({ d1: { t: 30, price: 1.85, tone: "stop", eventId: "h3" } });
    s().acknowledgeDrawingFire("d1");
    expect(s().drawingFires).toEqual({});
  });
});

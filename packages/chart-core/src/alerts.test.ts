import { describe, expect, test } from "bun:test";
import { alertDragDeletes, alertDragPrice, alertDragText, alertPriceAt, hitAlert, layoutAlerts, stepTick, type AlertLayer } from "./alerts.ts";
import { GestureController, HOLD_ALERT_MS, HOLD_RING_DELAY_MS, MOUSE_SLOP_PX, type GestureAction, type GestureContext, type GestureInput, type ObjectHit } from "./gestures.ts";
import { computeChartModel, type ChartState } from "./layout.ts";
import { defaultViewport, manualPrice, priceY, yPrice } from "./scale.ts";
import { DEFAULT_CHART_THEME } from "./theme.ts";
import { fixtureSnapshot } from "./testFixture.ts";

// 400×300 with a 2-decimal manual range: plot x 0–358, y 0–208; price axis x 358–400. A manual price
// range 2.00–3.00 makes the y ↔ price mapping easy to reason about.
const snap = fixtureSnapshot("1m", 20, [5]);
const state: ChartState = { timeframe: "1m", candles: snap.candles, gaps: snap.gaps, profile: snap.profile, inspectT: null, pointer: null };
const model = computeChartModel(state, manualPrice(defaultViewport(16), { lo: 2, hi: 3 }), { width: 400, height: 300 });
const yAt = (p: number) => priceY(model.price!, p);
const layer = (extra: Partial<AlertLayer> = {}): AlertLayer => ({ alerts: [{ id: "a", price: 2.5, dir: "above", label: "above 2.50 · once" }], last: 2.33, ...extra });

describe("alert geometry", () => {
  test("price from a height: the chart's own scale, snapped to the tick", () => {
    expect(alertPriceAt(model, yAt(2.5))).toBe(2.5);
    expect(alertPriceAt(model, yAt(2.4567))).toBe(2.46);
    const log = computeChartModel({ ...state, priceScaleType: "log" }, manualPrice(defaultViewport(16), { lo: 0.5, hi: 0.9 }), { width: 400, height: 300 });
    const y = priceY(log.price!, 0.71234);
    expect(alertPriceAt(log, y)).toBe(0.7123);
    expect(yPrice(log.price!, y)).toBeCloseTo(0.71234, 6);
    expect(stepTick(2.5, 1)).toBe(2.51);
    expect(stepTick(1, -1)).toBe(0.9999);
    expect(stepTick(0.9999, 1)).toBe(1);
  });

  test("drag: snapped, Shift a tenth as fast, delete off the right edge", () => {
    const dy = yAt(2.55) - yAt(2.5);
    expect(alertDragPrice(model, 2.5, dy)).toBe(2.55);
    expect(alertDragPrice(model, 2.5, dy, true)).toBe(2.5);
    expect(alertDragPrice(model, 2.5, dy * 10, true)).toBe(2.55);
    expect(alertDragDeletes(model, { x: 370, y: 100 }, { x: 398, y: 100 })).toBe(true);
    expect(alertDragDeletes(model, { x: 370, y: 100 }, { x: 380, y: 100 })).toBe(false);
    expect(alertDragDeletes(model, { x: 100, y: 100 }, { x: 300, y: 100 })).toBe(false);
    expect(alertDragText(2.5, 2.55, 2.5)).toBe("2.50 → 2.55 · +2.0%");
  });

  test("hits: the axis tag (44 px for a finger), the label, the line, the bell", () => {
    const y = yAt(2.5);
    expect(hitAlert(model, layer(), 372, y + 8, 6, false)).toEqual({ kind: "alert", id: "a", part: "tag" });
    expect(hitAlert(model, layer(), 372, y + 20, 6, false)).toBeNull();
    expect(hitAlert(model, layer(), 372, y + 20, 6, true)).toEqual({ kind: "alert", id: "a", part: "tag" });
    expect(hitAlert(model, layer(), 100, y + 4, 6, false)).toEqual({ kind: "alert", id: "a", part: "line" });
    expect(hitAlert(model, layer(), 335, y, 6, false)).toEqual({ kind: "alert", id: "a", part: "tag" });
    // At the plot's left edge, the far side from the price axis (owner, 2026-10-31).
    expect(hitAlert(model, layer({ bellY: 50 }), 12, 50, 6, false)).toEqual({ kind: "placeAlert", y: 50 });
    expect(hitAlert(model, layer({ bellY: 50 }), 346, 50, 6, false)).toBeNull();
    // Beside the "+", the bell moves right of it.
    expect(hitAlert(model, layer({ bellY: 50, besidePlace: true }), 12, 50, 6, false)).toBeNull();
    expect(hitAlert(model, layer({ bellY: 50, besidePlace: true }), 34, 50, 6, false)).toEqual({ kind: "placeAlert", y: 50 });
  });

  test("drawn as a dashed accent line, a label and an axis tag; dragging shows old → new", () => {
    const list = layoutAlerts(model, layer(), DEFAULT_CHART_THEME);
    expect(list.some((p) => p.kind === "line" && p.dash?.length === 2 && p.stroke === DEFAULT_CHART_THEME.alert)).toBe(true);
    expect(list.some((p) => p.kind === "text" && p.text === "above 2.50 · once")).toBe(true);
    expect(list.some((p) => p.kind === "text" && p.text === "2.50" && p.layer === "crosshair")).toBe(true);
    const drag = layoutAlerts(model, layer({ dragging: { id: "a", price: 2.55 } }), DEFAULT_CHART_THEME);
    expect(drag.some((p) => p.kind === "text" && p.text === "2.50 → 2.55 · +9.4%")).toBe(true);
    const del = layoutAlerts(model, layer({ dragging: { id: "a", price: 2.55, deleting: true } }), DEFAULT_CHART_THEME);
    expect(del.some((p) => p.kind === "text" && p.text === "Release to delete")).toBe(true);
    // Off the visible range: nothing (the alert still exists; the manager lists it).
    expect(layoutAlerts(model, layer({ alerts: [{ id: "b", price: 9, dir: "above", label: "x" }] }), DEFAULT_CHART_THEME)).toEqual([]);
  });
});

describe("alert gestures", () => {
  const alertHit = (x: number, y: number): ObjectHit | null => hitAlert(model, layer(), x, y, 6, false);
  const ctx = (extra: Partial<GestureContext> = {}): GestureContext => ({ model, inspectT: null, holdAlert: true, hitObject: alertHit, ...extra });
  const ev = (type: GestureInput["type"], x: number, y: number, t: number, extra: Partial<GestureInput> = {}): GestureInput => ({ type, pointerId: 1, pointerType: "mouse", x, y, t, button: 0, ...extra });
  const kinds = (a: GestureAction[]) => a.map((x) => x.kind);

  test("press and hold: the ring after 150 ms, the alert at 500 ms, at the press point", () => {
    const g = new GestureController();
    g.handle(ev("down", 100, 40, 0), ctx());
    expect(g.nextDeadline()).toBe(HOLD_RING_DELAY_MS);
    expect(g.tick(HOLD_RING_DELAY_MS - 1, ctx())).toEqual([]);
    expect(g.tick(HOLD_RING_DELAY_MS, ctx())).toEqual([{ kind: "holdRing", point: { x: 100, y: 40 }, start: 0 }]);
    expect(g.nextDeadline()).toBe(HOLD_ALERT_MS);
    // A wobble inside the slop keeps it.
    g.handle(ev("move", 102, 41, 300), ctx());
    expect(g.tick(HOLD_ALERT_MS, ctx())).toEqual([{ kind: "holdRing", point: null, start: 0 }, { kind: "holdAlert", x: 100, y: 40 }]);
    // The release after it is spent: no click.
    expect(g.handle(ev("up", 102, 41, 600), ctx())).toEqual([]);
    expect(g.state).toBe("idle");
  });

  test("a quick click never shows the ring; moving first is a pan as before", () => {
    const g = new GestureController();
    g.handle(ev("down", 100, 40, 0), ctx());
    expect(kinds(g.handle(ev("up", 100, 40, 100), ctx()))).toEqual(["select"]);
    g.handle(ev("down", 100, 40, 1000), ctx());
    g.tick(1000 + HOLD_RING_DELAY_MS, ctx());
    const acts = g.handle(ev("move", 100 + MOUSE_SLOP_PX + 1, 40, 1200), ctx());
    expect(kinds(acts)).toEqual(["holdRing", "pan"]);
    expect(g.state).toBe("pan");
    expect(g.tick(1000 + HOLD_ALERT_MS, ctx())).toEqual([]);
  });

  test("never with a drawing tool armed, nor on touch, nor on an object", () => {
    const g = new GestureController();
    g.handle(ev("down", 100, 40, 0), ctx({ tool: "trend" }));
    expect(g.tick(HOLD_ALERT_MS, ctx({ tool: "trend" }))).toEqual([]);
    const t = new GestureController();
    t.handle(ev("down", 100, 40, 0, { pointerType: "touch" }), ctx());
    expect(kinds(t.tick(HOLD_ALERT_MS, ctx()))).toEqual(["crosshair", "inspect"]);
    const o = new GestureController();
    o.handle(ev("down", 100, yAt(2.5), 0), ctx());
    expect(o.tick(HOLD_ALERT_MS, ctx())).toEqual([]);
  });

  test("an alert's axis tag is grabbed on the price axis; Shift is fine; Escape puts it back", () => {
    const g = new GestureController();
    const y = yAt(2.5);
    g.handle(ev("down", 372, y, 0), ctx());
    const first = g.handle(ev("move", 372, y - 10, 10, { shiftKey: true }), ctx());
    expect(first).toEqual([{ kind: "objectDrag", target: { kind: "alert", id: "a", part: "tag" }, from: { x: 372, y }, to: { x: 372, y: y - 10 }, done: false }]);
    expect(g.handle(ev("move", 372, y - 12, 20, { shiftKey: true }), ctx())).toEqual([{ kind: "objectDrag", target: { kind: "alert", id: "a", part: "tag" }, from: { x: 372, y }, to: { x: 372, y: y - 12 }, done: false, fine: true }]);
    expect(g.cancelDrag()).toEqual([{ kind: "objectDrag", target: { kind: "alert", id: "a", part: "tag" }, from: { x: 372, y }, to: { x: 372, y }, done: true, cancelled: true }]);
    expect(g.handle(ev("up", 372, y - 12, 30), ctx())).toEqual([]);
    expect(g.cancelDrag()).toEqual([]);
    // Elsewhere on the axis a drag still scales price.
    const s = new GestureController();
    s.handle(ev("down", 372, 20, 0), ctx());
    expect(kinds(s.handle(ev("move", 372, 40, 10), ctx()))).toEqual(["scalePrice"]);
  });

  test("the bell opens an alert at its y; lifting a touch scrub offers the chip", () => {
    const g = new GestureController();
    const bell = (x: number, yy: number): ObjectHit | null => hitAlert(model, layer({ bellY: 50 }), x, yy, 6, false);
    g.handle(ev("down", 12, 50, 0), ctx({ hitObject: bell }));
    expect(g.handle(ev("up", 12, 50, 50), ctx({ hitObject: bell }))).toEqual([{ kind: "placeAlert", y: 50 }]);
    const t = new GestureController();
    const tctx = ctx({ liftChip: true });
    t.handle(ev("down", 100, 40, 0, { pointerType: "touch" }), tctx);
    t.tick(500, tctx);
    const up = t.handle(ev("up", 500, 40, 900, { pointerType: "touch" }), tctx);
    // Clamped to the plot's right edge (the price column fits its cents labels).
    expect(up.at(-1)).toEqual({ kind: "liftChip", x: 363, y: 40 });
  });
});

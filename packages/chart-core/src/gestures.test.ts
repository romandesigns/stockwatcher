import { describe, expect, test } from "bun:test";
import {
  applyGesture,
  DOUBLE_TAP_MS,
  GestureController,
  LONG_PRESS_MS,
  PINCH_WHEEL_ZOOM_PER_PX,
  PRICE_DRAG_ZOOM_PER_PX,
  WHEEL_ZOOM_PER_PX,
  type GestureAction,
  type GestureContext,
  type GestureInput,
  type PointerKind,
} from "./gestures.ts";
import { computeChartModel, type ChartState } from "./layout.ts";
import { defaultViewport, isAutoPrice, slotX, type Viewport } from "./scale.ts";
import { fixtureSnapshot } from "./testFixture.ts";

// Geometry at 400×300 with default options: plot x 0–344, y 0–208; volume
// y 214–278; time axis y 278–300; price axis x 344–400, y 0–208. 20 slots at
// 16 px, following: slot i is centred at x = 344 − (21.5 − (i + 0.5)) × 16.
const snap = fixtureSnapshot("1m", 20, [5]);
const state: ChartState = { timeframe: "1m", candles: snap.candles, gaps: snap.gaps, profile: snap.profile, inspectT: null, pointer: null };
const vp = defaultViewport(16);
const model = computeChartModel(state, vp, { width: 400, height: 300 });
const ctx = (inspectT: number | null = null): GestureContext => ({ model, inspectT });
const xOf = (i: number) => slotX(model.index, i);
const tOf = (i: number) => model.slots[i]!.t;

const PLOT_Y = 100;
const AXIS_X = 372;
const TIME_Y = 290;

function ev(type: GestureInput["type"], x: number, y: number, t: number, extra: Partial<GestureInput> = {}): GestureInput {
  return { type, pointerId: 1, pointerType: "touch", x, y, t, ...extra };
}

/** A driver: feeds inputs at explicit times and collects every action. */
function driver(pointerType: PointerKind = "touch") {
  const g = new GestureController();
  let inspectT: number | null = null;
  const log: GestureAction[] = [];
  const run = (acts: GestureAction[]) => {
    for (const a of acts) if (a.kind === "inspect") inspectT = a.t;
    log.push(...acts);
    return acts;
  };
  return {
    g,
    log,
    get inspectT() {
      return inspectT;
    },
    send: (type: GestureInput["type"], x: number, y: number, t: number, extra: Partial<GestureInput> = {}) => run(g.handle(ev(type, x, y, t, { pointerType, ...extra }), ctx(inspectT))),
    tick: (t: number) => run(g.tick(t, ctx(inspectT))),
  };
}

const kinds = (acts: GestureAction[]) => acts.map((a) => a.kind);

describe("touch: tap, double tap, long press", () => {
  test("a tap pins the slot only after the double-tap window, and a second tap on it unpins", () => {
    const d = driver();
    d.send("down", xOf(10), PLOT_Y, 0);
    expect(d.send("up", xOf(10), PLOT_Y, 80)).toEqual([]);
    expect(d.g.nextDeadline()).toBe(80 + DOUBLE_TAP_MS);
    expect(d.tick(80 + DOUBLE_TAP_MS - 1)).toEqual([]);
    expect(d.tick(80 + DOUBLE_TAP_MS)).toEqual([{ kind: "inspect", t: tOf(10), pinned: true, announce: true }]);
    expect(d.g.nextDeadline()).toBeNull();
    d.send("down", xOf(10), PLOT_Y, 2000);
    d.send("up", xOf(10), PLOT_Y, 2050);
    expect(d.tick(2050 + DOUBLE_TAP_MS)).toEqual([{ kind: "inspect", t: null, pinned: false, announce: true }]);
  });

  test("a press elsewhere flushes the waiting tap at once", () => {
    const d = driver();
    d.send("down", xOf(10), PLOT_Y, 0);
    d.send("up", xOf(10), PLOT_Y, 50);
    expect(d.send("down", xOf(2), PLOT_Y, 120)).toEqual([{ kind: "inspect", t: tOf(10), pinned: true, announce: true }]);
  });

  test("double tap in the plot auto-scales price and never pins", () => {
    const d = driver();
    d.send("down", xOf(10), PLOT_Y, 0);
    d.send("up", xOf(10), PLOT_Y, 60);
    d.send("down", xOf(10) + 10, PLOT_Y + 5, 200);
    expect(d.send("up", xOf(10) + 10, PLOT_Y + 5, 260)).toEqual([{ kind: "autoScalePrice" }]);
    expect(d.tick(5000)).toEqual([]);
    expect(d.log.some((a) => a.kind === "inspect")).toBe(false);
  });

  test("double tap on the price axis auto-scales; on the time axis it follows latest", () => {
    const d = driver();
    d.send("down", AXIS_X, PLOT_Y, 0);
    d.send("up", AXIS_X, PLOT_Y, 50);
    d.send("down", AXIS_X, PLOT_Y, 150);
    expect(d.send("up", AXIS_X, PLOT_Y, 200)).toEqual([{ kind: "autoScalePrice" }]);
    d.send("down", xOf(8), TIME_Y, 1000);
    d.send("up", xOf(8), TIME_Y, 1050);
    d.send("down", xOf(8), TIME_Y, 1150);
    expect(d.send("up", xOf(8), TIME_Y, 1200)).toEqual([{ kind: "followLatest" }]);
  });

  test("taps too far apart or too slow are two single taps", () => {
    const d = driver();
    d.send("down", xOf(10), PLOT_Y, 0);
    d.send("up", xOf(10), PLOT_Y, 50);
    // 50 px away: flushes the first tap, starts a new one.
    expect(kinds(d.send("down", xOf(10) + 50, PLOT_Y, 150))).toEqual(["inspect"]);
    d.send("up", xOf(10) + 50, PLOT_Y, 200);
    expect(kinds(d.tick(200 + DOUBLE_TAP_MS))).toEqual(["inspect"]);
  });

  test("long press scrubs: the inspector follows the finger, lifting clears", () => {
    const d = driver();
    d.send("down", xOf(10), PLOT_Y, 0);
    // Jitter inside the slop does not cancel the press.
    d.send("move", xOf(10) + 3, PLOT_Y + 2, 200);
    expect(d.g.nextDeadline()).toBe(LONG_PRESS_MS);
    expect(d.tick(LONG_PRESS_MS - 1)).toEqual([]);
    const start = d.tick(LONG_PRESS_MS);
    expect(d.g.state).toBe("scrub");
    expect(start).toContainEqual({ kind: "inspect", t: tOf(10), pinned: false, announce: false });
    expect(d.g.capturing).toBe(true);
    const seen = new Set<number | null>();
    for (let i = 10; i >= 3; i--) for (const a of d.send("move", xOf(i), PLOT_Y + (10 - i) * 4, 500 + (10 - i) * 16)) if (a.kind === "inspect") seen.add(a.t);
    expect([...seen]).toEqual([9, 8, 7, 6, 5, 4, 3].map(tOf));
    // Past the plot's left edge the scrub clamps instead of dropping out.
    expect(d.send("move", -40, PLOT_Y, 700).find((a) => a.kind === "crosshair")).toEqual({ kind: "crosshair", point: { x: 0, y: PLOT_Y } });
    expect(d.send("up", 0, PLOT_Y, 800)).toEqual([
      { kind: "crosshair", point: null },
      { kind: "inspect", t: null, pinned: false, announce: false },
    ]);
    expect(d.g.state).toBe("idle");
  });

  test("long press on the price axis opens the axis menu at the finger", () => {
    const d = driver();
    d.send("down", AXIS_X, 60, 0);
    expect(d.g.capturing).toBe(true);
    expect(d.tick(LONG_PRESS_MS)).toEqual([{ kind: "openAxisMenu", x: AXIS_X, y: 60 }]);
    expect(d.send("up", AXIS_X, 60, 900)).toEqual([]);
    expect(d.tick(2000)).toEqual([]);
  });

  test("a long press is never the second half of a double tap", () => {
    const d = driver();
    d.send("down", xOf(10), PLOT_Y, 0);
    d.send("up", xOf(10), PLOT_Y, 50);
    d.send("down", xOf(10), PLOT_Y, 150);
    expect(d.tick(150 + LONG_PRESS_MS).some((a) => a.kind === "autoScalePrice")).toBe(false);
    expect(d.g.state).toBe("scrub");
    d.send("up", xOf(10), PLOT_Y, 900);
    expect(d.log.some((a) => a.kind === "autoScalePrice" || (a.kind === "inspect" && a.pinned))).toBe(false);
  });
});

describe("touch: swipe, scroll release, pinch, price-axis drag", () => {
  test("a horizontal swipe pans by slots and never inspects", () => {
    const d = driver();
    d.send("down", 200, PLOT_Y, 0);
    expect(d.g.capturing).toBe(false);
    expect(d.send("move", 205, PLOT_Y + 1, 16)).toEqual([]);
    expect(d.send("move", 212, PLOT_Y + 3, 32)).toEqual([{ kind: "pan", dxSlots: 12 / 16, dy: 0 }]);
    expect(d.g.capturing).toBe(true);
    expect(d.send("move", 244, PLOT_Y + 20, 48)).toEqual([{ kind: "pan", dxSlots: 2, dy: 0 }]);
    d.send("up", 244, PLOT_Y + 20, 64);
    expect(d.tick(10_000)).toEqual([]);
    expect(d.log.some((a) => a.kind === "inspect")).toBe(false);
  });

  test("a mostly vertical swipe is released to the page and ignored until lift", () => {
    const d = driver();
    d.send("down", 200, PLOT_Y, 0);
    expect(d.send("move", 203, PLOT_Y + 12, 16)).toEqual([{ kind: "release" }]);
    expect(d.g.capturing).toBe(false);
    expect(d.send("move", 260, PLOT_Y + 14, 32)).toEqual([]);
    expect(d.tick(LONG_PRESS_MS + 100)).toEqual([]);
    expect(d.send("up", 260, PLOT_Y + 14, 600)).toEqual([]);
    expect(d.g.state).toBe("idle");
  });

  test("the browser cancelling a press (it scrolled) leaves no state behind", () => {
    const d = driver();
    d.send("down", 200, PLOT_Y, 0);
    expect(d.send("cancel", 200, PLOT_Y, 30)).toEqual([]);
    expect(d.g.state).toBe("idle");
    expect(d.g.nextDeadline()).toBeNull();
  });

  test("pinch zooms time around the midpoint by the distance ratio", () => {
    const d = driver();
    d.send("down", 100, PLOT_Y, 0, { pointerId: 1 });
    d.send("down", 200, PLOT_Y, 10, { pointerId: 2 });
    expect(d.g.state).toBe("pinch");
    expect(d.g.capturing).toBe(true);
    // Fingers move apart symmetrically: 100 px → 150 px, midpoint stays at 150.
    d.send("move", 75, PLOT_Y, 30, { pointerId: 1 });
    const acts = d.send("move", 225, PLOT_Y, 31, { pointerId: 2 });
    const z = acts.filter((a) => a.kind === "zoomTime");
    expect(z).toHaveLength(1);
    expect(z[0]).toEqual({ kind: "zoomTime", factor: 150 / 125, anchorX: 150 });
    const total = d.log.filter((a) => a.kind === "zoomTime").reduce((f, a) => f * (a.kind === "zoomTime" ? a.factor : 1), 1);
    expect(total).toBeCloseTo(1.5);
    // Never price: no scalePrice, no inspection.
    expect(d.log.some((a) => a.kind === "scalePrice" || a.kind === "inspect")).toBe(false);
    // One finger lifts: the other is ignored until it lifts too.
    d.send("up", 75, PLOT_Y, 50, { pointerId: 1 });
    expect(d.send("move", 300, PLOT_Y, 60, { pointerId: 2 })).toEqual([]);
    d.send("up", 300, PLOT_Y, 70, { pointerId: 2 });
    expect(d.g.state).toBe("idle");
  });

  test("dragging the price axis up zooms price in around the plot centre; down zooms out", () => {
    const d = driver();
    d.send("down", AXIS_X, 120, 0);
    expect(d.send("move", AXIS_X, 115, 16)).toEqual([]);
    expect(d.send("move", AXIS_X, 100, 32)).toEqual([{ kind: "scalePrice", factor: Math.exp(20 * PRICE_DRAG_ZOOM_PER_PX), anchorY: 104 }]);
    const down = d.send("move", AXIS_X, 140, 48)[0];
    expect(down?.kind === "scalePrice" && down.factor < 1).toBe(true);
    d.send("up", AXIS_X, 140, 60);
    expect(d.tick(10_000)).toEqual([]);
  });
});

describe("mouse", () => {
  test("hover moves the crosshair and inspects; leaving clears", () => {
    const d = driver("mouse");
    const acts = d.send("move", xOf(12), PLOT_Y, 0);
    expect(acts).toEqual([
      { kind: "crosshair", point: { x: xOf(12), y: PLOT_Y } },
      { kind: "inspect", t: tOf(12), pinned: false, announce: false },
    ]);
    // Same slot again: crosshair only.
    expect(kinds(d.send("move", xOf(12) + 2, PLOT_Y + 5, 16))).toEqual(["crosshair"]);
    expect(d.send("leave", 500, PLOT_Y, 30)).toEqual([
      { kind: "crosshair", point: null },
      { kind: "inspect", t: null, pinned: false, announce: false },
    ]);
  });

  test("drag pans time (and carries dy for a manual price range); a click does nothing extra", () => {
    const d = driver("mouse");
    d.send("down", 200, PLOT_Y, 0, { button: 0 });
    expect(d.send("move", 203, PLOT_Y, 10).some((a) => a.kind === "pan")).toBe(false);
    expect(d.send("move", 216, PLOT_Y + 8, 20)).toEqual([{ kind: "pan", dxSlots: 1, dy: 8 }]);
    expect(d.send("up", 216, PLOT_Y + 8, 30)).toEqual([]);
    d.send("down", 200, PLOT_Y, 100, { button: 0 });
    expect(d.send("up", 200, PLOT_Y, 120)).toEqual([]);
    expect(d.tick(1000)).toEqual([]);
  });

  test("dragging the price axis scales price", () => {
    const d = driver("mouse");
    d.send("down", AXIS_X, 100, 0, { button: 0 });
    expect(d.send("move", AXIS_X, 90, 10)).toEqual([{ kind: "scalePrice", factor: Math.exp(10 * PRICE_DRAG_ZOOM_PER_PX), anchorY: 104 }]);
  });

  test("right-click opens the axis menu on the price axis only; right button never drags", () => {
    const d = driver("mouse");
    expect(d.send("down", AXIS_X, 50, 0, { button: 2 })).toEqual([]);
    expect(d.send("contextmenu", AXIS_X, 50, 5)).toEqual([{ kind: "openAxisMenu", x: AXIS_X, y: 50 }]);
    expect(d.send("contextmenu", 100, 50, 10)).toEqual([]);
    expect(d.g.state).toBe("idle");
  });

  test("a touch long press's contextmenu is ignored", () => {
    const d = driver();
    expect(d.send("contextmenu", AXIS_X, 50, 0)).toEqual([]);
  });

  test("double-click: plot and price axis auto-scale, the time axis follows latest", () => {
    const d = driver("mouse");
    expect(d.send("dblclick", 100, PLOT_Y, 0)).toEqual([{ kind: "autoScalePrice" }]);
    expect(d.send("dblclick", AXIS_X, PLOT_Y, 0)).toEqual([{ kind: "autoScalePrice" }]);
    expect(d.send("dblclick", 100, TIME_Y, 0)).toEqual([{ kind: "followLatest" }]);
  });

  test("a dblclick synthesized after a touch double tap does not act twice", () => {
    const g = new GestureController();
    g.handle(ev("down", 100, PLOT_Y, 0), ctx());
    g.handle(ev("up", 100, PLOT_Y, 40), ctx());
    expect(g.handle(ev("dblclick", 100, PLOT_Y, 300, { pointerType: "mouse" }), ctx())).toEqual([]);
  });

  test("wheel: zoom around the pointer, shift/horizontal pans, price axis scales, ctrl (trackpad pinch) zooms time", () => {
    const d = driver("mouse");
    expect(d.send("wheel", 120, PLOT_Y, 0, { deltaY: 100 })).toEqual([{ kind: "zoomTime", factor: Math.exp(-100 * WHEEL_ZOOM_PER_PX), anchorX: 120 }]);
    expect(d.send("wheel", 120, PLOT_Y, 0, { deltaY: 32, shiftKey: true })).toEqual([{ kind: "pan", dxSlots: -2, dy: 0 }]);
    expect(d.send("wheel", 120, PLOT_Y, 0, { deltaX: -48, deltaY: 4 })).toEqual([{ kind: "pan", dxSlots: 3, dy: 0 }]);
    expect(d.send("wheel", AXIS_X, 60, 0, { deltaY: -100 })).toEqual([{ kind: "scalePrice", factor: Math.exp(100 * WHEEL_ZOOM_PER_PX), anchorY: 60 }]);
    expect(d.send("wheel", 120, PLOT_Y, 0, { deltaY: -6, ctrlKey: true })).toEqual([{ kind: "zoomTime", factor: Math.exp(6 * PINCH_WHEEL_ZOOM_PER_PX), anchorX: 120 }]);
    // Ctrl+wheel zooms time even over the price axis (it is a pinch, and pinch is time-only).
    expect(kinds(d.send("wheel", AXIS_X, 60, 0, { deltaY: -6, ctrlKey: true }))).toEqual(["zoomTime"]);
  });
});

describe("applyGesture", () => {
  test("scalePrice switches to a manual range; autoScalePrice returns to auto", () => {
    expect(isAutoPrice(vp)).toBe(true);
    const zoomed = applyGesture(vp, { kind: "scalePrice", factor: 2, anchorY: 104 }, model);
    expect(zoomed.price?.mode).toBe("manual");
    const p = model.price!;
    if (zoomed.price?.mode !== "manual") throw new Error("expected manual");
    expect(zoomed.price.hi - zoomed.price.lo).toBeCloseTo((p.hi - p.lo) / 2);
    expect((zoomed.price.hi + zoomed.price.lo) / 2).toBeCloseTo((p.hi + p.lo) / 2);
    expect(isAutoPrice(applyGesture(zoomed, { kind: "autoScalePrice" }, model))).toBe(true);
  });

  test("successive scalePrice actions compound from the viewport, not a stale model", () => {
    let v: Viewport = vp;
    for (let k = 0; k < 4; k++) v = applyGesture(v, { kind: "scalePrice", factor: Math.SQRT2, anchorY: 104 }, model);
    if (v.price?.mode !== "manual") throw new Error("expected manual");
    expect(v.price.hi - v.price.lo).toBeCloseTo((model.price!.hi - model.price!.lo) / 4);
  });

  test("a manual range holds while time pans; mouse dy moves it; auto ignores dy", () => {
    const manual = applyGesture(vp, { kind: "scalePrice", factor: 2, anchorY: 104 }, model);
    const panned = applyGesture(manual, { kind: "pan", dxSlots: 3, dy: 0 }, model);
    expect(panned.anchorT).not.toBeNull();
    expect(panned.price).toEqual(manual.price);
    const shifted = applyGesture(manual, { kind: "pan", dxSlots: 0, dy: 20.8 }, model);
    if (shifted.price?.mode !== "manual" || manual.price?.mode !== "manual") throw new Error("expected manual");
    // Dragging down shows higher prices: 20.8 px of a 208 px plot = 10 % of the span.
    expect(shifted.price.lo - manual.price.lo).toBeCloseTo((manual.price.hi - manual.price.lo) * 0.1);
    expect(applyGesture(vp, { kind: "pan", dxSlots: 0, dy: 50 }, model).price).toBeUndefined();
  });

  test("zoomTime and followLatest drive the time viewport", () => {
    const back = applyGesture(vp, { kind: "pan", dxSlots: 5, dy: 0 }, model);
    expect(applyGesture(back, { kind: "followLatest" }, model).anchorT).toBeNull();
    expect(applyGesture(vp, { kind: "zoomTime", factor: 2, anchorX: 300 }, model).barSpacing).toBe(32);
  });
});

describe("options: momentum, touch price pan, haptics", () => {
  function opt(options: ConstructorParameters<typeof GestureController>[0], extra: Partial<GestureContext> = {}) {
    const g = new GestureController(options);
    const log: GestureAction[] = [];
    const c = { ...ctx(), ...extra };
    return {
      g,
      log,
      send: (type: GestureInput["type"], x: number, y: number, t: number) => {
        const a = g.handle(ev(type, x, y, t), c);
        log.push(...a);
        return a;
      },
      tick: (t: number) => {
        const a = g.tick(t, c);
        log.push(...a);
        return a;
      },
    };
  }

  test("a fast touch pan keeps going after release and decays to a stop", () => {
    const d = opt({ momentum: true });
    d.send("down", 200, PLOT_Y, 0);
    d.send("move", 180, PLOT_Y, 10);
    d.send("move", 120, PLOT_Y, 30);
    d.send("up", 100, PLOT_Y, 40);
    expect(d.g.state).toBe("fling");
    expect(d.g.nextDeadline()).toBe(40);
    const first = d.tick(56).find((a) => a.kind === "pan") as Extract<GestureAction, { kind: "pan" }>;
    expect(first.dxSlots).toBeLessThan(0);
    let t = 56;
    while (d.g.state === "fling" && t < 5000) d.tick((t += 16));
    expect(d.g.state).toBe("idle");
  });

  test("a press during a fling stops it and is not a tap", () => {
    const d = opt({ momentum: true });
    d.send("down", 200, PLOT_Y, 0);
    d.send("move", 120, PLOT_Y, 30);
    d.send("up", 100, PLOT_Y, 40);
    d.send("down", 150, PLOT_Y, 60);
    d.send("up", 150, PLOT_Y, 80);
    d.tick(80 + DOUBLE_TAP_MS + 1);
    expect(kinds(d.log).includes("inspect")).toBe(false);
  });

  test("lifting after a pause does not fling", () => {
    const d = opt({ momentum: true });
    d.send("down", 200, PLOT_Y, 0);
    d.send("move", 120, PLOT_Y, 30);
    d.send("up", 120, PLOT_Y, 300);
    expect(d.g.state).toBe("idle");
  });

  test('touchPricePan "always": a vertical swipe pans price from auto; "manual" releases it', () => {
    const free = opt({ touchPricePan: "always" });
    free.send("down", 200, PLOT_Y, 0);
    const acts = free.send("move", 202, PLOT_Y + 30, 20);
    const pan = acts.find((a) => a.kind === "pan") as Extract<GestureAction, { kind: "pan" }>;
    expect(pan.pricePan).toBe(true);
    expect(pan.dy).toBe(30);
    const after = applyGesture(vp, pan, model);
    expect(isAutoPrice(after)).toBe(false);

    const web = opt({});
    web.send("down", 200, PLOT_Y, 0);
    expect(kinds(web.send("move", 202, PLOT_Y + 30, 20))).toEqual(["release"]);
  });

  test("a horizontal touch swipe stays time-only on an auto range, even with jitter", () => {
    const d = opt({ touchPricePan: "always" });
    d.send("down", 200, PLOT_Y, 0);
    const pan = d.send("move", 170, PLOT_Y + 4, 20).find((a) => a.kind === "pan") as Extract<GestureAction, { kind: "pan" }>;
    expect(pan.dy).toBe(0);
  });

  test("haptics: entering a scrub, and each new candle while scrubbing", () => {
    const d = opt({ haptics: true });
    d.send("down", xOf(10), PLOT_Y, 0);
    expect(d.tick(LONG_PRESS_MS)[0]).toEqual({ kind: "haptic", style: "light" });
    expect(kinds(d.send("move", xOf(12), PLOT_Y, LONG_PRESS_MS + 20))).toContain("haptic");
  });
});

describe("objects and drawing tools", () => {
  const order = { kind: "order" as const, id: "o-1", part: "drag" as const };
  const hitObject = (x: number, y: number) => (Math.abs(y - 50) <= 6 ? (x > 300 ? { ...order, part: "cancel" as const } : order) : null);

  function run(extra: Partial<GestureContext>, pointerType: PointerKind = "touch") {
    const g = new GestureController();
    const c = { ...ctx(), ...extra };
    return (type: GestureInput["type"], x: number, y: number, t: number) => g.handle(ev(type, x, y, t, { pointerType }), c);
  }

  test("dragging an order emits objectDrag, then done on release, and never pans", () => {
    const send = run({ hitObject });
    send("down", 100, 50, 0);
    const mid = send("move", 100, 80, 20);
    expect(mid).toEqual([{ kind: "objectDrag", target: order, from: { x: 100, y: 50 }, to: { x: 100, y: 80 }, done: false }]);
    expect(send("up", 100, 90, 40)).toEqual([{ kind: "objectDrag", target: order, from: { x: 100, y: 50 }, to: { x: 100, y: 90 }, done: true }]);
  });

  test("tapping the cancel cell cancels; tapping empty chart clears the selection", () => {
    const send = run({ hitObject });
    send("down", 320, 50, 0);
    expect(send("up", 320, 50, 30)).toEqual([{ kind: "cancelOrder", id: "o-1" }]);
    send("down", 100, 150, 500);
    expect(send("up", 100, 150, 530)).toEqual([{ kind: "select", target: null }]);
  });

  test("a two-point tool: press-drag-release places it", () => {
    const send = run({ tool: "trend" }, "mouse");
    send("down", 100, 50, 0);
    expect(kinds(send("move", 160, 90, 20))).toEqual(["draft"]);
    expect(send("up", 170, 95, 40)).toEqual([
      { kind: "draft", draft: null },
      { kind: "createDrawing", tool: "trend", a: { x: 100, y: 50 }, b: { x: 170, y: 95 } },
    ]);
  });

  test("a two-point tool: tap, then tap again", () => {
    const send = run({ tool: "ray" });
    send("down", 100, 50, 0);
    expect(send("up", 100, 50, 30)).toEqual([{ kind: "draft", draft: { kind: "ray", a: { x: 100, y: 50 }, b: { x: 100, y: 50 } } }]);
    send("down", 200, 80, 500);
    expect(kinds(send("up", 200, 80, 530))).toEqual(["draft", "createDrawing"]);
  });

  test("a one-point tool places on a single tap", () => {
    const send = run({ tool: "hline" });
    send("down", 100, 70, 0);
    expect(send("up", 100, 70, 30)).toEqual([{ kind: "createDrawing", tool: "hline", a: { x: 100, y: 70 }, b: { x: 100, y: 70 } }]);
  });
});

test("right-click: price axis menu on the axis; trade menu in the plot only when the chart trades", () => {
  const g = new GestureController();
  const at = (x: number, y: number, extra: Partial<GestureContext> = {}) => g.handle({ type: "contextmenu", pointerId: 0, pointerType: "mouse", x, y, t: 0 }, { ...ctx(), ...extra });
  expect(at(AXIS_X, PLOT_Y)).toEqual([{ kind: "openAxisMenu", x: AXIS_X, y: PLOT_Y }]);
  expect(at(100, PLOT_Y)).toEqual([]);
  expect(at(100, PLOT_Y, { tradeMenu: true })).toEqual([{ kind: "openTradeMenu", x: 100, y: PLOT_Y }]);
  expect(at(100, TIME_Y, { tradeMenu: true })).toEqual([]);
});

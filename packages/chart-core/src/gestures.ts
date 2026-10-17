import { isOnePoint, type DrawingDraft, type DrawingKind, type DrawingPart } from "./drawings.ts";
import { hitTest, type ChartModel, type HitRegion, type Point } from "./layout.ts";
import { autoScalePrice, followLatest, isAutoPrice, manualPrice, panSlots, scalePriceRange, shiftPriceRange, slotAt, zoom, type PriceScale, type Viewport } from "./scale.ts";

// A pure gesture recognizer for the chart, shared by every renderer. It takes
// normalized pointer input (the web maps Pointer Events onto it; React Native
// will map its gesture responder or Gesture Handler events) and returns
// actions; it never touches a DOM, a timer or the viewport itself. Time comes
// only from the input's `t` and from explicit tick(t) calls, so a long press
// or a double tap is decided the same way in a unit test as on a phone.
//
// Behaviour, matched to Robinhood (docs/research/robinhood-chart-gestures.md):
//
// Touch and pen
// - Tap: pins the slot under the finger (tapping the pinned one again returns
//   to the latest). Deferred by DOUBLE_TAP_MS so a double tap never pins first.
// - Double tap (two taps within DOUBLE_TAP_MS and DOUBLE_TAP_SLOP_PX) in the
//   plot, volume pane or price axis: auto-scale the price axis (Robinhood's
//   "double-tap returns to an auto-scaled y-axis"). On the time axis: follow
//   the latest candle, like double-clicking it.
// - Horizontal swipe (the first move past TOUCH_SLOP_PX is mostly horizontal):
//   pans time. No momentum: inertia would need a per-frame decay the
//   renderers would have to drive, and Robinhood's own pan barely coasts.
// - Vertical swipe (the first move past the slop is mostly vertical): the
//   gesture is released to the platform so the PAGE scrolls; the chart
//   ignores that pointer until it lifts. On the web, touch-action: pan-y
//   lets the browser do this itself and it then sends pointercancel.
// - Long press (LONG_PRESS_MS without leaving the slop) in the plot, volume or
//   time axis: scrub. The inspector follows the finger, clamped to the plot,
//   until it lifts; lifting clears the inspection back to the latest candle,
//   like Robinhood's scrub. Haptics on entering scrub are the renderer's job
//   (the web has none worth using).
// - Long press on the price axis: open the price axis menu at the finger.
// - Drag on the price axis: scale price around the plot's vertical centre.
//   Up zooms in (a smaller range, taller candles), down zooms out, like
//   dragging Robinhood's y-axis. The range becomes manual until auto-scaled.
// - Pinch (two pointers): zooms TIME only, around the fingers' midpoint, and
//   the midpoint's horizontal travel pans, so the chart stays under the
//   fingers. Price is never pinched, as in Robinhood. When one finger lifts,
//   the other is ignored until it lifts too, so the chart does not jump.
//
// Mouse
// - Hover: crosshair and inspection follow the pointer; leaving clears both.
// - Drag in the plot, volume or time axis (past MOUSE_SLOP_PX): pans time, and
//   also price while the price range is manual.
// - Drag on the price axis: scales price, as on touch.
// - Wheel: zooms time around the pointer; Shift+wheel or a mostly horizontal
//   wheel pans; a wheel over the price axis scales price around the pointer.
// - Ctrl+wheel (what a trackpad pinch arrives as in browsers): zooms time.
// - Double-click in the plot, volume or price axis: auto-scales price; on the
//   time axis: follows the latest candle.
// - Right-click on the price axis: opens the price axis menu (the renderer
//   suppresses the platform's own menu there and only there).
//
// Objects (2026-10-17: drawings and simulated orders, both chart-owned)
// - The renderer passes ctx.hitObject, which names the drawing part, order
//   pill, cancel cell or place button under a point. A press on one never
//   pans: past the slop it drags the object (objectDrag, done on release; the
//   renderer commits then, so an order is modified once, not per frame); a
//   tap selects a drawing, cancels via the cancel cell, or opens the order
//   ticket via the place button. A tap on empty chart clears the selection.
// - With a drawing tool active (ctx.tool), a press in the plot draws instead:
//   press-drag-release places a two-point drawing; a tap places the first
//   point and a second tap the other (the draft follows the mouse between);
//   a one-point drawing (level, marker) goes where the tap or drag ends.
//
// Options (constructor; all off by default, so the plain recognizer above is
// what the unit tests pin):
// - momentum: a touch pan released at speed keeps going and decays (iOS
//   scroll feel, time constant MOMENTUM_TAU_MS). tick() drives it; the
//   renderer pumps tick() every frame while nextDeadline() is not null, as it
//   already does for long presses. A press during a fling stops it and is not
//   a tap.
// - touchPricePan: "manual" (web): a vertical swipe pans price only when the
//   range is already manual, otherwise it scrolls the page. "always" (native,
//   where the chart screen does not scroll): a mostly vertical swipe pans
//   price and makes the range manual; a mostly horizontal one stays time-only
//   unless the range is manual, so jitter never knocks price out of auto.
// - haptics: emits haptic actions (entering a scrub, each slot while
//   scrubbing, opening a menu by long press, grabbing an object, placing a
//   drawing). Renderers with a haptic engine play them; others ignore them.

export type PointerKind = "mouse" | "touch" | "pen";

export interface GestureInput {
  type: "down" | "move" | "up" | "cancel" | "leave" | "wheel" | "dblclick" | "contextmenu";
  pointerId: number;
  pointerType: PointerKind;
  /** CSS px from the chart surface's top-left. */
  x: number;
  y: number;
  /** Milliseconds on any monotonic clock, the same one tick() uses. */
  t: number;
  /** 0 = primary (left) button, 2 = secondary. Mouse down only. */
  button?: number;
  /** Wheel deltas in px (the renderer normalizes line and page modes). */
  deltaX?: number;
  deltaY?: number;
  ctrlKey?: boolean;
  shiftKey?: boolean;
}

/** What the controller needs to know about the chart at the moment of an input. */
export interface GestureContext {
  model: ChartModel;
  /** The inspected slot's t (null = latest), for tap-to-toggle. */
  inspectT: number | null;
  /** Whether the price range is manual (touch vertical pans follow it; see touchPricePan). */
  priceManual?: boolean;
  /** Active drawing tool, or null/absent for none. */
  tool?: DrawingKind | null;
  /** The object under a point (touch: true gives fingers a wider tolerance), or null. */
  hitObject?: (x: number, y: number, touch: boolean) => ObjectHit | null;
}

export type GestureAction =
  /**
   * Pan time by slots (positive = look back) and, while the price range is
   * manual, price by dy px. pricePan: pan price by dy even from an auto range,
   * which makes it manual (a free touch pan, see touchPricePan).
   */
  | { kind: "pan"; dxSlots: number; dy: number; pricePan?: boolean }
  /** Zoom time by factor (> 1 = wider candles) keeping the slot at anchorX (px from plot left) fixed. */
  | { kind: "zoomTime"; factor: number; anchorX: number }
  /** Scale price by factor (> 1 = zoom in) around the price at anchorY; makes the range manual. */
  | { kind: "scalePrice"; factor: number; anchorY: number }
  | { kind: "autoScalePrice" }
  | { kind: "followLatest" }
  /**
   * Inspect the slot at t (null = latest). pinned: a deliberate selection
   * (tap) rather than a transient one (hover, scrub). announce: worth a polite
   * screen reader announcement (taps yes; hover and scrub would chatter).
   */
  | { kind: "inspect"; t: number | null; pinned: boolean; announce: boolean }
  /** Horizontal crosshair position, or null to hide it. */
  | { kind: "crosshair"; point: Point | null }
  /** Open the price axis menu at x, y (chart surface px). */
  | { kind: "openAxisMenu"; x: number; y: number }
  /** A vertical swipe: the platform should scroll the page; the chart has let go of this pointer. */
  | { kind: "release" }
  /** A drawing being placed (px), or null when placement ended or was abandoned. */
  | { kind: "draft"; draft: DrawingDraft | null }
  /** Place a drawing of `tool` from a to b (px; one-point tools use a). */
  | { kind: "createDrawing"; tool: DrawingKind; a: Point; b: Point }
  /** Select an object (a drawing; an order pill), or clear the selection. */
  | { kind: "select"; target: ObjectHit | null }
  /** Drag of an object from `from` to `to` (px). done: the pointer lifted; commit now. */
  | { kind: "objectDrag"; target: ObjectHit; from: Point; to: Point; done: boolean }
  | { kind: "cancelOrder"; id: string }
  /** The place button was tapped at y: open the order ticket at that price. */
  | { kind: "placeOrder"; x: number; y: number }
  | { kind: "haptic"; style: HapticStyle };

export type HapticStyle = "selection" | "light" | "medium";

/** What ctx.hitObject reports under a point. */
export type ObjectHit = { kind: "drawing"; id: string; part: DrawingPart } | { kind: "order"; id: string; part: "drag" | "cancel" } | { kind: "place"; y: number };

export interface GestureOptions {
  momentum?: boolean;
  touchPricePan?: "manual" | "always";
  haptics?: boolean;
}

export const LONG_PRESS_MS = 450;
export const TOUCH_SLOP_PX = 8;
export const MOUSE_SLOP_PX = 4;
export const DOUBLE_TAP_MS = 300;
export const DOUBLE_TAP_SLOP_PX = 24;
/** Price-axis drag: zoom factor = exp(-dy × this); 140 px up ≈ 2×. */
export const PRICE_DRAG_ZOOM_PER_PX = 0.005;
/** Wheel zoom: factor = exp(-deltaY × this); one 100 px notch ≈ 0.86×. */
export const WHEEL_ZOOM_PER_PX = 0.0015;
/** Ctrl+wheel (trackpad pinch) sends small deltas, so it gets a larger gain. */
export const PINCH_WHEEL_ZOOM_PER_PX = 0.01;
/** Pinch distances are floored here so two nearly touching fingers cannot explode the factor. */
export const MIN_PINCH_DISTANCE_PX = 16;
/** Momentum: velocity decays as exp(−dt / TAU), close to iOS scroll deceleration. */
export const MOMENTUM_TAU_MS = 325;
/** A release slower than this (px/ms) does not fling; below MOMENTUM_STOP a fling ends. */
export const MOMENTUM_MIN_PX_MS = 0.25;
export const MOMENTUM_STOP_PX_MS = 0.02;
/** Velocity is measured over the last this-many ms of the pan, so a pause before lifting means no fling. */
export const VELOCITY_WINDOW_MS = 80;

type Region = Exclude<HitRegion, "none">;

type Phase =
  | { k: "idle" }
  /**
   * Pressed, not yet a drag, scrub or menu. second: may be the second tap of
   * a double tap. obj: pressed on an object. draw: pressed with a drawing
   * tool in the plot. quiet: this press stopped a fling, so it is not a tap.
   */
  | { k: "pending"; id: number; type: PointerKind; region: Region; x0: number; y0: number; t0: number; second: boolean; obj: ObjectHit | null; draw: boolean; quiet: boolean }
  /** free: vertical movement pans price too (see touchPricePan). */
  | { k: "pan"; id: number; type: PointerKind; lastX: number; lastY: number; free: boolean; samples: { x: number; y: number; t: number }[] }
  | { k: "drag"; id: number; target: ObjectHit; from: Point; to: Point }
  | { k: "draw"; id: number; tool: DrawingKind; a: Point; b: Point }
  | { k: "fling"; vx: number; vy: number; free: boolean; lastT: number }
  | { k: "scrub"; id: number }
  | { k: "axisScale"; id: number; lastY: number }
  | { k: "pinch"; a: number; b: number; lastDist: number; lastMidX: number }
  /** Waiting for every pointer to lift (after a release, a pinch, a menu or an ignored button). */
  | { k: "ignore" };

export type GesturePhase = Phase["k"];

interface Tap {
  x: number;
  y: number;
  t: number;
  region: Region;
}

const regionAt = (model: ChartModel, x: number, y: number): HitRegion => hitTest(model, x, y).region;
const touchLike = (type: PointerKind) => type !== "mouse";

export class GestureController {
  private phase: Phase = { k: "idle" };
  private readonly pointers = new Map<number, Point>();
  /** A finished tap waiting out the double-tap window before it pins. */
  private pendingTap: Tap | null = null;
  private lastType: PointerKind = "mouse";
  /** The first point of a two-point drawing placed by a tap, waiting for the second. */
  private placing: { tool: DrawingKind; a: Point } | null = null;
  private readonly opts: Required<GestureOptions>;

  constructor(options: GestureOptions = {}) {
    this.opts = { momentum: false, touchPricePan: "manual", haptics: false, ...options };
  }

  private haptic(style: HapticStyle): GestureAction[] {
    return this.opts.haptics ? [{ kind: "haptic", style }] : [];
  }

  /** Abandons a tap-placed first point (Escape, or the tool was switched). */
  cancelPlacing(): GestureAction[] {
    if (!this.placing) return [];
    this.placing = null;
    return [{ kind: "draft", draft: null }];
  }

  /** True while a tap-placed drawing waits for its second point. */
  get isPlacing(): boolean {
    return this.placing !== null;
  }

  /** The current recognizer state, for renderers and tests. */
  get state(): GesturePhase {
    return this.phase.k;
  }

  /** The pointer type of the last press (renderers use it to classify contextmenu / dblclick). */
  get lastPointerType(): PointerKind {
    return this.lastType;
  }

  /**
   * True while the chart owns the current touch: the renderer must stop the
   * platform from scrolling (the web calls preventDefault on touchmove). An
   * undecided press in the plot is NOT captured, so a vertical swipe still
   * scrolls the page; a press on the price axis is, because a vertical drag
   * there scales price.
   */
  get capturing(): boolean {
    const p = this.phase;
    if (p.k === "pan" || p.k === "scrub" || p.k === "axisScale" || p.k === "pinch" || p.k === "drag" || p.k === "draw") return true;
    return p.k === "pending" && (p.region === "priceAxis" || p.obj !== null || p.draw);
  }

  /** When tick() next has something to decide (a long press or a deferred tap), or null. */
  nextDeadline(): number | null {
    let d: number | null = null;
    const p = this.phase;
    // A fling wants every frame.
    if (p.k === "fling") return p.lastT;
    // Objects and drawing tools have no long press (no scrub on top of a drag).
    if (p.k === "pending" && touchLike(p.type) && !p.obj && !p.draw) d = p.t0 + LONG_PRESS_MS;
    // While a possible second tap is down, the first one waits for its outcome.
    if (this.pendingTap && !(p.k === "pending" && p.second)) {
      const tapDue = this.pendingTap.t + DOUBLE_TAP_MS;
      d = d === null ? tapDue : Math.min(d, tapDue);
    }
    return d;
  }

  /** Abandons any gesture (e.g. the chart lost its data or was hidden). */
  reset(): GestureAction[] {
    const wasScrub = this.phase.k === "scrub";
    const wasDraft = this.phase.k === "draw" || this.placing !== null;
    this.phase = { k: "idle" };
    this.pointers.clear();
    this.pendingTap = null;
    this.placing = null;
    return [...(wasScrub ? endScrub() : []), ...(wasDraft ? [{ kind: "draft" as const, draft: null }] : [])];
  }

  /** Advances time-driven decisions: long press → scrub or axis menu; an expired tap → pin. */
  tick(t: number, ctx: GestureContext): GestureAction[] {
    const out: GestureAction[] = [];
    const p = this.phase;
    if (p.k === "fling") {
      const dt = t - p.lastT;
      if (dt <= 0) return out;
      const decay = Math.exp(-dt / MOMENTUM_TAU_MS);
      // Distance covered over dt by a velocity decaying exponentially: v·τ·(1 − e^(−dt/τ)).
      const k = MOMENTUM_TAU_MS * (1 - decay);
      const dx = p.vx * k;
      const dy = p.free ? p.vy * k : 0;
      p.vx *= decay;
      p.vy *= decay;
      p.lastT = t;
      if (dx || dy) out.push({ kind: "pan", dxSlots: dx / ctx.model.index.barSpacing, dy, ...(p.free ? { pricePan: true } : {}) });
      if (Math.hypot(p.vx, p.free ? p.vy : 0) < MOMENTUM_STOP_PX_MS) this.phase = { k: "idle" };
      return out;
    }
    if (p.k === "pending" && touchLike(p.type) && !p.obj && !p.draw && t - p.t0 >= LONG_PRESS_MS) {
      // A long press is never the second tap of a double tap.
      if (p.second) this.pendingTap = null;
      const pt = this.pointers.get(p.id) ?? { x: p.x0, y: p.y0 };
      if (p.region === "priceAxis") {
        this.phase = { k: "ignore" };
        out.push(...this.haptic("medium"), { kind: "openAxisMenu", x: pt.x, y: pt.y });
      } else {
        this.phase = { k: "scrub", id: p.id };
        out.push(...this.haptic("light"), ...scrubAt(ctx, pt.x, pt.y));
      }
    }
    const q = this.phase;
    if (this.pendingTap && !(q.k === "pending" && q.second) && t - this.pendingTap.t >= DOUBLE_TAP_MS) {
      out.push(...this.flushTap(ctx));
    }
    return out;
  }

  handle(input: GestureInput, ctx: GestureContext): GestureAction[] {
    switch (input.type) {
      case "down":
        return this.down(input, ctx);
      case "move":
        return this.move(input, ctx);
      case "up":
        return this.up(input, ctx);
      case "cancel":
        return this.cancel(input);
      case "leave":
        // Only a mouse hovers; with a button held the pointer is captured.
        if (input.pointerType !== "mouse" || this.phase.k !== "idle") return [];
        if (this.placing) return [{ kind: "crosshair", point: null }];
        return [
          { kind: "crosshair", point: null },
          { kind: "inspect", t: null, pinned: false, announce: false },
        ];
      case "wheel":
        return wheel(input, ctx);
      case "dblclick":
        // Touch double taps are recognized from pointers; a compatibility
        // dblclick the browser synthesizes after one must not act twice.
        if (this.lastType !== "mouse") return [];
        return doubleTap(regionAt(ctx.model, input.x, input.y));
      case "contextmenu": {
        // A touch long press is recognized by tick(); its contextmenu is noise.
        if (input.pointerType !== "mouse") return [];
        return regionAt(ctx.model, input.x, input.y) === "priceAxis" ? [{ kind: "openAxisMenu", x: input.x, y: input.y }] : [];
      }
    }
  }

  private down(e: GestureInput, ctx: GestureContext): GestureAction[] {
    this.lastType = e.pointerType;
    // Only the primary mouse button drags; right-click arrives as contextmenu.
    if (e.pointerType === "mouse" && (e.button ?? 0) !== 0) return [];
    this.pointers.set(e.pointerId, { x: e.x, y: e.y });
    let quiet = false;
    if (this.phase.k === "fling") {
      // Catching a fling stops it, and the catching press is not a tap.
      this.phase = { k: "idle" };
      quiet = true;
    }
    const p = this.phase;

    if (p.k === "pinch" || p.k === "ignore") return [];
    if (p.k !== "idle") {
      // A second finger during an undecided press or a pan becomes a pinch.
      // During a scrub or a price drag it is ignored (one gesture at a time).
      if (touchLike(e.pointerType) && ((p.k === "pending" && !p.obj && !p.draw) || p.k === "pan")) {
        const a = p.id;
        const pa = this.pointers.get(a);
        if (!pa) return [];
        if (p.k === "pending" && p.second) this.pendingTap = null;
        this.phase = { k: "pinch", a, b: e.pointerId, lastDist: pinchDistance(pa, e), lastMidX: (pa.x + e.x) / 2 };
      }
      return [];
    }

    const region = regionAt(ctx.model, e.x, e.y);
    if (region === "none") {
      this.phase = { k: "ignore" };
      return [];
    }
    const out: GestureAction[] = [];
    let second = false;
    if (this.pendingTap) {
      const tap = this.pendingTap;
      if (touchLike(e.pointerType) && e.t - tap.t <= DOUBLE_TAP_MS && Math.hypot(e.x - tap.x, e.y - tap.y) <= DOUBLE_TAP_SLOP_PX) second = true;
      else out.push(...this.flushTap(ctx));
    }
    // A drawing tool owns presses in the plot; otherwise an object under the
    // press (never on the axes' own gestures, except the order pills that
    // sit inside the plot) takes it.
    const draw = !!ctx.tool && region === "plot";
    const obj = draw || region === "priceAxis" || region === "timeAxis" ? null : (ctx.hitObject?.(e.x, e.y, touchLike(e.pointerType)) ?? null);
    if (obj || draw) {
      second = false;
      this.pendingTap = null;
    }
    this.phase = { k: "pending", id: e.pointerId, type: e.pointerType, region, x0: e.x, y0: e.y, t0: e.t, second, obj, draw, quiet };
    return out;
  }

  private move(e: GestureInput, ctx: GestureContext): GestureAction[] {
    const p = this.phase;
    if (this.pointers.has(e.pointerId)) this.pointers.set(e.pointerId, { x: e.x, y: e.y });

    switch (p.k) {
      case "idle": {
        if (e.pointerType !== "mouse") return [];
        const out = hover(ctx, e.x, e.y);
        // The second point of a tap-placed drawing follows the mouse.
        if (this.placing) out.push({ kind: "draft", draft: { kind: this.placing.tool, a: this.placing.a, b: { x: e.x, y: e.y } } });
        return out;
      }
      case "pending": {
        if (e.pointerId !== p.id) return [];
        const dx = e.x - p.x0;
        const dy = e.y - p.y0;
        const slop = p.type === "mouse" ? MOUSE_SLOP_PX : TOUCH_SLOP_PX;
        if (Math.hypot(dx, dy) < slop) return p.type === "mouse" ? hover(ctx, e.x, e.y) : [];
        // Past the slop: whatever this becomes, it is not a tap.
        if (p.second) this.pendingTap = null;
        if (p.draw && ctx.tool) {
          const a = this.placing?.a ?? { x: p.x0, y: p.y0 };
          this.placing = null;
          this.phase = { k: "draw", id: p.id, tool: ctx.tool, a, b: { x: e.x, y: e.y } };
          return [{ kind: "draft", draft: draftOf(ctx.tool, a, { x: e.x, y: e.y }) }];
        }
        if (p.obj && p.obj.kind !== "place" && !(p.obj.kind === "order" && p.obj.part === "cancel")) {
          const from = { x: p.x0, y: p.y0 };
          const to = { x: e.x, y: e.y };
          this.phase = { k: "drag", id: p.id, target: p.obj, from, to };
          return [...this.haptic("light"), { kind: "objectDrag", target: p.obj, from, to, done: false }];
        }
        if (p.region === "priceAxis") {
          this.phase = { k: "axisScale", id: p.id, lastY: p.y0 };
          return this.move(e, ctx);
        }
        let free = p.type === "mouse" || !!ctx.priceManual;
        if (touchLike(p.type) && Math.abs(dy) > Math.abs(dx)) {
          if (this.opts.touchPricePan === "always" || ctx.priceManual) free = true;
          else {
            this.phase = { k: "ignore" };
            return [{ kind: "release" }];
          }
        }
        this.phase = { k: "pan", id: p.id, type: p.type, lastX: p.x0, lastY: p.y0, free, samples: [{ x: p.x0, y: p.y0, t: p.t0 }] };
        return this.move(e, ctx);
      }
      case "pan": {
        if (e.pointerId !== p.id) return [];
        const dx = e.x - p.lastX;
        // A touch pan is horizontal only unless it is free (see touchPricePan):
        // by default the vertical axis belongs to the page.
        const dy = p.free ? e.y - p.lastY : 0;
        p.lastX = e.x;
        p.lastY = e.y;
        p.samples.push({ x: e.x, y: e.y, t: e.t });
        while (p.samples.length > 2 && e.t - p.samples[0]!.t > VELOCITY_WINDOW_MS) p.samples.shift();
        if (dx === 0 && dy === 0) return [];
        // A mouse drag pans price only on a manual range (applyGesture checks);
        // a free touch pan may start one from auto.
        const pricePan = p.free && p.type !== "mouse";
        return [{ kind: "pan", dxSlots: dx / ctx.model.index.barSpacing, dy, ...(pricePan ? { pricePan: true } : {}) }];
      }
      case "drag": {
        if (e.pointerId !== p.id) return [];
        p.to = { x: e.x, y: e.y };
        return [{ kind: "objectDrag", target: p.target, from: p.from, to: p.to, done: false }];
      }
      case "draw": {
        if (e.pointerId !== p.id) return [];
        p.b = { x: e.x, y: e.y };
        return [{ kind: "draft", draft: draftOf(p.tool, p.a, p.b) }];
      }
      case "fling":
        return [];
      case "scrub": {
        if (e.pointerId !== p.id) return [];
        const acts = scrubAt(ctx, e.x, e.y);
        // A light tick each time the scrub crosses into another candle.
        return acts.some((a) => a.kind === "inspect") ? [...this.haptic("selection"), ...acts] : acts;
      }
      case "axisScale": {
        if (e.pointerId !== p.id) return [];
        const dy = e.y - p.lastY;
        p.lastY = e.y;
        if (dy === 0) return [];
        const plot = ctx.model.geometry.plot;
        return [{ kind: "scalePrice", factor: Math.exp(-dy * PRICE_DRAG_ZOOM_PER_PX), anchorY: plot.y + plot.h / 2 }];
      }
      case "pinch": {
        if (e.pointerId !== p.a && e.pointerId !== p.b) return [];
        const pa = this.pointers.get(p.a);
        const pb = this.pointers.get(p.b);
        if (!pa || !pb) return [];
        const dist = pinchDistance(pa, pb);
        const midX = (pa.x + pb.x) / 2;
        const out: GestureAction[] = [];
        const dmx = midX - p.lastMidX;
        if (dmx !== 0) out.push({ kind: "pan", dxSlots: dmx / ctx.model.index.barSpacing, dy: 0 });
        const factor = dist / p.lastDist;
        if (factor !== 1) out.push({ kind: "zoomTime", factor, anchorX: midX - ctx.model.geometry.plot.x });
        p.lastDist = dist;
        p.lastMidX = midX;
        return out;
      }
      case "ignore":
        return [];
    }
  }

  private up(e: GestureInput, ctx: GestureContext): GestureAction[] {
    this.pointers.delete(e.pointerId);
    const p = this.phase;
    switch (p.k) {
      case "pending": {
        if (e.pointerId !== p.id) return [];
        this.phase = { k: "idle" };
        if (p.quiet) return [];
        if (p.draw && ctx.tool) return this.tapDraw(ctx.tool, { x: p.x0, y: p.y0 });
        if (p.obj) return this.tapObject(p.obj);
        // A click or tap on empty chart clears any selection (when the chart
        // has objects at all); a mouse click inspects by hover already.
        const clear: GestureAction[] = ctx.hitObject ? [{ kind: "select", target: null }] : [];
        if (p.type === "mouse") return clear;
        if (p.second) {
          this.pendingTap = null;
          return doubleTap(p.region);
        }
        this.pendingTap = { x: p.x0, y: p.y0, t: e.t, region: p.region };
        return clear;
      }
      case "pan": {
        if (e.pointerId !== p.id) return [];
        this.phase = { k: "idle" };
        if (!this.opts.momentum || p.type === "mouse") return [];
        const first = p.samples[0]!;
        const dt = e.t - first.t;
        // Lifting after a pause (no sample inside the window but the first) is no fling.
        if (dt <= 0 || e.t - p.samples[p.samples.length - 1]!.t > VELOCITY_WINDOW_MS) return [];
        const vx = (e.x - first.x) / dt;
        const vy = p.free ? (e.y - first.y) / dt : 0;
        if (Math.hypot(vx, vy) < MOMENTUM_MIN_PX_MS) return [];
        this.phase = { k: "fling", vx, vy, free: p.free, lastT: e.t };
        return [];
      }
      case "axisScale":
        if (e.pointerId === p.id) this.phase = { k: "idle" };
        return [];
      case "drag":
        if (e.pointerId !== p.id) return [];
        this.phase = { k: "idle" };
        return [{ kind: "objectDrag", target: p.target, from: p.from, to: { x: e.x, y: e.y }, done: true }];
      case "draw": {
        if (e.pointerId !== p.id) return [];
        this.phase = { k: "idle" };
        const b = { x: e.x, y: e.y };
        // A one-point drawing goes where the drag ended.
        const a = isOnePoint(p.tool) ? b : p.a;
        return [{ kind: "draft", draft: null }, ...this.haptic("light"), { kind: "createDrawing", tool: p.tool, a, b }];
      }
      case "fling":
        return [];
      case "scrub":
        if (e.pointerId !== p.id) return [];
        this.phase = { k: "idle" };
        return endScrub();
      case "pinch":
        if (e.pointerId === p.a || e.pointerId === p.b) this.phase = this.pointers.size ? { k: "ignore" } : { k: "idle" };
        return [];
      case "ignore":
        if (this.pointers.size === 0) this.phase = { k: "idle" };
        return [];
      case "idle":
        return [];
    }
  }

  private cancel(e: GestureInput): GestureAction[] {
    this.pointers.delete(e.pointerId);
    const p = this.phase;
    if (p.k === "pending" && p.second) this.pendingTap = null;
    const wasScrub = p.k === "scrub" && p.id === e.pointerId;
    const wasDraw = p.k === "draw" && p.id === e.pointerId;
    // A cancelled object drag snaps back: done with the original point.
    const dragBack: GestureAction[] = p.k === "drag" && p.id === e.pointerId ? [{ kind: "objectDrag", target: p.target, from: p.from, to: p.from, done: true }] : [];
    if (p.k !== "idle" && p.k !== "fling") this.phase = this.pointers.size ? { k: "ignore" } : { k: "idle" };
    return [...(wasScrub ? endScrub() : []), ...(wasDraw ? [{ kind: "draft" as const, draft: null }] : []), ...dragBack];
  }

  /** A tap with a drawing tool: one-point tools place at once; two-point tools take two taps. */
  private tapDraw(tool: DrawingKind, at: Point): GestureAction[] {
    if (isOnePoint(tool)) return [...this.haptic("light"), { kind: "createDrawing", tool, a: at, b: at }];
    if (this.placing && this.placing.tool === tool) {
      const a = this.placing.a;
      this.placing = null;
      return [{ kind: "draft", draft: null }, ...this.haptic("light"), { kind: "createDrawing", tool, a, b: at }];
    }
    this.placing = { tool, a: at };
    return [...this.haptic("selection"), { kind: "draft", draft: { kind: tool, a: at, b: at } }];
  }

  private tapObject(obj: ObjectHit): GestureAction[] {
    if (obj.kind === "place") return [{ kind: "placeOrder", x: 0, y: obj.y }];
    if (obj.kind === "order" && obj.part === "cancel") return [...this.haptic("medium"), { kind: "cancelOrder", id: obj.id }];
    return [...this.haptic("selection"), { kind: "select", target: obj }];
  }

  private flushTap(ctx: GestureContext): GestureAction[] {
    const tap = this.pendingTap;
    this.pendingTap = null;
    if (!tap || tap.region === "priceAxis") return [];
    // Pin the tapped slot; tapping the pinned one again returns to the latest.
    const hit = hitTest(ctx.model, tap.x, tap.y);
    const next = hit.slot && hit.slot.t !== ctx.inspectT ? hit.slot.t : null;
    return [{ kind: "inspect", t: next, pinned: next !== null, announce: true }];
  }
}

function draftOf(kind: DrawingKind, a: Point, b: Point): DrawingDraft {
  return { kind, a: isOnePoint(kind) ? b : a, b };
}

function pinchDistance(a: Point, b: Point): number {
  return Math.max(MIN_PINCH_DISTANCE_PX, Math.hypot(a.x - b.x, a.y - b.y));
}

function doubleTap(region: HitRegion): GestureAction[] {
  if (region === "timeAxis") return [{ kind: "followLatest" }];
  if (region === "none") return [];
  return [{ kind: "autoScalePrice" }];
}

function hover(ctx: GestureContext, x: number, y: number): GestureAction[] {
  const out: GestureAction[] = [{ kind: "crosshair", point: { x, y } }];
  const hit = hitTest(ctx.model, x, y);
  if (hit.slot && hit.slot.t !== ctx.inspectT) out.push({ kind: "inspect", t: hit.slot.t, pinned: false, announce: false });
  return out;
}

/** Scrub: the slot under the finger, with x and y clamped into the plot so a finger past the edge keeps the end slot. */
function scrubAt(ctx: GestureContext, x: number, y: number): GestureAction[] {
  const plot = ctx.model.geometry.plot;
  const cx = Math.min(plot.x + plot.w, Math.max(plot.x, x));
  const cy = Math.min(plot.y + plot.h - 1, Math.max(plot.y + 1, y));
  const out: GestureAction[] = [{ kind: "crosshair", point: { x: cx, y: cy } }];
  const i = slotAt(ctx.model.index, cx, ctx.model.slots.length);
  const t = i === null ? null : ctx.model.slots[i]!.t;
  if (t !== null && t !== ctx.inspectT) out.push({ kind: "inspect", t, pinned: false, announce: false });
  return out;
}

function endScrub(): GestureAction[] {
  return [
    { kind: "crosshair", point: null },
    { kind: "inspect", t: null, pinned: false, announce: false },
  ];
}

function wheel(e: GestureInput, ctx: GestureContext): GestureAction[] {
  const dx = e.deltaX ?? 0;
  const dy = e.deltaY ?? 0;
  const anchorX = e.x - ctx.model.geometry.plot.x;
  if (e.ctrlKey) return dy ? [{ kind: "zoomTime", factor: Math.exp(-dy * PINCH_WHEEL_ZOOM_PER_PX), anchorX }] : [];
  if (regionAt(ctx.model, e.x, e.y) === "priceAxis") return dy ? [{ kind: "scalePrice", factor: Math.exp(-dy * WHEEL_ZOOM_PER_PX), anchorY: e.y }] : [];
  if (e.shiftKey || Math.abs(dx) > Math.abs(dy)) {
    const d = e.shiftKey ? dy : dx;
    return d ? [{ kind: "pan", dxSlots: -d / ctx.model.index.barSpacing, dy: 0 }] : [];
  }
  return dy ? [{ kind: "zoomTime", factor: Math.exp(-dy * WHEEL_ZOOM_PER_PX), anchorX }] : [];
}

/**
 * The price scale a viewport action should start from: the model's, with
 * the viewport's own manual range when it has one. Reading the range from
 * the viewport (not the last drawn model) keeps several actions applied
 * before the next frame from overwriting each other.
 */
function currentPriceScale(vp: Viewport, model: ChartModel): PriceScale | null {
  if (!model.price) return null;
  return vp.price?.mode === "manual" ? { ...model.price, lo: vp.price.lo, hi: vp.price.hi } : model.price;
}

/**
 * Applies a viewport action (pan, zoomTime, scalePrice, autoScalePrice,
 * followLatest) to a viewport. Other actions return it unchanged; the
 * renderer handles inspection, the crosshair and the menu itself.
 */
export function applyGesture(vp: Viewport, action: GestureAction, model: ChartModel): Viewport {
  switch (action.kind) {
    case "pan": {
      let next = panSlots(vp, model.slots, action.dxSlots);
      if (action.dy && (action.pricePan || !isAutoPrice(next))) {
        const s = currentPriceScale(next, model);
        if (s) next = manualPrice(next, shiftPriceRange(s, action.dy));
      }
      return next;
    }
    case "zoomTime":
      return zoom(vp, model.slots, action.factor, action.anchorX, model.geometry.plot.w);
    case "scalePrice": {
      const s = currentPriceScale(vp, model);
      return s ? manualPrice(vp, scalePriceRange(s, action.factor, action.anchorY)) : vp;
    }
    case "autoScalePrice":
      return autoScalePrice(vp);
    case "followLatest":
      return followLatest(vp);
    default:
      return vp;
  }
}

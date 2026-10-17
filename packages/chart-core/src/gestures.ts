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
}

export type GestureAction =
  /** Pan time by slots (positive = look back) and, while the price range is manual, price by dy px. */
  | { kind: "pan"; dxSlots: number; dy: number }
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
  | { kind: "release" };

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

type Region = Exclude<HitRegion, "none">;

type Phase =
  | { k: "idle" }
  /** Pressed, not yet a drag, scrub or menu. second: may be the second tap of a double tap. */
  | { k: "pending"; id: number; type: PointerKind; region: Region; x0: number; y0: number; t0: number; second: boolean }
  | { k: "pan"; id: number; type: PointerKind; lastX: number; lastY: number }
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
    if (p.k === "pan" || p.k === "scrub" || p.k === "axisScale" || p.k === "pinch") return true;
    return p.k === "pending" && p.region === "priceAxis";
  }

  /** When tick() next has something to decide (a long press or a deferred tap), or null. */
  nextDeadline(): number | null {
    let d: number | null = null;
    const p = this.phase;
    if (p.k === "pending" && touchLike(p.type)) d = p.t0 + LONG_PRESS_MS;
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
    this.phase = { k: "idle" };
    this.pointers.clear();
    this.pendingTap = null;
    return wasScrub ? endScrub() : [];
  }

  /** Advances time-driven decisions: long press → scrub or axis menu; an expired tap → pin. */
  tick(t: number, ctx: GestureContext): GestureAction[] {
    const out: GestureAction[] = [];
    const p = this.phase;
    if (p.k === "pending" && touchLike(p.type) && t - p.t0 >= LONG_PRESS_MS) {
      // A long press is never the second tap of a double tap.
      if (p.second) this.pendingTap = null;
      const pt = this.pointers.get(p.id) ?? { x: p.x0, y: p.y0 };
      if (p.region === "priceAxis") {
        this.phase = { k: "ignore" };
        out.push({ kind: "openAxisMenu", x: pt.x, y: pt.y });
      } else {
        this.phase = { k: "scrub", id: p.id };
        out.push(...scrubAt(ctx, pt.x, pt.y));
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
        return this.up(input);
      case "cancel":
        return this.cancel(input);
      case "leave":
        // Only a mouse hovers; with a button held the pointer is captured.
        if (input.pointerType !== "mouse" || this.phase.k !== "idle") return [];
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
    const p = this.phase;

    if (p.k === "pinch" || p.k === "ignore") return [];
    if (p.k !== "idle") {
      // A second finger during an undecided press or a pan becomes a pinch.
      // During a scrub or a price drag it is ignored (one gesture at a time).
      if (touchLike(e.pointerType) && (p.k === "pending" || p.k === "pan")) {
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
    this.phase = { k: "pending", id: e.pointerId, type: e.pointerType, region, x0: e.x, y0: e.y, t0: e.t, second };
    return out;
  }

  private move(e: GestureInput, ctx: GestureContext): GestureAction[] {
    const p = this.phase;
    if (this.pointers.has(e.pointerId)) this.pointers.set(e.pointerId, { x: e.x, y: e.y });

    switch (p.k) {
      case "idle":
        return e.pointerType === "mouse" ? hover(ctx, e.x, e.y) : [];
      case "pending": {
        if (e.pointerId !== p.id) return [];
        const dx = e.x - p.x0;
        const dy = e.y - p.y0;
        const slop = p.type === "mouse" ? MOUSE_SLOP_PX : TOUCH_SLOP_PX;
        if (Math.hypot(dx, dy) < slop) return p.type === "mouse" ? hover(ctx, e.x, e.y) : [];
        // Past the slop: whatever this becomes, it is not a tap.
        if (p.second) this.pendingTap = null;
        if (p.region === "priceAxis") {
          this.phase = { k: "axisScale", id: p.id, lastY: p.y0 };
          return this.move(e, ctx);
        }
        if (touchLike(p.type) && Math.abs(dy) > Math.abs(dx)) {
          this.phase = { k: "ignore" };
          return [{ kind: "release" }];
        }
        this.phase = { k: "pan", id: p.id, type: p.type, lastX: p.x0, lastY: p.y0 };
        return this.move(e, ctx);
      }
      case "pan": {
        if (e.pointerId !== p.id) return [];
        const dx = e.x - p.lastX;
        // A touch pan is horizontal only: the vertical axis belongs to the page.
        const dy = p.type === "mouse" ? e.y - p.lastY : 0;
        p.lastX = e.x;
        p.lastY = e.y;
        if (dx === 0 && dy === 0) return [];
        return [{ kind: "pan", dxSlots: dx / ctx.model.index.barSpacing, dy }];
      }
      case "scrub":
        return e.pointerId === p.id ? scrubAt(ctx, e.x, e.y) : [];
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

  private up(e: GestureInput): GestureAction[] {
    this.pointers.delete(e.pointerId);
    const p = this.phase;
    switch (p.k) {
      case "pending": {
        if (e.pointerId !== p.id) return [];
        this.phase = { k: "idle" };
        // A mouse click inspects by hover already; nothing more to do.
        if (p.type === "mouse") return [];
        if (p.second) {
          this.pendingTap = null;
          return doubleTap(p.region);
        }
        this.pendingTap = { x: p.x0, y: p.y0, t: e.t, region: p.region };
        return [];
      }
      case "pan":
      case "axisScale":
        if (e.pointerId === p.id) this.phase = { k: "idle" };
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
    if (p.k !== "idle") this.phase = this.pointers.size ? { k: "ignore" } : { k: "idle" };
    return wasScrub ? endScrub() : [];
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
      if (action.dy && !isAutoPrice(next)) {
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

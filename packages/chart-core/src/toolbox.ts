import type { Size } from "./layout.ts";

// The floating drawing-tools palette (owner request 2026-10-17: movable
// inside the chart, position persists). Where it sits is stored as
// FRACTIONS of the room it can move in (container minus palette), not as
// pixels: 0 is flush left/top, 1 flush right/bottom. So a palette parked in
// the bottom-right corner stays there when the window, the phone or the
// layout changes size, and it can never be stranded outside the chart.

export type ToolboxOrientation = "vertical" | "horizontal";

export interface ToolboxPlacement {
  x: number;
  y: number;
  orientation: ToolboxOrientation;
}

/** Desktop default: a vertical strip on the left edge, a little below the indicator legend. */
export const DEFAULT_TOOLBOX: ToolboxPlacement = { x: 0, y: 0.35, orientation: "vertical" };

/** Phone default: a row along the top of the chart. */
export const DEFAULT_TOOLBOX_PHONE: ToolboxPlacement = { x: 0, y: 0, orientation: "horizontal" };

/** Gap kept between the palette and the chart's edges, in px. */
export const TOOLBOX_INSET = 6;

const clamp01 = (v: number) => (Number.isFinite(v) ? Math.min(1, Math.max(0, v)) : 0);

/** The palette's top-left in px inside a container, for a placement and the palette's own size. */
export function toolboxPosition(p: ToolboxPlacement, container: Size, box: Size, inset = TOOLBOX_INSET): { left: number; top: number } {
  const roomX = Math.max(0, container.width - box.width - 2 * inset);
  const roomY = Math.max(0, container.height - box.height - 2 * inset);
  return { left: inset + clamp01(p.x) * roomX, top: inset + clamp01(p.y) * roomY };
}

/** The placement for a palette dragged to `left`, `top` (px), clamped into the container. */
export function toolboxPlacement(left: number, top: number, container: Size, box: Size, orientation: ToolboxOrientation, inset = TOOLBOX_INSET): ToolboxPlacement {
  const roomX = Math.max(0, container.width - box.width - 2 * inset);
  const roomY = Math.max(0, container.height - box.height - 2 * inset);
  return { x: roomX ? clamp01((left - inset) / roomX) : 0, y: roomY ? clamp01((top - inset) / roomY) : 0, orientation };
}

/** Nudges a placement by dx, dy px (keyboard arrows), clamped. */
export function nudgeToolbox(p: ToolboxPlacement, dx: number, dy: number, container: Size, box: Size): ToolboxPlacement {
  const at = toolboxPosition(p, container, box);
  return toolboxPlacement(at.left + dx, at.top + dy, container, box, p.orientation);
}

/** Reads a stored placement defensively (an old or hand-edited value never breaks the chart). */
export function parseToolboxPlacement(raw: unknown, fallback: ToolboxPlacement): ToolboxPlacement {
  if (typeof raw !== "object" || raw === null) return fallback;
  const r = raw as Partial<ToolboxPlacement>;
  const orientation = r.orientation === "horizontal" || r.orientation === "vertical" ? r.orientation : fallback.orientation;
  return { x: clamp01(Number(r.x)), y: clamp01(Number(r.y)), orientation };
}

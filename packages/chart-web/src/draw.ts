import type { DisplayList } from "@stockwatcher/chart-core";

/** The subset of CanvasRenderingContext2D the renderer uses (tests pass a recorder). */
export type Ctx2D = Pick<
  CanvasRenderingContext2D,
  | "beginPath"
  | "moveTo"
  | "lineTo"
  | "closePath"
  | "rect"
  | "roundRect"
  | "fill"
  | "stroke"
  | "fillText"
  | "setLineDash"
  | "fillStyle"
  | "strokeStyle"
  | "lineWidth"
  | "font"
  | "textAlign"
  | "textBaseline"
>;

/**
 * Executes a chart-core display list on a Canvas 2D context. The caller has
 * already scaled the context for devicePixelRatio. Primitives arrive in paint
 * order (chart-core sorts them), so this is a straight loop; the only state
 * it tracks is the dash pattern and font, to avoid redundant resets.
 */
export function drawDisplayList(ctx: Ctx2D, list: DisplayList): void {
  let dashKey = "";
  let font = "";
  const setDash = (dash: number[] | undefined) => {
    const key = dash ? dash.join(",") : "";
    if (key !== dashKey) {
      ctx.setLineDash(dash ?? []);
      dashKey = key;
    }
  };
  for (const p of list) {
    switch (p.kind) {
      case "rect": {
        ctx.beginPath();
        if (p.radius) ctx.roundRect(p.x, p.y, p.w, p.h, p.radius);
        else ctx.rect(p.x, p.y, p.w, p.h);
        if (p.fill) {
          ctx.fillStyle = p.fill;
          ctx.fill();
        }
        if (p.stroke) {
          setDash(p.dash);
          ctx.strokeStyle = p.stroke;
          ctx.lineWidth = p.lineWidth ?? 1;
          ctx.stroke();
        }
        break;
      }
      case "line": {
        setDash(p.dash);
        ctx.beginPath();
        ctx.moveTo(p.x1, p.y1);
        ctx.lineTo(p.x2, p.y2);
        ctx.strokeStyle = p.stroke;
        ctx.lineWidth = p.lineWidth;
        ctx.stroke();
        break;
      }
      case "text": {
        const f = `${p.fontWeight} ${p.fontSize}px ${p.fontFamily}`;
        if (f !== font) {
          ctx.font = f;
          font = f;
        }
        ctx.textAlign = p.align;
        ctx.textBaseline = p.baseline;
        ctx.fillStyle = p.fill;
        ctx.fillText(p.text, p.x, p.y);
        break;
      }
      case "path": {
        const pts = p.points;
        if (pts.length < 4) break;
        ctx.beginPath();
        ctx.moveTo(pts[0]!, pts[1]!);
        for (let i = 2; i + 1 < pts.length; i += 2) ctx.lineTo(pts[i]!, pts[i + 1]!);
        if (p.closed) ctx.closePath();
        if (p.fill) {
          ctx.fillStyle = p.fill;
          ctx.fill();
        }
        if (p.stroke) {
          setDash(undefined);
          ctx.strokeStyle = p.stroke;
          ctx.lineWidth = p.lineWidth ?? 1;
          ctx.stroke();
        }
        break;
      }
    }
  }
  setDash(undefined);
}

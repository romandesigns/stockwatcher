import { ClipOp, matchFont, PaintStyle, Skia, type SkCanvas, type SkFont, type SkPaint } from "@shopify/react-native-skia";
import type { DisplayList, Rect } from "@stockwatcher/chart-core";
import { Platform } from "react-native";

// Executes a chart-core display list on a Skia canvas: the native twin of
// chart-web's draw.ts. Coordinates are layout px (Skia scales for the
// screen density itself). The display list is the whole contract, so the
// chart looks the same on a phone as in a browser.
//
// Fonts: the theme's font strings are CSS stacks; here a stack containing
// "Mono" maps to the platform monospace face and anything else to the
// platform sans. Matched fonts are cached per family, size and weight.

const MONO = Platform.select({ ios: "Menlo", default: "monospace" });
const SANS = Platform.select({ ios: "Helvetica Neue", default: "sans-serif" });

const fontCache = new Map<string, SkFont>();

function fontFor(family: string, size: number, weight: number): SkFont {
  const face = /mono/i.test(family) ? MONO : SANS;
  const w = weight >= 600 ? "600" : weight >= 500 ? "500" : "400";
  const key = `${face}|${size}|${w}`;
  let f = fontCache.get(key);
  if (!f) {
    f = matchFont({ fontFamily: face, fontSize: size, fontWeight: w });
    fontCache.set(key, f);
  }
  return f;
}

const dashCache = new Map<string, ReturnType<typeof Skia.PathEffect.MakeDash>>();
function dash(intervals: number[] | undefined) {
  if (!intervals || intervals.length === 0) return null;
  const key = intervals.join(",");
  let e = dashCache.get(key);
  if (!e) {
    e = Skia.PathEffect.MakeDash(intervals, 0);
    dashCache.set(key, e);
  }
  return e;
}

function clipTo(canvas: SkCanvas, r: Rect) {
  canvas.save();
  canvas.clipRect(Skia.XYWHRect(r.x, r.y, r.w, r.h), ClipOp.Intersect, true);
}

/** Draws `list` (already in paint order) onto `canvas`. */
export function drawDisplayList(canvas: SkCanvas, list: DisplayList): void {
  const fill: SkPaint = Skia.Paint();
  fill.setAntiAlias(true);
  fill.setStyle(PaintStyle.Fill);
  const stroke: SkPaint = Skia.Paint();
  stroke.setAntiAlias(true);
  stroke.setStyle(PaintStyle.Stroke);

  for (const p of list) {
    switch (p.kind) {
      case "rect": {
        const rect = Skia.XYWHRect(p.x, p.y, p.w, p.h);
        const rr = p.radius ? Skia.RRectXY(rect, p.radius, p.radius) : null;
        if (p.fill) {
          fill.setColor(Skia.Color(p.fill));
          if (rr) canvas.drawRRect(rr, fill);
          else canvas.drawRect(rect, fill);
        }
        if (p.stroke) {
          stroke.setColor(Skia.Color(p.stroke));
          stroke.setStrokeWidth(p.lineWidth ?? 1);
          stroke.setPathEffect(dash(p.dash));
          if (rr) canvas.drawRRect(rr, stroke);
          else canvas.drawRect(rect, stroke);
        }
        break;
      }
      case "line": {
        if (p.clip) clipTo(canvas, p.clip);
        stroke.setColor(Skia.Color(p.stroke));
        stroke.setStrokeWidth(p.lineWidth);
        stroke.setPathEffect(dash(p.dash));
        canvas.drawLine(p.x1, p.y1, p.x2, p.y2, stroke);
        if (p.clip) canvas.restore();
        break;
      }
      case "text": {
        const font = fontFor(p.fontFamily, p.fontSize, p.fontWeight);
        fill.setColor(Skia.Color(p.fill));
        const w = font.measureText(p.text).width;
        const x = p.align === "left" ? p.x : p.align === "center" ? p.x - w / 2 : p.x - w;
        const m = font.getMetrics();
        // Skia draws at the baseline; ascent is negative.
        const y = p.baseline === "top" ? p.y - m.ascent : p.baseline === "bottom" ? p.y - m.descent : p.y - (m.ascent + m.descent) / 2;
        canvas.drawText(p.text, x, y, fill, font);
        break;
      }
      case "path": {
        const pts = p.points;
        if (pts.length < 4) break;
        if (p.clip) clipTo(canvas, p.clip);
        const b = Skia.PathBuilder.Make();
        b.moveTo(pts[0]!, pts[1]!);
        for (let i = 2; i + 1 < pts.length; i += 2) b.lineTo(pts[i]!, pts[i + 1]!);
        if (p.closed) b.close();
        const path = b.build();
        if (p.fill) {
          fill.setColor(Skia.Color(p.fill));
          canvas.drawPath(path, fill);
        }
        if (p.stroke) {
          stroke.setColor(Skia.Color(p.stroke));
          stroke.setStrokeWidth(p.lineWidth ?? 1);
          stroke.setPathEffect(null);
          canvas.drawPath(path, stroke);
        }
        if (p.clip) canvas.restore();
        break;
      }
    }
  }
}

/** Records a display list into a Skia picture (what <Picture> draws). */
export function recordPicture(list: DisplayList, width: number, height: number) {
  const recorder = Skia.PictureRecorder();
  const canvas = recorder.beginRecording(Skia.XYWHRect(0, 0, width, height));
  drawDisplayList(canvas, list);
  return recorder.finishRecordingAsPicture();
}

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
//
// Glyph fallback: a browser falls back to another font for a character its
// font lacks; Skia's matchFont does not, and draws a "tofu" box. Android's
// monospace face has none of the label symbols chart-core uses (↑ ↓ → ▲ ▼
// ✕ ≈), so each text is drawn in runs: the matched face, else the platform
// sans in the same size and weight, else (for the arrows, triangles and ✕)
// a small vector shape drawn at the text's size, else a plainer substitute
// (≈ → ~), so a label never shows a box. Checked per character, cached.

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

/** Symbols drawn as vector shapes when no face has them (see drawShape). */
const SHAPES = new Set(["▲", "▼", "↑", "↓", "→", "✕"]);
/** Plainer stand-ins for anything else no available face has. */
const SUBSTITUTES: Readonly<Record<string, string>> = { "≈": "~" };

const glyphCache = new Map<string, boolean>();
function hasGlyph(font: SkFont, key: string, ch: string): boolean {
  const k = `${key}|${ch}`;
  let v = glyphCache.get(k);
  if (v === undefined) {
    v = !font.getGlyphIDs(ch).includes(0);
    glyphCache.set(k, v);
  }
  return v;
}

type Run = { text: string; font: SkFont; shape?: boolean };

/** Splits `text` into runs each drawable by one font (see "Glyph fallback" above). */
function textRuns(text: string, family: string, size: number, weight: number): Run[] {
  const primary = fontFor(family, size, weight);
  // Fast path: almost every label is ASCII digits and letters.
  if (/^[\x20-\x7e]*$/.test(text)) return [{ text, font: primary }];
  const fallback = fontFor("sans", size, weight);
  const pk = `${family}|${size}|${weight}`;
  const fk = `sans|${size}|${weight}`;
  const out: Run[] = [];
  for (let ch of text) {
    let font: SkFont | null = null;
    for (let tries = 0; tries < 4 && !font; tries++) {
      if (hasGlyph(primary, pk, ch)) font = primary;
      else if (hasGlyph(fallback, fk, ch)) font = fallback;
      else if (SHAPES.has(ch)) break;
      else if (SUBSTITUTES[ch]) ch = SUBSTITUTES[ch]!;
      else break;
    }
    if (!font && SHAPES.has(ch)) {
      out.push({ text: ch, font: primary, shape: true });
      continue;
    }
    font ??= primary;
    const last = out.at(-1);
    if (last && last.font === font && !last.shape) last.text += ch;
    else out.push({ text: ch, font });
  }
  return out;
}

function advance(font: SkFont, text: string): number {
  let w = 0;
  for (const g of font.getGlyphWidths(font.getGlyphIDs(text))) w += g;
  return w;
}

/** A symbol's advance when drawn as a shape: about a glyph's width at this size. */
const shapeWidth = (size: number) => size * 0.72;

/** Draws ▲ ▼ ↑ ↓ → ✕ as vector shapes in a glyph-sized box whose left edge is x and baseline y. */
function drawShape(canvas: SkCanvas, ch: string, x: number, y: number, size: number, fill: SkPaint, stroke: SkPaint) {
  const w = shapeWidth(size);
  const h = size * 0.62; // about the cap height
  const cx = x + w / 2;
  const top = y - h;
  const b = Skia.PathBuilder.Make();
  if (ch === "▲" || ch === "▼") {
    const up = ch === "▲";
    const half = w * 0.42;
    b.moveTo(cx - half, up ? y - h * 0.1 : top + h * 0.1);
    b.lineTo(cx + half, up ? y - h * 0.1 : top + h * 0.1);
    b.lineTo(cx, up ? top + h * 0.05 : y - h * 0.05);
    b.close();
    canvas.drawPath(b.build(), fill);
    return;
  }
  stroke.setStrokeWidth(Math.max(1, size * 0.1));
  stroke.setPathEffect(null);
  stroke.setColor(fill.getColor());
  const head = w * 0.3;
  if (ch === "↑" || ch === "↓") {
    const up = ch === "↑";
    const tip = up ? top : y;
    const tail = up ? y : top;
    const d = up ? head : -head;
    b.moveTo(cx, tail);
    b.lineTo(cx, tip);
    b.moveTo(cx - head, tip + d);
    b.lineTo(cx, tip);
    b.lineTo(cx + head, tip + d);
  } else if (ch === "→") {
    const my = y - h / 2;
    b.moveTo(x + w * 0.1, my);
    b.lineTo(x + w * 0.9, my);
    b.moveTo(x + w * 0.9 - head, my - head);
    b.lineTo(x + w * 0.9, my);
    b.lineTo(x + w * 0.9 - head, my + head);
  } else {
    const i = w * 0.18;
    b.moveTo(x + i, top + i);
    b.lineTo(x + w - i, y - i);
    b.moveTo(x + w - i, top + i);
    b.lineTo(x + i, y - i);
  }
  canvas.drawPath(b.build(), stroke);
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
        const runs = textRuns(p.text, p.fontFamily, p.fontSize, p.fontWeight);
        fill.setColor(Skia.Color(p.fill));
        // Advance widths (glyph advances), not measureText's ink bounds, which drop a run's
        // trailing space and would draw the next run on top of it.
        const widths = runs.map((r) => (r.shape ? shapeWidth(p.fontSize) : advance(r.font, r.text)));
        const w = widths.reduce((a, b) => a + b, 0);
        let x = p.align === "left" ? p.x : p.align === "center" ? p.x - w / 2 : p.x - w;
        // Vertical placement from the label's own face, so a fallback run shares its baseline.
        const m = runs[0]!.font.getMetrics();
        // Skia draws at the baseline; ascent is negative.
        const y = p.baseline === "top" ? p.y - m.ascent : p.baseline === "bottom" ? p.y - m.descent : p.y - (m.ascent + m.descent) / 2;
        runs.forEach((r, i) => {
          if (r.shape) drawShape(canvas, r.text, x, y, p.fontSize, fill, stroke);
          else canvas.drawText(r.text, x, y, fill, r.font);
          x += widths[i]!;
        });
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

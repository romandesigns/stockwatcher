// OKLCH, for the indicator colour picker's hue strip (owner, 2026-11-07: "a color picker with the
// same brightness/saturation as the color samples you selected"). OKLCH is the perceptual space
// (Björn Ottosson's Oklab in polar form, the CSS Color 4 `oklch()`): holding L and C and turning
// only H keeps a colour's lightness and colourfulness as the eye sees them, which sRGB/HSL do not
// (an HSL yellow and blue at one "lightness" are nowhere near each other). Pure functions, shared
// by the web dialog and the phone sheet so both pick the same hex for the same hue.

export interface Oklch {
  /** Perceived lightness, 0–1. */
  l: number;
  /** Chroma (colourfulness), 0 to ~0.37 inside sRGB. */
  c: number;
  /** Hue angle in degrees, 0–360. */
  h: number;
}

const toLinear = (v: number) => (v <= 0.04045 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4);
const toGamma = (v: number) => (v <= 0.0031308 ? 12.92 * v : 1.055 * v ** (1 / 2.4) - 0.055);

/** `#rrggbb` → OKLCH (null for anything else). */
export function hexToOklch(hex: string): Oklch | null {
  const m = /^#([0-9a-f]{6})$/i.exec(hex);
  if (!m) return null;
  const n = Number.parseInt(m[1]!, 16);
  const r = toLinear(((n >> 16) & 255) / 255);
  const g = toLinear(((n >> 8) & 255) / 255);
  const b = toLinear((n & 255) / 255);
  const l_ = Math.cbrt(0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b);
  const m_ = Math.cbrt(0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b);
  const s_ = Math.cbrt(0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b);
  const L = 0.2104542553 * l_ + 0.793617785 * m_ - 0.0040720468 * s_;
  const A = 1.9779984951 * l_ - 2.428592205 * m_ + 0.4505937099 * s_;
  const B = 0.0259040371 * l_ + 0.7827717662 * m_ - 0.808675766 * s_;
  const c = Math.hypot(A, B);
  const h = c < 1e-7 ? 0 : ((Math.atan2(B, A) * 180) / Math.PI + 360) % 360;
  return { l: L, c, h };
}

/** OKLCH → linear sRGB, unclamped (a channel outside 0–1 = outside the sRGB gamut). */
function oklchToLinear({ l, c, h }: Oklch): [number, number, number] {
  const hr = (h * Math.PI) / 180;
  const A = c * Math.cos(hr);
  const B = c * Math.sin(hr);
  const l_ = (l + 0.3963377774 * A + 0.2158037573 * B) ** 3;
  const m_ = (l - 0.1055613458 * A - 0.0638541728 * B) ** 3;
  const s_ = (l - 0.0894841775 * A - 1.291485548 * B) ** 3;
  return [4.0767416621 * l_ - 3.3077115913 * m_ + 0.2309699292 * s_, -1.2684380046 * l_ + 2.6097574011 * m_ - 0.3413193965 * s_, -0.0041960863 * l_ - 0.7034186147 * m_ + 1.707614701 * s_];
}

const EPS = 1e-4;
const inGamut = (rgb: readonly number[]) => rgb.every((v) => v >= -EPS && v <= 1 + EPS);

/** Whether an OKLCH colour fits in sRGB as it is. */
export function oklchInGamut(c: Oklch): boolean {
  return inGamut(oklchToLinear(c));
}

/**
 * OKLCH → `#rrggbb`. Out of gamut, the chroma is lowered (lightness and hue kept, binary search)
 * until it fits: the CSS Color 4 approach without the ΔE shortcut, so the hue strip never jumps
 * hue or brightness where sRGB runs out, it only goes slightly less vivid there.
 */
export function oklchToHex(color: Oklch): string {
  let c = Math.max(0, color.c);
  const l = Math.min(1, Math.max(0, color.l));
  let rgb = oklchToLinear({ l, c, h: color.h });
  if (!inGamut(rgb)) {
    let lo = 0;
    let hi = c;
    for (let i = 0; i < 24; i++) {
      const mid = (lo + hi) / 2;
      if (inGamut(oklchToLinear({ l, c: mid, h: color.h }))) lo = mid;
      else hi = mid;
    }
    c = lo;
    rgb = oklchToLinear({ l, c, h: color.h });
  }
  const byte = (v: number) => Math.round(Math.min(1, Math.max(0, toGamma(Math.min(1, Math.max(0, v))))) * 255);
  return `#${rgb.map((v) => byte(v).toString(16).padStart(2, "0")).join("")}`;
}

/** Below this chroma a swatch is a neutral (the palette's grey and off-white): no hue to share. */
export const NEUTRAL_CHROMA = 0.03;

/** The tonal family the hue strip draws at: one lightness and one chroma. */
export interface ToneFamily {
  l: number;
  c: number;
}

const median = (xs: readonly number[]): number => {
  const s = [...xs].sort((a, b) => a - b);
  const mid = s.length >> 1;
  return s.length % 2 ? s[mid]! : (s[mid - 1]! + s[mid]!) / 2;
};

/**
 * The swatches' tonal family: the MEDIAN lightness and chroma of the chromatic swatches (the
 * neutrals, chroma under NEUTRAL_CHROMA, carry no hue and would drag the chroma toward grey).
 * Measured on the default palette (2026-11-07): the seven chromatic swatches (series 1–3, up,
 * down, warning, accent) sit in ONE band, L 0.62–0.76 and C 0.14–0.17, so they do not form
 * separate tonal rows and a second strip would be a near-duplicate; the family is L 0.678,
 * C 0.145. The median rather than the mean keeps a theme override with one outlier swatch from
 * pulling the whole strip. At that chroma about 30 % of hues (the cyans and a few greens) are
 * just outside sRGB and are drawn a little less vivid (oklchToHex). Null when no swatch has a hue.
 */
export function swatchFamily(swatches: readonly string[]): ToneFamily | null {
  const tones = swatches.map(hexToOklch).filter((t): t is Oklch => !!t && t.c >= NEUTRAL_CHROMA);
  if (!tones.length) return null;
  return { l: median(tones.map((t) => t.l)), c: median(tones.map((t) => t.c)) };
}

/** The hue strip's colour at `hue` degrees: the family's L and C at that hue, gamut-mapped. */
export function familyColor(family: ToneFamily, hue: number): string {
  return oklchToHex({ l: family.l, c: family.c, h: ((hue % 360) + 360) % 360 });
}

/** The strip's gradient stops (every `step` degrees, 0 to 360 inclusive), for CSS or SVG. */
export function familyGradient(family: ToneFamily, step = 15): { offset: number; color: string }[] {
  const out: { offset: number; color: string }[] = [];
  for (let h = 0; h <= 360; h += step) out.push({ offset: h / 360, color: familyColor(family, h) });
  return out;
}

/** A colour's hue (for placing the strip's thumb on the current colour); null for a neutral. */
export function hueOf(hex: string): number | null {
  const t = hexToOklch(hex);
  return t && t.c >= NEUTRAL_CHROMA ? t.h : null;
}

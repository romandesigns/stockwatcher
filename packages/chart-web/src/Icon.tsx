import { ICONS, type IconDef, type IconHue, type IconName, type IconPath } from "@stockwatcher/chart-core";
import type { CSSProperties } from "react";
import { cn } from "./ui/utils.ts";

// The web renderer of chart-core's duotone icons (icons.ts). Tones come from
// two CSS custom properties the host defines with its palette (apps/web
// index.css):
//   --icon-primary    the main mark; `currentColor`, so it follows the
//                     state's text colour (muted when idle, foreground when
//                     raised, accent when an active state spends it)
//   --icon-secondary  the soft second tone; currentColor mixed toward
//                     transparent by ICON_SECONDARY_OPACITY
// Both are written as `var(--icon-*, fallback)` so the package still draws
// correctly in a host without them. Both resolve currentColor at the PATH,
// so a hued path only sets its own `color` and still gets both tones.
// Paint goes through `style`, not presentation attributes: var() in an
// SVG attribute is not reliable across engines.

const PRIMARY = "var(--icon-primary, currentColor)";
const SECONDARY = "var(--icon-secondary, color-mix(in srgb, currentColor 40%, transparent))";
const HUE: Readonly<Record<IconHue, string>> = { good: "var(--good, #3fb97a)", critical: "var(--critical, #f0645c)", warning: "var(--warning, #e0a63a)" };

function pathStyle(p: IconPath, strokeWidth: number): CSSProperties {
  const tone = p.role === "primary" ? PRIMARY : SECONDARY;
  const fill = p.paint === "fill";
  return {
    ...(p.hue ? { color: HUE[p.hue] } : {}),
    fill: fill ? tone : "none",
    stroke: fill ? "none" : tone,
    strokeWidth: fill ? undefined : (p.width ?? strokeWidth),
    strokeDasharray: p.dash?.join(" "),
  };
}

export interface IconProps {
  /** A named icon, or any duotone definition (a layer swatch, an indicator glyph). */
  name?: IconName;
  def?: IconDef;
  /** Rendered size in px; default the viewBox size (the host's `[&_svg]:size-*` classes usually set it). */
  width?: number;
  height?: number;
  className?: string;
  /** Sets `color`, i.e. both tones at once (an indicator glyph tinted with its line colour). */
  color?: string;
}

/** A duotone icon; decorative (aria-hidden), so the control around it carries the name. */
export function Icon({ name, def, width, height, className, color }: IconProps) {
  const d = def ?? (name ? ICONS[name] : undefined);
  if (!d) return null;
  return (
    <svg
      viewBox={`0 0 ${d.w} ${d.h}`}
      width={width ?? d.w}
      height={height ?? d.h}
      className={cn("shrink-0", className)}
      style={color ? { color } : undefined}
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
      focusable="false"
    >
      {d.paths.map((p, i) => (
        <path key={i} d={p.d} style={pathStyle(p, d.strokeWidth)} />
      ))}
    </svg>
  );
}

/** A bare path list (INDICATOR_GLYPHS) as a 16 × 16 icon. */
export const glyphDef = (paths: readonly IconPath[]): IconDef => ({ w: 16, h: 16, strokeWidth: 1.5, paths });

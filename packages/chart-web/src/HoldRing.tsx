import { HOLD_ALERT_MS, HOLD_RING_DELAY_MS } from "@stockwatcher/chart-core";
import type { CSSProperties } from "react";

export interface HoldRingProps {
  /** Centre, in px from the chart surface's top-left. */
  x: number;
  y: number;
  /** Diameter (px). */
  size?: number;
}

const R = 14;
const C = 2 * Math.PI * R;

/**
 * The press-and-hold ring (approved design: "a ring fills under the cursor"). It appears
 * HOLD_RING_DELAY_MS into a hold (chart-core's GestureController decides when) and fills over
 * the rest of HOLD_ALERT_MS, when the quick alert card opens. The fill is a CSS animation of the
 * stroke's dash offset, so it runs on the compositor and costs the chart no frames. With reduced
 * motion it shows full at once (the card still waits for the hold).
 */
export function HoldRing({ x, y, size = 36 }: HoldRingProps) {
  const style = { left: x - size / 2, top: y - size / 2, width: size, height: size, "--sw-ring-c": `${C}px`, "--sw-ring-ms": `${HOLD_ALERT_MS - HOLD_RING_DELAY_MS}ms` } as CSSProperties;
  return (
    <svg aria-hidden="true" viewBox="0 0 36 36" className="pointer-events-none absolute z-[6]" style={style}>
      <style>{"@keyframes sw-hold-fill{from{stroke-dashoffset:var(--sw-ring-c)}to{stroke-dashoffset:0}}@media (prefers-reduced-motion: reduce){.sw-hold-fill{animation:none!important;stroke-dashoffset:0!important}}"}</style>
      <circle cx="18" cy="18" r={R} fill="none" className="stroke-bg-raised" strokeWidth="3" />
      <circle
        cx="18"
        cy="18"
        r={R}
        fill="none"
        className="sw-hold-fill stroke-primary"
        strokeWidth="3"
        strokeLinecap="round"
        strokeDasharray={C}
        transform="rotate(-90 18 18)"
        style={{ animation: "sw-hold-fill var(--sw-ring-ms) linear forwards" }}
      />
      <circle cx="18" cy="18" r="3" className="fill-foreground" />
    </svg>
  );
}

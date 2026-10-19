import { GAUGE, GAUGE_GRADIENT, gaugeAngle, gaugeArc, gaugeColors } from "@stockwatcher/shared";
import { useId } from "react";

const TRACK = gaugeArc(GAUGE.start, GAUGE.start + GAUGE.sweep);

/**
 * The Halt watcher's 240° speedometer (design canvas "Halt Watcher", Gauge; wavykit
 * `pressure-gauge`, ported onto this app's tokens like the nav). Track, value arc, needle and the
 * 0–100 value under the hub. Direction is the hue and strength the lightness: the side's gradient
 * spans the whole dial and the arc reveals as much of it as the pressure reaches. Halted: a grey
 * arc and a dim needle, the frozen value still shown.
 *
 * The value arc is the track itself, dashed to the pressure (`pathLength` 100), and the needle a
 * fixed line rotated to its angle: both are CSS-transitionable, so a new reading sweeps rather
 * than jumps (and holds still with reduced motion).
 *
 * In the phone's one-row strip (the Halt watcher container below @xl, ~56 px a dial) the needle,
 * hub and small value are hidden and the value is drawn large inside the arc instead: at that size
 * a needle covers the number and the number is unreadable (owner, 2026-10-19).
 */
export function PressureGauge({ pressure, side, halted, label }: { pressure: number; side: "bull" | "bear"; halted: boolean; label: string }) {
  // useId has colons, which a url(#…) reference does not survive.
  const id = `hw-${useId().replace(/[^a-zA-Z0-9_-]/g, "")}`;
  const v = Math.max(0, Math.min(100, Math.round(pressure)));
  const colors = gaugeColors(side, halted);
  return (
    <svg viewBox={`0 0 ${GAUGE.width} ${GAUGE.height}`} role="img" aria-label={label} className="block h-auto w-full">
      {colors && (
        <defs>
          <linearGradient id={id} gradientUnits="userSpaceOnUse" x1={GAUGE_GRADIENT.x1} y1={0} x2={GAUGE_GRADIENT.x2} y2={0}>
            <stop offset="0" stopColor={colors.from} />
            <stop offset="1" stopColor={colors.to} />
          </linearGradient>
        </defs>
      )}
      <path d={TRACK} fill="none" style={{ stroke: "var(--bg-raised)" }} strokeWidth={GAUGE.stroke} strokeLinecap="round" />
      <path
        d={TRACK}
        pathLength={100}
        fill="none"
        // Paint through style: var() in an SVG presentation attribute is not reliable (chart-web Icon).
        style={{ stroke: colors ? `url(#${id})` : "color-mix(in srgb, var(--text-faint) 55%, transparent)" }}
        strokeWidth={GAUGE.stroke}
        strokeLinecap="round"
        // A round cap would leave a dot at zero.
        strokeDasharray={`${v} 100`}
        opacity={v > 0 ? 1 : 0}
        className="transition-[stroke-dasharray] duration-300 ease-out motion-reduce:transition-none"
      />
      <g
        style={{ transform: `rotate(${gaugeAngle(v)}deg)`, transformOrigin: `${GAUGE.cx}px ${GAUGE.cy}px`, transformBox: "view-box" }}
        className="transition-transform duration-300 ease-out motion-reduce:transition-none @max-xl/hw:hidden"
      >
        <line x1={GAUGE.cx} y1={GAUGE.cy} x2={GAUGE.cx + GAUGE.needle} y2={GAUGE.cy} style={{ stroke: halted ? "var(--text-faint)" : "var(--text)" }} strokeWidth={3} strokeLinecap="round" />
      </g>
      <circle cx={GAUGE.cx} cy={GAUGE.cy} r={6} style={{ fill: halted ? "var(--text-faint)" : "var(--text)" }} className="@max-xl/hw:hidden" />
      <text x={GAUGE.cx} y={132} textAnchor="middle" style={{ fill: "var(--text)" }} className="font-mono @max-xl/hw:hidden" fontSize={26} fontWeight={600}>
        {v}
      </text>
      <text x={GAUGE.cx} y={118} textAnchor="middle" style={{ fill: halted ? "var(--text-faint)" : "var(--text)" }} className="hidden font-mono @max-xl/hw:inline" fontSize={58} fontWeight={600}>
        {v}
      </text>
    </svg>
  );
}

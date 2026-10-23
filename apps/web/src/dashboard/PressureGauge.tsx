import { GAUGE, gaugeArc, gaugeCaption, gaugeTone } from "@stockwatcher/shared";

const TRACK = gaugeArc(GAUGE.start, GAUGE.start + GAUGE.sweep);

const FILL = { good: "var(--good)", critical: "var(--critical)", halted: "color-mix(in srgb, var(--text-faint) 55%, transparent)" } as const;

/**
 * The Halt watcher's gauge (owner, 2026-10-19, design canvas "Halt Watcher", option A "fill"): a
 * shadcn-style radial half ring, filled in proportion to a 0–100 `value` (the halt risk since
 * 2026-10-20; the prop was `pressure` before), in the colour of the side the build-up pushes
 * (green toward limit up, red toward limit down), the reading inside the ring and
 * "limit-up/limit-down risk" under it. Halted: a grey fill, the frozen value still shown.
 *
 * The fill is the track itself, dashed to the value (`pathLength` 100), so a new reading sweeps
 * rather than jumps (CSS-transitioned; still with reduced motion).
 *
 * In the phone's one-row strip (the Halt watcher container below @xl, ~56 px a ring) the caption
 * is hidden and the reading drawn larger: at that size the caption is unreadable.
 */
export function PressureGauge({ value, side, halted, label, stale = false }: { value: number; side: "bull" | "bear"; halted: boolean; label: string; stale?: boolean }) {
  const v = Math.max(0, Math.min(100, Math.round(value)));
  const neutral = halted || stale;
  const ink = neutral ? "var(--text-faint)" : "var(--text)";
  return (
    <svg viewBox={`0 0 ${GAUGE.width} ${GAUGE.height}`} role="img" aria-label={label} className="block h-auto w-full">
      <path d={TRACK} fill="none" style={{ stroke: "var(--bg-raised)" }} strokeWidth={GAUGE.stroke} strokeLinecap="round" />
      <path
        d={TRACK}
        pathLength={100}
        fill="none"
        // Paint through style: var() in an SVG presentation attribute is not reliable (chart-web Icon).
        style={{ stroke: FILL[gaugeTone(side, neutral)] }}
        strokeWidth={GAUGE.stroke}
        strokeLinecap="round"
        // A round cap would leave a dot at zero.
        strokeDasharray={`${v} 100`}
        opacity={v > 0 ? 1 : 0}
        className={stale ? "transition-none" : "transition-[stroke-dasharray] duration-300 ease-out motion-reduce:transition-none"}
      />
      <text x={GAUGE.cx} y={90} textAnchor="middle" style={{ fill: ink }} className="font-mono @max-xl/hw:hidden" fontSize={36} fontWeight={600}>
        {v}
      </text>
      <text x={GAUGE.cx} y={110} textAnchor="middle" style={{ fill: "var(--text-dim)" }} className="@max-xl/hw:hidden" fontSize={12}>
        {stale ? "last known risk" : gaugeCaption(side, halted)}
      </text>
      <text x={GAUGE.cx} y={101} textAnchor="middle" style={{ fill: ink }} className="hidden font-mono @max-xl/hw:inline" fontSize={52} fontWeight={600}>
        {v}
      </text>
    </svg>
  );
}

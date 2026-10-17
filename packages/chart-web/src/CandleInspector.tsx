import { describeSlot, type Slot, type Timeframe } from "@stockwatcher/chart-core";
import { cn } from "./ui/utils.ts";

export interface CandleInspectorProps {
  slot: Slot | null;
  timeframe: Timeframe;
  /** "full": time, O/H/L/C, change and volume. "compact" (narrow screens): the same without volume. */
  variant?: "full" | "compact";
  className?: string;
}

const CHANGE_TONE = { up: "text-good", down: "text-critical", flat: "text-muted-foreground" } as const;

/**
 * Readout of the inspected candle (or gap), kept simple to scan (owner,
 * 2026-10-17): `09:42  O 3.19  H 3.19  L 3.16  C 3.18  −0.31%  Vol 26.8K`.
 * Labels are faint and values carry the weight; only the change is
 * coloured. The candle still forming says "Live"; a final one says
 * nothing, since nearly every candle is final. Purely visual: CandleChart
 * announces keyboard and tap inspections itself, so this stays silent while
 * it updates on every tick.
 */
export function CandleInspector({ slot, timeframe, variant = "full", className }: CandleInspectorProps) {
  const full = variant === "full";
  if (!slot) return <div className={cn("h-4 text-xs text-text-faint", className)}>Waiting for data…</div>;
  const info = describeSlot(slot, timeframe);
  return (
    <div className={cn("flex flex-wrap items-baseline font-mono tabular-nums", full ? "gap-x-3.5 gap-y-1 text-xs" : "gap-x-2.5 gap-y-0.5 text-[11px]", className)}>
      <span className="text-muted-foreground">{info.time}</span>
      {info.kind === "gap" ? (
        <span className="font-sans text-text-faint">No candle: {info.note}</span>
      ) : (
        <>
          {info.ohlc.map((r) => (
            <span key={r.k}>
              <span className="text-text-faint">{r.k}</span> <span className="text-foreground">{r.v}</span>
            </span>
          ))}
          {info.change && <span className={CHANGE_TONE[info.change.dir]}>{info.change.text}</span>}
          {full && info.volume && (
            <span>
              <span className="font-sans text-text-faint">Vol</span> <span className="text-foreground">{info.volume}</span>
            </span>
          )}
          {info.forming && (
            <span className="inline-flex items-center gap-1 font-sans text-[10px] font-medium text-warning" title="This candle is still forming">
              <span aria-hidden="true" className="size-1.5 rounded-full bg-warning" />
              Live
            </span>
          )}
        </>
      )}
    </div>
  );
}

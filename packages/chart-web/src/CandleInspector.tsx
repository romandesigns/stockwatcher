import { describeSlot, type Slot, type Timeframe } from "@stockwatcher/chart-core";
import { cn } from "./ui/utils.ts";

export interface CandleInspectorProps {
  slot: Slot | null;
  timeframe: Timeframe;
  /** "full": interval, O/H/L/C/V/Δ and Forming/Final. "compact": start time (* = forming) and O/H/L/C/V. */
  variant?: "full" | "compact";
  className?: string;
}

/**
 * Readout of the inspected candle (or gap). Purely visual: CandleChart
 * announces keyboard/tap inspections itself through a live region, so this
 * one stays silent while it updates on every tick.
 */
export function CandleInspector({ slot, timeframe, variant = "full", className }: CandleInspectorProps) {
  const full = variant === "full";
  if (!slot) return <div className={cn("h-4 font-mono text-xs text-text-faint", className)}>Waiting for data…</div>;
  const info = describeSlot(slot, timeframe);
  const forming = info.state === "Forming";
  const rows = full ? info.rows : info.rows.filter((r) => r.k !== "Δ");
  const time = full ? info.timeLabel : `${info.timeLabel.split(" – ")[0]}${forming ? "*" : ""}`;
  return (
    <div className={cn("flex flex-wrap font-mono tabular-nums text-muted-foreground", full ? "gap-x-4.5 gap-y-1 text-xs" : "gap-x-3 gap-y-0.5 text-[11px]", className)}>
      <span className="text-foreground">{time}</span>
      {info.kind === "gap" ? (
        <span className="font-sans text-text-faint">No candle: {info.note}</span>
      ) : (
        rows.map((r) => (
          <span key={r.k}>
            {r.k} <span className="text-foreground">{r.v}</span>
          </span>
        ))
      )}
      {full && info.kind === "candle" && <span className={forming ? "text-warning" : "text-muted-foreground"}>{info.state}</span>}
    </div>
  );
}

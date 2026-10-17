import { describeCounted, formatEtMillis, formatEtTime, formatPrice, formatSize, type CountedTone, type TapeTrade } from "@stockwatcher/chart-core";
import { cn } from "./ui/utils.ts";

export interface TapeProps {
  /** Newest first. */
  trades: readonly TapeTrade[];
  /** "full": ms timestamps, long wording, column header (desktop). "compact": seconds, short wording (phone). */
  variant?: "full" | "compact";
  maxRows?: number;
  className?: string;
}

// Row text: counted trades read as normal text, partial ones dimmed,
// excluded/cancelled ones faint. The note column carries the status color.
const ROW: Record<CountedTone, string> = {
  counted: "text-foreground",
  partial: "text-muted-foreground",
  excluded: "text-text-faint",
  ignored: "text-text-faint line-through",
};
const NOTE: Record<CountedTone, string> = {
  counted: "text-muted-foreground",
  partial: "text-warning",
  excluded: "text-critical",
  ignored: "text-critical",
};

/** Time and sales with how the engine counted each print ("counted as"). */
export function Tape({ trades, variant = "full", maxRows = 14, className }: TapeProps) {
  const full = variant === "full";
  const cols = full ? "grid-cols-[84px_52px_56px_minmax(0,1fr)]" : "grid-cols-[62px_48px_48px_minmax(0,1fr)]";
  const rows = trades.slice(0, maxRows);
  // Stable keys so a new print does not remount (and re-animate) every row.
  // (exchange, seq) identifies a print, except that a duplicate repeats it,
  // so the occurrence count is taken from the OLDEST end of the list, which
  // does not shift when newer rows are prepended.
  const keys: string[] = [];
  const seen = new Map<string, number>();
  for (let i = rows.length - 1; i >= 0; i--) {
    const tr = rows[i]!;
    const id = `${tr.exchange}-${tr.seq}-${tr.t}`;
    const n = seen.get(id) ?? 0;
    seen.set(id, n + 1);
    keys[i] = `${id}-${n}`;
  }
  return (
    <div className={cn("flex min-h-0 flex-col gap-1.5", className)} role="table" aria-label="Time and sales">
      {full && (
        <div role="row" className={cn("grid border-b border-bg-row pb-1 text-[10px] text-text-faint", cols)}>
          <span role="columnheader">Time</span>
          <span role="columnheader">Price</span>
          <span role="columnheader">Size</span>
          <span role="columnheader">Counted as</span>
        </div>
      )}
      {rows.length === 0 && <p className="m-0 text-xs text-text-faint">No trades yet.</p>}
      {rows.map((tr, i) => {
        const d = describeCounted(tr);
        return (
          <div
            role="row"
            key={keys[i]}
            className={cn("grid animate-tape-in font-mono text-[11px] tabular-nums", cols, ROW[d.tone])}
          >
            <span role="cell">{full ? formatEtMillis(tr.t) : formatEtTime(tr.t, true)}</span>
            <span role="cell">{formatPrice(tr.p)}</span>
            <span role="cell">{formatSize(tr.s)}</span>
            <span role="cell" className={cn("truncate font-sans no-underline", NOTE[d.tone], !full && "text-right text-[10px]")} title={`Exchange ${tr.exchange} · seq ${tr.seq}${tr.conditions.length ? ` · conditions ${tr.conditions.join(" ")}` : ""}`}>
              {full ? d.label : d.short}
            </span>
          </div>
        );
      })}
    </div>
  );
}

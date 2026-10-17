import { describeCounted, formatEtMillis, formatEtTime, formatPrice, formatSize, type CountedTone, type TapeTrade } from "@stockwatcher/chart-core";
import { Tip } from "./ui/tooltip.tsx";
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
  // Size is right-aligned in its column, with room before the note.
  // Seconds and the short note on every layout: the desktop's side panel is narrow too, and milliseconds
  // plus the long wording left the note cut off. Both stay in the row's tooltip.
  const cols = full ? "grid-cols-[62px_48px_48px_minmax(0,1fr)] gap-x-2" : "grid-cols-[62px_48px_44px_minmax(0,1fr)] gap-x-2";
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
    <div className={cn("flex min-h-0 flex-col gap-1", className)} role="table" aria-label="Time and sales">
      {full && (
        <div role="row" className={cn("grid pb-1 text-[10px] text-text-faint", cols)}>
          <span role="columnheader">Time</span>
          <span role="columnheader">Price</span>
          <span role="columnheader" className="text-right">
            Size
          </span>
          <Tip label="A blank note means the print counted toward price and volume" side="left">
            <span role="columnheader" className="w-fit justify-self-end">
              Note
            </span>
          </Tip>
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
            <span role="cell" className={cn(d.tone === "counted" && "text-muted-foreground")}>
              {formatEtTime(tr.t, true)}
            </span>
            <span role="cell">{formatPrice(tr.p)}</span>
            <span role="cell" className="text-right">
              {formatSize(tr.s)}
            </span>
            <Tip label={`${formatEtMillis(tr.t)} · ${d.label} · exchange ${tr.exchange} · seq ${tr.seq}${tr.conditions.length ? ` · conditions ${tr.conditions.join(" ")}` : ""}`} side="left">
              <span role="cell" className={cn("truncate font-sans no-underline", NOTE[d.tone], "text-right text-[10px]")}>
                {/* Only exceptions carry a note; a print counted in full is the normal case. */}
                {d.tone === "counted" ? "" : d.short}
              </span>
            </Tip>
          </div>
        );
      })}
    </div>
  );
}

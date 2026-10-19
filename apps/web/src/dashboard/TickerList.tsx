import { formatPrice, formatVolume } from "@stockwatcher/chart-core";
import { mostActiveTickers, tickerUniverse, type QuoteChannel, type TickerInfo } from "@stockwatcher/shared";
import { cn } from "@stockwatcher/chart-web/ui";
import { memo, useMemo } from "react";
import { useQuoteBoard } from "../feed/useQuoteBoard.ts";
import { useTickerPress } from "../lib/useTickerPress.ts";

/** One row: tap loads it into the selected chart; tap, then hold copies the ticker. */
const Row = memo(function Row({ row, active, onPick }: { row: TickerInfo; active: boolean; onPick: (symbol: string) => void }) {
  const press = useTickerPress(row.symbol, () => onPick(row.symbol));
  const down = (row.changePct ?? 0) < 0;
  return (
    <li>
      <button
        type="button"
        {...press}
        aria-current={active ? "true" : undefined}
        aria-label={`${row.symbol}, load into the selected chart`}
        className={cn(
          "grid w-full touch-manipulation grid-cols-[minmax(0,1fr)_auto_auto] items-baseline gap-x-3 rounded-lg px-1.5 py-1 text-left font-mono text-xs tabular-nums outline-none select-none transition-colors duration-150 focus-visible:ring-2 focus-visible:ring-ring",
          active ? "bg-bg-raised text-foreground" : "text-muted-foreground hover:bg-bg-row hover:text-foreground",
        )}
      >
        <span className="truncate font-sans text-[13px] font-semibold text-foreground">{row.symbol}</span>
        <span className="text-right">{row.price !== undefined ? formatPrice(row.price) : "—"}</span>
        <span className={cn("w-14 text-right", down ? "text-critical" : "text-good")}>{row.changePct !== undefined ? `${down ? "" : "+"}${row.changePct.toFixed(2)}%` : ""}</span>
        <span className="col-span-3 -mt-0.5 truncate text-[10px] text-text-faint">
          {row.name !== row.symbol ? `${row.name} · ` : ""}Vol {formatVolume(row.volume ?? 0)}
        </span>
      </button>
    </li>
  );
});

/** Rows shown: the most active of the price-band candidates (thousands on a real day). */
const LIMIT = 60;

/**
 * The dashboard's live ticker list (owner, 2026-10-18): the most active price-band stocks with
 * their live price, change % (against the previous close when known) and volume, from the
 * server's quote board, pushed at most once a second. Clicking a row loads it into the selected
 * chart. Subscribed only while the dashboard is open.
 */
export function TickerList({ quotes, symbols, selectedSymbol, selectedIndex, onPick }: { quotes: QuoteChannel | undefined; symbols: readonly string[]; selectedSymbol: string; selectedIndex: number; onPick: (symbol: string) => void }) {
  const board = useQuoteBoard(quotes, true);
  const rows = useMemo(() => mostActiveTickers(tickerUniverse(symbols, board), LIMIT), [symbols, board]);
  return (
    <section aria-label="Most active tickers" className="flex min-h-0 flex-col gap-1 rounded-xl bg-card p-2">
      <div className="flex items-baseline justify-between gap-2 px-1.5 pt-0.5">
        <h2 className="m-0 text-xs font-medium text-muted-foreground">Most active</h2>
        <span className="text-[11px] text-text-faint">Loads into chart {selectedIndex + 1}</span>
      </div>
      {rows.length === 0 ? (
        <p className="m-0 px-1.5 py-3 text-xs text-text-faint">{quotes ? "Waiting for live quotes…" : "Live quotes appear when connected to the server."}</p>
      ) : (
        <ul className="m-0 flex min-h-0 list-none flex-col gap-px overflow-y-auto p-0">
          {rows.map((r) => (
            <Row key={r.symbol} row={r} active={r.symbol === selectedSymbol} onPick={onPick} />
          ))}
        </ul>
      )}
    </section>
  );
}

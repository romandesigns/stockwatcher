import { formatEtTime, formatPrice } from "@stockwatcher/chart-core";
import { cn, ScrollArea } from "@stockwatcher/chart-web/ui";
import { formatChangePct, formatDollarVolume, moverRowMarks, rocketNow, runnersBySymbol, type MoverRow, type RocketState, type RowMarks } from "@stockwatcher/shared";
import { memo, useMemo } from "react";
import { StarButton } from "../favorites/StarButton.tsx";
import { useTickerPress } from "../lib/useTickerPress.ts";
import { Flame, MarksLine } from "./RowMarks.tsx";

const hhmm = (ms: number) => formatEtTime(ms);

export type MoversKind = "gainers" | "hot";

const TITLE: Record<MoversKind, string> = { gainers: "Top Gainers", hot: "Hot stocks" };
/** What each list ranks by, under its title (the definitions live in shared tickers/movers.ts). */
const CAPTION: Record<MoversKind, string> = { gainers: "% today · $250K+ traded", hot: "$ volume today" };

/**
 * Top Gainers and Hot stocks (owner, 2026-10-30), ranked from the server's quote board (shared
 * `topGainers` / `hotStocks`): the day's biggest % gainers with real money behind them, and the
 * most traded names by dollar volume, both $0.10–$25 with at least 100 prints, 20 rows each.
 *
 * One compact row per name, in the other panels' row language: ticker, price and day change as
 * text, the dollar volume on the right; the flame and the Runners path when today's Runners row
 * adds them (moverRowMarks). The ranked column (change for Gainers, dollars for Hot) is the one
 * in full ink. Pressing a row loads it into the selected chart (tap, then hold copies it).
 * Hidden at 0 rows like every panel (DashboardPanels); it takes its rows, so tests need no socket.
 */
export function MoversPanelView({ kind, rows, runners, wallNow, onPick }: { kind: MoversKind; rows: readonly MoverRow[]; runners?: RocketState; wallNow: number; onPick: (symbol: string) => void }) {
  const id = `movers-${kind}-title`;
  const nowMs = runners ? rocketNow(runners, wallNow) : wallNow;
  const marks = useMemo(() => {
    const byRunner = runnersBySymbol(runners);
    return new Map(rows.map((r) => [r.symbol, moverRowMarks(r.price, byRunner.get(r.symbol), nowMs, hhmm)]));
  }, [rows, runners, nowMs]);
  return (
    <aside aria-labelledby={id} data-panel={kind} className="flex h-full min-h-0 min-w-0 flex-col gap-2 rounded-xl bg-card p-2.5">
      <div className="flex items-baseline gap-2 px-1">
        <h2 id={id} className="panel-title">
          {TITLE[kind]}
        </h2>
        <span className="truncate font-mono text-[11px] text-text-faint">{CAPTION[kind]}</span>
        <span className="ml-auto font-mono text-[11px] text-text-faint tabular-nums">{rows.length}</span>
      </div>
      <ScrollArea>
        <ol className="m-0 flex list-none flex-col gap-1 p-0">
          {rows.map((r) => (
            <li key={r.symbol} className="relative">
              <MoverRowButton row={r} kind={kind} marks={marks.get(r.symbol) ?? null} onPick={onPick} />
              {/* Beside the row, not in it (a button cannot hold a button). */}
              <StarButton symbol={r.symbol} className="absolute top-1/2 right-1 -translate-y-1/2" />
            </li>
          ))}
        </ol>
      </ScrollArea>
    </aside>
  );
}

const MoverRowButton = memo(function MoverRowButton({ row: r, kind, marks: m, onPick }: { row: MoverRow; kind: MoversKind; marks: RowMarks | null; onPick: (symbol: string) => void }) {
  const press = useTickerPress(r.symbol, () => onPick(r.symbol));
  const change = formatChangePct(r.changePct);
  const dollars = formatDollarVolume(r.dollarVolume);
  const price = formatPrice(r.price);
  const up = r.changePct > 0;
  const down = r.changePct < 0;
  return (
    <button
      type="button"
      {...press}
      aria-label={`${r.symbol} ${price}, ${change} today, ${dollars} traded. Load into the selected chart`}
      data-flame={m?.flame?.lit || undefined}
      className={cn(
        "flex w-full cursor-pointer touch-manipulation flex-col justify-center gap-1 rounded-lg bg-bg-row py-1.5 pr-9 pl-2.5 text-left font-mono outline-none select-none transition-colors duration-150 hover:bg-bg-raised focus-visible:ring-2 focus-visible:ring-ring pointer-coarse:min-h-11 motion-reduce:transition-none",
        m?.flame?.lit && "bg-bg-raised/60",
      )}
    >
      <span className="flex min-w-0 items-center gap-1.5">
        <Flame flame={m?.flame ?? null} />
        <span className="text-sm font-semibold">{r.symbol}</span>
        <span className="text-sm tabular-nums">{price}</span>
        <span className={cn("text-xs tabular-nums", up ? "text-good" : down ? "text-critical" : "text-muted-foreground", kind === "gainers" && "font-semibold")}>{change}</span>
        <span className={cn("ml-auto shrink-0 text-xs tabular-nums", kind === "hot" ? "text-foreground" : "text-muted-foreground")}>{dollars}</span>
      </span>
      {m && <MarksLine marks={m} />}
    </button>
  );
});

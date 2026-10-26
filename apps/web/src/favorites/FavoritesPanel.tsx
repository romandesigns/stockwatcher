import { formatEtTime } from "@stockwatcher/chart-core";
import { Icon } from "@stockwatcher/chart-web";
import { cn, ScrollArea, Tip, TOUCH_HIT } from "@stockwatcher/chart-web/ui";
import { favoriteRows, rossLinePaths, type FavoriteRowView, type FavoritesState, type RossLineView, type RossTone } from "@stockwatcher/shared";
import { memo, useMemo, type KeyboardEvent } from "react";
import { useTickerPress } from "../lib/useTickerPress.ts";
import { getAppStore } from "../state/store.ts";
import { StarButton } from "./StarButton.tsx";

const TONE: Record<RossTone, string> = {
  good: "text-good",
  critical: "text-critical",
  warning: "text-warning",
  accent: "text-primary",
  text: "text-foreground",
  muted: "text-muted-foreground",
  dim: "text-text-faint",
};

const hhmm = (ms: number) => formatEtTime(ms);

/**
 * The Favorites panel (owner, 2026-10-26: "a favorite panel where I can add stocks to keep an eye
 * on ... a live view of the chart price action and any other relevant information so I can be
 * fully informed before clicking on it"). One row per favorite in the owner's order: price and
 * day change, the day's line with the previous close as a dotted baseline, volume, RVOL, distance
 * from the high, the premarket high, and the context the server already has (today's Runners
 * entry, the pullback trigger and stop, the pillars and news, a halt). A row with no print for
 * five minutes says so and dims. Pressing a row loads its ticker into the selected chart (tap,
 * then hold copies it, as on every panel). The star takes it off; the arrows (or Alt+↑/↓ on the
 * row) move it. The list is synced, so another device's change lands here live.
 */
export function FavoritesPanelView({ list, state, onPick }: { list: readonly string[]; state: FavoritesState; onPick: (symbol: string) => void }) {
  const rows = useMemo(() => favoriteRows(list, state, hhmm).map((r) => r.view), [list, state]);
  const reconnecting = !state.connected && state.rows !== null;
  return (
    <aside aria-labelledby="favorites-title" className="@container/fav flex min-h-0 min-w-0 flex-col gap-2 rounded-xl bg-card p-2.5">
      <div className="flex items-center gap-2 px-1">
        <h2 id="favorites-title" className="m-0 text-sm font-semibold">
          Favorites
        </h2>
        <span className="font-mono text-[11px] text-text-faint">{list.length}</span>
      </div>
      {reconnecting && (
        <p className="m-0 flex items-center gap-1.5 px-1 text-[11px] text-warning" role="status">
          <span className="size-1.5 rounded-full bg-warning" aria-hidden />
          Reconnecting · last known rows
        </p>
      )}
      <ScrollArea className={cn("transition-opacity duration-200 motion-reduce:transition-none", reconnecting && "opacity-60")}>
        <ul className="m-0 flex list-none flex-col gap-1 p-0">
          {list.map((symbol, i) => (
            <li key={symbol} className="group relative">
              <FavoriteRow symbol={symbol} row={rows[i] ?? null} onPick={onPick} />
              <RowControls symbol={symbol} first={i === 0} last={i === list.length - 1} />
            </li>
          ))}
        </ul>
      </ScrollArea>
    </aside>
  );
}

const move = (symbol: string, delta: number) => getAppStore().getState().moveFavorite(symbol, delta);

/** Move up, move down (shown on hover or focus with a mouse, always on touch) and the star. */
function RowControls({ symbol, first, last }: { symbol: string; first: boolean; last: boolean }) {
  const reveal = "opacity-0 group-hover:opacity-100 group-focus-within:opacity-100 pointer-coarse:opacity-100 transition-opacity duration-150 motion-reduce:transition-none";
  return (
    <div className="absolute top-1.5 right-1.5 flex items-center gap-0.5">
      <MoveButton label={`Move ${symbol} up`} icon="chevronUp" disabled={first} onClick={() => move(symbol, -1)} className={reveal} />
      <MoveButton label={`Move ${symbol} down`} icon="chevronDown" disabled={last} onClick={() => move(symbol, 1)} className={reveal} />
      <StarButton symbol={symbol} quiet />
    </div>
  );
}

function MoveButton({ label, icon, disabled, onClick, className }: { label: string; icon: "chevronUp" | "chevronDown"; disabled: boolean; onClick: () => void; className?: string }) {
  return (
    <Tip label={label}>
      <button
        type="button"
        aria-label={label}
        disabled={disabled}
        onClick={onClick}
        className={cn(
          "relative inline-grid size-7 place-items-center rounded-md text-text-faint outline-none hover:bg-bg-raised hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring disabled:pointer-events-none disabled:opacity-0 [&_svg]:size-3",
          TOUCH_HIT,
          className,
        )}
      >
        <Icon name={icon} />
      </button>
    </Tip>
  );
}

const FavoriteRow = memo(function FavoriteRow({ symbol, row: r, onPick }: { symbol: string; row: FavoriteRowView | null; onPick: (symbol: string) => void }) {
  const press = useTickerPress(symbol, () => onPick(symbol));
  const onKeyDown = (e: KeyboardEvent) => {
    if (!e.altKey || (e.key !== "ArrowUp" && e.key !== "ArrowDown")) return;
    e.preventDefault();
    move(symbol, e.key === "ArrowUp" ? -1 : 1);
  };
  const facts = r ? [r.volume && `${r.volume} vol`, r.rvol, r.fromHigh, r.pmHigh].filter(Boolean) : [];
  return (
    <button
      type="button"
      {...press}
      onKeyDown={onKeyDown}
      aria-label={`${r?.aria || symbol}. Load into the selected chart. Alt and arrow keys move it`}
      className={cn(
        "flex w-full cursor-pointer touch-manipulation flex-col gap-1 rounded-lg bg-bg-row py-2 pr-2.5 pl-2.5 text-left font-mono outline-none select-none transition-[background-color,opacity] duration-150 hover:bg-bg-raised focus-visible:ring-2 focus-visible:ring-ring motion-reduce:transition-none",
        r?.stale && "opacity-60",
      )}
    >
      <span className="flex items-baseline gap-1.5 pr-22">
        <span className="text-sm font-semibold">{symbol}</span>
        {r && !r.empty && <span className="text-[13px] tabular-nums">{r.price}</span>}
        {r?.change && <span className={cn("text-xs tabular-nums", TONE[r.changeTone])}>{r.change}</span>}
        {r?.halted && <span className="self-center rounded bg-warning px-1 py-px font-sans text-[9px] font-semibold text-background">HALTED</span>}
      </span>
      {!r ? (
        <span className="font-sans text-xs text-text-faint">Loading…</span>
      ) : r.empty ? (
        <span className="font-sans text-xs text-text-faint">No trades today yet</span>
      ) : (
        <span className="grid grid-cols-[minmax(0,1fr)_88px] items-center gap-2 @sm/fav:grid-cols-[minmax(0,1fr)_132px]">
          <span className="flex min-w-0 flex-col gap-1">
            {facts.length > 0 && (
              <span className="flex flex-wrap gap-x-2 text-[11px] text-muted-foreground tabular-nums">
                {facts.map((f) => (
                  <span key={f as string} className={cn("whitespace-nowrap", f === r.fromHigh && TONE[r.fromHighTone])}>
                    {f}
                  </span>
                ))}
              </span>
            )}
            {r.chips.filter((c) => c.label !== "Halted").length > 0 && (
              <span className="flex flex-wrap gap-1 font-sans">
                {r.chips
                  .filter((c) => c.label !== "Halted")
                  .map((c) => (
                    <span key={c.label} className={cn("rounded bg-bg-raised px-1 py-px text-[10px] font-medium whitespace-nowrap", TONE[c.tone])}>
                      {c.label}
                    </span>
                  ))}
              </span>
            )}
            {r.stale && <span className="font-sans text-[11px] text-text-faint">{r.stale}</span>}
          </span>
          <DayLine line={r.line} />
        </span>
      )}
    </button>
  );
});

/**
 * The row's chart: the day's 30 s closes from 04:00 ET ending at the live price, the previous
 * close dotted when it is near enough to draw (the Ross row chart, `rossLinePaths`), stretched to
 * the column's width with strokes that keep their weight.
 */
function DayLine({ line }: { line: RossLineView | null }) {
  if (!line) return <span className="h-9 w-full" aria-hidden />;
  const p = rossLinePaths(line, 132, 36);
  return (
    <svg viewBox="0 0 132 36" preserveAspectRatio="none" className={cn("h-9 w-full animate-line-in", line.tone === "good" ? "text-good" : "text-critical")} aria-hidden>
      {p.baseY !== null && <line x1={0} x2={132} y1={p.baseY} y2={p.baseY} stroke="currentColor" strokeOpacity={0.35} strokeWidth={1} strokeDasharray="1.5 2.5" vectorEffect="non-scaling-stroke" />}
      <path d={p.area} fill="currentColor" fillOpacity={0.12} />
      <path d={p.line} fill="none" stroke="currentColor" strokeWidth={1.5} strokeLinejoin="round" vectorEffect="non-scaling-stroke" />
    </svg>
  );
}

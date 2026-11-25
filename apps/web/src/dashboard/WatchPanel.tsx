import { Icon } from "@stockwatcher/chart-web";
import { cn, ScrollArea } from "@stockwatcher/chart-web/ui";
import { WATCH_BASE_RATE, WATCH_NO_FLOAT, watchCardView, type PrewatchState, type WatchCardWire } from "@stockwatcher/shared";
import { memo } from "react";
import { useTickerPress } from "../lib/useTickerPress.ts";
import { Reveal } from "./Reveal.tsx";

/**
 * The pre-open Watch card (owner, 2026-11-25, after AKAN 11-24): 04:00–09:30 ET (cleared 09:45),
 * stocks that match the AKAN profile (server crates/server/src/prewatch, shared prewatch.ts), each
 * with why: the gap, the pre-market volume as a share of the float, the float and the recent +50 %
 * day. **Quiet**: it never notifies, rings or toasts, and spends no accent (a list to read before
 * the bell, not a call to act); a condition that stopped holding dims the card and names it. The
 * footer says the profile's honest base rate. Laid out by its own width (`@container/watch`): one
 * column, then two. Pressing a card loads its ticker into the selected chart.
 */
export function WatchPanelView({ state, onPick }: { state: PrewatchState; onPick: (symbol: string) => void }) {
  const reconnecting = !state.connected && state.cards.length > 0;
  const live = state.cards.filter((c) => c.fails.length === 0).length;
  return (
    <aside aria-labelledby="watch-title" data-panel="watch" className="@container/watch flex h-full min-h-0 min-w-0 flex-col gap-2 rounded-xl bg-card p-2.5">
      <div className="flex min-w-0 items-center gap-2 px-1">
        <Icon name="gauge" className="size-4 shrink-0 text-muted-foreground" />
        <h2 id="watch-title" className="panel-title shrink-0">
          Pre-open watch
        </h2>
        <span className="truncate font-mono text-[11px] text-text-faint">AKAN-like · quiet</span>
        <span className="ml-auto font-mono text-[11px] text-text-faint tabular-nums" aria-label={`${live} matching`}>
          {live}
        </span>
      </div>
      {reconnecting && (
        <p className="m-0 flex items-center gap-1.5 px-1 text-[11px] text-warning" role="status">
          <span className="size-1.5 rounded-full bg-warning" aria-hidden />
          Reconnecting · last known state
        </p>
      )}
      {state.floatSource === "none" && <p className="m-0 px-1 text-[11px] text-text-faint">{WATCH_NO_FLOAT}</p>}
      <ScrollArea className={cn("min-h-0 flex-1 transition-opacity duration-200", reconnecting && "opacity-60")} viewportClassName="h-full">
        <ul aria-label="Pre-open watch" className="m-0 grid list-none grid-cols-1 content-start gap-1.5 p-0 @[34rem]/watch:grid-cols-2 @[34rem]/watch:[&>li:nth-child(odd):last-child]:col-span-2">
          {state.cards.map((c) => (
            <li key={c.symbol} className="min-w-0">
              <Reveal show className="h-full" innerClassName="h-full">
                <WatchCard card={c} onPick={onPick} />
              </Reveal>
            </li>
          ))}
        </ul>
      </ScrollArea>
      <p className="m-0 px-1 text-[11px] leading-snug text-text-faint">{WATCH_BASE_RATE}</p>
    </aside>
  );
}

const WatchCard = memo(function WatchCard({ card, onPick }: { card: WatchCardWire; onPick: (symbol: string) => void }) {
  const v = watchCardView(card);
  const press = useTickerPress(card.symbol, () => onPick(card.symbol));
  return (
    <button
      type="button"
      {...press}
      data-watch-card={card.symbol}
      aria-label={`${v.symbol} ${v.price}, ${v.reasons.map((r) => `${r.label} ${r.value}`).join(", ")}${v.faded ? `. ${v.faded}` : ""}. Load into the selected chart`}
      className={cn(
        "animate-tape-in flex h-full w-full min-w-0 cursor-pointer touch-manipulation flex-col gap-1.5 rounded-lg bg-bg-row px-2.5 py-2 text-left outline-none select-none transition-[opacity,background-color] duration-300 hover:bg-bg-raised focus-visible:ring-2 focus-visible:ring-ring pointer-coarse:min-h-11",
        v.faded && "opacity-60",
      )}
    >
      <span className="flex min-w-0 items-baseline gap-1.5 font-mono">
        <span className="shrink-0 text-sm font-semibold text-foreground">{v.symbol}</span>
        <span className="shrink-0 text-[13px] text-foreground tabular-nums">{v.price}</span>
        <span className="shrink-0 text-xs text-muted-foreground tabular-nums">{v.gap}</span>
        <span className="ml-auto truncate text-[11px] text-text-faint">{v.since}</span>
      </span>
      <dl className="m-0 grid min-w-0 grid-cols-[auto_1fr] gap-x-2 gap-y-0.5 text-[11px]">
        {v.reasons.map((r) => (
          <div key={r.label} className="contents">
            <dt className="text-text-faint">{r.label}</dt>
            <dd className="m-0 truncate font-mono text-muted-foreground tabular-nums">{r.value}</dd>
          </div>
        ))}
      </dl>
      {v.faded && <span className="text-[11px] text-muted-foreground">{v.faded}</span>}
    </button>
  );
});

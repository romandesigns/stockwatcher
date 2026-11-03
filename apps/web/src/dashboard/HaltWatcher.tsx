import { formatEtTime, formatPrice } from "@stockwatcher/chart-core";
import { Icon } from "@stockwatcher/chart-web";
import { cn, ScrollArea, Tip } from "@stockwatcher/chart-web/ui";
import { bandLine, bandLineShort, buildupCue, buildupWithEta, haltLine, haltSentence, pressureFreshnessLine, riskPhrase, type PressureCard, type PressureStatus } from "@stockwatcher/shared";
import { memo } from "react";
import { useTickerPress } from "../lib/useTickerPress.ts";
import { PressureGauge } from "./PressureGauge.tsx";

const hhmm = (ms: number) => formatEtTime(ms);
/** "$3.63": the band is an order price, so it reads as one. */
const dollars = (p: number) => `$${formatPrice(p)}`;

function changeText(pct: number): string {
  return `${pct >= 0 ? "+" : ""}${pct.toFixed(2)}%`;
}

/**
 * The dashboard's Halt watcher (owner, 2026-10-19; design canvas "Halt Watcher"; build-ups since
 * 2026-10-20: "observe for a rapid increasing amounting pressure that could potentially lead to a
 * halt", and no card when none is happening). The server's board (crates/server/src/pressure.rs):
 * only symbols whose one-sided pressure is rising fast while the price runs at its LULD band, at
 * most 6, usually none, so the panel is usually hidden (DashboardPanels: 0 items, no panel). It
 * reads the dashboard's own market (live by default, Settings → Data).
 *
 * A panel in the dashboard's bottom row since 2026-10-30 (it was a strip above the charts): one
 * compact row per build-up, the gauge filled with the halt risk on the left, then the ticker,
 * price and change, under them the cue "+45 in 30s · 1.2% to LULD up", and under that the band and
 * the time to it at this pace, "Upper band $3.63 · ~2 min at this pace" (owner, 2026-11-03: time
 * to place an order ahead of the halt; "↑ $3.63 · ~2 min" on a narrow row). Laid out by the
 * panel's own width (container queries): one column, two once the panel is wide enough for two
 * rows side by side. It scrolls inside the panel. Pressing a row loads its ticker into the
 * selected chart (tap, then hold copies it). A halted row keeps its frozen risk on a grey arc and
 * says "Halted" in amber.
 */
export function HaltWatcherView({ board, status, onPick }: { board: readonly PressureCard[]; status: PressureStatus; onPick: (symbol: string) => void }) {
  const freshness = pressureFreshnessLine(status, hhmm);
  return (
    <aside aria-labelledby="halt-watcher-title" data-panel="halt" className="@container/hw flex h-full min-h-0 min-w-0 flex-col gap-2 rounded-xl bg-card p-2.5">
      <div className="flex min-w-0 items-center gap-2 px-1">
        <Icon name="gauge" className="size-4 shrink-0 text-muted-foreground" />
        <h2 id="halt-watcher-title" className="panel-title shrink-0">
          Halt watcher
        </h2>
        <span role={freshness ? "status" : undefined} className={cn("truncate text-[11px]", freshness ? "text-warning" : "text-text-faint")}>{freshness || "Pressure toward a LULD band"}</span>
      </div>
      <ScrollArea className={cn("transition-opacity duration-200 motion-reduce:transition-none", !status.connected && "opacity-60")}>
        <ul className="m-0 grid list-none grid-cols-1 gap-1 p-0 @lg/hw:grid-cols-2">
          {board.map((c) => (
            <li key={c.symbol} className="@container/hwrow min-w-0">
              <GaugeRow card={c} onPick={onPick} stale={!status.connected} freshness={freshness} />
            </li>
          ))}
        </ul>
      </ScrollArea>
    </aside>
  );
}

const GaugeRow = memo(function GaugeRow({ card, onPick, stale, freshness }: { card: PressureCard; onPick: (symbol: string) => void; stale: boolean; freshness: string }) {
  const press = useTickerPress(card.symbol, () => onPick(card.symbol));
  const halted = card.halt !== undefined;
  const right = card.halt ? haltLine(card.halt, hhmm) : changeText(card.changePct);
  const phrase = riskPhrase(card);
  // A stale card's countdown would count from an old pace: the band only.
  const band = stale ? { ...card, toBandSecs: undefined } : card;
  const buildup = buildupWithEta(band, formatPrice);
  // The flow behind it: the side's share of the minute's volume by the tick rule, and its pace.
  const share = Math.round((card.side === "bear" ? 1 - card.buyShare : card.buyShare) * 100);
  const flow = `pressure ${Math.round(card.pressure)}: ${share}% ${card.side === "bear" ? "sold on downticks" : "bought on upticks"} in the last minute, ${card.pace.toFixed(1)}× its pace this session`;
  return (
    <Tip label={stale ? `${freshness} · last known: ${buildup}` : card.halt ? `${haltSentence(card.halt, hhmm)} · as of the halt: ${buildup}` : `${buildup} · ${flow}`}>
      <button
        type="button"
        {...press}
        aria-label={`${stale ? `${freshness}. Last known: ` : ""}${card.symbol} ${formatPrice(card.price)}, ${card.halt ? haltSentence(card.halt, hhmm) : `${changeText(card.changePct)} today`}, ${phrase}, ${buildup}. Load into the selected chart`}
        className="flex w-full cursor-pointer touch-manipulation items-center gap-2.5 rounded-lg bg-bg-row px-2 py-1.5 text-left outline-none select-none transition-colors duration-150 ease-out hover:bg-bg-raised focus-visible:ring-2 focus-visible:ring-ring pointer-coarse:min-h-11 motion-reduce:transition-none"
      >
        <div className="w-14 shrink-0">
          <PressureGauge compact value={card.risk} side={card.side} halted={halted} stale={stale} label={`${stale ? `${freshness}. Last known: ` : halted ? "Halted. " : ""}${phrase}`} />
        </div>
        <div className="flex min-w-0 flex-1 flex-col gap-0.5 font-mono">
          {/* The ticker never truncates; on a narrow row the price gives way (it is in the tooltip
              and the accessible name). */}
          <div className="flex min-w-0 items-baseline gap-1.5">
            <span className="shrink-0 text-sm font-semibold">{card.symbol}</span>
            <span className="hidden text-sm tabular-nums @[15rem]/hwrow:inline">{formatPrice(card.price)}</span>
            <span className={cn("ml-auto text-xs whitespace-nowrap tabular-nums", stale ? "text-text-faint" : halted ? "text-warning" : card.changePct < 0 ? "text-critical" : "text-good")}>{right}</span>
          </div>
          <span className="truncate text-[11px] text-muted-foreground tabular-nums">{stale ? "Last known" : buildupCue(card)}</span>
          {/* What to act on: the band price and how soon the price gets there (no time when not
              moving toward it, halted or stale). */}
          <span className={cn("truncate text-xs tabular-nums transition-colors duration-200 motion-reduce:transition-none", stale ? "text-text-faint" : "text-foreground")}>
            <span className="@[15rem]/hwrow:hidden">{bandLineShort(band, dollars)}</span>
            <span className="hidden @[15rem]/hwrow:inline">{bandLine(band, dollars)}</span>
          </span>
        </div>
      </button>
    </Tip>
  );
});

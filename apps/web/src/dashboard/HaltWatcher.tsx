import { formatEtTime, formatPrice } from "@stockwatcher/chart-core";
import { Icon } from "@stockwatcher/chart-web";
import { cn, Tip } from "@stockwatcher/chart-web/ui";
import { haltLine, haltSentence, pressurePhrase, type PressureCard, type PressureChannel } from "@stockwatcher/shared";
import { memo } from "react";
import { usePressureBoard } from "../feed/usePressureBoard.ts";
import { useTickerPress } from "../lib/useTickerPress.ts";
import { PressureGauge } from "./PressureGauge.tsx";

const hhmm = (ms: number) => formatEtTime(ms);

function changeText(pct: number): string {
  return `${pct >= 0 ? "+" : ""}${pct.toFixed(2)}%`;
}

/**
 * The dashboard's Halt watcher (owner, 2026-10-19; design canvas "Halt Watcher"): the server's
 * top 6 symbols by one-sided pressure in the last minute (crates/server/src/pressure.rs), each a
 * card with the gauge on the left and the ticker, price and change on the right. Pressing a card
 * loads its ticker into the selected chart, like a pick from the search (tap, then hold copies
 * it). It reads the dashboard's own market (live by default, Settings → Data).
 *
 * Laid out by the panel's own width (container queries): three columns wide, two narrower. On a
 * phone all six sit in ONE row of mini gauges, no scrolling (owner, 2026-10-19: "the gauge UI
 * should be smaller, fit them all on a single row"): the dial with its reading, the ticker and the
 * change, the price left to the tooltip and the accessible name. A halted card keeps its frozen
 * pressure on a grey arc and says "Halted" in amber where the change % was.
 */
export function HaltWatcher({ channel, onPick }: { channel: PressureChannel; onPick: (symbol: string) => void }) {
  const board = usePressureBoard(channel);
  return (
    <section aria-labelledby="halt-watcher-title" className="@container/hw flex shrink-0 flex-col gap-2">
      <div className="flex flex-wrap items-center gap-2">
        <Icon name="gauge" className="size-4 text-muted-foreground" />
        <h2 id="halt-watcher-title" className="m-0 text-sm font-semibold">
          Halt watcher
        </h2>
        <span className="text-xs text-text-faint">Top 6 by pressure</span>
      </div>
      {board.length === 0 ? (
        <p className="m-0 text-xs text-text-faint">No stock in the price band has traded enough in the last minute to measure.</p>
      ) : (
        <ul className="m-0 grid list-none grid-cols-6 gap-1.5 p-0 @xl/hw:grid-cols-2 @xl/hw:gap-2 @4xl/hw:grid-cols-3">
          {board.map((c) => (
            <li key={c.symbol} className="min-w-0">
              <GaugeCard card={c} onPick={onPick} />
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}

const GaugeCard = memo(function GaugeCard({ card, onPick }: { card: PressureCard; onPick: (symbol: string) => void }) {
  const press = useTickerPress(card.symbol, () => onPick(card.symbol));
  const halted = card.halt !== undefined;
  const right = card.halt ? haltLine(card.halt, hhmm) : changeText(card.changePct);
  const phrase = pressurePhrase(card);
  // The side's share of the minute's volume, by the tick rule, and how heavy the minute was.
  const share = Math.round((card.side === "bear" ? 1 - card.buyShare : card.buyShare) * 100);
  const detail = `${share}% ${card.side === "bear" ? "sold on downticks" : "bought on upticks"} in the last minute · ${card.pace.toFixed(1)}× its pace so far this session`;
  return (
    <Tip label={card.halt ? `${haltSentence(card.halt, hhmm)} · pressure as of the halt` : detail}>
      <button
        type="button"
        {...press}
        aria-label={`${card.symbol} ${formatPrice(card.price)}, ${card.halt ? haltSentence(card.halt, hhmm) : `${changeText(card.changePct)} today`}, ${phrase}. Load into the selected chart`}
        className="flex w-full cursor-pointer touch-manipulation flex-col items-center gap-0.5 rounded-lg bg-card px-0.5 pt-1.5 pb-1 text-left outline-none select-none transition-colors duration-150 ease-out hover:bg-bg-row focus-visible:ring-2 focus-visible:ring-ring motion-reduce:transition-none @xl/hw:grid @xl/hw:grid-cols-[minmax(0,46%)_minmax(0,1fr)] @xl/hw:items-center @xl/hw:gap-3.5 @xl/hw:rounded-xl @xl/hw:px-4 @xl/hw:py-2.5"
      >
        <div className="w-full @xl/hw:max-w-40">
          <PressureGauge pressure={card.pressure} side={card.side} halted={halted} label={`${halted ? "Halted. " : ""}${phrase}`} />
        </div>
        {/* Phone: the mini cell's ticker and change under the dial (one row of six). */}
        <div className="flex w-full min-w-0 flex-col items-center font-mono @xl/hw:hidden">
          <span className="max-w-full truncate text-[11px] font-semibold">{card.symbol}</span>
          <span className={cn("max-w-full truncate text-[10px] tabular-nums", halted ? "text-warning" : card.changePct < 0 ? "text-critical" : "text-good")}>{card.halt ? "Halted" : changeText(card.changePct)}</span>
        </div>
        <div className="hidden min-w-0 flex-col gap-1 font-mono @xl/hw:flex">
          <div className="flex min-w-0 items-baseline gap-2">
            <span className="truncate text-base font-semibold">{card.symbol}</span>
            <span className={cn("ml-auto text-xs whitespace-nowrap tabular-nums", halted ? "text-warning" : card.changePct < 0 ? "text-critical" : "text-good")}>{right}</span>
          </div>
          <span className="text-xl tabular-nums">{formatPrice(card.price)}</span>
        </div>
      </button>
    </Tip>
  );
});

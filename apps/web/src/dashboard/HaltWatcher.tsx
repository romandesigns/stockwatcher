import { formatEtTime, formatPrice } from "@stockwatcher/chart-core";
import { Icon } from "@stockwatcher/chart-web";
import { cn, Tip } from "@stockwatcher/chart-web/ui";
import { buildupCue, buildupCueShort, buildupSentence, haltLine, haltSentence, pressureFreshnessLine, riskPhrase, type PressureCard, type PressureChannel } from "@stockwatcher/shared";
import { memo } from "react";
import { usePressureBoard, usePressureStatus } from "../feed/usePressureBoard.ts";
import { useTickerPress } from "../lib/useTickerPress.ts";
import { PressureGauge } from "./PressureGauge.tsx";
import { Reveal } from "./Reveal.tsx";

const hhmm = (ms: number) => formatEtTime(ms);

function changeText(pct: number): string {
  return `${pct >= 0 ? "+" : ""}${pct.toFixed(2)}%`;
}

/**
 * The dashboard's Halt watcher (owner, 2026-10-19; design canvas "Halt Watcher"; build-ups since
 * 2026-10-20: "observe for a rapid increasing amounting pressure that could potentially lead to a
 * halt", and no card when none is happening). The server's board (crates/server/src/pressure.rs):
 * only symbols whose one-sided pressure is rising fast while the price runs at its LULD band, at
 * most 6, usually none, so the strip is usually hidden (no card, no heading). Each card: the
 * gauge filled with the halt risk on the left; the ticker, price and change on the right, and
 * under the ticker the cue "+45 in 30s · 1.2% to LULD up". Pressing a card loads its ticker into
 * the selected chart, like a pick from the search (tap, then hold copies it). It reads the
 * dashboard's own market (live by default, Settings → Data).
 *
 * Laid out by the panel's own width (container queries): three columns wide, two narrower. On a
 * phone the cards sit in ONE row of mini gauges, no scrolling (owner, 2026-10-19): the dial with
 * its reading, the ticker and the short cue ("+45 · 1.2%↑"), the rest left to the tooltip and the
 * accessible name. A halted card keeps its frozen risk on a grey arc and says "Halted" in amber.
 */
export function HaltWatcher({ channel, onPick }: { channel: PressureChannel; onPick: (symbol: string) => void }) {
  const board = usePressureBoard(channel);
  const status = usePressureStatus(channel);
  const freshness = pressureFreshnessLine(status, hhmm);
  // No build-ups, no strip (owner, 2026-10-22: a panel with 0 items is hidden): it opens and
  // closes by its real height, so the charts below slide rather than jump. The negative margin
  // cancels the panel's gap-3 while closed; the padding restores it while open.
  return (
    <Reveal show={board.length > 0} className="-mb-3 shrink-0" innerClassName="pb-3">
      <section aria-labelledby="halt-watcher-title" className="@container/hw flex flex-col gap-2">
        <div className="flex flex-wrap items-center gap-2">
          <Icon name="gauge" className="size-4 text-muted-foreground" />
          <h2 id="halt-watcher-title" className="m-0 text-sm font-semibold">
            Halt watcher
          </h2>
          <span role={freshness ? "status" : undefined} className="text-xs text-text-faint">{freshness || "Pressure building toward a LULD band"}</span>
        </div>
        <ul className="m-0 grid list-none grid-cols-6 gap-1.5 p-0 @xl/hw:grid-cols-2 @xl/hw:gap-2 @4xl/hw:grid-cols-3">
          {board.map((c) => (
            <li key={c.symbol} className="min-w-0">
              <GaugeCard card={c} onPick={onPick} stale={!status.connected} freshness={freshness} />
            </li>
          ))}
        </ul>
      </section>
    </Reveal>
  );
}

const GaugeCard = memo(function GaugeCard({ card, onPick, stale, freshness }: { card: PressureCard; onPick: (symbol: string) => void; stale: boolean; freshness: string }) {
  const press = useTickerPress(card.symbol, () => onPick(card.symbol));
  const halted = card.halt !== undefined;
  const right = card.halt ? haltLine(card.halt, hhmm) : changeText(card.changePct);
  const phrase = riskPhrase(card);
  const buildup = buildupSentence(card, formatPrice);
  // The flow behind it: the side's share of the minute's volume by the tick rule, and its pace.
  const share = Math.round((card.side === "bear" ? 1 - card.buyShare : card.buyShare) * 100);
  const flow = `pressure ${Math.round(card.pressure)}: ${share}% ${card.side === "bear" ? "sold on downticks" : "bought on upticks"} in the last minute, ${card.pace.toFixed(1)}× its pace this session`;
  const tone = stale ? "text-text-faint" : halted ? "text-warning" : card.side === "bear" ? "text-critical" : "text-good";
  return (
    <Tip label={stale ? `${freshness} · last known: ${buildup}` : card.halt ? `${haltSentence(card.halt, hhmm)} · as of the halt: ${buildup}` : `${buildup} · ${flow}`}>
      <button
        type="button"
        {...press}
        aria-label={`${stale ? `${freshness}. Last known: ` : ""}${card.symbol} ${formatPrice(card.price)}, ${card.halt ? haltSentence(card.halt, hhmm) : `${changeText(card.changePct)} today`}, ${phrase}, ${buildup}. Load into the selected chart`}
        className="flex w-full cursor-pointer touch-manipulation flex-col items-center gap-0.5 rounded-lg bg-card px-0.5 pt-1.5 pb-1 text-left outline-none select-none transition-colors duration-150 ease-out hover:bg-bg-row focus-visible:ring-2 focus-visible:ring-ring motion-reduce:transition-none @xl/hw:grid @xl/hw:grid-cols-[minmax(0,46%)_minmax(0,1fr)] @xl/hw:items-center @xl/hw:gap-3.5 @xl/hw:rounded-xl @xl/hw:px-4 @xl/hw:py-2.5"
      >
        <div className="w-full @xl/hw:max-w-40">
          <PressureGauge value={card.risk} side={card.side} halted={halted} stale={stale} label={`${stale ? `${freshness}. Last known: ` : halted ? "Halted. " : ""}${phrase}`} />
        </div>
        {/* Phone: the mini cell's ticker and short cue under the dial (one row of six). */}
        <div className="flex w-full min-w-0 flex-col items-center font-mono @xl/hw:hidden">
          <span className="max-w-full truncate text-[11px] font-semibold">{card.symbol}</span>
          <span className={cn("max-w-full truncate text-[10px] tabular-nums", tone)}>{stale ? "Last known" : card.halt ? "Halted" : buildupCueShort(card)}</span>
        </div>
        <div className="hidden min-w-0 flex-col gap-1 font-mono @xl/hw:flex">
          <div className="flex min-w-0 items-baseline gap-2">
            <span className="truncate text-base font-semibold">{card.symbol}</span>
            <span className={cn("ml-auto text-xs whitespace-nowrap tabular-nums", stale ? "text-text-faint" : halted ? "text-warning" : card.changePct < 0 ? "text-critical" : "text-good")}>{right}</span>
          </div>
          <span className="truncate text-[11px] text-muted-foreground tabular-nums">{buildupCue(card)}</span>
          <span className="text-xl tabular-nums">{formatPrice(card.price)}</span>
        </div>
      </button>
    </Tip>
  );
});

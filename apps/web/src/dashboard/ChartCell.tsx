import type { DashboardSlot } from "@stockwatcher/app-state";
import { buildSlots, changeReference, formatChange, formatEtDateKey, formatPrice, lastCandle, tradingDayOf, type ChartOptions } from "@stockwatcher/chart-core";
import { CandleChart, type ChartTrading } from "@stockwatcher/chart-web";
import { cn, Tip } from "@stockwatcher/chart-web/ui";
import { TickerLabel } from "../lib/TickerLabel.tsx";
import { useEffect, useMemo, useState, useSyncExternalStore } from "react";
import type { ChartFeed } from "../feed/types.ts";
import { useChartFeed } from "../feed/useChartFeed.ts";
import { getAppStore, useApp } from "../state/store.ts";

/** A compact chart for a panel of up to four. */
const CELL_CHART: Partial<ChartOptions> = { fontSize: 10, timeLabelSpacing: 64 };

export interface ChartCellProps {
  index: number;
  slot: DashboardSlot;
  /** What this cell shows: the slot's ticker, or a candidate while the slot has none (never written back). */
  symbol: string;
  feed: ChartFeed;
  trading: ChartTrading | null;
  selected: boolean;
  /** Mark the selection (only meaningful with more than one chart). */
  showSelection: boolean;
  fixture: boolean;
  onSelect: (index: number) => void;
  /** Opens the ticker search for this chart. */
  onPickTicker: (index: number) => void;
  /** Reports the trading day this cell's clock is on (the candidates follow it). */
  onDay: (day: string | null) => void;
}

/**
 * One chart of the dashboard's panel, in its own container. It streams on its own slot of the
 * page's one socket (`d0`…`d3`; up to 4 per connection, crates/server/src/ws.rs) and is the same
 * CandleChart as the chart page, with the person's synced indicators, levels, settings and this
 * symbol's drawings. Pressing anywhere in it selects it: the selected chart is ringed with the
 * accent (a 1px inset box shadow on an overlay, never a border: the no-borders rule) and is where a picked ticker loads.
 */
export function ChartCell({ index, slot, symbol, feed, trading, selected, showSelection, fixture, onSelect, onPickTicker, onDay }: ChartCellProps) {
  const [app] = useState(() => getAppStore().getState());
  const bin = useApp((s) => s.bin);
  const { series, clock } = useChartFeed(feed, symbol, slot.timeframe, bin, `d${index}`);

  const indicators = useApp((s) => s.indicators);
  const layers = useApp((s) => s.layers);
  const chartSettings = useApp((s) => s.chartSettings);
  const fromRemote = useApp((s) => s.chartSettingsFromRemote);
  const drawingSettings = useApp((s) => s.drawingSettings);
  const tradeQty = useApp((s) => s.tradeQty);
  // This symbol's drawings, synced per drawing like the chart page's.
  const drawingsHandle = useMemo(() => getAppStore().drawingsFor(symbol), [symbol]);
  const drawings = useSyncExternalStore(drawingsHandle.subscribe, drawingsHandle.get);

  // The inspected candle belongs to this cell; a new ticker or timeframe drops it (adjusted while rendering).
  const [inspectT, setInspectT] = useState<number | null>(null);
  const viewKey = `${symbol}/${slot.timeframe}`;
  const [lastView, setLastView] = useState(viewKey);
  if (viewKey !== lastView) {
    setLastView(viewKey);
    setInspectT(null);
  }

  const dayKey = clock ? formatEtDateKey(tradingDayOf(clock.now)) : "";
  // A block body: an effect must return nothing or a cleanup (React 19 throws on any other value).
  useEffect(() => {
    onDay(dayKey || null);
  }, [dayKey, onDay]);

  const slots = useMemo(() => (series ? buildSlots(series.candles, series.gaps) : []), [series]);
  const last = useMemo(() => lastCandle(slots), [slots]);
  const reference = useMemo(() => changeReference(series), [series]);
  const change = last && reference ? formatChange(reference.price, last.c) : "";
  const showRing = showSelection && selected;

  return (
    <section
      aria-label={`Chart ${index + 1}: ${symbol}${showRing ? ", selected" : ""}`}
      data-selected={selected || undefined}
      onPointerDownCapture={() => onSelect(index)}
      onFocusCapture={() => onSelect(index)}
      className={cn(
        // The ring is an inset 1px shadow on an overlay above the canvas: an outer shadow was clipped
        // by the scrolling panel to just the corners (owner, 2026-10-18), and an inset one on the
        // cell itself is painted over by the chart.
        "relative flex min-h-0 min-w-0 flex-1 flex-col overflow-hidden rounded-xl bg-card p-1.5",
        "after:pointer-events-none after:absolute after:inset-0 after:z-10 after:rounded-xl after:transition-shadow after:duration-150 after:ease-out motion-reduce:after:transition-none",
        showRing && "after:shadow-[inset_0_0_0_1px_var(--color-primary)]",
      )}
    >
      <CandleChart
        className="min-h-0 flex-1"
        toolbar
        inspector
        // The toolbar in its own row above the chart: inside a cell half a screen wide it would sit on the heading.
        toolbarInChart={false}
        heading={
          <div className="flex items-baseline gap-2">
            <Tip label="Change ticker · tap, then hold to copy">
              <TickerLabel symbol={symbol} onTap={() => onPickTicker(index)} aria-label={`${symbol}, change ticker`} className="text-sm" />
            </Tip>
            <span className="font-mono text-sm tabular-nums">{last ? formatPrice(last.c) : "—"}</span>
            <span className={cn("font-mono text-[11px]", change.startsWith("-") ? "text-critical" : "text-good")}>{change}</span>
          </div>
        }
        symbol={symbol}
        // Each cell keeps its own screen layout (toolbox place, pane heights, tools shown) on this device.
        persistKey={`sw-dash:${index}`}
        // A cell is small: the floating drawing tools would cover the ticker, so they start hidden (the pencil shows them).
        defaultToolsVisible={false}
        indicators={indicators}
        onIndicatorsChange={app.setIndicators}
        layers={layers}
        onLayersChange={app.setLayers}
        settings={chartSettings}
        onSettingsChange={app.setChartSettings}
        settingsFromRemote={fromRemote}
        drawingSettings={drawingSettings}
        drawings={drawings}
        onDrawingsChange={drawingsHandle.set}
        tradeQty={tradeQty}
        onTradeQtyChange={app.setTradeQty}
        trading={trading}
        now={clock?.now ?? null}
        onTimeframeChange={(tf) => app.setDashboardTimeframe(index, tf)}
        series={series}
        timeframe={slot.timeframe}
        inspectT={inspectT}
        onInspectChange={setInspectT}
        options={CELL_CHART}
        label={`Chart ${index + 1}: ${symbol} ${slot.timeframe} candles${fixture ? ", fixture data" : ""}`}
      />
    </section>
  );
}

import type { DashboardSlot } from "@stockwatcher/app-state";
import { buildSlots, changeReference, formatChange, formatEtDateKey, formatPrice, lastCandle, tradingDayOf, type ChartLayers, type ChartOptions, type IndicatorSpec } from "@stockwatcher/chart-core";
import { CandleChart, type ChartTrading } from "@stockwatcher/chart-web";
import { cn, Tip } from "@stockwatcher/chart-web/ui";
import { TickerLabel } from "../lib/TickerLabel.tsx";
import { useEffect, useMemo, useState, useSyncExternalStore } from "react";
import type { ChartFeed } from "../feed/types.ts";
import { useChartFeed } from "../feed/useChartFeed.ts";
import { useChartAlerts } from "../alerts/useChartAlerts.ts";
import { useMediaQuery } from "../lib/useMediaQuery.ts";
import { getAppStore, useApp } from "../state/store.ts";
import { BREAKOUT_LABEL, bindNoticeContext, breakoutClockText, breakoutOverlays, breakoutSummary, detectionOverlays } from "@stockwatcher/shared";
import { formatEtTime12 } from "@stockwatcher/chart-core";
import { useDetection } from "../feed/useDetection.ts";
import { useZones } from "../feed/useZones.ts";
import { ZoneNote } from "./ZoneNote.tsx";
import { StarButton } from "../favorites/StarButton.tsx";

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
  /** The panel shows more than one chart (the timeframe becomes the compact wheel). */
  multi: boolean;
  fixture: boolean;
  onSelect: (index: number) => void;
  /** Opens the ticker search for this chart. */
  onPickTicker: (index: number) => void;
  /** Reports the trading day this cell's clock is on (the candidates follow it). */
  onDay: (day: string | null) => void;
  detectionSelection?: { episodeId: string; runEpoch: string };
}

/**
 * One chart of the dashboard's panel, in its own container. It streams on its own slot of the
 * page's one socket (`d0`…`d3`; up to 4 per connection, crates/server/src/ws.rs) and is the same
 * CandleChart as the chart page, with the person's synced indicators, levels, settings and this
 * symbol's drawings. Pressing anywhere in it selects it: the selected chart is ringed with the
 * accent (a 1px inset box shadow on an overlay, never a border: the no-borders rule) and is where a picked ticker loads.
 */
export function ChartCell({ index, slot, symbol, feed, trading, selected, showSelection, multi, fixture, onSelect, onPickTicker, onDay, detectionSelection }: ChartCellProps) {
  const [app] = useState(() => getAppStore().getState());
  const bin = useApp((s) => s.bin);
  // A phone-width cell has no room for four touch-size segments beside the ticker either.
  const phone = useMediaQuery("(max-width: 600px)");
  const { series, clock } = useChartFeed(feed, symbol, slot.timeframe, bin, `d${index}`);
  const discovery = useDetection(feed.detection);
  const noticeSelection = useApp((s) => s.noticeSelection);
  const pickedContext = !fixture && noticeSelection?.index === index && noticeSelection.context.symbol === symbol && noticeSelection.source === "live" ? noticeSelection.context : undefined;
  const [bound, setBound] = useState(() => bindNoticeContext(null, feed, fixture ? "fixture" : "live", pickedContext));
  const currentBinding = bindNoticeContext(bound, feed, fixture ? "fixture" : "live", pickedContext);
  if (currentBinding !== bound) setBound(currentBinding);
  const noticeContext = !currentBinding.invalidated ? pickedContext : undefined;
  useEffect(() => {
    if (currentBinding.invalidated && pickedContext) app.selectNotice();
  }, [app, currentBinding.invalidated, pickedContext]);
  const annotations = useMemo(() => {
    if (noticeContext) return breakoutOverlays(noticeContext, symbol);
    // Only a picked Signals (or Ross) row's episode is drawn (design canvas, 2026-10-22): no row
    // selected, no marks, so deselecting the row clears the chart. A resolved episode yields none
    // (detectionOverlays), so the marks also leave the moment it resolves.
    const id = detectionSelection && detectionSelection.runEpoch === discovery.frame?.runEpoch ? detectionSelection.episodeId : undefined;
    if (!id) return [];
    return detectionOverlays(discovery.snapshot, symbol, id).map((x) => discovery.connected && !discovery.resyncing ? x : { ...x, stale: true, label: `Last known · ${x.label}` });
  }, [discovery, symbol, detectionSelection, noticeContext]);
  // Volume shelves for this symbol from the Runners channel (subscribed here too, so the bands and
  // the caption show with the Runners tab closed); one object per change, not per frame.
  const zones = useZones(feed.rocket, symbol);

  const indicators = useApp((s) => s.indicators);
  const layers = useApp((s) => s.layers);
  const chartSettings = useApp((s) => s.chartSettings);
  const fromRemote = useApp((s) => s.chartSettingsFromRemote);
  // Settings → Chart → "Same indicators and levels on every chart" (owner, 2026-10-18): linked, every
  // chart shows and edits the shared set; unlinked, this chart has its own (app-state slot), which
  // starts as the shared one and is written back to the slot only.
  const linked = chartSettings.visibility.linkStudies;
  const ownIndicators = linked ? indicators : (slot.indicators ?? indicators);
  const ownLayers = linked ? layers : (slot.layers ?? layers);
  const onIndicatorsChange = linked ? app.setIndicators : (next: IndicatorSpec[]) => app.setDashboardIndicators(index, next);
  const onLayersChange = linked ? app.setLayers : (next: ChartLayers) => app.setDashboardLayers(index, next);
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
  // This symbol's price alerts: lines, the bell, hold to create, drag to move (alerts/useChartAlerts.ts).
  const chartAlerts = useChartAlerts(symbol, last?.c ?? null);
  const showRing = showSelection && selected;

  return (
    <section
      aria-label={`Chart ${index + 1}: ${symbol}${showRing ? ", selected" : ""}`}
      data-selected={selected || undefined}
      data-chart-cell=""
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
      {noticeContext && <Tip label={`${noticeContext.event_id} · ${noticeContext.breakout.policy_id}`}><p role="status" className="m-0 truncate px-2 py-1 text-[10px] text-text-faint">
        {BREAKOUT_LABEL} · frozen event · {breakoutSummary(noticeContext.breakout)} · {breakoutClockText(noticeContext.breakout, (ms) => formatEtTime12(ms, true))}
      </p></Tip>}
      {!noticeContext && (detectionSelection && detectionSelection.runEpoch !== discovery.frame?.runEpoch ? <p role="status" className="m-0 px-2 py-1 text-[10px] text-warning">Selected detection belongs to an earlier run; its chart references are unavailable.</p> : annotations.length > 0 && <Tip label={annotations[0]!.episodeId}><p role="status" className="m-0 truncate px-2 py-1 text-[10px] text-text-faint">{annotations[0]!.label} · experimental · {discovery.frame?.source} / {discovery.snapshot?.fidelity}</p></Tip>)}
      {/* The shelf caption goes with the bands: both follow the "Volume shelves" layer (levels.ts). */}
      <ZoneNote note={ownLayers.shelves ? zones.note : null} className="px-2 py-1" />
      <CandleChart
        className="min-h-0 flex-1"
        toolbar
        inspector
        // The toolbar in its own row above the chart: inside a cell half a screen wide it would sit on the heading.
        toolbarInChart={false}
        // One row (owner, 2026-10-18): the ticker, price and change on the toolbar's left, the
        // timeframe and tools on its right; with several charts the timeframe is the compact wheel
        // (the phone's), since four segments would crowd a half-width cell; so on a phone too.
        headingInToolbar
        timeframeControl={multi || phone ? "wheel" : "segments"}
        heading={
          <div className="flex items-center gap-1">
            <div className="flex shrink-0 items-baseline gap-2">
              <Tip label="Change ticker · tap, then hold to copy">
                <TickerLabel symbol={symbol} onTap={() => onPickTicker(index)} aria-label={`${symbol}, change ticker`} className="text-sm" />
              </Tip>
              <span className="font-mono text-sm tabular-nums">{last ? formatPrice(last.c) : "—"}</span>
              <span className={cn("font-mono text-[11px]", change.startsWith("-") ? "text-critical" : "text-good")}>{change}</span>
            </div>
            {symbol && <StarButton symbol={symbol} className="pointer-events-auto" />}
          </div>
        }
        symbol={symbol}
        // Each cell keeps its own screen layout (toolbox place, pane heights, tools shown) on this device.
        persistKey={`sw-dash:${index}`}
        // A cell is small: the floating drawing tools would cover the ticker, so they start hidden (the pencil shows them).
        defaultToolsVisible={false}
        indicators={ownIndicators}
        onIndicatorsChange={onIndicatorsChange}
        layers={ownLayers}
        onLayersChange={onLayersChange}
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
        alerts={chartAlerts}
        detection={annotations}
        volumeShelves={zones.bands}
        label={`Chart ${index + 1}: ${symbol} ${slot.timeframe} candles${fixture ? ", fixture data" : ""}`}
      />
    </section>
  );
}

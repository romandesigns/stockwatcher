import type { DashboardSlot } from "@stockwatcher/app-state";
import { buildSlots, changeReference, formatChange, formatEtDateKey, formatPrice, lastCandle, tradingDayOf, type ChartLayers, type IndicatorSpec } from "@stockwatcher/chart-core";
import { CandleChart, type ChartTrading } from "@stockwatcher/chart-native";
import { useEffect, useMemo, useState, useSyncExternalStore } from "react";
import { Pressable, StyleSheet, Text, View } from "react-native";

import { useChartFeed } from "../feed/hooks";
import type { MobileFeed } from "../feed/serverFeed";
import { appActions, getLoadedStore, useApp } from "../state/store";
import { colors, fonts } from "../theme";
import { useTickerPress } from "../ui/useTickerPress";

/** Below this width a tile drops the chart's toolbar (a 2×2 grid on a phone is ~180 pt wide). */
const TOOLBAR_MIN_WIDTH = 320;

export interface ChartTileProps {
  index: number;
  slot: DashboardSlot;
  /** What this tile shows: the slot's ticker, or a candidate while it has none (never written back). */
  symbol: string;
  feed: MobileFeed;
  trading: ChartTrading;
  selected: boolean;
  showSelection: boolean;
  onPickTicker: (index: number) => void;
  onDay: (day: string | null) => void;
}

/**
 * One chart of the phone dashboard's panel: the web's ChartCell for React Native. Its own slot on
 * the dashboard's socket (`d0`…`d3`), the person's synced indicators, levels and settings and this
 * symbol's drawings. Touching it selects it (the touch still reaches the chart); the selected tile
 * is ringed with the accent by a box shadow, never a border.
 */
export function ChartTile({ index, slot, symbol, feed, trading, selected, showSelection, onPickTicker, onDay }: ChartTileProps) {
  const [app] = useState(appActions);
  const bin = useApp((s) => s.bin);
  const { series, clock } = useChartFeed(feed, symbol, slot.timeframe, bin, `d${index}`);
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
  const toolbox = useApp((s) => s.device?.toolbox);
  const toolsVisible = useApp((s) => s.device?.toolsVisible ?? true);
  const drawingsHandle = useMemo(() => getLoadedStore().drawingsFor(symbol), [symbol]);
  const drawings = useSyncExternalStore(drawingsHandle.subscribe, drawingsHandle.get);

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

  const [width, setWidth] = useState(0);
  // Tap: change this chart's ticker; tap, then hold: copy it.
  const tickerPress = useTickerPress(symbol, () => onPickTicker(index));
  const slots = useMemo(() => (series ? buildSlots(series.candles, series.gaps) : []), [series]);
  const last = lastCandle(slots);
  const reference = useMemo(() => changeReference(series), [series]);
  const change = last && reference ? formatChange(reference.price, last.c) : "";
  const ring = showSelection && selected;

  return (
    <View
      onTouchStart={() => app.selectDashboardChart(index)}
      onLayout={(e) => setWidth(e.nativeEvent.layout.width)}
      accessible={false}
      accessibilityLabel={`Chart ${index + 1}: ${symbol}${ring ? ", selected" : ""}`}
      style={[styles.tile, ring && styles.ring]}
    >
      <View style={styles.head}>
        <Pressable accessibilityRole="button" accessibilityLabel={`${symbol}, change ticker`} accessibilityHint="Tap, then press and hold to copy" {...tickerPress} hitSlop={8}>
          <Text style={styles.symbol} numberOfLines={1}>
            {symbol}
          </Text>
        </Pressable>
        <Text style={styles.price} numberOfLines={1}>
          {last ? formatPrice(last.c) : "—"}
        </Text>
        <Text style={[styles.change, { color: change.startsWith("-") ? colors.critical : colors.good }]} numberOfLines={1}>
          {change}
        </Text>
      </View>
      <CandleChart
        style={styles.chart}
        toolbar={width >= TOOLBAR_MIN_WIDTH}
        symbol={symbol}
        series={series}
        timeframe={slot.timeframe}
        inspectT={inspectT}
        onInspectChange={setInspectT}
        settings={chartSettings}
        onSettingsChange={app.setChartSettings}
        settingsFromRemote={fromRemote}
        drawingSettings={drawingSettings}
        indicators={ownIndicators}
        onIndicatorsChange={onIndicatorsChange}
        drawings={drawings}
        onDrawingsChange={drawingsHandle.set}
        trading={trading}
        tradeQty={tradeQty}
        onTradeQtyChange={app.setTradeQty}
        toolboxPlacement={toolbox}
        onToolboxPlacementChange={app.setToolbox}
        toolsVisible={toolsVisible}
        onToolsVisibleChange={app.setToolsVisible}
        layers={ownLayers}
        onLayersChange={onLayersChange}
        now={clock?.now ?? null}
        onTimeframeChange={(tf) => app.setDashboardTimeframe(index, tf)}
        options={{ priceAxisWidth: 40, volumeHeight: 28, timeAxisHeight: 16, paneGap: 4, fontSize: 8, timeLabelSpacing: 56, indicatorPaneHeight: 44 }}
        barSpacing={6}
        label={`Chart ${index + 1}: ${symbol} ${slot.timeframe} candles`}
      />
    </View>
  );
}

const styles = StyleSheet.create({
  tile: { flex: 1, minWidth: 0, minHeight: 0, backgroundColor: colors.bgPanel, borderRadius: 12, padding: 6, gap: 4 },
  // The selection: a 2 pt accent ring drawn as a spread shadow, so it adds no layout and no border.
  ring: { boxShadow: `0 0 0 2px ${colors.accent}` },
  head: { flexDirection: "row", alignItems: "baseline", gap: 6, paddingHorizontal: 2 },
  symbol: { color: colors.text, fontFamily: fonts.semibold, fontSize: 13 },
  price: { color: colors.text, fontSize: 12, fontVariant: ["tabular-nums"] },
  change: { fontSize: 10, fontVariant: ["tabular-nums"] },
  chart: { flex: 1 },
});

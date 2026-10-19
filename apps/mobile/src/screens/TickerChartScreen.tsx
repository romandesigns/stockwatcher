import { buildSlots, changeReference, formatChange, formatPrice, lastCandle, type Timeframe } from "@stockwatcher/chart-core";
import { CandleChart, Icon } from "@stockwatcher/chart-native";
import { useNavigation, type RouteProp } from "@react-navigation/native";
import { useMemo, useState, useSyncExternalStore } from "react";
import { Pressable, StyleSheet, Text, View } from "react-native";
import { useSafeAreaInsets } from "react-native-safe-area-context";

import { useChartAlerts } from "../alerts/useChartAlerts";
import { useDashboardFeed } from "../feed/dashboardFeed";
import { useChartFeed, usePaperTrading } from "../feed/hooks";
import type { MobileFeed } from "../feed/serverFeed";
import type { RootStackParamList } from "../navigation/RootStack";
import { appActions, getLoadedStore, useApp } from "../state/store";
import { colors, fonts } from "../theme";

/** The chart's metrics: the Chart tab's legible set (10 pt text, 40 pt volume, 52 pt panes). */
const OPTIONS = { priceAxisWidth: 52, volumeHeight: 40, timeAxisHeight: 18, paneGap: 4, fontSize: 10, timeLabelSpacing: 64, indicatorPaneHeight: 52 } as const;

/** The modal's chart slot on the dashboard's socket (the dashboard itself shows no chart). */
const SLOT = "m0";

/** The last timeframe picked in the modal, so the next ticker opens on it (this app run). */
let lastTimeframe: Timeframe | null = null;

type Props = { route: RouteProp<RootStackParamList, "TickerChart"> };

/**
 * A ticker's chart as a modal (owner, 2026-10-19): on the phone a tapped ticker (a Halt watcher
 * gauge, a search result) opens here instead of loading into a dashboard chart. For now it holds
 * the chart only; the ticker's other information comes later. It reads the dashboard's market
 * (live unless Settings → Data says replay) on the dashboard's own socket.
 *
 * Dismissed by the chevron, Android back, or a swipe down from the header strip (motion spec
 * `chartModal`; lower down a vertical drag belongs to the chart).
 */
export function TickerChartScreen({ route }: Props) {
  const feed = useDashboardFeed();
  const navigation = useNavigation();
  const insets = useSafeAreaInsets();
  const { symbol } = route.params;
  return (
    <View style={[styles.screen, { paddingTop: insets.top + 4, paddingBottom: insets.bottom + 4 }]} accessibilityViewIsModal>
      {feed ? (
        <ModalChart key={symbol} feed={feed} symbol={symbol} onClose={() => navigation.goBack()} />
      ) : (
        <Header symbol={symbol} price="—" change="" onClose={() => navigation.goBack()} />
      )}
    </View>
  );
}

function Header({ symbol, price, change, onClose }: { symbol: string; price: string; change: string; onClose: () => void }) {
  return (
    <View style={styles.header}>
      {/* The grab handle: the strip a swipe down dismisses from. */}
      <View style={styles.handle} />
      <View style={styles.headRow}>
        <Text accessibilityRole="header" style={styles.symbol} numberOfLines={1}>
          {symbol}
        </Text>
        <Text style={styles.price} numberOfLines={1}>
          {price}
        </Text>
        <Text style={[styles.change, { color: change.startsWith("-") ? colors.critical : colors.good }]} numberOfLines={1}>
          {change}
        </Text>
        <Pressable accessibilityRole="button" accessibilityLabel={`Close ${symbol} chart`} onPress={onClose} hitSlop={6} style={({ pressed }) => [styles.close, pressed && { backgroundColor: colors.bgRow }]}>
          <Icon name="chevronDown" color={colors.textDim} size={18} />
        </Pressable>
      </View>
    </View>
  );
}

function ModalChart({ feed, symbol, onClose }: { feed: MobileFeed; symbol: string; onClose: () => void }) {
  const [app] = useState(appActions);
  const firstSlotTf = useApp((s) => s.dashboard.slots[0]?.timeframe);
  const [timeframe, setTimeframe] = useState<Timeframe>(() => lastTimeframe ?? firstSlotTf ?? "5m");
  const bin = useApp((s) => s.bin);
  const { series, clock } = useChartFeed(feed, symbol, timeframe, bin, SLOT);
  const trading = usePaperTrading(feed);
  const indicators = useApp((s) => s.indicators);
  const layers = useApp((s) => s.layers);
  const chartSettings = useApp((s) => s.chartSettings);
  const fromRemote = useApp((s) => s.chartSettingsFromRemote);
  const drawingSettings = useApp((s) => s.drawingSettings);
  const tradeQty = useApp((s) => s.tradeQty);
  const toolbox = useApp((s) => s.device?.toolbox);
  const toolsVisible = useApp((s) => s.device?.toolsVisible ?? true);
  const drawingsHandle = useMemo(() => getLoadedStore().drawingsFor(symbol), [symbol]);
  const drawings = useSyncExternalStore(drawingsHandle.subscribe, drawingsHandle.get);
  const [inspectT, setInspectT] = useState<number | null>(null);

  const slots = useMemo(() => (series ? buildSlots(series.candles, series.gaps) : []), [series]);
  const last = lastCandle(slots);
  const reference = useMemo(() => changeReference(series), [series]);
  const change = last && reference ? formatChange(reference.price, last.c) : "";
  const alerts = useChartAlerts(symbol, last?.c ?? null);

  return (
    <>
      <Header symbol={symbol} price={last ? formatPrice(last.c) : "—"} change={change} onClose={onClose} />
      <View style={styles.chartWrap}>
        <CandleChart
          style={styles.chart}
          toolbar
          symbol={symbol}
          series={series}
          timeframe={timeframe}
          inspectT={inspectT}
          onInspectChange={setInspectT}
          settings={chartSettings}
          onSettingsChange={app.setChartSettings}
          settingsFromRemote={fromRemote}
          drawingSettings={drawingSettings}
          indicators={indicators}
          onIndicatorsChange={app.setIndicators}
          drawings={drawings}
          onDrawingsChange={drawingsHandle.set}
          trading={trading}
          tradeQty={tradeQty}
          onTradeQtyChange={app.setTradeQty}
          toolboxPlacement={toolbox}
          onToolboxPlacementChange={app.setToolbox}
          toolsVisible={toolsVisible}
          onToolsVisibleChange={app.setToolsVisible}
          layers={layers}
          onLayersChange={app.setLayers}
          now={clock?.now ?? null}
          onTimeframeChange={(tf) => {
            lastTimeframe = tf;
            setInspectT(null);
            setTimeframe(tf);
          }}
          options={OPTIONS}
          barSpacing={8}
          alerts={alerts}
          label={`${symbol} ${timeframe} candles`}
        />
      </View>
    </>
  );
}

const styles = StyleSheet.create({
  screen: { flex: 1, backgroundColor: colors.bg, paddingHorizontal: 6, gap: 6 },
  // With the top inset, the handle and the 44 pt row fill the swipe-to-dismiss strip (CHART_MODAL_GRAB).
  header: { gap: 6 },
  handle: { alignSelf: "center", width: 36, height: 4, borderRadius: 2, backgroundColor: colors.bgRow, marginTop: 2 },
  headRow: { flexDirection: "row", alignItems: "center", gap: 8, paddingHorizontal: 6 },
  symbol: { color: colors.text, fontFamily: fonts.semibold, fontSize: 18 },
  price: { color: colors.text, fontSize: 16, fontVariant: ["tabular-nums"] },
  change: { fontSize: 13, fontVariant: ["tabular-nums"] },
  close: { marginLeft: "auto", width: 44, height: 44, borderRadius: 22, alignItems: "center", justifyContent: "center", backgroundColor: colors.bgPanel },
  chartWrap: { flex: 1, minHeight: 0, backgroundColor: colors.bgPanel, borderRadius: 12, padding: 6 },
  chart: { flex: 1 },
});

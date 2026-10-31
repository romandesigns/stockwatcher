import { buildSlots, changeReference, formatChange, formatEtTime12, formatPrice, indicatorDemand, lastCandle, type Timeframe } from "@stockwatcher/chart-core";
import { CandleChart } from "@stockwatcher/chart-native";
import { BREAKOUT_LABEL, bindNoticeContext, breakoutClockText, breakoutOverlays, breakoutSummary, detectionFamilyLabel, detectionOverlays, detectionPrice, detectionStateLabel, groupDetections, type NoticeChartContext } from "@stockwatcher/shared";
import { useNavigation, type RouteProp } from "@react-navigation/native";
import { useMemo, useState, useSyncExternalStore } from "react";
import { Pressable, StyleSheet, Text, View } from "react-native";
import Animated, { FadeIn, FadeOut, LayoutAnimationConfig, LinearTransition, ReduceMotion } from "react-native-reanimated";
import { useSafeAreaInsets } from "react-native-safe-area-context";

import { useChartAlerts } from "../alerts/useChartAlerts";
import { DetectionEpisodeDetails } from "../dashboard/DetectionPanel";
import { useDashboardFeed } from "../feed/dashboardFeed";
import { useChartFeed, useDetection, usePaperTrading, useZones } from "../feed/hooks";
import { ZoneNote } from "../dashboard/ZoneNote";
import type { MobileFeed } from "../feed/serverFeed";
import { chartSwapTiming } from "../motion/transitions";
import { useReducedMotion } from "../motion/useReducedMotion";
import type { RootStackParamList } from "../navigation/RootStack";
import { appActions, getLoadedStore, useApp } from "../state/store";
import { colors, fonts } from "../theme";
import { useTickerPress } from "../ui/useTickerPress";

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
 * Dismissed by a swipe down from the header strip (motion spec `chartModal`; lower down a vertical
 * drag belongs to the chart), Android back, or VoiceOver's escape gesture. No close button (owner,
 * 2026-10-19: the swipe is known); the header row keeps the button's 44 pt height so the swipe
 * strip still ends above the chart's toolbar and its timeframe wheel.
 */
export function TickerChartScreen({ route }: Props) {
  const feed = useDashboardFeed();
  const navigation = useNavigation();
  const insets = useSafeAreaInsets();
  // The ticker comes from the route's params on every render, never copied into state: a ticker
  // opened while this modal is up (a notification tap, owner 2026-10-23) replaces the params of
  // this same route (navigation/openTicker.ts), and the chart below is keyed by the ticker, so its
  // data subscription, drawings, readout and detection marks start over for the new one.
  const { symbol, detectionEpisodeId, detectionRunEpoch } = route.params;
  const { noticeContext } = route.params;
  const reducedMotion = useReducedMotion();
  return (
    <View style={[styles.screen, { paddingTop: insets.top + 4, paddingBottom: insets.bottom + 4 }]} accessibilityViewIsModal onAccessibilityEscape={() => navigation.goBack()}>
      {/* No fade on the modal's first ticker (the modal's own present runs); a later one fades in
          (chartSwapTiming) while the old one is already gone, so it never shows under the new name. */}
      <LayoutAnimationConfig skipEntering>
        <Animated.View key={symbol} style={styles.swap} entering={FadeIn.duration(chartSwapTiming.duration).easing(chartSwapTiming.easing).reduceMotion(reducedMotion ? ReduceMotion.Always : ReduceMotion.System)}>
          {feed ? (
            <ModalChart feed={feed} symbol={symbol} detectionEpisodeId={detectionEpisodeId} detectionRunEpoch={detectionRunEpoch} noticeContext={noticeContext} />
          ) : (
            <Header symbol={symbol} price="—" change="" />
          )}
        </Animated.View>
      </LayoutAnimationConfig>
    </View>
  );
}

/** The modal already shows this ticker, so a single tap does nothing; it only arms the copy. */
const noop = () => {};

function Header({ symbol, price, change }: { symbol: string; price: string; change: string }) {
  // Tap, then press and hold copies the ticker, like every other ticker label in the app (owner,
  // 2026-10-20: it did nothing here). The press sits inside the swipe-down strip; a swipe takes the
  // responder away, which cancels the press, so dismissing never copies.
  const press = useTickerPress(symbol, noop);
  return (
    <View style={styles.header}>
      {/* The grab handle: the strip a swipe down dismisses from. */}
      <View style={styles.handle} />
      <View style={styles.headRow}>
        <Pressable accessibilityLabel={symbol} accessibilityHint="Tap, then press and hold to copy" {...press} hitSlop={8} style={({ pressed }) => pressed && { opacity: 0.6 }}>
          <Text accessibilityRole="header" style={styles.symbol} numberOfLines={1}>
            {symbol}
          </Text>
        </Pressable>
        <Text style={styles.price} numberOfLines={1}>
          {price}
        </Text>
        <Text style={[styles.change, { color: change.startsWith("-") ? colors.critical : colors.good }]} numberOfLines={1}>
          {change}
        </Text>
      </View>
    </View>
  );
}

function ModalChart({ feed, symbol, detectionEpisodeId, detectionRunEpoch, noticeContext }: { feed: MobileFeed; symbol: string; detectionEpisodeId?: string; detectionRunEpoch?: string; noticeContext?: NoticeChartContext }) {
  const [app] = useState(appActions);
  const source = useApp((s) => s.dashboard.source);
  // A context is tied to the live feed it first inspects. Switching source/connection invalidates
  // it permanently for this route; switching back cannot resurrect another source's geometry.
  const [bound, setBound] = useState(() => bindNoticeContext(null, feed, source, noticeContext));
  const currentBinding = bindNoticeContext(bound, feed, source, noticeContext);
  if (currentBinding !== bound) setBound(currentBinding);
  const eventContext = !currentBinding.invalidated && noticeContext?.symbol === symbol ? noticeContext : undefined;
  const firstSlotTf = useApp((s) => s.dashboard.slots[0]?.timeframe);
  const [timeframe, setTimeframe] = useState<Timeframe>(() => lastTimeframe ?? firstSlotTf ?? "5m");
  const bin = useApp((s) => s.bin);
  const indicators = useApp((s) => s.indicators);
  const chartSettings = useApp((s) => s.chartSettings);
  // What the chart shows of its indicators, for the server to compute (indicators.v1).
  const demand = useMemo(() => indicatorDemand(indicators, chartSettings), [indicators, chartSettings]);
  const { series, clock, indicators: serverIndicators } = useChartFeed(feed, symbol, timeframe, bin, SLOT, demand);
  const trading = usePaperTrading(feed);
  const layers = useApp((s) => s.layers);
  const fromRemote = useApp((s) => s.chartSettingsFromRemote);
  const drawingSettings = useApp((s) => s.drawingSettings);
  const tradeQty = useApp((s) => s.tradeQty);
  const toolbox = useApp((s) => s.device?.toolbox);
  const toolsVisible = useApp((s) => s.device?.toolsVisible ?? true);
  const drawingsHandle = useMemo(() => getLoadedStore().drawingsFor(symbol), [symbol]);
  const drawings = useSyncExternalStore(drawingsHandle.subscribe, drawingsHandle.get);
  const [inspectT, setInspectT] = useState<number | null>(null);
  const [detailsOpen, setDetailsOpen] = useState(false);
  const reducedMotion = useReducedMotion();
  const detection = useDetection(feed.detection);
  const matchingRun = !detectionRunEpoch || detectionRunEpoch === detection.frame?.runEpoch;
  const episode = detectionEpisodeId
    ? matchingRun ? detection.snapshot?.episodes.find((e) => e.episode_id === detectionEpisodeId && e.symbol === symbol) : undefined
    : groupDetections(detection.snapshot).find((g) => g.symbol === symbol)?.selected;
  const annotations = eventContext ? breakoutOverlays(eventContext, symbol) : episode ? detectionOverlays(detection.snapshot, symbol, episode.episode_id).map((item) => detection.connected && !detection.resyncing ? item : { ...item, stale: true, label: `Last known · ${item.label}` }) : [];

  const slots = useMemo(() => (series ? buildSlots(series.candles, series.gaps) : []), [series]);
  const last = lastCandle(slots);
  const reference = useMemo(() => changeReference(series), [series]);
  const change = last && reference ? formatChange(reference.price, last.c) : "";
  const alerts = useChartAlerts(symbol, last?.c ?? null);
  // Volume shelves from the Runners channel (subscribed here too, whether or not the card is shown).
  const zones = useZones(feed.rocket, symbol);

  return (
    <>
      <Header symbol={symbol} price={last ? formatPrice(last.c) : "—"} change={change} />
      {eventContext && <View style={styles.detectionSummary}>
        <Text style={styles.detectionState}>{BREAKOUT_LABEL} · frozen event</Text>
        <Text style={styles.detectionMeta}>{breakoutSummary(eventContext.breakout)}</Text>
        <Text style={styles.detectionMeta}>{breakoutClockText(eventContext.breakout, (ms) => formatEtTime12(ms, true))}</Text>
        <Text style={styles.detectionMeta}>{eventContext.breakout.exact_state === "restored" ? "Original event measurements restored from complete history." : "Original event measurements captured at evaluation."}</Text>
      </View>}
      <ZoneNote note={layers.shelves ? zones.note : null} reducedMotion={reducedMotion} gap={6} style={styles.zone} />
      {/* Only with the detector on and an episode for this ticker (or one the user picked): an
          "off" caption would take chart height on the cover screen for nothing. */}
      {detection.enabled === true && (episode || detectionEpisodeId) ? (
      <View style={styles.detectionSummary}>
        <Text style={styles.detectionMeta}>Detection · experimental · {detection.frame?.source ?? "source pending"} · canonical 15s</Text>
        {!detection.connected && <Text style={styles.detectionWarning}>Current assessment unavailable · {detection.snapshot ? "last received state" : "waiting for snapshot"}</Text>}
        {episode ? <>
          <Pressable accessibilityRole="button" accessibilityState={{ expanded: detailsOpen }} accessibilityLabel={`${detailsOpen ? "Hide" : "Show"} selected detection details`} onPress={() => setDetailsOpen(!detailsOpen)} style={styles.detectionButton}>
            <Text style={styles.detectionState}>{detectionFamilyLabel(episode.family)} · {detectionStateLabel(episode.state)} · {detailsOpen ? "hide details" : "details"}</Text>
          </Pressable>
          <Text style={styles.detectionMeta}>{episode.level?.kind === "range_high" ? "Range high" : "T"} {detectionPrice(episode.level?.trigger ?? null)} · I {detectionPrice(episode.level?.invalidation ?? null)}{episode.level ? ` · v${episode.level.version}` : ""} · {episode.fidelity.replace(/_/g, " ")}</Text>
          <Text selectable numberOfLines={1} style={styles.detectionId}>{episode.episode_id}</Text>
        </> : <Text style={styles.detectionMeta}>Selected episode is unavailable in this source/run or bounded history.</Text>}
      </View>
      ) : null}
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
          serverIndicators={serverIndicators}
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
          detection={annotations}
          volumeShelves={zones.bands}
          label={`${symbol} ${timeframe} candles`}
        />
      </View>
      {detailsOpen && episode && detection.snapshot && <Animated.ScrollView entering={FadeIn.duration(140).reduceMotion(reducedMotion ? ReduceMotion.Always : ReduceMotion.System)} exiting={FadeOut.duration(100).reduceMotion(reducedMotion ? ReduceMotion.Always : ReduceMotion.System)} layout={LinearTransition.duration(160).reduceMotion(reducedMotion ? ReduceMotion.Always : ReduceMotion.System)} style={styles.detectionDetails} showsVerticalScrollIndicator={false} showsHorizontalScrollIndicator={false}><DetectionEpisodeDetails episode={episode} snapshot={detection.snapshot} currentAvailable={detection.connected && !detection.resyncing} /></Animated.ScrollView>}
    </>
  );
}

const styles = StyleSheet.create({
  screen: { flex: 1, backgroundColor: colors.bg, paddingHorizontal: 6 },
  swap: { flex: 1, gap: 6 },
  // With the top inset, the handle and the 44 pt row fill the swipe-to-dismiss strip (CHART_MODAL_GRAB).
  header: { gap: 6 },
  handle: { alignSelf: "center", width: 36, height: 4, borderRadius: 2, backgroundColor: colors.bgRow, marginTop: 2 },
  headRow: { minHeight: 44, flexDirection: "row", alignItems: "center", gap: 8, paddingHorizontal: 6 },
  symbol: { color: colors.text, fontFamily: fonts.semibold, fontSize: 18 },
  price: { color: colors.text, fontSize: 16, fontVariant: ["tabular-nums"] },
  change: { fontSize: 13, fontVariant: ["tabular-nums"] },
  chartWrap: { flex: 1, minHeight: 0, backgroundColor: colors.bgPanel, borderRadius: 12, padding: 6 },
  chart: { flex: 1 },
  detectionSummary: { gap: 3, paddingHorizontal: 6 },
  zone: { paddingHorizontal: 6 },
  detectionMeta: { color: colors.textFaint, fontFamily: fonts.regular, fontSize: 10, lineHeight: 14 },
  detectionWarning: { color: colors.warning, fontFamily: fonts.regular, fontSize: 10 },
  detectionButton: { minHeight: 36, justifyContent: "center" },
  detectionState: { color: colors.accent, fontFamily: fonts.semibold, fontSize: 11 },
  detectionId: { color: colors.textFaint, fontFamily: fonts.regular, fontSize: 9 },
  detectionDetails: { maxHeight: "42%", paddingHorizontal: 6 },
});

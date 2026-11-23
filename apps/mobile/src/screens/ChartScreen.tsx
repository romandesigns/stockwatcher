import {
  buildSlots,
  changeReference,
  currentReplayDay,
  formatDayKey,
  replayDayOptions,
  formatChange,
  formatPrice,
  indicatorDemand,
  lastCandle,
  formatEtDateKey,
  REPLAY_BAR_EXPANDED_KEY,
  sessionBands,
  tradingDayOf,
  type Candle,
} from "@stockwatcher/chart-core";
import { PROFILE_BINS } from "@stockwatcher/app-state";
import { CandleChart, Icon, ReplayBar, ReplayDaySheet, Sheet, TapeSheet } from "@stockwatcher/chart-native";
import { detectionFamilyLabel, detectionOverlays, detectionStateLabel, groupDetections, type ConnectionStatus } from "@stockwatcher/shared";
import { useEffect, useLayoutEffect, useMemo, useState } from "react";
import { Pressable, ScrollView, StyleSheet, Text, useWindowDimensions, View } from "react-native";
import { useSafeAreaInsets } from "react-native-safe-area-context";

import { DetectionEpisodeDetails } from "../dashboard/DetectionPanel";
import { PracticePanels } from "../dashboard/PracticePanels";
import { ZoneNote } from "../dashboard/ZoneNote";
import { useCandidates, useChartFeed, useDetection, usePaperTrading, useReplayAlerts, useReplayDays, useZones } from "../feed/hooks";
import type { MobileFeed } from "../feed/serverFeed";
import { useConnection } from "../feed/useConnection";
import { useReducedMotion } from "../motion/useReducedMotion";
import { usePracticeToasts } from "../notifications/practiceToasts";
import { TickerSearchSheet } from "../search/TickerSearchSheet";
import { getPreference, setPreference } from "../storage/preferences";
import { appActions, useApp } from "../state/store";
import { useTickerPress } from "../ui/useTickerPress";
import { SignInScreen } from "./SignInScreen";
import { colors, fonts } from "../theme";
import { StarToggle } from "../ui/StarToggle";

/** The screen's gap between its blocks, which a closed volume-shelf note cancels (Collapse). */
const SCREEN_GAP = 8;
/** The screen's side padding: the replay bar's width is the window's less this on each side. */
const SCREEN_PAD = 6;
const NO_CANDLES: readonly Candle[] = [];
/** How long a picked day may read "Loading…" (a refused switch never changes the day). */
const PENDING_DAY_TIMEOUT_MS = 5 * 60_000;
/** From this window width (the Fold unfolded, sw600dp) the dashboard's cards sit beside the chart. */
const PANELS_BESIDE_MIN = 600;

/**
 * The practice screen (the Chart tab; replay parity plan Phase 1, owner 2026-11-23): the chart on
 * the REPLAY market with the replay bar and the order ticket, plus the dashboard's cards on the
 * same replay connection (beside the chart unfolded, in a sheet on the cover screen), and the
 * replay's own alerts as in-app "Practice" toasts. The only screen with replay data: the
 * dashboard reads live only. It connects once (useConnection: token, app store, its own socket).
 */
export function ChartScreen() {
  const { ready, status, signIn } = useConnection("mobile", "replay");
  if (!ready) return <View style={styles.screen} />;
  // The server refused the hello: no token yet, or not this one (production requires one).
  if (status === "unauthorized") return <SignInScreen rejected={ready.token !== null} onSubmit={signIn} />;
  return <ChartView feed={ready.feed} token={ready.token} status={status} />;
}

function ChartView({ feed, token, status }: { feed: MobileFeed; token: string | null; status: ConnectionStatus }) {
  const insets = useSafeAreaInsets();
  // App state lives in the one store (src/state/store.ts, @stockwatcher/app-state): the ticker,
  // timeframe, bin, trade size, search recents, chart and drawing settings, indicators, levels and
  // drawings sync live with every other client (desktop, web, other phones) and are kept on the
  // server; the toolbox place, pane heights and tools shown are this phone's own layout; sheets
  // and the inspected candle are this session's.
  // The store's actions (the same functions in every state, so read once; values come through useApp).
  const [app] = useState(appActions);
  // The replay's own alerts as "Practice" toasts, real live ones meanwhile as "Live" toasts.
  usePracticeToasts(feed, app.openTicker);
  // The dashboard's cards on the cover screen: a sheet over the chart.
  const [panelsOpen, setPanelsOpen] = useState(false);
  // The trading day the replay clock is on; it changes on a day switch (and once a night while playing).
  const [clockDay, setClockDay] = useState<string | null>(null);
  const [detectionDetail, setDetectionDetail] = useState<{ id: string; epoch: string } | null>(null);
  const detection = useDetection(feed.detection);
  const { symbols: candidates, fixture } = useCandidates(token, clockDay, "replay", feed.symbols);
  const replayDays = useReplayDays(token, clockDay);
  const dayOptions = useMemo(() => replayDayOptions(replayDays.state?.days ?? []), [replayDays.state]);
  const daysOpen = useApp((s) => s.daysOpen);
  const symbol = useApp((s) => s.symbol);
  const timeframe = useApp((s) => s.timeframe);
  const inspectT = useApp((s) => s.inspectT);
  const chartSettings = useApp((s) => s.chartSettings);
  const chartSettingsFromRemote = useApp((s) => s.chartSettingsFromRemote);
  const drawingSettings = useApp((s) => s.drawingSettings);
  const indicators = useApp((s) => s.indicators);
  const layers = useApp((s) => s.layers);
  const settingsOpen = useApp((s) => s.settingsOpen);
  // The store on the phone always has a device slice (loadAppStore); the fallbacks only satisfy the type.
  const toolbox = useApp((s) => s.device?.toolbox);
  const paneHeights = useApp((s) => s.device?.paneHeights);
  const toolsVisible = useApp((s) => s.device?.toolsVisible ?? true);

  // Follow the price band: a symbol that leaves it is no longer let through. The fallback is not
  // written back (a phone with a momentarily stale list must not move every other device's chart).
  const active = symbol && candidates.includes(symbol) ? symbol : (candidates[0] ?? null);
  const detectionEpisode = groupDetections(detection.snapshot).find((g) => g.symbol === active)?.selected;
  const detailedEpisode = detectionDetail && detectionDetail.epoch === detection.frame?.runEpoch ? detection.snapshot?.episodes.find((e) => e.episode_id === detectionDetail.id && e.symbol === active) : undefined;
  const selectedEpisodeId = detailedEpisode?.episode_id ?? detectionEpisode?.episode_id;
  const detectionAnnotations = selectedEpisodeId && (!detectionDetail || detailedEpisode) ? detectionOverlays(detection.snapshot, active ?? "", selectedEpisodeId).map((item) => detection.connected && !detection.resyncing ? item : { ...item, stale: true, label: `Last known · ${item.label}` }) : [];
  // Drawings follow the shown symbol; before paint, so a ticker switch never flashes the old marks.
  useLayoutEffect(() => app.showSymbol(active ?? ""), [app, active]);
  const bin = useApp((s) => s.bin);
  const tradeQty = useApp((s) => s.tradeQty);
  const recents = useApp((s) => s.searchRecents);
  // What the chart shows of its indicators (and whether its zones layer is on), for the server to
  // compute (indicators.v1, zones.v1).
  const demand = useMemo(() => indicatorDemand(indicators, chartSettings, layers), [indicators, chartSettings, layers]);
  const { series, clock, tape, indicators: serverIndicators, loadIndicatorHistory, levels: serverLevels, zones: serverZones, macdStyles } = useChartFeed(feed, active ?? "", timeframe, bin, undefined, demand);
  // This symbol's drawings, synced per drawing (another device's edit of a different drawing is never overwritten).
  const drawings = useApp((s) => s.drawings);
  const tapeOpen = useApp((s) => s.tapeOpen);
  // Ticker quick-open, the web's Ctrl+P: tap the ticker (or the search icon) in the header.
  const searchOpen = useApp((s) => s.searchOpen);
  const reducedMotion = useReducedMotion();
  // The header ticker: tap opens the search; tap, then hold copies it.
  const tickerPress = useTickerPress(active ?? "", () => app.setSearchOpen(true));

  // The day's session bands and a 20-minute volume minimap for the timeline (bands change only with the day).
  const dayKey = clock ? formatEtDateKey(tradingDayOf(clock.now)) : "";
  // Derived during render (React's "adjusting state when a prop changes"), not in an effect: the hooks
  // above that read it run before the clock is known, and an effect would cost an extra commit.
  if ((dayKey || null) !== clockDay) setClockDay(dayKey || null);
  const bands = useMemo(() => {
    if (!dayKey) return [];
    const [year, month, day] = dayKey.split("-").map(Number) as [number, number, number];
    // The server's schedule for the day when the chart has one (an early close ends at 13:00).
    return sessionBands({ year, month, day }, series?.sessions);
  }, [dayKey, series?.sessions]);
  const trading = usePaperTrading(feed);
  // Volume shelves for the shown ticker from this connection's Runners channel (zones.ts).
  const zones = useZones(feed.rocket, active ?? "");

  const slots =useMemo(() => (series ? buildSlots(series.candles, series.gaps) : []), [series]);
  const last = lastCandle(slots);
  // Previous close when the server knows it, else the anchor session's first candle: the web
  // header's and the search rows' rule (changeReference in chart-core).
  const reference = useMemo(() => changeReference(series), [series]);
  const change = last && reference ? formatChange(reference.price, last.c) : "";

  const live = clock?.mode === "live";
  // The Fold's cover screen or unfolded: the replay bar picks its artboard from this width.
  const { width: windowWidth } = useWindowDimensions();
  const panelsBeside = windowWidth >= PANELS_BESIDE_MIN;
  const offline = status !== "open";
  const replayDay = currentReplayDay(replayDays.state, clock);
  // The date is the day picker only when there is something to pick (replay mode, days offered).
  const canPickDay = !live && !!clock && !!replayDay && dayOptions.length > 0;
  // A picked day the server is still switching to: a whole-market rebuild takes a minute or two,
  // and without this the sheet closed on the old date as if the pick was ignored (owner,
  // 2026-10-23; the web's ReplayBar `pendingDay`). Cleared once the replay names it, or after
  // a timeout when the server refused it.
  const [pendingDay, setPendingDay] = useState<string | null>(null);
  if (pendingDay && replayDay === pendingDay) setPendingDay(null);
  useEffect(() => {
    if (!pendingDay) return;
    const id = setTimeout(() => setPendingDay(null), PENDING_DAY_TIMEOUT_MS);
    return () => clearTimeout(id);
  }, [pendingDay]);
  const loadingDay = pendingDay && pendingDay !== replayDay ? pendingDay : null;
  // The day's Runners entries: the scrubber's alert dots and the Prev/Next alert jumps.
  const replayAlerts = useReplayAlerts(token, replayDay ?? clockDay);
  // The compact replay bar's "more" sheet, remembered on this phone (a device-local preference,
  // never synced: another phone or the desktop has its own room for it).
  const [barExpanded, setBarExpanded] = useState(false);
  useEffect(() => {
    let mounted = true;
    getPreference<boolean>(REPLAY_BAR_EXPANDED_KEY)
      .then((v) => mounted && v === true && setBarExpanded(true))
      .catch(() => {});
    return () => {
      mounted = false;
    };
  }, []);
  const changeBarExpanded = (v: boolean) => {
    setBarExpanded(v);
    void setPreference(REPLAY_BAR_EXPANDED_KEY, v).catch(() => {});
  };

  return (
    // No bottom inset: the main nav's tab bar sits below this screen and takes the home-indicator inset.
    <View style={[styles.screen, { paddingTop: insets.top + 4, paddingBottom: 4 }]}>
      <View style={styles.header}>
        <Pressable accessibilityRole="button" accessibilityLabel={`${active ?? "No ticker"}, search tickers`} accessibilityHint="Tap, then press and hold to copy" {...tickerPress} hitSlop={8} style={({ pressed }) => [styles.tickerButton, pressed && { opacity: 0.6 }]}>
          <Text style={styles.symbol}>{active ?? "—"}</Text>
          <Icon name="chevronDown" color={colors.textDim} size={10} />
        </Pressable>
        <Text style={styles.price}>{last ? formatPrice(last.c) : "—"}</Text>
        <Text style={[styles.change, { color: change.startsWith("-") ? colors.critical : colors.good }]}>{change}</Text>
        {active && <StarToggle symbol={active} />}
        {/* Time and sales (its button left the replay row in the two-tier redesign; the bar's own
            ticker button is the search now, beside the header ticker's tap). */}
        <Pressable accessibilityRole="button" accessibilityLabel="Time and sales" accessibilityState={{ expanded: tapeOpen }} onPress={() => app.setTapeOpen(true)} hitSlop={6} style={({ pressed }) => [styles.tapeButton, (pressed || tapeOpen) && { backgroundColor: colors.bgRow }]}>
          <Icon name="menu" color={colors.textDim} size={15} />
        </Pressable>
        {/* The dashboard's cards, on the cover screen (unfolded they sit beside the chart). */}
        {!panelsBeside && (
          <Pressable accessibilityRole="button" accessibilityLabel="Practice panels" accessibilityState={{ expanded: panelsOpen }} onPress={() => setPanelsOpen(true)} hitSlop={6} style={({ pressed }) => [styles.roundButton, (pressed || panelsOpen) && { backgroundColor: colors.bgRow }]}>
            <Icon name="dashboard" color={panelsOpen ? colors.accent : colors.textDim} size={15} />
          </Pressable>
        )}
        {/* The practice marker (owner, 2026-11-23: never to be mistaken for live): the replay clock
            and "Practice" in the quiet neutral chip; offline still says so in words. */}
        <View
          style={[styles.chip, offline ? { backgroundColor: colors.criticalBg } : live ? { backgroundColor: colors.goodBg } : null]}
          accessible
          accessibilityRole="image"
          accessibilityLabel={offline ? `Connection ${status}` : live ? "Live market, simulated trading" : `Practice${replayDay ? `, ${formatDayKey(replayDay)}` : ""}: replay, not live`}
        >
          {offline ? (
            <Text style={[styles.chipText, { color: colors.critical }]}>{status}</Text>
          ) : (
            <>
              <Icon name={live ? "live" : "replay"} color={live ? colors.good : colors.textDim} size={15} />
              {!live && <Text style={styles.chipText}>Practice</Text>}
            </>
          )}
        </View>
        {/* Settings (owner request 2026-10-18), at the header's right edge as on the web: the search button's
            round 32 pt chip, neutral, raised while its sheet is open. */}
        <Pressable
          accessibilityRole="button"
          accessibilityLabel="Settings"
          accessibilityState={{ expanded: settingsOpen }}
          onPress={() => app.setSettingsOpen(true)}
          hitSlop={6}
          style={({ pressed }) => [styles.roundButton, (pressed || settingsOpen) && { backgroundColor: colors.bgRow }]}
        >
          <Icon name="settings" color={settingsOpen ? colors.accent : colors.textDim} size={15} />
        </Pressable>
      </View>

      <ZoneNote note={layers.shelves ? zones.note : null} reducedMotion={reducedMotion} gap={SCREEN_GAP} style={styles.zone} />

      {/* Only with the detector on and an episode for this ticker (see TickerChartScreen). */}
      {detection.enabled === true && detectionEpisode ? (
      <View style={styles.detectionSummary}>
        <Text style={styles.detectionMeta}>Detection · experimental · {detection.frame?.source ?? "replay source pending"} · {detection.snapshot?.fidelity.replace(/_/g, " ") ?? "fidelity pending"} · canonical 15s{detection.connected ? "" : " · current assessment unavailable"}</Text>
        <Pressable accessibilityRole="button" accessibilityLabel="Inspect the chart's authoritative detection episode" onPress={() => setDetectionDetail({ id: detectionEpisode.episode_id, epoch: detection.frame!.runEpoch })} style={styles.detectionButton}><Text style={styles.detectionState}>{detectionFamilyLabel(detectionEpisode.family)} · {detectionStateLabel(detectionEpisode.state)} · details</Text></Pressable>
      </View>
      ) : null}

      <View style={styles.body}>
      <CandleChart
        style={styles.chart}
        toolbar
        inspector
        symbol={active ?? ""}
        series={series}
        timeframe={timeframe}
        inspectT={inspectT}
        onInspectChange={app.setInspectT}
        settings={chartSettings}
        onSettingsChange={app.setChartSettings}
        settingsFromRemote={chartSettingsFromRemote}
        drawingSettings={drawingSettings}
        indicators={indicators}
        onIndicatorsChange={app.setIndicators}
        indicatorColorActions={app.indicatorColorActions}
        serverIndicators={serverIndicators}
        macdStyles={macdStyles}
        onIndicatorHistory={loadIndicatorHistory}
        serverLevels={serverLevels}
        serverZones={serverZones}
        drawings={active ? drawings : []}
        onDrawingsChange={app.setDrawings}
        trading={trading}
        tradeQty={tradeQty}
        onTradeQtyChange={app.setTradeQty}
        toolboxPlacement={toolbox}
        onTimeframeChange={app.setTimeframe}
        toolsVisible={toolsVisible}
        onToolsVisibleChange={app.setToolsVisible}
        layers={layers}
        onLayersChange={app.setLayers}
        now={clock?.now ?? null}
        onToolboxPlacementChange={app.setToolbox}
        paneHeights={paneHeights}
        onPaneHeightsChange={app.setPaneHeights}
        profileBins={PROFILE_BINS}
        profileBin={bin}
        onProfileBinChange={app.setBin}
        // The session volume profile sits inside the chart on a phone (no room for a side panel), as on the web's phone layout.
        options={{ priceAxisWidth: 52, volumeHeight: 40, timeAxisHeight: 18, paneGap: 4, fontSize: 10, timeLabelSpacing: 60, indicatorPaneHeight: 56, showProfile: true, profileWidth: 0.2 }}
        barSpacing={8}
        label={`${active ?? ""} ${timeframe} candles`}
        detection={detectionAnnotations}
        volumeShelves={zones.bands}
      />
      {panelsBeside && <PracticePanels feed={feed} onPick={app.openTicker} reducedMotion={reducedMotion} style={styles.side} />}
      </View>

      {/* The compact replay bar (owner, 2026-10-24: the two-tier bar took half the cover screen):
          one row, the thin scrubber over it on the cover screen and inside it unfolded
          (chart-native ReplayBar, by width); "more" opens the rest in a sheet over the chart. */}
      <ReplayBar
        width={windowWidth - 2 * SCREEN_PAD}
        expanded={barExpanded}
        onExpandedChange={changeBarExpanded}
        clock={clock}
        onPlayPause={(play) => feed.control({ action: play ? "play" : "pause" })}
        onSpeedChange={(value) => feed.control({ action: "speed", value })}
        onSeek={(t) => feed.control({ action: "seek", value: t })}
        bands={bands}
        candles={series?.candles ?? NO_CANDLES}
        symbol={active ?? ""}
        price={last ? formatPrice(last.c) : ""}
        // The ticker search sheet; its pick (app.openTicker) replaces this tab's chart.
        onTickerPress={() => app.setSearchOpen(true)}
        dayLabel={replayDay ? (dayOptions.find((d) => d.value === replayDay)?.label ?? formatDayKey(replayDay)) : null}
        loadingDay={loadingDay ? formatDayKey(loadingDay) : null}
        onDayPress={
          canPickDay
            ? () => {
                replayDays.refresh();
                app.setDaysOpen(true);
              }
            : undefined
        }
        alerts={replayAlerts}
        // An "any ticker" alert jump: that ticker replaces the chart, as a search pick would.
        onOpenTicker={app.openTicker}
        reducedMotion={reducedMotion}
      />
      <TickerSearchSheet
        open={searchOpen}
        onClose={() => app.setSearchOpen(false)}
        symbols={candidates}
        quotes={feed.quotes}
        search={feed.quoteSearch}
        mock={fixture}
        current={active ?? ""}
        recents={recents}
        onSelect={app.openTicker}
        reducedMotion={reducedMotion}
      />
      <TapeSheet open={tapeOpen} onClose={() => app.setTapeOpen(false)} trades={tape} note={active ?? undefined} />
      <ReplayDaySheet
        open={daysOpen}
        onClose={() => app.setDaysOpen(false)}
        days={dayOptions}
        day={replayDay}
        // The server rebuilds on that day and resends every snapshot; the clock's new trading day
        // then refetches the candidates and the day list.
        onSelect={(day) => {
          setPendingDay(day);
          feed.control({ action: "day", value: day });
        }}
        reducedMotion={reducedMotion}
      />
      {!panelsBeside && (
        <Sheet open={panelsOpen} onClose={() => setPanelsOpen(false)} title={`Practice${replayDay ? ` · ${formatDayKey(replayDay)}` : ""}`} reducedMotion={reducedMotion} tall>
          <PracticePanels
            feed={feed}
            onPick={(symbol) => {
              setPanelsOpen(false);
              app.openTicker(symbol);
            }}
            reducedMotion={reducedMotion}
          />
        </Sheet>
      )}
      <Sheet open={detectionDetail !== null} onClose={() => setDetectionDetail(null)} title="Replay detection details" reducedMotion={reducedMotion} tall>
        <ScrollView showsVerticalScrollIndicator={false} showsHorizontalScrollIndicator={false}>
          {detailedEpisode && detection.snapshot ? <DetectionEpisodeDetails episode={detailedEpisode} snapshot={detection.snapshot} currentAvailable={detection.connected && !detection.resyncing} /> : <Text style={styles.detectionMeta}>The replay run changed or the selected episode left bounded history. Select a current detection to inspect.</Text>}
        </ScrollView>
      </Sheet>
    </View>
  );
}

const styles = StyleSheet.create({
  screen: { flex: 1, backgroundColor: colors.bg, paddingHorizontal: SCREEN_PAD, gap: SCREEN_GAP },
  zone: { paddingHorizontal: 2 },
  header: { flexDirection: "row", alignItems: "center", gap: 8 },
  tickerButton: { flexDirection: "row", alignItems: "center", gap: 4 },
  tapeButton: { marginLeft: "auto", width: 32, height: 32, borderRadius: 16, alignItems: "center", justifyContent: "center", backgroundColor: colors.bgPanel },
  roundButton: { width: 32, height: 32, borderRadius: 16, alignItems: "center", justifyContent: "center", backgroundColor: colors.bgPanel },
  symbol: { color: colors.text, fontFamily: fonts.semibold, fontSize: 16 },
  price: { color: colors.text, fontSize: 16, fontVariant: ["tabular-nums"] },
  change: { fontSize: 11, fontVariant: ["tabular-nums"] },
  chip: { flexDirection: "row", alignItems: "center", gap: 6, backgroundColor: colors.bgPanel, borderRadius: 999, paddingHorizontal: 10, paddingVertical: 4, alignSelf: "center" },
  chipText: { color: colors.textDim, fontSize: 11, fontWeight: "500" },
  // The chart, and unfolded the cards beside it (about 40 % of the width, at most 380 pt).
  body: { flex: 1, flexDirection: "row", gap: SCREEN_GAP },
  chart: { flex: 1 },
  side: { flex: 0, width: "40%", maxWidth: 380 },
  detectionSummary: { gap: 2 },
  detectionMeta: { color: colors.textFaint, fontFamily: fonts.regular, fontSize: 10, lineHeight: 14 },
  detectionButton: { minHeight: 34, justifyContent: "center" },
  detectionState: { color: colors.accent, fontFamily: fonts.semibold, fontSize: 11 },
});

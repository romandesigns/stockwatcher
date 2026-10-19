import {
  buildSlots,
  changeReference,
  currentReplayDay,
  formatDayKey,
  replayDayOptions,
  formatChange,
  formatEtTime,
  formatPrice,
  lastCandle,
  sessionOf,
  formatEtDateKey,
  sessionBands,
  tradingDayOf,
  volumeBuckets,
} from "@stockwatcher/chart-core";
import { PROFILE_BINS } from "@stockwatcher/app-state";
import { dataSourceSection } from "../settings/dataSource";
import { CandleChart, chartSettingsSection, drawingSettingsSection, Icon, ReplayDaySheet, SessionTimeline, SettingsSheet, TapeSheet, ToolButton, UI } from "@stockwatcher/chart-native";
import type { ConnectionStatus } from "@stockwatcher/shared";
import { useLayoutEffect, useMemo, useState } from "react";
import Animated, { FadeIn, FadeOut, ReduceMotion } from "react-native-reanimated";
import { Pressable, StyleSheet, Text, View } from "react-native";
import { useSafeAreaInsets } from "react-native-safe-area-context";

import { useCandidates, useChartFeed, usePaperTrading, useReplayDays } from "../feed/hooks";
import type { MobileFeed } from "../feed/serverFeed";
import { useConnection } from "../feed/useConnection";
import { useReducedMotion } from "../motion/useReducedMotion";
import { TickerSearchSheet } from "../search/TickerSearchSheet";
import { appActions, useApp } from "../state/store";
import { useTickerPress } from "../ui/useTickerPress";
import { SignInScreen } from "./SignInScreen";
import { colors, fonts } from "../theme";

const SPEEDS = [1, 2, 10, 60, 0] as const;
/** Timeline minimap bucket: 20 minutes, as on the web's phone layout. */
const bucketMs = 20 * 60_000;
const speedLabel = (s: number) => (s === 0 ? "Max" : `${s}×`);
const SESSION = { overnight: "Overnight", pre: "Premarket", regular: "Regular", after: "After hours" } as const;
const SESSION_A11Y = { overnight: "overnight session", pre: "premarket", regular: "regular session", after: "after hours" } as const;

/** The chart screen: connects once (useConnection: token, app store, its own socket), then renders the chart. */
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
  // The trading day the replay clock is on; it changes on a day switch (and once a night while playing).
  const [clockDay, setClockDay] = useState<string | null>(null);
  const { symbols: candidates, fixture } = useCandidates(token, clockDay, "replay");
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
  const dashboardSource = useApp((s) => s.dashboard.source);
  // The store on the phone always has a device slice (loadAppStore); the fallbacks only satisfy the type.
  const toolbox = useApp((s) => s.device?.toolbox);
  const paneHeights = useApp((s) => s.device?.paneHeights);
  const toolsVisible = useApp((s) => s.device?.toolsVisible ?? true);

  // Follow the price band: a symbol that leaves it is no longer let through. The fallback is not
  // written back (a phone with a momentarily stale list must not move every other device's chart).
  const active = symbol && candidates.includes(symbol) ? symbol : (candidates[0] ?? null);
  // Drawings follow the shown symbol; before paint, so a ticker switch never flashes the old marks.
  useLayoutEffect(() => app.showSymbol(active ?? ""), [app, active]);
  const bin = useApp((s) => s.bin);
  const tradeQty = useApp((s) => s.tradeQty);
  const recents = useApp((s) => s.searchRecents);
  const { series, clock, tape } = useChartFeed(feed, active ?? "", timeframe, bin);
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
    return sessionBands({ year, month, day });
  }, [dayKey]);
  const buckets = useMemo(() => (series && bands.length ? volumeBuckets(series.candles, bands[0]!.start, bands[3]!.end, bucketMs) : []), [series, bands]);
  const trading = usePaperTrading(feed);

  const slots = useMemo(() => (series ? buildSlots(series.candles, series.gaps) : []), [series]);
  const last = lastCandle(slots);
  // Previous close when the server knows it, else the anchor session's first candle: the web
  // header's and the search rows' rule (changeReference in chart-core).
  const reference = useMemo(() => changeReference(series), [series]);
  const change = last && reference ? formatChange(reference.price, last.c) : "";

  const playing = !!clock?.playing;
  const live = clock?.mode === "live";
  const speed = clock?.speed ?? 1;
  const offline = status !== "open";
  const replayDay = currentReplayDay(replayDays.state, clock);
  // The date is the day picker only when there is something to pick (replay mode, days offered).
  const canPickDay = !live && !!clock && !!replayDay && dayOptions.length > 0;

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
        <Pressable accessibilityRole="button" accessibilityLabel="Search tickers" onPress={() => app.setSearchOpen(true)} hitSlop={6} style={({ pressed }) => [styles.searchButton, pressed && { backgroundColor: colors.bgRow }]}>
          <Icon name="search" color={colors.textDim} size={15} />
        </Pressable>
        {/* An icon, not words (owner, 2026-10-17): replay clock or live broadcast; offline still says so in words. */}
        <View
          style={[styles.chip, offline ? { backgroundColor: colors.criticalBg } : live ? { backgroundColor: colors.goodBg } : null]}
          accessible
          accessibilityRole="image"
          accessibilityLabel={offline ? `Connection ${status}` : `${live ? "Live market" : "Replay"}, simulated trading`}
        >
          {offline ? (
            <Text style={[styles.chipText, { color: colors.critical }]}>{status}</Text>
          ) : (
            <Icon name={live ? "live" : "replay"} color={live ? colors.good : colors.textDim} size={15} />
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
      />


      <View style={styles.replayCard}>
      <View style={styles.replay}>
        <ToolButton primary label={playing ? "Pause replay" : "Play replay"} disabled={!clock || live} onPress={() => feed.control({ action: playing ? "pause" : "play" })} style={styles.play}>
          <Icon name={playing ? "pause" : "play"} color={UI.accentFg} />
        </ToolButton>
        {/* The replay clock (Eastern) over its session and the replay date. The date is the day
            picker: there is no room for another button in this row, and the date is what it changes. */}
        <View style={styles.clockCol}>
          <Text style={styles.clock} accessibilityLabel={clock ? `${formatEtTime(clock.now, true)} Eastern, ${SESSION_A11Y[sessionOf(clock.now)]}` : "Clock not started"}>
            {clock ? formatEtTime(clock.now, true) : "--:--:--"}
          </Text>
          <View style={styles.subRow}>
            <Text style={styles.session} numberOfLines={1}>
              {clock ? SESSION[sessionOf(clock.now)] : ""}
              {clock && replayDay && !canPickDay ? ` · ${formatDayKey(replayDay)}` : ""}
            </Text>
            {canPickDay && replayDay && (
              <Pressable
                accessibilityRole="button"
                accessibilityLabel={`Replay day ${formatDayKey(replayDay)}, change`}
                hitSlop={{ top: 10, bottom: 10, left: 4, right: 8 }}
                onPress={() => {
                  replayDays.refresh();
                  app.setDaysOpen(true);
                }}
                style={({ pressed }) => [styles.dayButton, pressed && { backgroundColor: UI.row }]}
              >
                <Text style={styles.session}>· {formatDayKey(replayDay)}</Text>
                <Icon name="chevronDown" color={colors.textFaint} size={8} />
              </Pressable>
            )}
          </View>
        </View>
        {/* Step only does something while paused, so it only shows then. It sits beside the speed button,
            in space the clock does not use, so nothing shifts when it appears. */}
        {!playing && !live && clock && (
          <Animated.View entering={FadeIn.duration(150).reduceMotion(ReduceMotion.System)} exiting={FadeOut.duration(120).reduceMotion(ReduceMotion.System)} style={styles.stepWrap}>
            <ToolButton label="Step one event" onPress={() => feed.control({ action: "step" })} style={styles.step}>
              <Icon name="step" color={colors.textDim} size={14} />
            </ToolButton>
          </Animated.View>
        )}
        <ToolButton
          label={`Replay speed ${speedLabel(speed)}, change`}
          disabled={!clock || live}
          onPress={() => {
            const i = SPEEDS.indexOf(speed as (typeof SPEEDS)[number]);
            feed.control({ action: "speed", value: SPEEDS[(i + 1) % SPEEDS.length] });
          }}
          style={styles.speed}
        >
          {speedLabel(speed)}
        </ToolButton>
        <ToolButton label="Time and sales" onPress={() => app.setTapeOpen(true)} style={styles.tapeButton}>
          <Icon name="menu" color={colors.textDim} />
        </ToolButton>
      </View>
      {/* Tap a time to jump the replay there. */}
      <SessionTimeline variant="minimal" bands={bands} buckets={buckets} bucketMs={bucketMs} now={clock?.now ?? null} onSeek={live ? undefined : (t) => feed.control({ action: "seek", value: t })} />
      </View>
      <TickerSearchSheet
        open={searchOpen}
        onClose={() => app.setSearchOpen(false)}
        symbols={candidates}
        quotes={feed.quotes}
        mock={fixture}
        current={active ?? ""}
        recents={recents}
        onSelect={app.openTicker}
        reducedMotion={reducedMotion}
      />
      <SettingsSheet open={settingsOpen} onClose={() => app.setSettingsOpen(false)} sections={[dataSourceSection(dashboardSource, app.setDashboardSource), chartSettingsSection(chartSettings, app.setChartSettings), drawingSettingsSection(drawingSettings, app.setDrawingSettings, reducedMotion)]} reducedMotion={reducedMotion} />
      <TapeSheet open={tapeOpen} onClose={() => app.setTapeOpen(false)} trades={tape} note={active ?? undefined} />
      <ReplayDaySheet
        open={daysOpen}
        onClose={() => app.setDaysOpen(false)}
        days={dayOptions}
        day={replayDay}
        // The server rebuilds on that day and resends every snapshot; the clock's new trading day
        // then refetches the candidates and the day list.
        onSelect={(day) => feed.control({ action: "day", value: day })}
        reducedMotion={reducedMotion}
      />
    </View>
  );
}

const styles = StyleSheet.create({
  screen: { flex: 1, backgroundColor: colors.bg, paddingHorizontal: 6, gap: 8 },
  header: { flexDirection: "row", alignItems: "center", gap: 8 },
  tickerButton: { flexDirection: "row", alignItems: "center", gap: 4 },
  searchButton: { marginLeft: "auto", width: 32, height: 32, borderRadius: 16, alignItems: "center", justifyContent: "center", backgroundColor: colors.bgPanel },
  roundButton: { width: 32, height: 32, borderRadius: 16, alignItems: "center", justifyContent: "center", backgroundColor: colors.bgPanel },
  stepWrap: {},
  step: { width: 36, height: 36, paddingHorizontal: 0, backgroundColor: UI.row },
  symbol: { color: colors.text, fontFamily: fonts.semibold, fontSize: 16 },
  price: { color: colors.text, fontSize: 16, fontVariant: ["tabular-nums"] },
  change: { fontSize: 11, fontVariant: ["tabular-nums"] },
  chip: { flexDirection: "row", alignItems: "center", gap: 6, backgroundColor: colors.bgPanel, borderRadius: 999, paddingHorizontal: 10, paddingVertical: 4, alignSelf: "center" },
  chipText: { color: colors.textDim, fontSize: 11, fontWeight: "500" },
  chart: { flex: 1 },
  replayCard: { backgroundColor: UI.panel, borderRadius: 12, paddingHorizontal: 10, paddingVertical: 8, gap: 6 },
  replay: { flexDirection: "row", alignItems: "center", gap: 8 },
  tapeButton: { backgroundColor: UI.row, height: 36, width: 36, paddingHorizontal: 0 },
  play: { width: 36, height: 36, paddingHorizontal: 0 },
  // The clock takes the free width, so step, speed and tape stay pinned right.
  clockCol: { flex: 1, minWidth: 0 },
  clock: { color: colors.text, fontSize: 14, lineHeight: 17, fontVariant: ["tabular-nums"] },
  subRow: { flexDirection: "row", alignItems: "center" },
  session: { color: colors.textFaint, fontSize: 10, lineHeight: 13 },
  // Pressed shows the row colour (no border); the padding is the touch target, the hitSlop the rest.
  dayButton: { flexDirection: "row", alignItems: "center", gap: 2, marginLeft: 3, paddingHorizontal: 3, borderRadius: 4 },
  speed: { backgroundColor: UI.row, height: 36, minWidth: 48 },
});

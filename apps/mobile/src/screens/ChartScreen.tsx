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
  parseToolboxPlacement,
  parseLayers,
  DEFAULT_TOOLBOX_PHONE,
  parseChartSettings,
  parseDrawingSettings,
  parseIndicators,
  SYNC_NS,
  type ToolboxPlacement,
  type Timeframe,
} from "@stockwatcher/chart-core";
import { CandleChart, chartSettingsSection, drawingSettingsSection, Icon, ReplayDaySheet, SessionTimeline, SettingsSheet, TapeSheet, ToolButton, UI } from "@stockwatcher/chart-native";
import type { ConnectionStatus, SettingsSync } from "@stockwatcher/shared";
import { useEffect, useMemo, useState } from "react";
import Animated, { FadeIn, FadeOut, ReduceMotion } from "react-native-reanimated";
import { Pressable, StyleSheet, Text, View } from "react-native";
import { useSafeAreaInsets } from "react-native-safe-area-context";

import { useCandidates, useChartFeed, usePaperTrading, useReplayDays } from "../feed/hooks";
import { createMobileFeed, type MobileFeed } from "../feed/serverFeed";
import { useReducedMotion } from "../motion/useReducedMotion";
import { TickerSearchSheet } from "../search/TickerSearchSheet";
import { getPreference, setPreference } from "../storage/preferences";
import { getAccessToken, setAccessToken } from "../storage/secure";
import { loadSettingsSync } from "../settings/sync";
import { useSynced, useSyncedDrawings } from "../settings/useSynced";
import { SignInScreen } from "./SignInScreen";
import { colors, fonts } from "../theme";

const SPEEDS = [1, 2, 10, 60, 0] as const;
/** Timeline minimap bucket: 20 minutes, as on the web's phone layout. */
const bucketMs = 20 * 60_000;
const speedLabel = (s: number) => (s === 0 ? "Max" : `${s}×`);
/** Profile bin widths, as on the web's profile panel. */
const BINS = [0.01, 0.02, 0.05] as const;
const SESSION = { overnight: "Overnight", pre: "Premarket", regular: "Regular", after: "After hours" } as const;
const SESSION_A11Y = { overnight: "overnight session", pre: "premarket", regular: "regular session", after: "after hours" } as const;

/** The chart screen: connects once (token from the keystore), then renders the chart. */
export function ChartScreen() {
  const [ready, setReady] = useState<{ feed: MobileFeed; token: string | null; sync: SettingsSync } | null>(null);
  const [status, setStatus] = useState<ConnectionStatus>("connecting");
  // Each sign-in restarts the connection with the new token.
  const [attempt, setAttempt] = useState(0);
  useEffect(() => {
    let feed: MobileFeed | null = null;
    let alive = true;
    // The synced settings load with the token, so the chart opens with the last known settings
    // (and the indicators, levels and drawings already on it) before the socket connects.
    void Promise.all([getAccessToken().catch(() => null), loadSettingsSync()]).then(([token, sync]) => {
      if (!alive) return;
      feed = createMobileFeed(token, setStatus, sync);
      setReady({ feed, token, sync });
    });
    return () => {
      alive = false;
      feed?.close();
    };
  }, [attempt]);
  if (!ready) return <View style={styles.screen} />;
  // The server refused the hello: no token yet, or not this one (production requires one).
  if (status === "unauthorized") {
    return (
      <SignInScreen
        rejected={ready.token !== null}
        onSubmit={(t) => {
          void setAccessToken(t).then(() => {
            setStatus("connecting");
            setAttempt((n) => n + 1);
          });
        }}
      />
    );
  }
  return <ChartView feed={ready.feed} token={ready.token} status={status} sync={ready.sync} />;
}

function ChartView({ feed, token, status, sync }: { feed: MobileFeed; token: string | null; status: ConnectionStatus; sync: SettingsSync }) {
  const insets = useSafeAreaInsets();
  // The trading day the replay clock is on; it changes on a day switch (and once a night while playing).
  const [clockDay, setClockDay] = useState<string | null>(null);
  const { symbols: candidates, fixture } = useCandidates(token, clockDay);
  const replayDays = useReplayDays(token, clockDay);
  const dayOptions = useMemo(() => replayDayOptions(replayDays.state?.days ?? []), [replayDays.state]);
  const [daysOpen, setDaysOpen] = useState(false);
  const [symbol, setSymbol] = useState<string | null>(null);
  const [timeframe, setTimeframe] = useState<Timeframe>("1m");
  const [inspectT, setInspectT] = useState<number | null>(null);
  // The user's preferences, synced live with every other client (desktop, web, other phones) and
  // kept on the server (src/settings/sync.ts): chart settings (type, scale, sessions, zoom),
  // which buttons the drawing toolbox shows (the Settings sheet's Drawing tab), the indicators
  // and the context layers. This symbol's drawings are synced below, once the symbol is known.
  const [chartSettings, changeChartSettings, chartSettingsOrigin] = useSynced(sync, SYNC_NS.chartSettings, parseChartSettings);
  const [drawingSettings, changeDrawingSettings] = useSynced(sync, SYNC_NS.drawingSettings, parseDrawingSettings);
  const [indicators, changeIndicators] = useSynced(sync, SYNC_NS.indicators, parseIndicators);
  const [layers, changeLayers] = useSynced(sync, SYNC_NS.layers, parseLayers);
  const [settingsOpen, setSettingsOpen] = useState(false);

  // Device-local, kept across launches on this phone only (each screen's own layout):
  // where the floating drawing tools were left.
  const [toolbox, setToolbox] = useState<ToolboxPlacement>(DEFAULT_TOOLBOX_PHONE);
  useEffect(() => {
    void getPreference<unknown>("chart.toolbox").then((v) => v && setToolbox(parseToolboxPlacement(v, DEFAULT_TOOLBOX_PHONE)));
  }, []);
  // Oscillator pane heights (dragged or expanded), kept across launches.
  const [paneHeights, setPaneHeights] = useState<Record<string, number>>({});
  useEffect(() => {
    void getPreference<Record<string, number>>("chart.paneHeights").then((v) => v && typeof v === "object" && setPaneHeights(v));
  }, []);
  const changePaneHeights = (h: Record<string, number>) => {
    setPaneHeights(h);
    void setPreference("chart.paneHeights", h);
  };
  // Drawing tools shown or hidden, kept across launches.
  const [toolsVisible, setToolsVisible] = useState(true);
  useEffect(() => {
    void getPreference<boolean>("chart.toolsVisible").then((v) => v === false && setToolsVisible(false));
  }, []);
  const changeToolsVisible = (v: boolean) => {
    setToolsVisible(v);
    void setPreference("chart.toolsVisible", v);
  };
  const changeToolbox = (p: ToolboxPlacement) => {
    setToolbox(p);
    void setPreference("chart.toolbox", p);
  };

  // Follow the price band: a symbol that leaves it is no longer let through.
  const active = symbol && candidates.includes(symbol) ? symbol : (candidates[0] ?? null);
  const [bin, setBin] = useState<number>(BINS[0]);
  const { series, clock, tape } = useChartFeed(feed, active ?? "", timeframe, bin);
  // This symbol's drawings, synced per drawing (another device's edit of a different drawing is never overwritten).
  const [drawings, changeDrawings] = useSyncedDrawings(sync, active ?? "");
  const [tapeOpen, setTapeOpen] = useState(false);
  // Ticker quick-open, the web's Ctrl+P: tap the ticker (or the search icon) in the header.
  const [searchOpen, setSearchOpen] = useState(false);
  const reducedMotion = useReducedMotion();

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
    <View style={[styles.screen, { paddingTop: insets.top + 8, paddingBottom: insets.bottom + 8 }]}>
      <View style={styles.header}>
        <Pressable accessibilityRole="button" accessibilityLabel={`${active ?? "No ticker"}, search tickers`} onPress={() => setSearchOpen(true)} hitSlop={8} style={({ pressed }) => [styles.tickerButton, pressed && { opacity: 0.6 }]}>
          <Text style={styles.symbol}>{active ?? "—"}</Text>
          <Icon name="chevronDown" color={colors.textDim} size={10} />
        </Pressable>
        <Text style={styles.price}>{last ? formatPrice(last.c) : "—"}</Text>
        <Text style={[styles.change, { color: change.startsWith("-") ? colors.critical : colors.good }]}>{change}</Text>
        <Pressable accessibilityRole="button" accessibilityLabel="Search tickers" onPress={() => setSearchOpen(true)} hitSlop={6} style={({ pressed }) => [styles.searchButton, pressed && { backgroundColor: colors.bgRow }]}>
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
          onPress={() => setSettingsOpen(true)}
          hitSlop={6}
          style={({ pressed }) => [styles.roundButton, (pressed || settingsOpen) && { backgroundColor: colors.bgRow }]}
        >
          <Icon name="settings" color={colors.textDim} size={15} />
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
        onInspectChange={setInspectT}
        settings={chartSettings}
        onSettingsChange={changeChartSettings}
        settingsFromRemote={chartSettingsOrigin === "remote"}
        drawingSettings={drawingSettings}
        indicators={indicators}
        onIndicatorsChange={changeIndicators}
        drawings={active ? drawings : []}
        onDrawingsChange={changeDrawings}
        trading={trading}
        toolboxPlacement={toolbox}
        onTimeframeChange={(tf) => {
          setTimeframe(tf);
          setInspectT(null);
        }}
        toolsVisible={toolsVisible}
        onToolsVisibleChange={changeToolsVisible}
        layers={layers}
        onLayersChange={changeLayers}
        now={clock?.now ?? null}
        onToolboxPlacementChange={changeToolbox}
        paneHeights={paneHeights}
        onPaneHeightsChange={changePaneHeights}
        profileBins={BINS}
        profileBin={bin}
        onProfileBinChange={setBin}
        // The session volume profile sits inside the chart on a phone (no room for a side panel), as on the web's phone layout.
        options={{ priceAxisWidth: 46, volumeHeight: 40, timeAxisHeight: 18, paneGap: 4, fontSize: 9, timeLabelSpacing: 60, indicatorPaneHeight: 56, showProfile: true, profileWidth: 0.2 }}
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
                  setDaysOpen(true);
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
        <ToolButton label="Time and sales" onPress={() => setTapeOpen(true)} style={styles.tapeButton}>
          <Icon name="menu" color={colors.textDim} />
        </ToolButton>
      </View>
      {/* Tap a time to jump the replay there. */}
      <SessionTimeline variant="minimal" bands={bands} buckets={buckets} bucketMs={bucketMs} now={clock?.now ?? null} onSeek={live ? undefined : (t) => feed.control({ action: "seek", value: t })} />
      </View>
      <TickerSearchSheet
        open={searchOpen}
        onClose={() => setSearchOpen(false)}
        symbols={candidates}
        quotes={feed.quotes}
        mock={fixture}
        current={active ?? ""}
        onSelect={(sym) => {
          setSymbol(sym);
          setInspectT(null);
        }}
        reducedMotion={reducedMotion}
      />
      <SettingsSheet open={settingsOpen} onClose={() => setSettingsOpen(false)} sections={[chartSettingsSection(chartSettings, changeChartSettings), drawingSettingsSection(drawingSettings, changeDrawingSettings, reducedMotion)]} reducedMotion={reducedMotion} />
      <TapeSheet open={tapeOpen} onClose={() => setTapeOpen(false)} trades={tape} note={active ?? undefined} />
      <ReplayDaySheet
        open={daysOpen}
        onClose={() => setDaysOpen(false)}
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
  screen: { flex: 1, backgroundColor: colors.bg, paddingHorizontal: 12, gap: 8 },
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

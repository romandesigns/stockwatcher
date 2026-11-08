import { dayChartContext, DEFAULT_CHART_SETTINGS, DEFAULT_LAYERS, formatEtTime12, INDICATOR_PLACEMENT, indicatorDemand, tickerIndicatorValues, type ChartLayers, type ChartSettings, type EventMarker, type IndicatorSpec } from "@stockwatcher/chart-core";
import { CandleChart, Icon, UI } from "@stockwatcher/chart-native";
import { NO_TICKER, tickerPageView, type TickerAlertView, type TickerChannel, type TickerMissedView, type TickerPackageWire, type TickerPageView, type TickerState, type Tone } from "@stockwatcher/shared";
import { useNavigation, type RouteProp } from "@react-navigation/native";
import { useCallback, useEffect, useMemo, useState, useSyncExternalStore, type ReactNode } from "react";
import { Pressable, ScrollView, StyleSheet, Text, View, type LayoutChangeEvent } from "react-native";
import { useSafeAreaInsets } from "react-native-safe-area-context";

import { useDashboardFeed } from "../feed/dashboardFeed";
import { Collapse } from "../motion/Collapse";
import { useReducedMotion } from "../motion/useReducedMotion";
import type { RootStackParamList } from "../navigation/RootStack";
import { appActions, useApp } from "../state/store";
import { colors, fonts } from "../theme";

/**
 * The approved design's chart: candles, the day's high and low, the markers. The key-level lines
 * (premarket high/low, HOD/LOD, previous close) are off: their labels sat on top of the numbered
 * markers and the day's own high/low labels, and the page states those levels in its tiles.
 */
const TICKER_LAYERS: ChartLayers = { ...DEFAULT_LAYERS, premarket: false, dayRange: false, prevClose: false };

type Props = { route: RouteProp<RootStackParamList, "InsightsTicker"> };

const NOOP = () => () => {};
const time = (ms: number, seconds?: boolean) => formatEtTime12(ms, seconds);
const TONE: Record<Tone, string> = { neutral: colors.text, up: colors.good, down: colors.critical, dim: colors.textDim };
/** The chart's metrics: the ticker modal's legible set. */
const OPTIONS = { priceAxisWidth: 52, volumeHeight: 32, timeAxisHeight: 18, paneGap: 4, fontSize: 10, timeLabelSpacing: 64 } as const;
/** The chart's tool row above the plot, and each oscillator pane (RSI, MACD) under it: the chart grows by them rather than squeezing the candles. */
const TOOLBAR_H = 36;
const PANE_H = 80;

/** What the page chart's indicators need: the channel to ask the server, its answer, and the list. */
interface ChartIndicators {
  channel: TickerChannel | undefined;
  state: TickerState;
  indicators: IndicatorSpec[];
  onIndicatorsChange: (specs: IndicatorSpec[]) => void;
}

/**
 * The Insights ticker page on the phone (owner, 2026-10-31, approved design "Ticker page ·
 * phone"): the web page stacked for the Fold's cover screen, two columns of tiles unfolded
 * (sw600dp). A pushed screen (`screenPush`: iOS edge swipe, Android back through the same spec);
 * the in-screen back button pops it too. Everything shown is the server's package
 * (crates/server/src/insights) on the dashboard's socket; nothing is computed here.
 */
export function InsightsTickerScreen({ route }: Props) {
  const navigation = useNavigation();
  const insets = useSafeAreaInsets();
  const reducedMotion = useReducedMotion();
  const feed = useDashboardFeed();
  const channel = feed?.ticker;
  const { symbol } = route.params;
  // The day switcher changes the route's own params (no new history entry): back still returns to the log.
  const day = route.params.day;
  const subscribe = useCallback((cb: () => void) => (channel ? channel.subscribe(cb) : NOOP()), [channel]);
  const get = useCallback((): TickerState => (channel ? channel.get() : NO_TICKER), [channel]);
  const state = useSyncExternalStore(subscribe, get, get);
  useEffect(() => (channel ? channel.open({ symbol, day }) : undefined), [channel, symbol, day]);
  const pkg = state.pkg && state.pkg.symbol === symbol && state.pkg.day === day ? state.pkg : null;
  const view = useMemo(() => (pkg ? tickerPageView(pkg, time) : null), [pkg]);
  const [width, setWidth] = useState(0);
  const wide = width >= 600;
  // Held in the app store, not the chart (it remounts with every day): synced
  // (SYNC_NS.tickerIndicators, owner 2026-11-07: indicator settings follow the person to every
  // device, live), apart from the main chart's list. Before that this phone's own preference
  // (TICKER_INDICATORS_LEGACY_KEY), seeded once by the store. Off and on keeps an indicator's settings.
  const indicators = useApp((st) => st.tickerIndicators);
  const [setIndicators] = useState(() => appActions().setTickerIndicators);
  const setDay = (d: string | null) => {
    if (d) navigation.setParams({ symbol, day: d } as never);
  };

  return (
    <View style={[styles.screen, { paddingTop: insets.top + 4 }]} onLayout={(e: LayoutChangeEvent) => setWidth(e.nativeEvent.layout.width)}>
      <View style={styles.top}>
        <Pressable accessibilityRole="button" accessibilityLabel="Back to the notification log" onPress={() => navigation.goBack()} style={({ pressed }) => [styles.square, pressed && { backgroundColor: UI.raised }]}>
          <Icon name="chevronLeft" color={colors.textDim} size={18} />
        </Pressable>
        <View style={{ flexShrink: 1 }}>
          <Text style={styles.symbol} accessibilityRole="header">
            {symbol}
          </Text>
          {view?.name ? (
            <Text style={styles.small} numberOfLines={1}>
              {view.name}
            </Text>
          ) : null}
        </View>
        <View style={styles.days}>
          <DayStep dir="prev" day={view?.prevDay ?? null} onDay={setDay} />
          <View style={styles.dayChip}>
            <Icon name="calendar" color={colors.text} size={14} />
            <Text style={styles.dayText}>{view?.dayShort ?? day}</Text>
          </View>
          <DayStep dir="next" day={view?.nextDay ?? null} onDay={setDay} />
        </View>
      </View>
      <ScrollView showsVerticalScrollIndicator={false} showsHorizontalScrollIndicator={false} contentContainerStyle={{ gap: 16, paddingBottom: insets.bottom + 24, paddingTop: 8 }}>
        {state.available === false ? (
          <Text style={styles.empty}>This server does not build ticker pages yet.</Text>
        ) : !view || !pkg ? (
          <Text style={styles.empty} accessibilityLiveRegion="polite">{`Building ${symbol} for ${day}: the server replays the whole day…`}</Text>
        ) : view.unavailable ? (
          <Text style={styles.empty}>{view.unavailable}</Text>
        ) : (
          <Body pkg={pkg} view={view} wide={wide} reducedMotion={reducedMotion} chart={{ channel, state, indicators, onIndicatorsChange: setIndicators }} />
        )}
      </ScrollView>
    </View>
  );
}

function DayStep({ dir, day, onDay }: { dir: "prev" | "next"; day: string | null; onDay: (d: string | null) => void }) {
  const label = dir === "prev" ? "Previous trading day" : "Next trading day";
  return (
    <Pressable accessibilityRole="button" accessibilityLabel={day ? `${label}, ${day}` : `${label} (none)`} accessibilityState={{ disabled: !day }} disabled={!day} onPress={() => onDay(day)} style={({ pressed }) => [styles.square, !day && { opacity: 0.4 }, pressed && { backgroundColor: UI.raised }]}>
      <Icon name={dir === "prev" ? "chevronLeft" : "chevronRight"} color={colors.textDim} size={16} />
    </Pressable>
  );
}

function Body({ pkg, view, wide, reducedMotion, chart }: { pkg: TickerPackageWire; view: TickerPageView; wide: boolean; reducedMotion: boolean; chart: ChartIndicators }) {
  const [settings, setSettings] = useState<ChartSettings>(() => ({ ...DEFAULT_CHART_SETTINGS, highLow: true }));
  const [layers, setLayers] = useState<ChartLayers>(TICKER_LAYERS);
  // Indicators are computed on the server over this package's bars (`insights.ticker.indicators.v1`),
  // as on the web page; the chart draws them as sent. Absent (an older server) = no lines and the
  // chart's "needs a server update" note; null = asked, not answered yet.
  const { channel, state, indicators, onIndicatorsChange } = chart;
  const demand = useMemo(() => indicatorDemand(indicators, settings), [indicators, settings]);
  useEffect(() => channel?.setIndicators({ indicators: demand.indicators, rth: demand.rth }), [channel, demand]);
  const serverIndicators = useMemo(() => (state.indicatorsAvailable ? tickerIndicatorValues(state.indicators) : undefined), [state.indicatorsAvailable, state.indicators]);
  const panes = settings.visibility.indicators ? indicators.filter((s) => INDICATOR_PLACEMENT[s.kind] === "pane").length : 0;
  // The server's levels, ranges, zones and session schedule over these bars (the package's
  // `chart`); a package from an older server has none, and the chart reads them off its candles.
  const context = useMemo(() => dayChartContext(pkg.chart), [pkg]);
  const series = useMemo(() => ({ candles: pkg.bars, gaps: [], profile: null, halts: pkg.halts?.halts ?? [], prevClose: pkg.stats?.prev_close?.price ?? null, ...(context.sessions ? { sessions: context.sessions } : {}) }), [pkg, context]);
  const serverContext = pkg.chart ? { serverLevels: context.levels, serverZones: settings.extendedHours ? context.zones : context.zonesRth } : {};
  // A tapped history card turns its marker orange until it is tapped again or another is (owner, 2026-11-01).
  const [selected, setSelected] = useState<number | null>(null);
  const onSelect = useCallback((n: number) => setSelected((cur) => (cur === n ? null : n)), []);
  const markers = useMemo<EventMarker[]>(() => view.markers.map((m) => ({ kind: m.kind, at: m.at, price: m.price, n: m.n, selected: m.kind === "alert" && m.n === selected })), [view.markers, selected]);
  const [later, setLater] = useState(false);
  // Tiles: two across on the cover screen, four unfolded (eight stats fill both evenly).
  const tile = wide ? styles.tile4 : styles.tile2;
  return (
    <>
      <View style={styles.chartWrap} accessibilityLabel={`${pkg.symbol} on ${view.dayLabel}, 1-minute candles with alerts numbered and missed setups ringed`}>
        <View style={styles.legend}>
          <Text style={styles.sectionSmall}>{view.dayShort} · 1-minute</Text>
          <View style={{ flex: 1 }} />
          <View style={[styles.dot, { backgroundColor: colors.accent }]} />
          <Text style={styles.note}>Sent</Text>
          <View style={[styles.dot, styles.ring]} />
          <Text style={styles.note}>Missed</Text>
        </View>
        <CandleChart
          key={pkg.day}
          style={{ height: (wide ? 320 : 260) + TOOLBAR_H + panes * PANE_H }}
          symbol={pkg.symbol}
          series={series}
          timeframe="1m"
          settings={settings}
          onSettingsChange={setSettings}
          eventMarkers={markers}
          {...serverContext}
          initialFocus={pkg.focus}
          layers={layers}
          onLayersChange={setLayers}
          toolbar
          // The drawing tools stay folded until the pencil asks for them: this chart is for reading the day.
          defaultToolsVisible={false}
          indicators={indicators}
          onIndicatorsChange={onIndicatorsChange}
          // Line colours are global: a colour picked here recolours every chart (indicatorColors.ts).
          indicatorColorActions={appActions().indicatorColorActions}
          serverIndicators={serverIndicators}
          macdStyles={state.macdStyles ?? false}
          inspector
          options={OPTIONS}
          label={`${pkg.symbol} 1 minute candles`}
        />
      </View>

      <View style={styles.headline}>
        <Text style={styles.price}>{view.headline.price}</Text>
        {view.headline.change ? <Text style={[styles.change, { color: TONE[view.headline.tone] }]}>{view.headline.change}</Text> : null}
        {view.headline.note ? <Text style={styles.small}>{view.headline.note}</Text> : null}
      </View>
      <View style={styles.grid}>
        {view.stats.map((s) => (
          <View key={s.id} style={[styles.card, tile]}>
            <Text style={styles.label}>{s.label}</Text>
            <Text style={[styles.value, { color: TONE[s.tone] }]} numberOfLines={1} adjustsFontSizeToFit>
              {s.value}
            </Text>
            <Text style={styles.note} numberOfLines={1}>
              {s.note || " "}
            </Text>
          </View>
        ))}
      </View>


      <Section title="Notification history" note={view.alertSummary}>
        {view.alerts.length === 0 ? <Text style={styles.empty}>No notification for {view.symbol} that day.</Text> : view.alerts.map((a) => <AlertCard key={a.key} a={a} selected={a.marker != null && a.marker === selected} onSelect={onSelect} />)}
      </Section>

      <Section title="Scorecard">
        <View style={styles.grid}>
          {view.score.map((k) => (
            <View key={k.label} style={[styles.card, tile]}>
              <Text style={styles.label}>{k.label}</Text>
              <Text style={styles.value}>{k.value}</Text>
              <Text style={styles.note}>{k.note}</Text>
            </View>
          ))}
        </View>
        {view.byKind.map((b) => (
          <View key={b.key} style={styles.kindRow}>
            <Text style={[styles.body, { flex: 1 }]} numberOfLines={1}>
              {b.label}
            </Text>
            <Text style={styles.mono}>{b.value}</Text>
          </View>
        ))}
      </Section>

      <Section title="Halts & LULD">
        <View style={[styles.card, { gap: 6 }]}>
          {view.halts.lines.map((l) => (
            <Text key={l} style={styles.body}>
              {l}
            </Text>
          ))}
          {view.halts.luld ? <Text style={[styles.body, { color: colors.textDim }]}>{view.halts.luld}</Text> : null}
          <Text style={styles.note}>{view.halts.closestTitle}</Text>
          {view.halts.closest.map((c) => (
            <Text key={c} style={styles.mono}>
              {c}
            </Text>
          ))}
        </View>
      </Section>

      <Section title="Missed setups" note={view.missed.length ? "Found by the detector, not sent" : undefined}>
        {view.missed.length === 0 ? <Text style={styles.empty}>Every setup the detector armed that day was notified.</Text> : view.missed.map((m) => <MissedCard key={m.key} m={m} />)}
        {view.missedLater.length > 0 && (
          <>
            <Pressable accessibilityRole="button" accessibilityState={{ expanded: later }} onPress={() => setLater((v) => !v)} style={({ pressed }) => [styles.more, pressed && { backgroundColor: UI.raised }]}>
              <Icon name={later ? "chevronDown" : "chevronRight"} color={colors.textDim} size={12} />
              <Text style={styles.small}>{`${view.missedLater.length} later setup${view.missedLater.length === 1 ? "" : "s"}`}</Text>
            </Pressable>
            <Collapse open={later} reducedMotion={reducedMotion} gap={8}>
              <View style={{ gap: 8 }}>
                {view.missedLater.map((m) => (
                  <MissedCard key={m.key} m={m} />
                ))}
              </View>
            </Collapse>
          </>
        )}
      </Section>
    </>
  );
}

function Section({ title, note, children }: { title: string; note?: string; children: ReactNode }) {
  return (
    <View style={{ gap: 8 }}>
      <View style={styles.sectionHead}>
        <Text style={styles.section} accessibilityRole="header">
          {title}
        </Text>
        {note ? <Text style={styles.small}>{note}</Text> : null}
      </View>
      {children}
    </View>
  );
}

function AlertCard({ a, selected, onSelect }: { a: TickerAlertView; selected: boolean; onSelect: (n: number) => void }) {
  // A held alert has no marker on the chart, so tapping it picks nothing out.
  const marker = a.marker || null;
  return (
    <Pressable
      disabled={marker === null}
      onPress={() => marker !== null && onSelect(marker)}
      accessibilityRole={marker === null ? undefined : "button"}
      accessibilityState={marker === null ? undefined : { selected }}
      accessibilityHint={marker === null ? undefined : "Marks this alert on the chart"}
      style={({ pressed }) => [styles.card, { gap: 5 }, (selected || pressed) && { backgroundColor: UI.raised }]}
      accessibilityLabel={`${a.marker ? `Alert ${a.marker}` : "Held"}, ${a.time}, ${a.kind}, ${a.price}. ${a.held ?? a.detail}. Best after ${a.best}, ${a.bestPct}. Plus 5 percent: ${a.plus5}. Stop: ${a.stop}.`}
    >
      <View style={styles.rowTop}>
        {a.marker ? (
          <View style={[styles.marker, selected && { backgroundColor: colors.flame }]}>
            <Text style={styles.markerText}>{a.marker}</Text>
          </View>
        ) : (
          <View style={[styles.marker, { backgroundColor: colors.warningBg }]}>
            <Icon name="pause" color={colors.warning} size={11} />
          </View>
        )}
        <Text style={styles.time}>{a.time}</Text>
        <Text style={[styles.body, styles.kind]} numberOfLines={1}>
          {a.kind}
        </Text>
        <Text style={[styles.mono, { color: colors.text }]}>{a.price}</Text>
      </View>
      <Text style={[styles.small, a.held ? { color: colors.warning } : null]} numberOfLines={2}>
        {a.held ?? a.detail}
      </Text>
      <View style={styles.outcomes}>
        <Text style={[styles.mono, { color: TONE[a.bestTone] }]}>best {a.bestPct}</Text>
        {a.dip ? <Text style={[styles.small, { color: TONE[a.dipTone] }]}>{a.dip}</Text> : null}
        <Text style={[styles.small, { color: TONE[a.plus5Tone] }]}>+5% {a.plus5}</Text>
        <Text style={[styles.small, { color: TONE[a.stopTone] }]}>stop {a.stop}</Text>
      </View>
    </Pressable>
  );
}

function MissedCard({ m }: { m: TickerMissedView }) {
  return (
    <View style={[styles.card, { gap: 4 }]} accessible accessibilityLabel={`Missed ${m.kind} at ${m.time}. ${m.levels}. ${m.outcome}. ${m.why}`}>
      <View style={styles.rowTop}>
        <View style={[styles.dot, styles.ring, { width: 12, height: 12, borderRadius: 6 }]} />
        <Text style={styles.time}>{m.time}</Text>
        <Text style={[styles.body, styles.kind]}>{m.kind}</Text>
        <Text style={[styles.mono, { color: TONE[m.outcomeTone] }]} numberOfLines={1}>
          {m.outcome}
        </Text>
      </View>
      <Text style={styles.monoSmall}>{m.levels}</Text>
      <Text style={styles.body}>{m.why}</Text>
    </View>
  );
}

const mono = { fontVariant: ["tabular-nums" as const] };
const styles = StyleSheet.create({
  screen: { flex: 1, backgroundColor: colors.bg, paddingHorizontal: 12 },
  top: { flexDirection: "row", alignItems: "center", gap: 8, minHeight: 48 },
  square: { width: 44, height: 44, borderRadius: 12, backgroundColor: colors.bgPanel, alignItems: "center", justifyContent: "center" },
  symbol: { color: colors.text, fontFamily: fonts.semibold, fontSize: 20 },
  days: { marginLeft: "auto", flexDirection: "row", alignItems: "center", gap: 4 },
  dayChip: { flexDirection: "row", alignItems: "center", gap: 6, height: 44, paddingHorizontal: 10, borderRadius: 12, backgroundColor: colors.bgRow },
  dayText: { color: colors.text, fontFamily: fonts.semibold, fontSize: 13 },
  headline: { flexDirection: "row", alignItems: "baseline", gap: 10, flexWrap: "wrap" },
  price: { color: colors.text, fontSize: 28, fontFamily: fonts.semibold, ...mono },
  change: { fontSize: 16, ...mono },
  grid: { flexDirection: "row", flexWrap: "wrap", gap: 8 },
  card: { padding: 12, borderRadius: 12, backgroundColor: colors.bgPanel, gap: 2 },
  // Two (cover screen) or four (unfolded) equal tiles a row; the gap is 8.
  tile2: { flexBasis: "47%", flexGrow: 1 },
  tile4: { flexBasis: "22%", flexGrow: 1 },
  label: { color: colors.textDim, fontSize: 11 },
  value: { color: colors.text, fontSize: 15, ...mono },
  note: { color: colors.textFaint, fontSize: 11 },
  chartWrap: { borderRadius: 16, backgroundColor: colors.bgPanel, padding: 8, gap: 6 },
  legend: { flexDirection: "row", alignItems: "center", gap: 6, paddingHorizontal: 4 },
  dot: { width: 10, height: 10, borderRadius: 5 },
  ring: { borderWidth: 2, borderColor: colors.textDim },
  sectionHead: { flexDirection: "row", alignItems: "baseline", gap: 8, flexWrap: "wrap" },
  section: { color: colors.heading, fontFamily: fonts.semibold, fontSize: 15 },
  sectionSmall: { color: colors.text, fontFamily: fonts.semibold, fontSize: 13 },
  small: { color: colors.textDim, fontSize: 12 },
  body: { color: colors.text, fontSize: 13, lineHeight: 19 },
  mono: { color: colors.textDim, fontSize: 12, ...mono },
  monoSmall: { color: colors.textDim, fontSize: 11, ...mono },
  empty: { color: colors.textFaint, fontSize: 13, paddingVertical: 8 },
  rowTop: { flexDirection: "row", alignItems: "center", gap: 8 },
  time: { color: colors.textDim, fontSize: 12, ...mono },
  kind: { flexShrink: 1, flexGrow: 1, fontFamily: fonts.semibold },
  marker: { width: 22, height: 22, borderRadius: 11, backgroundColor: colors.accent, alignItems: "center", justifyContent: "center" },
  markerText: { color: colors.accentFg, fontSize: 11, fontFamily: fonts.semibold },
  outcomes: { flexDirection: "row", flexWrap: "wrap", columnGap: 12, rowGap: 2 },
  kindRow: { flexDirection: "row", alignItems: "center", gap: 10, minHeight: 36, paddingHorizontal: 12, borderRadius: 8, backgroundColor: colors.bgRow },
  more: { alignSelf: "flex-start", flexDirection: "row", alignItems: "center", gap: 6, minHeight: 44, paddingHorizontal: 8, borderRadius: 10 },
});

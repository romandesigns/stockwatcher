import {
  buildSlots,
  formatChange,
  formatEtTime,
  formatPrice,
  lastCandle,
  sessionOf,
  formatEtDateKey,
  sessionBands,
  tradingDayOf,
  volumeBuckets,
  slotIndexOf,
  type Drawing,
  type IndicatorSpec,
  parseToolboxPlacement,
  parseLayers,
  DEFAULT_LAYERS,
  type ChartLayers,
  DEFAULT_TOOLBOX_PHONE,
  type PriceScaleType,
  type ToolboxPlacement,
  type Timeframe,
} from "@stockwatcher/chart-core";
import { CandleChart, CandleInspector, Segmented, SessionTimeline, TapeSheet, ToolButton, UI } from "@stockwatcher/chart-native";
import type { ConnectionStatus } from "@stockwatcher/shared";
import { useEffect, useMemo, useState } from "react";
import { StyleSheet, Text, View } from "react-native";
import { useSafeAreaInsets } from "react-native-safe-area-context";
import Svg, { Circle, Path } from "react-native-svg";

import { useCandidates, useChartFeed, usePaperTrading } from "../feed/hooks";
import { createMobileFeed, type MobileFeed } from "../feed/serverFeed";
import { getPreference, setPreference } from "../storage/preferences";
import { getAccessToken } from "../storage/secure";
import { colors, fonts } from "../theme";

const SPEEDS = [1, 2, 10, 60, 0] as const;
/** Timeline minimap bucket: 20 minutes, as on the web's phone layout. */
const bucketMs = 20 * 60_000;
const speedLabel = (s: number) => (s === 0 ? "Max" : `${s}×`);
const SESSION = { overnight: "Overnight", pre: "Premarket", regular: "Regular", after: "After hours" } as const;

/** The chart screen: connects once (token from the keystore), then renders the chart. */
export function ChartScreen() {
  const [ready, setReady] = useState<{ feed: MobileFeed; token: string | null } | null>(null);
  const [status, setStatus] = useState<ConnectionStatus>("connecting");
  useEffect(() => {
    let feed: MobileFeed | null = null;
    let alive = true;
    void getAccessToken()
      .catch(() => null)
      .then((token) => {
        if (!alive) return;
        feed = createMobileFeed(token, setStatus);
        setReady({ feed, token });
      });
    return () => {
      alive = false;
      feed?.close();
    };
  }, []);
  if (!ready) return <View style={styles.screen} />;
  return <ChartView feed={ready.feed} token={ready.token} status={status} />;
}

function ChartView({ feed, token, status }: { feed: MobileFeed; token: string | null; status: ConnectionStatus }) {
  const insets = useSafeAreaInsets();
  const candidates = useCandidates(token);
  const [symbol, setSymbol] = useState<string | null>(null);
  const [timeframe, setTimeframe] = useState<Timeframe>("1m");
  const [inspectT, setInspectT] = useState<number | null>(null);
  const [scale, setScale] = useState<PriceScaleType>("linear");

  // Indicators and drawings are the chart's; the screen only persists them.
  const [indicators, setIndicators] = useState<IndicatorSpec[]>([]);
  const [drawings, setDrawings] = useState<Record<string, Drawing[]>>({});
  // Where the floating drawing tools were left, kept across launches.
  const [toolbox, setToolbox] = useState<ToolboxPlacement>(DEFAULT_TOOLBOX_PHONE);
  useEffect(() => {
    void getPreference<unknown>("chart.toolbox").then((v) => v && setToolbox(parseToolboxPlacement(v, DEFAULT_TOOLBOX_PHONE)));
  }, []);
  // Which context layers show (halts, LULD, key levels, fills), kept across launches.
  const [layers, setLayers] = useState<ChartLayers>(DEFAULT_LAYERS);
  useEffect(() => {
    void getPreference<unknown>("chart.layers").then((v) => v && setLayers(parseLayers(v)));
  }, []);
  const changeLayers = (l: ChartLayers) => {
    setLayers(l);
    void setPreference("chart.layers", l);
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
  useEffect(() => {
    void getPreference<IndicatorSpec[]>("chart.indicators").then((v) => v && setIndicators(v));
    void getPreference<Record<string, Drawing[]>>("chart.drawings").then((v) => v && setDrawings(v));
  }, []);
  const changeIndicators = (v: IndicatorSpec[]) => {
    setIndicators(v);
    void setPreference("chart.indicators", v);
  };

  // Follow the price band: a symbol that leaves it is no longer let through.
  const active = symbol && candidates.includes(symbol) ? symbol : (candidates[0] ?? null);
  const { series, clock, tape } = useChartFeed(feed, active ?? "", timeframe);
  const [tapeOpen, setTapeOpen] = useState(false);

  // The day's session bands and a 20-minute volume minimap for the timeline (bands change only with the day).
  const dayKey = clock ? formatEtDateKey(tradingDayOf(clock.now)) : "";
  const bands = useMemo(() => {
    if (!dayKey) return [];
    const [year, month, day] = dayKey.split("-").map(Number) as [number, number, number];
    return sessionBands({ year, month, day });
  }, [dayKey]);
  const buckets = useMemo(() => (series && bands.length ? volumeBuckets(series.candles, bands[0]!.start, bands[3]!.end, bucketMs) : []), [series, bands]);
  const trading = usePaperTrading(feed);

  const slots = useMemo(() => (series ? buildSlots(series.candles, series.gaps) : []), [series]);
  const last = lastCandle(slots);
  const sessionOpen = useMemo(() => {
    const start = series?.session?.start;
    return start === undefined ? undefined : series?.candles.find((c) => c.t >= start)?.o;
  }, [series]);
  const inspected = inspectT === null ? slots.at(-1) : slots[slotIndexOf(slots, inspectT) ?? -1];
  const change = last && sessionOpen ? formatChange(sessionOpen, last.c) : "";

  const changeDrawings = (d: Drawing[]) => {
    if (!active) return;
    const next = { ...drawings, [active]: d };
    setDrawings(next);
    void setPreference("chart.drawings", next);
  };

  const playing = !!clock?.playing;
  const live = clock?.mode === "live";
  const speed = clock?.speed ?? 1;
  const offline = status !== "open";

  return (
    <View style={[styles.screen, { paddingTop: insets.top + 8, paddingBottom: insets.bottom + 8 }]}>
      <View style={styles.header}>
        <Text style={styles.symbol}>{active ?? "—"}</Text>
        <Text style={styles.price}>{last ? formatPrice(last.c) : "—"}</Text>
        <Text style={[styles.change, { color: change.startsWith("-") ? colors.critical : colors.good }]}>{change}</Text>
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
            <Svg width={15} height={15} viewBox="0 0 16 16" fill="none" stroke={live ? colors.good : colors.textDim} strokeWidth={1.6} strokeLinecap="round" strokeLinejoin="round">
              {live ? (
                <>
                  <Circle cx={8} cy={8} r={1.6} fill={colors.good} stroke="none" />
                  <Path d="M5.2 5.2a4 4 0 0 0 0 5.6M10.8 5.2a4 4 0 0 1 0 5.6" />
                  <Path d="M3.1 3.1a7 7 0 0 0 0 9.8M12.9 3.1a7 7 0 0 1 0 9.8" strokeOpacity={0.55} />
                </>
              ) : (
                <>
                  <Path d="M2.5 8a5.5 5.5 0 1 0 1.6-3.9" />
                  <Path d="M2.5 2.5v2.2h2.2" />
                  <Path d="M8 5.2V8l1.9 1.3" />
                </>
              )}
            </Svg>
          )}
        </View>
      </View>

      <View style={styles.row}>
        {candidates.length > 1 && (
          <Segmented
            value={active ?? ""}
            onChange={(s) => {
              setSymbol(s);
              setInspectT(null);
            }}
            options={candidates.slice(0, 4).map((s) => ({ value: s, label: s, a11y: s }))}
          />
        )}
      </View>

      <CandleChart
        style={styles.chart}
        toolbar
        symbol={active ?? ""}
        series={series}
        timeframe={timeframe}
        inspectT={inspectT}
        onInspectChange={setInspectT}
        priceScaleType={scale}
        onPriceScaleTypeChange={setScale}
        indicators={indicators}
        onIndicatorsChange={changeIndicators}
        drawings={active ? (drawings[active] ?? []) : []}
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
        // The session volume profile sits inside the chart on a phone (no room for a side panel), as on the web's phone layout.
        options={{ priceAxisWidth: 46, volumeHeight: 40, timeAxisHeight: 18, paneGap: 4, fontSize: 9, timeLabelSpacing: 60, indicatorPaneHeight: 56, showProfile: true, profileWidth: 0.26 }}
        barSpacing={8}
        label={`${active ?? ""} ${timeframe} candles`}
      />

      <CandleInspector slot={inspected} timeframe={timeframe} />

      <View style={styles.replayCard}>
      <View style={styles.replay}>
        <ToolButton primary label={playing ? "Pause replay" : "Play replay"} disabled={!clock || live} onPress={() => feed.control({ action: playing ? "pause" : "play" })} style={styles.play}>
          {playing ? "❚❚" : "▶"}
        </ToolButton>
        <Text style={styles.clock}>{clock ? formatEtTime(clock.now, true) : "--:--:--"}</Text>
        <Text style={styles.session}>{clock ? `ET · ${SESSION[sessionOf(clock.now)]}` : "ET"}</Text>
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
          <Svg width={16} height={16} viewBox="0 0 16 16" fill="none" stroke={colors.textDim} strokeWidth={1.5} strokeLinecap="round">
            <Path d="M3 4h10M3 8h10M3 12h6" />
          </Svg>
        </ToolButton>
      </View>
      {/* Tap a time to jump the replay there. */}
      <SessionTimeline bands={bands} buckets={buckets} bucketMs={bucketMs} now={clock?.now ?? null} onSeek={live ? undefined : (t) => feed.control({ action: "seek", value: t })} />
      </View>
      <TapeSheet open={tapeOpen} onClose={() => setTapeOpen(false)} trades={tape} note={active ?? undefined} />
    </View>
  );
}

const styles = StyleSheet.create({
  screen: { flex: 1, backgroundColor: colors.bg, paddingHorizontal: 12, gap: 8 },
  header: { flexDirection: "row", alignItems: "baseline", gap: 8 },
  symbol: { color: colors.text, fontFamily: fonts.semibold, fontSize: 16 },
  price: { color: colors.text, fontSize: 16, fontVariant: ["tabular-nums"] },
  change: { fontSize: 11, fontVariant: ["tabular-nums"] },
  chip: { marginLeft: "auto", flexDirection: "row", alignItems: "center", gap: 6, backgroundColor: colors.bgPanel, borderRadius: 999, paddingHorizontal: 10, paddingVertical: 4, alignSelf: "center" },
  chipText: { color: colors.textDim, fontSize: 11, fontWeight: "500" },
  row: { flexDirection: "row", alignItems: "center", gap: 8, flexWrap: "wrap" },
  chart: { flex: 1 },
  replayCard: { backgroundColor: UI.panel, borderRadius: 12, paddingHorizontal: 10, paddingTop: 8, paddingBottom: 4, gap: 4 },
  replay: { flexDirection: "row", alignItems: "center", gap: 8 },
  tapeButton: { backgroundColor: UI.row, height: 36, width: 36, paddingHorizontal: 0 },
  play: { width: 36, height: 36, paddingHorizontal: 0 },
  clock: { color: colors.text, fontSize: 14, fontVariant: ["tabular-nums"] },
  session: { color: colors.textFaint, fontSize: 10 },
  speed: { marginLeft: "auto", backgroundColor: UI.row, height: 36, minWidth: 48 },
});

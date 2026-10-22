import { formatEtTime } from "@stockwatcher/chart-core";
import { UI } from "@stockwatcher/chart-native";
import { detectionFamilyLabel, detectionStateLabel, groupDetections, rossAttention, rossFreshness, rossItemCount, rossLinePaths, rossRowView, type DetectionChannel, type DetectionEpisode, type RossChannel, type RossRowView, type RossTone } from "@stockwatcher/shared";
import { memo, useEffect, useMemo, useState } from "react";
import { Animated, Pressable, StyleSheet, Text, View } from "react-native";
import Svg, { Circle, Line, Path, Rect } from "react-native-svg";

import { useDetection, useRoss } from "../feed/hooks";
import { Collapse } from "../motion/Collapse";
import { fonts } from "../theme";
import { useTickerPress } from "../ui/useTickerPress";

const TONE: Record<RossTone, string> = {
  good: UI.good,
  critical: UI.critical,
  warning: UI.warning,
  accent: UI.accent,
  text: UI.text,
  muted: UI.textDim,
  dim: UI.textFaint,
};

const hhmm = (ns: number) => formatEtTime(Math.round(ns / 1_000_000));

/**
 * The Ross Panel on the phone (owner, 2026-10-20; design canvas "Ross Panel", phone): its own
 * card under the Halt watcher row, the web side panel's rows stacked for the narrow screen. A
 * row earns a coloured edge only at the trigger or crossing it. The cues are the micro-pullback
 * stages the Detection Panel shows (one detector, 2026-10-21): stages, not entry signals.
 * Tapping a row opens its chart (the chart modal); tap, then hold copies the ticker. No rows, no
 * card (owner, 2026-10-22): it opens and closes by its real height (Collapse), `gap` being the
 * list's gap it sits in.
 */
export function RossPanel({ channel, detection, onPick, reducedMotion, gap = 0 }: { channel: RossChannel; detection: DetectionChannel; onPick: (symbol: string, episodeId?: string, runEpoch?: string) => void; reducedMotion: boolean; gap?: number }) {
  const state = useRoss(channel);
  const detections = useDetection(detection);
  const detectionEpoch = detections.frame?.runEpoch;
  const bySymbol = new Map(groupDetections(detections.snapshot).map((g) => [g.symbol, g.selected]));
  const { enabled, panel } = state;
  const rows = panel?.rows.map((r) => rossRowView(r, panel.cutoff, hhmm)) ?? [];
  const attention = panel ? rossAttention(panel) : 0;
  // A dropped socket keeps the last rows and says so: they are the last known state, not now.
  const fresh = rossFreshness(state, hhmm);
  // No rows, no card (owner, 2026-10-22): off, not yet known and empty all hide it. It opens and
  // closes by its real height; a reconnect keeps the last rows, so it does not flash.
  return (
    <Collapse open={rossItemCount(state) > 0} reducedMotion={reducedMotion} gap={gap}>
    <View style={styles.card} accessibilityLabel="Ross Panel">
      <View style={styles.head}>
        <Text accessibilityRole="header" style={styles.title}>
          Ross Panel
        </Text>
        <Text style={styles.band}>$0.10–$25</Text>
        {attention > 0 && (
          <View style={styles.count} accessibilityLabel={`${attention} need attention`}>
            <Pulse color={UI.accent} size={6} reducedMotion={reducedMotion} />
            <Text style={styles.countText}>{attention}</Text>
          </View>
        )}
      </View>
      {fresh.stale && enabled === true && (
        <Text accessibilityRole="alert" style={[styles.stale, { color: TONE[fresh.tone] }]}>
          {fresh.label}
        </Text>
      )}
      <View style={[styles.rows, fresh.stale && styles.dimmed]}>
        {rows.map((r) => (
          <Row key={r.symbol} row={r} episode={bySymbol.get(r.symbol)} detectionCurrent={detections.connected && !detections.resyncing} onPick={(symbol) => onPick(symbol, bySymbol.get(symbol)?.episode_id, detectionEpoch)} reducedMotion={reducedMotion} />
        ))}
      </View>
    </View>
    </Collapse>
  );
}

/** Only a crossing in progress (accent) and a price at the trigger (amber) earn an edge: a broke-trigger is a past stage, never a green "go". */
const EDGE: Partial<Record<RossRowView["setup"]["state"], string>> = { crossing: UI.accent, near: UI.warning };

const Row = memo(function Row({ row: r, episode, detectionCurrent, onPick, reducedMotion }: { row: RossRowView; episode?: DetectionEpisode; detectionCurrent: boolean; onPick: (symbol: string) => void; reducedMotion: boolean }) {
  const press = useTickerPress(r.symbol, () => onPick(r.symbol));
  const edge = EDGE[r.setup.state];
  return (
    <Pressable
      accessibilityRole="button"
      accessibilityLabel={r.aria}
      accessibilityHint="Opens its chart. Tap, then press and hold to copy"
      {...press}
      style={({ pressed }) => [
        styles.row,
        r.setup.state === "crossing" && { backgroundColor: "rgba(139,156,255,0.10)" },
        r.dimmed && { opacity: 0.6 },
        pressed && { backgroundColor: UI.raised },
      ]}
    >
      {/* The cue's edge: a filled bar, not a border (house rule: no borders). */}
      {edge && <View style={[styles.edge, { backgroundColor: edge }]} />}
      {/* The two text lines on the left, the day's line chart on their right (like the web row):
          the chart's box is always reserved, so a line arriving never moves the text. */}
      <View style={styles.top}>
        <View style={styles.lines}>
          <View style={styles.line}>
            <Text style={styles.symbol}>{r.symbol}</Text>
            <Text style={styles.price}>{r.price}</Text>
            <Text style={[styles.small, { color: UI.good }]}>{r.gain}</Text>
            {r.badge && (
              <View style={[styles.badge, { backgroundColor: r.badge.label === "NEW" ? UI.accent : UI.warning }]}>
                <Text style={styles.badgeText}>{r.badge.label}</Text>
              </View>
            )}
            <Text style={[styles.m1, { color: TONE[r.m1Tone] }]}>{r.m1}</Text>
          </View>
          <View style={styles.line}>
            {r.setup.pulse && <Pulse color={TONE[r.setup.tone]} size={6} reducedMotion={reducedMotion} />}
            <Text numberOfLines={1} style={[styles.setup, { color: TONE[r.setup.tone], fontFamily: r.setup.attention ? fonts.semibold : fonts.regular }]}>
              {r.setup.label}
              {r.setup.note ? <Text style={styles.note}>{` ${r.setup.note}`}</Text> : null}
            </Text>
            {r.tag ? <Text style={[styles.tag, { color: TONE[r.tagTone] }]}>{r.tag}</Text> : null}
          </View>
        </View>
        <DayLine line={r.line} reducedMotion={reducedMotion} />
      </View>
      {r.setup.meter !== null && <Meter value={r.setup.meter} color={TONE[r.setup.tone]} />}
      {episode && <Text style={styles.note}>Detection: {detectionFamilyLabel(episode.family)} · {detectionStateLabel(episode.state)} · experimental{detectionCurrent ? "" : " · last received"}</Text>}
    </Pressable>
  );
});

/** The row chart's box (dp): fits the Fold's ~360 dp cover screen beside the row's text. */
export const DAY_LINE_W = 72;
export const DAY_LINE_H = 28;

/**
 * The day's price line (owner, 2026-10-21: "a sense of how the actual price action ... is
 * developing before I actually click on the ticker"): every 30 s close from 04:00 ET and the
 * live price as its end (an older server: the last 15 s closes), good when it ends at or above
 * the day's first price, critical below, with a faint dotted previous-close baseline when it is
 * near the line (`rossLinePaths`, shared with the web row). It fades in when its line first
 * arrives; with reduced motion it simply appears. The box is reserved either way.
 */
export function DayLine({ line, reducedMotion }: { line: RossRowView["line"]; reducedMotion: boolean }) {
  const has = line !== null;
  const [o] = useState(() => new Animated.Value(has ? (reducedMotion ? 1 : 0) : 0));
  useEffect(() => {
    if (!has) {
      o.setValue(0);
      return;
    }
    if (reducedMotion) {
      o.setValue(1);
      return;
    }
    const a = Animated.timing(o, { toValue: 1, duration: 240, useNativeDriver: true });
    a.start();
    return () => a.stop();
  }, [o, has, reducedMotion]);
  const paths = useMemo(() => (line ? rossLinePaths(line, DAY_LINE_W, DAY_LINE_H) : null), [line]);
  const color = line?.tone === "critical" ? UI.critical : UI.good;
  return (
    <Animated.View style={[styles.chart, { opacity: o }]} accessibilityElementsHidden importantForAccessibility="no-hide-descendants" testID="ross-day-line">
      {paths && (
        <Svg width={DAY_LINE_W} height={DAY_LINE_H} viewBox={`0 0 ${DAY_LINE_W} ${DAY_LINE_H}`}>
          {paths.baseY !== null && <Line x1={0} x2={DAY_LINE_W} y1={paths.baseY} y2={paths.baseY} stroke={UI.textFaint} strokeWidth={1} strokeDasharray="1.5 2.5" />}
          <Path d={paths.area} fill={color} fillOpacity={0.14} />
          <Path d={paths.line} fill="none" stroke={color} strokeWidth={1.5} strokeLinejoin="round" />
        </Svg>
      )}
    </Animated.View>
  );
}

/** Pullback low (left, red: the failure level) to trigger (right, neutral: a level, not a "go"), the price as a dot. */
function Meter({ value, color }: { value: number; color: string }) {
  const v = Math.max(0, Math.min(1, value));
  return (
    <Svg width="100%" height={8} viewBox="0 0 160 8" preserveAspectRatio="none">
      <Rect x={0} y={3} width={160} height={2} rx={1} fill={UI.raised} />
      <Rect x={0} y={0} width={2} height={8} fill={UI.critical} />
      <Rect x={158} y={0} width={2} height={8} fill={UI.textDim} />
      <Rect x={0} y={3} width={v * 156} height={2} rx={1} fill={color} />
      <Circle cx={2 + v * 154} cy={4} r={3} fill={color} />
    </Svg>
  );
}

/** A dot that breathes while something needs attention; still with reduced motion. */
function Pulse({ color, size, reducedMotion }: { color: string; size: number; reducedMotion: boolean }) {
  const [o] = useState(() => new Animated.Value(1));
  useEffect(() => {
    if (reducedMotion) {
      o.setValue(1);
      return;
    }
    const loop = Animated.loop(Animated.sequence([Animated.timing(o, { toValue: 0.35, duration: 500, useNativeDriver: true }), Animated.timing(o, { toValue: 1, duration: 500, useNativeDriver: true })]));
    loop.start();
    return () => loop.stop();
  }, [o, reducedMotion]);
  return <Animated.View style={{ width: size, height: size, borderRadius: size / 2, backgroundColor: color, opacity: o }} />;
}

const styles = StyleSheet.create({
  card: { borderRadius: 14, backgroundColor: UI.panel, paddingVertical: 10, paddingHorizontal: 8, gap: 6 },
  head: { flexDirection: "row", alignItems: "center", gap: 8, paddingHorizontal: 4 },
  title: { color: UI.text, fontSize: 14, fontFamily: fonts.semibold },
  band: { color: UI.textFaint, fontSize: 11, fontVariant: ["tabular-nums"] },
  count: { marginLeft: "auto", flexDirection: "row", alignItems: "center", gap: 5, height: 22, paddingHorizontal: 8, borderRadius: 11, backgroundColor: "rgba(139,156,255,0.16)" },
  countText: { color: "#c3cbff", fontSize: 11 },
  stale: { fontSize: 11, paddingHorizontal: 4, paddingBottom: 2 },
  // The card's own row gap: the rows are separated by space, never borders.
  rows: { gap: 6 },
  dimmed: { opacity: 0.6 },
  row: { gap: 4, paddingVertical: 8, paddingHorizontal: 10, borderRadius: 10, backgroundColor: UI.row, overflow: "hidden" },
  edge: { position: "absolute", left: 0, top: 0, bottom: 0, width: 3 },
  top: { flexDirection: "row", alignItems: "center", gap: 8 },
  lines: { flex: 1, minWidth: 0, gap: 4 },
  chart: { width: DAY_LINE_W, height: DAY_LINE_H },
  line: { flexDirection: "row", alignItems: "center", gap: 7 },
  symbol: { color: UI.text, fontSize: 14, fontFamily: fonts.semibold },
  price: { color: UI.text, fontSize: 13, fontVariant: ["tabular-nums"] },
  small: { fontSize: 12, fontVariant: ["tabular-nums"] },
  m1: { marginLeft: "auto", fontSize: 12, fontFamily: fonts.semibold, fontVariant: ["tabular-nums"] },
  badge: { borderRadius: 4, paddingHorizontal: 5, paddingVertical: 1 },
  badgeText: { color: UI.accentFg, fontSize: 9, fontFamily: fonts.semibold },
  setup: { flexShrink: 1, fontSize: 11 },
  note: { color: UI.textDim, fontFamily: fonts.regular, fontVariant: ["tabular-nums"] },
  tag: { marginLeft: "auto", fontSize: 11 },
});

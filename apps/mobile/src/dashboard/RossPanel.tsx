import { formatEtTime } from "@stockwatcher/chart-core";
import { UI } from "@stockwatcher/chart-native";
import { detectionFamilyLabel, detectionStateLabel, groupDetections, rocketNow, rossAttention, rossFreshness, rossItemCount, rossRowMarks, rossRowView, runnersBySymbol, type DetectionChannel, type DetectionEpisode, type RocketChannel, type RossChannel, type RossRowView, type RossTone, type RowMarks, type TailMark } from "@stockwatcher/shared";
import { memo, useEffect, useState } from "react";
import { Animated, Pressable, StyleSheet, Text, View } from "react-native";

import { useDetection, useRocket, useRoss } from "../feed/hooks";
import { Flame, MarksLine, RowLine, useReadoutPress, useWall } from "./RowMarks";
import { Collapse } from "../motion/Collapse";
import { fonts } from "../theme";
import { useTickerPress } from "../ui/useTickerPress";
import { StarToggle } from "../ui/StarToggle";

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
export function RossPanel({ channel, detection, runners, onPick, reducedMotion, gap = 0 }: { channel: RossChannel; detection: DetectionChannel; runners: RocketChannel; onPick: (symbol: string, episodeId?: string, runEpoch?: string) => void; reducedMotion: boolean; gap?: number }) {
  const state = useRoss(channel);
  const detections = useDetection(detection);
  const detectionEpoch = detections.frame?.runEpoch;
  const bySymbol = new Map(groupDetections(detections.snapshot).map((g) => [g.symbol, g.selected]));
  const { enabled, panel } = state;
  // Today's Runners row per member: its path and the burst behind the flame (shared rowMarks.ts).
  const rocket = useRocket(runners);
  const wall = useWall();
  const byRunner = runnersBySymbol(rocket);
  const nowMs = rocketNow(rocket, wall);
  const rows = panel?.rows.map((w) => ({ ...rossRowView(w, panel.cutoff, hhmm), marks: rossRowMarks(w, byRunner.get(w.symbol), nowMs, formatEtTime), trigger: w.setup?.trigger?.price ?? null })) ?? [];
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

/**
 * One member as a lighter row (owner, 2026-10-26, design canvas "StockWatcher lighter rows";
 * phone artboard): ticker, price and day gain as text with the day line (the last minute its
 * thick tail, the trigger a dotted level), then the marks line. Press and hold reads every mark
 * out; the flame lights on a fresh Buying burst that holds its alert price. The cue edges stay.
 */
const Row = memo(function Row({ row: r, episode, detectionCurrent, onPick, reducedMotion }: { row: RossRowView & { marks: RowMarks; trigger: number | null }; episode?: DetectionEpisode; detectionCurrent: boolean; onPick: (symbol: string) => void; reducedMotion: boolean }) {
  const press = useReadoutPress(useTickerPress(r.symbol, () => onPick(r.symbol)), r.marks);
  const edge = EDGE[r.setup.state];
  const m = r.marks;
  return (
    <Pressable
      accessibilityRole="button"
      accessibilityLabel={r.aria}
      accessibilityHint="Opens its chart. Press and hold to read its marks; tap, then press and hold to copy"
      {...press}
      style={({ pressed }) => [
        styles.row,
        m.flame?.lit && { backgroundColor: UI.raised },
        r.setup.state === "crossing" && { backgroundColor: "rgba(139,156,255,0.10)" },
        r.dimmed && { opacity: 0.6 },
        pressed && { backgroundColor: UI.raised },
      ]}
    >
      {/* The cue's edge: a filled bar, not a border (house rule: no borders). */}
      {edge && <View style={[styles.edge, { backgroundColor: edge }]} />}
      <View style={styles.line}>
        <Flame flame={m.flame} />
        <Text style={styles.symbol}>{r.symbol}</Text>
        <Text style={styles.price}>{r.price}</Text>
        <Text style={[styles.small, { color: UI.good }]}>{r.gain}</Text>
        {r.badge && (
          <View style={[styles.badge, { backgroundColor: r.badge.label === "NEW" ? UI.accent : UI.warning }]}>
            <Text style={styles.badgeText}>{r.badge.label}</Text>
          </View>
        )}
        <View style={styles.chartSlot}>
          <DayLine line={r.line} tail={m.tail} trigger={m.stage ? r.trigger : null} reducedMotion={reducedMotion} />
        </View>
        <StarToggle symbol={r.symbol} size={14} />
      </View>
      <MarksLine marks={m} />
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
 * near the line (`rossLinePaths`, shared with the web row). Lighter rows (2026-10-26): the last
 * minute is its thick tail and a setup's trigger a dotted level (RowLine). It fades in when its
 * line first arrives; with reduced motion it simply appears. The box is reserved either way.
 */
export function DayLine({ line, tail = null, trigger = null, reducedMotion }: { line: RossRowView["line"]; tail?: TailMark | null; trigger?: number | null; reducedMotion: boolean }) {
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
  const color = line?.tone === "critical" ? UI.critical : UI.good;
  return (
    <Animated.View style={[styles.chart, { opacity: o }]} testID="ross-day-line">
      <RowLine line={line} tail={tail} trigger={trigger} width={DAY_LINE_W} height={DAY_LINE_H} color={color} />
    </Animated.View>
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
  // The day line takes the first line's free space's right end, before the star.
  chartSlot: { marginLeft: "auto" },
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

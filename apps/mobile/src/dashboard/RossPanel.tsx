import { formatEtTime } from "@stockwatcher/chart-core";
import { UI } from "@stockwatcher/chart-native";
import { detectionFamilyLabel, detectionStateLabel, groupDetections, rossAttention, rossFreshness, rossReadinessView, rossRowView, type DetectionChannel, type DetectionEpisode, type RossChannel, type RossRowView, type RossTone } from "@stockwatcher/shared";
import { memo, useEffect, useState } from "react";
import { Animated, Pressable, StyleSheet, Text, View } from "react-native";
import Svg, { Circle, Rect } from "react-native-svg";

import { useDetection, useRoss } from "../feed/hooks";
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
 * row earns a coloured edge only when it needs you (near the trigger, crossing, triggered).
 * Tapping a row opens its chart (the chart modal); tap, then hold copies the ticker.
 */
export function RossPanel({ channel, detection, onPick, reducedMotion }: { channel: RossChannel; detection: DetectionChannel; onPick: (symbol: string, episodeId?: string, runEpoch?: string) => void; reducedMotion: boolean }) {
  const state = useRoss(channel);
  const detections = useDetection(detection);
  const bySymbol = new Map(groupDetections(detections.snapshot).map((g) => [g.symbol, g.selected]));
  const { enabled, panel, readiness } = state;
  const rows = panel?.rows.map((r) => rossRowView(r, panel.cutoff, hhmm)) ?? [];
  const attention = panel ? rossAttention(panel) : 0;
  // A dropped socket keeps the last rows and says so: they are the last known state, not now.
  const fresh = rossFreshness(state, hhmm);
  const why = readiness ? rossReadinessView(readiness, hhmm) : null;
  return (
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
      {fresh.stale && enabled !== false && (
        <Text accessibilityRole="alert" style={[styles.stale, { color: TONE[fresh.tone] }]}>
          {fresh.label}
        </Text>
      )}
      {enabled === false ? (
        <Text style={[styles.empty, { color: UI.warning }]}>Off on this server</Text>
      ) : enabled === null ? (
        <Text style={styles.empty}>Connecting…</Text>
      ) : rows.length === 0 ? (
        // Not "nothing qualifies": the server's own counts say whether a pillar could even be
        // decided (crates/server/src/context.rs `Readiness`).
        <View style={styles.whyBox}>
          <Text style={styles.empty}>{why ? why.headline : "No stock at 5/5 yet"}</Text>
          {why?.reasons.map((r) => (
            <Text key={r} style={styles.whyLine}>
              {r}
            </Text>
          ))}
          {why ? <Text style={styles.whenLine}>{why.takenAt}</Text> : null}
        </View>
      ) : (
        <View style={fresh.stale ? styles.dimmed : undefined}>
          {rows.map((r) => (
            <Row key={r.symbol} row={r} episode={bySymbol.get(r.symbol)} detectionCurrent={detections.connected && !detections.resyncing} onPick={(symbol) => onPick(symbol, bySymbol.get(symbol)?.episode_id, detections.frame?.runEpoch)} reducedMotion={reducedMotion} />
          ))}
        </View>
      )}
    </View>
  );
}

const EDGE: Partial<Record<RossRowView["setup"]["state"], string>> = { crossing: UI.accent, near: UI.warning, triggered: UI.good };

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
      {r.setup.meter !== null && <Meter value={r.setup.meter} color={TONE[r.setup.tone]} />}
      {episode && <Text style={styles.note}>Detection: {detectionFamilyLabel(episode.family)} · {detectionStateLabel(episode.state)} · experimental{detectionCurrent ? "" : " · last received"}</Text>}
    </Pressable>
  );
});

/** Invalidation (left, red) to trigger (right, green), the price as a dot. */
function Meter({ value, color }: { value: number; color: string }) {
  const v = Math.max(0, Math.min(1, value));
  return (
    <Svg width="100%" height={8} viewBox="0 0 160 8" preserveAspectRatio="none">
      <Rect x={0} y={3} width={160} height={2} rx={1} fill={UI.raised} />
      <Rect x={0} y={0} width={2} height={8} fill={UI.critical} />
      <Rect x={158} y={0} width={2} height={8} fill={UI.good} />
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
  empty: { color: UI.textFaint, fontSize: 12, paddingHorizontal: 4, paddingVertical: 6 },
  stale: { fontSize: 11, paddingHorizontal: 4, paddingBottom: 2 },
  whyBox: { gap: 2, paddingBottom: 4 },
  whyLine: { color: UI.textFaint, fontSize: 11, fontFamily: fonts.regular, paddingHorizontal: 4 },
  whenLine: { color: UI.textFaint, fontSize: 10, opacity: 0.7, paddingHorizontal: 4 },
  dimmed: { opacity: 0.6, gap: 4 },
  row: { gap: 4, paddingVertical: 8, paddingHorizontal: 10, borderRadius: 10, backgroundColor: UI.row, overflow: "hidden" },
  edge: { position: "absolute", left: 0, top: 0, bottom: 0, width: 3 },
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

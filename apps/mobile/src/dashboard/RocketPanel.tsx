import { Icon, UI } from "@stockwatcher/chart-native";
import { newRocketFlags, rocketHeaderView, rocketLinePaths, rocketNow, rocketSectionCounts, rocketSectionsView, type RocketChannel, type RocketRowView, type RocketSectionsView, type RocketTone, withZoneSections } from "@stockwatcher/shared";
import { memo, useEffect, useMemo, useRef, useState } from "react";
import { Animated, Pressable, StyleSheet, Text, View } from "react-native";
import Svg, { Circle, Line, Path, Rect } from "react-native-svg";

import { useRocket } from "../feed/hooks";
import { notifyRocketFlags } from "../notifications/rocket";
import { Collapse } from "../motion/Collapse";
import { ZoneNote } from "./ZoneNote";
import { fonts } from "../theme";
import { useTickerPress } from "../ui/useTickerPress";

const TONE: Record<RocketTone, string> = {
  good: UI.good,
  loss: UI.critical,
  warning: UI.warning,
  accent: UI.accent,
  text: UI.text,
  muted: UI.textDim,
  dim: UI.textFaint,
};

/** The row chart's box (dp), the Ross row's: fits the Fold's ~360 dp cover screen beside the text. */
export const DAY_LINE_W = 72;
export const DAY_LINE_H = 28;
/** The card's gap between its blocks, which a closed section must cancel (Collapse). */
const CARD_GAP = 6;
/** A row's gap between its lines, which a closed volume-shelf note must cancel (Collapse). */
const ROW_GAP = 4;

/**
 * Runners on the phone (owner, 2026-10-21/22; EXPERIMENTAL): its own card under the Ross card,
 * the web tab's rows stacked for the narrow screen (the Fold's cover screen and unfolded alike:
 * rows take the card's width, chips truncate, the day line keeps its fixed box). Every flagged
 * row is watched for bullish momentum: rows are ordered by state (breaking out first, fading
 * last) and colour-coded in the Ross row language: accent for a breakout (pulsing) or a run with
 * an inset edge bar, amber while building (edge) and on a halt (badge), a pullback muted with its
 * depth and trigger meter, a fade dimmed. Green only for the strict outcome tag. A state change
 * crossfades the edge and wash (opacity, native driver); with reduced motion it simply changes.
 * Tapping a row opens its chart (the chart modal); tap, then hold copies the ticker. New flags
 * reach the phone as a server push (Settings → Runners); only while this phone has no push token
 * does the card ring a local banner for them instead (notifications/rocket.ts). No layout
 * animation on the rows: a moving Pressable takes stale taps on Android.
 *
 * Buying bursts (2026-10-22) get their own section at the top, newest first (the shared
 * `rocketSectionsView`): heavy buying confirmed, typically about halfway into a move, so a row
 * says how far the burst already went and what typically remains, never "buy". The accent marks
 * only a live burst (pulsing dot, headline and edge while fresh and pending); green and red only
 * its strict outcome tag.
 *
 * A row whose symbol came down into a volume shelf (the frame's `zones`, shared zones.ts) says so
 * on its own full-width line with the research base rate muted beside it, then "Shelf held" /
 * "Shelf lost" once resolved (ZoneNote): plain text, never green, never the accent.
 *
 * Nothing empty is drawn (owner, 2026-10-22): no card while off, not yet known or empty, no
 * Bursts section without bursts, no Flags without flags. Each opens and closes by its real height
 * (Collapse: layout, not a transform, so taps stay true); a reconnect keeps the last rows, dimmed.
 */
export function RocketPanel({ channel, onPick, reducedMotion, gap = 0 }: { channel: RocketChannel; onPick: (symbol: string) => void; reducedMotion: boolean; gap?: number }) {
  const state = useRocket(channel);
  // Ages tick between frames.
  const [wall, setWall] = useState(() => Date.now());
  useEffect(() => {
    const id = setInterval(() => setWall(Date.now()), 15_000);
    return () => clearInterval(id);
  }, []);
  // New flags since this card mounted (the first frame only primes: no burst on open).
  const seen = useRef(new Set<string>());
  const primed = useRef(false);
  useEffect(() => {
    if (!state.panel) return;
    const fresh = newRocketFlags(state, seen.current, primed.current);
    primed.current = true;
    if (fresh.length) notifyRocketFlags(fresh);
  }, [state]);
  const { panel } = state;
  const now = rocketNow(state, wall);
  const { bursts, runners: rows }: RocketSectionsView = panel ? withZoneSections(rocketSectionsView(panel, now), panel.zones) : { bursts: null, runners: [] };
  const head = panel ? rocketHeaderView(panel, state.connected) : null;
  const counts = rocketSectionCounts(state);
  const burstCount = bursts ? bursts.rows.length : 0;
  // No rows, no card (owner, 2026-10-22): off, not yet known and empty all hide it. It stays
  // mounted (the subscription and the local-banner memory above), only collapsed.
  return (
    <Collapse open={counts.bursts + counts.flags > 0} reducedMotion={reducedMotion} gap={gap}>
    <View style={styles.card} accessibilityLabel="Runners">
      <View style={styles.head}>
        <Text accessibilityRole="header" style={styles.title}>
          Runners
        </Text>
        <View style={styles.chip}>
          <Text style={styles.chipText}>Experimental</Text>
        </View>
        {head && head.attention > 0 ? (
          <View style={styles.count} accessibilityLabel={`${head.attention} building or breaking out`}>
            <Pulse color={UI.accent} size={6} reducedMotion={reducedMotion} />
            <Text style={styles.countText}>{head.attention}</Text>
          </View>
        ) : null}
        {head && (
          <Text numberOfLines={1} style={styles.record}>
            {head.record}
          </Text>
        )}
      </View>
      {head && (head.strategy || head.history || head.ranLater) ? (
        <Text numberOfLines={1} style={styles.sub}>
          {[head.ranLater, head.placeholder ? `${head.strategy} (placeholder)` : head.strategy, head.history].filter(Boolean).join(" · ")}
        </Text>
      ) : null}
      {panel && !state.connected && (
        <Text accessibilityRole="alert" style={[styles.sub, { color: UI.warning }]}>
          Reconnecting · last known flags
        </Text>
      )}
      {/* Each section only while it has rows, its heading included (owner, 2026-10-22). */}
      <Collapse open={burstCount > 0} reducedMotion={reducedMotion} gap={CARD_GAP}>
        {bursts && (
          <View style={styles.section} accessibilityLabel="Bursts">
            <View style={styles.sectionHead}>
              <Icon name="trend" color={UI.textDim} size={14} />
              <Text accessibilityRole="header" style={styles.sectionTitle}>
                Bursts
              </Text>
              <Text style={styles.record}>{bursts.head.count}</Text>
            </View>
            <Text style={styles.sub}>{[bursts.head.what, bursts.head.typical].filter(Boolean).join(". ")}</Text>
            <View style={[styles.rows, !state.connected && styles.dimmed]}>
              {bursts.rows.map((r) => (
                <Row key={r.key} row={r} onPick={onPick} reducedMotion={reducedMotion} />
              ))}
            </View>
          </View>
        )}
      </Collapse>
      <Collapse open={rows.length > 0} reducedMotion={reducedMotion} gap={CARD_GAP}>
        <View accessibilityLabel="Flags">
          {/* The heading only beside a Bursts section: alone, the card's title says it. */}
          <Collapse open={burstCount > 0} reducedMotion={reducedMotion}>
            <Text accessibilityRole="header" style={[styles.sectionTitle, styles.sectionPad, styles.flagsHead]}>
              Flags & running
            </Text>
          </Collapse>
          <View style={[styles.rows, !state.connected && styles.dimmed]}>
            {rows.map((r) => (
              <Row key={r.key} row={r} onPick={onPick} reducedMotion={reducedMotion} />
            ))}
          </View>
        </View>
      </Collapse>
    </View>
    </Collapse>
  );
}

/** Fades to 1 whenever `key` changes (0 → 1 over 300 ms); with reduced motion it is simply 1. */
function useCrossfade(key: string, reducedMotion: boolean): Animated.Value {
  const [o] = useState(() => new Animated.Value(1));
  const first = useRef(true);
  useEffect(() => {
    if (first.current || reducedMotion) {
      first.current = false;
      o.setValue(1);
      return;
    }
    o.setValue(0);
    const a = Animated.timing(o, { toValue: 1, duration: 300, useNativeDriver: true });
    a.start();
    return () => a.stop();
  }, [o, key, reducedMotion]);
  return o;
}

const Row = memo(function Row({ row: r, onPick, reducedMotion }: { row: RocketRowView; onPick: (symbol: string) => void; reducedMotion: boolean }) {
  const press = useTickerPress(r.symbol, () => onPick(r.symbol));
  const m = r.momentum;
  const b = r.burst;
  // A live burst takes the accent edge; once it settles the row falls back to its momentum edge
  // (the change crossfades like a state change).
  const fade = useCrossfade(`${m.state}${b?.live ? ":live" : ""}`, reducedMotion);
  const edgeTone = b?.live ? "accent" : m.edge;
  const edge = edgeTone ? TONE[edgeTone] : null;
  return (
    <Pressable accessibilityRole="button" accessibilityLabel={r.aria} accessibilityHint="Opens its chart. Tap, then press and hold to copy" {...press} style={({ pressed }) => [styles.row, m.dimmed && { opacity: 0.6 }, pressed && { backgroundColor: UI.raised }]}>
      {/* The state's wash and inset edge: filled views, never borders, crossfading in on a change. */}
      {m.state === "breaking_out" && <Animated.View pointerEvents="none" style={[StyleSheet.absoluteFill, styles.wash, { opacity: fade }]} />}
      {edge && <Animated.View pointerEvents="none" style={[styles.edge, { backgroundColor: edge, opacity: fade }]} />}
      <View style={styles.line}>
        <Text style={styles.symbol}>{r.symbol}</Text>
        <Text style={styles.price}>{r.price}</Text>
        {r.gain ? <Text style={[styles.small, { color: UI.textDim }]}>{r.gain}</Text> : null}
        {/* Where the row came from ("R2 flag", "Burst", "Running", "Run +20%", "Fast move"): quiet, never accent. */}
        <View style={styles.source} testID="rocket-source">
          <Text numberOfLines={1} style={styles.sourceText}>
            {r.source.label}
          </Text>
        </View>
        {r.halted && (
          <View style={styles.badge}>
            <Text style={styles.badgeText}>HALTED</Text>
          </View>
        )}
        <View style={styles.age}>
          {r.fresh && <Pulse color={UI.accent} size={6} reducedMotion={reducedMotion} />}
          <Text style={[styles.small, r.fresh ? { color: UI.accent, fontFamily: fonts.semibold } : { color: UI.textFaint }]}>{r.age}</Text>
        </View>
      </View>
      {/* The state and metrics on the left, the day line on their right (the Ross row's layout):
          the chart's box is always reserved, so a line arriving never moves the text. */}
      <View style={styles.top}>
        <View style={styles.lines}>
          {b && (
            <Animated.View style={[styles.line, { opacity: fade }]}>
              {b.live && <Pulse color={UI.accent} size={6} reducedMotion={reducedMotion} />}
              <Text numberOfLines={1} style={[styles.state, { color: b.live ? UI.accent : UI.text, fontFamily: fonts.semibold }]}>
                {b.headline}
                <Text style={styles.note}>{` ${b.since}`}</Text>
              </Text>
            </Animated.View>
          )}
          <Animated.View style={[styles.line, { opacity: fade }]}>
            {m.pulse && <Pulse color={TONE[m.tone]} size={6} reducedMotion={reducedMotion} />}
            <Text numberOfLines={1} style={[styles.state, { color: TONE[m.tone], fontFamily: m.attention ? fonts.semibold : fonts.regular }]}>
              {m.label}
              {m.provisional ? <Text style={styles.faint}> provisional</Text> : null}
              {m.since ? <Text style={styles.faint}>{` ${m.since}`}</Text> : null}
              {m.note ? <Text style={styles.note}>{` ${m.note}`}</Text> : null}
            </Text>
          </Animated.View>
          <View style={styles.line} accessibilityLabel={b ? "last 30 s before the alert, from the alert, from high, last minute" : `from ${r.fromWhat}, from high, last minute`}>
            {b?.move ? <Text style={[styles.metric, { color: UI.text }]}>{b.move}</Text> : null}
            <Text style={[styles.metric, { color: TONE[r.fromFlagTone] }]}>{r.fromFlag}</Text>
            <Text style={[styles.metric, { color: TONE[r.fromHighTone] }]}>{r.fromHigh}</Text>
            <Text style={[styles.metric, { color: TONE[r.move1mTone] }]}>{r.move1m}</Text>
          </View>
        </View>
        <DayLine line={r.line} />
      </View>
      {r.meter !== null && <Meter value={r.meter} color={TONE[m.tone]} />}
      {/* The row's whole width (the Fold's cover screen needs it for the range and base rate). */}
      <ZoneNote note={r.zone ?? null} reducedMotion={reducedMotion} gap={ROW_GAP} />
      <View style={styles.line}>
        <View style={styles.chips}>
          {r.reasons.map((x) => (
            <View key={x} style={styles.reason}>
              <Text numberOfLines={1} style={styles.reasonText}>
                {x}
              </Text>
            </View>
          ))}
        </View>
        {r.outcome && <Text style={[styles.outcome, { color: TONE[r.outcome.tone] }]}>{r.outcome.label}</Text>}
        {r.score ? <Text style={styles.score}>{r.score}</Text> : null}
      </View>
    </Pressable>
  );
});

/**
 * The day's price line: every 30 s close from 04:00 ET and the live price as its end
 * (`rocketLinePaths`, shared with the web row), in the neutral dim tone (green belongs to the
 * strict outcome), with a faint dotted previous-close baseline when it is near the line.
 */
function DayLine({ line }: { line: RocketRowView["line"] }) {
  const paths = useMemo(() => (line ? rocketLinePaths(line, DAY_LINE_W, DAY_LINE_H) : null), [line]);
  return (
    <View style={styles.chart} accessibilityElementsHidden importantForAccessibility="no-hide-descendants" testID="rocket-day-line">
      {paths && (
        <Svg width={DAY_LINE_W} height={DAY_LINE_H} viewBox={`0 0 ${DAY_LINE_W} ${DAY_LINE_H}`}>
          {paths.baseY !== null && <Line x1={0} x2={DAY_LINE_W} y1={paths.baseY} y2={paths.baseY} stroke={UI.textFaint} strokeWidth={1} strokeDasharray="1.5 2.5" />}
          <Path d={paths.area} fill={UI.textDim} fillOpacity={0.14} />
          <Path d={paths.line} fill="none" stroke={UI.textDim} strokeWidth={1.5} strokeLinejoin="round" />
        </Svg>
      )}
    </View>
  );
}

/** Pullback low (left, red: the failure level) to trigger (right, neutral), the price as a dot: the Ross row's meter. */
function Meter({ value, color }: { value: number; color: string }) {
  const v = Math.max(0, Math.min(1, value));
  return (
    <Svg width="100%" height={8} viewBox="0 0 160 8" preserveAspectRatio="none" accessibilityLabel={`price ${Math.round(v * 100)}% of the way from the pullback low to the trigger`}>
      <Rect x={0} y={3} width={160} height={2} rx={1} fill={UI.raised} />
      <Rect x={0} y={0} width={2} height={8} fill={UI.critical} />
      <Rect x={158} y={0} width={2} height={8} fill={UI.textDim} />
      <Rect x={0} y={3} width={v * 156} height={2} rx={1} fill={color} />
      <Circle cx={2 + v * 154} cy={4} r={3} fill={color} />
    </Svg>
  );
}

/** A dot that breathes on a fresh flag or a breakout; still with reduced motion. */
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
  card: { borderRadius: 14, backgroundColor: UI.panel, paddingVertical: 10, paddingHorizontal: 8, gap: CARD_GAP },
  head: { flexDirection: "row", alignItems: "center", gap: 8, paddingHorizontal: 4 },
  title: { color: UI.text, fontSize: 14, fontFamily: fonts.semibold },
  chip: { borderRadius: 4, paddingHorizontal: 6, paddingVertical: 1, backgroundColor: UI.raised },
  chipText: { color: UI.textDim, fontSize: 10, fontFamily: fonts.regular },
  count: { flexDirection: "row", alignItems: "center", gap: 5, height: 22, paddingHorizontal: 8, borderRadius: 11, backgroundColor: "rgba(139,156,255,0.16)" },
  countText: { color: "#c3cbff", fontSize: 11 },
  record: { marginLeft: "auto", flexShrink: 1, color: UI.textDim, fontSize: 11, fontVariant: ["tabular-nums"] },
  sub: { color: UI.textFaint, fontSize: 10, paddingHorizontal: 4, fontVariant: ["tabular-nums"] },
  // Bursts on top: a heading, its one-line context and its rows; separated by space, never borders.
  section: { gap: 6, paddingBottom: 4 },
  sectionHead: { flexDirection: "row", alignItems: "center", gap: 6, paddingHorizontal: 4 },
  sectionTitle: { color: UI.text, fontSize: 12, fontFamily: fonts.semibold },
  sectionPad: { paddingHorizontal: 4 },
  flagsHead: { paddingBottom: CARD_GAP },
  // The card's own row gap: the rows are separated by space, never borders.
  rows: { gap: 6 },
  dimmed: { opacity: 0.6 },
  row: { gap: 4, paddingVertical: 8, paddingHorizontal: 10, borderRadius: 10, backgroundColor: UI.row, overflow: "hidden" },
  wash: { backgroundColor: "rgba(139,156,255,0.10)" },
  edge: { position: "absolute", left: 0, top: 0, bottom: 0, width: 3 },
  top: { flexDirection: "row", alignItems: "center", gap: 8 },
  lines: { flex: 1, minWidth: 0, gap: 4 },
  chart: { width: DAY_LINE_W, height: DAY_LINE_H },
  line: { flexDirection: "row", alignItems: "center", gap: 7 },
  symbol: { color: UI.text, fontSize: 14, fontFamily: fonts.semibold },
  price: { color: UI.text, fontSize: 13, fontVariant: ["tabular-nums"] },
  small: { fontSize: 12, fontVariant: ["tabular-nums"] },
  source: { flexShrink: 0, borderRadius: 4, paddingHorizontal: 4, paddingVertical: 1, backgroundColor: UI.raised },
  sourceText: { color: UI.textDim, fontSize: 9, fontFamily: fonts.semibold },
  badge: { borderRadius: 4, paddingHorizontal: 5, paddingVertical: 1, backgroundColor: UI.warning },
  badgeText: { color: UI.accentFg, fontSize: 9, fontFamily: fonts.semibold },
  age: { marginLeft: "auto", flexDirection: "row", alignItems: "center", gap: 4 },
  state: { flexShrink: 1, fontSize: 12 },
  faint: { color: UI.textFaint, fontFamily: fonts.regular },
  note: { color: UI.textDim, fontFamily: fonts.regular, fontVariant: ["tabular-nums"] },
  metric: { fontSize: 11, fontVariant: ["tabular-nums"] },
  chips: { flex: 1, flexDirection: "row", gap: 5, overflow: "hidden" },
  reason: { flexShrink: 1, borderRadius: 4, paddingHorizontal: 6, paddingVertical: 1, backgroundColor: UI.raised },
  reasonText: { color: UI.textDim, fontSize: 11, fontFamily: fonts.regular },
  outcome: { fontSize: 11, fontFamily: fonts.semibold },
  score: { color: UI.textFaint, fontSize: 10, fontVariant: ["tabular-nums"] },
});

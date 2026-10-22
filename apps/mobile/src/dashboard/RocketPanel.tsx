import { UI } from "@stockwatcher/chart-native";
import { newRocketFlags, rocketHeaderView, rocketNow, rocketRowView, type RocketChannel, type RocketRowView, type RocketTone } from "@stockwatcher/shared";
import { memo, useEffect, useRef, useState } from "react";
import { Animated, Pressable, StyleSheet, Text, View } from "react-native";

import { useRocket } from "../feed/hooks";
import { notifyRocketFlags, useRocketNotify } from "../notifications/rocket";
import { fonts } from "../theme";
import { useTickerPress } from "../ui/useTickerPress";

const TONE: Record<RocketTone, string> = {
  good: UI.good,
  accent: UI.accent,
  text: UI.text,
  muted: UI.textDim,
  dim: UI.textFaint,
};

/**
 * Rocket watch on the phone (owner, 2026-10-21; EXPERIMENTAL): its own card under the Ross card,
 * the web tab's rows stacked for the narrow screen (the Fold's cover screen and unfolded alike:
 * rows take the card's width, chips truncate). Accent only for a flag under a minute old; green
 * only for an outcome that rocketed. Tapping a row opens its chart (the chart modal); tap, then
 * hold copies the ticker. With Settings → Rocket watch on, each new flag also rings a local
 * notification. No layout animation on the rows: a moving Pressable takes stale taps on Android.
 */
export function RocketPanel({ channel, onPick, reducedMotion }: { channel: RocketChannel; onPick: (symbol: string) => void; reducedMotion: boolean }) {
  const state = useRocket(channel);
  const notify = useRocketNotify();
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
    if (notify && fresh.length) notifyRocketFlags(fresh);
  }, [state, notify]);
  const { enabled, panel } = state;
  const now = rocketNow(state, wall);
  const rows = panel?.rows.map((r) => rocketRowView(r, now)) ?? [];
  const head = panel ? rocketHeaderView(panel) : null;
  return (
    <View style={styles.card} accessibilityLabel="Rocket watch">
      <View style={styles.head}>
        <Text accessibilityRole="header" style={styles.title}>
          Rocket watch
        </Text>
        <View style={styles.chip}>
          <Text style={styles.chipText}>Experimental</Text>
        </View>
        {head && (
          <Text numberOfLines={1} style={styles.record}>
            {head.record}
          </Text>
        )}
      </View>
      {head && (head.strategy || head.history) ? (
        <Text numberOfLines={1} style={styles.sub}>
          {[head.placeholder ? `${head.strategy} (placeholder)` : head.strategy, head.history].filter(Boolean).join(" · ")}
        </Text>
      ) : null}
      {panel && !state.connected && (
        <Text accessibilityRole="alert" style={[styles.sub, { color: UI.warning }]}>
          Reconnecting · last known flags
        </Text>
      )}
      {enabled === false ? (
        <Text style={[styles.empty, { color: UI.warning }]}>Off on this server</Text>
      ) : enabled === null ? (
        <Text style={styles.empty}>Connecting…</Text>
      ) : rows.length === 0 ? (
        <Text style={styles.empty}>Nothing flagged right now</Text>
      ) : (
        <View style={[styles.rows, !state.connected && styles.dimmed]}>
          {rows.map((r) => (
            <Row key={r.key} row={r} onPick={onPick} reducedMotion={reducedMotion} />
          ))}
        </View>
      )}
    </View>
  );
}

const Row = memo(function Row({ row: r, onPick, reducedMotion }: { row: RocketRowView; onPick: (symbol: string) => void; reducedMotion: boolean }) {
  const press = useTickerPress(r.symbol, () => onPick(r.symbol));
  return (
    <Pressable accessibilityRole="button" accessibilityLabel={r.aria} accessibilityHint="Opens its chart. Tap, then press and hold to copy" {...press} style={({ pressed }) => [styles.row, pressed && { backgroundColor: UI.raised }]}>
      <View style={styles.line}>
        <Text style={styles.symbol}>{r.symbol}</Text>
        <Text style={styles.price}>{r.price}</Text>
        {r.gain ? <Text style={[styles.small, { color: UI.textDim }]}>{r.gain}</Text> : null}
        <View style={styles.age}>
          {r.fresh && <Pulse color={UI.accent} size={6} reducedMotion={reducedMotion} />}
          <Text style={[styles.small, r.fresh ? { color: UI.accent, fontFamily: fonts.semibold } : { color: UI.textFaint }]}>{r.age}</Text>
        </View>
      </View>
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

/** A dot that breathes on a fresh flag; still with reduced motion. */
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
  chip: { borderRadius: 4, paddingHorizontal: 6, paddingVertical: 1, backgroundColor: UI.raised },
  chipText: { color: UI.textDim, fontSize: 10, fontFamily: fonts.regular },
  record: { marginLeft: "auto", flexShrink: 1, color: UI.textDim, fontSize: 11, fontVariant: ["tabular-nums"] },
  sub: { color: UI.textFaint, fontSize: 10, paddingHorizontal: 4, fontVariant: ["tabular-nums"] },
  empty: { color: UI.textFaint, fontSize: 12, paddingHorizontal: 4, paddingVertical: 6 },
  // The card's own row gap: the rows are separated by space, never borders.
  rows: { gap: 6 },
  dimmed: { opacity: 0.6 },
  row: { gap: 4, paddingVertical: 8, paddingHorizontal: 10, borderRadius: 10, backgroundColor: UI.row, overflow: "hidden" },
  line: { flexDirection: "row", alignItems: "center", gap: 7 },
  symbol: { color: UI.text, fontSize: 14, fontFamily: fonts.semibold },
  price: { color: UI.text, fontSize: 13, fontVariant: ["tabular-nums"] },
  small: { fontSize: 12, fontVariant: ["tabular-nums"] },
  age: { marginLeft: "auto", flexDirection: "row", alignItems: "center", gap: 4 },
  chips: { flex: 1, flexDirection: "row", gap: 5, overflow: "hidden" },
  reason: { flexShrink: 1, borderRadius: 4, paddingHorizontal: 6, paddingVertical: 1, backgroundColor: UI.raised },
  reasonText: { color: UI.textDim, fontSize: 11, fontFamily: fonts.regular },
  outcome: { fontSize: 11, fontFamily: fonts.semibold },
  score: { color: UI.textFaint, fontSize: 10, fontVariant: ["tabular-nums"] },
});

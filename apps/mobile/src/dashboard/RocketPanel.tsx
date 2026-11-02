import { Icon, UI } from "@stockwatcher/chart-native";
import { formatEtTime } from "@stockwatcher/chart-core";
import { newRocketFlags, rocketHeaderView, rocketNow, rocketRowMarks, rocketSectionCounts, rocketSectionsView, type RocketChannel, type RocketRowView, type RocketSectionsView, type RocketTone, type RowMarks, withZoneSections } from "@stockwatcher/shared";
import { memo, useEffect, useRef, useState } from "react";
import { Animated, Pressable, StyleSheet, Text, View } from "react-native";

import { useRocket } from "../feed/hooks";
import { Flame, MarksLine, RowLine, useReadoutPress, useWall } from "./RowMarks";
import { notifyRocketFlags } from "../notifications/rocket";
import { phoneNotices } from "../notifications/runnersNotices";
import { Collapse } from "../motion/Collapse";
import { ZoneNote } from "./ZoneNote";
import { fonts } from "../theme";
import { useTickerPress } from "../ui/useTickerPress";
import { StarToggle } from "../ui/StarToggle";

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
const NO_MARKS: RowMarks = { flame: null, pillars: null, rvol: null, news: null, path: null, zone: null, stage: null, tail: null };
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
 * depth and trigger meter (fading rows are left out). Green only for the strict outcome tag. A state change
 * crossfades the edge and wash (opacity, native driver); with reduced motion it simply changes.
 * Tapping a row opens its chart (the chart modal); tap, then hold copies the ticker. New flags
 * reach the phone as a server push (Settings → Runners); while this phone has no push token the
 * server's `runners.notice` frames ring a local banner instead (notifications/runnersNotices.ts),
 * and only against a server older than those does the card diff its rows for banners itself
 * (notifications/rocket.ts). No layout
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
  // Ages tick between frames, by the wall clock only against a server without clock.v1 (the
  // server's clock frames re-render the card otherwise).
  const wall = useWall(15_000, state.serverEvents === true);
  // New flags since this card mounted (the first frame only primes: no burst on open), rung only
  // against a server without `runners.notice.v1`: a newer one sends the notices themselves
  // (notifications/runnersNotices.ts). The seen set keeps following either way, so a reconnect to
  // an older server does not announce the rows already shown.
  const seen = useRef(new Set<string>());
  const primed = useRef(false);
  useEffect(() => {
    if (!state.panel) return;
    const fresh = newRocketFlags(state, seen.current, primed.current);
    primed.current = true;
    if (fresh.length && !phoneNotices.serverSends()) notifyRocketFlags(fresh);
  }, [state]);
  const { panel } = state;
  const now = rocketNow(state, wall);
  const { bursts, runners: rows }: RocketSectionsView = panel ? withZoneSections(rocketSectionsView(panel, now), panel.zones) : { bursts: null, runners: [] };
  const head = panel ? rocketHeaderView(panel, state.connected) : null;
  // The lighter rows' marks (shared rowMarks.ts), by row key (the wire id).
  const marks = new Map((panel?.rows ?? []).map((w) => [w.id, { marks: rocketRowMarks(w, now, formatEtTime), trigger: w.levels?.trigger ?? null }]));
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
            <View style={[styles.rows, !state.connected && styles.dimmed]}>
              {bursts.rows.map((r) => (
                <Row key={r.key} row={r} marks={marks.get(r.key)?.marks ?? NO_MARKS} trigger={marks.get(r.key)?.trigger ?? null} onPick={onPick} reducedMotion={reducedMotion} />
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
              <Row key={r.key} row={r} marks={marks.get(r.key)?.marks ?? NO_MARKS} trigger={marks.get(r.key)?.trigger ?? null} onPick={onPick} reducedMotion={reducedMotion} />
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

/**
 * One Runners row as a lighter row (owner, 2026-10-26, design canvas "StockWatcher lighter rows",
 * phone artboard): ticker, price and day gain with the day line, then the marks line (the path
 * with its age, the momentum word, the outcome tag, the setup's stage). Press and hold reads every
 * mark out, the old sentences included (the path's label carries the move since the flag, the
 * later paths and the reasons). The flame lights on a fresh burst that holds its alert price.
 */
const Row = memo(function Row({ row: r, marks, trigger, onPick, reducedMotion }: { row: RocketRowView; marks: RowMarks; trigger: number | null; onPick: (symbol: string) => void; reducedMotion: boolean }) {
  const press = useReadoutPress(useTickerPress(r.symbol, () => onPick(r.symbol)), marks);
  const m = r.momentum;
  const b = r.burst;
  // A live burst takes the accent edge; once it settles the row falls back to its momentum edge
  // (the change crossfades like a state change).
  const fade = useCrossfade(`${m.state}${b?.live ? ":live" : ""}`, reducedMotion);
  const edgeTone = b?.live ? "accent" : m.edge;
  const edge = edgeTone ? TONE[edgeTone] : null;
  // Everything the old state and burst lines spelled out, spoken with the momentum word.
  const stateLabel = [b ? [b.headline, b.since, b.move].filter(Boolean).join(" · ") : "", m.label, m.provisional ? "provisional" : "", m.since, m.note].filter(Boolean).join(" · ");
  return (
    <Pressable accessibilityRole="button" accessibilityLabel={r.aria} accessibilityHint="Opens its chart. Press and hold to read its marks; tap, then press and hold to copy" {...press} style={({ pressed }) => [styles.row, marks.flame?.lit && { backgroundColor: UI.raised }, m.dimmed && { opacity: 0.6 }, pressed && { backgroundColor: UI.raised }]}>
      {/* The state's wash and inset edge: filled views, never borders, crossfading in on a change. */}
      {m.state === "breaking_out" && <Animated.View pointerEvents="none" style={[StyleSheet.absoluteFill, styles.wash, { opacity: fade }]} />}
      {edge && <Animated.View pointerEvents="none" style={[styles.edge, { backgroundColor: edge, opacity: fade }]} />}
      <View style={styles.line}>
        <Flame flame={marks.flame} />
        <Text style={styles.symbol}>{r.symbol}</Text>
        <Text style={styles.price}>{r.price}</Text>
        {r.gain ? <Text style={[styles.small, { color: UI.textDim }]}>{r.gain}</Text> : null}
        {r.fresh && (
          <View accessible accessibilityLabel="Flagged under a minute ago">
            <Pulse color={UI.accent} size={6} reducedMotion={reducedMotion} />
          </View>
        )}
        {r.halted && (
          <View style={styles.badge}>
            <Text style={styles.badgeText}>HALTED</Text>
          </View>
        )}
        {/* Neutral (green belongs to the strict outcome here); its tail still says the last minute. */}
        <View style={styles.chartSlot}>
          <RowLine line={r.line} tail={marks.tail} trigger={marks.stage ? trigger : null} width={DAY_LINE_W} height={DAY_LINE_H} color={UI.textDim} />
        </View>
        <StarToggle symbol={r.symbol} size={14} />
      </View>
      <MarksLine
        marks={marks}
        extra={
          <>
            {/* The momentum word (the row's verdict; a live burst says its headline), crossfading
                on a change. The rest of the old sentences is in the press-and-hold read-out. */}
            <Animated.View style={[styles.stateBox, { opacity: fade }]} accessible accessibilityLabel={stateLabel}>
              {(m.pulse || b?.live) && <Pulse color={b?.live ? UI.accent : TONE[m.tone]} size={6} reducedMotion={reducedMotion} />}
              <Text numberOfLines={1} style={[styles.state, { color: b ? (b.live ? UI.accent : UI.text) : TONE[m.tone], fontFamily: m.attention || b?.live ? fonts.semibold : fonts.regular }]}>
                {b?.live ? b.headline : m.label}
              </Text>
            </Animated.View>
            {r.outcome && <Text style={[styles.outcome, { color: TONE[r.outcome.tone] }]}>{r.outcome.label}</Text>}
          </>
        }
      />
      {/* The row's whole width (the Fold's cover screen needs it for the range and base rate). */}
      <ZoneNote note={r.zone ?? null} reducedMotion={reducedMotion} gap={ROW_GAP} />
    </Pressable>
  );
});

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
  chartSlot: { marginLeft: "auto" },
  stateBox: { flexDirection: "row", alignItems: "center", gap: 5, flexShrink: 1, minWidth: 0 },
  line: { flexDirection: "row", alignItems: "center", gap: 7 },
  symbol: { color: UI.text, fontSize: 14, fontFamily: fonts.semibold },
  price: { color: UI.text, fontSize: 13, fontVariant: ["tabular-nums"] },
  small: { fontSize: 12, fontVariant: ["tabular-nums"] },
  source: { flexShrink: 0, borderRadius: 4, paddingHorizontal: 4, paddingVertical: 1, backgroundColor: UI.raised },
  sourceText: { color: UI.textDim, fontSize: 9, fontFamily: fonts.semibold },
  alsoText: { flexShrink: 1, color: UI.textFaint, fontSize: 9, fontFamily: fonts.regular },
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

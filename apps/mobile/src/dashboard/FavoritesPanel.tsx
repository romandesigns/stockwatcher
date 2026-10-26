import { formatEtTime } from "@stockwatcher/chart-core";
import { Icon, UI } from "@stockwatcher/chart-native";
import { favoriteRows, rossLinePaths, type FavoriteRowView, type FavoritesChannel, type RossTone } from "@stockwatcher/shared";
import { memo, useMemo, useState } from "react";
import { Pressable, StyleSheet, Text, View, type LayoutChangeEvent } from "react-native";
import Svg, { Line, Path } from "react-native-svg";

import { useFavorites } from "../feed/hooks";
import { Collapse } from "../motion/Collapse";
import { appActions, useApp } from "../state/store";
import { fonts } from "../theme";
import { StarToggle } from "../ui/StarToggle";
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

const hhmm = (ms: number) => formatEtTime(ms);

/**
 * The Favorites card on the phone (owner, 2026-10-26; the web's Favorites tab): the owner's own
 * tickers, in their order, each with its price and day change, the day's line, volume, RVOL,
 * distance from the high and the context the server has (today's Runners entry, the pullback
 * trigger and stop, pillars and news, a halt). A row quiet for five minutes says so and dims.
 * Tapping a row opens its chart; tap, then hold copies the ticker; the star takes it off; the
 * arrows move it. No favorites, no card (the dashboard's rule): stars on the chart, the search,
 * Runners and Ross rows add them. The row chart's width follows the row (Fold cover to unfolded).
 */
export function FavoritesPanel({ channel, onPick, reducedMotion, gap = 0 }: { channel: FavoritesChannel; onPick: (symbol: string) => void; reducedMotion: boolean; gap?: number }) {
  const list = useApp((s) => s.favorites);
  const state = useFavorites(channel);
  const rows = useMemo(() => favoriteRows(list, state, hhmm), [list, state]);
  const reconnecting = !state.connected && state.rows !== null;
  return (
    <Collapse open={list.length > 0} reducedMotion={reducedMotion} gap={gap}>
      <View style={styles.card} accessibilityLabel="Favorites">
        <View style={styles.head}>
          <Text accessibilityRole="header" style={styles.title}>
            Favorites
          </Text>
          <Text style={styles.count}>{list.length}</Text>
        </View>
        {reconnecting && (
          <Text accessibilityRole="alert" style={styles.stale}>
            Reconnecting · last known rows
          </Text>
        )}
        <View style={[styles.rows, reconnecting && styles.dimmed]}>
          {rows.map((r, i) => (
            <Row key={r.symbol} symbol={r.symbol} row={r.view} first={i === 0} last={i === rows.length - 1} onPick={onPick} />
          ))}
        </View>
      </View>
    </Collapse>
  );
}

const Row = memo(function Row({ symbol, row: r, first, last, onPick }: { symbol: string; row: FavoriteRowView | null; first: boolean; last: boolean; onPick: (symbol: string) => void }) {
  const press = useTickerPress(symbol, () => onPick(symbol));
  // The row chart takes about a quarter of the row: 72 dp on the Fold's cover screen, wider unfolded.
  const [width, setWidth] = useState(0);
  const onLayout = (e: LayoutChangeEvent) => setWidth(e.nativeEvent.layout.width);
  const chartW = Math.max(72, Math.min(160, Math.round(width * 0.26)));
  const facts = r ? [r.volume && `${r.volume} vol`, r.rvol, r.fromHigh, r.pmHigh].filter(Boolean).join("  ·  ") : "";
  const chips = r ? r.chips.filter((c) => c.label !== "Halted") : [];
  return (
    <Pressable
      accessibilityRole="button"
      accessibilityLabel={r?.aria || symbol}
      accessibilityHint="Opens its chart. Tap, then press and hold to copy"
      {...press}
      onLayout={onLayout}
      style={({ pressed }) => [styles.row, r?.stale && styles.dimmed, pressed && { backgroundColor: UI.raised }]}
    >
      <View style={styles.line}>
        <Text style={styles.symbol}>{symbol}</Text>
        {r && !r.empty && <Text style={styles.price}>{r.price}</Text>}
        {r?.change ? <Text style={[styles.small, { color: TONE[r.changeTone] }]}>{r.change}</Text> : null}
        {r?.halted && (
          <View style={styles.badge}>
            <Text style={styles.badgeText}>HALTED</Text>
          </View>
        )}
        <View style={styles.controls}>
          {!first && <Move symbol={symbol} up />}
          {!last && <Move symbol={symbol} />}
          <StarToggle symbol={symbol} quiet />
        </View>
      </View>
      {!r ? (
        <Text style={styles.note}>Loading…</Text>
      ) : r.empty ? (
        <Text style={styles.note}>No trades today yet</Text>
      ) : (
        <View style={styles.body}>
          <View style={styles.lines}>
            {facts ? <Text style={styles.facts}>{facts}</Text> : null}
            {chips.length > 0 && (
              <View style={styles.chips}>
                {chips.map((c) => (
                  <View key={c.label} style={styles.chip}>
                    <Text style={[styles.chipText, { color: TONE[c.tone] }]}>{c.label}</Text>
                  </View>
                ))}
              </View>
            )}
            {r.stale ? <Text style={styles.note}>{r.stale}</Text> : null}
          </View>
          <DayLine line={r.line} width={chartW} />
        </View>
      )}
    </Pressable>
  );
});

function Move({ symbol, up = false }: { symbol: string; up?: boolean }) {
  return (
    <Pressable
      accessibilityRole="button"
      accessibilityLabel={`Move ${symbol} ${up ? "up" : "down"}`}
      onPress={() => appActions().moveFavorite(symbol, up ? -1 : 1)}
      hitSlop={8}
      style={({ pressed }) => [styles.move, pressed && { backgroundColor: UI.raised }]}
    >
      <Icon name={up ? "chevronUp" : "chevronDown"} color={UI.textFaint} size={12} />
    </Pressable>
  );
}

const LINE_H = 32;

/** The day's line ending at the live price, the previous close dotted when near (`rossLinePaths`). */
function DayLine({ line, width }: { line: FavoriteRowView["line"]; width: number }) {
  const paths = useMemo(() => (line ? rossLinePaths(line, width, LINE_H) : null), [line, width]);
  const color = line?.tone === "critical" ? UI.critical : UI.good;
  return (
    <View style={{ width, height: LINE_H }} accessibilityElementsHidden importantForAccessibility="no-hide-descendants" testID="favorite-day-line">
      {paths && (
        <Svg width={width} height={LINE_H} viewBox={`0 0 ${width} ${LINE_H}`}>
          {paths.baseY !== null && <Line x1={0} x2={width} y1={paths.baseY} y2={paths.baseY} stroke={UI.textFaint} strokeWidth={1} strokeDasharray="1.5 2.5" />}
          <Path d={paths.area} fill={color} fillOpacity={0.14} />
          <Path d={paths.line} fill="none" stroke={color} strokeWidth={1.5} strokeLinejoin="round" />
        </Svg>
      )}
    </View>
  );
}

const styles = StyleSheet.create({
  card: { borderRadius: 14, backgroundColor: UI.panel, paddingVertical: 10, paddingHorizontal: 8, gap: 6 },
  head: { flexDirection: "row", alignItems: "center", gap: 8, paddingHorizontal: 4 },
  title: { color: UI.text, fontSize: 14, fontFamily: fonts.semibold },
  count: { color: UI.textFaint, fontSize: 11, fontVariant: ["tabular-nums"] },
  stale: { color: UI.warning, fontSize: 11, paddingHorizontal: 4, paddingBottom: 2 },
  rows: { gap: 6 },
  dimmed: { opacity: 0.6 },
  row: { gap: 4, paddingVertical: 8, paddingHorizontal: 10, borderRadius: 10, backgroundColor: UI.row },
  line: { flexDirection: "row", alignItems: "center", gap: 7 },
  symbol: { color: UI.text, fontSize: 14, fontFamily: fonts.semibold },
  price: { color: UI.text, fontSize: 13, fontVariant: ["tabular-nums"] },
  small: { fontSize: 12, fontVariant: ["tabular-nums"] },
  badge: { borderRadius: 4, paddingHorizontal: 5, paddingVertical: 1, backgroundColor: UI.warning },
  badgeText: { color: UI.accentFg, fontSize: 9, fontFamily: fonts.semibold },
  controls: { marginLeft: "auto", flexDirection: "row", alignItems: "center", gap: 2 },
  move: { width: 28, height: 28, borderRadius: 8, alignItems: "center", justifyContent: "center" },
  body: { flexDirection: "row", alignItems: "center", gap: 8 },
  lines: { flex: 1, minWidth: 0, gap: 4 },
  facts: { color: UI.textDim, fontSize: 11, fontVariant: ["tabular-nums"] },
  chips: { flexDirection: "row", flexWrap: "wrap", gap: 4 },
  chip: { borderRadius: 4, paddingHorizontal: 5, paddingVertical: 1, backgroundColor: UI.raised },
  chipText: { fontSize: 10, fontFamily: fonts.semibold },
  note: { color: UI.textFaint, fontSize: 11, fontFamily: fonts.regular },
});

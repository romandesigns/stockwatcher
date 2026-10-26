import { formatEtTime } from "@stockwatcher/chart-core";
import { Icon, UI } from "@stockwatcher/chart-native";
import { favoriteRowMarks, favoriteRows, rocketNow, runnersBySymbol, type FavoriteRowView, type FavoritesChannel, type RocketChannel, type RossTone, type RowMarks } from "@stockwatcher/shared";
import { memo, useMemo, useState } from "react";
import { Pressable, StyleSheet, Text, View, type LayoutChangeEvent } from "react-native";

import { useFavorites, useRocket } from "../feed/hooks";
import { Flame, MarksLine, RowLine, useReadoutPress, useWall } from "./RowMarks";
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
export function FavoritesPanel({ channel, runners, onPick, reducedMotion, gap = 0 }: { channel: FavoritesChannel; runners: RocketChannel; onPick: (symbol: string) => void; reducedMotion: boolean; gap?: number }) {
  const list = useApp((s) => s.favorites);
  const state = useFavorites(channel);
  const rows = useMemo(() => favoriteRows(list, state, hhmm), [list, state]);
  // The lighter rows' marks (shared rowMarks.ts), joined to today's Runners row for the flame.
  const rocket = useRocket(runners);
  const wall = useWall();
  const nowMs = rocketNow(rocket, wall);
  const marks = useMemo(() => {
    const byRunner = runnersBySymbol(rocket);
    const bySymbol = new Map(state.rows?.map((r) => [r.symbol, r]) ?? []);
    return new Map(
      list.flatMap((s) => {
        const w = bySymbol.get(s);
        return w ? [[s, { marks: favoriteRowMarks(w, byRunner.get(s), nowMs, hhmm), trigger: w.levels?.trigger ?? null }] as const] : [];
      }),
    );
  }, [list, state, rocket, nowMs]);
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
            <Row key={r.symbol} symbol={r.symbol} row={r.view} marks={marks.get(r.symbol)?.marks ?? null} trigger={marks.get(r.symbol)?.trigger ?? null} first={i === 0} last={i === rows.length - 1} onPick={onPick} />
          ))}
        </View>
      </View>
    </Collapse>
  );
}

/**
 * One favorite as a lighter row (owner, 2026-10-26, design canvas "StockWatcher lighter rows"):
 * ticker, price and change with the day line, then the marks line; press and hold reads every mark
 * out, with the volume, distance from the high and premarket high the row used to spell out. On
 * the narrowest rows (the Fold's cover screen) the line moves to the end of the marks line, so the
 * move and star controls keep their room. A stale row says how long ago it last traded.
 */
const Row = memo(function Row({ symbol, row: r, marks: m, trigger, first, last, onPick }: { symbol: string; row: FavoriteRowView | null; marks: RowMarks | null; trigger: number | null; first: boolean; last: boolean; onPick: (symbol: string) => void }) {
  const facts = r ? [r.volume && `${r.volume} volume`, r.fromHigh, r.pmHigh].filter(Boolean).join(" · ") : "";
  // The read-out adds the facts the row stopped spelling out to the marks' labels.
  const readMarks = useMemo(() => (m && facts ? { ...m, tail: { tone: m.tail?.tone ?? "muted", label: [m.tail?.label, facts].filter(Boolean).join(" · ") } } : m), [m, facts]);
  const press = useReadoutPress(useTickerPress(symbol, () => onPick(symbol)), readMarks);
  // The row chart takes about a quarter of the row: 72 dp on the Fold's cover screen, wider unfolded.
  const [width, setWidth] = useState(0);
  const onLayout = (e: LayoutChangeEvent) => setWidth(e.nativeEvent.layout.width);
  const chartW = Math.max(72, Math.min(160, Math.round(width * 0.26)));
  const narrow = width > 0 && width < 340;
  const chart = r && !r.empty ? <RowLine line={r.line} tail={m?.tail ?? null} trigger={m?.stage ? trigger : null} width={chartW} color={r.line?.tone === "critical" ? UI.critical : UI.good} /> : null;
  return (
    <Pressable
      accessibilityRole="button"
      accessibilityLabel={r?.aria || symbol}
      accessibilityHint="Opens its chart. Press and hold to read its marks; tap, then press and hold to copy"
      {...press}
      onLayout={onLayout}
      style={({ pressed }) => [styles.row, m?.flame?.lit && { backgroundColor: UI.raised }, r?.stale && styles.dimmed, pressed && { backgroundColor: UI.raised }]}
    >
      <View style={styles.line}>
        <Flame flame={m?.flame ?? null} />
        <Text style={styles.symbol}>{symbol}</Text>
        {r && !r.empty && <Text style={styles.price}>{r.price}</Text>}
        {r?.change ? <Text style={[styles.small, { color: TONE[r.changeTone] }]}>{r.change}</Text> : null}
        {r?.halted && (
          <View style={styles.badge}>
            <Text style={styles.badgeText}>HALTED</Text>
          </View>
        )}
        <View style={styles.controls}>
          {!narrow && chart}
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
          <View style={styles.lines}>{m && <MarksLine marks={m} extra={r.stale ? <Text style={styles.note}>{r.stale}</Text> : undefined} />}</View>
          {narrow && chart}
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

import { formatPrice, formatVolume } from "@stockwatcher/chart-core";
import { mostActiveTickers, tickerUniverse, type QuoteChannel, type TickerInfo } from "@stockwatcher/shared";
import { memo, useMemo } from "react";
import { FlatList, Pressable, StyleSheet, Text, View } from "react-native";

import { useQuoteBoard } from "../feed/hooks";
import { colors, fonts } from "../theme";
import { useTickerPress } from "../ui/useTickerPress";

const LIMIT = 40;

/**
 * The phone dashboard's live ticker list: the most active price-band stocks with live price,
 * change % and volume from the quote board. Tapping a row loads it into the selected chart.
 * The web's TickerList for React Native.
 */
export function TickerList({ quotes, symbols, selectedSymbol, selectedIndex, onPick }: { quotes: QuoteChannel; symbols: readonly string[]; selectedSymbol: string; selectedIndex: number; onPick: (symbol: string) => void }) {
  const board = useQuoteBoard(quotes, true);
  const rows = useMemo(() => mostActiveTickers(tickerUniverse(symbols, board), LIMIT), [symbols, board]);
  return (
    <View style={styles.box} accessibilityLabel="Most active tickers">
      <View style={styles.head}>
        <Text style={styles.title} accessibilityRole="header">
          Most active
        </Text>
        <Text style={styles.hint}>Loads into chart {selectedIndex + 1}</Text>
      </View>
      {rows.length === 0 ? (
        <Text style={styles.empty}>Waiting for live quotes…</Text>
      ) : (
        <FlatList data={rows} keyExtractor={(r) => r.symbol} renderItem={({ item }) => <Row row={item} active={item.symbol === selectedSymbol} onPick={onPick} />} />
      )}
    </View>
  );
}

const Row = memo(function Row({ row, active, onPick }: { row: TickerInfo; active: boolean; onPick: (s: string) => void }) {
  const down = (row.changePct ?? 0) < 0;
  // Tap: load into the selected chart; tap, then hold: copy the ticker.
  const press = useTickerPress(row.symbol, () => onPick(row.symbol));
  return (
    <Pressable
      accessibilityRole="button"
      accessibilityLabel={`${row.symbol}, load into the selected chart`}
      accessibilityHint="Tap, then press and hold to copy"
      accessibilityState={{ selected: active }}
      {...press}
      style={({ pressed }) => [styles.row, (active || pressed) && { backgroundColor: colors.bgRow }]}
    >
      <Text style={styles.symbol} numberOfLines={1}>
        {row.symbol}
      </Text>
      <Text style={styles.num}>{row.price !== undefined ? formatPrice(row.price) : "—"}</Text>
      <Text style={[styles.num, styles.pct, { color: down ? colors.critical : colors.good }]}>{row.changePct !== undefined ? `${down ? "" : "+"}${row.changePct.toFixed(2)}%` : ""}</Text>
      <Text style={[styles.num, styles.vol]}>{formatVolume(row.volume ?? 0)}</Text>
    </Pressable>
  );
});

const styles = StyleSheet.create({
  box: { backgroundColor: colors.bgPanel, borderRadius: 12, paddingVertical: 6, paddingHorizontal: 6, gap: 2 },
  head: { flexDirection: "row", alignItems: "baseline", justifyContent: "space-between", paddingHorizontal: 4, paddingBottom: 2 },
  title: { color: colors.textDim, fontSize: 12, fontFamily: fonts.semibold },
  hint: { color: colors.textFaint, fontSize: 11 },
  empty: { color: colors.textFaint, fontSize: 12, padding: 6 },
  row: { flexDirection: "row", alignItems: "center", gap: 10, minHeight: 36, paddingHorizontal: 6, borderRadius: 8 },
  symbol: { flex: 1, color: colors.text, fontSize: 13, fontFamily: fonts.semibold },
  num: { color: colors.textDim, fontSize: 12, fontVariant: ["tabular-nums"], textAlign: "right" },
  pct: { width: 62 },
  vol: { width: 48, color: colors.textFaint },
});

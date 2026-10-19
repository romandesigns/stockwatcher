import { formatEtTime, formatPrice } from "@stockwatcher/chart-core";
import { UI } from "@stockwatcher/chart-native";
import { haltLine, haltSentence, pressurePhrase, type PressureCard, type PressureChannel } from "@stockwatcher/shared";
import { memo, useState } from "react";
import { Pressable, ScrollView, StyleSheet, Text, View } from "react-native";

import { usePressureBoard } from "../feed/hooks";
import { fonts } from "../theme";
import { useTickerPress } from "../ui/useTickerPress";
import { PressureGauge } from "./PressureGauge";

const hhmm = (ms: number) => formatEtTime(ms);
const GAP = 8;

function changeText(pct: number): string {
  return `${pct >= 0 ? "+" : ""}${pct.toFixed(2)}%`;
}

/**
 * The phone dashboard's Halt watcher (the web's HaltWatcher; design canvas "Halt Watcher", Phone
 * and Fold boards): the server's top 6 symbols by one-sided pressure, compact gauge cards in
 * `columns` columns (one on a phone, two on the unfolded Fold), scrolling inside the region the
 * screen gives it. Tap a card to load its ticker into the selected chart; tap, then hold copies it.
 */
export function HaltWatcher({ channel, columns, onPick, reducedMotion }: { channel: PressureChannel; columns: 1 | 2; onPick: (symbol: string) => void; reducedMotion: boolean }) {
  const board = usePressureBoard(channel);
  const [width, setWidth] = useState(0);
  const cardWidth = width > 0 ? (width - GAP * (columns - 1)) / columns : 0;
  return (
    <View style={styles.box} onLayout={(e) => setWidth(e.nativeEvent.layout.width)}>
      {board.length === 0 ? (
        <Text style={styles.empty}>No stock in the price band has traded enough in the last minute to measure.</Text>
      ) : (
        <ScrollView contentContainerStyle={styles.grid} accessibilityLabel="Halt watcher, top 6 by pressure">
          {cardWidth > 0 && board.map((c) => <GaugeCard key={c.symbol} card={c} width={cardWidth} onPick={onPick} reducedMotion={reducedMotion} />)}
        </ScrollView>
      )}
    </View>
  );
}

const GaugeCard = memo(function GaugeCard({ card, width, onPick, reducedMotion }: { card: PressureCard; width: number; onPick: (symbol: string) => void; reducedMotion: boolean }) {
  const press = useTickerPress(card.symbol, () => onPick(card.symbol));
  const halted = card.halt !== undefined;
  const phrase = pressurePhrase(card);
  // The gauge column is 46 % of the card, as designed, capped so a wide card stays compact.
  const gaugeWidth = Math.min(132, Math.max(80, (width - 24) * 0.46));
  const status = card.halt ? haltSentence(card.halt, hhmm) : `${changeText(card.changePct)} today`;
  return (
    <Pressable
      accessibilityRole="button"
      accessibilityLabel={`${card.symbol} ${formatPrice(card.price)}, ${status}, ${phrase}`}
      accessibilityHint="Loads it into the selected chart. Tap, then press and hold to copy"
      {...press}
      style={({ pressed }) => [styles.card, { width }, pressed && { backgroundColor: UI.row }]}
    >
      <PressureGauge pressure={card.pressure} side={card.side} halted={halted} label={`${halted ? "Halted. " : ""}${phrase}`} width={gaugeWidth} reducedMotion={reducedMotion} />
      <View style={styles.info}>
        <View style={styles.top}>
          <Text style={styles.symbol} numberOfLines={1}>
            {card.symbol}
          </Text>
          <Text style={[styles.right, { color: halted ? UI.warning : card.changePct < 0 ? UI.critical : UI.good }]} numberOfLines={1}>
            {card.halt ? haltLine(card.halt, hhmm) : changeText(card.changePct)}
          </Text>
        </View>
        <Text style={styles.price}>{formatPrice(card.price)}</Text>
      </View>
    </Pressable>
  );
});

const styles = StyleSheet.create({
  box: { flex: 1, minHeight: 0 },
  grid: { flexDirection: "row", flexWrap: "wrap", gap: GAP },
  empty: { color: UI.textFaint, fontSize: 12, paddingVertical: 6 },
  card: { flexDirection: "row", alignItems: "center", gap: 12, backgroundColor: UI.panel, borderRadius: 14, paddingVertical: 8, paddingHorizontal: 12 },
  info: { flex: 1, minWidth: 0, gap: 2 },
  top: { flexDirection: "row", alignItems: "baseline", gap: 8 },
  symbol: { flexShrink: 1, color: UI.text, fontSize: 15, fontFamily: fonts.semibold },
  right: { marginLeft: "auto", fontSize: 12, fontVariant: ["tabular-nums"] },
  price: { color: UI.text, fontSize: 19, fontVariant: ["tabular-nums"], fontFamily: fonts.regular },
});

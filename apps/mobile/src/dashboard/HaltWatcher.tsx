import { formatEtTime, formatPrice } from "@stockwatcher/chart-core";
import { UI } from "@stockwatcher/chart-native";
import { haltLine, haltSentence, pressurePhrase, type PressureCard, type PressureChannel } from "@stockwatcher/shared";
import { memo, useState } from "react";
import { Pressable, StyleSheet, Text, View } from "react-native";

import { usePressureBoard } from "../feed/hooks";
import { fonts } from "../theme";
import { useTickerPress } from "../ui/useTickerPress";
import { PressureGauge } from "./PressureGauge";

const hhmm = (ms: number) => formatEtTime(ms);
const GAP = 6;

function changeText(pct: number): string {
  return `${pct >= 0 ? "+" : ""}${pct.toFixed(2)}%`;
}

/**
 * The phone dashboard's Halt watcher (the web's HaltWatcher; design canvas "Halt Watcher"): the
 * server's top 6 symbols by one-sided pressure as ONE row of mini gauges across the screen
 * (owner, 2026-10-19: "the gauge UI should be smaller, fit them all on a single row"), so all six
 * are in view without scrolling, above the charts. Each cell: the dial with its 0–100 reading,
 * the ticker and the change (the price too when the cell is wide enough, on the unfolded Fold).
 * Tap a cell to open its ticker's chart (the chart modal); tap, then hold copies it.
 */
export function HaltWatcher({ channel, onPick, reducedMotion }: { channel: PressureChannel; onPick: (symbol: string) => void; reducedMotion: boolean }) {
  const board = usePressureBoard(channel);
  const [width, setWidth] = useState(0);
  // Six equal cells, whatever the board holds, so a short board does not stretch two dials wide.
  const cellWidth = width > 0 ? (width - GAP * (SLOTS - 1)) / SLOTS : 0;
  return (
    <View onLayout={(e) => setWidth(e.nativeEvent.layout.width)} accessibilityLabel="Halt watcher, top 6 by pressure">
      {board.length === 0 ? (
        <Text style={styles.empty}>Halt watcher: no stock in the price band has traded enough in the last minute.</Text>
      ) : (
        <View style={styles.row}>{cellWidth > 0 && board.slice(0, SLOTS).map((c) => <MiniCard key={c.symbol} card={c} width={cellWidth} onPick={onPick} reducedMotion={reducedMotion} />)}</View>
      )}
    </View>
  );
}

const SLOTS = 6;

const MiniCard = memo(function MiniCard({ card, width, onPick, reducedMotion }: { card: PressureCard; width: number; onPick: (symbol: string) => void; reducedMotion: boolean }) {
  const press = useTickerPress(card.symbol, () => onPick(card.symbol));
  const halted = card.halt !== undefined;
  const phrase = pressurePhrase(card);
  const status = card.halt ? haltSentence(card.halt, hhmm) : `${changeText(card.changePct)} today`;
  // Wide cells (the unfolded Fold) have room for the price under the ticker.
  const roomy = width >= 110;
  return (
    <Pressable
      accessibilityRole="button"
      accessibilityLabel={`${card.symbol} ${formatPrice(card.price)}, ${status}, ${phrase}`}
      accessibilityHint="Opens its chart. Tap, then press and hold to copy"
      {...press}
      style={({ pressed }) => [styles.cell, { width }, pressed && { backgroundColor: UI.row }]}
    >
      <PressureGauge pressure={card.pressure} side={card.side} halted={halted} label={`${halted ? "Halted. " : ""}${phrase}`} width={Math.min(width - 8, 120)} reducedMotion={reducedMotion} compact={!roomy} />
      <Text style={styles.symbol} numberOfLines={1} adjustsFontSizeToFit minimumFontScale={0.7}>
        {card.symbol}
      </Text>
      {roomy && <Text style={styles.price}>{formatPrice(card.price)}</Text>}
      <Text style={[styles.change, { color: halted ? UI.warning : card.changePct < 0 ? UI.critical : UI.good }]} numberOfLines={1} adjustsFontSizeToFit minimumFontScale={0.7}>
        {card.halt ? (roomy ? haltLine(card.halt, hhmm) : "Halted") : changeText(card.changePct)}
      </Text>
    </Pressable>
  );
});

const styles = StyleSheet.create({
  row: { flexDirection: "row", gap: GAP },
  empty: { color: UI.textFaint, fontSize: 12, paddingVertical: 6 },
  cell: { alignItems: "center", gap: 1, backgroundColor: UI.panel, borderRadius: 10, paddingTop: 6, paddingBottom: 5, paddingHorizontal: 2 },
  symbol: { color: UI.text, fontSize: 11.5, fontFamily: fonts.semibold, maxWidth: "100%" },
  price: { color: UI.text, fontSize: 13, fontVariant: ["tabular-nums"], fontFamily: fonts.regular },
  change: { fontSize: 10, fontVariant: ["tabular-nums"], maxWidth: "100%" },
});

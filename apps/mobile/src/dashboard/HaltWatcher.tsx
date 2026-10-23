import { formatEtTime, formatPrice } from "@stockwatcher/chart-core";
import { UI } from "@stockwatcher/chart-native";
import { buildupCue, buildupCueShort, buildupSentence, haltLine, haltSentence, pressureFreshnessLine, riskPhrase, type PressureCard, type PressureChannel } from "@stockwatcher/shared";
import { memo, useState } from "react";
import { Pressable, StyleSheet, Text, View } from "react-native";

import { usePressureBoard, usePressureStatus } from "../feed/hooks";
import { Collapse } from "../motion/Collapse";
import { fonts } from "../theme";
import { useTickerPress } from "../ui/useTickerPress";
import { PressureGauge } from "./PressureGauge";

const hhmm = (ms: number) => formatEtTime(ms);
const GAP = 6;

function changeText(pct: number): string {
  return `${pct >= 0 ? "+" : ""}${pct.toFixed(2)}%`;
}

/**
 * The phone dashboard's Halt watcher (the web's HaltWatcher; design canvas "Halt Watcher";
 * build-ups since 2026-10-20): only the symbols whose one-sided pressure is rising fast while the
 * price runs at its LULD band (crates/server/src/pressure.rs), at most 6 and usually none, so the
 * row is usually hidden: no cards, no line. The cards are ONE row of mini gauges across the screen (owner,
 * 2026-10-19: "fit them all on a single row"), above the charts. Each cell: the dial filled with
 * the halt risk, the ticker and the short cue ("+45 · 1.2%↑"); on a wide cell (the unfolded
 * Fold) the price, the change and the full cue ("+45 in 30s · 1.2% to LULD up") too. Tap a cell
 * to open its ticker's chart (the chart modal); tap, then hold copies it.
 */
export function HaltWatcher({ channel, onPick, reducedMotion, gap = 0 }: { channel: PressureChannel; onPick: (symbol: string) => void; reducedMotion: boolean; gap?: number }) {
  const board = usePressureBoard(channel);
  const status = usePressureStatus(channel);
  const freshness = pressureFreshnessLine(status, hhmm);
  const [width, setWidth] = useState(0);
  // Six equal cells, whatever the board holds, so a short board does not stretch two dials wide.
  const cellWidth = width > 0 ? (width - GAP * (SLOTS - 1)) / SLOTS : 0;
  // No build-ups, no row (owner, 2026-10-22: a panel with 0 items is hidden): it opens and closes
  // by its real height, so the cards below slide rather than jump.
  return (
    <Collapse open={board.length > 0} reducedMotion={reducedMotion} gap={gap}>
      <View onLayout={(e) => setWidth(e.nativeEvent.layout.width)} accessibilityLabel={`Halt watcher, ${freshness || "pressure building toward a LULD band"}`}>
        {!!freshness && <Text accessibilityLiveRegion="polite" style={styles.freshness}>{freshness}</Text>}
        <View style={styles.row}>{cellWidth > 0 && board.slice(0, SLOTS).map((c) => <MiniCard key={c.symbol} card={c} width={cellWidth} onPick={onPick} reducedMotion={reducedMotion} stale={!status.connected} freshness={freshness} />)}</View>
      </View>
    </Collapse>
  );
}

const SLOTS = 6;

const MiniCard = memo(function MiniCard({ card, width, onPick, reducedMotion, stale, freshness }: { card: PressureCard; width: number; onPick: (symbol: string) => void; reducedMotion: boolean; stale: boolean; freshness: string }) {
  const press = useTickerPress(card.symbol, () => onPick(card.symbol));
  const halted = card.halt !== undefined;
  const phrase = riskPhrase(card);
  const status = card.halt ? haltSentence(card.halt, hhmm) : `${changeText(card.changePct)} today`;
  const tone = stale ? UI.textFaint : halted ? UI.warning : card.side === "bear" ? UI.critical : UI.good;
  // Wide cells (the unfolded Fold) have room for the price, the change and the full cue.
  const roomy = width >= 110;
  return (
    <Pressable
      accessibilityRole="button"
      accessibilityLabel={`${stale ? `${freshness}. Last known: ` : ""}${card.symbol} ${formatPrice(card.price)}, ${status}, ${phrase}, ${buildupSentence(card, formatPrice)}`}
      accessibilityHint="Opens its chart. Tap, then press and hold to copy"
      {...press}
      style={({ pressed }) => [styles.cell, { width }, pressed && { backgroundColor: UI.row }]}
    >
      {/* A freshness transition resets any interrupted sweep to the authoritative reading. */}
      <PressureGauge key={stale ? "stale" : "live"} value={card.risk} side={card.side} halted={halted} stale={stale} label={`${stale ? `${freshness}. Last known: ` : halted ? "Halted. " : ""}${phrase}`} width={Math.min(width - 8, 120)} reducedMotion={reducedMotion} compact={!roomy} />
      <Text style={styles.symbol} numberOfLines={1} adjustsFontSizeToFit minimumFontScale={0.7}>
        {card.symbol}
      </Text>
      {roomy && <Text style={styles.price}>{formatPrice(card.price)}</Text>}
      {roomy && (
        <Text style={[styles.change, { color: stale ? UI.textFaint : halted ? UI.warning : card.changePct < 0 ? UI.critical : UI.good }]} numberOfLines={1} adjustsFontSizeToFit minimumFontScale={0.7}>
          {card.halt ? haltLine(card.halt, hhmm) : changeText(card.changePct)}
        </Text>
      )}
      <Text style={[styles.cue, { color: stale ? UI.textFaint : roomy ? UI.textDim : tone }]} numberOfLines={1} adjustsFontSizeToFit minimumFontScale={0.7}>
        {stale ? "Last known" : roomy ? buildupCue(card) : card.halt ? "Halted" : buildupCueShort(card)}
      </Text>
    </Pressable>
  );
});

const styles = StyleSheet.create({
  freshness: { color: UI.textFaint, fontSize: 11, fontFamily: fonts.regular, marginBottom: 4 },
  row: { flexDirection: "row", gap: GAP },
  cell: { alignItems: "center", gap: 1, backgroundColor: UI.panel, borderRadius: 10, paddingTop: 6, paddingBottom: 5, paddingHorizontal: 2 },
  symbol: { color: UI.text, fontSize: 11.5, fontFamily: fonts.semibold, maxWidth: "100%" },
  price: { color: UI.text, fontSize: 13, fontVariant: ["tabular-nums"], fontFamily: fonts.regular },
  change: { fontSize: 10, fontVariant: ["tabular-nums"], maxWidth: "100%" },
  cue: { fontSize: 10, fontVariant: ["tabular-nums"], maxWidth: "100%" },
});

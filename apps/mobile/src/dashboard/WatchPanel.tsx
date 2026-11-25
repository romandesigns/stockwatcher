import { Icon, UI } from "@stockwatcher/chart-native";
import { WATCH_BASE_RATE, WATCH_NO_FLOAT, watchCardView, type PrewatchChannel, type WatchCardWire } from "@stockwatcher/shared";
import { memo, useCallback, useState, useSyncExternalStore } from "react";
import { Pressable, StyleSheet, Text, View } from "react-native";

import { Collapse } from "../motion/Collapse";
import { fonts } from "../theme";
import { useTickerPress } from "../ui/useTickerPress";

/** From this card-list width two cards sit side by side (the Fold unfolded, tablets). */
const TWO_UP = 600;
const CARD_GAP = 6;

function usePrewatchState(channel: PrewatchChannel) {
  const subscribe = useCallback((cb: () => void) => channel.subscribe(cb), [channel]);
  return useSyncExternalStore(subscribe, useCallback(() => channel.get(), [channel]));
}

/**
 * The pre-open Watch card on the phone (the web's apps/web/src/dashboard/WatchPanel.tsx; server
 * crates/server/src/prewatch): 04:00–09:30 ET, stocks matching the AKAN profile, each with why it
 * matched. Quiet: no banner, sound or haptic, and no accent; a condition that stopped holding dims
 * the card and names it. Hidden while it has no card. One card a row on a phone and the Fold's
 * cover screen, two unfolded; tapping a card opens its chart.
 */
export function WatchPanel({ channel, onPick, reducedMotion, gap = 0 }: { channel: PrewatchChannel; onPick: (symbol: string) => void; reducedMotion: boolean; gap?: number }) {
  const state = usePrewatchState(channel);
  const cards = state.enabled ? state.cards : [];
  const [width, setWidth] = useState(0);
  const two = width >= TWO_UP && cards.length > 1;
  const cardWidth = two ? (width - CARD_GAP) / 2 : undefined;
  const reconnecting = !state.connected && cards.length > 0;
  const live = cards.filter((c) => c.fails.length === 0).length;
  return (
    <Collapse open={cards.length > 0} reducedMotion={reducedMotion} gap={gap}>
      <View style={styles.card} accessibilityLabel="Pre-open watch">
        <View style={styles.head}>
          <Icon name="gauge" color={UI.textDim} size={14} />
          <Text accessibilityRole="header" style={styles.title}>
            Pre-open watch
          </Text>
          <Text style={styles.caption} numberOfLines={1}>
            AKAN-like · quiet
          </Text>
          <Text style={styles.count}>{live}</Text>
        </View>
        {reconnecting && (
          <Text accessibilityRole="alert" style={styles.stale}>
            Reconnecting · last known state
          </Text>
        )}
        {state.floatSource === "none" && <Text style={styles.foot}>{WATCH_NO_FLOAT}</Text>}
        <View style={[styles.list, reconnecting && styles.dimmed]} onLayout={(e) => setWidth(e.nativeEvent.layout.width)}>
          {cards.map((c, i) => (
            // An odd last card on two columns takes the whole row (no orphan beside a gap).
            <View key={c.symbol} style={{ width: two && !(i === cards.length - 1 && cards.length % 2 === 1) ? cardWidth : "100%" }}>
              <WatchCard card={c} onPick={onPick} />
            </View>
          ))}
        </View>
        <Text style={styles.foot}>{WATCH_BASE_RATE}</Text>
      </View>
    </Collapse>
  );
}

const WatchCard = memo(function WatchCard({ card, onPick }: { card: WatchCardWire; onPick: (s: string) => void }) {
  const v = watchCardView(card);
  const press = useTickerPress(card.symbol, () => onPick(card.symbol));
  return (
    <Pressable
      accessibilityRole="button"
      accessibilityLabel={`${v.symbol} ${v.price}, ${v.reasons.map((r) => `${r.label} ${r.value}`).join(", ")}${v.faded ? `. ${v.faded}` : ""}`}
      accessibilityHint="Opens its chart; tap, then press and hold to copy"
      {...press}
      style={({ pressed }) => [styles.item, { opacity: v.faded ? 0.6 : 1 }, pressed && { backgroundColor: UI.raised }]}
    >
      <View style={styles.line}>
        <Text style={styles.symbol}>{v.symbol}</Text>
        <Text style={styles.price}>{v.price}</Text>
        <Text style={styles.gap}>{v.gap}</Text>
        <Text style={styles.since} numberOfLines={1}>
          {v.since}
        </Text>
      </View>
      {v.reasons.map((r) => (
        <View key={r.label} style={styles.reason}>
          <Text style={styles.label}>{r.label}</Text>
          <Text style={styles.value} numberOfLines={1}>
            {r.value}
          </Text>
        </View>
      ))}
      {v.faded && <Text style={styles.faded}>{v.faded}</Text>}
    </Pressable>
  );
});

const styles = StyleSheet.create({
  card: { borderRadius: 14, backgroundColor: UI.panel, paddingVertical: 10, paddingHorizontal: 8, gap: 6 },
  head: { flexDirection: "row", alignItems: "center", gap: 8, paddingHorizontal: 4, minHeight: 32 },
  title: { color: UI.text, fontSize: 14, fontFamily: fonts.semibold },
  caption: { flexShrink: 1, color: UI.textFaint, fontSize: 11, fontFamily: fonts.regular },
  count: { marginLeft: "auto", color: UI.textFaint, fontSize: 11, fontVariant: ["tabular-nums"] },
  stale: { color: UI.warning, fontSize: 11, paddingHorizontal: 4 },
  dimmed: { opacity: 0.6 },
  list: { flexDirection: "row", flexWrap: "wrap", gap: CARD_GAP },
  item: { borderRadius: 10, backgroundColor: UI.row, paddingVertical: 8, paddingHorizontal: 10, gap: 3, minHeight: 44 },
  line: { flexDirection: "row", alignItems: "baseline", gap: 6 },
  symbol: { color: UI.text, fontSize: 14, fontFamily: fonts.semibold },
  price: { color: UI.text, fontSize: 13, fontVariant: ["tabular-nums"] },
  gap: { color: UI.textDim, fontSize: 12, fontVariant: ["tabular-nums"] },
  since: { flexShrink: 1, marginLeft: "auto", color: UI.textFaint, fontSize: 10, fontFamily: fonts.regular },
  reason: { flexDirection: "row", gap: 8 },
  label: { width: 96, color: UI.textFaint, fontSize: 11, fontFamily: fonts.regular },
  value: { flex: 1, color: UI.textDim, fontSize: 11, fontVariant: ["tabular-nums"] },
  faded: { color: UI.textDim, fontSize: 11, fontFamily: fonts.regular },
  foot: { color: UI.textFaint, fontSize: 11, lineHeight: 15, fontFamily: fonts.regular, paddingHorizontal: 4 },
});

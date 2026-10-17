import { describeCounted, formatEtTime, formatPrice, formatSize, type CountedTone, type TapeTrade } from "@stockwatcher/chart-core";
import { ScrollView, StyleSheet, Text, View } from "react-native";
import { Sheet } from "./sheets.tsx";
import { UI } from "./ui.tsx";

// Row text: counted prints read normally, volume-only ones dimmed,
// excluded or cancelled ones faint; the note carries the status colour (as chart-web's Tape).
const ROW: Record<CountedTone, string> = { counted: UI.text, partial: UI.textDim, excluded: UI.textFaint, ignored: UI.textFaint };
const NOTE: Record<CountedTone, string> = { counted: UI.textDim, partial: UI.warning, excluded: UI.critical, ignored: UI.critical };

/**
 * Time and sales with how the engine counted each print, newest first: the phone's twin of chart-web's Tape.
 * Only exceptions carry a note (odd lot, late, excluded...): a print counted in full is the normal case, and
 * labelling every one of them buried the few that matter.
 */
export function Tape({ trades, max = 60 }: { trades: readonly TapeTrade[]; max?: number }) {
  const rows = trades.slice(0, max);
  if (!rows.length) return <Text style={s.empty}>No trades yet.</Text>;
  return (
    <View accessibilityLabel="Time and sales">
      <View style={s.head}>
        <Text style={[s.headCell, { width: 66 }]}>Time</Text>
        <Text style={[s.headCell, { width: 52 }]}>Price</Text>
        <Text style={[s.headCell, s.size]}>Size</Text>
        <Text style={[s.headCell, s.note]}>Note</Text>
      </View>
      {rows.map((tr, i) => {
        const d = describeCounted(tr);
        return (
          <View key={`${tr.exchange}-${tr.seq}-${tr.t}-${i}`} style={s.row} accessibilityLabel={`${formatEtTime(tr.t, true)}, ${formatPrice(tr.p)}, ${formatSize(tr.s)} shares, ${d.short}`}>
            <Text style={[s.cell, { width: 66, color: d.tone === "counted" ? UI.textDim : ROW[d.tone] }, d.tone === "ignored" && s.struck]}>{formatEtTime(tr.t, true)}</Text>
            <Text style={[s.cell, { width: 52, color: ROW[d.tone] }]}>{formatPrice(tr.p)}</Text>
            <Text style={[s.cell, s.size, { color: ROW[d.tone] }]}>{formatSize(tr.s)}</Text>
            <Text style={[s.note, { color: NOTE[d.tone] }]} numberOfLines={1}>
              {d.tone === "counted" ? "" : d.short}
            </Text>
          </View>
        );
      })}
    </View>
  );
}

/** The tape in a bottom sheet, so the chart keeps its room on a phone. */
export function TapeSheet({ open, onClose, trades, note, reducedMotion }: { open: boolean; onClose: () => void; trades: readonly TapeTrade[]; note?: string; reducedMotion?: boolean }) {
  return (
    <Sheet open={open} onClose={onClose} title="Time & sales" badge={note ? <Text style={s.symbolChip}>{note}</Text> : undefined} reducedMotion={reducedMotion}>
      <Text style={s.caption}>A blank note means the print counted toward price and volume.</Text>
      <ScrollView>
        <Tape trades={trades} />
      </ScrollView>
    </Sheet>
  );
}

const s = StyleSheet.create({
  head: { flexDirection: "row", alignItems: "center", gap: 6, paddingHorizontal: 4, paddingBottom: 6 },
  headCell: { color: UI.textFaint, fontSize: 10, fontWeight: "500" },
  row: { flexDirection: "row", alignItems: "center", gap: 6, minHeight: 30, paddingHorizontal: 4 },
  cell: { fontSize: 13, fontVariant: ["tabular-nums"] },
  size: { width: 56, textAlign: "right" },
  note: { flex: 1, textAlign: "right", fontSize: 11 },
  caption: { color: UI.textFaint, fontSize: 11, marginTop: -4, marginBottom: 10 },
  symbolChip: { color: UI.text, fontSize: 12, fontWeight: "600", backgroundColor: UI.bg, borderRadius: 6, paddingHorizontal: 8, paddingVertical: 3, overflow: "hidden" },
  struck: { textDecorationLine: "line-through" },
  empty: { color: UI.textFaint, fontSize: 12, paddingVertical: 8 },
});

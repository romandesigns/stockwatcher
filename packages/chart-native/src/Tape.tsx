import { describeCounted, formatEtTime, formatPrice, formatSize, type CountedTone, type TapeTrade } from "@stockwatcher/chart-core";
import { ScrollView, StyleSheet, Text, View } from "react-native";
import { Sheet } from "./sheets.tsx";
import { UI } from "./ui.tsx";

// Row text: counted prints read normally, volume-only ones dimmed,
// excluded or cancelled ones faint; the note carries the status colour (as chart-web's Tape).
const ROW: Record<CountedTone, string> = { counted: UI.text, partial: UI.textDim, excluded: UI.textFaint, ignored: UI.textFaint };
const NOTE: Record<CountedTone, string> = { counted: UI.textDim, partial: UI.warning, excluded: UI.critical, ignored: UI.critical };

/** Time and sales with how the engine counted each print, newest first: the phone's twin of chart-web's Tape. */
export function Tape({ trades, max = 60 }: { trades: readonly TapeTrade[]; max?: number }) {
  const rows = trades.slice(0, max);
  if (!rows.length) return <Text style={s.empty}>No trades yet.</Text>;
  return (
    <View accessibilityLabel="Time and sales">
      {rows.map((tr, i) => {
        const d = describeCounted(tr);
        return (
          <View key={`${tr.exchange}-${tr.seq}-${tr.t}-${i}`} style={[s.row, i % 2 === 0 && { backgroundColor: UI.row }]}>
            <Text style={[s.cell, { width: 64, color: ROW[d.tone] }, d.tone === "ignored" && s.struck]}>{formatEtTime(tr.t, true)}</Text>
            <Text style={[s.cell, { width: 48, color: ROW[d.tone] }]}>{formatPrice(tr.p)}</Text>
            <Text style={[s.cell, { width: 52, color: ROW[d.tone] }]}>{formatSize(tr.s)}</Text>
            <Text style={[s.note, { color: NOTE[d.tone] }]} numberOfLines={1}>
              {d.short}
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
    <Sheet open={open} onClose={onClose} title={note ? `Time & sales · ${note}` : "Time & sales"} reducedMotion={reducedMotion}>
      <ScrollView>
        <Tape trades={trades} />
      </ScrollView>
    </Sheet>
  );
}

const s = StyleSheet.create({
  row: { flexDirection: "row", alignItems: "center", gap: 6, minHeight: 28, paddingHorizontal: 8, borderRadius: 6 },
  cell: { fontSize: 12, fontVariant: ["tabular-nums"] },
  note: { flex: 1, textAlign: "right", fontSize: 11 },
  struck: { textDecorationLine: "line-through" },
  empty: { color: UI.textFaint, fontSize: 12, paddingVertical: 8 },
});

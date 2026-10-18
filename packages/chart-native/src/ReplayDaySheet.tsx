import { formatSymbolCount, type ReplayDayOption } from "@stockwatcher/chart-core";
import { Pressable, ScrollView, StyleSheet, Text, View } from "react-native";
import { Sheet } from "./sheets.tsx";
import { UI } from "./ui.tsx";

/**
 * The replay day picker, the phone's twin of chart-web's ReplayDayMenu: the
 * archive's days, most recent first, in the Indicators sheet's row design
 * (the current day on the row colour with the accent check, the only accent
 * here). Fixture days carry the warning-tone tag so synthetic data never
 * passes for a real session. Picking a day closes the sheet; the server then
 * rebuilds the replay on it and resends every snapshot, so there is nothing
 * to confirm here. Picking the current day just closes it.
 */
export function ReplayDaySheet({
  open,
  onClose,
  days,
  day,
  onSelect,
  reducedMotion,
}: {
  open: boolean;
  onClose: () => void;
  days: readonly ReplayDayOption[];
  /** The day being replayed, "YYYY-MM-DD". */
  day: string | null;
  onSelect: (day: string) => void;
  reducedMotion?: boolean;
}) {
  return (
    <Sheet open={open} onClose={onClose} title="Replay day" reducedMotion={reducedMotion}>
      <Text style={s.caption}>Switching days restarts the replay and resets the simulated account.</Text>
      <ScrollView>
        {days.length === 0 ? (
          <Text style={s.empty}>No archived days.</Text>
        ) : (
          days.map((d) => {
            const current = d.value === day;
            return (
              <Pressable
                key={d.value}
                accessibilityRole="radio"
                accessibilityState={{ checked: current }}
                accessibilityLabel={`${d.longLabel}${d.fixture ? ", fixture data" : ""}, ${formatSymbolCount(d.symbols)}`}
                onPress={() => {
                  onClose();
                  if (!current) onSelect(d.value);
                }}
                style={({ pressed }) => [s.row, current && { backgroundColor: UI.row }, pressed && { backgroundColor: UI.raised }]}
              >
                <Text style={s.label}>{d.label}</Text>
                {d.fixture && <Text style={s.fixture}>FIXTURE</Text>}
                <View style={s.right}>
                  <Text style={s.count}>{d.symbols.toLocaleString("en-US")}</Text>
                  {/* Fixed width, so the counts line up whether or not the row has the check. */}
                  <Text style={s.check}>{current ? "✓" : ""}</Text>
                </View>
              </Pressable>
            );
          })
        )}
      </ScrollView>
    </Sheet>
  );
}

const s = StyleSheet.create({
  caption: { color: UI.textFaint, fontSize: 11, marginTop: -4, marginBottom: 10 },
  row: { minHeight: 48, flexDirection: "row", alignItems: "center", gap: 8, paddingHorizontal: 10, borderRadius: 10 },
  label: { color: UI.text, fontSize: 14, fontVariant: ["tabular-nums"] },
  fixture: { color: UI.warning, fontSize: 9, fontWeight: "600", letterSpacing: 0.6, backgroundColor: "rgba(224,166,58,0.14)", borderRadius: 4, paddingHorizontal: 6, paddingVertical: 2, overflow: "hidden" },
  right: { marginLeft: "auto", flexDirection: "row", alignItems: "center", gap: 6 },
  count: { color: UI.textFaint, fontSize: 11, fontVariant: ["tabular-nums"] },
  check: { width: 16, textAlign: "right", color: UI.accent, fontSize: 13 },
  empty: { color: UI.textFaint, fontSize: 12, paddingVertical: 8 },
});

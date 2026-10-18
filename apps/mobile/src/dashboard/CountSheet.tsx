import { MAX_DASHBOARD_CHARTS } from "@stockwatcher/app-state";
import { Icon, playHaptic, Sheet, UI } from "@stockwatcher/chart-native";
import { Pressable, StyleSheet, Text, View } from "react-native";

const COUNTS = Array.from({ length: MAX_DASHBOARD_CHARTS }, (_, i) => i + 1);

/**
 * How many charts the dashboard panel shows, opened by the "screen × n" button: the web's
 * MultiViewMenu as a bottom sheet. The current count is marked by the accent check alone, as in
 * the other sheets (no filled row).
 */
export function CountSheet({ open, onClose, count, onChange, reducedMotion }: { open: boolean; onClose: () => void; count: number; onChange: (n: number) => void; reducedMotion?: boolean }) {
  return (
    <Sheet open={open} onClose={onClose} title="Charts in the panel" reducedMotion={reducedMotion}>
      {COUNTS.map((n) => {
        const on = n === count;
        return (
          <Pressable
            key={n}
            accessibilityRole="radio"
            accessibilityState={{ checked: on }}
            accessibilityLabel={`${n} ${n === 1 ? "chart" : "charts"}`}
            onPress={() => {
              playHaptic("selection");
              onChange(n);
              onClose();
            }}
            style={({ pressed }) => [styles.row, pressed && { backgroundColor: UI.row }]}
          >
            <Text style={styles.label}>
              {n} {n === 1 ? "chart" : "charts"}
            </Text>
            <View style={{ opacity: on ? 1 : 0 }}>
              <Icon name="check" color={UI.accent} size={14} />
            </View>
          </Pressable>
        );
      })}
    </Sheet>
  );
}

const styles = StyleSheet.create({
  row: { minHeight: 48, flexDirection: "row", alignItems: "center", justifyContent: "space-between", paddingHorizontal: 8, borderRadius: 10 },
  label: { color: UI.text, fontSize: 15 },
});

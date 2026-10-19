import type { DashboardSource } from "@stockwatcher/app-state";
import type { IconName } from "@stockwatcher/chart-core";
import { Icon, playHaptic, UI } from "@stockwatcher/chart-native";
import { Pressable, StyleSheet, Text, View } from "react-native";

const OPTIONS: readonly { value: DashboardSource; icon: IconName; name: string; description: string }[] = [
  {
    value: "live",
    icon: "live",
    name: "Live market",
    description: "Real trades from Alpaca as they happen: SIP from 4:00 to 20:00 ET, Blue Ocean overnight. Empty when nothing is trading.",
  },
  {
    value: "replay",
    icon: "replay",
    name: "Replay",
    description: "A recorded day from the archive, on the replay clock the Chart tab controls (the practice feature).",
  },
];

/**
 * Settings → Data's choice list (the web's DataSourcePanel): the market the dashboard reads. The
 * chosen row spends the accent on its icon and check only, as in the other sheets.
 */
export function DataSourcePanel({ source, onChange }: { source: DashboardSource; onChange: (s: DashboardSource) => void }) {
  return (
    <View accessibilityRole="radiogroup" style={s.list}>
      {OPTIONS.map((o) => {
        const on = o.value === source;
        return (
          <Pressable
            key={o.value}
            accessibilityRole="radio"
            accessibilityState={{ checked: on }}
            accessibilityLabel={`${o.name}. ${o.description}`}
            onPress={() => {
              playHaptic("selection");
              onChange(o.value);
            }}
            style={({ pressed }) => [s.row, (pressed || on) && { backgroundColor: UI.row }]}
          >
            <Icon name={o.icon} color={on ? UI.accent : UI.textDim} size={18} />
            <View style={s.text}>
              <Text style={s.name}>{o.name}</Text>
              <Text style={s.description}>{o.description}</Text>
            </View>
            <View style={{ opacity: on ? 1 : 0 }}>
              <Icon name="check" color={UI.accent} size={14} />
            </View>
          </Pressable>
        );
      })}
    </View>
  );
}

const s = StyleSheet.create({
  list: { gap: 4 },
  row: { minHeight: 56, flexDirection: "row", alignItems: "flex-start", gap: 12, paddingHorizontal: 12, paddingVertical: 10, borderRadius: 10 },
  text: { flex: 1, gap: 2 },
  name: { color: UI.text, fontSize: 15, fontWeight: "500" },
  description: { color: UI.textFaint, fontSize: 12, lineHeight: 16 },
});

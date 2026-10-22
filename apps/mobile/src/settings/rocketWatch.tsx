import type { SettingsSheetSection } from "@stockwatcher/chart-native";
import { Icon, playHaptic, UI } from "@stockwatcher/chart-native";
import { useState } from "react";
import { Pressable, StyleSheet, Text, View } from "react-native";

import { setRocketNotify, useRocketNotify } from "../notifications/rocket";
import { fonts } from "../theme";

/** Settings → Rocket watch: the optional local notification for new flags (off by default). */
export function rocketSection(): SettingsSheetSection {
  return {
    id: "rocket",
    label: "Runners",
    icon: "bell",
    description: "Experimental flags of a likely +30 % move.",
    render: () => <RocketWatchPanel />,
  };
}

function RocketWatchPanel() {
  const on = useRocketNotify();
  const [busy, setBusy] = useState(false);
  const [refused, setRefused] = useState(false);
  return (
    <View style={s.list}>
      <Pressable
        accessibilityRole="switch"
        accessibilityState={{ checked: on, busy }}
        accessibilityLabel="Notify me about new flags. A banner with sound for each new Runners flag while the dashboard is open."
        disabled={busy}
        onPress={() => {
          playHaptic("selection");
          setBusy(true);
          void setRocketNotify(!on)
            .then((now) => setRefused(!on && !now))
            .finally(() => setBusy(false));
        }}
        style={({ pressed }) => [s.row, pressed && { backgroundColor: UI.row }]}
      >
        <Icon name="bell" color={on ? UI.accent : UI.textDim} size={18} />
        <View style={s.text}>
          <Text style={s.name}>Notify me about new flags</Text>
          <Text style={s.description}>A banner with sound for each new flag while the dashboard is open. This phone only. Experimental: most flags do not rocket.</Text>
          {refused && <Text style={[s.description, { color: UI.warning }]}>Notifications are not allowed for this app (system settings).</Text>}
        </View>
        <View style={[s.track, on && { backgroundColor: UI.accent }]}>
          <View style={[s.knob, on && { transform: [{ translateX: 14 }] }]} />
        </View>
      </Pressable>
    </View>
  );
}

const s = StyleSheet.create({
  list: { gap: 4 },
  row: { minHeight: 56, flexDirection: "row", alignItems: "flex-start", gap: 12, paddingHorizontal: 12, paddingVertical: 10, borderRadius: 10 },
  text: { flex: 1, gap: 2 },
  name: { color: UI.text, fontSize: 14, fontFamily: fonts.semibold },
  description: { color: UI.textDim, fontSize: 12, fontFamily: fonts.regular },
  track: { width: 34, height: 20, borderRadius: 10, backgroundColor: UI.raised, padding: 3, alignSelf: "center" },
  knob: { width: 14, height: 14, borderRadius: 7, backgroundColor: UI.text },
});

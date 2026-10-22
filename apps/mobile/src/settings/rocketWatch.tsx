import type { SettingsSheetSection } from "@stockwatcher/chart-native";
import { Icon, playHaptic, UI } from "@stockwatcher/chart-native";
import { useState, useSyncExternalStore } from "react";
import { Pressable, StyleSheet, Text, View } from "react-native";

import { enablePush, pushToken, subscribePushToken } from "../notifications/push";
import { setRunnersPush, useRunnersPush, type RunnersPushPrefs } from "../notifications/runnersPush";
import { fonts } from "../theme";

/**
 * Settings → Runners: this phone's Runners pushes (owner, 2026-10-22), sent by the server even
 * with the app closed (crates/server/src/alerts/runners.rs). Two switches, per device: new flags
 * (on by default) and breakouts (off). Turning one on when this phone has no push token asks for
 * notification permission and registers it (the Alerts tab's "Turn on phone alerts" does the
 * same). Without push, new flags ring a local banner while the dashboard is open instead.
 */
export function rocketSection(): SettingsSheetSection {
  return {
    id: "rocket",
    label: "Runners",
    icon: "bell",
    description: "Experimental flags of a likely +30 % move.",
    render: () => <RunnersPushPanel />,
  };
}

function RunnersPushPanel() {
  const prefs = useRunnersPush();
  const token = useSyncExternalStore(subscribePushToken, pushToken);
  const [busy, setBusy] = useState(false);
  const [note, setNote] = useState<string | null>(null);
  const toggle = (key: keyof RunnersPushPrefs) => {
    playHaptic("selection");
    const want = !prefs[key];
    void setRunnersPush({ [key]: want });
    if (!want || token) return;
    setBusy(true);
    void enablePush()
      .then((r) => setNote(r === "granted" ? null : r === "denied" ? "Notifications are off for StockWatcher in the phone's settings." : r))
      .finally(() => setBusy(false));
  };
  return (
    <View style={s.list}>
      <Switch
        on={prefs.pushNew}
        busy={busy}
        icon="bell"
        name="New flags"
        description="Ticker, price and the top reasons as soon as Runners flags it."
        onPress={() => toggle("pushNew")}
      />
      <Switch
        on={prefs.pushBreakout}
        busy={busy}
        icon="gauge"
        name="Breakouts"
        description="When a flagged stock turns breaking out, with its % from the flag."
        onPress={() => toggle("pushBreakout")}
      />
      <Text style={[s.status, note ? { color: UI.warning } : null]}>
        {note ??
          (token
            ? "Pushed to this phone, also when the app is closed. At most one per stock per 10 min, 30 a day; the alerts' quiet hours apply. Tap one to open its chart."
            : "Phone push is off on this phone: new flags ring only while the dashboard is open. Turning a switch on asks to allow notifications.")}
      </Text>
    </View>
  );
}

function Switch({ on, busy, icon, name, description, onPress }: { on: boolean; busy: boolean; icon: "bell" | "gauge"; name: string; description: string; onPress: () => void }) {
  return (
    <Pressable
      accessibilityRole="switch"
      accessibilityState={{ checked: on, busy }}
      accessibilityLabel={`${name}. ${description}`}
      disabled={busy}
      onPress={onPress}
      style={({ pressed }) => [s.row, pressed && { backgroundColor: UI.row }]}
    >
      <Icon name={icon} color={on ? UI.accent : UI.textDim} size={18} />
      <View style={s.text}>
        <Text style={s.name}>{name}</Text>
        <Text style={s.description}>{description}</Text>
      </View>
      <View style={[s.track, on && { backgroundColor: UI.accent }]}>
        <View style={[s.knob, on && { transform: [{ translateX: 14 }] }]} />
      </View>
    </Pressable>
  );
}

const s = StyleSheet.create({
  list: { gap: 2 },
  row: { minHeight: 48, flexDirection: "row", alignItems: "center", gap: 12, paddingHorizontal: 12, paddingVertical: 8, borderRadius: 10 },
  text: { flex: 1, gap: 2 },
  name: { color: UI.text, fontSize: 14, fontFamily: fonts.semibold },
  description: { color: UI.textDim, fontSize: 12, fontFamily: fonts.regular },
  status: { color: UI.textFaint, fontSize: 12, fontFamily: fonts.regular, paddingHorizontal: 12, paddingTop: 4 },
  track: { width: 34, height: 20, borderRadius: 10, backgroundColor: UI.raised, padding: 3 },
  knob: { width: 14, height: 14, borderRadius: 7, backgroundColor: UI.text },
});

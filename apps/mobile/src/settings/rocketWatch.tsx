import type { SettingsSheetSection } from "@stockwatcher/chart-native";
import { Icon, playHaptic, UI } from "@stockwatcher/chart-native";
import { useState, useSyncExternalStore } from "react";
import { Pressable, StyleSheet, Text, View } from "react-native";

import { enablePush, pushToken, subscribePushToken } from "../notifications/push";
import { setRunnersPush, useRunnersPush, type RunnersPushPrefs } from "../notifications/runnersPush";
import { fonts } from "../theme";

/**
 * Settings → Runners: this phone's Runners pushes (owner, 2026-10-22), sent by the server even
 * with the app closed (crates/server/src/alerts/runners.rs). Six switches, per device: new flags
 * (on by default), breakouts (off), buying bursts (on, 2026-10-22), running entries, "Run +20%"
 * and "Fast move" entries (on, 2026-10-22: the owner does not want to miss a move while away
 * from the phone). Turning one on when this phone has no push token asks for
 * notification permission and registers it (the Alerts tab's "Turn on phone alerts" does the
 * same). Without push, new flags ring a local banner while the dashboard is open instead.
 */
export function rocketSection(): SettingsSheetSection {
  return {
    id: "rocket",
    label: "Runners",
    icon: "bell",
    description: "Stocks that start running, from five detectors: flags, bursts, running, Run +20% and fast moves.",
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
      <Switch
        on={prefs.pushBurst}
        busy={busy}
        icon="trend"
        name="Buying bursts"
        description="When heavy buying is confirmed on the tape, with how far the burst already went."
        onPress={() => toggle("pushBurst")}
      />
      <Switch
        on={prefs.pushRunning}
        busy={busy}
        icon="gauge"
        name="Running entries"
        description="Every stock that starts running by its own range, with its move and money."
        onPress={() => toggle("pushRunning")}
      />
      <Switch
        on={prefs.pushRun}
        busy={busy}
        icon="trend"
        name="Run +20%"
        description="A +20% leg within 15 minutes on real trades, with its window and start price."
        onPress={() => toggle("pushRun")}
      />
      <Switch
        on={prefs.pushFast}
        busy={busy}
        icon="live"
        name="Fast move"
        description="+8% in a minute or +12% in 15 s on real trades, with the move and its money."
        onPress={() => toggle("pushFast")}
      />
      <Text style={[s.status, note ? { color: UI.warning } : null]}>
        {note ??
          (token
            ? "Pushed to this phone, also when the app is closed. At most one per stock per 10 min (a burst: 2 min); past 120 a day only bursts and new flags. The alerts' quiet hours apply. Tap one to open its chart."
            : "Phone push is off on this phone: new flags and bursts ring only while the dashboard is open. Turning a switch on asks to allow notifications.")}
      </Text>
    </View>
  );
}

function Switch({ on, busy, icon, name, description, onPress }: { on: boolean; busy: boolean; icon: "bell" | "gauge" | "trend" | "live"; name: string; description: string; onPress: () => void }) {
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

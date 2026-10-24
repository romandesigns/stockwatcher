import type { SettingsSheetSection } from "@stockwatcher/chart-native";
import { Icon, playHaptic, UI } from "@stockwatcher/chart-native";
import { ALERT_SWITCH_SECTIONS, type AlertSwitch } from "@stockwatcher/shared";
import { useEffect, useState, useSyncExternalStore } from "react";
import { AccessibilityInfo, Animated, Easing, Pressable, StyleSheet, Text, View } from "react-native";

import { enablePush, pushToken, subscribePushToken } from "../notifications/push";
import { PUSH_PREF_OF, setRunnersPush, useRunnersPush, type RunnersPushPrefs } from "../notifications/runnersPush";
import { fonts } from "../theme";

/**
 * Settings → Alerts: this phone's pushes, sent by the server even with the app closed
 * (crates/server/src/alerts/runners.rs), by panel (owner, 2026-10-23: every panel event backed by
 * evidence notifies). Runners: new flags, breakouts (off), bursts, running, Run +20%, fast moves,
 * follow-ups and volume-shelf entries. Ross and Signals: crossing (on) and at trigger (off). Halt
 * watcher: the gauge appearing (off). Per device; the words are shared with the web and the
 * desktop (packages/shared `ALERT_SWITCH_SECTIONS`). Turning one on when this phone has no push
 * token asks for notification permission and registers it (the Alerts tab's "Turn on phone
 * alerts" does the same). Without push, new flags ring a local banner while the dashboard is open.
 */
export function rocketSection(): SettingsSheetSection {
  return {
    // The id stays "rocket" so a remembered open section still finds it.
    id: "rocket",
    label: "Alerts",
    icon: "bell",
    description: "Pushes for Runners, Ross, Signals and the Halt watcher, by panel.",
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
      {ALERT_SWITCH_SECTIONS.map((section) => (
        <View key={section.id} style={s.section} accessibilityRole="summary" accessibilityLabel={`${section.label} alerts`}>
          <Text style={s.heading} accessibilityRole="header">
            {section.label}
          </Text>
          {section.items.map((item) => {
            const key = PUSH_PREF_OF[item.key];
            return <Switch key={item.key} item={item} on={prefs[key]} busy={busy} onPress={() => toggle(key)} />;
          })}
        </View>
      ))}
      <Text style={[s.status, note ? { color: UI.warning } : null]}>
        {note ??
          (token
            ? "Pushed to this phone, also when the app is closed. The same kind at most once per stock per 10 min; new evidence (a burst, crossing, shelf entry or follow-up) 2 min after any other. No daily limit. The alerts' quiet hours apply. Tap one to open its chart."
            : "Phone push is off on this phone: new flags and bursts ring only while the dashboard is open. Turning a switch on asks to allow notifications.")}
      </Text>
    </View>
  );
}

function Switch({ item, on, busy, onPress }: { item: AlertSwitch; on: boolean; busy: boolean; onPress: () => void }) {
  // The knob slides (smooth motion), or jumps with reduced motion on.
  const [x] = useState(() => new Animated.Value(on ? 1 : 0));
  useEffect(() => {
    let cancelled = false;
    void AccessibilityInfo.isReduceMotionEnabled()
      .catch(() => false)
      .then((reduced) => {
        if (cancelled) return;
        if (reduced) x.setValue(on ? 1 : 0);
        else Animated.timing(x, { toValue: on ? 1 : 0, duration: 160, easing: Easing.out(Easing.cubic), useNativeDriver: true }).start();
      });
    return () => {
      cancelled = true;
    };
  }, [on, x]);
  return (
    <Pressable
      accessibilityRole="switch"
      accessibilityState={{ checked: on, busy }}
      accessibilityLabel={`${item.name}. ${item.description}`}
      disabled={busy}
      onPress={onPress}
      style={({ pressed }) => [s.row, pressed && { backgroundColor: UI.row }]}
    >
      <Icon name={item.icon} color={on ? UI.accent : UI.textDim} size={18} />
      <View style={s.text}>
        <Text style={s.name}>{item.name}</Text>
        <Text style={s.description}>{item.description}</Text>
      </View>
      <View style={[s.track, on && { backgroundColor: UI.accent }]}>
        <Animated.View style={[s.knob, { transform: [{ translateX: x.interpolate({ inputRange: [0, 1], outputRange: [0, 14] }) }] }]} />
      </View>
    </Pressable>
  );
}

const s = StyleSheet.create({
  list: { gap: 10 },
  section: { gap: 2 },
  heading: { color: UI.textFaint, fontSize: 11, fontFamily: fonts.semibold, letterSpacing: 0.6, textTransform: "uppercase", paddingHorizontal: 12, paddingBottom: 2 },
  row: { minHeight: 48, flexDirection: "row", alignItems: "center", gap: 12, paddingHorizontal: 12, paddingVertical: 8, borderRadius: 10 },
  text: { flex: 1, gap: 2 },
  name: { color: UI.text, fontSize: 14, fontFamily: fonts.semibold },
  description: { color: UI.textDim, fontSize: 12, fontFamily: fonts.regular },
  status: { color: UI.textFaint, fontSize: 12, fontFamily: fonts.regular, paddingHorizontal: 12, paddingTop: 4 },
  track: { width: 34, height: 20, borderRadius: 10, backgroundColor: UI.raised, padding: 3 },
  knob: { width: 14, height: 14, borderRadius: 7, backgroundColor: UI.text },
});

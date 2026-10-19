import {
  alertDisplay,
  etDayKey,
  isActiveAlert,
  metaText,
  newAlert,
  PHONE_BUDGETS,
  shortConditionText,
  type AlertDisplayKind,
  type AlertRule,
} from "@stockwatcher/app-state";
import { etParts, etToUtc, formatEtDate, formatEtTime } from "@stockwatcher/chart-core";
import { Icon, Segmented, Sheet, UI } from "@stockwatcher/chart-native";
import type { AlertEventWire } from "@stockwatcher/shared";
import { useEffect, useMemo, useState, useSyncExternalStore } from "react";
import { Pressable, ScrollView, StyleSheet, Text, View } from "react-native";
import { useSafeAreaInsets } from "react-native-safe-area-context";

import { deleteAlert, openAlertSheet } from "../alerts/actions";
import { useReducedMotion } from "../motion/useReducedMotion";
import { enablePush, pushToken, subscribePushToken } from "../notifications/push";
import { appActions, loadAppStore, useApp } from "../state/store";
import { colors, fonts } from "../theme";

const DAY_MS = 86_400_000;

/** State chips: tinted surfaces, never a border; accent only for "Armed", the good tone for "Fired". */
const CHIP: Readonly<Record<AlertDisplayKind, { bg: string; fg: string }>> = {
  armed: { bg: "rgba(139,156,255,0.16)", fg: "#b3beff" },
  cooling: { bg: colors.bgRow, fg: colors.textDim },
  snoozed: { bg: colors.bgRow, fg: colors.textFaint },
  fired: { bg: colors.goodBg, fg: colors.good },
  expired: { bg: colors.bgRow, fg: colors.textFaint },
};

/**
 * The Alerts tab (approved design, "Alerts on the phone"): every alert with its state, a New
 * alert button, the phone budget, and Activity (fires and hold-backs, with the reason). It reads
 * the app store only: the dashboard's always-mounted connection brings the frames.
 */
export function AlertsScreen() {
  const [ready, setReady] = useState(false);
  useEffect(() => {
    void loadAppStore().then(() => setReady(true));
  }, []);
  if (!ready) return <View style={styles.screen} />;
  return <Alerts />;
}

function Alerts() {
  const insets = useSafeAreaInsets();
  const reducedMotion = useReducedMotion();
  const rules = useApp((s) => s.alerts);
  const states = useApp((s) => s.alertStates);
  const activity = useApp((s) => s.alertActivity);
  const status = useApp((s) => s.alertStatus);
  const prefs = useApp((s) => s.alertPrefs);
  const token = useSyncExternalStore(subscribePushToken, pushToken);
  const [pushNote, setPushNote] = useState<string | null>(null);
  const [view, setView] = useState<"alerts" | "activity">("alerts");
  const [menu, setMenu] = useState<AlertRule | null>(null);
  const [budgetOpen, setBudgetOpen] = useState(false);
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const t = setInterval(() => setNow(Date.now()), 30_000);
    return () => clearInterval(t);
  }, []);

  const ordered = useMemo(() => {
    const live = rules.filter((r) => isActiveAlert(r, states[r.id], now)).reverse();
    const done = rules.filter((r) => !isActiveAlert(r, states[r.id], now)).reverse();
    return [...live, ...done];
  }, [rules, states, now]);
  const budget = status?.phoneBudget ?? prefs.phoneBudget;
  const app = appActions();

  return (
    <View style={[styles.screen, { paddingTop: insets.top + 4 }]}>
      <View style={styles.header}>
        <Text style={styles.title} accessibilityRole="header">
          Alerts
        </Text>
        <Pressable accessibilityRole="button" accessibilityLabel={`Phone notifications today: ${status?.phoneToday ?? 0} of ${budget}. Change the budget`} onPress={() => setBudgetOpen(true)} hitSlop={8}>
          <Text style={styles.budget}>{`${status?.phoneToday ?? 0} / ${budget} phone today`}</Text>
        </Pressable>
        <Pressable accessibilityRole="button" accessibilityLabel="New alert" onPress={() => openAlertSheet(newAlert("", { kind: "price", dir: "above", price: 1 }, prefs))} style={({ pressed }) => [styles.newBtn, pressed && { opacity: 0.85 }]}>
          <Icon name="plus" color={colors.accentFg} size={18} />
        </Pressable>
      </View>
      {!token && (
        <Pressable
          accessibilityRole="button"
          onPress={() => void enablePush().then((r) => setPushNote(r === "granted" ? null : r === "denied" ? "Notifications are off for StockWatcher in the phone's settings." : r))}
          style={({ pressed }) => [styles.enable, pressed && { backgroundColor: UI.raised }]}
        >
          <Icon name="bell" color={colors.accent} size={16} />
          <View style={{ flex: 1, gap: 2 }}>
            <Text style={styles.enableText}>Turn on phone alerts</Text>
            <Text style={styles.small}>{pushNote ?? "Alerts reach this phone even when the app is closed."}</Text>
          </View>
        </Pressable>
      )}
      <Seg value={view} onChange={setView} options={[{ value: "alerts", label: `Alerts · ${rules.filter((r) => isActiveAlert(r, states[r.id], now)).length}` }, { value: "activity", label: "Activity" }]} />
      <ScrollView contentContainerStyle={{ gap: 8, paddingBottom: 24 }}>
        {view === "alerts" ? (
          ordered.length === 0 ? (
            <Text style={styles.empty}>No alerts yet. Long-press a chart, lift, and tap Alert; or long-press the price axis; or New alert.</Text>
          ) : (
            ordered.map((r) => {
              const d = alertDisplay(r, states[r.id], now);
              return (
                <Pressable
                  key={r.id}
                  accessibilityRole="button"
                  accessibilityLabel={`${r.symbol}: ${shortConditionText(r.rule)}, ${d.label}`}
                  accessibilityHint="Opens the alert; long-press for more"
                  onPress={() => openAlertSheet(r, { editId: r.id })}
                  onLongPress={() => setMenu(r)}
                  style={({ pressed }) => [styles.row, pressed && { backgroundColor: UI.row }]}
                >
                  <Text style={styles.ticker}>{r.symbol}</Text>
                  <View style={{ flex: 1, minWidth: 0, gap: 2 }}>
                    <Text style={styles.rule} numberOfLines={1}>
                      {shortConditionText(r.rule)}
                    </Text>
                    <Text style={styles.small} numberOfLines={1}>
                      {metaText(r, now)}
                    </Text>
                  </View>
                  <Text style={[styles.chip, { backgroundColor: CHIP[d.kind].bg, color: CHIP[d.kind].fg }]}>{d.kind === "cooling" ? "Cooling" : d.label}</Text>
                </Pressable>
              );
            })
          )
        ) : (
          <ActivityList events={activity} now={now} />
        )}
        <Text style={styles.note}>Alerts watch the live market only. Delivery to a phone cannot be guaranteed; Activity is the record.</Text>
      </ScrollView>

      <Sheet open={menu !== null} onClose={() => setMenu(null)} title={menu ? `${menu.symbol} · ${shortConditionText(menu.rule)}` : ""} reducedMotion={reducedMotion}>
        {menu && <RowActions r={menu} now={now} close={() => setMenu(null)} />}
      </Sheet>
      <Sheet open={budgetOpen} onClose={() => setBudgetOpen(false)} title="Phone notifications a day" reducedMotion={reducedMotion}>
        <View style={{ gap: 10 }}>
          <Seg value={String(prefs.phoneBudget)} onChange={(v) => app.setAlertPrefs({ ...prefs, phoneBudget: Number(v) })} options={PHONE_BUDGETS.map((b) => ({ value: String(b), label: String(b) }))} />
          <Text style={styles.small}>Past it, alerts still show in the app and in Activity. At most one per stock every 5 minutes.</Text>
          <Pressable accessibilityRole="switch" accessibilityState={{ checked: prefs.quietHours.on }} onPress={() => app.setAlertPrefs({ ...prefs, quietHours: { ...prefs.quietHours, on: !prefs.quietHours.on } })} style={({ pressed }) => [styles.action, pressed && { backgroundColor: UI.raised }]}>
            <Text style={styles.actionText}>{`Quiet hours ${prefs.quietHours.start}–${prefs.quietHours.end} ET`}</Text>
            <Text style={[styles.small, prefs.quietHours.on && { color: colors.accent }]}>{prefs.quietHours.on ? "On" : "Off"}</Text>
          </Pressable>
        </View>
      </Sheet>
    </View>
  );
}

/** Tomorrow 04:00 ET (the pre-market open). */
function tomorrowMorning(now: number): number {
  const p = etParts(now + DAY_MS);
  return etToUtc(p.year, p.month, p.day, 4);
}

function RowActions({ r, now, close }: { r: AlertRule; now: number; close: () => void }) {
  const app = appActions();
  const states = useApp((s) => s.alertStates);
  const d = alertDisplay(r, states[r.id], now);
  const act = (f: () => void) => () => {
    close();
    f();
  };
  const items: { label: string; run: () => void; danger?: boolean }[] = [{ label: "Edit", run: () => setTimeout(() => openAlertSheet(r, { editId: r.id }), 0) }];
  if (d.kind === "fired" || d.kind === "expired") items.push({ label: "Re-arm", run: () => app.updateAlert(r.id, { expiry: r.expiry }) });
  else if (d.kind === "snoozed") items.push({ label: "Unsnooze", run: () => app.snoozeAlert(r.id, null) });
  else {
    items.push({ label: "Snooze until tomorrow", run: () => app.snoozeAlert(r.id, tomorrowMorning(now)) });
    items.push({ label: "Snooze for a week", run: () => app.snoozeAlert(r.id, now + 7 * DAY_MS) });
  }
  items.push({ label: "Delete", run: () => deleteAlert(r.id), danger: true });
  return (
    <View style={{ gap: 8 }}>
      {items.map((i) => (
        <Pressable key={i.label} accessibilityRole="button" onPress={act(i.run)} style={({ pressed }) => [styles.action, pressed && { backgroundColor: UI.raised }]}>
          <Text style={[styles.actionText, i.danger && { color: colors.critical }]}>{i.label}</Text>
        </Pressable>
      ))}
    </View>
  );
}

function ActivityList({ events, now }: { events: readonly AlertEventWire[]; now: number }) {
  if (events.length === 0) return <Text style={styles.empty}>Fired alerts and every hold-back, with the reason, show here for 30 days.</Text>;
  const today = etDayKey(now);
  return (
    <View style={{ gap: 2 }}>
      {events.map((e, i) => {
        const key = etDayKey(e.t);
        const head = i === 0 || etDayKey(events[i - 1]!.t) !== key ? (key === today ? "Today" : formatEtDate(e.t)) : null;
        const held = e.kind === "held" || e.phone === "held" || e.phone === "failed";
        return (
          <View key={e.id}>
            {head && <Text style={styles.dayHead}>{head}</Text>}
            <View style={styles.event} accessible accessibilityLabel={`${formatEtTime(e.t)}, ${e.symbol}: ${e.text}. ${e.note}`}>
              <Text style={styles.time}>{formatEtTime(e.t)}</Text>
              <View style={{ flex: 1, gap: 2 }}>
                <Text style={styles.rule}>
                  <Text style={styles.ticker}>{`${e.symbol}  `}</Text>
                  {e.text}
                </Text>
                <Text style={[styles.small, held && { color: colors.warning }]}>{e.note}</Text>
              </View>
            </View>
          </View>
        );
      })}
    </View>
  );
}

const styles = StyleSheet.create({
  screen: { flex: 1, backgroundColor: colors.bg, paddingHorizontal: 6, gap: 10 },
  header: { flexDirection: "row", alignItems: "center", gap: 10 },
  title: { color: colors.text, fontFamily: fonts.semibold, fontSize: 20, flexShrink: 0 },
  budget: { color: colors.textDim, fontSize: 12, fontVariant: ["tabular-nums"] },
  newBtn: { marginLeft: "auto", width: 44, height: 44, borderRadius: 12, backgroundColor: colors.accent, alignItems: "center", justifyContent: "center" },
  enable: { flexDirection: "row", alignItems: "center", gap: 10, padding: 12, borderRadius: 12, backgroundColor: colors.bgPanel },
  enableText: { color: colors.text, fontFamily: fonts.semibold, fontSize: 14 },
  row: { minHeight: 56, flexDirection: "row", alignItems: "center", gap: 10, paddingHorizontal: 12, paddingVertical: 10, borderRadius: 12, backgroundColor: colors.bgPanel },
  ticker: { color: colors.text, fontFamily: fonts.semibold, fontSize: 13, fontVariant: ["tabular-nums"], minWidth: 52 },
  rule: { color: colors.text, fontSize: 13 },
  small: { color: colors.textFaint, fontSize: 11.5 },
  chip: { fontSize: 11, fontFamily: fonts.semibold, paddingHorizontal: 9, paddingVertical: 3, borderRadius: 11, overflow: "hidden" },
  empty: { color: colors.textFaint, fontSize: 13, paddingVertical: 8 },
  note: { color: colors.textFaint, fontSize: 11, marginTop: 8 },
  dayHead: { color: colors.textDim, fontSize: 11, fontFamily: fonts.semibold, letterSpacing: 0.6, textTransform: "uppercase", marginTop: 8, marginBottom: 2 },
  event: { flexDirection: "row", gap: 10, paddingVertical: 8 },
  time: { color: colors.textDim, fontSize: 12, fontVariant: ["tabular-nums"], width: 44 },
  action: { minHeight: 48, flexDirection: "row", alignItems: "center", justifyContent: "space-between", paddingHorizontal: 14, borderRadius: 12, backgroundColor: UI.row },
  actionText: { color: UI.text, fontSize: 14, fontWeight: "500" },
});

/** chart-native Segmented, filled, each option named by its label for screen readers. */
function Seg<T extends string>({ value, onChange, options }: { value: T; onChange: (v: T) => void; options: readonly { value: T; label: string }[] }) {
  return <Segmented fill value={value} onChange={onChange} options={options.map((o) => ({ ...o, a11y: o.label }))} />;
}

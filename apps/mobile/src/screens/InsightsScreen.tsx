import { Icon, Segmented, Sheet, UI } from "@stockwatcher/chart-native";
import { filterNotices, NOTICE_GROUPS, noticeSummary, type NoticeGroup, type NoticeStatus } from "@stockwatcher/shared";
import { useMemo, useState } from "react";
import { Pressable, ScrollView, StyleSheet, Text, TextInput, View } from "react-native";
import { useSafeAreaInsets } from "react-native-safe-area-context";

import { GROUP_ICON, noticeRowView, type NoticeRowView } from "../insights/model";
import { ScorecardView } from "../insights/ScorecardView";
import { useNoticeLog } from "../insights/useNoticeLog";
import { useReducedMotion } from "../motion/useReducedMotion";
import { openTickerChart } from "../navigation/ref";
import { colors, fonts } from "../theme";

type Tab = "notifications" | "scorecard";

/**
 * Insights (owner, 2026-10-26: "create an insight page where we can display a notification log"),
 * the web's Insights page on the phone: the notification log (every notice the server decided,
 * sent or held and why) and the scorecard. Its own tab, so it is one tap away on the Fold's cover
 * screen too; it reads the server over HTTP, not the dashboard's socket.
 */
export function InsightsScreen() {
  const insets = useSafeAreaInsets();
  const [tab, setTab] = useState<Tab>("notifications");
  return (
    <View style={[styles.screen, { paddingTop: insets.top + 4 }]}>
      <View style={styles.header}>
        <Text style={styles.title} accessibilityRole="header">
          Insights
        </Text>
      </View>
      <Segmented
        fill
        value={tab}
        onChange={setTab}
        options={[
          { value: "notifications", label: "Notifications", a11y: "Notifications" },
          { value: "scorecard", label: "Scorecard", a11y: "Scorecard" },
        ]}
      />
      {tab === "notifications" ? (
        <NotificationLog />
      ) : (
        // The scorecard has no scroll of its own (it is laid out for a screen to scroll it).
        <ScrollView style={{ flex: 1 }} showsVerticalScrollIndicator={false} showsHorizontalScrollIndicator={false} contentContainerStyle={{ paddingTop: 8, paddingBottom: insets.bottom + 24 }}>
          <ScorecardView />
        </ScrollView>
      )}
    </View>
  );
}

const STATUS: readonly { value: "all" | NoticeStatus; label: string }[] = [
  { value: "all", label: "All" },
  { value: "sent", label: "Sent" },
  { value: "held", label: "Held" },
];

function NotificationLog() {
  const reducedMotion = useReducedMotion();
  const [day, setDay] = useState<string | null>(null);
  const [dayOpen, setDayOpen] = useState(false);
  const [groups, setGroups] = useState<ReadonlySet<NoticeGroup>>(new Set());
  const [status, setStatus] = useState<"all" | NoticeStatus>("all");
  const [ticker, setTicker] = useState("");
  const [readout, setReadout] = useState<NoticeRowView | null>(null);
  const load = useNoticeLog(day);
  const log = load.state === "ok" ? load.log : null;
  const rows = useMemo(() => (log ? filterNotices(log.items, { groups, status: status === "all" ? null : status, ticker }).map(noticeRowView) : []), [log, groups, status, ticker]);
  const summary = useMemo(() => noticeSummary(log?.items ?? []), [log]);
  const toggle = (g: NoticeGroup) =>
    setGroups((prev) => {
      const next = new Set(prev);
      if (next.has(g)) next.delete(g);
      else next.add(g);
      return next;
    });

  return (
    <ScrollView showsVerticalScrollIndicator={false} showsHorizontalScrollIndicator={false} contentContainerStyle={{ gap: 8, paddingBottom: 24 }}>
      <View style={styles.titleRow}>
        <Text style={styles.panelTitle} accessibilityRole="header">
          Notification log
        </Text>
        {log && (
          <Pressable accessibilityRole="button" accessibilityLabel={`Day: ${log.day}. Change the day`} onPress={() => setDayOpen(true)} style={({ pressed }) => [styles.day, pressed && { backgroundColor: UI.raised }]}>
            <Text style={styles.dayText}>{day === null ? `Today · ${log.day}` : log.day}</Text>
            <Icon name="chevronDown" color={colors.textDim} size={14} />
          </Pressable>
        )}
      </View>
      {log && (
        <Text style={styles.small} accessibilityLiveRegion="polite">
          <Text style={styles.count}>{summary.sent}</Text> sent · <Text style={styles.count}>{summary.held}</Text> held
          {summary.reasons.length > 0 ? ` (${summary.reasons.map((r) => `${r.n} ${r.text}`).join(", ")})` : ""}
        </Text>
      )}
      <View style={styles.chips}>
        {NOTICE_GROUPS.map((g) => {
          const on = groups.has(g.id);
          return (
            <Pressable key={g.id} accessibilityRole="button" accessibilityState={{ selected: on }} accessibilityLabel={`${g.label} only`} onPress={() => toggle(g.id)} style={[styles.chip, on && { backgroundColor: UI.raised }]}>
              <Icon name={GROUP_ICON[g.id]} color={on ? colors.text : colors.textDim} size={14} />
              <Text style={[styles.chipText, on && { color: colors.text }]}>{g.label}</Text>
            </Pressable>
          );
        })}
      </View>
      <View style={styles.filterRow}>
        <Segmented value={status} onChange={setStatus} options={STATUS.map((s) => ({ value: s.value, label: s.label, a11y: s.label }))} />
        <TextInput
          value={ticker}
          onChangeText={(t) => setTicker(t.toUpperCase())}
          placeholder="Ticker"
          placeholderTextColor={colors.textFaint}
          autoCapitalize="characters"
          autoCorrect={false}
          accessibilityLabel="Filter by ticker"
          style={styles.input}
        />
      </View>

      {load.state === "loading" && <Text style={styles.empty}>Loading…</Text>}
      {load.state === "error" && <Text style={[styles.empty, { color: colors.warning }]}>{load.message}</Text>}
      {log && log.items.length === 0 && <Text style={styles.empty}>{`No notifications recorded ${day === null ? "yet today" : `on ${log.day}`}. The log starts on October 26, 2026: every notice the server decides, sent or held, with the reason.`}</Text>}
      {log && log.items.length > 0 && rows.length === 0 && <Text style={styles.empty}>Nothing matches these filters.</Text>}
      {rows.map((r) => (
        <Pressable
          key={r.key}
          accessibilityRole="button"
          accessibilityLabel={r.a11y}
          accessibilityHint="Opens the chart; long-press for the details"
          disabled={!r.symbol}
          onPress={() => r.symbol && openTickerChart(r.symbol)}
          onLongPress={() => setReadout(r)}
          style={({ pressed }) => [styles.row, pressed && { backgroundColor: UI.row }]}
        >
          <View style={styles.rowTop}>
            <Text style={styles.time}>{r.time}</Text>
            <Text style={styles.ticker}>{r.symbol ?? "—"}</Text>
            <Icon name={r.icon} color={colors.textDim} size={14} />
            <Text style={styles.kind} numberOfLines={1}>
              {r.kind}
            </Text>
            <View style={styles.paths}>
              {r.desk && <Icon name="screen" color={r.desk.sent ? colors.textDim : colors.warning} size={14} />}
              {r.phone && <Icon name="phone" color={r.phone.sent ? colors.textDim : colors.warning} size={14} />}
              <Text style={[styles.status, r.sent ? styles.statusSent : styles.statusHeld]}>{r.status}</Text>
            </View>
          </View>
          <Text style={[styles.detail, !r.sent && { color: colors.warning }]} numberOfLines={1}>
            {r.detail}
          </Text>
        </Pressable>
      ))}
      {log?.truncated && <Text style={styles.small}>Showing the newest 2,000 of the day.</Text>}

      <Sheet open={dayOpen} onClose={() => setDayOpen(false)} title="Day" reducedMotion={reducedMotion}>
        <View style={{ gap: 6 }}>
          {(log?.days.length ? log.days : log ? [log.day] : []).map((d, i) => (
            <Pressable
              key={d}
              accessibilityRole="button"
              accessibilityState={{ selected: (day ?? log?.day) === d }}
              onPress={() => {
                setDay(i === 0 ? null : d);
                setDayOpen(false);
              }}
              style={({ pressed }) => [styles.action, ((day ?? log?.day) === d || pressed) && { backgroundColor: UI.raised }]}
            >
              <Text style={styles.actionText}>{i === 0 ? `Today · ${d}` : d}</Text>
            </Pressable>
          ))}
        </View>
      </Sheet>
      <Sheet open={readout !== null} onClose={() => setReadout(null)} title={readout ? `${readout.symbol ?? "Notice"} · ${readout.kind}` : ""} reducedMotion={reducedMotion}>
        {readout && <Text style={styles.readout}>{readout.a11y.split(". ").join("\n")}</Text>}
      </Sheet>
    </ScrollView>
  );
}

const styles = StyleSheet.create({
  screen: { flex: 1, backgroundColor: colors.bg, paddingHorizontal: 6, gap: 10 },
  header: { flexDirection: "row", alignItems: "center", gap: 10, minHeight: 44 },
  title: { color: colors.text, fontFamily: fonts.semibold, fontSize: 20 },
  titleRow: { flexDirection: "row", alignItems: "center", gap: 10, flexWrap: "wrap", marginTop: 4 },
  panelTitle: { color: colors.heading, fontFamily: fonts.semibold, fontSize: 15 },
  day: { flexDirection: "row", alignItems: "center", gap: 4, minHeight: 36, paddingHorizontal: 10, borderRadius: 10, backgroundColor: colors.bgPanel },
  dayText: { color: colors.text, fontSize: 12 },
  small: { color: colors.textFaint, fontSize: 12 },
  count: { color: colors.text, fontVariant: ["tabular-nums"] },
  chips: { flexDirection: "row", flexWrap: "wrap", gap: 6 },
  chip: { flexDirection: "row", alignItems: "center", gap: 5, minHeight: 36, paddingHorizontal: 11, borderRadius: 18, backgroundColor: colors.bgPanel },
  chipText: { color: colors.textDim, fontSize: 12 },
  filterRow: { flexDirection: "row", alignItems: "center", gap: 8 },
  input: { flex: 1, minHeight: 40, paddingHorizontal: 10, borderRadius: 10, backgroundColor: colors.bgPanel, color: colors.text, fontSize: 13 },
  empty: { color: colors.textFaint, fontSize: 13, paddingVertical: 8 },
  row: { minHeight: 56, gap: 4, paddingHorizontal: 12, paddingVertical: 10, borderRadius: 12, backgroundColor: colors.bgPanel },
  rowTop: { flexDirection: "row", alignItems: "center", gap: 6 },
  time: { color: colors.textDim, fontSize: 12, fontVariant: ["tabular-nums"] },
  ticker: { color: colors.text, fontFamily: fonts.semibold, fontSize: 13 },
  kind: { color: colors.textDim, fontSize: 12, flexShrink: 1 },
  paths: { marginLeft: "auto", flexDirection: "row", alignItems: "center", gap: 6 },
  status: { fontSize: 11, fontFamily: fonts.semibold, paddingHorizontal: 8, paddingVertical: 2, borderRadius: 10, overflow: "hidden" },
  statusSent: { color: colors.textDim, backgroundColor: colors.bgRow },
  statusHeld: { color: colors.warning, backgroundColor: colors.warningBg },
  detail: { color: colors.textFaint, fontSize: 12 },
  action: { minHeight: 48, justifyContent: "center", paddingHorizontal: 14, borderRadius: 12, backgroundColor: UI.row },
  actionText: { color: UI.text, fontSize: 14 },
  readout: { color: colors.text, fontSize: 14, lineHeight: 21 },
});

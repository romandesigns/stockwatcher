import type { DashboardSource } from "@stockwatcher/app-state";
import { Icon } from "@stockwatcher/chart-native";
import { useCallback, useEffect, useState } from "react";
import { Pressable, StyleSheet, Text, useWindowDimensions, View } from "react-native";
import { useSafeAreaInsets } from "react-native-safe-area-context";

import { ChartTile } from "../dashboard/ChartTile";
import { CountSheet } from "../dashboard/CountSheet";
import { maxChartsFor } from "../dashboard/layout";
import { TickerList } from "../dashboard/TickerList";
import { useCandidates, usePaperTrading } from "../feed/hooks";
import type { MobileFeed } from "../feed/serverFeed";
import { useConnection } from "../feed/useConnection";
import { useReducedMotion } from "../motion/useReducedMotion";
import { TickerSearchSheet } from "../search/TickerSearchSheet";
import { appActions, useApp } from "../state/store";
import { colors, fonts } from "../theme";
import { SignInScreen } from "./SignInScreen";

/** The ticker list's share of the screen under the panel. */
const LIST_HEIGHT = 200;

/**
 * The main page, the first tab (owner, 2026-10-18): the LIVE market's charts, a "screen × n"
 * control for how many, and the live ticker list below. Touch a chart to select it, then tap a
 * ticker (in the list, or from the search) to load it there. The panel fills the space between
 * the header and the list (charts inside a scrolling page would fight the chart's own pan
 * gestures).
 *
 * The count is the synced one (the web's, up to 4) capped by the screen (`maxChartsFor`): one
 * chart on a phone-size screen, where the count control is hidden (it would offer one choice),
 * two on a tablet-size one, stacked while the panel is taller than wide (a Fold unfolded in
 * portrait: two wide charts read better than two narrow ones) and side by side otherwise. The cap
 * is never written back, so the phone does not shrink the web's panel; picking a count here does
 * set the synced one. What each chart shows syncs across devices; the selection is this phone's.
 * Its own socket (useConnection), so the chart tab's slot never counts against the server's 4.
 */
export function DashboardScreen() {
  // Settings → Data (synced): the live market unless replay was chosen; a change reconnects.
  const source = useApp((s) => s.dashboard.source);
  const { ready, status, signIn } = useConnection("mobile-dashboard", source);
  if (!ready) return <View style={styles.screen} />;
  if (status === "unauthorized") return <SignInScreen rejected={ready.token !== null} onSubmit={signIn} />;
  return <DashboardView feed={ready.feed} token={ready.token} source={source} />;
}

function DashboardView({ feed, token, source }: { feed: MobileFeed; token: string | null; source: DashboardSource }) {
  const insets = useSafeAreaInsets();
  const reducedMotion = useReducedMotion();
  const [app] = useState(appActions);
  const dashboard = useApp((s) => s.dashboard);
  const selected = useApp((s) => s.dashboardSelected);
  const recents = useApp((s) => s.searchRecents);
  const [clockDay, setClockDay] = useState<string | null>(null);
  const { symbols: candidates, fixture } = useCandidates(token, clockDay, source);
  const trading = usePaperTrading(feed);
  const [searchOpen, setSearchOpen] = useState(false);
  const [countOpen, setCountOpen] = useState(false);
  const [panel, setPanel] = useState({ width: 0, height: 0 });
  const screen = useWindowDimensions();
  const maxCharts = maxChartsFor(screen.width, screen.height);
  const n = Math.min(dashboard.count, maxCharts);
  // A chart selected on a bigger screen (or before folding) may not be shown here: select the
  // first, so a picked ticker never loads into a hidden chart. The selection is device state.
  useEffect(() => {
    if (selected >= n) app.selectDashboardChart(0);
  }, [app, selected, n]);

  // Each chart's ticker, or (empty slot / left the band) a different candidate per chart, shown but never written back.
  const shown = dashboard.slots.slice(0, n).map((slot, i) => {
    const fallback = candidates[i] ?? candidates[0] ?? "";
    const s = slot.symbol ?? fallback;
    return candidates.length === 0 || candidates.includes(s) ? s : fallback;
  });
  const onDay = useCallback((day: string | null) => {
    if (day) setClockDay(day);
  }, []);
  const pickFor = useCallback(
    (i: number) => {
      app.selectDashboardChart(i);
      setSearchOpen(true);
    },
    [app],
  );

  // Two charts side by side only when the panel (measured, a container query) is wider than tall.
  const rows: number[][] = n === 2 && panel.width > panel.height ? [[0, 1]] : shown.map((_, i) => [i]);

  const tile = (i: number) => (
    <ChartTile
      key={i}
      index={i}
      slot={dashboard.slots[i]!}
      symbol={shown[i]!}
      feed={feed}
      trading={trading}
      selected={selected === i}
      showSelection={n > 1}
      onPickTicker={pickFor}
      onDay={onDay}
    />
  );

  return (
    <View style={[styles.screen, { paddingTop: insets.top + 8 }]}>
      <View style={styles.header}>
        <Text accessibilityRole="header" style={styles.title}>
          Dashboard
        </Text>
        {n > 1 && <Text style={styles.sub}>Chart {selected + 1} selected</Text>}
        <Pressable accessibilityRole="button" accessibilityLabel="Search tickers" onPress={() => setSearchOpen(true)} hitSlop={6} style={({ pressed }) => [styles.round, { marginLeft: "auto" }, pressed && { backgroundColor: colors.bgRow }]}>
          <Icon name="search" color={colors.textDim} size={15} />
        </Pressable>
        {maxCharts > 1 && (
        <Pressable
          accessibilityRole="button"
          accessibilityLabel={`${n} ${n === 1 ? "chart" : "charts"} shown, change`}
          accessibilityState={{ expanded: countOpen }}
          onPress={() => setCountOpen(true)}
          hitSlop={6}
          style={({ pressed }) => [styles.multi, (pressed || countOpen) && { backgroundColor: colors.bgRow }]}
        >
          <Icon name="screen" color={countOpen ? colors.accent : colors.textDim} size={16} />
          <Text style={styles.multiText}>×{n}</Text>
        </Pressable>
        )}
      </View>

      <View style={styles.panel} onLayout={(e) => setPanel({ width: e.nativeEvent.layout.width, height: e.nativeEvent.layout.height })}>
        {candidates.length === 0 && !fixture ? (
          // The dashboard reads the LIVE market: an empty list means nothing has traded yet this
          // session (or it is the weekend), not a slow server.
          <Text style={styles.sub}>{source === "live" ? "No live trades yet. Charts appear with the session's first trades (overnight from 20:00 ET Sunday to Thursday, then 4:00 to 20:00 ET)." : "Waiting for the replay's symbols…"}</Text>
        ) : (
          rows.map((r, ri) => (
            <View key={ri} style={styles.row}>
              {r.map(tile)}
            </View>
          ))
        )}
      </View>

      <View style={{ height: LIST_HEIGHT }}>
        <TickerList quotes={feed.quotes} symbols={candidates} selectedSymbol={shown[selected] ?? ""} selectedIndex={selected} onPick={app.loadIntoSelected} />
      </View>

      <TickerSearchSheet open={searchOpen} onClose={() => setSearchOpen(false)} symbols={candidates} quotes={feed.quotes} mock={fixture} current={shown[selected] ?? ""} recents={recents} onSelect={app.loadIntoSelected} reducedMotion={reducedMotion} />
      <CountSheet open={countOpen} onClose={() => setCountOpen(false)} count={n} max={maxCharts} onChange={app.setDashboardCount} reducedMotion={reducedMotion} />
    </View>
  );
}

const styles = StyleSheet.create({
  screen: { flex: 1, backgroundColor: colors.bg, paddingHorizontal: 12, paddingBottom: 8, gap: 8 },
  header: { flexDirection: "row", alignItems: "center", gap: 8 },
  title: { color: colors.text, fontFamily: fonts.semibold, fontSize: 18 },
  sub: { color: colors.textFaint, fontFamily: fonts.regular, fontSize: 12 },
  round: { width: 32, height: 32, borderRadius: 16, alignItems: "center", justifyContent: "center", backgroundColor: colors.bgPanel },
  multi: { height: 32, flexDirection: "row", alignItems: "center", gap: 4, paddingHorizontal: 10, borderRadius: 16, backgroundColor: colors.bgPanel },
  multiText: { color: colors.textDim, fontSize: 12, fontVariant: ["tabular-nums"] },
  panel: { flex: 1, minHeight: 0, gap: 8 },
  row: { flex: 1, minHeight: 0, flexDirection: "row", gap: 8 },
});

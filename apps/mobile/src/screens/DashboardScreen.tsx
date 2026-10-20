import type { DashboardSource } from "@stockwatcher/app-state";
import { Icon } from "@stockwatcher/chart-native";
import { lastSessionLabel } from "@stockwatcher/shared";
import { useCallback, useEffect, useState } from "react";
import { Pressable, ScrollView, StyleSheet, Text, View } from "react-native";
import { useSafeAreaInsets } from "react-native-safe-area-context";

import { HaltWatcher } from "../dashboard/HaltWatcher";
import { RossPanel } from "../dashboard/RossPanel";
import { setDashboardFeed } from "../feed/dashboardFeed";
import { useCandidates } from "../feed/hooks";
import type { MobileFeed } from "../feed/serverFeed";
import { useConnection } from "../feed/useConnection";
import { useReducedMotion } from "../motion/useReducedMotion";
import { openTickerChart } from "../navigation/ref";
import { TickerSearchSheet } from "../search/TickerSearchSheet";
import { SettingsHost } from "../settings/SettingsHost";
import { appActions, useApp } from "../state/store";
import { colors, fonts } from "../theme";
import { SignInScreen } from "./SignInScreen";

/**
 * The main page, the first tab: the Halt watcher's gauges for the dashboard's market (live
 * unless Settings → Data says replay). Tapping a ticker, a gauge or a search result, opens its
 * chart in a modal (owner, 2026-10-19: "the chart should always be displayed on a modal"), so the
 * phone shows no chart of its own here and no "screen × n" count. The web keeps its panel: a
 * click there loads the ticker into the one chart, or the focused one of several.
 *
 * Its own socket (useConnection), shared with the modal (dashboardFeed), so the chart tab's slot
 * never counts against the server's 4.
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
  const recents = useApp((s) => s.searchRecents);
  const { symbols: candidates, fixture, session } = useCandidates(token, null, source);
  // A closed market shows its last session from the archive (owner, 2026-10-18): say which day.
  const sessionLabel = source === "live" ? lastSessionLabel(session) : null;
  const [searchOpen, setSearchOpen] = useState(false);
  useEffect(() => {
    setDashboardFeed(feed);
    return () => setDashboardFeed(null);
  }, [feed]);
  const open = useCallback(
    (symbol: string) => {
      app.rememberTicker(symbol);
      openTickerChart(symbol);
    },
    [app],
  );

  return (
    <View style={[styles.screen, { paddingTop: insets.top + 4 }]}>
      <View style={styles.header}>
        <Text accessibilityRole="header" style={styles.title}>
          Dashboard
        </Text>
        {sessionLabel && <Text style={styles.sub} numberOfLines={1}>{sessionLabel}</Text>}
        <Pressable accessibilityRole="button" accessibilityLabel="Search tickers" onPress={() => setSearchOpen(true)} hitSlop={6} style={({ pressed }) => [styles.round, { marginLeft: "auto" }, pressed && { backgroundColor: colors.bgRow }]}>
          <Icon name="search" color={colors.textDim} size={15} />
        </Pressable>
      </View>

      {/* One row of six mini gauges, all in view (owner, 2026-10-19); a tap opens the ticker's chart. */}
      <HaltWatcher channel={feed.pressure} onPick={open} reducedMotion={reducedMotion} />
      {/* The Ross Panel card (owner, 2026-10-20): a tapped row opens the chart modal like a gauge. */}
      <ScrollView style={{ flex: 1 }} showsVerticalScrollIndicator={false} showsHorizontalScrollIndicator={false} contentContainerStyle={{ paddingBottom: 8 }}>
        <RossPanel channel={feed.ross} onPick={open} reducedMotion={reducedMotion} />
      </ScrollView>
      {candidates.length === 0 && !fixture && (
        // The dashboard reads the LIVE market: an empty list means nothing has traded yet this
        // session (or it is the weekend), not a slow server.
        <Text style={styles.sub}>{source === "live" ? "No live trades yet. Tickers appear with the session's first trades (overnight from 20:00 ET Sunday to Thursday, then 4:00 to 20:00 ET)." : "Waiting for the replay's symbols…"}</Text>
      )}

      <TickerSearchSheet
        open={searchOpen}
        onClose={() => setSearchOpen(false)}
        symbols={candidates}
        quotes={feed.quotes}
        mock={fixture}
        current=""
        recents={recents}
        onSelect={(symbol) => {
          setSearchOpen(false);
          open(symbol);
        }}
        reducedMotion={reducedMotion}
      />
      <SettingsHost />
    </View>
  );
}

const styles = StyleSheet.create({
  screen: { flex: 1, backgroundColor: colors.bg, paddingHorizontal: 6, paddingBottom: 4, gap: 8 },
  header: { flexDirection: "row", alignItems: "center", gap: 8 },
  title: { color: colors.text, fontFamily: fonts.semibold, fontSize: 18 },
  sub: { color: colors.textFaint, fontFamily: fonts.regular, fontSize: 12 },
  round: { width: 32, height: 32, borderRadius: 16, alignItems: "center", justifyContent: "center", backgroundColor: colors.bgPanel },
});

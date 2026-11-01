import type { DashboardSource } from "@stockwatcher/app-state";
import { Icon } from "@stockwatcher/chart-native";
import { lastSessionLabel } from "@stockwatcher/shared";
import { useCallback, useEffect, useState } from "react";
import { Pressable, ScrollView, StyleSheet, Text, View } from "react-native";
import { useSafeAreaInsets } from "react-native-safe-area-context";

import { HaltWatcher } from "../dashboard/HaltWatcher";
import { DetectionPanel } from "../dashboard/DetectionPanel";
import { FavoritesPanel } from "../dashboard/FavoritesPanel";
import { MoversPanel } from "../dashboard/MoversPanel";
import { RossPanel } from "../dashboard/RossPanel";
import { RocketPanel } from "../dashboard/RocketPanel";
import { setDashboardFeed } from "../feed/dashboardFeed";
import { useCandidates } from "../feed/hooks";
import type { MobileFeed } from "../feed/serverFeed";
import { useConnection } from "../feed/useConnection";
import { useReducedMotion } from "../motion/useReducedMotion";
import { openTickerChart } from "../navigation/ref";
import { TickerSearchSheet } from "../search/TickerSearchSheet";
import { SettingsHost } from "../settings/SettingsHost";
import { useApp } from "../state/store";
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
  const recents = useApp((s) => s.searchRecents);
  const { symbols: candidates, fixture, session } = useCandidates(token, null, source, feed.symbols);
  // A closed market shows its last session from the archive (owner, 2026-10-18): say which day.
  const sessionLabel = source === "live" ? lastSessionLabel(session) : null;
  const [searchOpen, setSearchOpen] = useState(false);
  useEffect(() => {
    setDashboardFeed(feed);
    return () => setDashboardFeed(null);
  }, [feed]);
  // Every row, gauge and search result opens through the app's one action (navigation/ref.ts),
  // which also records the recents.
  const open = useCallback((symbol: string, detectionEpisodeId?: string, detectionRunEpoch?: string) => openTickerChart(symbol, detectionEpisodeId, detectionRunEpoch), []);

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
      <HaltWatcher channel={feed.pressure} onPick={open} reducedMotion={reducedMotion} gap={GAP} />
      {/* The cards, one scroll under the gauges. */}
      <ScrollView style={{ flex: 1 }} showsVerticalScrollIndicator={false} showsHorizontalScrollIndicator={false} contentContainerStyle={{ paddingBottom: 8, gap: GAP }}>
        {/* The web's panel order (owner, 2026-10-30 wireframe; shared panels.ts SIDE_PANELS then
            BOTTOM_PANELS), stacked for the phone: its right column (Favorites, Signals, Top
            Gainers), then its bottom row (Ross, Runners, Hot stocks). The Halt watcher keeps its
            row of gauges above the scroll (owner, 2026-10-19). Every card hides while it has 0
            items and opens or closes by its real height; a tapped row opens the chart modal. */}
        {/* The owner's own list first (owner, 2026-10-26). */}
        <FavoritesPanel channel={feed.favorites} runners={feed.rocket} onPick={open} reducedMotion={reducedMotion} gap={GAP} />
        {/* Signals (design canvas "Detection Panel", phone): only while the server's detector is
            on; a tapped row opens the chart modal with the episode drawn. */}
        <DetectionPanel channel={feed.detection} onPick={open} reducedMotion={reducedMotion} gap={GAP} />
        {/* Top Gainers (owner, 2026-10-30): ranked on the server, the movers channel. */}
        <MoversPanel kind="gainers" channel={feed.movers} runners={feed.rocket} onPick={open} reducedMotion={reducedMotion} gap={GAP} />
        <RossPanel channel={feed.ross} detection={feed.detection} runners={feed.rocket} onPick={open} reducedMotion={reducedMotion} gap={GAP} />
        {/* Rocket watch (owner, 2026-10-21; experimental): same row gestures. */}
        <RocketPanel channel={feed.rocket} onPick={open} reducedMotion={reducedMotion} gap={GAP} />
        {/* Hot stocks, Bull and Bear (owner, 2026-10-30): the movers channel too. */}
        <MoversPanel kind="hot" channel={feed.movers} runners={feed.rocket} onPick={open} reducedMotion={reducedMotion} gap={GAP} />
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
        search={feed.quoteSearch}
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

/**
 * The space between the screen's blocks and between the cards. Every card (and the Halt watcher
 * row) is hidden while it has 0 items (owner, 2026-10-22) and opens or closes by its real height;
 * it gets this gap so a closed one cancels the gap it would still take (motion/Collapse).
 */
const GAP = 8;

const styles = StyleSheet.create({
  screen: { flex: 1, backgroundColor: colors.bg, paddingHorizontal: 6, paddingBottom: 4, gap: GAP },
  header: { flexDirection: "row", alignItems: "center", gap: 8 },
  title: { color: colors.text, fontFamily: fonts.semibold, fontSize: 18 },
  sub: { color: colors.textFaint, fontFamily: fonts.regular, fontSize: 12 },
  round: { width: 32, height: 32, borderRadius: 16, alignItems: "center", justifyContent: "center", backgroundColor: colors.bgPanel },
});

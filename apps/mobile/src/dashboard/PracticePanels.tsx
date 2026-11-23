import { EMPTY_HOD, EMPTY_PENDING, type HodChannel, type PendingChannel } from "@stockwatcher/shared";
import { useCallback, useSyncExternalStore } from "react";
import { ScrollView, StyleSheet, Text } from "react-native";

import type { MobileFeed } from "../feed/serverFeed";
import { colors, fonts } from "../theme";
import { DetectionPanel } from "./DetectionPanel";
import { FavoritesPanel } from "./FavoritesPanel";
import { HaltWatcher } from "./HaltWatcher";
import { HodPanel } from "./HodPanel";
import { MoversPanel } from "./MoversPanel";
import { PendingPanel } from "./PendingPanel";
import { RocketPanel } from "./RocketPanel";
import { RossPanel } from "./RossPanel";

/** The space between the cards (the dashboard's GAP: a closed card cancels its gap, motion/Collapse). */
const GAP = 8;

const NOOP = () => () => {};

/** A channel's state, re-read on its changes (the HOD scanner, the News pending card). */
function useChannel<S>(channel: { get(): S; subscribe(cb: () => void): () => void } | undefined, empty: S): S {
  const subscribe = useCallback((cb: () => void) => (channel ? channel.subscribe(cb) : NOOP()), [channel]);
  const get = useCallback(() => (channel ? channel.get() : empty), [channel, empty]);
  return useSyncExternalStore(subscribe, get, get);
}

/**
 * The dashboard's cards for the practice screen (the Chart tab; replay parity plan Phase 1, owner
 * 2026-11-23), in the dashboard's own order (DashboardScreen.tsx), every one on the REPLAY
 * connection the chart uses. A tapped row loads its ticker into the practice chart (`onPick`),
 * never the live dashboard. Each card hides while it has 0 items; HOD Momentum and News pending
 * do not run on the replay clock yet, which one quiet line says rather than two empty cards.
 */
export function PracticePanels({ feed, onPick, reducedMotion, style }: { feed: MobileFeed; onPick: (symbol: string) => void; reducedMotion: boolean; style?: object }) {
  const hod = useChannel<ReturnType<HodChannel["get"]>>(feed.hod, EMPTY_HOD);
  const pending = useChannel<ReturnType<PendingChannel["get"]>>(feed.pending, EMPTY_PENDING);
  const off = [hod.available && hod.connected && !hod.enabled ? "HOD Momentum" : null, pending.available && pending.snapshot && !pending.enabled ? "News pending" : null].filter((x): x is string => x !== null);
  const open = useCallback((symbol: string) => onPick(symbol), [onPick]);
  return (
    <ScrollView style={[{ flex: 1 }, style]} showsVerticalScrollIndicator={false} showsHorizontalScrollIndicator={false} contentContainerStyle={{ paddingBottom: 8, gap: GAP }}>
      {off.length > 0 && <Text style={styles.note}>Not in practice yet: {off.join(", ")} (live market only for now), and the research shadow alerts.</Text>}
      <HaltWatcher channel={feed.pressure} onPick={open} reducedMotion={reducedMotion} gap={GAP} />
      <PendingPanel channel={feed.pending} onPick={open} reducedMotion={reducedMotion} gap={GAP} />
      <RossPanel channel={feed.ross} detection={feed.detection} runners={feed.rocket} onPick={open} reducedMotion={reducedMotion} gap={GAP} />
      <HodPanel channel={feed.hod} onPick={open} reducedMotion={reducedMotion} gap={GAP} />
      <RocketPanel channel={feed.rocket} onPick={open} reducedMotion={reducedMotion} gap={GAP} />
      <FavoritesPanel channel={feed.favorites} runners={feed.rocket} onPick={open} reducedMotion={reducedMotion} gap={GAP} />
      <DetectionPanel channel={feed.detection} onPick={open} reducedMotion={reducedMotion} gap={GAP} />
      <MoversPanel kind="gainers" channel={feed.movers} runners={feed.rocket} onPick={open} reducedMotion={reducedMotion} gap={GAP} />
      <MoversPanel kind="hot" channel={feed.movers} runners={feed.rocket} onPick={open} reducedMotion={reducedMotion} gap={GAP} />
    </ScrollView>
  );
}

const styles = StyleSheet.create({
  note: { color: colors.textFaint, fontFamily: fonts.regular, fontSize: 12, lineHeight: 16, paddingHorizontal: 2 },
});

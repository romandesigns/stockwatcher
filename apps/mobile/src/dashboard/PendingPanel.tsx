import { Icon, UI } from "@stockwatcher/chart-native";
import {
  fmtPendingClock,
  fmtPendingHm,
  fmtPendingPct,
  fmtPendingPrice,
  fmtPendingTime,
  pendingReopenPct,
  pendingSentence,
  pendingStatus,
  visiblePendingCards,
  type PendingCardWire,
  type PendingChannel,
} from "@stockwatcher/shared";
import { memo, useCallback, useEffect, useState, useSyncExternalStore } from "react";
import { Linking, Pressable, StyleSheet, Text, View } from "react-native";

import { Collapse } from "../motion/Collapse";
import { colors, fonts } from "../theme";
import { useTickerPress } from "../ui/useTickerPress";

/** From this card-list width two cards sit side by side (the Fold unfolded, tablets). */
const TWO_UP = 600;
const CARD_GAP = 6;

/** `a` blended toward `b` by `t` (both #rrggbb): a palette token tinted by another, no new colour. */
function mix(a: string, b: string, t: number): string {
  const p = (h: string, i: number) => parseInt(h.slice(1 + i * 2, 3 + i * 2), 16);
  const c = [0, 1, 2].map((i) => Math.round(p(a, i) * (1 - t) + p(b, i) * t));
  return `#${c.map((v) => v.toString(16).padStart(2, "0")).join("")}`;
}
const OFFERING_ROW = mix(UI.row, UI.critical, 0.06);

function usePendingState(channel: PendingChannel) {
  const subscribe = useCallback((cb: () => void) => channel.subscribe(cb), [channel]);
  return useSyncExternalStore(subscribe, useCallback(() => channel.get(), [channel]));
}

/** The wall clock, re-read once a second only while a card is on screen (timers come from timestamps). */
function useSecondClock(on: boolean): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!on) return;
    const id = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(id);
  }, [on]);
  return now;
}

const tone = (x: number | null) => (x === null ? UI.textFaint : x > 0 ? UI.good : x < 0 ? UI.critical : UI.textDim);

/**
 * The News pending card on the phone (the web's apps/web/src/dashboard/PendingPanel.tsx; server
 * crates/server/src/pending): one card per stock halted because news is pending (T1, $0.10–$25),
 * halted (warning colour, count-up from the halt) → news found (the headline or filing slides in;
 * an offering is flagged in the danger colour) → resumption set (a countdown to the reopen, the
 * card's only accent) → resumed (dims, then folds away when the server drops it).
 *
 * First in the dashboard's scroll: a halt for news is the most time-sensitive thing on it. Laid out
 * by its measured width: one card a row on a phone and the Fold's cover screen, two unfolded.
 * Tapping a card's head opens its chart (tap, then hold copies); a headline with a link opens it.
 */
export function PendingPanel({ channel, onPick, reducedMotion, gap = 0 }: { channel: PendingChannel; onPick: (symbol: string) => void; reducedMotion: boolean; gap?: number }) {
  const state = usePendingState(channel);
  const now = useSecondClock(state.cards.length > 0);
  const cards = visiblePendingCards(state, now);
  const [width, setWidth] = useState(0);
  const two = width >= TWO_UP && cards.length > 1;
  const cardWidth = two ? (width - CARD_GAP) / 2 : undefined;
  const reconnecting = !state.connected && cards.length > 0;
  const live = cards.filter((c) => c.phase !== "resumed").length;
  return (
    <Collapse open={cards.length > 0} reducedMotion={reducedMotion} gap={gap}>
      <View style={styles.card} accessibilityLabel="News pending">
        <View style={styles.head}>
          <Icon name="news" color={UI.textDim} size={14} />
          <Text accessibilityRole="header" style={styles.title}>
            News pending
          </Text>
          <Text style={styles.caption} numberOfLines={1}>
            T1 halts{state.band ? ` · $${state.band[0].toFixed(2)}–$${state.band[1]}` : ""}
          </Text>
          <Text style={styles.count}>{live}</Text>
        </View>
        {reconnecting && (
          <Text accessibilityRole="alert" style={styles.stale}>
            Reconnecting · last known state
          </Text>
        )}
        <View style={[styles.list, reconnecting && styles.dimmed]} onLayout={(e) => setWidth(e.nativeEvent.layout.width)}>
          {cards.map((c, i) => (
            // An odd last card on two columns takes the whole row (no orphan beside a gap).
            <View key={c.id} style={{ width: two && !(i === cards.length - 1 && cards.length % 2 === 1) ? cardWidth : "100%" }}>
              <PendingCard card={c} now={now} onPick={onPick} reducedMotion={reducedMotion} />
            </View>
          ))}
        </View>
      </View>
    </Collapse>
  );
}

const PendingCard = memo(function PendingCard({ card: c, now, onPick, reducedMotion }: { card: PendingCardWire; now: number; onPick: (s: string) => void; reducedMotion: boolean }) {
  const press = useTickerPress(c.sym, () => onPick(c.sym));
  const resumed = c.phase === "resumed";
  const counting = !resumed && c.resume_at !== null;
  const news = c.news[0];
  const reopenPct = pendingReopenPct(c);
  const statusColor = resumed ? UI.textDim : c.offering ? UI.critical : UI.warning;
  const url = news?.url ?? null;
  return (
    <View style={[styles.item, { backgroundColor: c.offering && !resumed ? OFFERING_ROW : UI.row, opacity: resumed ? 0.6 : 1 }]}>
      <Pressable
        accessibilityRole="button"
        accessibilityLabel={pendingSentence(c, now)}
        accessibilityHint="Opens its chart; tap, then press and hold to copy"
        {...press}
        style={({ pressed }) => [styles.top, pressed && { backgroundColor: UI.raised }]}
      >
        <View style={styles.left}>
          <View style={styles.line}>
            <Text style={styles.symbol}>{c.sym}</Text>
            <Text style={styles.price}>{fmtPendingPrice(c.price)}</Text>
            <Text style={[styles.chg, { color: tone(c.chg_pct) }]}>{fmtPendingPct(c.chg_pct)}</Text>
          </View>
          <View style={styles.status}>
            <Icon name={resumed ? "play" : "pause"} color={statusColor} size={11} />
            <Text style={[styles.statusText, { color: statusColor }]} numberOfLines={1}>
              {pendingStatus(c)}
              <Text style={{ color: UI.textFaint }}> · {fmtPendingTime(c.halted_at)}</Text>
            </Text>
          </View>
        </View>
        <View style={styles.timer}>
          <Text style={styles.timerLabel}>{resumed ? "resumed" : counting ? "reopens in" : "halted"}</Text>
          <Text style={[styles.timerValue, { color: resumed ? UI.textDim : counting ? UI.accent : statusColor }, counting && { fontFamily: fonts.semibold }]}>
            {resumed ? (c.resumed_at !== null ? fmtPendingHm(c.resumed_at) : "—") : counting ? `${c.resume_est ? "~" : ""}${fmtPendingClock((c.resume_at ?? now) - now)}` : fmtPendingClock(now - c.halted_at)}
          </Text>
        </View>
      </Pressable>
      <Collapse open={!!news || c.released_at !== null} reducedMotion={reducedMotion}>
        <View style={styles.news}>
          <View style={styles.newsHead}>
            <Icon name="news" color={UI.textDim} size={11} />
            <Text style={styles.newsSource} numberOfLines={1}>
              {news ? news.label : "Exchange"}
              <Text style={{ color: UI.textFaint }}> · {fmtPendingTime(news ? news.at : (c.released_at ?? now))}</Text>
            </Text>
            {c.offering && (
              <View style={styles.offering}>
                <Text style={styles.offeringText}>Offering</Text>
              </View>
            )}
          </View>
          {news ? (
            url ? (
              <Pressable accessibilityRole="link" onPress={() => void Linking.openURL(url)} style={({ pressed }) => [styles.link, pressed && { opacity: 0.7 }]}>
                <Text style={styles.headline} numberOfLines={2}>
                  {news.headline ?? `${news.label} item`}
                </Text>
              </Pressable>
            ) : (
              <Text style={styles.headline} numberOfLines={2}>
                {news.headline ?? (news.kind === "offering" ? "Offering filed" : `${news.label} item, no headline`)}
              </Text>
            )
          ) : (
            <Text style={[styles.headline, { color: UI.textDim }]}>News released, no story matched yet</Text>
          )}
        </View>
      </Collapse>
      <Collapse open={counting || (resumed && c.reopen_price !== null)} reducedMotion={reducedMotion}>
        <Text style={styles.foot}>
          {resumed ? (
            <>
              Reopened <Text style={{ color: UI.text }}>{fmtPendingPrice(c.reopen_price)}</Text> <Text style={{ color: tone(reopenPct) }}>{fmtPendingPct(reopenPct)}</Text> <Text style={{ color: UI.textFaint }}>from the halt</Text>
            </>
          ) : (
            <>
              Trading resumes {c.resume_est ? "~" : ""}
              <Text style={{ color: UI.text }}>{c.resume_at !== null ? fmtPendingHm(c.resume_at) : "—"}</Text>
              {c.resume_est ? <Text style={{ color: UI.textFaint }}> est.</Text> : null}
            </>
          )}
        </Text>
      </Collapse>
    </View>
  );
});

const styles = StyleSheet.create({
  card: { borderRadius: 14, backgroundColor: UI.panel, paddingVertical: 10, paddingHorizontal: 8, gap: 6 },
  head: { flexDirection: "row", alignItems: "center", gap: 8, paddingHorizontal: 4, minHeight: 32 },
  title: { color: UI.text, fontSize: 14, fontFamily: fonts.semibold },
  caption: { flexShrink: 1, color: UI.textFaint, fontSize: 11, fontFamily: fonts.regular },
  count: { marginLeft: "auto", color: UI.textFaint, fontSize: 11, fontVariant: ["tabular-nums"] },
  stale: { color: UI.warning, fontSize: 11, paddingHorizontal: 4 },
  dimmed: { opacity: 0.6 },
  list: { flexDirection: "row", flexWrap: "wrap", gap: CARD_GAP },
  item: { borderRadius: 10, paddingVertical: 6, paddingHorizontal: 6, gap: 4 },
  top: { flexDirection: "row", alignItems: "center", gap: 8, minHeight: 44, paddingHorizontal: 4, borderRadius: 8 },
  left: { flex: 1, minWidth: 0, gap: 2 },
  line: { flexDirection: "row", alignItems: "baseline", gap: 6 },
  symbol: { color: UI.text, fontSize: 14, fontFamily: fonts.semibold },
  price: { color: UI.text, fontSize: 13, fontVariant: ["tabular-nums"] },
  chg: { fontSize: 12, fontVariant: ["tabular-nums"] },
  status: { flexDirection: "row", alignItems: "center", gap: 4 },
  statusText: { flexShrink: 1, fontSize: 11, fontFamily: fonts.regular },
  timer: { alignItems: "flex-end", gap: 1 },
  timerLabel: { color: UI.textFaint, fontSize: 10, fontFamily: fonts.regular },
  timerValue: { fontSize: 14, fontVariant: ["tabular-nums"] },
  news: { gap: 3, borderRadius: 8, backgroundColor: UI.raised, paddingVertical: 6, paddingHorizontal: 8, marginHorizontal: 2 },
  newsHead: { flexDirection: "row", alignItems: "center", gap: 5 },
  newsSource: { flexShrink: 1, color: UI.textDim, fontSize: 11, fontFamily: fonts.regular },
  offering: { marginLeft: "auto", borderRadius: 4, paddingHorizontal: 6, paddingVertical: 1, backgroundColor: colors.criticalBg },
  offeringText: { color: UI.critical, fontSize: 10, fontFamily: fonts.semibold },
  link: { minHeight: 44, justifyContent: "center" },
  headline: { color: UI.text, fontSize: 12, lineHeight: 16, fontFamily: fonts.regular },
  foot: { color: UI.textDim, fontSize: 11, fontVariant: ["tabular-nums"], paddingHorizontal: 6, paddingBottom: 2 },
});

import { formatPrice, formatVolume } from "@stockwatcher/chart-core";
import { Icon, playHaptic } from "@stockwatcher/chart-native";
import {
  activeTickerFilters,
  refreshTickerRows,
  searchTickers,
  TICKER_SECTION_TITLES,
  tickerUniverse,
  visibleTickerFilterGroups,
  type QuoteChannel,
  type TickerFilterId,
  type TickerRow,
} from "@stockwatcher/shared";
import { Fragment, memo, useCallback, useLayoutEffect, useMemo, useRef, useState } from "react";
import { FlatList, Modal, Pressable, ScrollView, StyleSheet, Text, TextInput, View } from "react-native";
import { useSafeAreaInsets } from "react-native-safe-area-context";

import { useQuoteBoard } from "../feed/hooks";
import { colors, fonts } from "../theme";

/** Text with the matched letters in the accent, as on the web. */
function Highlight({ text, indices, style }: { text: string; indices: readonly number[]; style: object }) {
  if (!indices.length) return <Text style={style} numberOfLines={1}>{text}</Text>;
  const hit = new Set(indices);
  return (
    <Text style={style} numberOfLines={1}>
      {[...text].map((ch, i) => (hit.has(i) ? <Text key={i} style={s.hit}>{ch}</Text> : ch))}
    </Text>
  );
}

interface RowProps {
  row: TickerRow;
  current: boolean;
  onChoose: (r: TickerRow) => void;
  /** "See more" under the last shown recent (it carries `moreRecents`). */
  onMore: () => void;
}

/**
 * One row: ticker, name over "exchange · volume", and the live price over the change % (vs the
 * previous close when known, else the open; good/critical, the status colours). Memoized: refreshTickerRows keeps an unchanged row's
 * identity, so a quote update re-renders only the rows whose values moved.
 */
const Row = memo(function Row({ row: r, current, onChoose, onMore }: RowProps) {
  const t = r.item;
  const up = (t.changePct ?? 0) >= 0;
  const sub = [t.exchange, t.volume === undefined ? "" : `${formatVolume(t.volume)} vol`].filter(Boolean).join(" · ");
  const quote = t.price === undefined ? "" : `, ${formatPrice(t.price)}, ${up ? "up" : "down"} ${Math.abs(t.changePct ?? 0).toFixed(1)} percent`;
  return (
    <View>
      {r.section && <Text style={s.section}>{TICKER_SECTION_TITLES[r.section]}</Text>}
      <Pressable
        accessibilityRole="button"
        accessibilityState={{ disabled: !t.available, selected: current }}
        accessibilityLabel={`${t.symbol}, ${t.name}${t.exchange ? `, ${t.exchange}` : ""}${quote}${t.mock ? ", mock, not in the simulator" : ""}${current ? ", open" : ""}`}
        disabled={!t.available}
        onPress={() => onChoose(r)}
        style={({ pressed }) => [s.row, pressed && { backgroundColor: colors.bgRow }, !t.available && { opacity: 0.55 }]}
      >
        <Highlight text={t.symbol} indices={r.symbolIdx} style={s.symbol} />
        <View style={s.nameCol}>
          <Highlight text={t.name} indices={r.nameIdx} style={s.name} />
          {sub.length > 0 && <Text style={s.exchange}>{sub}</Text>}
        </View>
        <View style={s.right}>
          {current && <Text style={s.openBadge}>Open</Text>}
          {t.mock && <Text style={s.mock}>mock</Text>}
          {t.price !== undefined && (
            <View style={s.quote}>
              <Text style={s.price}>{formatPrice(t.price)}</Text>
              <Text style={[s.change, { color: up ? colors.good : colors.critical }]}>
                {up ? "+" : "−"}
                {Math.abs(t.changePct ?? 0).toFixed(1)}%
              </Text>
            </View>
          )}
        </View>
      </Pressable>
      {r.moreRecents ? (
        <Pressable accessibilityRole="button" accessibilityLabel={`See ${r.moreRecents} more recently opened`} onPress={onMore} style={({ pressed }) => [s.more, pressed && { backgroundColor: colors.bgRow }]}>
          <Text style={s.moreText}>See more ({r.moreRecents})</Text>
        </Pressable>
      ) : null}
    </View>
  );
});

export interface TickerSearchSheetProps {
  open: boolean;
  onClose: () => void;
  /** The server's candidates (openable symbols). */
  symbols: readonly string[];
  /** The feed's live quote board; subscribed only while the sheet is open. */
  quotes: QuoteChannel;
  /** Append the mock universe (fixture data only: noise next to a real day). */
  mock: boolean;
  current: string;
  /** Recently opened tickers, newest first: synced across devices by the app's store, which also records an opening (openTicker). */
  recents: readonly string[];
  onSelect: (symbol: string) => void;
  reducedMotion?: boolean;
}

/**
 * The phone's twin of the web's Ctrl+P quick-open: the same list, fuzzy
 * ranking, filters, sections and recents (all from @stockwatcher/shared),
 * as a full-screen sheet. The field is focused on open so the keyboard is
 * up at once; filters scroll sideways in one row under it; dragging the
 * list drops the keyboard. Every row shows the live price, change % (vs
 * the previous close when known, else the open) and day volume, pushed by
 * the server while the sheet is open.
 * Mock rows (fixture sources only) are shown, labelled, and not openable.
 * The FlatList virtualizes, so thousands of rows cost only what is on screen.
 */
export function TickerSearchSheet({ open, onClose, symbols, quotes, mock, current, recents, onSelect, reducedMotion }: TickerSearchSheetProps) {
  const insets = useSafeAreaInsets();
  const board = useQuoteBoard(quotes, open);
  const tickers = useMemo(() => tickerUniverse(symbols, board, { mock }), [symbols, board, mock]);
  const groups = useMemo(() => visibleTickerFilterGroups(tickers), [tickers]);
  const [query, setQuery] = useState("");
  // Recently opened shows RECENTS_COLLAPSED tickers until "See more" (owner, 2026-10-19).
  const [recentsExpanded, setRecentsExpanded] = useState(false);
  const more = useCallback(() => setRecentsExpanded(true), []);
  // Filters persist between openings (unlike the query), as on the web.
  const [filters, setFilters] = useState<ReadonlySet<TickerFilterId>>(new Set());

  // Every opening starts with an empty field, like the web: clear it on the way out.
  const close = () => {
    setQuery("");
    setRecentsExpanded(false);
    onClose();
  };
  // Ranked only when the query, filters or recents change, and once more when the first quote
  // snapshot lands (most-active order needs volumes); other updates refresh values in place, so
  // the list does not reshuffle under the thumb once a second (the web panel does the same).
  // React's "adjust state while rendering" pattern: re-rank when the key changes, else keep.
  const rankKey = [query, [...filters].sort().join(","), recents.join(","), board.size > 0, recentsExpanded].join("|");
  const [rank, setRank] = useState<{ key: string; rows: TickerRow[] }>({ key: "", rows: [] });
  let ranked = rank.rows;
  if (rank.key !== rankKey) {
    ranked = searchTickers(tickers, query, filters, recents, { recentsExpanded });
    setRank({ key: rankKey, rows: ranked });
  }
  // Titles whenever there is no query: Recently opened stays on top with filters on.
  const grouped = !query.trim();
  const filtered = activeTickerFilters(tickers, filters).size > 0;
  const rows = useMemo(() => refreshTickerRows(ranked, tickers, grouped, filtered), [ranked, tickers, grouped, filtered]);

  const toggle = (id: TickerFilterId) => {
    playHaptic("selection");
    setFilters((f) => {
      const next = new Set(f);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  };
  // Stable for the memoized rows; the latest callbacks arrive through a ref that is updated after
  // each render (never during it).
  const chooseRef = useRef<(r: TickerRow) => void>(() => {});
  useLayoutEffect(() => {
    chooseRef.current = (r: TickerRow) => {
      if (!r.item.available) return;
      playHaptic("light");
      onSelect(r.item.symbol);
      close();
    };
  });
  const choose = useCallback((r: TickerRow) => chooseRef.current(r), []);

  return (
    <Modal visible={open} animationType={reducedMotion ? "fade" : "slide"} onRequestClose={close} statusBarTranslucent navigationBarTranslucent>
      <View style={[s.screen, { paddingTop: insets.top + 8 }]} accessibilityViewIsModal>
        <View style={s.inputRow}>
          <View style={s.field}>
            <Icon name="search" color={colors.textDim} />
            <TextInput
              value={query}
              onChangeText={setQuery}
              autoFocus
              placeholder="Search tickers by symbol or name"
              placeholderTextColor={colors.textFaint}
              autoCorrect={false}
              autoCapitalize="none"
              spellCheck={false}
              returnKeyType="go"
              onSubmitEditing={() => {
                const first = rows.find((r) => r.item.available);
                if (first) choose(first);
              }}
              accessibilityLabel="Search tickers"
              style={s.input}
            />
            {query.length > 0 && (
              <Pressable accessibilityRole="button" accessibilityLabel="Clear search" hitSlop={10} onPress={() => setQuery("")} style={s.clearQuery}>
                <Icon name="clear" color={colors.textDim} size={18} />
              </Pressable>
            )}
          </View>
          <Pressable accessibilityRole="button" onPress={close} hitSlop={8} style={({ pressed }) => [s.cancel, pressed && { opacity: 0.6 }]}>
            <Text style={s.cancelText}>Cancel</Text>
          </Pressable>
        </View>

        {/* One sideways-scrolling row of chips; a soft bar between groups, as on the web. */}
        <ScrollView horizontal showsHorizontalScrollIndicator={false} keyboardShouldPersistTaps="handled" contentContainerStyle={s.chips} style={s.chipsScroll} accessibilityLabel="Filters">
          {groups.map((g, gi) => (
            <Fragment key={g.id}>
              {gi > 0 && <View style={s.groupBar} />}
              {g.chips.map((c) => {
                const on = filters.has(c.id);
                return (
                  <Pressable
                    key={c.id}
                    accessibilityRole="togglebutton"
                    accessibilityState={{ checked: on }}
                    onPress={() => toggle(c.id)}
                    style={({ pressed }) => [s.chip, on && s.chipOn, pressed && { opacity: 0.8 }]}
                  >
                    <Text style={[s.chipText, on && { color: colors.text }]}>{c.label}</Text>
                  </Pressable>
                );
              })}
            </Fragment>
          ))}
        </ScrollView>
        <View style={s.countRow}>
          <Text style={s.count}>
            {rows.length} {rows.length === 1 ? "ticker" : "tickers"}
          </Text>
          {filters.size > 0 && (
            <Pressable accessibilityRole="button" accessibilityLabel="Clear filters" hitSlop={8} onPress={() => setFilters(new Set())}>
              <Text style={s.clearText}>Clear filters</Text>
            </Pressable>
          )}
        </View>

        <FlatList
          data={rows}
          keyExtractor={(r) => r.item.symbol}
          keyboardShouldPersistTaps="handled"
          keyboardDismissMode="on-drag"
          contentContainerStyle={{ paddingBottom: insets.bottom + 16 }}
          ListEmptyComponent={<Text style={s.empty}>{query ? `No ticker matches “${query}”.` : "No ticker matches these filters."}</Text>}
          initialNumToRender={16}
          maxToRenderPerBatch={24}
          windowSize={7}
          renderItem={({ item: r }) => <Row row={r} current={r.item.symbol === current} onChoose={choose} onMore={more} />}
        />
      </View>
    </Modal>
  );
}

const s = StyleSheet.create({
  screen: { flex: 1, backgroundColor: colors.bg, paddingHorizontal: 12 },
  inputRow: { height: 42, flexDirection: "row", alignItems: "center", gap: 10 },
  field: { flex: 1, height: 42, flexDirection: "row", alignItems: "center", gap: 8, backgroundColor: colors.bgPanel, borderRadius: 12, paddingHorizontal: 12 },
  input: { flex: 1, height: 42, color: colors.text, fontSize: 15, fontFamily: fonts.regular, padding: 0 },
  // The duotone "clear" icon is its own soft disc, so the button needs no surface of its own.
  clearQuery: { width: 18, height: 18, alignItems: "center", justifyContent: "center" },
  cancel: { height: 42, justifyContent: "center" },
  cancelText: { color: colors.accent, fontSize: 15 },
  chipsScroll: { flexGrow: 0, height: 32, marginTop: 10, marginHorizontal: -12 },
  chips: { flexDirection: "row", alignItems: "center", gap: 6, paddingHorizontal: 12 },
  groupBar: { width: 1, height: 18, borderRadius: 1, backgroundColor: "#272e38", marginHorizontal: 2 },
  chip: { height: 32, paddingHorizontal: 12, borderRadius: 16, backgroundColor: colors.bgPanel, justifyContent: "center" },
  chipOn: { backgroundColor: "#272e38" },
  chipText: { color: colors.textDim, fontSize: 13 },
  // Fixed height: "Clear filters" appearing must not nudge the list (no layout jumps).
  countRow: { height: 34, flexDirection: "row", alignItems: "center", justifyContent: "space-between", paddingHorizontal: 4 },
  count: { color: colors.textFaint, fontSize: 11, fontVariant: ["tabular-nums"] },
  clearText: { color: colors.textDim, fontSize: 12 },
  more: { minHeight: 44, justifyContent: "center", paddingHorizontal: 8, borderRadius: 10 },
  moreText: { color: colors.textDim, fontSize: 13 },
  section: { color: colors.textFaint, fontSize: 10, fontWeight: "600", letterSpacing: 0.6, textTransform: "uppercase", paddingHorizontal: 8, paddingTop: 12, paddingBottom: 4 },
  row: { minHeight: 52, flexDirection: "row", alignItems: "center", gap: 10, paddingHorizontal: 8, borderRadius: 10 },
  symbol: { width: 62, color: colors.text, fontSize: 14, fontWeight: "600", fontVariant: ["tabular-nums"] },
  nameCol: { flex: 1, minWidth: 0, gap: 1 },
  name: { color: colors.textDim, fontSize: 13 },
  exchange: { color: colors.textFaint, fontSize: 10, fontVariant: ["tabular-nums"] },
  right: { flexDirection: "row", alignItems: "center", gap: 6 },
  quote: { minWidth: 58, alignItems: "flex-end", gap: 1 },
  openBadge: { color: colors.accent, fontSize: 10, fontWeight: "600", backgroundColor: "rgba(139,156,255,0.15)", borderRadius: 4, paddingHorizontal: 6, paddingVertical: 2, overflow: "hidden" },
  price: { color: colors.text, fontSize: 12, fontVariant: ["tabular-nums"] },
  change: { fontSize: 11, fontVariant: ["tabular-nums"] },
  mock: { color: colors.warning, fontSize: 9, fontWeight: "600", backgroundColor: colors.warningBg, borderRadius: 3, paddingHorizontal: 4, paddingVertical: 1, overflow: "hidden" },
  hit: { color: colors.accent, fontWeight: "700" },
  empty: { color: colors.textFaint, fontSize: 12, textAlign: "center", paddingVertical: 32 },
});

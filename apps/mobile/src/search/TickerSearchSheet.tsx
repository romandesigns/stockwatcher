import { formatPrice } from "@stockwatcher/chart-core";
import { playHaptic } from "@stockwatcher/chart-native";
import {
  pushRecentTicker,
  searchTickers,
  TICKER_FILTER_GROUPS,
  TICKER_SECTION_TITLES,
  type TickerFilterId,
  type TickerInfo,
  type TickerRow,
} from "@stockwatcher/shared";
import { Fragment, useEffect, useMemo, useState } from "react";
import { FlatList, Modal, Pressable, ScrollView, StyleSheet, Text, TextInput, View } from "react-native";
import { useSafeAreaInsets } from "react-native-safe-area-context";
import Svg, { Circle, Path } from "react-native-svg";

import { getPreference, setPreference } from "../storage/preferences";
import { colors, fonts } from "../theme";

const RECENTS_KEY = "search.recent";

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

export interface TickerSearchSheetProps {
  open: boolean;
  onClose: () => void;
  tickers: readonly TickerInfo[];
  current: string;
  onSelect: (symbol: string) => void;
  reducedMotion?: boolean;
}

/**
 * The phone's twin of the web's Ctrl+P quick-open: the same list, fuzzy
 * ranking, filters, sections and recents (all from @stockwatcher/shared),
 * as a full-screen sheet. The field is focused on open so the keyboard is
 * up at once; filters scroll sideways in one row under it; dragging the
 * list drops the keyboard. Mock rows are shown, labelled, and not openable.
 */
export function TickerSearchSheet({ open, onClose, tickers, current, onSelect, reducedMotion }: TickerSearchSheetProps) {
  const insets = useSafeAreaInsets();
  const [query, setQuery] = useState("");
  // Filters persist between openings (unlike the query), as on the web.
  const [filters, setFilters] = useState<ReadonlySet<TickerFilterId>>(new Set());
  const [recents, setRecents] = useState<string[]>([]);
  useEffect(() => {
    if (!open) return;
    void getPreference<unknown>(RECENTS_KEY).then((v) => setRecents(Array.isArray(v) ? v.filter((x): x is string => typeof x === "string") : []));
  }, [open]);

  // Every opening starts with an empty field, like the web: clear it on the way out.
  const close = () => {
    setQuery("");
    onClose();
  };
  const rows = useMemo(() => searchTickers(tickers, query, filters, recents), [tickers, query, filters, recents]);

  const toggle = (id: TickerFilterId) => {
    playHaptic("selection");
    setFilters((f) => {
      const next = new Set(f);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  };
  const choose = (r: TickerRow) => {
    if (!r.item.available) return;
    playHaptic("light");
    const next = pushRecentTicker(recents, r.item.symbol);
    setRecents(next);
    void setPreference(RECENTS_KEY, next);
    onSelect(r.item.symbol);
    close();
  };

  return (
    <Modal visible={open} animationType={reducedMotion ? "fade" : "slide"} onRequestClose={close} statusBarTranslucent navigationBarTranslucent>
      <View style={[s.screen, { paddingTop: insets.top + 8 }]} accessibilityViewIsModal>
        <View style={s.inputRow}>
          <View style={s.field}>
            <Svg width={16} height={16} viewBox="0 0 16 16" fill="none" stroke={colors.textDim} strokeWidth={1.5} strokeLinecap="round">
              <Circle cx={7} cy={7} r={4.5} />
              <Path d="M10.5 10.5 14 14" />
            </Svg>
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
                <Svg width={10} height={10} viewBox="0 0 10 10" stroke={colors.bg} strokeWidth={1.6} strokeLinecap="round">
                  <Path d="M2 2l6 6M8 2 2 8" />
                </Svg>
              </Pressable>
            )}
          </View>
          <Pressable accessibilityRole="button" onPress={close} hitSlop={8} style={({ pressed }) => [s.cancel, pressed && { opacity: 0.6 }]}>
            <Text style={s.cancelText}>Cancel</Text>
          </Pressable>
        </View>

        {/* One sideways-scrolling row of chips; a soft bar between groups, as on the web. */}
        <ScrollView horizontal showsHorizontalScrollIndicator={false} keyboardShouldPersistTaps="handled" contentContainerStyle={s.chips} style={s.chipsScroll} accessibilityLabel="Filters">
          {TICKER_FILTER_GROUPS.map((g, gi) => (
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
          renderItem={({ item: r }) => {
            const t = r.item;
            const up = (t.changePct ?? 0) >= 0;
            return (
              <View>
                {r.section && <Text style={s.section}>{TICKER_SECTION_TITLES[r.section]}</Text>}
                <Pressable
                  accessibilityRole="button"
                  accessibilityState={{ disabled: !t.available, selected: t.symbol === current }}
                  accessibilityLabel={`${t.symbol}, ${t.name}, ${t.exchange}${t.mock ? ", mock, not in the simulator" : ""}${t.symbol === current ? ", open" : ""}`}
                  disabled={!t.available}
                  onPress={() => choose(r)}
                  style={({ pressed }) => [s.row, pressed && { backgroundColor: colors.bgRow }, !t.available && { opacity: 0.55 }]}
                >
                  <Highlight text={t.symbol} indices={r.symbolIdx} style={s.symbol} />
                  <View style={s.nameCol}>
                    <Highlight text={t.name} indices={r.nameIdx} style={s.name} />
                    <Text style={s.exchange}>{t.exchange}</Text>
                  </View>
                  <View style={s.right}>
                    {t.symbol === current && <Text style={s.openBadge}>Open</Text>}
                    {t.available && t.symbol !== current && <Text style={s.inSim}>In simulator</Text>}
                    {t.mock && t.price !== undefined && (
                      <>
                        <Text style={s.price}>{formatPrice(t.price)}</Text>
                        <Text style={[s.change, { color: up ? colors.good : colors.critical }]}>
                          {up ? "+" : "−"}
                          {Math.abs(t.changePct ?? 0).toFixed(1)}%
                        </Text>
                        <Text style={s.mock}>mock</Text>
                      </>
                    )}
                  </View>
                </Pressable>
              </View>
            );
          }}
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
  clearQuery: { width: 18, height: 18, borderRadius: 9, backgroundColor: colors.textFaint, alignItems: "center", justifyContent: "center" },
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
  section: { color: colors.textFaint, fontSize: 10, fontWeight: "600", letterSpacing: 0.6, textTransform: "uppercase", paddingHorizontal: 8, paddingTop: 12, paddingBottom: 4 },
  row: { minHeight: 52, flexDirection: "row", alignItems: "center", gap: 10, paddingHorizontal: 8, borderRadius: 10 },
  symbol: { width: 62, color: colors.text, fontSize: 14, fontWeight: "600", fontVariant: ["tabular-nums"] },
  nameCol: { flex: 1, minWidth: 0, gap: 1 },
  name: { color: colors.textDim, fontSize: 13 },
  exchange: { color: colors.textFaint, fontSize: 10 },
  right: { flexDirection: "row", alignItems: "center", gap: 6 },
  openBadge: { color: colors.accent, fontSize: 10, fontWeight: "600", backgroundColor: "rgba(139,156,255,0.15)", borderRadius: 4, paddingHorizontal: 6, paddingVertical: 2, overflow: "hidden" },
  inSim: { color: colors.good, fontSize: 10 },
  price: { color: colors.text, fontSize: 12, fontVariant: ["tabular-nums"] },
  change: { fontSize: 11, fontVariant: ["tabular-nums"] },
  mock: { color: colors.warning, fontSize: 9, fontWeight: "600", backgroundColor: colors.warningBg, borderRadius: 3, paddingHorizontal: 4, paddingVertical: 1, overflow: "hidden" },
  hit: { color: colors.accent, fontWeight: "700" },
  empty: { color: colors.textFaint, fontSize: 12, textAlign: "center", paddingVertical: 32 },
});

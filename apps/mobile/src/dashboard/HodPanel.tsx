import { Icon, UI } from "@stockwatcher/chart-native";
import {
  filterHodRows,
  fmtHodMultiple,
  fmtHodPct,
  fmtHodPrice,
  fmtHodShares,
  fmtHodTime,
  hodItemCount,
  hodPolicyLine,
  hodShort,
  hodTint,
  parseHodFilter,
  toggleHodFilter,
  type HodChannel,
  type HodRowWire,
  type HodTint,
} from "@stockwatcher/shared";
import { memo, useCallback, useMemo, useState, useSyncExternalStore } from "react";
import { Pressable, StyleSheet, Text, View } from "react-native";

import { Collapse } from "../motion/Collapse";
import { useApp } from "../state/store";
import { colors, fonts } from "../theme";
import { useTickerPress } from "../ui/useTickerPress";

/** Rows shown before "Show all": the dashboard is one scroll. */
export const HOD_PREVIEW = 10;
/** From this row width the figures sit on one line with the ticker (the Fold unfolded, tablets). */
const WIDE_ROW = 560;

/** `a` blended toward `b` by `t` (both #rrggbb): a palette token tinted by another, no new colour. */
function mix(a: string, b: string, t: number): string {
  const p = (h: string, i: number) => parseInt(h.slice(1 + i * 2, 3 + i * 2), 16);
  const c = [0, 1, 2].map((i) => Math.round(p(a, i) * (1 - t) + p(b, i) * t));
  return `#${c.map((v) => v.toString(16).padStart(2, "0")).join("")}`;
}

/** Row tints by category family (the web's HodPanel TINT_ROW): a few percent of a token over the row. */
const TINT_ROW: Record<HodTint, string> = {
  lowFloat: mix(UI.row, colors.warning, 0.07),
  medFloat: mix(UI.row, colors.textDim, 0.06),
  squeeze: mix(UI.row, colors.flame, 0.07),
};
const TINT_DOT: Record<HodTint, string> = {
  lowFloat: mix(UI.panel, colors.warning, 0.7),
  medFloat: UI.textFaint,
  squeeze: mix(UI.panel, colors.flame, 0.7),
};

function useHod(channel: HodChannel) {
  const subscribe = useCallback((cb: () => void) => channel.subscribe(cb), [channel]);
  return useSyncExternalStore(subscribe, useCallback(() => channel.get(), [channel]));
}

const tone = (x: number | null | undefined) => (x === null || x === undefined || !Number.isFinite(x) ? UI.textFaint : x > 0 ? UI.good : x < 0 ? UI.critical : UI.textDim);

/**
 * The HOD Momentum scanner on the phone (the web's apps/web/src/dashboard/HodPanel.tsx; server
 * crates/server/src/hod/): every new high of day of a stock in the band that matches one of the
 * policy's categories, newest first, with time, ticker, price, the day's volume, float, rel vol
 * (daily rate and last 5 minutes), gap, change from the close and over 5 and 10 minutes, and its
 * categories as tags. Short interest is not shown (no source yet). Not an alert.
 *
 * "Categories" opens a checklist (the same synced `hodFilter` as the web, per user). Rows are
 * tinted by category family from the palette; the accent marks only a changed filter. Laid out by
 * its measured width (onLayout): two lines on a phone and the Fold's cover screen, one wider line
 * unfolded. Every target is at least 44 pt. Tapping a row opens its chart; tap, then hold copies.
 */
export function HodPanel({ channel, onPick, reducedMotion, gap = 0 }: { channel: HodChannel; onPick: (symbol: string) => void; reducedMotion: boolean; gap?: number }) {
  const state = useHod(channel);
  const raw = useApp((s) => s.hodFilter);
  const setFilter = useApp((s) => s.setHodFilter);
  const on = useMemo(() => parseHodFilter(raw, state.policy), [raw, state.policy]);
  const rows = useMemo(() => filterHodRows(state.rows, on), [state.rows, on]);
  const cats = state.policy?.categories ?? [];
  const changed = cats.some((c) => on.has(c.id) !== c.default_on);
  const [menu, setMenu] = useState(false);
  const [all, setAll] = useState(false);
  const [width, setWidth] = useState(0);
  const reconnecting = !state.connected && state.rows.length > 0;
  const wide = width >= WIDE_ROW;
  const head = rows.slice(0, HOD_PREVIEW);
  const rest = rows.slice(HOD_PREVIEW);
  const item = (r: { row: HodRowWire; cats: string[] }) => <HodRow key={r.row.id} row={r.row} cats={r.cats} wide={wide} onPick={onPick} />;
  return (
    <Collapse open={hodItemCount(state) > 0} reducedMotion={reducedMotion} gap={gap}>
      <View style={styles.card} accessibilityLabel="HOD Momentum" onLayout={(e) => setWidth(e.nativeEvent.layout.width)}>
        <View style={styles.head}>
          <Text accessibilityRole="header" style={styles.title}>
            HOD Momentum
          </Text>
          <Text style={styles.caption} numberOfLines={1}>
            new highs of day · not alerts
          </Text>
          <Text style={styles.count}>{rows.length}</Text>
          <Pressable
            accessibilityRole="button"
            accessibilityState={{ expanded: menu }}
            accessibilityLabel={`Categories, ${on.size} of ${cats.length} shown`}
            onPress={() => setMenu((v) => !v)}
            style={({ pressed }) => [styles.menuButton, pressed && { backgroundColor: UI.raised }]}
          >
            <Icon name="menu" color={changed ? UI.accent : UI.textDim} size={14} />
            <Text style={[styles.menuText, changed && { color: UI.accent }]}>
              {on.size}/{cats.length}
            </Text>
          </Pressable>
        </View>
        <Collapse open={menu} reducedMotion={reducedMotion} gap={6}>
          <View style={styles.menu} accessibilityLabel="Categories (saved for you)">
            {cats.map((c) => {
              const checked = on.has(c.id);
              const unavailable = c.needs_history && !state.hi52Available;
              return (
                <Pressable
                  key={c.id}
                  accessibilityRole="checkbox"
                  accessibilityState={{ checked }}
                  accessibilityLabel={c.name}
                  onPress={() => setFilter(toggleHodFilter(on, c.id))}
                  style={({ pressed }) => [styles.menuRow, pressed && { backgroundColor: UI.raised }]}
                >
                  <View style={[styles.check, checked && { backgroundColor: UI.raised }]}>{checked && <Icon name="check" color={UI.accent} size={12} />}</View>
                  <View style={[styles.dot, { backgroundColor: TINT_DOT[hodTint(c.id)] }]} />
                  <View style={{ flex: 1, minWidth: 0 }}>
                    <Text style={styles.menuName} numberOfLines={2}>
                      {c.name}
                    </Text>
                    {unavailable && <Text style={styles.note}>Needs 52 weeks of daily bars (not kept yet)</Text>}
                  </View>
                </Pressable>
              );
            })}
          </View>
        </Collapse>
        {(reconnecting || !state.complete || !state.on) && (
          <Text accessibilityRole="alert" style={styles.stale}>
            {!state.on ? "Switched off (Settings → Runners) · no new rows" : reconnecting ? "Reconnecting · last known rows" : `Watching since ${state.sinceMs ? fmtHodTime(state.sinceMs) : "a restart"} · highs since then`}
          </Text>
        )}
        <View style={[styles.rows, reconnecting && styles.dimmed]}>
          {head.map(item)}
          <Collapse open={all && rest.length > 0} reducedMotion={reducedMotion} gap={6}>
            <View style={styles.rows}>{rest.map(item)}</View>
          </Collapse>
          {rest.length > 0 && (
            <Pressable accessibilityRole="button" accessibilityState={{ expanded: all }} onPress={() => setAll((v) => !v)} style={({ pressed }) => [styles.more, pressed && { backgroundColor: UI.raised }]}>
              <Text style={styles.moreText}>{all ? "Show fewer" : `Show all ${rows.length}`}</Text>
              <Icon name={all ? "chevronUp" : "chevronDown"} color={UI.textDim} size={11} />
            </Pressable>
          )}
        </View>
        {state.policy && <Text style={styles.note}>{hodPolicyLine(state.policy)} Short interest: no data source yet.</Text>}
      </View>
    </Collapse>
  );
}

function Fig({ label, value, color }: { label: string; value: string; color?: string }) {
  return (
    <Text style={styles.fig}>
      <Text style={styles.figLabel}>{label} </Text>
      <Text style={color ? { color } : undefined}>{value}</Text>
    </Text>
  );
}

const HodRow = memo(function HodRow({ row: r, cats, wide, onPick }: { row: HodRowWire; cats: string[]; wide: boolean; onPick: (s: string) => void }) {
  const press = useTickerPress(r.sym, () => onPick(r.sym));
  const tint = TINT_ROW[hodTint(cats[0] ?? "")];
  const figs = (
    <View style={styles.figs}>
      <Fig label="Vol" value={fmtHodShares(r.vol)} />
      <Fig label="Float" value={fmtHodShares(r.float)} />
      <Fig label="RV" value={fmtHodMultiple(r.rvol)} />
      <Fig label="RV 5m" value={r.rvol5 === null ? "—" : `${Math.round(r.rvol5 * 100)}%`} />
      <Fig label="Gap" value={fmtHodPct(r.gap)} color={tone(r.gap)} />
      <Fig label="5m" value={fmtHodPct(r.chg5)} color={tone(r.chg5)} />
      <Fig label="10m" value={fmtHodPct(r.chg10)} color={tone(r.chg10)} />
    </View>
  );
  return (
    <Pressable
      accessibilityRole="button"
      accessibilityLabel={`${r.sym} new high ${fmtHodPrice(r.price)} at ${fmtHodTime(r.t)}, ${fmtHodPct(r.chg)} today, ${cats.map(hodShort).join(", ")}`}
      accessibilityHint="Opens its chart; tap, then press and hold to copy"
      {...press}
      style={({ pressed }) => [styles.row, { backgroundColor: pressed ? UI.raised : tint }]}
    >
      <View style={styles.line}>
        <Text style={styles.time}>{fmtHodTime(r.t)}</Text>
        <Text style={styles.symbol}>{r.sym}</Text>
        <Text style={styles.price}>{fmtHodPrice(r.price)}</Text>
        <Text style={[styles.chg, { color: tone(r.chg) }]}>{fmtHodPct(r.chg)}</Text>
        {wide && figs}
      </View>
      {!wide && figs}
      <View style={styles.tags}>
        {cats.map((c) => (
          <View key={c} style={styles.tag}>
            <View style={[styles.dot, { backgroundColor: TINT_DOT[hodTint(c)] }]} />
            <Text style={styles.tagText}>{hodShort(c)}</Text>
          </View>
        ))}
      </View>
    </Pressable>
  );
});

const styles = StyleSheet.create({
  card: { borderRadius: 14, backgroundColor: UI.panel, paddingVertical: 10, paddingHorizontal: 8, gap: 6 },
  head: { flexDirection: "row", alignItems: "center", gap: 8, paddingHorizontal: 4, minHeight: 44 },
  title: { color: UI.text, fontSize: 14, fontFamily: fonts.semibold },
  caption: { flexShrink: 1, color: UI.textFaint, fontSize: 11, fontFamily: fonts.regular },
  count: { marginLeft: "auto", color: UI.textFaint, fontSize: 11, fontVariant: ["tabular-nums"] },
  menuButton: { flexDirection: "row", alignItems: "center", gap: 5, minHeight: 44, minWidth: 44, paddingHorizontal: 10, borderRadius: 10, backgroundColor: UI.row, justifyContent: "center" },
  menuText: { color: UI.textDim, fontSize: 12, fontVariant: ["tabular-nums"] },
  menu: { gap: 2, borderRadius: 10, backgroundColor: UI.row, padding: 4 },
  menuRow: { flexDirection: "row", alignItems: "center", gap: 8, minHeight: 44, paddingHorizontal: 8, borderRadius: 8 },
  check: { width: 20, height: 20, borderRadius: 5, alignItems: "center", justifyContent: "center", backgroundColor: UI.panel },
  menuName: { color: UI.text, fontSize: 13, fontFamily: fonts.regular },
  note: { color: UI.textFaint, fontSize: 10, paddingHorizontal: 4, lineHeight: 14 },
  stale: { color: UI.warning, fontSize: 11, paddingHorizontal: 4 },
  dimmed: { opacity: 0.6 },
  rows: { gap: 6 },
  row: { gap: 4, minHeight: 44, justifyContent: "center", paddingVertical: 8, paddingHorizontal: 10, borderRadius: 10 },
  line: { flexDirection: "row", alignItems: "baseline", gap: 8, flexWrap: "wrap" },
  time: { color: UI.textFaint, fontSize: 11, fontVariant: ["tabular-nums"] },
  symbol: { color: UI.text, fontSize: 14, fontFamily: fonts.semibold },
  price: { color: UI.text, fontSize: 13, fontVariant: ["tabular-nums"], marginLeft: "auto" },
  chg: { fontSize: 13, fontVariant: ["tabular-nums"], fontFamily: fonts.semibold },
  figs: { flexDirection: "row", flexWrap: "wrap", columnGap: 10, rowGap: 2 },
  fig: { color: UI.textDim, fontSize: 11, fontVariant: ["tabular-nums"] },
  figLabel: { color: UI.textFaint },
  tags: { flexDirection: "row", flexWrap: "wrap", gap: 4 },
  tag: { flexDirection: "row", alignItems: "center", gap: 4, borderRadius: 4, paddingHorizontal: 6, paddingVertical: 2, backgroundColor: UI.raised },
  tagText: { color: UI.textDim, fontSize: 10 },
  dot: { width: 6, height: 6, borderRadius: 3 },
  more: { flexDirection: "row", alignItems: "center", justifyContent: "center", gap: 6, minHeight: 44, borderRadius: 10 },
  moreText: { color: UI.textDim, fontSize: 12, fontFamily: fonts.regular },
});

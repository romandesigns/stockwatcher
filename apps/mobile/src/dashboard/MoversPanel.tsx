import { formatEtTime, formatPrice } from "@stockwatcher/chart-core";
import { Icon, Segmented, UI } from "@stockwatcher/chart-native";
import {
  DEFAULT_MOVER_SORT,
  formatChangePct,
  formatDollarVolume,
  MOVER_SORT_KEYS,
  MOVER_SORT_LABEL,
  moverLine,
  moverRowMarks,
  moversCount,
  moversRows,
  nextMoverSort,
  parseMoverSort,
  rocketNow,
  runnersBySymbol,
  sortMovers,
  type MoverRowWire,
  type MoversChannel,
  type MoversList,
  type MoverSort,
  type MoverSortKey,
  type RocketChannel,
  type RocketRowWire,
} from "@stockwatcher/shared";
import { memo, useCallback, useEffect, useMemo, useState } from "react";
import { Pressable, StyleSheet, Text, View } from "react-native";

import { useMovers, useRocket } from "../feed/hooks";
import { Collapse } from "../motion/Collapse";
import { getPreference, setPreference } from "../storage/preferences";
import { fonts } from "../theme";
import { StarToggle } from "../ui/StarToggle";
import { useTickerPress } from "../ui/useTickerPress";
import { Flame, MarksLine, RowLine, useReadoutPress, useWall } from "./RowMarks";

const hhmm = (ms: number) => formatEtTime(ms);

export type MoversKind = "gainers" | "hot";

const TITLE: Record<MoversKind, string> = { gainers: "Top Gainers", hot: "Hot stocks" };
/** What each list is, under its title (the definitions live in crates/server/src/movers.rs). */
const CAPTION: Record<MoversKind, string> = { gainers: "% today · $250K+", hot: "$ volume today" };

/** Rows a list shows before "Show all": the dashboard is one scroll, so 25 a list would bury the cards under it. */
export const MOVERS_PREVIEW = 8;
/** The card's gap between its blocks, which a closed section must cancel (Collapse). */
const CARD_GAP = 6;
/** Below this card width the sort control takes its own line, filled (the Fold's cover screen). */
const SORT_INLINE_MIN = 440;
/** Below this row width the day line moves under the numbers, so every row still draws it. */
const NARROW_ROW = 340;

const SORT_PREF = (kind: MoversKind) => `moversSort.${kind}`;

/** The panel's sort, kept on this phone (a screen preference, never synced: the web keeps its own). */
function useMoverSort(kind: MoversKind): [MoverSort, (key: MoverSortKey) => void] {
  const [sort, setSort] = useState<MoverSort>(DEFAULT_MOVER_SORT);
  useEffect(() => {
    let live = true;
    void getPreference<unknown>(SORT_PREF(kind))
      .then((raw) => {
        if (live && raw !== null) setSort(parseMoverSort(raw));
      })
      .catch(() => {
        // Storage unavailable: the default order, and the choice lasts this session.
      });
    return () => {
      live = false;
    };
  }, [kind]);
  const press = useCallback(
    (key: MoverSortKey) =>
      setSort((cur) => {
        const next = nextMoverSort(cur, key);
        void setPreference(SORT_PREF(kind), next).catch(() => {});
        return next;
      }),
    [kind],
  );
  return [sort, press];
}

const DIR_WORDS: Record<MoverSortKey, [string, string]> = {
  dv: ["most traded first", "least traded first"],
  chg: ["biggest move first", "smallest move first"],
  price: ["highest price first", "lowest price first"],
  symbol: ["A to Z", "Z to A"],
};

/**
 * The sort as the app's segmented control (chart-native Segmented, as Signals and Insights use):
 * one segment per key, the active one raised (never accent) with a chevron for its direction
 * (down: the key's natural order, strongest first; up: reversed). Pressing the active segment
 * again flips it (shared nextMoverSort).
 */
function SortControl({ kind, sort, onPress, fill }: { kind: MoversKind; sort: MoverSort; onPress: (key: MoverSortKey) => void; fill: boolean }) {
  return (
    <Segmented
      fill={fill}
      value={sort.key}
      onChange={onPress}
      options={MOVER_SORT_KEYS.map((k) => {
        const on = sort.key === k;
        return {
          value: k,
          label: MOVER_SORT_LABEL[k],
          icon: on ? <Icon name={sort.dir === 1 ? "chevronDown" : "chevronUp"} color={UI.text} size={10} /> : undefined,
          a11y: on ? `${TITLE[kind]} sorted by ${MOVER_SORT_LABEL[k]}, ${DIR_WORDS[k][sort.dir === 1 ? 0 : 1]}. Press to reverse` : `Sort ${TITLE[kind]} by ${MOVER_SORT_LABEL[k]}`,
        };
      })}
    />
  );
}

/**
 * Top Gainers, and Hot stocks as two sections, Bull and Bear, on the phone (owner, 2026-10-30;
 * the web's apps/web/src/dashboard/MoversPanel.tsx). The lists are ranked on the server once a
 * tick for every client and pushed over the movers channel (crates/server/src/movers.rs; shared
 * movers.ts folds the frames, sorts and draws the line), so the phone never holds the whole quote
 * board for them: the day's biggest % gainers with $250K+ behind them, and the most traded names
 * up and down on the day, all $0.10–$25 with at least 100 prints, 25 each.
 *
 * Each card has its own sort ($ volume by default, %, price, ticker; press the active key again to
 * reverse it), kept on this phone. A list shows its first rows and opens the rest on "Show all"
 * (the dashboard is one scroll). Nothing empty is drawn (owner, 2026-10-22; shared moversCount):
 * no card while its lists are empty, no Bull or Bear section without rows; each opens and closes
 * by its real height (Collapse: layout, not a transform, so taps stay true). A reconnect keeps the
 * last rows, dimmed.
 *
 * One row per name in the lighter-row language (Favorites): flame, ticker, price, day change, the
 * live day line (the same RowLine: 30 s closes from 04:00 ET with the live price as its end, the
 * previous close dotted, the last minute as a tail), dollar volume, the star; the Runners path on
 * the marks line. Tapping a row opens its chart; tap, then hold copies; press and hold reads the
 * marks. Rows are memoized on their wire object, which the channel keeps for a row an update did
 * not touch, so a frame re-renders only the rows it changed.
 */
export function MoversPanel({ kind, channel, runners, onPick, reducedMotion, gap = 0 }: { kind: MoversKind; channel: MoversChannel; runners: RocketChannel; onPick: (symbol: string) => void; reducedMotion: boolean; gap?: number }) {
  const state = useMovers(channel);
  const rocket = useRocket(runners);
  const wall = useWall();
  const nowMs = rocketNow(rocket, wall);
  const byRunner = useMemo(() => runnersBySymbol(rocket), [rocket]);
  const [sort, pressSort] = useMoverSort(kind);
  const [width, setWidth] = useState(0);
  const reconnecting = !state.connected && state.rows !== null;
  const section = (list: MoversList) => sortMovers(moversRows(state, list), sort, list);
  const inlineSort = width >= SORT_INLINE_MIN;
  // The rows' own width: the card's, less its horizontal padding.
  const rowWidth = Math.max(0, width - 16);
  return (
    <Collapse open={moversCount(state, kind) > 0} reducedMotion={reducedMotion} gap={gap}>
      <View style={styles.card} accessibilityLabel={TITLE[kind]} onLayout={(e) => setWidth(e.nativeEvent.layout.width)}>
        <View style={styles.head}>
          <Text accessibilityRole="header" style={styles.title}>
            {TITLE[kind]}
          </Text>
          <Text style={styles.caption} numberOfLines={1}>
            {CAPTION[kind]}
          </Text>
          {kind === "gainers" && <Text style={styles.count}>{state.gainers.length}</Text>}
          {inlineSort && (
            <View style={styles.sortInline}>
              <SortControl kind={kind} sort={sort} onPress={pressSort} fill={false} />
            </View>
          )}
        </View>
        {!inlineSort && width > 0 && <SortControl kind={kind} sort={sort} onPress={pressSort} fill />}
        {reconnecting && (
          <Text accessibilityRole="alert" style={styles.stale}>
            Reconnecting · last known rows
          </Text>
        )}
        <View style={[styles.body, reconnecting && styles.dimmed]}>
          {kind === "gainers" ? (
            <MoverList label="Top Gainers" rows={section("gainers")} list="gainers" byRunner={byRunner} nowMs={nowMs} width={rowWidth} onPick={onPick} reducedMotion={reducedMotion} />
          ) : (
            <>
              <MoverSection list="bull" rows={section("bull")} byRunner={byRunner} nowMs={nowMs} width={rowWidth} onPick={onPick} reducedMotion={reducedMotion} />
              <MoverSection list="bear" rows={section("bear")} byRunner={byRunner} nowMs={nowMs} width={rowWidth} onPick={onPick} reducedMotion={reducedMotion} />
            </>
          )}
        </View>
      </View>
    </Collapse>
  );
}

const SECTION: Record<"bull" | "bear", { title: string; dot: string }> = {
  bull: { title: "Bull", dot: UI.good },
  bear: { title: "Bear", dot: UI.critical },
};

/** One Hot stocks section: its dot, title and count over its rows; closed while it has none. */
function MoverSection({ list, rows, byRunner, nowMs, width, onPick, reducedMotion }: { list: "bull" | "bear"; rows: MoverRowWire[]; byRunner: ReadonlyMap<string, RocketRowWire>; nowMs: number; width: number; onPick: (symbol: string) => void; reducedMotion: boolean }) {
  const s = SECTION[list];
  return (
    <Collapse open={rows.length > 0} reducedMotion={reducedMotion} gap={CARD_GAP}>
      <View style={styles.section} accessibilityLabel={s.title}>
        <View style={styles.sectionHead}>
          <View style={[styles.dot, { backgroundColor: s.dot }]} />
          <Text accessibilityRole="header" style={styles.sectionTitle}>
            {s.title}
          </Text>
          <Text style={styles.count}>{rows.length}</Text>
        </View>
        <MoverList label={s.title} rows={rows} list={list} byRunner={byRunner} nowMs={nowMs} width={width} onPick={onPick} reducedMotion={reducedMotion} />
      </View>
    </Collapse>
  );
}

/** A list's first rows, then the rest behind "Show all" (opened by its real height). */
function MoverList({ label, rows, list, byRunner, nowMs, width, onPick, reducedMotion }: { label: string; rows: MoverRowWire[]; list: MoversList; byRunner: ReadonlyMap<string, RocketRowWire>; nowMs: number; width: number; onPick: (symbol: string) => void; reducedMotion: boolean }) {
  const [all, setAll] = useState(false);
  const head = rows.slice(0, MOVERS_PREVIEW);
  const rest = rows.slice(MOVERS_PREVIEW);
  const row = (r: MoverRowWire) => <MoverRow key={r.symbol} row={r} list={list} runner={byRunner.get(r.symbol)} nowMs={nowMs} width={width} onPick={onPick} />;
  return (
    <View style={styles.rows} accessibilityLabel={`${label}, ${rows.length} rows`}>
      {head.map(row)}
      <Collapse open={all && rest.length > 0} reducedMotion={reducedMotion} gap={ROWS_GAP}>
        <View style={styles.rows}>{rest.map(row)}</View>
      </Collapse>
      {rest.length > 0 && (
        <Pressable accessibilityRole="button" accessibilityState={{ expanded: all }} accessibilityLabel={all ? `Show the first ${MOVERS_PREVIEW} of ${label}` : `Show all ${rows.length} of ${label}`} onPress={() => setAll((v) => !v)} style={({ pressed }) => [styles.more, pressed && { backgroundColor: UI.raised }]}>
          <Text style={styles.moreText}>{all ? "Show fewer" : `Show all ${rows.length}`}</Text>
          <Icon name={all ? "chevronUp" : "chevronDown"} color={UI.textDim} size={11} />
        </Pressable>
      )}
    </View>
  );
}

const ROWS_GAP = 6;

const MoverRow = memo(function MoverRow({ row: r, list, runner, nowMs, width, onPick }: { row: MoverRowWire; list: MoversList; runner: RocketRowWire | undefined; nowMs: number; width: number; onPick: (symbol: string) => void }) {
  const price = r.price / 1_000_000;
  const m = useMemo(() => moverRowMarks(price, runner, nowMs, hhmm, r.day_line), [price, runner, nowMs, r.day_line]);
  const press = useReadoutPress(useTickerPress(r.symbol, () => onPick(r.symbol)), m);
  const line = useMemo(() => moverLine(r), [r]);
  const change = formatChangePct(r.chg);
  const dollars = formatDollarVolume(r.dv);
  const shown = formatPrice(price);
  const changeColor = r.chg > 0 ? UI.good : r.chg < 0 ? UI.critical : UI.textDim;
  // The split note (crates/server/src/quotes.rs PcBasis): why the change may not be against the close.
  const basis = r.pcb === "split" ? "vs previous close, split-adjusted" : r.pcb === "guard" ? "vs today's first price: the previous close is on another share basis (split)" : "";
  // The row chart takes about a quarter of the row beside the numbers; on the narrowest rows (the
  // Fold's cover screen) it moves under them, beside the marks, and takes the room that is left.
  const narrow = width > 0 && width < NARROW_ROW;
  const chartW = narrow ? Math.max(72, width - 20 - 120) : Math.max(72, Math.min(160, Math.round(width * 0.26)));
  const color = line?.tone === "critical" ? UI.critical : UI.good;
  const chart = width > 0 ? <RowLine line={line} tail={m.tail} width={chartW} color={color} /> : null;
  const hasMarks = !!(m.path || m.pillars || m.rvol || m.news || m.zone || m.stage);
  return (
    <Pressable
      accessibilityRole="button"
      accessibilityLabel={`${r.symbol} ${shown}, ${change} today${basis ? ` ${basis}` : ""}, ${dollars} traded${r.halted ? ", halted" : ""}`}
      accessibilityHint="Opens its chart. Press and hold to read its marks; tap, then press and hold to copy"
      {...press}
      style={({ pressed }) => [styles.row, m.flame?.lit && { backgroundColor: UI.raised }, pressed && { backgroundColor: UI.raised }]}
    >
      <View style={styles.line}>
        <Flame flame={m.flame} />
        <Text style={styles.symbol}>{r.symbol}</Text>
        <Text style={styles.price}>{shown}</Text>
        <Text style={[styles.small, { color: changeColor }, list === "gainers" && styles.strong]}>
          {change}
          {basis ? <Text style={styles.basis}> s</Text> : null}
        </Text>
        {r.halted && (
          <View style={styles.badge}>
            <Text style={styles.badgeText}>HALTED</Text>
          </View>
        )}
        <View style={styles.controls}>
          {!narrow && chart}
          <Text style={[styles.dollars, { color: list === "gainers" ? UI.textDim : UI.text }]}>{dollars}</Text>
          <StarToggle symbol={r.symbol} />
        </View>
      </View>
      {(narrow || hasMarks) && (
        <View style={styles.bodyLine}>
          <View style={styles.marks}>
            <MarksLine marks={m} />
          </View>
          {narrow && chart}
        </View>
      )}
    </Pressable>
  );
});

const styles = StyleSheet.create({
  card: { borderRadius: 14, backgroundColor: UI.panel, paddingVertical: 10, paddingHorizontal: 8, gap: CARD_GAP },
  head: { flexDirection: "row", alignItems: "center", gap: 8, paddingHorizontal: 4, minHeight: 24 },
  title: { color: UI.text, fontSize: 14, fontFamily: fonts.semibold },
  caption: { flexShrink: 1, color: UI.textFaint, fontSize: 11, fontFamily: fonts.regular },
  count: { marginLeft: "auto", color: UI.textFaint, fontSize: 11, fontVariant: ["tabular-nums"] },
  sortInline: { marginLeft: "auto" },
  stale: { color: UI.warning, fontSize: 11, paddingHorizontal: 4, paddingBottom: 2 },
  dimmed: { opacity: 0.6 },
  body: { gap: CARD_GAP },
  section: { gap: 6 },
  sectionHead: { flexDirection: "row", alignItems: "center", gap: 6, paddingHorizontal: 4 },
  dot: { width: 6, height: 6, borderRadius: 3 },
  sectionTitle: { color: UI.textDim, fontSize: 12, fontFamily: fonts.semibold },
  rows: { gap: ROWS_GAP },
  row: { gap: 4, minHeight: 44, justifyContent: "center", paddingVertical: 8, paddingLeft: 10, paddingRight: 4, borderRadius: 10, backgroundColor: UI.row },
  line: { flexDirection: "row", alignItems: "center", gap: 7 },
  symbol: { color: UI.text, fontSize: 14, fontFamily: fonts.semibold },
  price: { color: UI.text, fontSize: 13, fontVariant: ["tabular-nums"] },
  small: { fontSize: 12, fontVariant: ["tabular-nums"] },
  strong: { fontFamily: fonts.semibold },
  basis: { color: UI.textFaint, fontSize: 9 },
  badge: { borderRadius: 4, paddingHorizontal: 5, paddingVertical: 1, backgroundColor: UI.warning },
  badgeText: { color: UI.accentFg, fontSize: 9, fontFamily: fonts.semibold },
  controls: { marginLeft: "auto", flexDirection: "row", alignItems: "center", gap: 6 },
  dollars: { fontSize: 12, fontVariant: ["tabular-nums"], fontFamily: fonts.regular },
  bodyLine: { flexDirection: "row", alignItems: "center", gap: 8 },
  marks: { flex: 1, minWidth: 0 },
  more: { flexDirection: "row", alignItems: "center", justifyContent: "center", gap: 6, minHeight: 44, borderRadius: 10 },
  moreText: { color: UI.textDim, fontSize: 12, fontFamily: fonts.regular },
});

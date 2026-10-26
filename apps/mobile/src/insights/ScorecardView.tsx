import { authenticatedFetch, parseScorecard, pct, scorecardRows, type ScorecardDay, type ScorecardRow } from "@stockwatcher/shared";
import { useCallback, useEffect, useMemo, useState } from "react";
import { Pressable, StyleSheet, Text, View, type LayoutChangeEvent } from "react-native";
import Svg, { Circle, Line, Polyline } from "react-native-svg";
import { SERVER_URL } from "../feed/server";
import { getAccessToken } from "../storage/secure";
import { colors, fonts, radius, space } from "../theme";
import { detailLines, summaryLine, trendRuns } from "./scorecardText";

// Insights › Scorecard on the phone (the web's apps/web/src/insights/ScorecardView.tsx): every
// alert kind scored each night (GET /api/scorecard). Self-contained: it reads the token from the
// keystore unless one is passed, fetches, and handles loading, empty and error. No ScrollView of
// its own: the Insights screen scrolls, so this never nests one scroller in another.

type Load = { state: "loading" } | { state: "error" } | { state: "ready"; days: ScorecardDay[] };

function useScorecard(days: number, token: string | null | undefined): { load: Load; retry: () => void } {
  const [attempt, setAttempt] = useState(0);
  const key = `${days}:${attempt}:${token ?? ""}`;
  const [got, setGot] = useState<{ key: string; load: Load } | null>(null);
  useEffect(() => {
    let live = true;
    void (token !== undefined ? Promise.resolve(token) : getAccessToken().catch(() => null))
      .then((t) => authenticatedFetch(`${SERVER_URL}/api/scorecard?days=${days}`, t))
      .then((res) => (res.ok ? res.json() : Promise.reject(new Error(String(res.status)))))
      .then((body: unknown) => {
        if (live) setGot({ key, load: { state: "ready", days: parseScorecard(body) } });
      })
      .catch(() => {
        if (live) setGot({ key, load: { state: "error" } });
      });
    return () => {
      live = false;
    };
  }, [days, token, key]);
  return { load: got && got.key === key ? got.load : { state: "loading" }, retry: useCallback(() => setAttempt((n) => n + 1), []) };
}

const dayLabel = (iso: string) => new Date(`${iso}T12:00:00Z`).toLocaleDateString("en-US", { weekday: "short", month: "short", day: "numeric", timeZone: "UTC" });

export function ScorecardView({ days = 30, token }: { days?: number; token?: string | null }) {
  const { load, retry } = useScorecard(days, token);
  if (load.state === "loading") return <Text style={styles.faint} accessibilityRole="text">Loading the scorecard…</Text>;
  if (load.state === "error") {
    return (
      <View style={styles.stack} accessibilityRole="alert">
        <Text style={styles.body}>The scorecard could not be loaded.</Text>
        <Pressable onPress={retry} accessibilityRole="button" style={({ pressed }) => [styles.button, pressed && styles.pressed]}>
          <Text style={styles.buttonText}>Try again</Text>
        </Pressable>
      </View>
    );
  }
  return <ScorecardBody days={load.days} />;
}

function ScorecardBody({ days }: { days: readonly ScorecardDay[] }) {
  const rows = useMemo(() => scorecardRows(days), [days]);
  const latest = days[0];
  if (!latest || rows.length === 0) {
    return (
      <View style={styles.stack}>
        <Text style={styles.strong}>Nothing scored yet</Text>
        <Text style={styles.faint}>Each trading day is scored after the nightly import (around 9:30 PM ET), once the full tape for the day is in.</Text>
      </View>
    );
  }
  const r = latest.rule;
  return (
    <View style={styles.stack}>
      <Text style={styles.faint}>
        Latest scored day {dayLabel(latest.date)} · follow-through = +{r.target_pct}% before −{r.stop_pct}% within {r.window_min} min · tap a card for details
      </Text>
      {rows.map((row) => (
        <KindCard key={row.kind} row={row} />
      ))}
    </View>
  );
}

function KindCard({ row }: { row: ScorecardRow }) {
  const [open, setOpen] = useState(false);
  const [width, setWidth] = useState(0);
  const k = row.latest;
  const onLayout = (e: LayoutChangeEvent) => setWidth(e.nativeEvent.layout.width);
  // The trend takes about a third of the card, between 72 and 140 dp, so it fits the Fold's cover
  // screen and does not sprawl unfolded.
  const tw = Math.max(72, Math.min(140, Math.round(width * 0.34)));
  const label = `${row.label}: ${k ? `${pct(k.rate)} followed through on the latest day` : "did not fire on the latest day"}; ${pct(row.pooled.rate)} over the period`;
  return (
    <Pressable onLayout={onLayout} onPress={() => k && setOpen((o) => !o)} accessibilityRole="button" accessibilityLabel={label} accessibilityState={{ expanded: open }} style={({ pressed }) => [styles.card, pressed && styles.pressed]}>
      <View style={styles.row}>
        <View style={styles.grow}>
          <Text style={styles.strong} numberOfLines={1}>
            {row.label}
          </Text>
          <Text style={styles.rate}>{pct(k?.rate ?? null)}</Text>
          <Text style={styles.faint}>follow-through, latest day</Text>
        </View>
        <View style={styles.trend}>
          <Trend values={row.trend} w={tw} h={32} />
          <Text style={styles.mono}>{pct(row.pooled.rate)} pooled</Text>
        </View>
      </View>
      {k ? <Text style={styles.dim}>{summaryLine(k)}</Text> : <Text style={styles.faint}>Did not fire on the latest day.</Text>}
      {k?.batch.fired ? <Text style={styles.warning}>{k.batch.fired} on the 04:00 report batch</Text> : null}
      {open && k ? (
        <View style={styles.detail}>
          {detailLines(k).map((l) => (
            <Text key={l} style={styles.dim}>
              {l}
            </Text>
          ))}
        </View>
      ) : null}
    </Pressable>
  );
}

function Trend({ values, w, h }: { values: readonly (number | null)[]; w: number; h: number }) {
  const runs = trendRuns(values, w, h);
  const n = values.length;
  const last = n ? values[n - 1] : null;
  const y = (v: number) => h - 3 - v * (h - 6);
  return (
    <Svg width={w} height={h} accessibilityElementsHidden importantForAccessibility="no-hide-descendants">
      <Line x1={0} x2={w} y1={y(0.5)} y2={y(0.5)} stroke={colors.textDim} strokeOpacity={0.25} strokeDasharray="2 3" />
      {runs.map((pts, i) => {
        if (pts.includes(" ")) return <Polyline key={i} points={pts} fill="none" stroke={colors.textDim} strokeWidth={1.5} strokeLinejoin="round" />;
        const [cx, cy] = pts.split(",");
        return <Circle key={i} cx={cx} cy={cy} r={1.5} fill={colors.textDim} />;
      })}
      {last !== null && last !== undefined ? <Circle cx={n <= 1 ? w / 2 : w - 2} cy={y(last)} r={2.5} fill={colors.text} /> : null}
    </Svg>
  );
}

const styles = StyleSheet.create({
  stack: { gap: space.sm },
  card: { backgroundColor: colors.bgPanel, borderRadius: radius.lg + 4, padding: space.md, gap: 6, minHeight: 44 },
  pressed: { backgroundColor: colors.bgRow },
  row: { flexDirection: "row", alignItems: "flex-start", gap: space.md },
  grow: { flex: 1, minWidth: 0, gap: 2 },
  trend: { alignItems: "flex-end", gap: 2 },
  detail: { gap: 4, paddingTop: 4 },
  body: { color: colors.text, fontFamily: fonts.regular, fontSize: 14 },
  strong: { color: colors.text, fontFamily: fonts.semibold, fontSize: 14 },
  rate: { color: colors.text, fontFamily: fonts.semibold, fontSize: 24, fontVariant: ["tabular-nums"] },
  dim: { color: colors.textDim, fontFamily: fonts.regular, fontSize: 12, lineHeight: 17 },
  faint: { color: colors.textFaint, fontFamily: fonts.regular, fontSize: 12, lineHeight: 17 },
  mono: { color: colors.textFaint, fontSize: 11, fontVariant: ["tabular-nums"] },
  warning: { color: colors.warning, fontFamily: fonts.regular, fontSize: 12 },
  button: { alignSelf: "flex-start", minHeight: 44, justifyContent: "center", paddingHorizontal: space.lg, borderRadius: radius.lg, backgroundColor: colors.bgRow },
  buttonText: { color: colors.text, fontFamily: fonts.semibold, fontSize: 14 },
});

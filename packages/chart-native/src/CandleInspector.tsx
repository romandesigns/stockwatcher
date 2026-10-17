import { describeSlot, type Slot, type Timeframe } from "@stockwatcher/chart-core";
import { StyleSheet, Text, View, type StyleProp, type ViewStyle } from "react-native";
import { UI } from "./ui.tsx";

const CHANGE = { up: UI.good, down: UI.critical, flat: UI.textDim } as const;

/**
 * The inspected candle on one line, the phone's twin of chart-web's
 * CandleInspector: `09:42  O 3.19  H 3.19  L 3.16  C 3.18  −0.31%`, faint
 * labels, coloured change, "Live" while the candle is forming. Screen
 * readers get the full sentence, not the abbreviations.
 */
export function CandleInspector({ slot, timeframe, style }: { slot: Slot | null | undefined; timeframe: Timeframe; style?: StyleProp<ViewStyle> }) {
  if (!slot) return <Text style={[s.faint, s.line]}>Waiting for data…</Text>;
  const info = describeSlot(slot, timeframe);
  return (
    <View style={[s.row, style]} accessible accessibilityLabel={info.summary}>
      <Text style={[s.mono, s.dim]}>{info.time}</Text>
      {info.kind === "gap" ? (
        <Text style={s.faint} numberOfLines={1}>{`No candle: ${info.note}`}</Text>
      ) : (
        <>
          {info.ohlc.map((r) => (
            <Text key={r.k} style={s.mono}>
              <Text style={s.faint}>{r.k} </Text>
              {r.v}
            </Text>
          ))}
          {info.change && <Text style={[s.mono, { color: CHANGE[info.change.dir] }]}>{info.change.text}</Text>}
          {info.forming && (
            <View style={s.live}>
              <View style={s.dot} />
              <Text style={s.liveText}>Live</Text>
            </View>
          )}
        </>
      )}
    </View>
  );
}

const s = StyleSheet.create({
  row: { flexDirection: "row", alignItems: "baseline", gap: 9, flexWrap: "wrap" },
  line: { fontSize: 11 },
  mono: { color: UI.text, fontSize: 11, fontVariant: ["tabular-nums"] },
  dim: { color: UI.textDim },
  faint: { color: UI.textFaint, fontSize: 11 },
  live: { flexDirection: "row", alignItems: "center", gap: 4, alignSelf: "center" },
  dot: { width: 6, height: 6, borderRadius: 3, backgroundColor: UI.warning },
  liveText: { color: UI.warning, fontSize: 10, fontWeight: "500" },
});

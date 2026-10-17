import { flipPosition, formatPrice, formatSize, positionPlan, type Drawing } from "@stockwatcher/chart-core";
import { useEffect, useState } from "react";
import { StyleSheet, Text, TextInput, View, type StyleProp, type ViewStyle } from "react-native";
import Animated, { FadeInDown, FadeOutDown, ReduceMotion } from "react-native-reanimated";
import { Segmented, ToolButton, UI } from "./ui.tsx";

/**
 * The phone's position plan, docked under the chart while a position tool is
 * selected (design canvas https://claude.ai/artifact/UkBuQdEMyZjAYmdMsTzatW,
 * "Phone · plan a long"): side, risk budget, reward / risk / R:R, and Place
 * bracket. Edits write straight back into the drawing, like chart-web's
 * PositionPlanCard.
 */
export function PositionPlanPanel({ drawing, onChange, onPlace, style }: { drawing: Drawing; onChange: (d: Drawing) => void; onPlace?: () => void; style?: StyleProp<ViewStyle> }) {
  const plan = positionPlan(drawing);
  const [risk, setRisk] = useState(String(plan.riskBudget));
  useEffect(() => setRisk(String(plan.riskBudget)), [plan.riskBudget]);
  const long = plan.side === "long";
  return (
    // Slides up from the chart's bottom edge and back; with reduced motion on, Reanimated skips it.
    <Animated.View entering={FadeInDown.duration(180).reduceMotion(ReduceMotion.System)} exiting={FadeOutDown.duration(140).reduceMotion(ReduceMotion.System)} style={[s.panel, style]} accessibilityLabel="Position plan">
      <View style={s.head}>
        <Text style={s.title}>{`${long ? "Long" : "Short"} · ${formatSize(plan.qty)} sh @ ${formatPrice(plan.entry)}`}</Text>
        <Text style={s.sim}>SIMULATED</Text>
      </View>
      <View style={s.row}>
        <Segmented
          value={plan.side}
          onChange={(v) => v !== plan.side && onChange(flipPosition(drawing))}
          options={[
            { value: "long", label: "Long", a11y: "Long", tint: UI.good },
            { value: "short", label: "Short", a11y: "Short", tint: UI.critical },
          ]}
        />
        <View style={s.riskBox}>
          <Text style={s.riskLabel}>Risk $</Text>
          <TextInput
            style={s.riskInput}
            keyboardType="decimal-pad"
            value={risk}
            accessibilityLabel="Risk per trade in dollars"
            selectionColor={UI.accent}
            onChangeText={(t) => {
              const v = t.replace(/[^\d.]/g, "");
              setRisk(v);
              const n = Number(v);
              if (Number.isFinite(n) && n > 0) onChange({ ...drawing, risk: n, qty: null });
            }}
          />
        </View>
      </View>
      <View style={s.stats}>
        <Stat label="Reward" value={`+$${plan.reward.toFixed(0)}`} color={UI.good} />
        <Stat label="Risk" value={`−$${plan.risk.toFixed(0)}`} color={UI.critical} />
        <Stat label="R:R" value={plan.rr.toFixed(2)} color={UI.text} />
      </View>
      {onPlace ? (
        <ToolButton primary label="Place simulated bracket order" disabled={!plan.valid} onPress={onPlace} style={s.place}>
          Place bracket
        </ToolButton>
      ) : (
        <Text style={s.note}>Planning only: trading is off on this chart.</Text>
      )}
    </Animated.View>
  );
}

function Stat({ label, value, color }: { label: string; value: string; color: string }) {
  return (
    <View style={s.stat}>
      <Text style={s.statLabel}>{label}</Text>
      <Text style={[s.statValue, { color }]}>{value}</Text>
    </View>
  );
}

const s = StyleSheet.create({
  panel: { backgroundColor: UI.panel, borderRadius: 12, padding: 10, gap: 8 },
  head: { flexDirection: "row", alignItems: "baseline", justifyContent: "space-between" },
  title: { color: UI.text, fontSize: 13, fontWeight: "600", fontVariant: ["tabular-nums"] },
  sim: { color: UI.warning, fontSize: 10, fontWeight: "500", letterSpacing: 0.4 },
  row: { flexDirection: "row", alignItems: "center", gap: 8 },
  riskBox: { flex: 1, flexDirection: "row", alignItems: "center", gap: 6, backgroundColor: UI.row, borderRadius: 8, paddingHorizontal: 10, height: 34 },
  riskLabel: { color: UI.textDim, fontSize: 12 },
  riskInput: { flex: 1, color: UI.text, fontSize: 14, padding: 0, fontVariant: ["tabular-nums"] },
  stats: { flexDirection: "row", gap: 6 },
  stat: { flex: 1, backgroundColor: UI.row, borderRadius: 8, padding: 8, gap: 2 },
  statLabel: { color: UI.textDim, fontSize: 10 },
  statValue: { fontSize: 14, fontWeight: "500", fontVariant: ["tabular-nums"] },
  place: { height: 44, borderRadius: 10 },
  note: { color: UI.textFaint, fontSize: 11 },
});

import { formatPrice, stepTick } from "@stockwatcher/chart-core";
import { useState } from "react";
import { Pressable, StyleSheet, Text, View } from "react-native";
import { Sheet } from "./sheets.tsx";
import { playHaptic, UI } from "./ui.tsx";

export interface QuickAlertDraft {
  price: number;
  dir: "above" | "below";
}

/**
 * The phone's quick alert sheet (approved design, "2 · One tap to create"): arrives filled in
 * with the price pressed and its direction (above the last price = crosses above); − / + nudge
 * it one price tick (a haptic tick each); Create alert uses the defaults (Once, this week, in app
 * + phone); More options hands the draft to the host's full sheet. Keyed by its opening (the
 * chart remounts it per draft), so the nudged price starts from the pressed one.
 */
export function QuickAlertSheet({
  open,
  onClose,
  symbol,
  draft,
  last,
  onCreate,
  onMore,
  haptics = true,
  reducedMotion,
}: {
  open: boolean;
  onClose: () => void;
  symbol: string;
  draft: QuickAlertDraft;
  last: number | null;
  onCreate: (d: QuickAlertDraft) => void;
  onMore?: (d: QuickAlertDraft) => void;
  haptics?: boolean;
  reducedMotion?: boolean;
}) {
  const [price, setPrice] = useState(draft.price);
  const dir: "above" | "below" = last === null ? draft.dir : price >= last ? "above" : "below";
  const pct = last ? ((price - last) / last) * 100 : null;
  const nudge = (d: 1 | -1) => {
    setPrice((p) => stepTick(p, d));
    if (haptics) playHaptic("selection");
  };
  return (
    <Sheet open={open} onClose={onClose} title={`Alert when ${symbol} crosses ${dir}`} reducedMotion={reducedMotion}>
      <View style={{ gap: 14 }}>
        <View style={st.stepper}>
          <Pressable accessibilityRole="button" accessibilityLabel="Lower by one tick" onPress={() => nudge(-1)} hitSlop={6} style={({ pressed }) => [st.step, pressed && { backgroundColor: UI.raised }]}>
            <Text style={st.stepText}>−</Text>
          </Pressable>
          <View style={st.priceBox} accessible accessibilityLabel={`${formatPrice(price)}${pct === null ? "" : `, ${pct.toFixed(1)} percent from the last price`}`}>
            <Text style={st.price}>{formatPrice(price)}</Text>
            {pct !== null && <Text style={[st.pct, { color: pct >= 0 ? UI.good : UI.critical }]}>{`${pct >= 0 ? "+" : "−"}${Math.abs(pct).toFixed(1)}%`}</Text>}
          </View>
          <Pressable accessibilityRole="button" accessibilityLabel="Raise by one tick" onPress={() => nudge(1)} hitSlop={6} style={({ pressed }) => [st.step, pressed && { backgroundColor: UI.raised }]}>
            <Text style={st.stepText}>+</Text>
          </Pressable>
        </View>
        <Text style={st.meta}>Once · this week · in app + phone</Text>
        <Pressable
          accessibilityRole="button"
          onPress={() => {
            onCreate({ price, dir });
            if (haptics) playHaptic("medium");
            onClose();
          }}
          style={({ pressed }) => [st.cta, pressed && { opacity: 0.85 }]}
        >
          <Text style={st.ctaText}>Create alert</Text>
        </Pressable>
        {onMore && (
          <Pressable
            accessibilityRole="button"
            onPress={() => {
              onClose();
              // After the sheet's own close, so two modals never animate at once.
              setTimeout(() => onMore({ price, dir }), 0);
            }}
            style={({ pressed }) => [st.more, pressed && { backgroundColor: UI.raised }]}
          >
            <Text style={st.moreText}>More options</Text>
          </Pressable>
        )}
      </View>
    </Sheet>
  );
}

const st = StyleSheet.create({
  stepper: { flexDirection: "row", alignItems: "center", gap: 10 },
  // 48 pt targets: the design's −/+ for a thumb.
  step: { width: 48, height: 48, borderRadius: 12, backgroundColor: UI.row, alignItems: "center", justifyContent: "center" },
  stepText: { color: UI.text, fontSize: 22, fontWeight: "500" },
  priceBox: { flex: 1, height: 48, borderRadius: 12, backgroundColor: UI.bg, flexDirection: "row", alignItems: "baseline", justifyContent: "center", gap: 8, paddingTop: 10 },
  price: { color: UI.text, fontSize: 22, fontWeight: "600", fontVariant: ["tabular-nums"] },
  pct: { fontSize: 13, fontVariant: ["tabular-nums"] },
  meta: { color: UI.textDim, fontSize: 13, textAlign: "center" },
  cta: { height: 48, borderRadius: 12, backgroundColor: UI.accent, alignItems: "center", justifyContent: "center" },
  ctaText: { color: UI.accentFg, fontSize: 15, fontWeight: "600" },
  more: { height: 44, borderRadius: 12, backgroundColor: UI.row, alignItems: "center", justifyContent: "center" },
  moreText: { color: UI.text, fontSize: 14, fontWeight: "500" },
});

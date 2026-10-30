import { brokerLabel, brokerOf, buyingPower, freshNotice, type PaperState } from "@stockwatcher/chart-core";
import { useEffect, useState } from "react";
import { Pressable, StyleSheet, Text, View } from "react-native";
import { Icon, UI } from "./ui.tsx";

// chart-web's AccountBadge / BuyingPower / BrokerNotice on the phone (same words, same tones):
// "Simulator" in warning tones, "Alpaca paper" in good tones while its trade stream is live.

const tint = (hex: string, a: number) => {
  const n = parseInt(hex.slice(1), 16);
  return `rgba(${(n >> 16) & 255},${(n >> 8) & 255},${n & 255},${a})`;
};

export function AccountBadge({ state }: { state: PaperState | null | undefined }) {
  const broker = brokerOf(state);
  const alpaca = broker === "alpaca_paper";
  const down = alpaca && state?.link !== undefined && state.link.state !== "live";
  const color = alpaca && !down ? UI.good : UI.warning;
  return (
    <View
      accessible
      accessibilityLabel={alpaca ? `Alpaca paper account${down ? ", offline" : ""}` : "Simulator, simulated fills"}
      style={[s.badge, { backgroundColor: tint(color, 0.14) }]}
    >
      <Icon name={alpaca ? "trade" : "gauge"} color={color} size={11} />
      <Text style={[s.badgeText, { color }]}>{`${brokerLabel(broker).toUpperCase()}${down ? " · OFFLINE" : ""}`}</Text>
    </View>
  );
}

export function BuyingPower({ state }: { state: PaperState | null | undefined }) {
  const bp = buyingPower(state);
  if (bp === null) return null;
  return (
    <View style={s.row}>
      <Text style={s.small}>Buying power</Text>
      <Text style={[s.small, { color: UI.textDim, fontVariant: ["tabular-nums"] }]}>{`$${bp.toLocaleString("en-US", { minimumFractionDigits: 2, maximumFractionDigits: 2 })}`}</Text>
    </View>
  );
}

/** Alpaca paper's latest refusal, shown once for a few seconds; tap to dismiss. */
export function BrokerNotice({ state }: { state: PaperState | null | undefined }) {
  const [seenAt, setSeenAt] = useState(() => state?.notice?.at ?? 0);
  const notice = freshNotice(state, seenAt, Date.now());
  useEffect(() => {
    if (!notice) return;
    const t = setTimeout(() => setSeenAt(notice.at), 6000);
    return () => clearTimeout(t);
  }, [notice]);
  if (!notice) return null;
  return (
    <Pressable accessibilityRole="alert" accessibilityHint="Tap to dismiss" onPress={() => setSeenAt(notice.at)} style={s.notice}>
      <Icon name="bell" color={UI.critical} size={14} />
      <Text style={s.noticeText}>{notice.text}</Text>
    </Pressable>
  );
}

const s = StyleSheet.create({
  badge: { flexDirection: "row", alignItems: "center", gap: 4, borderRadius: 4, paddingHorizontal: 6, paddingVertical: 2 },
  badgeText: { fontSize: 9, fontWeight: "600", letterSpacing: 0.6 },
  row: { flexDirection: "row", justifyContent: "space-between", alignItems: "baseline" },
  small: { color: UI.textFaint, fontSize: 12 },
  notice: {
    position: "absolute",
    left: 12,
    right: 12,
    bottom: 12,
    minHeight: 44,
    flexDirection: "row",
    alignItems: "center",
    gap: 8,
    borderRadius: 10,
    paddingHorizontal: 12,
    paddingVertical: 10,
    backgroundColor: UI.raised,
  },
  noticeText: { flex: 1, color: UI.text, fontSize: 13 },
});

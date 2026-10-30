import { alpacaUnavailable, type BrokerStatus, type LiveAccount } from "@stockwatcher/app-state";
import type { IconName } from "@stockwatcher/chart-core";
import { Icon, playHaptic, UI } from "@stockwatcher/chart-native";
import { Pressable, StyleSheet, Text, View } from "react-native";

const OPTIONS: readonly { value: LiveAccount; icon: IconName; name: string; description: string }[] = [
  {
    value: "simulator",
    icon: "gauge",
    name: "Simulator",
    description: "Orders fill on our own server against the live tape. Nothing leaves the app.",
  },
  {
    value: "alpaca_paper",
    icon: "trade",
    name: "Alpaca paper",
    description: "Orders go to your Alpaca paper account: paper money, real market rules. Outside 9:30–16:00 ET only limit orders are accepted.",
  },
];

function statusLine(st: BrokerStatus): { text: string; color: string } | null {
  if (alpacaUnavailable(st)) return { text: `Not configured. ${st.message}`, color: UI.warning };
  if (st.state === "live") return { text: "Connected", color: UI.good };
  if (st.state === "connecting" || st.state === "down") return { text: st.message || "Connecting…", color: UI.textFaint };
  return null;
}

/**
 * Settings → Trading (the web's TradingAccountPanel): which account the LIVE market's chart orders
 * go to. Replay always uses the simulator. The chosen row spends the accent on its icon and check.
 */
export function TradingAccountPanel({ account, status, onChange }: { account: LiveAccount; status: BrokerStatus; onChange: (a: LiveAccount) => void }) {
  return (
    <View accessibilityRole="radiogroup" style={s.list}>
      {OPTIONS.map((o) => {
        const on = o.value === account;
        const line = o.value === "alpaca_paper" ? statusLine(status) : null;
        return (
          <Pressable
            key={o.value}
            accessibilityRole="radio"
            accessibilityState={{ checked: on }}
            accessibilityLabel={`${o.name}. ${o.description}${line ? `. ${line.text}` : ""}`}
            onPress={() => {
              playHaptic("selection");
              onChange(o.value);
            }}
            style={({ pressed }) => [s.row, (pressed || on) && { backgroundColor: UI.row }]}
          >
            <Icon name={o.icon} color={on ? UI.accent : UI.textDim} size={18} />
            <View style={s.text}>
              <Text style={s.name}>{o.name}</Text>
              <Text style={s.description}>{o.description}</Text>
              {line && <Text style={[s.description, { color: line.color }]}>{line.text}</Text>}
            </View>
            <View style={{ opacity: on ? 1 : 0 }}>
              <Icon name="check" color={UI.accent} size={14} />
            </View>
          </Pressable>
        );
      })}
      <Text style={[s.description, s.foot]}>Applies to the live market on every device. Replay practice always uses the simulator.</Text>
    </View>
  );
}

const s = StyleSheet.create({
  list: { gap: 4 },
  row: { minHeight: 56, flexDirection: "row", alignItems: "flex-start", gap: 12, paddingHorizontal: 12, paddingVertical: 10, borderRadius: 10 },
  text: { flex: 1, gap: 2 },
  name: { color: UI.text, fontSize: 15, fontWeight: "500" },
  description: { color: UI.textFaint, fontSize: 12, lineHeight: 16 },
  foot: { paddingHorizontal: 12, paddingTop: 4 },
});

import {
  canAddIndicator,
  formatPrice,
  INDICATOR_PLACEMENT,
  indicatorLabel,
  INDICATOR_PRESETS,
  orderTypeAt,
  snapPrice,
  toggleIndicator,
  type IndicatorSpec,
  type OrderPlace,
  type OrderSide,
  type OrderType,
  type PriceScaleType,
} from "@stockwatcher/chart-core";
import { useEffect, useState, type ReactNode } from "react";
import { Modal, Pressable, ScrollView, StyleSheet, Text, TextInput, View } from "react-native";
import { Segmented, ToolButton, UI } from "./ui.tsx";

/**
 * A bottom sheet on a dimmed backdrop. Tapping the backdrop or the system
 * back closes it. RN's Modal "slide" is the same motion on both platforms;
 * with reduced motion the host passes reducedMotion and it only fades.
 */
export function Sheet({ open, onClose, title, children, reducedMotion }: { open: boolean; onClose: () => void; title: string; children: ReactNode; reducedMotion?: boolean }) {
  return (
    <Modal visible={open} transparent animationType={reducedMotion ? "fade" : "slide"} onRequestClose={onClose} statusBarTranslucent navigationBarTranslucent>
      <Pressable style={s.backdrop} onPress={onClose} accessibilityLabel="Close" accessibilityRole="button" />
      <View style={s.sheet} accessibilityViewIsModal>
        <View style={s.grabber} />
        <Text style={s.title} accessibilityRole="header">
          {title}
        </Text>
        {children}
      </View>
    </Modal>
  );
}

function Row({ label, on, disabled, onPress, right }: { label: string; on?: boolean; disabled?: boolean; onPress: () => void; right?: string }) {
  return (
    <Pressable
      accessibilityRole={on === undefined ? "button" : "checkbox"}
      accessibilityState={on === undefined ? { disabled: !!disabled } : { checked: on, disabled: !!disabled }}
      disabled={disabled}
      onPress={onPress}
      style={({ pressed }) => [s.row, pressed && { backgroundColor: UI.row }, disabled && { opacity: 0.4 }]}
    >
      <Text style={s.rowText}>{label}</Text>
      {right !== undefined && <Text style={s.rowRight}>{right}</Text>}
      {on && <Text style={[s.rowRight, { color: UI.accent }]}>✓</Text>}
    </Pressable>
  );
}

export function IndicatorSheet({ open, onClose, indicators, onChange, reducedMotion }: { open: boolean; onClose: () => void; indicators: readonly IndicatorSpec[]; onChange: (s: IndicatorSpec[]) => void; reducedMotion?: boolean }) {
  const on = new Set(indicators.map((x) => x.id));
  const group = (placement: "overlay" | "pane") => INDICATOR_PRESETS.filter((p) => INDICATOR_PLACEMENT[p.kind] === placement);
  return (
    <Sheet open={open} onClose={onClose} title="Indicators" reducedMotion={reducedMotion}>
      <ScrollView>
        <Text style={s.label}>On the chart · up to 3</Text>
        {group("overlay").map((p) => (
          <Row key={p.id} label={indicatorLabel(p)} on={on.has(p.id)} disabled={!canAddIndicator(indicators, p)} onPress={() => onChange(toggleIndicator(indicators, p))} />
        ))}
        <Text style={s.label}>Below · up to 2</Text>
        {group("pane").map((p) => (
          <Row key={p.id} label={indicatorLabel(p)} on={on.has(p.id)} disabled={!canAddIndicator(indicators, p)} onPress={() => onChange(toggleIndicator(indicators, p))} />
        ))}
      </ScrollView>
    </Sheet>
  );
}

export const SCALE_LABELS: Readonly<Record<PriceScaleType, string>> = { linear: "Linear (price)", percent: "Linear (%)", log: "Logarithmic" };

/** The price axis menu (long press on the axis): auto-scale, scale type, and an order at the pressed price. */
export function AxisSheet({
  open,
  onClose,
  price,
  autoScaled,
  onAutoScale,
  scaleType,
  onScaleType,
  onOrderAt,
  reducedMotion,
}: {
  open: boolean;
  onClose: () => void;
  price: number | null;
  autoScaled: boolean;
  onAutoScale: () => void;
  scaleType: PriceScaleType;
  onScaleType: (t: PriceScaleType) => void;
  onOrderAt?: () => void;
  reducedMotion?: boolean;
}) {
  const close = (f: () => void) => () => {
    f();
    onClose();
  };
  return (
    <Sheet open={open} onClose={onClose} title={price === null ? "Price axis" : `Price axis · ${formatPrice(price)}`} reducedMotion={reducedMotion}>
      {onOrderAt && <Row label="Order at this price…" onPress={close(onOrderAt)} />}
      <Row label="Auto-scale price axis" right={autoScaled ? "on" : ""} onPress={close(onAutoScale)} />
      <Text style={s.label}>Scale</Text>
      {(Object.keys(SCALE_LABELS) as PriceScaleType[]).map((t) => (
        <Row key={t} label={SCALE_LABELS[t]} on={scaleType === t} onPress={close(() => onScaleType(t))} />
      ))}
    </Sheet>
  );
}

export type TicketOrder = Omit<OrderPlace, "type" | "clientOrderId">;

/** The simulated order ticket (see chart-web's OrderTicket for the rules; same behaviour). */
export function OrderTicketSheet({
  open,
  onClose,
  symbol,
  last,
  price,
  onSubmit,
  reducedMotion,
}: {
  open: boolean;
  onClose: () => void;
  symbol: string;
  last: number | null;
  price: number | null;
  onSubmit: (o: TicketOrder) => void;
  reducedMotion?: boolean;
}) {
  const [side, setSide] = useState<OrderSide>("buy");
  const [type, setType] = useState<Exclude<OrderType, "stop_limit">>("market");
  const [byHand, setByHand] = useState(false);
  const [qty, setQty] = useState("100");
  const [px, setPx] = useState("");

  useEffect(() => {
    if (!open) return;
    setSide("buy");
    setByHand(false);
    if (price !== null && last !== null) {
      setType(orderTypeAt("buy", price, last));
      setPx(formatPrice(snapPrice(price)));
    } else {
      setType("market");
      setPx(last !== null ? formatPrice(snapPrice(last)) : "");
    }
  }, [open, price, last]);

  const n = Number(qty);
  const qtyOk = Number.isInteger(n) && n > 0 && n <= 1_000_000;
  const p = Number(px);
  const priceOk = type === "market" || (Number.isFinite(p) && p > 0);
  const snapped = priceOk && type !== "market" ? snapPrice(p) : null;
  const est = type === "market" ? last : snapped;

  const changeSide = (v: OrderSide) => {
    setSide(v);
    if (!byHand && type !== "market" && last !== null && Number.isFinite(p)) setType(orderTypeAt(v, p, last));
  };
  const submit = () => {
    if (!qtyOk || !priceOk) return;
    const o: TicketOrder = { symbol, side, orderType: type, qty: n };
    if (type === "limit" && snapped !== null) o.limitPrice = snapped;
    if (type === "stop" && snapped !== null) o.stopPrice = snapped;
    onSubmit(o);
    onClose();
  };

  return (
    <Sheet open={open} onClose={onClose} title={`${symbol} · simulated order`} reducedMotion={reducedMotion}>
      <View style={{ gap: 10 }}>
        <Segmented
          fill
          value={side}
          onChange={changeSide}
          options={[
            { value: "buy", label: "Buy", a11y: "Buy", tint: UI.good },
            { value: "sell", label: "Sell", a11y: "Sell", tint: UI.critical },
          ]}
        />
        <Segmented
          fill
          value={type}
          onChange={(v) => {
            setType(v);
            setByHand(true);
          }}
          options={[
            { value: "market", label: "Market", a11y: "Market order" },
            { value: "limit", label: "Limit", a11y: "Limit order" },
            { value: "stop", label: "Stop", a11y: "Stop order" },
          ]}
        />
        <Text style={s.label}>Shares</Text>
        <TextInput style={s.input} keyboardType="number-pad" value={qty} onChangeText={(t) => setQty(t.replace(/[^\d]/g, ""))} accessibilityLabel="Shares" selectionColor={UI.accent} />
        <View style={{ flexDirection: "row", gap: 6 }}>
          {[100, 500, 1000].map((q) => (
            <ToolButton key={q} label={`${q} shares`} onPress={() => setQty(String(q))} style={{ flex: 1, backgroundColor: UI.row }}>
              {q.toLocaleString("en-US")}
            </ToolButton>
          ))}
        </View>
        {type !== "market" && (
          <>
            <Text style={s.label}>{type === "limit" ? "Limit price" : "Stop price"}</Text>
            <TextInput
              style={s.input}
              keyboardType="decimal-pad"
              value={px}
              onChangeText={(t) => setPx(t.replace(/[^\d.]/g, ""))}
              onBlur={() => Number.isFinite(p) && p > 0 && setPx(formatPrice(snapPrice(p)))}
              accessibilityLabel={type === "limit" ? "Limit price" : "Stop price"}
              selectionColor={UI.accent}
            />
          </>
        )}
        <View style={{ flexDirection: "row", justifyContent: "space-between" }}>
          <Text style={s.small}>{type === "market" ? "Est. at last" : "Value"}</Text>
          <Text style={[s.small, { color: UI.text, fontVariant: ["tabular-nums"] }]}>{est !== null && qtyOk ? `$${(est * n).toFixed(2)}` : "—"}</Text>
        </View>
        <ToolButton primary label="Place simulated order" disabled={!qtyOk || !priceOk} onPress={submit} style={{ height: 40 }}>
          {`${side === "buy" ? "Buy" : "Sell"} ${qtyOk ? n.toLocaleString("en-US") : ""} ${type === "market" ? "at market" : `${type} ${snapped !== null ? formatPrice(snapped) : ""}`}`}
        </ToolButton>
        <Text style={s.note}>Simulated: fills in the simulator against the replay tape, in full, when a trade prints through the price. No money, no broker.</Text>
      </View>
    </Sheet>
  );
}

const s = StyleSheet.create({
  backdrop: { flex: 1, backgroundColor: "rgba(0,0,0,0.45)" },
  sheet: { backgroundColor: UI.panel, borderTopLeftRadius: 16, borderTopRightRadius: 16, paddingHorizontal: 16, paddingTop: 8, paddingBottom: 28, maxHeight: "75%" },
  grabber: { alignSelf: "center", width: 36, height: 4, borderRadius: 2, backgroundColor: UI.raised, marginBottom: 10 },
  title: { color: UI.text, fontSize: 15, fontWeight: "600", marginBottom: 8 },
  label: { color: UI.textFaint, fontSize: 11, marginTop: 8, marginBottom: 2 },
  row: { minHeight: 44, flexDirection: "row", alignItems: "center", paddingHorizontal: 8, borderRadius: 8 },
  rowText: { flex: 1, color: UI.text, fontSize: 14 },
  rowRight: { color: UI.textDim, fontSize: 13, marginLeft: 8 },
  input: { height: 40, backgroundColor: UI.row, borderRadius: 8, paddingHorizontal: 10, color: UI.text, fontSize: 15, fontVariant: ["tabular-nums"] },
  small: { color: UI.textDim, fontSize: 12 },
  note: { color: UI.textFaint, fontSize: 11, lineHeight: 15 },
});

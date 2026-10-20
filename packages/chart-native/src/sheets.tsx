import {
  formatEtTime,
  formatSize,
  LAYER_IDS,
  LAYER_INFO,
  LAYER_SWATCHES,
  type LayerId,
  type ChartLayers,
  type FillMark,
  canAddIndicator,
  DEFAULT_CHART_THEME,
  INDICATOR_GLYPHS,
  indicatorInfo,
  MAX_OVERLAYS,
  MAX_PANES,
  formatPrice,
  INDICATOR_PLACEMENT,
  INDICATOR_PRESETS,
  orderTypeAt,
  priceDecimals,
  snapPrice,
  toggleIndicator,
  TRADE_SIZES,
  type IndicatorSpec,
  type OrderPlace,
  type OrderSide,
  type OrderType,
  type PriceScaleType,
  type ProfileWire,
} from "@stockwatcher/chart-core";
import { useEffect, useState, type ReactNode } from "react";
import { Modal, Pressable, ScrollView, StyleSheet, Text, TextInput, View } from "react-native";
import { useSafeAreaInsets } from "react-native-safe-area-context";
import { Icon, Segmented, ToolButton, UI } from "./ui.tsx";

/**
 * A bottom sheet on a dimmed backdrop. Tapping the backdrop or the system
 * back closes it. RN's Modal "slide" is the same motion on both platforms;
 * with reduced motion the host passes reducedMotion and it only fades.
 * `tall` makes it a full-height sheet (the Settings sheet): a fixed 92 %
 * height, so the backdrop still shows above it and it still reads as a
 * sheet you can dismiss, and its children get the room to fill (flex: 1).
 * `onBack` takes over the Android hardware / gesture back (Modal's
 * onRequestClose) when back should mean less than "close": the Settings
 * sheet pops a section first. The backdrop tap still calls onClose.
 */
export function Sheet({ open, onClose, onBack, title, badge, children, reducedMotion, tall }: { open: boolean; onClose: () => void; onBack?: () => void; title: string; badge?: ReactNode; children: ReactNode; reducedMotion?: boolean; tall?: boolean }) {
  // The modal draws under Android's translucent navigation bar: pad by its real height, so the last row
  // is never under the gesture handle.
  const insets = useSafeAreaInsets();
  return (
    <Modal visible={open} transparent animationType={reducedMotion ? "fade" : "slide"} onRequestClose={onBack ?? onClose} statusBarTranslucent navigationBarTranslucent>
      <Pressable style={s.backdrop} onPress={onClose} accessibilityLabel="Close" accessibilityRole="button" />
      <View style={[s.sheet, tall && s.tall, { paddingBottom: Math.max(20, insets.bottom + 14) }]} accessibilityViewIsModal>
        <View style={s.grabber} />
        <View style={s.titleRow}>
          <Text style={s.title} accessibilityRole="header">
            {title}
          </Text>
          {badge}
        </View>
        {children}
      </View>
    </Modal>
  );
}

/**
 * A layer's swatch: its duotone icon (shared with chart-web through chart-core); accent while the
 * layer is on (owner: an active item's icon shows its accent duotone; status hues keep their own),
 * faded while it is off, as an off indicator's glyph is.
 */
function LayerSwatch({ id, on }: { id: LayerId; on: boolean }) {
  return (
    <View style={{ opacity: on ? 1 : 0.5 }}>
      <Icon def={LAYER_SWATCHES[id]} color={on ? UI.accent : UI.text} />
    </View>
  );
}

/** An indicator's glyph (shared with chart-web through chart-core), both tones tinted with its line colour. */
function Glyph({ kind, color }: { kind: IndicatorSpec["kind"]; color: string }) {
  return <Icon def={{ w: 16, h: 16, strokeWidth: 1.5, paths: INDICATOR_GLYPHS[kind] }} color={color} size={18} />;
}

/**
 * The selection check (accent, both tones): the sheets' one accent mark per
 * checked row. It always takes its room and only its opacity changes, so the
 * name and description do not reflow (or re-truncate) when a row toggles.
 */
const Check = ({ on }: { on: boolean }) => (
  <View style={[s.rowRight, { opacity: on ? 1 : 0 }]}>
    <Icon name="check" color={UI.accent} size={14} />
  </View>
);

/** A row's "opens more" affordance. */
const Chevron = () => <Icon name="chevronRight" color={UI.textDim} size={14} />;

/**
 * The indicator sheet, the phone's twin of chart-web's menu: per row the
 * glyph tile tinted with the line's colour, name, settings chip, one line on
 * what it measures and a check; section headers count against the caps.
 */
export function IndicatorSheet({
  open,
  onClose,
  indicators,
  onChange,
  seriesColors = [DEFAULT_CHART_THEME.series1, DEFAULT_CHART_THEME.series2, DEFAULT_CHART_THEME.series3],
  reducedMotion,
}: {
  open: boolean;
  onClose: () => void;
  indicators: readonly IndicatorSpec[];
  onChange: (s: IndicatorSpec[]) => void;
  seriesColors?: readonly string[];
  reducedMotion?: boolean;
}) {
  const on = new Set(indicators.map((x) => x.id));
  const active = (placement: "overlay" | "pane") => indicators.filter((x) => INDICATOR_PLACEMENT[x.kind] === placement);
  const group = (placement: "overlay" | "pane") => INDICATOR_PRESETS.filter((p) => INDICATOR_PLACEMENT[p.kind] === placement);
  const colorOf = (p: IndicatorSpec) => {
    if (INDICATOR_PLACEMENT[p.kind] === "pane") return seriesColors[0]!;
    const list = active("overlay");
    const i = list.findIndex((x) => x.id === p.id);
    return seriesColors[(i >= 0 ? i : list.length) % seriesColors.length]!;
  };
  const row = (p: IndicatorSpec) => {
    const checked = on.has(p.id);
    const allowed = canAddIndicator(indicators, p);
    const info = indicatorInfo(p);
    return (
      <Pressable
        key={p.id}
        accessibilityRole="checkbox"
        accessibilityState={{ checked, disabled: !allowed }}
        accessibilityLabel={`${info.name} ${info.params}`}
        accessibilityHint={info.description}
        disabled={!allowed}
        onPress={() => onChange(toggleIndicator(indicators, p))}
        // No fill for a checked row (owner, 2026-10-18): the check, the tinted glyph and its raised
        // tile already say "on", so every row shares the sheet's background; only a press lights one.
        style={({ pressed }) => [s.indRow, pressed && { backgroundColor: UI.raised }, !allowed && { opacity: 0.45 }]}
      >
        <View style={[s.tile, { backgroundColor: checked ? UI.raised : UI.row }]}>
          <Glyph kind={p.kind} color={checked ? colorOf(p) : UI.textDim} />
        </View>
        <View style={{ flex: 1, gap: 2 }}>
          <View style={{ flexDirection: "row", alignItems: "center", gap: 6 }}>
            <Text style={s.indName}>{info.name}</Text>
            {info.params ? <Text style={s.chip}>{info.params}</Text> : null}
          </View>
          <Text style={s.indDesc} numberOfLines={1}>
            {allowed ? info.description : "Section full: turn one off first"}
          </Text>
        </View>
        <Check on={checked} />
      </Pressable>
    );
  };
  const header = (label: string, used: number, cap: number) => (
    <View style={s.indHeader}>
      <Text style={s.label}>{label}</Text>
      <Text style={[s.label, { color: used >= cap ? UI.text : UI.textFaint, fontVariant: ["tabular-nums"] }]}>{`${used}/${cap}`}</Text>
    </View>
  );
  return (
    <Sheet
      open={open}
      onClose={onClose}
      title="Indicators"
      reducedMotion={reducedMotion}
      badge={
        indicators.length > 0 ? (
          // A quiet action beside the title, not another row that reads like an indicator.
          <Pressable accessibilityRole="button" onPress={() => onChange([])} hitSlop={8} style={({ pressed }) => [s.titleAction, pressed && { opacity: 0.6 }]}>
            <Text style={s.titleActionText}>Turn all off</Text>
          </Pressable>
        ) : undefined
      }
    >
      <ScrollView showsVerticalScrollIndicator={false} showsHorizontalScrollIndicator={false}>
        {header("On the chart", active("overlay").length, MAX_OVERLAYS)}
        {group("overlay").map(row)}
        {header("Below the chart", active("pane").length, MAX_PANES)}
        {group("pane").map(row)}
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
  onAlertAt,
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
  /** Price alerts: "Alert at 2.47" (the approved design's long-press axis item). */
  onAlertAt?: () => void;
  reducedMotion?: boolean;
}) {
  const close = (f: () => void) => () => {
    f();
    onClose();
  };
  return (
    <Sheet
      open={open}
      onClose={onClose}
      title="Price axis"
      badge={price !== null ? <Text style={s.priceChip}>{formatPrice(price)}</Text> : undefined}
      reducedMotion={reducedMotion}
    >
      <View style={{ gap: 12 }}>
        {onAlertAt && price !== null && (
          <Pressable accessibilityRole="button" onPress={() => {
            onClose();
            // After this sheet closes, so two modals never animate at once.
            setTimeout(onAlertAt, 0);
          }} style={({ pressed }) => [s.actionRow, pressed && { backgroundColor: UI.raised }]}>
            <Icon name="bell" color={UI.accent} size={16} />
            <Text style={s.actionText}>{`Alert at ${formatPrice(price)}`}</Text>
            <Chevron />
          </Pressable>
        )}
        {onOrderAt && price !== null && (
          <Pressable accessibilityRole="button" onPress={close(onOrderAt)} style={({ pressed }) => [s.actionRow, pressed && { backgroundColor: UI.raised }]}>
            <Text style={s.actionText}>{`Order at ${formatPrice(price)}`}</Text>
            <Chevron />
          </Pressable>
        )}
        {/* The three scales are one choice, so they are one control. */}
        <View style={{ gap: 6 }}>
          <Text style={s.fieldLabel}>Scale</Text>
          <Segmented
            fill
            value={scaleType}
            onChange={(t) => {
              onScaleType(t);
              onClose();
            }}
            options={[
              { value: "linear", label: "Price", a11y: SCALE_LABELS.linear },
              { value: "percent", label: "Percent", a11y: SCALE_LABELS.percent },
              { value: "log", label: "Log", a11y: SCALE_LABELS.log },
            ]}
          />
        </View>
        <View style={s.actionRow}>
          <View style={{ flex: 1, gap: 2 }}>
            <Text style={[s.actionText, { flex: 0 }]}>Auto-scale</Text>
            <Text style={s.indDesc}>{autoScaled ? "Fits the visible candles" : "Off: you set the range by dragging"}</Text>
          </View>
          {autoScaled ? (
            <Text style={s.onChip}>On</Text>
          ) : (
            <Pressable accessibilityRole="button" accessibilityLabel="Turn auto-scale on" onPress={close(onAutoScale)} hitSlop={8} style={({ pressed }) => [s.turnOn, pressed && { opacity: 0.7 }]}>
              <Text style={s.turnOnText}>Turn on</Text>
            </Pressable>
          )}
        </View>
      </View>
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
  fills,
  realized = 0,
}: {
  open: boolean;
  onClose: () => void;
  symbol: string;
  last: number | null;
  price: number | null;
  onSubmit: (o: TicketOrder) => void;
  reducedMotion?: boolean;
  /** This symbol's fills and realized P&L: the history under the action buttons. */
  fills?: readonly FillMark[];
  realized?: number;
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
    <Sheet
      open={open}
      onClose={onClose}
      title={`${side === "buy" ? "Buy" : "Sell"} ${symbol}`}
      badge={<Text style={s.simChip}>SIMULATED</Text>}
      reducedMotion={reducedMotion}
    >
      <View style={{ gap: 12 }}>
        {/* Controls on a recessed track: the sheet is the panel colour, so a panel track would vanish into it. */}
        <Segmented
          fill
          style={s.track}
          value={side}
          onChange={changeSide}
          options={[
            { value: "buy", label: "Buy", a11y: "Buy", tint: UI.good, activeBg: "rgba(63,185,122,0.16)" },
            { value: "sell", label: "Sell", a11y: "Sell", tint: UI.critical, activeBg: "rgba(240,100,92,0.16)" },
          ]}
        />
        <Segmented
          fill
          style={s.track}
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
        {/* Shares: the field and the presets on one row. */}
        <View style={{ gap: 6 }}>
          <Text style={s.fieldLabel}>Shares</Text>
          <View style={{ flexDirection: "row", gap: 6 }}>
            <TextInput style={[s.input, { flex: 1 }]} keyboardType="number-pad" value={qty} onChangeText={(t) => setQty(t.replace(/[^\d]/g, ""))} accessibilityLabel="Shares" selectionColor={UI.accent} />
            {[100, 500, 1000].map((q) => (
              <ToolButton key={q} label={`${q} shares`} onPress={() => setQty(String(q))} style={[s.preset, qty === String(q) && { backgroundColor: UI.raised }]}>
                {q === 1000 ? "1K" : String(q)}
              </ToolButton>
            ))}
          </View>
        </View>
        {type !== "market" && (
          <View style={{ gap: 6 }}>
            <Text style={s.fieldLabel}>{type === "limit" ? "Limit price" : "Stop price"}</Text>
            <TextInput
              style={s.input}
              keyboardType="decimal-pad"
              value={px}
              onChangeText={(t) => setPx(t.replace(/[^\d.]/g, ""))}
              onBlur={() => Number.isFinite(p) && p > 0 && setPx(formatPrice(snapPrice(p)))}
              accessibilityLabel={type === "limit" ? "Limit price" : "Stop price"}
              selectionColor={UI.accent}
            />
          </View>
        )}
        <View style={s.summary}>
          <Text style={s.small}>{side === "buy" ? "Est. cost" : "Est. proceeds"}</Text>
          <Text style={[s.small, { color: UI.text, fontVariant: ["tabular-nums"] }]}>{est !== null && qtyOk ? `$${(est * n).toLocaleString("en-US", { minimumFractionDigits: 2, maximumFractionDigits: 2 })}` : "—"}</Text>
        </View>
        <View style={{ gap: 6 }}>
          <ToolButton primary label="Place simulated order" disabled={!qtyOk || !priceOk} onPress={submit} style={s.cta}>
            <Text style={s.ctaText}>{`${side === "buy" ? "Buy" : "Sell"} ${qtyOk ? n.toLocaleString("en-US") : ""} ${symbol} · ${type === "market" ? "Market" : `${type === "limit" ? "Limit" : "Stop"} ${snapped !== null ? formatPrice(snapped) : ""}`}`}</Text>
          </ToolButton>
          <Text style={[s.note, { textAlign: "center" }]}>Fills against the replay tape · no money, no broker</Text>
        </View>
        {fills && <TradeHistory fills={fills} realized={realized} />}
      </View>
    </Sheet>
  );
}

const s = StyleSheet.create({
  backdrop: { flex: 1, backgroundColor: "rgba(0,0,0,0.45)" },
  sheet: { backgroundColor: UI.panel, borderTopLeftRadius: 16, borderTopRightRadius: 16, paddingHorizontal: 16, paddingTop: 8, paddingBottom: 28, maxHeight: "75%" },
  tall: { height: "92%", maxHeight: "92%" },
  grabber: { alignSelf: "center", width: 36, height: 4, borderRadius: 2, backgroundColor: UI.raised, marginBottom: 10 },
  titleRow: { flexDirection: "row", alignItems: "center", gap: 8, marginBottom: 10 },
  priceChip: { color: UI.text, fontSize: 12, fontVariant: ["tabular-nums"], backgroundColor: UI.bg, borderRadius: 6, paddingHorizontal: 8, paddingVertical: 3, overflow: "hidden" },
  actionRow: { minHeight: 48, flexDirection: "row", alignItems: "center", gap: 10, paddingHorizontal: 14, paddingVertical: 10, borderRadius: 12, backgroundColor: UI.row },
  actionText: { color: UI.text, fontSize: 14, fontWeight: "500", flex: 1 },
  onChip: { color: UI.accent, fontSize: 12, fontWeight: "600", backgroundColor: "rgba(139,156,255,0.14)", borderRadius: 6, paddingHorizontal: 8, paddingVertical: 3, overflow: "hidden" },
  halfAction: { flex: 1, height: 44, borderRadius: 12, backgroundColor: UI.row, alignItems: "center", justifyContent: "center" },
  halfText: { fontSize: 14, fontWeight: "600" },
  turnOn: { borderRadius: 8, paddingHorizontal: 10, paddingVertical: 6, backgroundColor: UI.raised },
  turnOnText: { color: UI.text, fontSize: 12, fontWeight: "600" },
  titleAction: { marginLeft: "auto", paddingHorizontal: 4, paddingVertical: 2 },
  titleActionText: { color: UI.textDim, fontSize: 13 },
  title: { color: UI.text, fontSize: 15, fontWeight: "600" },
  simChip: { color: UI.warning, fontSize: 9, fontWeight: "600", letterSpacing: 0.6, backgroundColor: "rgba(224,166,58,0.14)", borderRadius: 4, paddingHorizontal: 6, paddingVertical: 2, overflow: "hidden" },
  track: { backgroundColor: UI.bg, borderRadius: 10, padding: 3 },
  fieldLabel: { color: UI.textDim, fontSize: 11, fontWeight: "500" },
  preset: { width: 48, height: 42, borderRadius: 10, backgroundColor: UI.row },
  summary: { flexDirection: "row", justifyContent: "space-between", alignItems: "baseline", paddingHorizontal: 2 },
  cta: { height: 48, borderRadius: 12 },
  ctaText: { color: UI.accentFg, fontSize: 15, fontWeight: "600" },
  historyHeader: { flexDirection: "row", justifyContent: "space-between", alignItems: "baseline", paddingBottom: 4 },
  label: { color: UI.textFaint, fontSize: 11, marginTop: 8, marginBottom: 2 },
  row: { minHeight: 44, flexDirection: "row", alignItems: "center", paddingHorizontal: 8, borderRadius: 8 },
  rowText: { flex: 1, color: UI.text, fontSize: 14 },
  indHeader: { flexDirection: "row", justifyContent: "space-between", alignItems: "baseline", paddingHorizontal: 8, marginTop: 6 },
  indRow: { minHeight: 56, flexDirection: "row", alignItems: "center", gap: 12, paddingHorizontal: 8, borderRadius: 10 },
  tile: { width: 36, height: 36, borderRadius: 9, alignItems: "center", justifyContent: "center" },
  indName: { color: UI.text, fontSize: 14, fontWeight: "500" },
  // A translucent chip reads on a row at rest (panel) and while it is pressed (raised colour).
  chip: { color: UI.textDim, fontSize: 10, backgroundColor: "rgba(228,232,237,0.08)", borderRadius: 4, paddingHorizontal: 4, paddingVertical: 1, overflow: "hidden", fontVariant: ["tabular-nums"] },
  indDesc: { color: UI.textFaint, fontSize: 11 },
  profile: { gap: 6, paddingBottom: 6 },
  profileRow: { flexDirection: "row", alignItems: "center", gap: 10, paddingHorizontal: 8 },
  history: { gap: 2, marginTop: 2, padding: 12, borderRadius: 12, backgroundColor: UI.bg },
  // Rows bleed into the card's padding so their text lines up with the header above.
  tradeRow: { flexDirection: "row", alignItems: "center", gap: 8, minHeight: 32, paddingHorizontal: 8, marginHorizontal: -8, borderRadius: 6 },
  tradeCell: { color: UI.text, fontSize: 12, fontVariant: ["tabular-nums"] },
  rowRight: { marginLeft: 8 },
  input: { height: 42, backgroundColor: UI.row, borderRadius: 10, paddingHorizontal: 10, color: UI.text, fontSize: 15, fontVariant: ["tabular-nums"] },
  small: { color: UI.textDim, fontSize: 12 },
  note: { color: UI.textFaint, fontSize: 11, lineHeight: 15 },
  sim: { color: UI.warning, fontSize: 10, fontWeight: "500", letterSpacing: 0.4 },
  big: { flex: 1, height: 52, borderRadius: 12 },
  bigText: { fontSize: 15, fontWeight: "600" },
  mid: { flex: 1, height: 40, backgroundColor: UI.row, borderRadius: 10 },
});

/** The context layers as switches (halts, LULD, key levels, fills), the phone's twin of chart-web's LayersMenu. */
/**
 * The Levels sheet: the context layers, and the session volume profile's
 * readout and bin width (the web's side panel; a phone draws the profile
 * inside the chart, so its settings live here, beside the other overlays).
 */
export function LayersSheet({
  open,
  onClose,
  layers,
  onChange,
  reducedMotion,
  layerIds = LAYER_IDS,
  profile,
  profileBins,
  profileBin,
  onProfileBinChange,
}: {
  open: boolean;
  onClose: () => void;
  layers: ChartLayers;
  onChange: (l: ChartLayers) => void;
  reducedMotion?: boolean;
  /** Which layers to list (default all). */
  layerIds?: readonly LayerId[];
  profile?: ProfileWire | null;
  profileBins?: readonly number[];
  profileBin?: number;
  onProfileBinChange?: (bin: number) => void;
}) {
  const va = profile?.valueArea ?? null;
  const onCount = layerIds.filter((id) => layers[id]).length;
  return (
    <Sheet open={open} onClose={onClose} title="Levels" reducedMotion={reducedMotion}>
      <ScrollView showsVerticalScrollIndicator={false} showsHorizontalScrollIndicator={false}>
        {profileBins && profileBin !== undefined && onProfileBinChange && (!layerIds.includes("profile") || layers.profile) && (
          <View style={s.profile} accessibilityLabel="Session volume profile">
            <View style={s.indHeader}>
              <Text style={s.label}>Volume profile</Text>
              {profile?.poc != null && (
                <Text style={s.small}>
                  POC <Text style={{ color: UI.accent, fontVariant: ["tabular-nums"] }}>{formatPrice(profile.poc)}</Text>
                  {va ? `  ·  VA ${formatPrice(va.low)}–${formatPrice(va.high)}` : ""}
                </Text>
              )}
            </View>
            <View style={s.profileRow}>
              <Text style={s.small}>Bin</Text>
              <Segmented
                value={String(profileBin)}
                onChange={(v) => onProfileBinChange(Number(v))}
                options={profileBins.map((b) => ({ value: String(b), label: b.toFixed(2), a11y: `Bin ${b.toFixed(2)} dollars` }))}
              />
            </View>
          </View>
        )}
        {/* Rows in the Indicators sheet's design: a tile showing how the layer looks on the chart, name and description, a check when on. */}
        <View style={s.indHeader}>
          <Text style={s.label}>On the chart</Text>
          <Text style={[s.label, { color: onCount === layerIds.length ? UI.text : UI.textFaint, fontVariant: ["tabular-nums"] }]}>{`${onCount}/${layerIds.length}`}</Text>
        </View>
        {layerIds.map((id) => {
          const checked = layers[id];
          return (
            <Pressable
              key={id}
              accessibilityRole="checkbox"
              accessibilityState={{ checked }}
              accessibilityLabel={LAYER_INFO[id].name}
              accessibilityHint={LAYER_INFO[id].description}
              onPress={() => onChange({ ...layers, [id]: !checked })}
              // As in the Indicators sheet: no fill when checked, only while pressed.
              style={({ pressed }) => [s.indRow, pressed && { backgroundColor: UI.raised }]}
            >
              <View style={[s.tile, { backgroundColor: checked ? UI.raised : UI.row }]}>
                <LayerSwatch id={id} on={checked} />
              </View>
              <View style={{ flex: 1, gap: 2 }}>
                <Text style={s.indName}>{LAYER_INFO[id].name}</Text>
                <Text style={s.indDesc} numberOfLines={1}>
                  {LAYER_INFO[id].description}
                </Text>
              </View>
              <Check on={checked} />
            </Pressable>
          );
        })}
      </ScrollView>
    </Sheet>
  );
}

const ROLE: Record<string, string> = { take_profit: "TP", stop_loss: "SL", entry: "Entry" };

/** This symbol's simulated fills, newest first, with realized P&L: the bottom of the order ticket. */
export function TradeHistory({ fills, realized }: { fills: readonly FillMark[]; realized: number }) {
  const list = [...fills].reverse().slice(0, 30);
  return (
    <View style={s.history} accessibilityLabel="Trade history">
      <View style={s.historyHeader}>
        <Text style={s.fieldLabel}>Trades</Text>
        <Text style={[s.small, { color: realized > 0 ? UI.good : realized < 0 ? UI.critical : UI.text, fontVariant: ["tabular-nums"] }]}>{`Realized ${realized >= 0 ? "+" : "\u2212"}$${Math.abs(realized).toFixed(2)}`}</Text>
      </View>
      {list.length === 0 ? (
        <Text style={s.note}>No fills yet.</Text>
      ) : (
        list.map((f, i) => (
          <View key={`${f.orderId}-${i}`} style={[s.tradeRow, i % 2 === 0 && { backgroundColor: UI.row }]}>
            <Text style={[s.tradeCell, { color: UI.textDim, width: 64 }]}>{formatEtTime(f.t, true)}</Text>
            <Text style={[s.tradeCell, { color: f.side === "buy" ? UI.good : UI.critical, width: 36 }]}>{f.side === "buy" ? "Buy" : "Sell"}</Text>
            <Text style={[s.tradeCell, { flex: 1 }]}>{`${formatSize(f.qty)} @ ${formatPrice(f.price)}`}</Text>
            <Text style={[s.tradeCell, { color: UI.textFaint }]}>{f.role ? ROLE[f.role] : ""}</Text>
          </View>
        ))
      )}
    </View>
  );
}

/** chart-core's one list (the size syncs across devices, so the web's menu offers the same). */
export const QUICK_TRADE_SIZES = TRADE_SIZES;

/**
 * Trade at a price, the phone's twin of chart-web's right-click TradeMenu:
 * opened from the "+" chip a scrub leaves on the chart (Robinhood's
 * scrub-then-tap). One tap places a simulated order at the price: the
 * resting order it means for each side (below the market a buy limit or
 * sell stop, above it a sell limit or buy stop), or a market order; the
 * share size is picked once and kept. It also opens the full ticket or
 * drops a position plan at that price.
 */
export function QuickTradeSheet({
  open,
  onClose,
  symbol,
  price,
  last,
  qty,
  onQtyChange,
  onOrder,
  onTicket,
  onPosition,
  reducedMotion,
}: {
  open: boolean;
  onClose: () => void;
  symbol: string;
  price: number;
  last: number | null;
  qty: number;
  onQtyChange: (q: number) => void;
  onOrder: (side: OrderSide, type: "limit" | "stop" | "market") => void;
  onTicket: () => void;
  onPosition: (kind: "long" | "short") => void;
  reducedMotion?: boolean;
}) {
  const buyType = last === null ? "limit" : orderTypeAt("buy", price, last);
  const sellType = last === null ? "limit" : orderTypeAt("sell", price, last);
  const p = formatPrice(price);
  const diff = last === null ? null : Math.abs(price - last);
  const where = last === null ? null : price < last ? "below" : price > last ? "above" : "at";
  const act = (f: () => void) => () => {
    f();
    onClose();
  };
  return (
    <Sheet
      open={open}
      onClose={onClose}
      title={`Trade ${symbol}`}
      badge={
        <>
          <Text style={s.priceChip}>{p}</Text>
          <Text style={s.simChip}>SIMULATED</Text>
        </>
      }
      reducedMotion={reducedMotion}
    >
      <View style={{ gap: 12 }}>
        {where !== null && <Text style={[s.small, { marginTop: -4 }]}>{where === "at" ? "At the last trade" : `${formatPrice(diff!, priceDecimals(price))} ${where} the last trade`}</Text>}
        {/* Size first: it is what every button below places. */}
        <View style={{ gap: 6 }}>
          <Text style={s.fieldLabel}>Shares</Text>
          <Segmented fill value={String(qty)} onChange={(v) => onQtyChange(Number(v))} options={QUICK_TRADE_SIZES.map((n) => ({ value: String(n), label: n.toLocaleString("en-US"), a11y: `${n} shares` }))} />
        </View>
        <View style={{ gap: 8 }}>
          <View style={{ flexDirection: "row", gap: 8 }}>
            <ToolButton label={`Buy ${buyType} ${qty} at ${p}`} onPress={act(() => onOrder("buy", buyType))} style={[s.big, { backgroundColor: "rgba(63,185,122,0.16)" }]}>
              <Text style={[s.bigText, { color: UI.good }]}>{`Buy ${buyType}`}</Text>
            </ToolButton>
            <ToolButton label={`Sell ${sellType} ${qty} at ${p}`} onPress={act(() => onOrder("sell", sellType))} style={[s.big, { backgroundColor: "rgba(240,100,92,0.16)" }]}>
              <Text style={[s.bigText, { color: UI.critical }]}>{`Sell ${sellType}`}</Text>
            </ToolButton>
          </View>
          <View style={{ flexDirection: "row", gap: 8 }}>
            <ToolButton label={`Buy ${qty} at market`} onPress={act(() => onOrder("buy", "market"))} style={[s.mid]}>
              Buy at market
            </ToolButton>
            <ToolButton label={`Sell ${qty} at market`} onPress={act(() => onOrder("sell", "market"))} style={[s.mid]}>
              Sell at market
            </ToolButton>
          </View>
        </View>
        {/* More: the full ticket, or plan a position here; buttons, not loose text. */}
        <Pressable accessibilityRole="button" onPress={act(onTicket)} style={({ pressed }) => [s.actionRow, pressed && { backgroundColor: UI.raised }]}>
          <Text style={s.actionText}>{`Order ticket at ${p}`}</Text>
          <Chevron />
        </Pressable>
        <View style={{ flexDirection: "row", gap: 8 }}>
          <Pressable accessibilityRole="button" accessibilityLabel="Plan a long position here" onPress={act(() => onPosition("long"))} style={({ pressed }) => [s.halfAction, pressed && { backgroundColor: UI.raised }]}>
            <Text style={[s.halfText, { color: UI.good }]}>Long here</Text>
          </Pressable>
          <Pressable accessibilityRole="button" accessibilityLabel="Plan a short position here" onPress={act(() => onPosition("short"))} style={({ pressed }) => [s.halfAction, pressed && { backgroundColor: UI.raised }]}>
            <Text style={[s.halfText, { color: UI.critical }]}>Short here</Text>
          </Pressable>
        </View>
      </View>
    </Sheet>
  );
}

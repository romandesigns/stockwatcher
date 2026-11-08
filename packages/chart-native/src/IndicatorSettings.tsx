import {
  defaultRoleColor,
  familyColor,
  familyGradient,
  hueOf,
  indicatorColorRoles,
  indicatorSwatches,
  isHexColor,
  MACD_STYLES,
  macdMarkersOn,
  macdStyleOf,
  isPresetSpec,
  resetIndicator,
  roleColor,
  swatchFamily,
  withIndicatorColor,
  type ChartTheme,
  type ColorRole,
  type IndicatorSpec,
  type ToneFamily,
} from "@stockwatcher/chart-core";
import { useEffect, useMemo, useRef, useState } from "react";
import { LayoutAnimation, PanResponder, Pressable, ScrollView, StyleSheet, Text, TextInput, View, type AccessibilityActionEvent } from "react-native";
import Svg, { Defs, LinearGradient, Rect, Stop } from "react-native-svg";
import { Icon, UI } from "./ui.tsx";

/**
 * One indicator's settings inside the phone's indicator sheet (chart-web's IndicatorSettingsDialog,
 * owner 2026-11-07): for a MACD its style (when the server computes them), crossover markers and
 * periods; for every indicator its line colours, each a row that opens a swatch row (the palette's
 * indicator hues, 44 px targets), a hue strip at the swatches' own lightness and chroma (HueStrip)
 * and a hex field, and "Reset to default". Style, markers and periods are a new spec for this
 * chart's list; colours are GLOBAL through `onColor` / `onResetColors` (the app store's
 * indicatorColorActions: every chart, every device), or written onto the spec without them.
 */
export function IndicatorSettings({
  spec,
  colorSlot,
  theme,
  macdStyles,
  onChange,
  onColor,
  onResetColors,
  reducedMotion,
}: {
  spec: IndicatorSpec;
  colorSlot: number;
  theme: ChartTheme;
  macdStyles: boolean;
  onChange: (next: IndicatorSpec) => void;
  onColor?: (spec: IndicatorSpec, role: ColorRole, color: string | null) => void;
  onResetColors?: (spec: IndicatorSpec) => void;
  reducedMotion?: boolean;
}) {
  const [open, setOpen] = useState<ColorRole | null>(null);
  const style = macdStyleOf(spec);
  const swatches = indicatorSwatches(theme);
  const swatchKey = swatches.join();
  const family = useMemo(() => swatchFamily(swatchKey.split(",")), [swatchKey]);
  // "Reset to default" puts the whole indicator back to its preset (style, markers, periods,
  // colours): an indicator turned off and on keeps its settings, so this is the way back.
  const custom = !isPresetSpec(spec);
  const toggle = (role: ColorRole) => {
    // Smooth motion: the picker row opens and closes rather than jumping (owner rule); cut with reduced motion.
    if (!reducedMotion) LayoutAnimation.configureNext(LayoutAnimation.create(160, "easeInEaseOut", "opacity"));
    setOpen((o) => (o === role ? null : role));
  };
  return (
    <ScrollView showsVerticalScrollIndicator={false} showsHorizontalScrollIndicator={false} keyboardShouldPersistTaps="handled" contentContainerStyle={{ gap: 6, paddingBottom: 8 }}>
      {spec.kind === "macd" && macdStyles && (
        <View accessibilityRole="radiogroup" accessibilityLabel="Style">
          <Text style={s.label}>Style</Text>
          {MACD_STYLES.map((m) => {
            const on = m.id === style;
            return (
              <Pressable
                key={m.id}
                accessibilityRole="radio"
                accessibilityState={{ checked: on }}
                accessibilityLabel={m.name}
                accessibilityHint={m.hint}
                onPress={() => onChange(m.id === "classic" ? withoutStyle(spec) : { ...spec, style: m.id })}
                style={({ pressed }) => [s.row, pressed && { backgroundColor: UI.raised }]}
              >
                <View style={{ flex: 1, gap: 2 }}>
                  <Text style={s.name}>{m.name}</Text>
                  <Text style={s.hint} numberOfLines={1}>
                    {m.hint}
                  </Text>
                </View>
                <View style={{ opacity: on ? 1 : 0 }}>
                  <Icon name="check" color={UI.accent} size={14} />
                </View>
              </Pressable>
            );
          })}
        </View>
      )}
      {spec.kind === "macd" && (
        <View>
          <Pressable accessibilityRole="switch" accessibilityState={{ checked: macdMarkersOn(spec) }} onPress={() => onChange({ ...spec, markers: !macdMarkersOn(spec) })} style={({ pressed }) => [s.row, pressed && { backgroundColor: UI.raised }]}>
            <View style={{ flex: 1, gap: 2 }}>
              <Text style={s.name}>Crossover markers</Text>
              <Text style={s.hint} numberOfLines={1}>
                Triangles at signal crosses, rings at zero crosses
              </Text>
            </View>
            <View style={[s.track, macdMarkersOn(spec) && s.trackOn]}>
              <View style={[s.thumb, macdMarkersOn(spec) && s.thumbOn]} />
            </View>
          </Pressable>
          <View style={s.periods}>
            {(["fast", "slow", "signal"] as const).map((k) => (
              <PeriodField key={k} label={k === "fast" ? "Fast" : k === "slow" ? "Slow" : "Signal"} value={spec[k] ?? (k === "fast" ? 12 : k === "slow" ? 26 : 9)} onValue={(v) => onChange({ ...spec, [k]: v })} />
            ))}
          </View>
        </View>
      )}
      <View style={s.colorsHeader}>
        <Text style={[s.label, { marginTop: 0 }]}>Colors</Text>
        <Pressable accessibilityRole="button" disabled={!custom} onPress={() => {
            onChange(resetIndicator(spec));
            onResetColors?.(spec);
          }}
          hitSlop={8} style={({ pressed }) => [s.reset, pressed && { opacity: 0.6 }, !custom && { opacity: 0.4 }]}>
          <Text style={s.resetText}>Reset to default</Text>
        </Pressable>
      </View>
      {indicatorColorRoles(spec).map(({ role, name }) => {
        const value = roleColor(theme, spec, colorSlot, role);
        const fallback = defaultRoleColor(theme, spec, colorSlot, role);
        const picked = !!spec.colors?.[role];
        const pick = (raw: string | null) => {
          const c = raw && raw.toLowerCase() === fallback.toLowerCase() ? null : raw;
          if (onColor) onColor(spec, role, c);
          else onChange(withIndicatorColor(spec, role, c));
        };
        return (
          <View key={role}>
            <Pressable accessibilityRole="button" accessibilityState={{ expanded: open === role }} accessibilityLabel={`${name} color, ${picked ? value : "default"}`} onPress={() => toggle(role)} style={({ pressed }) => [s.row, pressed && { backgroundColor: UI.raised }]}>
              <View style={[s.dot, { backgroundColor: value }]} />
              <Text style={[s.name, { flex: 1 }]}>{name}</Text>
              <Text style={s.hex}>{picked ? value : "Default"}</Text>
            </Pressable>
            {open === role && <Picker value={value} swatches={swatches} family={family} onPick={pick} name={name} />}
          </View>
        );
      })}
    </ScrollView>
  );
}

function withoutStyle(spec: IndicatorSpec): IndicatorSpec {
  const { style: _, ...rest } = spec;
  return rest;
}

/** The phone's colour picker: the preset swatches (44 px, wrapping to fill the row evenly), the hue strip and a hex field. */
function Picker({ value, swatches, family, onPick, name }: { value: string; swatches: readonly string[]; family: ToneFamily | null; onPick: (c: string | null) => void; name: string }) {
  const [hex, setHex] = useState(value);
  useEffect(() => setHex(value), [value]);
  return (
    <View style={s.picker}>
      <View style={s.swatches} accessibilityLabel={`${name} presets`}>
        {swatches.map((c) => {
          const on = c.toLowerCase() === value.toLowerCase();
          return (
            <Pressable key={c} accessibilityRole="button" accessibilityLabel={c} accessibilityState={{ selected: on }} onPress={() => onPick(c)} style={({ pressed }) => [s.swatchHit, pressed && { transform: [{ scale: 0.94 }] }]}>
              <View style={[s.ring, on && { backgroundColor: UI.text }]}>
                <View style={[s.swatch, { backgroundColor: c }]} />
              </View>
            </Pressable>
          );
        })}
      </View>
      {family && <HueStrip name={name} value={value} family={family} onPick={onPick} />}
      <View style={{ flexDirection: "row", alignItems: "center", gap: 10 }}>
        <View style={[s.dot, { width: 32, height: 32, borderRadius: 8, backgroundColor: isHexColor(hex) ? hex : value }]} />
        <TextInput
          style={[s.input, { flex: 1 }]}
          value={hex}
          autoCapitalize="none"
          autoCorrect={false}
          placeholder="#rrggbb"
          placeholderTextColor={UI.textFaint}
          accessibilityLabel={`${name} hex color`}
          selectionColor={UI.accent}
          onChangeText={(t) => {
            const raw = t.trim();
            setHex(raw);
            const next = raw.startsWith("#") ? raw : `#${raw}`;
            if (isHexColor(next)) onPick(next);
          }}
        />
      </View>
    </View>
  );
}

/**
 * Any hue in the swatches' tonal family (chart-web's HueStrip, same chart-core OKLCH maths, so a
 * hue gives the same hex on both): a gradient strip drawn with react-native-svg at the family's L
 * and C, and a thumb dragged with RN core's PanResponder (no native module: the app's native
 * fingerprint, and so its OTA updates, stay as they are). 48 px tall, a tap anywhere jumps there.
 * Screen readers get an adjustable control: swipe up/down moves 15 degrees.
 */
function HueStrip({ name, value, family, onPick }: { name: string; value: string; family: ToneFamily; onPick: (c: string) => void }) {
  const [hue, setHue] = useState(() => Math.round(hueOf(value) ?? 0) % 360);
  const [width, setWidth] = useState(0);
  const widthRef = useRef(0);
  const pickRef = useRef(onPick);
  pickRef.current = onPick;
  // Follow a colour picked elsewhere, not this strip's own echo (its hue read back may differ by a degree).
  useEffect(() => {
    if (value.toLowerCase() === familyColor(family, hue)) return;
    const h = hueOf(value);
    if (h !== null) setHue(Math.round(h) % 360);
    // eslint-disable-next-line react-hooks/exhaustive-deps -- `hue` is the strip's own echo, not a trigger
  }, [value, family]);
  const familyRef = useRef(family);
  familyRef.current = family;
  const choose = (h: number) => {
    const next = Math.min(359, Math.max(0, Math.round(h)));
    setHue(next);
    pickRef.current(familyColor(familyRef.current, next));
  };
  const chooseRef = useRef(choose);
  chooseRef.current = choose;
  const responder = useMemo(
    () =>
      PanResponder.create({
        onStartShouldSetPanResponder: () => true,
        onMoveShouldSetPanResponder: () => true,
        // The sheet's scroll view must not take a horizontal drag away mid-pick.
        onPanResponderTerminationRequest: () => false,
        onPanResponderGrant: (e) => widthRef.current > 0 && chooseRef.current((e.nativeEvent.locationX / widthRef.current) * 359),
        onPanResponderMove: (e) => widthRef.current > 0 && chooseRef.current((e.nativeEvent.locationX / widthRef.current) * 359),
      }),
    [],
  );
  const stops = useMemo(() => familyGradient(family), [family]);
  const current = familyColor(family, hue);
  const THUMB = 30;
  const x = width > 0 ? (hue / 359) * width : 0;
  const onAction = (e: AccessibilityActionEvent) => {
    if (e.nativeEvent.actionName === "increment") choose(Math.min(359, hue + 15));
    if (e.nativeEvent.actionName === "decrement") choose(Math.max(0, hue - 15));
  };
  return (
    <View style={{ gap: 4 }}>
      <Text style={s.hint}>Any hue, same tone as the swatches</Text>
      <View
        accessible
        accessibilityRole="adjustable"
        accessibilityLabel={`${name} hue`}
        accessibilityValue={{ min: 0, max: 359, now: hue, text: `${hue} degrees, ${current}` }}
        accessibilityActions={[{ name: "increment" }, { name: "decrement" }]}
        onAccessibilityAction={onAction}
        onLayout={(e) => {
          widthRef.current = e.nativeEvent.layout.width;
          setWidth(e.nativeEvent.layout.width);
        }}
        style={s.strip}
        {...responder.panHandlers}
      >
        <View pointerEvents="none" style={StyleSheet.absoluteFill}>
          <Svg width="100%" height="100%">
            <Defs>
              <LinearGradient id="hue" x1="0" y1="0" x2="1" y2="0">
                {stops.map((st) => (
                  <Stop key={st.offset} offset={st.offset} stopColor={st.color} />
                ))}
              </LinearGradient>
            </Defs>
            <Rect x="0" y="12" width="100%" height="24" rx="12" fill="url(#hue)" />
          </Svg>
        </View>
        {width > 0 && (
          <View pointerEvents="none" style={[s.thumbRing, { left: x - THUMB / 2 - 3 }]}>
            <View style={[s.thumbFill, { backgroundColor: current }]} />
          </View>
        )}
      </View>
    </View>
  );
}

function PeriodField({ label, value, onValue }: { label: string; value: number; onValue: (v: number) => void }) {
  const [draft, setDraft] = useState(String(value));
  useEffect(() => setDraft(String(value)), [value]);
  return (
    <View style={{ flex: 1, gap: 4 }}>
      <Text style={s.hint}>{label}</Text>
      <TextInput
        style={s.input}
        keyboardType="number-pad"
        value={draft}
        accessibilityLabel={`${label} period`}
        selectionColor={UI.accent}
        onChangeText={(t) => {
          const raw = t.replace(/\D/g, "").slice(0, 3);
          setDraft(raw);
          const n = Number(raw);
          if (raw && n >= 1 && n <= 500 && n !== value) onValue(n);
        }}
        onBlur={() => setDraft(String(value))}
      />
    </View>
  );
}

const s = StyleSheet.create({
  label: { color: UI.textFaint, fontSize: 11, marginTop: 8, marginBottom: 2, paddingHorizontal: 8 },
  row: { minHeight: 48, flexDirection: "row", alignItems: "center", gap: 12, paddingHorizontal: 8, borderRadius: 10 },
  name: { color: UI.text, fontSize: 14, fontWeight: "500" },
  hint: { color: UI.textFaint, fontSize: 11 },
  hex: { color: UI.textFaint, fontSize: 11, fontVariant: ["tabular-nums"] },
  dot: { width: 22, height: 22, borderRadius: 6 },
  colorsHeader: { flexDirection: "row", alignItems: "center", justifyContent: "space-between", paddingRight: 8, marginTop: 8 },
  reset: { minHeight: 44, justifyContent: "center", paddingHorizontal: 4 },
  resetText: { color: UI.textDim, fontSize: 13 },
  track: { width: 36, height: 20, borderRadius: 10, backgroundColor: UI.raised, padding: 2 },
  trackOn: { backgroundColor: UI.text },
  thumb: { width: 16, height: 16, borderRadius: 8, backgroundColor: UI.textDim },
  thumbOn: { backgroundColor: UI.panel, transform: [{ translateX: 16 }] },
  periods: { flexDirection: "row", gap: 8, paddingHorizontal: 8, marginTop: 4 },
  input: { height: 44, backgroundColor: UI.row, borderRadius: 10, paddingHorizontal: 10, color: UI.text, fontSize: 15, fontVariant: ["tabular-nums"] },
  picker: { gap: 10, padding: 8, marginHorizontal: 4, marginBottom: 4, borderRadius: 12, backgroundColor: UI.row },
  // Five to a row (ten presets: two full rows, no orphan), each cell at least 44 px.
  swatches: { flexDirection: "row", flexWrap: "wrap", rowGap: 4 },
  swatchHit: { width: "20%", minWidth: 44, height: 44, alignItems: "center", justifyContent: "center" },
  // The picked swatch's ring is a fill behind it, not a border (owner rule).
  ring: { width: 40, height: 40, borderRadius: 11, alignItems: "center", justifyContent: "center" },
  swatch: { width: 34, height: 34, borderRadius: 9 },
  // The hue strip: 48 px of touch height around a 24 px gradient; the thumb's ring is a fill behind it (no border).
  strip: { height: 48, justifyContent: "center", marginHorizontal: 15 },
  thumbRing: { position: "absolute", top: 6, width: 36, height: 36, borderRadius: 18, backgroundColor: UI.text, alignItems: "center", justifyContent: "center" },
  thumbFill: { width: 30, height: 30, borderRadius: 15 },
});

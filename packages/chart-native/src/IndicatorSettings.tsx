import {
  defaultRoleColor,
  indicatorColorRoles,
  indicatorSwatches,
  isHexColor,
  MACD_STYLES,
  macdMarkersOn,
  macdStyleOf,
  isPresetSpec,
  resetIndicator,
  roleColor,
  withIndicatorColor,
  type ChartTheme,
  type ColorRole,
  type IndicatorSpec,
} from "@stockwatcher/chart-core";
import { useEffect, useState } from "react";
import { LayoutAnimation, Pressable, ScrollView, StyleSheet, Text, TextInput, View } from "react-native";
import { Icon, UI } from "./ui.tsx";

/**
 * One indicator's settings inside the phone's indicator sheet (chart-web's IndicatorSettingsDialog,
 * owner 2026-11-07): for a MACD its style (when the server computes them), crossover markers and
 * periods; for every indicator its line colours, each a row that opens a swatch row (the palette's
 * indicator hues, 44 px targets) and a hex field, and "Reset to default". Every change is a new
 * spec for the chart's list, saved and synced with the indicator.
 */
export function IndicatorSettings({ spec, colorSlot, theme, macdStyles, onChange, reducedMotion }: { spec: IndicatorSpec; colorSlot: number; theme: ChartTheme; macdStyles: boolean; onChange: (next: IndicatorSpec) => void; reducedMotion?: boolean }) {
  const [open, setOpen] = useState<ColorRole | null>(null);
  const style = macdStyleOf(spec);
  const swatches = indicatorSwatches(theme);
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
        <Pressable accessibilityRole="button" disabled={!custom} onPress={() => onChange(resetIndicator(spec))} hitSlop={8} style={({ pressed }) => [s.reset, pressed && { opacity: 0.6 }, !custom && { opacity: 0.4 }]}>
          <Text style={s.resetText}>Reset to default</Text>
        </Pressable>
      </View>
      {indicatorColorRoles(spec).map(({ role, name }) => {
        const value = roleColor(theme, spec, colorSlot, role);
        const fallback = defaultRoleColor(theme, spec, colorSlot, role);
        const picked = !!spec.colors?.[role];
        const pick = (c: string | null) => onChange(withIndicatorColor(spec, role, c && c.toLowerCase() === fallback.toLowerCase() ? null : c));
        return (
          <View key={role}>
            <Pressable accessibilityRole="button" accessibilityState={{ expanded: open === role }} accessibilityLabel={`${name} color, ${picked ? value : "default"}`} onPress={() => toggle(role)} style={({ pressed }) => [s.row, pressed && { backgroundColor: UI.raised }]}>
              <View style={[s.dot, { backgroundColor: value }]} />
              <Text style={[s.name, { flex: 1 }]}>{name}</Text>
              <Text style={s.hex}>{picked ? value : "Default"}</Text>
            </Pressable>
            {open === role && <Picker value={value} swatches={swatches} onPick={pick} name={name} />}
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

/** The phone's colour picker: the preset swatches (44 px, wrapping to fill the row evenly) and a hex field. */
function Picker({ value, swatches, onPick, name }: { value: string; swatches: readonly string[]; onPick: (c: string | null) => void; name: string }) {
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
});

import {
  CHART_TYPE_ICONS,
  CHART_TYPE_INFO,
  CHART_TYPES,
  DEFAULT_CHART_SETTINGS,
  SETTINGS_SECTIONS,
  ZOOM_PRESETS,
  type ChartSettings,
  type ChartVisibility,
  type PriceScaleType,
  type ZoomPreset,
} from "@stockwatcher/chart-core";
import type { ReactNode } from "react";
import { Pressable, ScrollView, StyleSheet, Text, View } from "react-native";
import type { SettingsSheetSection } from "./SettingsSheet.tsx";
import { Icon, Segmented, UI } from "./ui.tsx";

// The phone's twin of chart-web's ChartSettingsPanel, the Settings sheet's
// Chart tab: the same sections, the same words. It was a Sheet of its own
// behind a Chart button in the chart toolbar until the owner moved the chart
// settings into the Settings dialog and asked for that button to go
// (2026-10-18), so this tab is now their one home on the phone too. Choices
// of several are Segmented controls (selected raised, not accented); on/off
// rows end with the accent check the Indicators and Levels sheets use.
// Changes apply immediately; the values are the host's synced settings
// (namespace `chartSettings`), so a change on another device shows live.

const SHORT = { candles: "Candles", hollow: "Hollow", bars: "Bars", heikinAshi: "Heikin-Ashi", line: "Line", area: "Area", baseline: "Baseline" } as const;

type Switch = "extendedHours" | "volume" | "grid" | "sessionShading" | "sessionBreaks" | "highLow";
const SHOW: readonly { key: Switch; name: string; description: string }[] = [
  { key: "extendedHours", name: "Extended hours", description: "Pre-market, after-hours and overnight candles" },
  { key: "volume", name: "Volume", description: "Volume bars under the price" },
  { key: "grid", name: "Grid lines", description: "Horizontal lines at the price labels" },
  { key: "sessionShading", name: "Highlight extended hours", description: "Shade candles outside 09:30–16:00" },
  { key: "sessionBreaks", name: "Session breaks", description: "A dashed line where each day starts" },
  { key: "highLow", name: "High / low labels", description: "Mark the highest and lowest price on screen" },
];
const VISIBILITY: readonly { key: keyof ChartVisibility; name: string }[] = [
  { key: "indicators", name: "Indicators" },
  { key: "drawings", name: "Drawings" },
  { key: "trading", name: "Orders and positions" },
];

export function ChartSettingsPanel({ settings, onChange }: { settings: ChartSettings; onChange: (s: ChartSettings) => void }) {
  const set = (patch: Partial<ChartSettings>) => onChange({ ...settings, ...patch });
  return (
    <ScrollView contentContainerStyle={{ gap: 14, paddingBottom: 8 }}>
      <Section label="Type">
        <View style={s.types} accessibilityRole="radiogroup">
          {CHART_TYPES.map((t) => {
            const on = settings.chartType === t;
            return (
              <Pressable
                key={t}
                accessibilityRole="radio"
                accessibilityState={{ selected: on }}
                accessibilityLabel={CHART_TYPE_INFO[t].name}
                accessibilityHint={CHART_TYPE_INFO[t].description}
                onPress={() => set({ chartType: t })}
                style={({ pressed }) => [s.tile, on && { backgroundColor: UI.raised }, pressed && !on && { backgroundColor: UI.row }]}
              >
                <Icon name={CHART_TYPE_ICONS[t]} color={on ? UI.text : UI.textDim} size={18} />
                <Text style={[s.tileText, { color: on ? UI.text : UI.textDim }]} numberOfLines={1}>
                  {SHORT[t]}
                </Text>
              </Pressable>
            );
          })}
        </View>
        {settings.chartType === "heikinAshi" && <Text style={s.note}>Averaged candles. The price label, readout and orders use real prices.</Text>}
      </Section>

      <Section label="Zoom">
        <Row name="Auto stretch" description={settings.autoStretch ? "Price fits the candles on screen" : "Fits once on open, then holds"} checked={settings.autoStretch} onToggle={() => set({ autoStretch: !settings.autoStretch })} />
        <View style={s.zoomRow}>
          <Text style={s.desc}>Of the trading day</Text>
          <Segmented value={String(settings.zoom)} onChange={(v) => set({ zoom: Number(v) as ZoomPreset })} options={ZOOM_PRESETS.map((z) => ({ value: String(z), label: `${z}%`, a11y: `${z} percent of the trading day` }))} />
        </View>
      </Section>

      <Section label="Price scale">
        <Segmented
          fill
          value={settings.priceScale}
          onChange={(v) => set({ priceScale: v as PriceScaleType })}
          options={[
            { value: "linear", label: "Price", a11y: "Linear (price)" },
            { value: "percent", label: "Percent", a11y: "Linear (%)" },
            { value: "log", label: "Log", a11y: "Logarithmic" },
          ]}
        />
      </Section>

      <Section label="Show">
        {SHOW.map((r) => (
          <Row key={r.key} name={r.name} description={r.description} checked={settings[r.key]} onToggle={() => set({ [r.key]: !settings[r.key] } as Partial<ChartSettings>)} />
        ))}
      </Section>

      <Section label="Visibility">
        {VISIBILITY.map((r) => (
          <Row key={r.key} name={r.name} checked={settings.visibility[r.key]} onToggle={() => set({ visibility: { ...settings.visibility, [r.key]: !settings.visibility[r.key] } })} />
        ))}
      </Section>
    </ScrollView>
  );
}

/** Reset for the panel's heading, the Drawing tab's twin: dimmed (not removed) when already default, so the heading never shifts. */
export function ChartSettingsReset({ settings, onChange }: { settings: ChartSettings; onChange: (s: ChartSettings) => void }) {
  const isDefault = JSON.stringify(settings) === JSON.stringify(DEFAULT_CHART_SETTINGS);
  return (
    <Pressable accessibilityRole="button" accessibilityLabel="Reset chart settings to defaults" accessibilityState={{ disabled: isDefault }} disabled={isDefault} onPress={() => onChange({ ...DEFAULT_CHART_SETTINGS, visibility: { ...DEFAULT_CHART_SETTINGS.visibility } })} hitSlop={8} style={({ pressed }) => [s.reset, isDefault && { opacity: 0.4 }, pressed && { backgroundColor: UI.raised }]}>
      <Text style={s.resetText}>Reset</Text>
    </Pressable>
  );
}

/** The Chart tab as a SettingsSheet section: chart-core's label, icon and description, this panel and its reset. */
export function chartSettingsSection(settings: ChartSettings, onChange: (s: ChartSettings) => void): SettingsSheetSection {
  const info = SETTINGS_SECTIONS.find((x) => x.id === "chart")!;
  return { ...info, action: <ChartSettingsReset settings={settings} onChange={onChange} />, render: () => <ChartSettingsPanel settings={settings} onChange={onChange} /> };
}

function Section({ label, children }: { label: string; children: ReactNode }) {
  return (
    <View style={{ gap: 4 }}>
      <Text style={s.label} accessibilityRole="header">
        {label}
      </Text>
      {children}
    </View>
  );
}

function Row({ name, description, checked, onToggle }: { name: string; description?: string; checked: boolean; onToggle: () => void }) {
  return (
    <Pressable
      accessibilityRole="switch"
      accessibilityState={{ checked }}
      accessibilityLabel={name}
      accessibilityHint={description}
      onPress={onToggle}
      style={({ pressed }) => [s.row, checked && { backgroundColor: UI.row }, pressed && { backgroundColor: UI.raised }]}
    >
      <View style={{ flex: 1, gap: 2 }}>
        <Text style={s.name}>{name}</Text>
        {description ? (
          <Text style={s.desc} numberOfLines={1}>
            {description}
          </Text>
        ) : null}
      </View>
      {/* The check fades rather than popping, as on the web (no layout shift: it always takes its room). */}
      <View style={{ opacity: checked ? 1 : 0 }}>
        <Icon name="check" color={UI.accent} size={14} />
      </View>
    </Pressable>
  );
}

const s = StyleSheet.create({
  label: { color: UI.textFaint, fontSize: 11, paddingHorizontal: 4 },
  types: { flexDirection: "row", flexWrap: "wrap", gap: 4, backgroundColor: UI.bg, borderRadius: 12, padding: 4 },
  // Four per row on a phone: (100% - 3 gaps) / 4.
  tile: { width: "23.6%", flexGrow: 1, height: 52, borderRadius: 9, alignItems: "center", justifyContent: "center", gap: 4 },
  tileText: { fontSize: 10 },
  note: { color: UI.textFaint, fontSize: 11, paddingHorizontal: 8 },
  zoomRow: { flexDirection: "row", alignItems: "center", justifyContent: "space-between", gap: 8, paddingHorizontal: 8 },
  row: { minHeight: 48, flexDirection: "row", alignItems: "center", gap: 12, paddingHorizontal: 8, paddingVertical: 6, borderRadius: 10 },
  name: { color: UI.text, fontSize: 14, fontWeight: "500" },
  desc: { color: UI.textFaint, fontSize: 11 },
  reset: { backgroundColor: UI.row, borderRadius: 8, paddingHorizontal: 10, paddingVertical: 5 },
  resetText: { color: UI.textDim, fontSize: 12 },
});

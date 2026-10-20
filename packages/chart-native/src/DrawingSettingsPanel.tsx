import {
  DEFAULT_DRAWING_SETTINGS,
  DEFAULT_TOOLBOX_PHONE,
  isDefaultDrawingSettings,
  isToolboxItemVisible,
  setToolboxGroupVisible,
  setToolboxItemVisible,
  SETTINGS_SECTIONS,
  TOOLBOX_ITEMS,
  type DrawingSettings,
  type ToolboxGroup,
  type ToolboxItemInfo,
} from "@stockwatcher/chart-core";
import { useEffect, useState } from "react";
import { Pressable, ScrollView, StyleSheet, Text, View } from "react-native";
import Animated, { interpolateColor, useAnimatedStyle, useSharedValue, withTiming } from "react-native-reanimated";
import { DrawingToolbox } from "./DrawingToolbox.tsx";
import type { SettingsSheetSection } from "./SettingsSheet.tsx";
import { Icon, UI } from "./ui.tsx";

// The phone's twin of chart-web's DrawingSettingsPanel: the same rows from
// chart-core's TOOLBOX_ITEMS (the list the toolbox is built from), the same
// words, the same rules (the cursor locked on, with its reason). The
// preview is the real native DrawingToolbox, inert, sized to the panel, so
// it also shows the two-line split a phone row needs when everything is on.
// As on the web it sits centred on the panel itself, with no recessed box
// behind it, and the toolbox carries a 1 px muted border instead (owner,
// 2026-10-18; an approved exception to the no-borders rule).

const GROUPS: readonly { id: ToolboxGroup; label: string }[] = [
  { id: "tools", label: "Tools" },
  { id: "actions", label: "Actions" },
];

const noop = () => {};
/** The toolbox preview's border width (DrawingToolbox previewBox), taken off the room it lays out in. */
const PREVIEW_BORDER = 1;

export function DrawingSettingsPanel({ settings, onChange, reducedMotion }: { settings: DrawingSettings; onChange: (s: DrawingSettings) => void; reducedMotion?: boolean }) {
  const [previewWidth, setPreviewWidth] = useState(0);
  return (
    <ScrollView showsVerticalScrollIndicator={false} showsHorizontalScrollIndicator={false} contentContainerStyle={{ gap: 14, paddingBottom: 8 }}>
      <View style={{ gap: 4 }}>
        <Text style={s.label} accessibilityRole="header">
          Preview
        </Text>
        <View style={s.preview} onLayout={(e) => setPreviewWidth(e.nativeEvent.layout.width - 2 * PREVIEW_BORDER)}>
          {previewWidth > 0 && (
            <DrawingToolbox
              preview
              settings={settings}
              placement={{ ...DEFAULT_TOOLBOX_PHONE, orientation: "horizontal" }}
              onPlacementChange={noop}
              container={{ width: previewWidth, height: 200 }}
              tool={null}
              onToolChange={noop}
              canColor
              onColorChange={noop}
              canDelete
              onDelete={noop}
              canUndo
              canRedo
              onUndo={noop}
              onRedo={noop}
              haptics={false}
            />
          )}
        </View>
      </View>
      {GROUPS.map((g) => {
        const items = TOOLBOX_ITEMS.filter((i) => i.group === g.id);
        const shown = items.filter((i) => isToolboxItemVisible(settings, i.id)).length;
        const allShown = items.every((i) => isToolboxItemVisible(settings, i.id));
        return (
          <View key={g.id} style={{ gap: 2 }}>
            <View style={s.groupHead}>
              <Text style={s.label} accessibilityRole="header">
                {g.label}
              </Text>
              <Text style={s.count}>
                {shown}/{items.length}
              </Text>
              <Pressable accessibilityRole="button" accessibilityLabel={`${allShown ? "Hide" : "Show"} all ${g.label.toLowerCase()}`} onPress={() => onChange(setToolboxGroupVisible(settings, g.id, !allShown))} hitSlop={8} style={({ pressed }) => [s.textButton, pressed && { opacity: 0.6 }]}>
                <Text style={s.textButtonText}>{allShown ? "Hide all" : "Show all"}</Text>
              </Pressable>
            </View>
            {items.map((item) => (
              <ItemRow key={item.id} item={item} on={isToolboxItemVisible(settings, item.id)} onToggle={() => onChange(setToolboxItemVisible(settings, item.id, !isToolboxItemVisible(settings, item.id)))} reducedMotion={reducedMotion} />
            ))}
          </View>
        );
      })}
      <Text style={s.note}>Hiding a tool only removes its button. Drawings already on the chart stay, and can still be selected, moved and deleted. A tool you hide while it is armed goes back to the cursor.</Text>
    </ScrollView>
  );
}

function ItemRow({ item, on, onToggle, reducedMotion }: { item: ToolboxItemInfo; on: boolean; onToggle: () => void; reducedMotion?: boolean }) {
  const locked = !!item.locked;
  return (
    <Pressable
      accessibilityRole="switch"
      accessibilityState={{ checked: on, disabled: locked }}
      accessibilityLabel={`Show ${item.name}`}
      accessibilityHint={locked ? item.locked : item.description}
      disabled={locked}
      onPress={onToggle}
      style={({ pressed }) => [s.row, pressed && { backgroundColor: UI.row }]}
    >
      <View style={[s.tile, !on && { opacity: 0.5 }]}>
        <Icon name={item.icon} color={on ? UI.accent : UI.textDim} size={16} />
      </View>
      <View style={{ flex: 1, gap: 2 }}>
        <Text style={[s.name, !on && { color: UI.textDim }]} numberOfLines={1}>
          {item.name}
        </Text>
        <Text style={s.desc} numberOfLines={2}>
          {locked ? item.locked : item.description}
        </Text>
      </View>
      <Toggle on={on} disabled={locked} reducedMotion={reducedMotion} />
    </Pressable>
  );
}

const TRACK_W = 34;
const TRACK_H = 20;
const THUMB = 16;

/**
 * The switch, drawn rather than RN's Switch so it looks and moves the same
 * on iOS and Android (workspace rule: one motion vocabulary). Neutral, like
 * the web's ui/switch.tsx: on is a light track with a dark thumb, off a
 * raised track with a dim one; the accent stays on the active tab.
 */
function Toggle({ on, disabled, reducedMotion }: { on: boolean; disabled?: boolean; reducedMotion?: boolean }) {
  const v = useSharedValue(on ? 1 : 0);
  useEffect(() => {
    v.value = withTiming(on ? 1 : 0, { duration: reducedMotion ? 0 : 150 });
  }, [on, reducedMotion, v]);
  const track = useAnimatedStyle(() => ({ backgroundColor: interpolateColor(v.value, [0, 1], [UI.raised, UI.text]) }));
  const thumb = useAnimatedStyle(() => ({ transform: [{ translateX: v.value * (TRACK_W - THUMB - 4) }], backgroundColor: interpolateColor(v.value, [0, 1], [UI.textDim, UI.bg]) }));
  return (
    <Animated.View style={[s.track, track, disabled && { opacity: 0.5 }]} importantForAccessibility="no-hide-descendants" accessibilityElementsHidden>
      <Animated.View style={[s.thumb, thumb]} />
    </Animated.View>
  );
}

/** Reset for the panel's heading; dimmed (not removed) when already default, so the heading never shifts. */
export function DrawingSettingsReset({ settings, onChange }: { settings: DrawingSettings; onChange: (s: DrawingSettings) => void }) {
  const isDefault = isDefaultDrawingSettings(settings);
  return (
    <Pressable accessibilityRole="button" accessibilityLabel="Reset drawing settings to defaults" accessibilityState={{ disabled: isDefault }} disabled={isDefault} onPress={() => onChange({ ...DEFAULT_DRAWING_SETTINGS, hidden: [] })} hitSlop={8} style={({ pressed }) => [s.reset, isDefault && { opacity: 0.4 }, pressed && { backgroundColor: UI.raised }]}>
      <Text style={s.textButtonText}>Reset</Text>
    </Pressable>
  );
}

/** The Drawing tab as a SettingsSheet section: chart-core's label, icon and description, this panel and its reset. */
export function drawingSettingsSection(settings: DrawingSettings, onChange: (s: DrawingSettings) => void, reducedMotion?: boolean): SettingsSheetSection {
  const info = SETTINGS_SECTIONS.find((x) => x.id === "drawing")!;
  return { ...info, action: <DrawingSettingsReset settings={settings} onChange={onChange} />, render: () => <DrawingSettingsPanel settings={settings} onChange={onChange} reducedMotion={reducedMotion} /> };
}

const s = StyleSheet.create({
  label: { color: UI.textFaint, fontSize: 11, paddingHorizontal: 4 },
  preview: { minHeight: 56, alignItems: "center", justifyContent: "center" },
  groupHead: { flexDirection: "row", alignItems: "center", gap: 6, paddingBottom: 2 },
  count: { color: UI.textFaint, fontSize: 11, fontVariant: ["tabular-nums"] },
  textButton: { marginLeft: "auto", paddingHorizontal: 4, paddingVertical: 2 },
  textButtonText: { color: UI.textDim, fontSize: 12 },
  row: { minHeight: 52, flexDirection: "row", alignItems: "center", gap: 10, paddingHorizontal: 4, paddingVertical: 6, borderRadius: 10 },
  tile: { width: 32, height: 32, borderRadius: 9, backgroundColor: UI.row, alignItems: "center", justifyContent: "center" },
  name: { color: UI.text, fontSize: 14, fontWeight: "500" },
  desc: { color: UI.textFaint, fontSize: 11 },
  note: { color: UI.textFaint, fontSize: 11, lineHeight: 16, paddingHorizontal: 4 },
  track: { width: TRACK_W, height: TRACK_H, borderRadius: TRACK_H / 2, padding: 2, justifyContent: "center" },
  thumb: { width: THUMB, height: THUMB, borderRadius: THUMB / 2 },
  reset: { backgroundColor: UI.row, borderRadius: 8, paddingHorizontal: 10, paddingVertical: 5 },
});

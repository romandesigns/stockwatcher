import type { DrawingKind, HapticStyle } from "@stockwatcher/chart-core";
import * as Haptics from "expo-haptics";
import type { ReactNode } from "react";
import { Pressable, StyleSheet, Text, View, type StyleProp, type ViewStyle } from "react-native";
import Svg, { Circle, Path, Rect } from "react-native-svg";

// The chart's own native UI pieces (toolbar buttons, segmented controls,
// icons) and its haptics. They live in the chart package so every screen
// that shows a chart gets the same controls. Colours mirror the app palette
// (60/30/10; no borders, boundaries by surface contrast) and can be
// overridden per chart.

export const UI = {
  bg: "#0f1216",
  panel: "#151a20",
  row: "#1c222a",
  raised: "#272e38",
  text: "#e4e8ed",
  textDim: "#98a2ae",
  textFaint: "#66707c",
  accent: "#8b9cff",
  accentFg: "#0f1216",
  good: "#3fb97a",
  critical: "#f0645c",
  warning: "#e0a63a",
} as const;

/** Plays a chart haptic. Fire and forget: a device without a haptic engine just stays quiet. */
export function playHaptic(style: HapticStyle | "success"): void {
  const p =
    style === "selection"
      ? Haptics.selectionAsync()
      : style === "success"
        ? Haptics.notificationAsync(Haptics.NotificationFeedbackType.Success)
        : Haptics.impactAsync(style === "medium" ? Haptics.ImpactFeedbackStyle.Medium : Haptics.ImpactFeedbackStyle.Light);
  p.catch(() => {});
}

const icon = (children: ReactNode, color: string) => (
  <Svg width={16} height={16} viewBox="0 0 16 16" fill="none" stroke={color} strokeWidth={1.5} strokeLinecap="round">
    {children}
  </Svg>
);

/** The same 16 px icons as chart-web's toolbar. */
export function ToolIcon({ kind, color }: { kind: DrawingKind | "cursor" | "trash" | "indicators" | "levels" | "trades" | "undo" | "redo"; color: string }) {
  switch (kind) {
    case "cursor":
      return icon(<Path d="M4 2.5v10l3-3 2 4.5 1.6-.7-2-4.4H13z" fill={color} stroke="none" />, color);
    case "trend":
      return icon(
        <>
          <Path d="M3 12.5 13 3.5" />
          <Circle cx={3} cy={12.5} r={1.3} fill={color} />
          <Circle cx={13} cy={3.5} r={1.3} fill={color} />
        </>,
        color,
      );
    case "ray":
      return icon(
        <>
          <Path d="M3 12.5 15 2" />
          <Circle cx={3} cy={12.5} r={1.3} fill={color} />
        </>,
        color,
      );
    case "hline":
      return icon(<Path d="M1.5 8h13" />, color);
    case "vline":
      return icon(<Path d="M8 1.5v13" />, color);
    case "rect":
      return icon(<Rect x={3} y={4} width={10} height={8} rx={1} fill={color} fillOpacity={0.35} stroke="none" />, color);
    case "long":
      return icon(
        <>
          <Rect x={3} y={2} width={10} height={6} rx={1} fill={UI.good} fillOpacity={0.55} stroke="none" />
          <Rect x={3} y={8} width={10} height={5} rx={1} fill={UI.critical} fillOpacity={0.55} stroke="none" />
          <Path d="M2 8h12" />
        </>,
        color,
      );
    case "short":
      return icon(
        <>
          <Rect x={3} y={3} width={10} height={5} rx={1} fill={UI.critical} fillOpacity={0.55} stroke="none" />
          <Rect x={3} y={8} width={10} height={6} rx={1} fill={UI.good} fillOpacity={0.55} stroke="none" />
          <Path d="M2 8h12" />
        </>,
        color,
      );
    case "indicators":
      return icon(
        <>
          <Path d="M1.5 12.5 4.5 9l2.5 2 3.5-5 4 3" strokeOpacity={0.45} />
          <Path d="M1.5 9.5c2.5-4 4.5-5 6.5-3s4 1.5 6.5-3" />
        </>,
        color,
      );
    case "levels":
      return icon(
        <>
          <Path d="M1.5 4h13" strokeDasharray="2 2" />
          <Path d="M1.5 8h13" />
          <Path d="M1.5 12h13" strokeOpacity={0.5} />
        </>,
        color,
      );
    case "trades":
      return icon(
        <>
          <Path d="M6.5 4h8M6.5 8h8M6.5 12h8" strokeOpacity={0.6} />
          <Path d="M2 5.5 3.5 3 5 5.5z" fill={color} stroke="none" />
          <Path d="M2 10.5 3.5 13 5 10.5z" fill={color} stroke="none" />
        </>,
        color,
      );
    case "measure":
      return icon(
        <>
          <Path d="M2 11.5 11.5 2l2.5 2.5L4.5 14z" />
          <Path d="M5 9.5l1 1M7 7.5l1.5 1.5M9 5.5l1 1" />
        </>,
        color,
      );
    case "undo":
      return icon(<Path d="M5.5 3 2.5 6l3 3M2.5 6h7a4 4 0 0 1 0 8h-2" />, color);
    case "redo":
      return icon(<Path d="M10.5 3l3 3-3 3M13.5 6h-7a4 4 0 0 0 0 8h2" />, color);
    case "trash":
      return icon(<Path d="M3 4.5h10M6.5 4.5V3h3v1.5M4.5 4.5l.6 8.5h5.8l.6-8.5" />, color);
  }
}

export interface SegmentedProps<T extends string> {
  options: readonly { value: T; label?: string; icon?: ReactNode; a11y: string; tint?: string }[];
  value: T;
  onChange: (v: T) => void;
  style?: StyleProp<ViewStyle>;
  /** Stretch the options to fill the width. */
  fill?: boolean;
}

/** Segmented control: a panel track, the selected option raised one surface step. */
export function Segmented<T extends string>({ options, value, onChange, style, fill }: SegmentedProps<T>) {
  return (
    <View style={[styles.track, style]} accessibilityRole="radiogroup">
      {options.map((o) => {
        const on = o.value === value;
        return (
          <Pressable
            key={o.value}
            accessibilityRole="radio"
            accessibilityState={{ selected: on }}
            accessibilityLabel={o.a11y}
            onPress={() => onChange(o.value)}
            hitSlop={4}
            style={[styles.segment, fill && styles.grow, on && styles.segmentOn]}
          >
            {o.icon}
            {o.label !== undefined && <Text style={[styles.segmentText, { color: on ? (o.tint ?? UI.text) : UI.textDim }]}>{o.label}</Text>}
          </Pressable>
        );
      })}
    </View>
  );
}

export function ToolButton({ onPress, label, children, disabled, primary, style }: { onPress: () => void; label: string; children: ReactNode; disabled?: boolean; primary?: boolean; style?: StyleProp<ViewStyle> }) {
  return (
    <Pressable
      accessibilityRole="button"
      accessibilityLabel={label}
      accessibilityState={{ disabled: !!disabled }}
      disabled={disabled}
      onPress={onPress}
      hitSlop={4}
      style={({ pressed }) => [styles.button, primary && styles.primary, pressed && !primary && styles.pressed, disabled && styles.disabled, style]}
    >
      {typeof children === "string" ? <Text style={[styles.buttonText, primary && { color: UI.accentFg, fontWeight: "600" }]}>{children}</Text> : children}
    </Pressable>
  );
}

export const styles = StyleSheet.create({
  track: { flexDirection: "row", backgroundColor: UI.panel, borderRadius: 8, padding: 2, gap: 2 },
  segment: { minWidth: 32, height: 30, paddingHorizontal: 8, borderRadius: 6, alignItems: "center", justifyContent: "center", flexDirection: "row", gap: 4 },
  grow: { flex: 1 },
  segmentOn: { backgroundColor: UI.raised },
  segmentText: { fontSize: 12, fontWeight: "500" },
  button: { height: 30, minWidth: 30, paddingHorizontal: 8, borderRadius: 6, alignItems: "center", justifyContent: "center", backgroundColor: "transparent" },
  primary: { backgroundColor: UI.accent, paddingHorizontal: 12 },
  pressed: { backgroundColor: UI.row },
  disabled: { opacity: 0.4 },
  buttonText: { color: UI.textDim, fontSize: 12, fontWeight: "500" },
});

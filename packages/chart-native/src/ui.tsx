import { ICONS, iconPaint, type DrawingKind, type HapticStyle, type IconDef, type IconName } from "@stockwatcher/chart-core";
import * as Haptics from "expo-haptics";
import { useEffect, useRef, useState, type ReactNode } from "react";
import { Pressable, StyleSheet, Text, View, type StyleProp, type ViewStyle } from "react-native";
import Svg, { Path } from "react-native-svg";

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

const HUES = { good: UI.good, critical: UI.critical, warning: UI.warning } as const;

export interface IconProps {
  /** A named icon, or any duotone definition (a layer swatch, an indicator glyph). */
  name?: IconName;
  def?: IconDef;
  /** The primary tone: the state's text colour (UI.textDim idle, UI.text raised, UI.accent where a state spends it). */
  color: string;
  /** Rendered width in px (height follows the viewBox's aspect); default the viewBox size. */
  size?: number;
}

/**
 * chart-core's duotone icons (icons.ts) with react-native-svg: the same
 * definitions chart-web draws, so the phone and the web show the same marks
 * in the same two tones. A path's role picks the tone (iconPaint): primary
 * is `color`, secondary the same colour at ICON_SECONDARY_OPACITY, which is
 * what the web's --icon-secondary color-mix spells.
 */
export function Icon({ name, def, color, size }: IconProps) {
  const d = def ?? (name ? ICONS[name] : undefined);
  if (!d) return null;
  const w = size ?? d.w;
  return (
    <Svg width={w} height={(w * d.h) / d.w} viewBox={`0 0 ${d.w} ${d.h}`} fill="none" strokeLinecap="round" strokeLinejoin="round">
      {d.paths.map((p, i) => {
        const paint = iconPaint(p, color, HUES);
        return p.paint === "fill" ? (
          <Path key={i} d={p.d} fill={paint.color} fillOpacity={paint.opacity} stroke="none" />
        ) : (
          <Path key={i} d={p.d} fill="none" stroke={paint.color} strokeOpacity={paint.opacity} strokeWidth={p.width ?? d.strokeWidth} strokeDasharray={p.dash ? [...p.dash] : undefined} />
        );
      })}
    </Svg>
  );
}

/** The toolbar icons by name (kept for callers of the old per-kind switch); the same 16 px marks as chart-web's toolbar. */
export function ToolIcon({ kind, color }: { kind: DrawingKind | "cursor" | "trash" | "indicators" | "levels" | "trades" | "undo" | "redo" | "pencil" | "trade"; color: string }) {
  return <Icon name={kind} color={color} />;
}

/**
 * An armed drawing tool's icon colour: accent (both tones), because drawing is
 * a mode the next tap acts on; the cursor is the resting state and stays
 * neutral, so the accent stays rare. Same rule as chart-web's ARMED.
 */
export const toolColor = (kind: DrawingKind | "cursor", on: boolean) => (on ? (kind === "cursor" ? UI.text : UI.accent) : UI.textDim);

export interface SegmentedProps<T extends string> {
  /** tint colours the selected label; activeBg replaces the selected segment's raised surface (Buy/Sell). */
  options: readonly { value: T; label?: string; icon?: ReactNode; a11y: string; tint?: string; activeBg?: string }[];
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
            style={[styles.segment, fill && styles.grow, on && styles.segmentOn, on && o.activeBg ? { backgroundColor: o.activeBg } : null]}
          >
            {o.icon}
            {o.label !== undefined && <Text style={[styles.segmentText, { color: on ? (o.tint ?? UI.text) : UI.textDim }]}>{o.label}</Text>}
          </Pressable>
        );
      })}
    </View>
  );
}

/**
 * A toolbar button. Touch has no hover tooltips, so a long press shows the
 * button's name in a small bubble for a moment (with a selection haptic):
 * the phone's version of the web's shadcn tooltips. Screen readers get the
 * same name as the accessible label.
 */
export function ToolButton({ onPress, label, children, disabled, primary, style, hint = true }: { onPress: () => void; label: string; children: ReactNode; disabled?: boolean; primary?: boolean; style?: StyleProp<ViewStyle>; hint?: boolean }) {
  const [showHint, setShowHint] = useState(false);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  useEffect(() => () => {
    if (timer.current) clearTimeout(timer.current);
  }, []);
  // The wrapper (which anchors the hint) takes the button's place in the row, so layout props
  // given for the button (flex, margins, alignment) apply to it; the button fills it.
  const flat = StyleSheet.flatten(style) ?? {};
  const outer: ViewStyle = { flex: flat.flex, flexGrow: flat.flexGrow, alignSelf: flat.alignSelf, margin: flat.margin, marginLeft: flat.marginLeft, marginRight: flat.marginRight, marginTop: flat.marginTop, marginBottom: flat.marginBottom, marginHorizontal: flat.marginHorizontal };
  const inner: ViewStyle = { ...flat, flex: undefined, flexGrow: undefined, alignSelf: undefined, margin: undefined, marginLeft: undefined, marginRight: undefined, marginTop: undefined, marginBottom: undefined, marginHorizontal: undefined };
  return (
    <View style={[styles.hintWrap, outer]}>
      <Pressable
        accessibilityRole="button"
        accessibilityLabel={label}
        accessibilityState={{ disabled: !!disabled }}
        disabled={disabled}
        onPress={onPress}
        onLongPress={
          hint
            ? () => {
                playHaptic("selection");
                setShowHint(true);
                if (timer.current) clearTimeout(timer.current);
                timer.current = setTimeout(() => setShowHint(false), 1400);
              }
            : undefined
        }
        delayLongPress={350}
        hitSlop={4}
        style={({ pressed }) => [styles.button, primary && styles.primary, pressed && !primary && styles.pressed, disabled && styles.disabled, inner]}
      >
        {typeof children === "string" ? <Text style={[styles.buttonText, primary && { color: UI.accentFg, fontWeight: "600" }]}>{children}</Text> : children}
      </Pressable>
      {showHint && (
        <View pointerEvents="none" style={styles.hint} accessibilityElementsHidden importantForAccessibility="no-hide-descendants">
          <Text style={styles.hintText} numberOfLines={1}>
            {label}
          </Text>
        </View>
      )}
    </View>
  );
}

export const styles = StyleSheet.create({
  // Recessed: segmented controls sit in sheets (the panel colour), so the track is the darker base.
  track: { flexDirection: "row", backgroundColor: UI.bg, borderRadius: 10, padding: 3, gap: 2 },
  segment: { minWidth: 32, height: 32, paddingHorizontal: 10, borderRadius: 8, alignItems: "center", justifyContent: "center", flexDirection: "row", gap: 4 },
  grow: { flex: 1 },
  segmentOn: { backgroundColor: UI.raised },
  segmentText: { fontSize: 12, fontWeight: "500" },
  button: { height: 30, minWidth: 30, paddingHorizontal: 8, borderRadius: 6, alignItems: "center", justifyContent: "center", backgroundColor: "transparent" },
  primary: { backgroundColor: UI.accent, paddingHorizontal: 12 },
  pressed: { backgroundColor: UI.row },
  disabled: { opacity: 0.4 },
  buttonText: { color: UI.textDim, fontSize: 12, fontWeight: "500" },
  hintWrap: { position: "relative", zIndex: 1 },
  hint: { position: "absolute", top: 36, right: 0, zIndex: 20, elevation: 8, backgroundColor: UI.raised, borderRadius: 6, paddingHorizontal: 8, paddingVertical: 4, minWidth: 64 },
  hintText: { color: UI.text, fontSize: 11 },
});

import { DRAWING_COLORS, DRAWING_KINDS, DRAWING_LABELS, toolboxPlacement, toolboxPosition, type DrawingKind, type Size, type ToolboxPlacement } from "@stockwatcher/chart-core";
import { useRef, useState } from "react";
import { Pressable, StyleSheet, View, type LayoutChangeEvent } from "react-native";
import { GestureDetector, usePanGesture } from "react-native-gesture-handler";
import Svg, { Circle, Path } from "react-native-svg";
import { playHaptic, ToolIcon, UI } from "./ui.tsx";

export interface DrawingToolboxProps {
  placement: ToolboxPlacement;
  /** Live while dragging; `done` once the finger lifts (persist then). */
  onPlacementChange: (p: ToolboxPlacement, done: boolean) => void;
  container: Size;
  tool: DrawingKind | null;
  onToolChange: (tool: DrawingKind | null) => void;
  color?: string | null;
  canColor: boolean;
  onColorChange: (color: string) => void;
  canDelete: boolean;
  onDelete: () => void;
  canUndo: boolean;
  canRedo: boolean;
  onUndo: () => void;
  onRedo: () => void;
  haptics?: boolean;
}

const BTN = 30;

/**
 * The drawing tools as a floating palette over the native chart, the twin of
 * chart-web's DrawingToolbox: drag it by the grip anywhere inside the chart
 * (it never leaves it), flip it between a row and a column, pick the
 * selected drawing's colour (the swatches open inside the palette), delete.
 * Its place is stored as fractions of the room it can move in (chart-core
 * toolbox.ts), so it lands in the same spot on any screen size.
 */
export function DrawingToolbox({ placement, onPlacementChange, container, tool, onToolChange, color, canColor, onColorChange, canDelete, onDelete, canUndo, canRedo, onUndo, onRedo, haptics = true }: DrawingToolboxProps) {
  const [box, setBox] = useState<Size>({ width: 300, height: 40 });
  const [colorsOpen, setColorsOpen] = useState(false);
  const start = useRef<{ left: number; top: number } | null>(null);
  const vertical = placement.orientation === "vertical";
  const at = toolboxPosition(placement, container, box);

  const onLayout = (e: LayoutChangeEvent) => {
    const { width, height } = e.nativeEvent.layout;
    setBox((b) => (b.width === width && b.height === height ? b : { width, height }));
  };

  const pan = usePanGesture({
    runOnJS: true,
    disableReanimated: true,
    minDistance: 2,
    onActivate: () => {
      start.current = { left: at.left, top: at.top };
      if (haptics) playHaptic("light");
    },
    onUpdate: (e) => {
      const s = start.current;
      if (s) onPlacementChange(toolboxPlacement(s.left + e.translationX, s.top + e.translationY, container, box, placement.orientation), false);
    },
    onDeactivate: (e) => {
      const s = start.current;
      start.current = null;
      if (s) onPlacementChange(toolboxPlacement(s.left + e.translationX, s.top + e.translationY, container, box, placement.orientation), true);
    },
  });

  const swatch = (c: string, size: number) => <View style={{ width: size, height: size, borderRadius: size / 2, backgroundColor: c }} />;
  const current = color ?? DRAWING_COLORS[0]!.value;

  return (
    // Wraps to a second row (or column) when the chart is too small for one, so it never spills out.
    <View
      style={[s.box, { left: at.left, top: at.top, flexDirection: vertical ? "column" : "row", flexWrap: "wrap" }, vertical ? { maxHeight: Math.max(BTN * 3, container.height - 12) } : { maxWidth: Math.max(BTN * 3, container.width - 12) }]}
      onLayout={onLayout}
      accessibilityRole="toolbar"
      accessibilityLabel="Drawing tools"
    >
      <GestureDetector gesture={pan}>
        <View style={[s.grip, vertical ? { width: BTN, height: 18 } : { width: 18, height: BTN }]} accessible accessibilityLabel="Move drawing tools" accessibilityHint="Drag to move the drawing tools">
          <Svg width={14} height={14} viewBox="0 0 16 16" style={vertical ? undefined : { transform: [{ rotate: "90deg" }] }}>
            {[4, 8, 12].flatMap((y) => [6, 10].map((x) => <Circle key={`${x}-${y}`} cx={x} cy={y} r={1.1} fill={UI.textFaint} />))}
          </Svg>
        </View>
      </GestureDetector>
      {(["cursor", ...DRAWING_KINDS, "measure"] as const).map((k) => {
        const on = (tool ?? "cursor") === k;
        return (
          <Pressable
            key={k}
            accessibilityRole="radio"
            accessibilityState={{ selected: on }}
            accessibilityLabel={k === "cursor" ? "Cursor" : DRAWING_LABELS[k]}
            onPress={() => onToolChange(k === "cursor" ? null : k)}
            style={[s.btn, on && { backgroundColor: UI.raised }]}
          >
            <ToolIcon kind={k} color={on ? UI.text : UI.textDim} />
          </Pressable>
        );
      })}
      <View style={vertical ? s.sepV : s.sepH} />
      <Pressable accessibilityRole="button" accessibilityLabel="Drawing colour" accessibilityState={{ disabled: !canColor, expanded: colorsOpen }} disabled={!canColor} onPress={() => setColorsOpen((o) => !o)} style={[s.btn, !canColor && { opacity: 0.4 }]}>
        {swatch(canColor ? current : UI.textDim, 14)}
      </Pressable>
      {colorsOpen && canColor && (
        <View style={[s.colors, { flexDirection: vertical ? "column" : "row" }]} accessibilityRole="radiogroup" accessibilityLabel="Drawing colour">
          {DRAWING_COLORS.map((c) => (
            <Pressable
              key={c.value}
              accessibilityRole="radio"
              accessibilityState={{ checked: current === c.value }}
              accessibilityLabel={c.name}
              onPress={() => {
                onColorChange(c.value);
                setColorsOpen(false);
                if (haptics) playHaptic("selection");
              }}
              style={[s.btn, current === c.value && { backgroundColor: UI.raised }]}
            >
              {swatch(c.value, 16)}
            </Pressable>
          ))}
        </View>
      )}
      <Pressable accessibilityRole="button" accessibilityLabel="Delete selected drawing" disabled={!canDelete} onPress={onDelete} style={[s.btn, !canDelete && { opacity: 0.4 }]}>
        <ToolIcon kind="trash" color={UI.textDim} />
      </Pressable>
      <Pressable accessibilityRole="button" accessibilityLabel="Undo" disabled={!canUndo} onPress={onUndo} style={[s.btn, !canUndo && { opacity: 0.4 }]}>
        <ToolIcon kind="undo" color={UI.textDim} />
      </Pressable>
      <Pressable accessibilityRole="button" accessibilityLabel="Redo" disabled={!canRedo} onPress={onRedo} style={[s.btn, !canRedo && { opacity: 0.4 }]}>
        <ToolIcon kind="redo" color={UI.textDim} />
      </Pressable>
      <Pressable
        accessibilityRole="button"
        accessibilityLabel={vertical ? "Lay the tools out in a row" : "Lay the tools out in a column"}
        onPress={() => onPlacementChange({ ...placement, orientation: vertical ? "horizontal" : "vertical" }, true)}
        style={s.btn}
      >
        <Svg width={15} height={15} viewBox="0 0 16 16" fill="none" stroke={UI.textDim} strokeWidth={1.5} strokeLinecap="round">
          <Path d="M3 8a5 5 0 0 1 8.5-3.5L13 6M13 3v3h-3M13 8a5 5 0 0 1-8.5 3.5L3 10M3 13v-3h3" />
        </Svg>
      </Pressable>
    </View>
  );
}

const s = StyleSheet.create({
  box: { position: "absolute", alignItems: "center", gap: 2, padding: 3, borderRadius: 12, backgroundColor: "rgba(21,26,32,0.95)", shadowColor: "#000", shadowOpacity: 0.35, shadowRadius: 8, shadowOffset: { width: 0, height: 2 }, elevation: 6 },
  grip: { alignItems: "center", justifyContent: "center" },
  btn: { width: BTN, height: BTN, borderRadius: 7, alignItems: "center", justifyContent: "center" },
  sepV: { width: 18, height: 1, backgroundColor: UI.row, marginVertical: 2 },
  sepH: { width: 1, height: 18, backgroundColor: UI.row, marginHorizontal: 2 },
  colors: { gap: 2, padding: 2, borderRadius: 9, backgroundColor: UI.row },
});

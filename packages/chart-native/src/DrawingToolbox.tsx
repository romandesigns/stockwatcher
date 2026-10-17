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
const GAP = 2;
const PAD = 3;
const GRIP = 18;
/** Tools: cursor, the drawing kinds and measure. Actions: colour, delete, undo, redo, flip. */
const TOOL_KEYS = ["cursor", ...DRAWING_KINDS, "measure"] as const;
const ACTIONS = 5;

/** Length of one line holding `n` buttons plus the grip and a separator. */
const lineLength = (n: number) => PAD * 2 + GRIP + GAP + n * BTN + (n - 1) * GAP + 5 + GAP;

/**
 * The drawing tools as a floating palette over the native chart, the twin of
 * chart-web's DrawingToolbox: drag it by the grip anywhere inside the chart
 * (it never leaves it), flip it between a row and a column, pick the
 * selected drawing's colour (the swatches open inside the palette), delete.
 * Its place is stored as fractions of the room it can move in (chart-core
 * toolbox.ts), so it lands in the same spot on any screen size.
 *
 * A phone is too narrow for every button in one row (about 500 pt against
 * ~370), and letting the row wrap stranded a few buttons under the grip.
 * So when one line does not fit, the palette splits on purpose: the tools on
 * the first line, the actions (colour, delete | undo, redo, flip) on the
 * second, spread to the same length, and the grip spans both. A column
 * splits the same way into two columns.
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
  const room = (vertical ? container.height : container.width) - 12;
  const split = room > 0 && lineLength(TOOL_KEYS.length + ACTIONS) > room;
  // Lines run along the orientation; the box stacks them across it.
  const line = vertical ? "column" : "row";
  const across = vertical ? "row" : "column";

  return (
    <View style={[s.box, { left: at.left, top: at.top, flexDirection: line }]} onLayout={onLayout} accessibilityRole="toolbar" accessibilityLabel="Drawing tools">
      <GestureDetector gesture={pan}>
        {/* Stretches across both lines when split, so there is one grip for the whole palette. */}
        <View style={[s.grip, vertical ? { width: "100%", height: GRIP } : { width: GRIP, alignSelf: "stretch", minHeight: BTN }]} accessible accessibilityLabel="Move drawing tools" accessibilityHint="Drag to move the drawing tools">
          <Svg width={14} height={14} viewBox="0 0 16 16" style={vertical ? undefined : { transform: [{ rotate: "90deg" }] }}>
            {[4, 8, 12].flatMap((y) => [6, 10].map((x) => <Circle key={`${x}-${y}`} cx={x} cy={y} r={1.1} fill={UI.textFaint} />))}
          </Svg>
        </View>
      </GestureDetector>
      <View style={{ flexDirection: split ? across : line, gap: GAP }}>
      <View style={{ flexDirection: line, alignItems: "center", gap: GAP }} accessibilityRole="radiogroup" accessibilityLabel="Tool">
      {TOOL_KEYS.map((k) => {
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
      </View>
      {!split && <View style={vertical ? s.sepV : s.sepH} />}
      {/* Actions: colour and delete act on the selection, then history and layout, pushed apart when split. */}
      <View style={{ flexDirection: line, alignItems: "center", gap: GAP, justifyContent: "space-between" }}>
      <View style={{ flexDirection: line, alignItems: "center", gap: GAP }}>
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
      </View>
      <View style={{ flexDirection: line, alignItems: "center", gap: GAP }}>
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
      </View>
      </View>
    </View>
  );
}

const s = StyleSheet.create({
  box: { position: "absolute", alignItems: "center", gap: GAP, padding: PAD, borderRadius: 12, backgroundColor: "rgba(21,26,32,0.95)", shadowColor: "#000", shadowOpacity: 0.35, shadowRadius: 8, shadowOffset: { width: 0, height: 2 }, elevation: 6 },
  grip: { alignItems: "center", justifyContent: "center" },
  btn: { width: BTN, height: BTN, borderRadius: 7, alignItems: "center", justifyContent: "center" },
  sepV: { width: 18, height: 1, backgroundColor: UI.row, marginVertical: 2 },
  sepH: { width: 1, height: 18, backgroundColor: UI.row, marginHorizontal: 2 },
  colors: { gap: 2, padding: 2, borderRadius: 9, backgroundColor: UI.row },
});

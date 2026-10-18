import { DEFAULT_DRAWING_SETTINGS, DRAWING_COLORS, DRAWING_LABELS, toolboxLayout, toolboxPlacement, toolboxPosition, type DrawingKind, type DrawingSettings, type Size, type ToolboxPlacement } from "@stockwatcher/chart-core";
import { useRef, useState, type ReactNode } from "react";
import { Pressable, StyleSheet, View, type LayoutChangeEvent } from "react-native";
import { GestureDetector, usePanGesture } from "react-native-gesture-handler";
import { Icon, playHaptic, toolColor, ToolIcon, UI } from "./ui.tsx";

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
  /** Which buttons show (the Settings sheet's Drawing tab; chart-core drawingSettings.ts). Default: all. */
  settings?: DrawingSettings;
  /**
   * Laid out in the flow, not floating, and not interactive: the Drawing
   * tab's live preview, so it is the real toolbox (including the two-line
   * split when its line does not fit `container`).
   */
  preview?: boolean;
}

const BTN = 30;
const GAP = 2;
const PAD = 3;
const GRIP = 18;
/** Length of one line holding `n` buttons, plus the grip and the separator when they show. */
const lineLength = (n: number, grip: boolean, separator: boolean) => PAD * 2 + (grip ? GRIP + GAP : 0) + n * BTN + (n - 1) * GAP + (separator ? 5 + GAP : 0);

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
 * the first line, the actions (colour, then delete, undo, redo and flip in
 * one pill) on the second, and the grip spans both. A column
 * splits the same way into two columns.
 *
 * Which buttons show comes from `settings` through chart-core's
 * toolboxLayout (owner request 2026-10-18), the derivation chart-web uses:
 * hidden tools drop out, the edit pill holds only what is left of delete /
 * undo / redo / flip and is not drawn when that is nothing, the separator
 * only shows when an action follows it, and the split rule measures the
 * line that is actually left (fewer buttons often fit on one line again).
 * Buttons are added and removed without a layout animation: the Settings
 * sheet that changes them covers the chart while it is open, so the change
 * is never seen happening, and Reanimated layout transitions on button
 * rows left stale touch targets on Android before (see the workspace's
 * motion notes).
 */
export function DrawingToolbox({ placement, onPlacementChange, container, tool, onToolChange, color, canColor, onColorChange, canDelete, onDelete, canUndo, canRedo, onUndo, onRedo, haptics = true, settings = DEFAULT_DRAWING_SETTINGS, preview = false }: DrawingToolboxProps) {
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
  const layout = toolboxLayout(settings);
  const actions = layout.color || layout.edit.length > 0;
  const room = (vertical ? container.height : container.width) - (preview ? 0 : 12);
  // Split only when there are actions to move to a second line; a tools-only strip just wraps nowhere.
  const split = actions && room > 0 && lineLength(layout.buttons, layout.grip, layout.separator) > room;
  // Lines run along the orientation; the box stacks them across it.
  const line = vertical ? "column" : "row";
  const across = vertical ? "row" : "column";

  const gripEl = (
    // Stretches across both lines when split, so there is one grip for the whole palette.
    <View style={[s.grip, vertical ? { width: "100%", height: GRIP } : { width: GRIP, alignSelf: "stretch", minHeight: BTN }]} accessible accessibilityLabel="Move drawing tools" accessibilityHint="Drag to move the drawing tools">
      <View style={vertical ? undefined : { transform: [{ rotate: "90deg" }] }}>
        <Icon name="grip" color={UI.textFaint} size={14} />
      </View>
    </View>
  );
  const editButtons: Record<(typeof layout.edit)[number], ReactNode> = {
    delete: (
      <Pressable key="delete" accessibilityRole="button" accessibilityLabel="Delete selected drawing" disabled={!canDelete} onPress={onDelete} style={[s.btn, !canDelete && { opacity: 0.4 }]}>
        <ToolIcon kind="trash" color={UI.textDim} />
      </Pressable>
    ),
    undo: (
      <Pressable key="undo" accessibilityRole="button" accessibilityLabel="Undo" disabled={!canUndo} onPress={onUndo} style={[s.btn, !canUndo && { opacity: 0.4 }]}>
        <ToolIcon kind="undo" color={UI.textDim} />
      </Pressable>
    ),
    redo: (
      <Pressable key="redo" accessibilityRole="button" accessibilityLabel="Redo" disabled={!canRedo} onPress={onRedo} style={[s.btn, !canRedo && { opacity: 0.4 }]}>
        <ToolIcon kind="redo" color={UI.textDim} />
      </Pressable>
    ),
    rotate: (
      <Pressable
        key="rotate"
        accessibilityRole="button"
        accessibilityLabel={vertical ? "Lay the tools out in a row" : "Lay the tools out in a column"}
        onPress={() => onPlacementChange({ ...placement, orientation: vertical ? "horizontal" : "vertical" }, true)}
        style={s.btn}
      >
        <Icon name="rotate" color={UI.textDim} size={15} />
      </Pressable>
    ),
  };

  return (
    <View
      style={[s.box, preview ? s.previewBox : { left: at.left, top: at.top }, { flexDirection: line }]}
      onLayout={preview ? undefined : onLayout}
      pointerEvents={preview ? "none" : "auto"}
      accessibilityRole="toolbar"
      accessibilityLabel={preview ? "Drawing tools preview" : "Drawing tools"}
      accessibilityElementsHidden={preview}
      importantForAccessibility={preview ? "no-hide-descendants" : "auto"}
    >
      {/* The preview never drags (and a GestureDetector inside a Modal would need its own gesture root). */}
      {layout.grip && (preview ? gripEl : <GestureDetector gesture={pan}>{gripEl}</GestureDetector>)}
      {/* Lines stack across the orientation: the main line(s), then the colour swatches when open. The swatches
          get a line of their own so opening them never widens a row past the chart (it pushed flip off-screen). */}
      <View style={{ flexDirection: across, gap: GAP }}>
      <View style={{ flexDirection: split ? across : line, gap: GAP }}>
      <View style={{ flexDirection: line, alignItems: "center", gap: GAP }} accessibilityRole="radiogroup" accessibilityLabel="Tool">
      {layout.tools.map((k) => {
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
            <ToolIcon kind={k} color={toolColor(k, on)} />
          </Pressable>
        );
      })}
      </View>
      {!split && layout.separator && <View style={vertical ? s.sepV : s.sepH} />}
      {/* Actions: colour (the selection), then the editing pill right beside it (owner: no space between). */}
      {actions && (
      <View style={{ flexDirection: line, alignItems: "center", gap: GAP }}>
      {layout.color && (
      <Pressable accessibilityRole="button" accessibilityLabel="Drawing colour" accessibilityState={{ disabled: !canColor, expanded: colorsOpen }} disabled={!canColor} onPress={() => setColorsOpen((o) => !o)} style={[s.btn, !canColor && { opacity: 0.4 }]}>
        {swatch(canColor ? current : UI.textDim, 14)}
      </Pressable>
      )}
      {/* Delete, undo, redo and flip: one pill on a dimmed white surface, buttons touching. Only what shows is in it;
          with none there is no pill (an empty dimmed surface would read as a broken button). */}
      {layout.edit.length > 0 && <View style={[s.group, { flexDirection: line }]}>{layout.edit.map((id) => editButtons[id])}</View>}
      </View>
      )}
      </View>
        {colorsOpen && canColor && layout.color && !preview && (
          <View style={[s.colors, { flexDirection: line, alignSelf: "flex-start" }]} accessibilityRole="radiogroup" accessibilityLabel="Drawing colour">
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
      </View>
    </View>
  );
}

const s = StyleSheet.create({
  // The preview sits in the flow (the Drawing tab), centred, with a muted 1 px border (owner-approved) because
  // it sits on the sheet's own colour; the floating palette is absolute.
  previewBox: { position: "relative", alignSelf: "center", borderWidth: 1, borderColor: UI.border },
  box: { position: "absolute", alignItems: "center", gap: GAP, padding: PAD, borderRadius: 12, backgroundColor: "rgba(21,26,32,0.95)", shadowColor: "#000", shadowOpacity: 0.35, shadowRadius: 8, shadowOffset: { width: 0, height: 2 }, elevation: 6 },
  grip: { alignItems: "center", justifyContent: "center" },
  btn: { width: BTN, height: BTN, borderRadius: 7, alignItems: "center", justifyContent: "center" },
  sepV: { width: 18, height: 1, backgroundColor: UI.row, marginVertical: 2 },
  sepH: { width: 1, height: 18, backgroundColor: UI.row, marginHorizontal: 2 },
  group: { alignItems: "center", gap: 0, borderRadius: 9, backgroundColor: "rgba(228,232,237,0.07)" },
  colors: { gap: 2, padding: 2, borderRadius: 9, backgroundColor: UI.row },
});

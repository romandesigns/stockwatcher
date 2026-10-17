import {
  DRAWING_COLORS,
  DRAWING_KINDS,
  DRAWING_LABELS,
  nudgeToolbox,
  toolboxPlacement,
  toolboxPosition,
  type DrawingKind,
  type Size,
  type ToolboxPlacement,
} from "@stockwatcher/chart-core";
import { useLayoutEffect, useRef, useState, type KeyboardEvent, type PointerEvent } from "react";
import { TOOL_ICONS } from "./ChartToolbar.tsx";
import { Button } from "./ui/button.tsx";
import { Popover, PopoverContent } from "./ui/popover.tsx";
import { ToggleGroup, ToggleGroupItem } from "./ui/toggle-group.tsx";
import { cn } from "./ui/utils.ts";

export interface DrawingToolboxProps {
  /** Where it sits (fractions of the room it can move in; chart-core toolbox.ts). */
  placement: ToolboxPlacement;
  /** Live while dragging; `done` once the pointer lifts (persist then). */
  onPlacementChange: (p: ToolboxPlacement, done: boolean) => void;
  /** The chart surface it floats over, in px. */
  container: Size;
  tool: DrawingKind | null;
  onToolChange: (tool: DrawingKind | null) => void;
  /** The selected drawing's colour, or undefined when nothing colourable is selected. */
  color?: string | null;
  canColor: boolean;
  onColorChange: (color: string) => void;
  canDelete: boolean;
  onDelete: () => void;
  canUndo: boolean;
  canRedo: boolean;
  onUndo: () => void;
  onRedo: () => void;
}

const svg = (d: string) => (
  <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" aria-hidden="true">
    <path d={d} />
  </svg>
);
const GripIcon = (
  <svg viewBox="0 0 16 16" fill="currentColor" aria-hidden="true">
    {[4, 8, 12].flatMap((y) => [6, 10].map((x) => <circle key={`${x}-${y}`} cx={x} cy={y} r="1.1" />))}
  </svg>
);
const TrashIcon = svg("M3 4.5h10M6.5 4.5V3h3v1.5M4.5 4.5l.6 8.5h5.8l.6-8.5");
const UndoIcon = svg("M5.5 3 2.5 6l3 3M2.5 6h7a4 4 0 0 1 0 8h-2");
const RedoIcon = svg("M10.5 3l3 3-3 3M13.5 6h-7a4 4 0 0 0 0 8h2");
const RotateIcon = svg("M3 8a5 5 0 0 1 8.5-3.5L13 6M13 3v3h-3M13 8a5 5 0 0 1-8.5 3.5L3 10M3 13v-3h3");

/**
 * The drawing tools as a floating palette over the chart (owner request
 * 2026-10-17): drag it by its grip anywhere inside the chart, or focus the
 * grip and use the arrow keys (Shift for bigger steps); flip it between a
 * column and a row; its place persists. It also holds the selected
 * drawing's colour and delete, so everything about drawings is in one spot.
 * It sits on a card surface with a shadow, no border (owner rule).
 */
export function DrawingToolbox({ placement, onPlacementChange, container, tool, onToolChange, color, canColor, onColorChange, canDelete, onDelete, canUndo, canRedo, onUndo, onRedo }: DrawingToolboxProps) {
  const ref = useRef<HTMLDivElement>(null);
  const colorRef = useRef<HTMLButtonElement>(null);
  const [box, setBox] = useState<Size>({ width: 40, height: 300 });
  const [colorsOpen, setColorsOpen] = useState(false);
  const drag = useRef<{ x0: number; y0: number; left: number; top: number; id: number } | null>(null);
  const vertical = placement.orientation === "vertical";

  // The palette's own size decides how far it can move; re-measure when it flips or wraps.
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const measure = () => setBox((b) => (b.width === el.offsetWidth && b.height === el.offsetHeight ? b : { width: el.offsetWidth, height: el.offsetHeight }));
    measure();
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  const at = toolboxPosition(placement, container, box);

  const onGripDown = (e: PointerEvent<HTMLButtonElement>) => {
    if (e.button !== 0) return;
    e.currentTarget.setPointerCapture(e.pointerId);
    drag.current = { x0: e.clientX, y0: e.clientY, left: at.left, top: at.top, id: e.pointerId };
  };
  const onGripMove = (e: PointerEvent<HTMLButtonElement>) => {
    const d = drag.current;
    if (!d || d.id !== e.pointerId) return;
    onPlacementChange(toolboxPlacement(d.left + e.clientX - d.x0, d.top + e.clientY - d.y0, container, box, placement.orientation), false);
  };
  const onGripUp = (e: PointerEvent<HTMLButtonElement>) => {
    const d = drag.current;
    if (!d || d.id !== e.pointerId) return;
    drag.current = null;
    onPlacementChange(toolboxPlacement(d.left + e.clientX - d.x0, d.top + e.clientY - d.y0, container, box, placement.orientation), true);
  };
  const onGripKey = (e: KeyboardEvent<HTMLButtonElement>) => {
    const step = e.shiftKey ? 48 : 8;
    const move = { ArrowLeft: [-step, 0], ArrowRight: [step, 0], ArrowUp: [0, -step], ArrowDown: [0, step] }[e.key];
    if (!move) return;
    e.preventDefault();
    onPlacementChange(nudgeToolbox(placement, move[0]!, move[1]!, container, box), true);
  };

  const iconButton = "size-7 rounded-md text-muted-foreground [&_svg]:size-3.5";
  return (
    <div
      ref={ref}
      role="toolbar"
      aria-label="Drawing tools"
      aria-orientation={vertical ? "vertical" : "horizontal"}
      // Wraps to a second row (or column) when the chart is too small for one, so it never spills out.
      style={{ left: at.left, top: at.top, ...(vertical ? { maxHeight: Math.max(120, container.height - 12) } : { maxWidth: Math.max(120, container.width - 12) }) }}
      className={cn("absolute z-10 flex flex-wrap items-center gap-0.5 rounded-xl bg-card/95 p-1 shadow-lg backdrop-blur-sm", vertical ? "flex-col" : "flex-row")}
    >
      <button
        type="button"
        aria-label="Move drawing tools (drag, or arrow keys)"
        title="Drag to move"
        onPointerDown={onGripDown}
        onPointerMove={onGripMove}
        onPointerUp={onGripUp}
        onPointerCancel={onGripUp}
        onKeyDown={onGripKey}
        className={cn("grid touch-none place-items-center rounded-md text-text-faint outline-none hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring [&_svg]:size-3.5", vertical ? "h-4 w-7 cursor-grab active:cursor-grabbing" : "h-7 w-4 cursor-grab active:cursor-grabbing")}
      >
        <span className={cn(!vertical && "rotate-90")}>{GripIcon}</span>
      </button>
      <ToggleGroup
        aria-label="Drawing tool"
        size="sm"
        spacing={0.5}
        variant="segmented"
        orientation={vertical ? "vertical" : "horizontal"}
        // The palette is the surface; the group needs no track of its own.
        className={cn("data-[variant=segmented]:bg-transparent data-[variant=segmented]:p-0", vertical && "flex-col")}
        value={[tool ?? "cursor"]}
        onValueChange={(v) => v[0] && onToolChange(v[0] === "cursor" ? null : (v[0] as DrawingKind))}
      >
        {(["cursor", ...DRAWING_KINDS, "measure"] as const).map((k) => (
          <ToggleGroupItem key={k} value={k} aria-label={k === "cursor" ? "Cursor" : DRAWING_LABELS[k]} title={k === "cursor" ? "Cursor (Esc)" : k === "long" ? "Long position (L)" : k === "short" ? "Short position (S)" : k === "measure" ? "Measure (or Shift+drag)" : DRAWING_LABELS[k]} className="w-7 px-0 [&_svg]:size-3.5">
            {TOOL_ICONS[k]}
          </ToggleGroupItem>
        ))}
      </ToggleGroup>
      <span aria-hidden="true" className={cn("shrink-0 rounded-full bg-bg-row", vertical ? "my-0.5 h-px w-5" : "mx-0.5 h-5 w-px")} />
      <Button ref={colorRef} variant="ghost" size="icon" className={iconButton} aria-label="Drawing colour" title="Colour of the selected drawing" disabled={!canColor} onClick={() => setColorsOpen(true)}>
        <span className="block size-3.5 rounded-full" style={{ background: canColor ? (color ?? DRAWING_COLORS[0]!.value) : "var(--color-muted-foreground)", opacity: canColor ? 1 : 0.5 }} />
      </Button>
      <Button variant="ghost" size="icon" className={iconButton} aria-label="Delete selected drawing" title="Delete selected drawing (Del)" disabled={!canDelete} onClick={onDelete}>
        {TrashIcon}
      </Button>
      <Button variant="ghost" size="icon" className={iconButton} aria-label="Undo" title="Undo (Ctrl+Z)" disabled={!canUndo} onClick={onUndo}>
        {UndoIcon}
      </Button>
      <Button variant="ghost" size="icon" className={iconButton} aria-label="Redo" title="Redo (Ctrl+Shift+Z)" disabled={!canRedo} onClick={onRedo}>
        {RedoIcon}
      </Button>
      <Button
        variant="ghost"
        size="icon"
        className={iconButton}
        aria-label={vertical ? "Lay the tools out in a row" : "Lay the tools out in a column"}
        title={vertical ? "Row" : "Column"}
        onClick={() => onPlacementChange({ ...placement, orientation: vertical ? "horizontal" : "vertical" }, true)}
      >
        {RotateIcon}
      </Button>

      <Popover open={colorsOpen && canColor} onOpenChange={setColorsOpen}>
        <PopoverContent anchor={colorRef} side={vertical ? "right" : "bottom"} align="start" sideOffset={8} className="w-auto p-2" aria-label="Drawing colour">
          <div role="radiogroup" aria-label="Drawing colour" className="grid grid-cols-4 gap-1.5">
            {DRAWING_COLORS.map((c) => {
              const on = (color ?? DRAWING_COLORS[0]!.value) === c.value;
              return (
                <button
                  key={c.value}
                  type="button"
                  role="radio"
                  aria-checked={on}
                  aria-label={c.name}
                  title={c.name}
                  onClick={() => {
                    onColorChange(c.value);
                    setColorsOpen(false);
                  }}
                  className={cn("grid size-7 place-items-center rounded-md outline-none hover:bg-bg-row focus-visible:ring-2 focus-visible:ring-ring", on && "bg-bg-raised")}
                >
                  <span className="block size-4 rounded-full" style={{ background: c.value }} />
                </button>
              );
            })}
          </div>
        </PopoverContent>
      </Popover>
    </div>
  );
}

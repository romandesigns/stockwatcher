import {
  DEFAULT_DRAWING_SETTINGS,
  DRAWING_COLORS,
  DRAWING_LABELS,
  nudgeToolbox,
  TOOLBOX_EDIT,
  TOOLBOX_TOOLS,
  toolboxLayout,
  toolboxPlacement,
  toolboxPosition,
  type DrawingKind,
  type DrawingSettings,
  type Size,
  type ToolboxPlacement,
} from "@stockwatcher/chart-core";
import { useLayoutEffect, useRef, useState, type KeyboardEvent, type PointerEvent, type ReactNode } from "react";
import { ARMED, TOOL_ICONS, toolTip } from "./ChartToolbar.tsx";
import { Icon } from "./Icon.tsx";
import { Tip } from "./ui/tooltip.tsx";
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
  /** The selected drawing can carry a drawing alert (a line, ray, level or position; chart-core drawingAlerts.ts). */
  canAlert?: boolean;
  /** Opens the host's drawing alert form for the selected drawing. */
  onAlert?: () => void;
  /** The selected drawing already has a live alert: the bell shows pressed. */
  alerted?: boolean;
  canUndo: boolean;
  canRedo: boolean;
  onUndo: () => void;
  onRedo: () => void;
  /** Which buttons show (the Settings dialog's Drawing tab; chart-core drawingSettings.ts). Default: all. */
  settings?: DrawingSettings;
  /**
   * Draw it in the flow instead of floating over a chart, and inert: the
   * Drawing tab's live preview, so the preview IS the toolbox and can never
   * drift from it.
   */
  preview?: boolean;
}

/**
 * One button's slot in the palette, shown or collapsed. Hiding a button in
 * Settings must not make the palette jump (owner rule: every visible layout
 * change animates), so a hidden button stays mounted and its slot's grid
 * track slides from 1fr to 0fr along the palette's line (columns in a row,
 * rows in a column) while it fades; showing it slides back. The global
 * reduced-motion rule (apps/web index.css) makes that instant. A collapsed
 * slot is `inert`: out of the tab order and the accessibility tree, so a
 * keyboard or a screen reader never lands on an invisible button.
 *
 * The gap between buttons is each slot's own 1 px padding, inside the clip,
 * not the container's flex gap, so a collapsed slot leaves no gap behind.
 * The clip is what collapses, which is why the buttons' focus rings are
 * inset (an outset ring would be clipped).
 */
function Slot({ shown, vertical, pad = true, children }: { shown: boolean; vertical: boolean; pad?: boolean; children: ReactNode }) {
  return (
    <span
      inert={!shown}
      data-shown={shown}
      className={cn(
        "grid shrink-0 transition-[grid-template-columns,grid-template-rows,opacity] duration-200 ease-out",
        vertical ? (shown ? "grid-rows-[1fr]" : "grid-rows-[0fr] opacity-0") : shown ? "grid-cols-[1fr]" : "grid-cols-[0fr] opacity-0",
      )}
    >
      <span className="flex min-h-0 min-w-0 overflow-hidden">
        <span className={cn("flex", vertical ? "flex-col" : "flex-row", pad && "p-px")}>{children}</span>
      </span>
    </span>
  );
}

/**
 * The drawing tools as a floating palette over the chart (owner request
 * 2026-10-17): drag it by its grip anywhere inside the chart, or focus the
 * grip and use the arrow keys (Shift for bigger steps); flip it between a
 * column and a row; its place persists. It also holds the selected
 * drawing's colour and delete, so everything about drawings is in one spot.
 * It sits on a card surface with a shadow, no border (owner rule).
 *
 * Which buttons show comes from `settings` through chart-core's
 * toolboxLayout (owner request 2026-10-18), the derivation the phone uses
 * too: hidden tools collapse out of the line, the edit pill holds only what
 * is left of delete / undo / redo / flip and collapses itself when that is
 * nothing, and the soft separator only shows when an action follows it.
 * The cursor always shows.
 */
export function DrawingToolbox({ placement, onPlacementChange, container, tool, onToolChange, color, canColor, onColorChange, canDelete, onDelete, canAlert = false, onAlert, alerted = false, canUndo, canRedo, onUndo, onRedo, settings = DEFAULT_DRAWING_SETTINGS, preview = false }: DrawingToolboxProps) {
  const ref = useRef<HTMLDivElement>(null);
  const colorRef = useRef<HTMLButtonElement>(null);
  const [box, setBox] = useState<Size>({ width: 40, height: 300 });
  const [colorsOpen, setColorsOpen] = useState(false);
  const drag = useRef<{ x0: number; y0: number; left: number; top: number; id: number } | null>(null);
  const vertical = placement.orientation === "vertical";
  const layout = toolboxLayout(settings);
  const tools = new Set(layout.tools);
  const edit = new Set(layout.edit);

  // The palette's own size decides how far it can move; re-measure when it flips, wraps or a button collapses.
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

  // Inset focus rings: the slots clip (see Slot). The clip would also cut a touch hit area, so on
  // a coarse pointer every button is a real 44 px square instead (owner, 2026-10-19).
  const iconButton = "size-7 pointer-coarse:size-11 rounded-md text-muted-foreground focus-visible:ring-2 focus-visible:ring-inset [&_svg]:size-3.5";
  const side = vertical ? "right" : "bottom";
  const editButton = (id: (typeof TOOLBOX_EDIT)[number]): ReactNode => {
    switch (id) {
      case "alert":
        return (
          <Tip label={alerted ? "Alerts on this drawing" : "Alert when the price touches this drawing"} side={side}>
            <Button variant="ghost" size="icon" className={cn(iconButton, alerted && canAlert && "text-primary")} aria-label="Alert on this drawing" aria-pressed={alerted && canAlert} disabled={!canAlert} onClick={onAlert}>
              <Icon name="bell" />
            </Button>
          </Tip>
        );
      case "delete":
        return (
          <Tip label={"Delete selected drawing (Del)"} side={side}>
            <Button variant="ghost" size="icon" className={iconButton} aria-label="Delete selected drawing" disabled={!canDelete} onClick={onDelete}>
              <Icon name="trash" />
            </Button>
          </Tip>
        );
      case "undo":
        return (
          <Tip label={"Undo (Ctrl+Z)"} side={side}>
            <Button variant="ghost" size="icon" className={iconButton} aria-label="Undo" disabled={!canUndo} onClick={onUndo}>
              <Icon name="undo" />
            </Button>
          </Tip>
        );
      case "redo":
        return (
          <Tip label={"Redo (Ctrl+Shift+Z)"} side={side}>
            <Button variant="ghost" size="icon" className={iconButton} aria-label="Redo" disabled={!canRedo} onClick={onRedo}>
              <Icon name="redo" />
            </Button>
          </Tip>
        );
      case "rotate":
        return (
          <Tip label={vertical ? "Lay out as a row" : "Lay out as a column"} side={side}>
            <Button
              variant="ghost"
              size="icon"
              className={iconButton}
              aria-label={vertical ? "Lay the tools out in a row" : "Lay the tools out in a column"}
              onClick={() => onPlacementChange({ ...placement, orientation: vertical ? "horizontal" : "vertical" }, true)}
            >
              <Icon name="rotate" />
            </Button>
          </Tip>
        );
    }
  };

  return (
    <div
      ref={ref}
      role="toolbar"
      aria-label={preview ? "Drawing tools preview" : "Drawing tools"}
      aria-orientation={vertical ? "vertical" : "horizontal"}
      inert={preview}
      // Wraps to a second row (or column) when the chart is too small for one, so it never spills out.
      style={preview ? undefined : { left: at.left, top: at.top, ...(vertical ? { maxHeight: Math.max(120, container.height - 12) } : { maxWidth: Math.max(120, container.width - 12) }) }}
      className={cn(
        "z-10 flex flex-wrap items-center rounded-xl bg-card/95 p-[3px] shadow-lg backdrop-blur-sm",
        // The preview sits on the Settings panel's own colour, so a muted 1 px border (owner-approved) separates it.
        preview ? "relative w-fit max-w-full border border-border" : "absolute",
        vertical ? "flex-col" : "flex-row",
      )}
    >
      <Slot shown={layout.grip} vertical={vertical}>
        <Tip label="Drag to move (or arrow keys)" side={side}>
          <button
            type="button"
            aria-label="Move drawing tools (drag, or arrow keys)"
            onPointerDown={onGripDown}
            onPointerMove={onGripMove}
            onPointerUp={onGripUp}
            onPointerCancel={onGripUp}
            onKeyDown={onGripKey}
            className={cn("grid touch-none place-items-center rounded-md text-text-faint outline-none hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-inset [&_svg]:size-3.5", vertical ? "h-4 w-7 cursor-grab active:cursor-grabbing" : "h-7 w-4 cursor-grab active:cursor-grabbing", "pointer-coarse:size-11")}
          >
            <span className={cn(!vertical && "rotate-90")}><Icon name="grip" /></span>
          </button>
        </Tip>
      </Slot>
      <ToggleGroup
        aria-label="Drawing tool"
        size="sm"
        spacing={0.5}
        variant="segmented"
        orientation={vertical ? "vertical" : "horizontal"}
        // The palette is the surface; the group needs no track of its own. Its gap is the slots' padding.
        // In a row the group is `display: contents`, so each tool is an item of the palette's own wrapping
        // line: when the room is narrower than every button (the Drawing tab's preview on a phone, a narrow
        // chart) the row breaks between tools, beside the grip. As one flex item the group could only wrap
        // whole, which stranded the grip alone on the first line and pushed tools out of the palette.
        // (Roving focus and the group's role follow the DOM, not the box, so they are unchanged.)
        className={cn("gap-0 data-[variant=segmented]:bg-transparent data-[variant=segmented]:p-0", vertical ? "flex-col" : "contents")}
        value={[tool ?? "cursor"]}
        onValueChange={(v) => v[0] && onToolChange(v[0] === "cursor" ? null : (v[0] as DrawingKind))}
      >
        {TOOLBOX_TOOLS.map((k) => (
          <Slot key={k} shown={tools.has(k)} vertical={vertical}>
            <Tip label={toolTip(k)} side={side}>
              <ToggleGroupItem value={k} disabled={!tools.has(k)} aria-label={k === "cursor" ? "Cursor" : DRAWING_LABELS[k]} className={cn("w-7 px-0 focus-visible:ring-2 focus-visible:ring-inset pointer-coarse:size-11 [&_svg]:size-3.5", ARMED)}>
                {TOOL_ICONS[k]}
              </ToggleGroupItem>
            </Tip>
          </Slot>
        ))}
      </ToggleGroup>
      <Slot shown={layout.separator} vertical={vertical} pad={false}>
        <span aria-hidden="true" className={cn("shrink-0 rounded-full bg-bg-row", vertical ? "my-1 h-px w-5" : "mx-1 h-5 w-px")} />
      </Slot>
      <Slot shown={layout.color} vertical={vertical}>
        <Tip label={"Colour of the selected drawing"} side={side}>
          <Button ref={colorRef} variant="ghost" size="icon" className={iconButton} aria-label="Drawing colour" disabled={!canColor} onClick={() => setColorsOpen(true)}>
            <span className="block size-3.5 rounded-full" style={{ background: canColor ? (color ?? DRAWING_COLORS[0]!.value) : "var(--color-muted-foreground)", opacity: canColor ? 1 : 0.5 }} />
          </Button>
        </Tip>
      </Slot>
      {/* Delete, undo, redo and flip: one pill on a dimmed white surface, buttons touching (as on the phone).
          Only the ones that show are in it, and with none the whole pill collapses. */}
      <Slot shown={layout.edit.length > 0} vertical={vertical}>
        <div role="group" aria-label="Edit" className={cn("flex shrink-0 rounded-lg bg-foreground/[0.07]", vertical ? "flex-col" : "flex-row")}>
          {TOOLBOX_EDIT.map((id) => (
            <Slot key={id} shown={edit.has(id)} vertical={vertical} pad={false}>
              {editButton(id)}
            </Slot>
          ))}
        </div>
      </Slot>

      <Popover open={colorsOpen && canColor && layout.color && !preview} onOpenChange={setColorsOpen}>
        <PopoverContent anchor={colorRef} side={side} align="start" sideOffset={8} className="w-auto" aria-label="Drawing colour">
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
                  onClick={() => {
                    onColorChange(c.value);
                    setColorsOpen(false);
                  }}
                  className={cn("grid size-7 place-items-center rounded-md outline-none pointer-coarse:size-11 hover:bg-bg-row focus-visible:ring-2 focus-visible:ring-ring", on && "bg-bg-raised")}
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

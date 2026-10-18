import {
  DEFAULT_DRAWING_SETTINGS,
  DEFAULT_TOOLBOX,
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
import { useId } from "react";
import { DrawingToolbox } from "./DrawingToolbox.tsx";
import { Icon } from "./Icon.tsx";
import type { SettingsDialogSection } from "./SettingsDialog.tsx";
import { Switch } from "./ui/switch.tsx";
import { cn } from "./ui/utils.ts";

export interface DrawingSettingsPanelProps {
  settings: DrawingSettings;
  onChange: (s: DrawingSettings) => void;
}

const GROUPS: readonly { id: ToolboxGroup; label: string }[] = [
  { id: "tools", label: "Tools" },
  { id: "actions", label: "Actions" },
];

const noop = () => {};

/**
 * The Settings dialog's Drawing tab (owner request 2026-10-18: "select the
 * features the drawing feature should display when active"). One row per
 * toolbox button, from chart-core's TOOLBOX_ITEMS (the list the toolbox
 * itself is built from), grouped Tools / Actions: its duotone icon, name, a
 * one-line description and a switch. Changes apply at once; the toolbox
 * over the chart collapses the button out (or back in) as it happens.
 *
 * The cursor's row is locked on, with the reason in place of the
 * description. Above the rows, a live preview: the real DrawingToolbox,
 * inert, laid out as a row (the orientation whose wrapping broke before),
 * so what you see is exactly what the chart will show. It sits centred on
 * the panel itself, with no recessed box behind it; the toolbox carries a
 * 1 px muted border instead (owner, 2026-10-18: "remove the darker
 * background of the preview section, you can add a border to the actual
 * widget"), an approved exception to the no-borders rule because a
 * bg-card toolbox on the bg-card panel has no contrast of its own.
 *
 * Rows have no dividers (owner rule): space, and a soft hover fill. The
 * switches are neutral (ui/switch.tsx explains why the accent is not spent
 * on them).
 */
export function DrawingSettingsPanel({ settings, onChange }: DrawingSettingsPanelProps) {
  return (
    <div className="flex flex-col gap-5">
      <section aria-label="Preview" className="flex flex-col gap-1.5">
        <h3 className="m-0 px-2 text-[11px] font-medium text-muted-foreground">Preview</h3>
        <div className="flex justify-center">
          <DrawingToolbox
            preview
            settings={settings}
            placement={{ ...DEFAULT_TOOLBOX, orientation: "horizontal" }}
            onPlacementChange={noop}
            container={{ width: 0, height: 0 }}
            tool={null}
            onToolChange={noop}
            canColor
            color={null}
            onColorChange={noop}
            canDelete
            onDelete={noop}
            canUndo
            canRedo
            onUndo={noop}
            onRedo={noop}
          />
        </div>
      </section>
      {GROUPS.map((g) => {
        const items = TOOLBOX_ITEMS.filter((i) => i.group === g.id);
        const shown = items.filter((i) => isToolboxItemVisible(settings, i.id)).length;
        const hideable = items.filter((i) => !i.locked);
        const allShown = hideable.every((i) => isToolboxItemVisible(settings, i.id));
        return (
          <section key={g.id} aria-label={g.label} className="flex flex-col gap-0.5">
            <div className="flex items-center gap-2 px-2 pb-1">
              <h3 className="m-0 text-[11px] font-medium text-muted-foreground">{g.label}</h3>
              <span className="font-mono text-[11px] text-text-faint tabular-nums">
                {shown}/{items.length}
              </span>
              <button
                type="button"
                onClick={() => onChange(setToolboxGroupVisible(settings, g.id, !allShown))}
                className="ml-auto rounded px-1 text-[11px] text-muted-foreground outline-none hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring"
              >
                {allShown ? "Hide all" : "Show all"}
              </button>
            </div>
            {items.map((item) => (
              <ItemRow key={item.id} item={item} on={isToolboxItemVisible(settings, item.id)} onToggle={(v) => onChange(setToolboxItemVisible(settings, item.id, v))} />
            ))}
          </section>
        );
      })}
      <p className="m-0 px-2 text-[11px] leading-relaxed text-text-faint">
        Hiding a tool only removes its button. Drawings already on the chart stay, and can still be selected, moved and deleted (Del). A tool you hide while it is armed goes back to the cursor.
      </p>
    </div>
  );
}

/** "Reset to defaults" for the panel's heading; disabled (not removed) when already default, so the heading never shifts. */
export function DrawingSettingsReset({ settings, onChange }: DrawingSettingsPanelProps) {
  const isDefault = isDefaultDrawingSettings(settings);
  return (
    <button
      type="button"
      disabled={isDefault}
      onClick={() => onChange({ ...DEFAULT_DRAWING_SETTINGS, hidden: [] })}
      className="shrink-0 rounded-md bg-bg-row px-2.5 py-1 text-xs text-muted-foreground outline-none transition-[opacity,background-color,color] hover:bg-bg-raised hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring disabled:pointer-events-none disabled:opacity-40"
    >
      Reset to defaults
    </button>
  );
}

function ItemRow({ item, on, onToggle }: { item: ToolboxItemInfo; on: boolean; onToggle: (v: boolean) => void }) {
  const descId = useId();
  const locked = !!item.locked;
  return (
    <label className={cn("flex min-h-11 items-center gap-3 rounded-lg px-2 py-1.5 transition-colors", locked ? "cursor-default" : "cursor-pointer hover:bg-bg-row")}>
      <span className={cn("grid size-8 shrink-0 place-items-center rounded-lg bg-bg-row transition-[opacity,color] duration-150 [&_svg]:size-4", on ? "text-primary" : "text-muted-foreground opacity-50")}>
        <Icon name={item.icon} />
      </span>
      <span className="flex min-w-0 flex-1 flex-col">
        <span className={cn("text-[13px] transition-colors", on ? "text-foreground" : "text-foreground/70")}>{item.name}</span>
        <span id={descId} className="truncate text-[11px] text-text-faint">
          {locked ? item.locked : item.description}
        </span>
      </span>
      <Switch checked={on} disabled={locked} onCheckedChange={(v) => onToggle(v)} aria-label={`Show ${item.name}`} aria-describedby={descId} />
    </label>
  );
}

/** The Drawing tab as a SettingsDialog section: chart-core's label, icon and description, this panel and its reset. */
export function drawingSettingsSection(settings: DrawingSettings, onChange: (s: DrawingSettings) => void): SettingsDialogSection {
  const info = SETTINGS_SECTIONS.find((x) => x.id === "drawing")!;
  return { ...info, action: <DrawingSettingsReset settings={settings} onChange={onChange} />, render: () => <DrawingSettingsPanel settings={settings} onChange={onChange} /> };
}

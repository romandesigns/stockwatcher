import { resolveSettingsSection, settingsRailMode, type IconName } from "@stockwatcher/chart-core";
import { useCallback, useRef, useState, useSyncExternalStore, type ReactNode } from "react";
import { Icon } from "./Icon.tsx";
import { Dialog, DialogClose, DialogContent, DialogTitle } from "./ui/dialog.tsx";
import { ScrollArea } from "./ui/scroll-area.tsx";
import { Tabs, TabsContent, TabsIndicator, TabsList, TabsTrigger } from "./ui/tabs.tsx";
import { Tip } from "./ui/tooltip.tsx";
import { cn } from "./ui/utils.ts";

/** One tab of the dialog: a future section is one more entry in the list the host passes. */
export interface SettingsDialogSection {
  id: string;
  label: string;
  icon: IconName;
  /** One line under the panel's heading. */
  description?: string;
  /** Beside the panel's heading, e.g. "Reset to defaults". */
  action?: ReactNode;
  render: () => ReactNode;
}

export interface SettingsDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  sections: readonly SettingsDialogSection[];
  /** Remember the selected tab in localStorage under `${persistKey}:tab` (it is kept while the app is open regardless). */
  persistKey?: string;
}

/** The dialog's rendered width: min(880px, 92vw), the same expression as its class below. */
const dialogWidth = (vw: number) => Math.min(880, vw * 0.92);

const subscribeResize = (cb: () => void) => {
  addEventListener("resize", cb);
  return () => removeEventListener("resize", cb);
};
const useViewportWidth = () => useSyncExternalStore(subscribeResize, () => innerWidth, () => 1024);

const loadTab = (key: string | undefined): unknown => {
  if (!key) return null;
  try {
    return localStorage.getItem(key);
  } catch {
    return null;
  }
};
const saveTab = (key: string | undefined, id: string) => {
  if (!key) return;
  try {
    localStorage.setItem(key, id);
  } catch {
    // Private mode or blocked storage: the tab is still kept while the app is open.
  }
};

/**
 * The Settings dialog (owner request 2026-10-18): two columns, a 15 % rail
 * of tab-like labels and an 85 % panel showing the active section. Generic
 * and data-driven: the host passes `sections`, so a new tab is one entry.
 *
 * - Size: min(880 px, 92 vw) × min(600 px, 85 vh), centred: generous on a
 *   desktop, never full-screen. Centred with `inset-0 m-auto`, not a
 *   translate, because the shared DialogContent animates `translate` on
 *   open and close, and a centring translate would fight it.
 * - Rail: a recessed strip (the page tone on the dialog's card, so it
 *   separates by contrast, no border). The active tab is a raised surface
 *   (Base UI's indicator, sliding between tabs) with accent text and icon:
 *   the dialog's one accent. Idle tabs are muted.
 * - Responsive (chart-core settingsRailMode): the 15 / 85 split is KEPT at
 *   every width that leaves the rail a 44 px target. When 15 % is too
 *   narrow for "Drawing" beside its icon (dialog under ~747 px, so any
 *   window under ~812 px), the rail shows icons only, the label being the
 *   tooltip and the accessible name, and the panel's heading still names
 *   the section. Only below a ~293 px dialog does the rail become a row of
 *   tabs across the top. Chosen over "a top row below 520 px" because it
 *   keeps the owner's layout on a phone (a 390 px screen gives a 54 px
 *   rail), and the heading already carries the words.
 * - Keyboard: Base UI's tablist, vertical, so ↑ / ↓ (and Home / End) move
 *   between tabs and activate them; Tab moves into the panel; Escape or the
 *   ✕ closes and focus returns to the gear. Focus opens on the active tab.
 * - Motion: the panel cross-fades with a short rise on a tab switch, the
 *   indicator slides; both are instant under reduced motion (index.css).
 * - The panel scrolls on its own; the heading stays put above it.
 * - The selected tab survives closing and reopening while the app runs,
 *   and with `persistKey` a reload as well.
 */
export function SettingsDialog({ open, onOpenChange, sections, persistKey }: SettingsDialogProps) {
  const ids = sections.map((s) => s.id);
  const key = persistKey && `${persistKey}:tab`;
  const [stored, setStored] = useState<unknown>(() => loadTab(key));
  const tab = resolveSettingsSection(stored, ids);
  const choose = (id: string) => {
    setStored(id);
    saveTab(key, id);
  };
  const mode = settingsRailMode(dialogWidth(useViewportWidth()));
  const top = mode === "top";
  const labels = mode === "labels";
  // Focus lands on the active tab when the dialog opens (the roving tab stop of the list). Looked up
  // when Base UI asks, after the tabs have mounted, so the first open finds it too.
  const tabRefs = useRef(new Map<string, HTMLElement>());
  const refFor = useCallback(
    (id: string) => (el: HTMLElement | null) => {
      if (el) tabRefs.current.set(id, el);
      else tabRefs.current.delete(id);
    },
    [],
  );

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent initialFocus={() => (tab === null ? true : (tabRefs.current.get(tab) ?? true))} className="inset-0 m-auto h-[min(600px,85dvh)] w-[min(880px,92vw)] overflow-hidden p-0">
        <DialogTitle>Settings</DialogTitle>
        <Tabs
          orientation={top ? "horizontal" : "vertical"}
          value={tab}
          onValueChange={(v) => typeof v === "string" && choose(v)}
          className={cn("size-full gap-0", top ? "flex-col" : "flex-row")}
        >
          <div className={cn("flex shrink-0 flex-col gap-3 bg-background/60", top ? "w-full px-3 pt-3 pb-2" : "w-[15%] p-2 pt-4")}>
            {!top && (
              <div aria-hidden="true" className={cn("flex items-center gap-2 text-text-faint [&_svg]:size-3.5", labels ? "px-2.5 text-[11px] font-medium" : "justify-center")}>
                <Icon name="settings" />
                {labels && <span>Settings</span>}
              </div>
            )}
            <TabsList aria-label="Settings sections" activateOnFocus className={cn(top && "flex-row")}>
              <TabsIndicator />
              {sections.map((s) => {
                const trigger = (
                  <TabsTrigger
                    key={s.id}
                    ref={refFor(s.id)}
                    value={s.id}
                    aria-label={s.label}
                    className={cn("[&_svg]:size-4", labels || top ? "h-9 justify-start px-2.5" : "h-11 w-full justify-center px-0")}
                  >
                    <Icon name={s.icon} />
                    {(labels || top) && <span className="truncate">{s.label}</span>}
                  </TabsTrigger>
                );
                // Icons only: the label moves into a tooltip (and stays the accessible name).
                return labels || top ? trigger : (
                  <Tip key={s.id} label={s.label} side="right">
                    {trigger}
                  </Tip>
                );
              })}
            </TabsList>
          </div>
          {/* The 85 %: every panel in one grid cell, so the outgoing one can fade while the next fades in. */}
          <div className="grid min-h-0 min-w-0 flex-1 grid-cols-1 grid-rows-[minmax(0,1fr)]">
            {sections.map((s) => (
              <TabsContent
                key={s.id}
                value={s.id}
                className="col-start-1 row-start-1 flex min-h-0 flex-col transition-[opacity,translate] duration-150 ease-out data-ending-style:opacity-0 data-starting-style:translate-y-1 data-starting-style:opacity-0"
              >
                {/* Heading and action on one line, the description under both at full width, so a narrow
                    panel (a phone) never squeezes the sentence into a column beside the button. */}
                <header className="flex flex-col gap-1 py-4 pr-14 pl-5">
                  <div className="flex min-h-7 items-center gap-3">
                    <h2 className="m-0 min-w-0 flex-1 truncate text-[15px] font-semibold">{s.label}</h2>
                    {s.action}
                  </div>
                  {s.description && <p className="m-0 text-xs text-muted-foreground">{s.description}</p>}
                </header>
                {/* The panel scrolls on its own, with the shadcn scrollbar (the menus' slim thumb), not the browser's. */}
                <ScrollArea className="min-h-0 flex-1" viewportClassName="px-5 pb-5">
                  {s.render()}
                </ScrollArea>
              </TabsContent>
            ))}
          </div>
        </Tabs>
        <Tip label="Close (Esc)" side="left">
          <DialogClose
            aria-label="Close settings"
            className="absolute top-3 right-3 grid size-8 place-items-center rounded-md text-muted-foreground outline-none transition-colors hover:bg-bg-row hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring [&_svg]:size-4"
          >
            <Icon name="clear" />
          </DialogClose>
        </Tip>
      </DialogContent>
    </Dialog>
  );
}

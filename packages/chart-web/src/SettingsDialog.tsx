import { resolveSettingsSection, SETTINGS_NAV_ROOT, settingsBack, settingsLayout, settingsNavReduce, settingsRailMode, type IconName } from "@stockwatcher/chart-core";
import { useCallback, useEffect, useReducer, useRef, useState, useSyncExternalStore, type ReactNode } from "react";
import { Icon } from "./Icon.tsx";
import { Dialog, DialogClose, DialogContent, DialogTitle } from "./ui/dialog.tsx";
import { ScrollArea } from "./ui/scroll-area.tsx";
import { Tabs, TabsContent, TabsIndicator, TabsList, TabsTrigger } from "./ui/tabs.tsx";
import { Tip } from "./ui/tooltip.tsx";
import { cn, TOUCH_HIT } from "./ui/utils.ts";

/** One section of the dialog: a future section is one more entry in the list the host passes. */
export interface SettingsDialogSection {
  id: string;
  label: string;
  icon: IconName;
  /** One line under the panel's heading (and under the label on the single-column root list). */
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

/** The two-column dialog's rendered width: min(880px, 92vw), the same expression as its class below. */
const dialogWidth = (vw: number) => Math.min(880, vw * 0.92);

const subscribeResize = (cb: () => void) => {
  addEventListener("resize", cb);
  return () => removeEventListener("resize", cb);
};
const useViewportWidth = () => useSyncExternalStore(subscribeResize, () => innerWidth, () => 1024);

/**
 * The connected active tab (owner, 2026-10-18: "connect the active tab with
 * the associated section, have them both have the same background"). The
 * indicator wears the panel's colour (bg-card, the dialog's own surface,
 * which the panel shows through), is rounded only on the side away from the
 * panel, and reaches the rail's inner edge: the rail has no padding on that
 * side, so the tab's edge IS the panel's edge and the two read as one shape.
 *
 * The folder-tab "notches": two r × r squares (r = 8 px, the tab's corner
 * radius) at the tab's panel-side corners, outside the tab, painted with the
 * panel colour except a quarter circle of the rail showing through
 * (radial-gradient, transparent inside r). That turns the rail's straight
 * edge into a concave curve where it meets the tab, above and below it.
 * They are pseudo elements of the indicator, so they slide with it for free.
 */
const CONNECTED_SIDE =
  "rounded-l-lg rounded-r-none bg-card before:pointer-events-none before:absolute before:right-0 before:bottom-full before:size-2 before:bg-[radial-gradient(circle_at_0_0,transparent_7.5px,var(--color-card)_8px)] after:pointer-events-none after:absolute after:top-full after:right-0 after:size-2 after:bg-[radial-gradient(circle_at_0_100%,transparent_7.5px,var(--color-card)_8px)]";

/** The ✕, the same control in both layouts: top-right of the dialog, outside anything that slides. */
// 40 px with a 44 px touch area on a coarse pointer (TOUCH_HIT; the button is already absolute).
const CLOSE_CLASS = cn(
  "absolute top-3 right-3 z-10 grid size-8 place-items-center rounded-md text-muted-foreground outline-none transition-colors hover:bg-bg-row hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring pointer-coarse:top-2 pointer-coarse:right-2 pointer-coarse:size-10 [&_svg]:size-4",
  TOUCH_HIT,
);

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

/** A ref callback per id into one Map, so focus can be sent to a given tab or row by id. */
function useRefMap<T extends HTMLElement>() {
  const map = useRef(new Map<string, T>());
  const refFor = useCallback(
    (id: string) => (el: T | null) => {
      if (el) map.current.set(id, el);
      else map.current.delete(id);
    },
    [],
  );
  return [map, refFor] as const;
}

/**
 * The Settings dialog. Generic and data-driven: the host passes `sections`,
 * so a new section is one entry. Two layouts, chosen by chart-core's
 * settingsLayout from the viewport width (one rule, shared with the phone
 * app, which is always single column):
 *
 * - Two columns (640 px and wider; owner request 2026-10-18): a 15 % rail of
 *   tab-like labels and an 85 % panel showing the active section. See
 *   TwoColumnSettings.
 * - Single column (narrower; owner follow-up the same day, "Instead of two
 *   columns tab navigation settings modal on mobile, I think we should make
 *   it a single column instead"): a root list that drills into a section,
 *   the phone's system Settings pattern. See SingleColumnSettings.
 *
 * The remembered tab lives here, above both, so it survives a resize across
 * the line: opening a section on the narrow layout also selects it as the
 * wide dialog's tab. The other way round does not apply, because single
 * column always opens on its root (settingsNavReduce "reset").
 */
export function SettingsDialog({ open, onOpenChange, sections, persistKey }: SettingsDialogProps) {
  const key = persistKey && `${persistKey}:tab`;
  const [stored, setStored] = useState<unknown>(() => loadTab(key));
  const tab = resolveSettingsSection(stored, sections.map((s) => s.id));
  const choose = (id: string) => {
    setStored(id);
    saveTab(key, id);
  };
  const vw = useViewportWidth();
  return settingsLayout(vw, "web") === "single-column" ? (
    <SingleColumnSettings open={open} onOpenChange={onOpenChange} sections={sections} onVisit={choose} />
  ) : (
    <TwoColumnSettings open={open} onOpenChange={onOpenChange} sections={sections} tab={tab} onTab={choose} labels={settingsRailMode(dialogWidth(vw)) === "labels"} />
  );
}

/**
 * The two-column layout (owner request 2026-10-18): a 15 % rail of tab-like
 * labels and an 85 % panel showing the active section.
 *
 * - Size: min(880 px, 92 vw) × min(600 px, 85 vh), centred: generous on a
 *   desktop, never full-screen. Centred with `inset-0 m-auto`, not a
 *   translate, because the shared DialogContent animates `translate` on
 *   open and close, and a centring translate would fight it.
 * - Rail: a recessed strip (the page tone on the dialog's card, so it
 *   separates by contrast, no border). The active tab is CONNECTED to the
 *   panel (CONNECTED_SIDE above): the panel's colour, flush with its edge,
 *   with folder-tab notches, sliding between tabs as Base UI's indicator.
 *   Its text and duotone icon are accent (primary tone accent, secondary
 *   accent at ICON_SECONDARY_OPACITY): the dialog's one accent. Idle tabs
 *   stay on the rail, muted.
 * - Responsive (chart-core settingsRailMode): when 15 % is too narrow for
 *   "Drawing" beside its icon (dialog under ~747 px, so any window under
 *   ~812 px), the rail shows icons only, the label being the tooltip and the
 *   accessible name, and the panel's heading still names the section. Below
 *   640 px the whole dialog is single column instead (SettingsDialog).
 * - Keyboard: Base UI's tablist, vertical, so ↑ / ↓ (and Home / End) move
 *   between tabs and activate them; Tab moves into the panel; Escape or the
 *   ✕ closes and focus returns to the gear. Focus opens on the active tab.
 * - Motion: the panel cross-fades with a short rise on a tab switch, the
 *   connected tab slides (notches with it); both are instant under reduced
 *   motion (index.css).
 * - The panel scrolls on its own; the heading stays put above it.
 * - The selected tab survives closing and reopening while the app runs,
 *   and with `persistKey` a reload as well.
 */
function TwoColumnSettings({
  open,
  onOpenChange,
  sections,
  tab,
  onTab,
  labels,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  sections: readonly SettingsDialogSection[];
  tab: string | null;
  onTab: (id: string) => void;
  labels: boolean;
}) {
  // Focus lands on the active tab when the dialog opens (the roving tab stop of the list). Looked up
  // when Base UI asks, after the tabs have mounted, so the first open finds it too.
  const [tabRefs, refFor] = useRefMap<HTMLElement>();

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent initialFocus={() => (tab === null ? true : (tabRefs.current.get(tab) ?? true))} className="h-[min(600px,85dvh)] w-[min(880px,92vw)] max-w-none overflow-hidden p-0">
        <DialogTitle>Settings</DialogTitle>
        <Tabs orientation="vertical" value={tab} onValueChange={(v) => typeof v === "string" && onTab(v)} className="size-full flex-row gap-0">
          {/* No padding on the panel side (right), so the active tab meets the panel. */}
          <div className="flex w-[15%] shrink-0 flex-col gap-3 bg-background/60 py-2 pt-4 pl-2">
            <div aria-hidden="true" className={cn("flex items-center gap-2 text-text-faint [&_svg]:size-3.5", labels ? "px-2.5 text-[11px] font-medium" : "justify-center")}>
              <Icon name="settings" />
              {labels && <span>Settings</span>}
            </div>
            <TabsList aria-label="Settings sections" activateOnFocus>
              <TabsIndicator className={CONNECTED_SIDE} />
              {sections.map((s) => {
                const trigger = (
                  <TabsTrigger key={s.id} ref={refFor(s.id)} value={s.id} aria-label={s.label} className={cn("rounded-l-lg rounded-r-none [&_svg]:size-4", labels ? "h-9 justify-start px-2.5 pointer-coarse:h-11" : "h-11 w-full justify-center px-0")}>
                    <Icon name={s.icon} />
                    {labels && <span className="truncate">{s.label}</span>}
                  </TabsTrigger>
                );
                // Icons only: the label moves into a tooltip (and stays the accessible name).
                return labels ? (
                  trigger
                ) : (
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
                    panel never squeezes the sentence into a column beside the button. */}
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
        <CloseButton />
      </DialogContent>
    </Dialog>
  );
}

function CloseButton() {
  return (
    <Tip label="Close (Esc)" side="left">
      <DialogClose aria-label="Close settings" className={CLOSE_CLASS}>
        <Icon name="clear" />
      </DialogClose>
    </Tip>
  );
}

/**
 * The single-column layout, for a phone-width window (owner follow-up
 * 2026-10-18: single column instead of the two-column tabs on mobile). The
 * web twin of chart-native's SettingsSheet, so the phone app and the phone
 * browser behave the same:
 *
 * - Shape: a bottom sheet, full width and 92 dvh tall (the phone Sheet's
 *   `tall` height), so the dimmed page still shows above it and a tap there
 *   closes it. It rises from the bottom on open and sinks on close. Not the
 *   centred desktop modal shrunk down: at 390 px that left a cramped card.
 * - Root screen: the "Settings" title (a real heading here, sr-only in the
 *   two-column layout where the rail says it) and one full-width list of the
 *   sections, each row its duotone icon on a tile, label, one-line
 *   description and a duotone chevron. A list of buttons in a <nav>.
 * - Section screen: a tap slides the section in from the right at full
 *   width over the root, which slides a quarter left and fades (the iOS
 *   push). Its header is back (duotone chevron-left) + the section's title
 *   + its action ("Reset to defaults"), the description under them; below,
 *   the SAME panel the two-column dialog shows, now full width.
 * - Back: the back button, or Escape, pops to the root with the reverse
 *   slide; Escape on the root closes (chart-core settingsBack). The ✕ stays
 *   top-right on both screens, outside the sliding layer, so closing is
 *   always one tap.
 * - Focus: the first row on open; the section's heading on a push (it is
 *   tabIndex -1, a target rather than a stop); the row that opened it on a
 *   pop. The off-screen layer is `inert`, so neither Tab nor a screen reader
 *   can wander into the screen that is not showing.
 * - Always opens on the root (settingsNavReduce "reset", see chart-core for
 *   why), reset at the moment of opening so the close animation never
 *   shows the section snapping back.
 * - Motion: 250 ms ease-out translate (+ opacity on the root); the global
 *   reduced-motion rule in apps/web index.css collapses it to instant.
 */
function SingleColumnSettings({ open, onOpenChange, sections, onVisit }: { open: boolean; onOpenChange: (open: boolean) => void; sections: readonly SettingsDialogSection[]; onVisit: (id: string) => void }) {
  const [nav, dispatch] = useReducer(settingsNavReduce, SETTINGS_NAV_ROOT);
  // Reset on every open, during render (React's "adjust state when a prop changes" pattern), so the
  // first frame of the open is already the root.
  const [wasOpen, setWasOpen] = useState(open);
  if (open !== wasOpen) {
    setWasOpen(open);
    if (open) dispatch({ type: "reset" });
  }
  // The section stays rendered while it slides back out after a pop, so remember the last one pushed.
  const [shown, setShown] = useState<string | null>(nav.section);
  if (nav.section !== null && nav.section !== shown) setShown(nav.section);
  const section = sections.find((s) => s.id === shown);
  const inSection = nav.section !== null;

  const [rowRefs, refFor] = useRefMap<HTMLButtonElement>();
  const headingRef = useRef<HTMLHeadingElement>(null);
  useEffect(() => {
    if (nav.section !== null) headingRef.current?.focus({ preventScroll: true });
    else if (nav.returnTo !== null) rowRefs.current.get(nav.returnTo)?.focus({ preventScroll: true });
  }, [nav, rowRefs]);

  const push = (id: string) => {
    dispatch({ type: "push", id });
    onVisit(id);
  };

  return (
    <Dialog
      open={open}
      onOpenChange={(next, details) => {
        // Escape inside a section is "back", not "close" (chart-core settingsBack).
        if (!next && details.reason === "escape-key" && settingsBack(nav) === "pop") {
          details.cancel();
          dispatch({ type: "pop" });
          return;
        }
        onOpenChange(next);
      }}
    >
      <DialogContent
        initialFocus={() => (sections[0] ? (rowRefs.current.get(sections[0].id) ?? true) : true)}
        // bottom-sheet-ok: the phone-width web twin of the phone's SettingsSheet, a sheet on purpose (listed for the owner, 2026-11-07).
        className="inset-x-0 top-auto bottom-0 m-0 flex h-[92dvh] max-h-none w-full max-w-none flex-col overflow-hidden rounded-t-xl rounded-b-none p-0 duration-250 ease-out data-ending-style:translate-y-full data-ending-style:opacity-100 data-starting-style:translate-y-full data-starting-style:opacity-100"
      >
        <div className="relative min-h-0 flex-1 overflow-hidden">
          {/* Root: the list of sections. */}
          <div inert={inSection} className={cn("absolute inset-0 flex flex-col transition-[translate,opacity] duration-250 ease-out", inSection && "-translate-x-1/4 opacity-0")}>
            <header className="flex min-h-14 items-center pr-14 pl-5">
              <DialogTitle className="not-sr-only m-0 text-[17px] font-semibold">Settings</DialogTitle>
            </header>
            <ScrollArea className="min-h-0 flex-1" viewportClassName="px-3 pt-1 pb-5">
              <nav aria-label="Settings sections">
                <ul className="m-0 flex list-none flex-col gap-1 p-0">
                  {sections.map((s) => (
                    <li key={s.id}>
                      <button
                        ref={refFor(s.id)}
                        type="button"
                        onClick={() => push(s.id)}
                        className="flex min-h-14 w-full items-center gap-3 rounded-lg bg-bg-row px-3 py-2.5 text-left text-foreground outline-none transition-colors hover:bg-bg-raised focus-visible:ring-2 focus-visible:ring-ring active:bg-bg-raised"
                      >
                        <span aria-hidden="true" className="grid size-9 shrink-0 place-items-center rounded-md bg-bg-raised [&_svg]:size-[18px]">
                          <Icon name={s.icon} />
                        </span>
                        <span className="flex min-w-0 flex-1 flex-col gap-0.5">
                          <span className="text-sm font-medium">{s.label}</span>
                          {s.description && <span className="truncate text-xs text-muted-foreground">{s.description}</span>}
                        </span>
                        <span aria-hidden="true" className="text-muted-foreground [&_svg]:size-3.5">
                          <Icon name="chevronRight" />
                        </span>
                      </button>
                    </li>
                  ))}
                </ul>
              </nav>
            </ScrollArea>
          </div>
          {/* Section: slides in from the right over the root, on the dialog's own surface so it covers it. */}
          <div inert={!inSection} className={cn("absolute inset-0 flex flex-col bg-card transition-[translate] duration-250 ease-out", !inSection && "translate-x-full")}>
            {section && (
              <>
                <header className="flex flex-col gap-1 pt-1 pr-14 pb-3 pl-2">
                  <div className="flex min-h-12 items-center gap-1">
                    <button
                      type="button"
                      aria-label="Back to Settings"
                      onClick={() => dispatch({ type: "pop" })}
                      className={cn("relative grid size-10 shrink-0 place-items-center rounded-md text-muted-foreground outline-none transition-colors hover:bg-bg-row hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring [&_svg]:size-4", TOUCH_HIT)}
                    >
                      <Icon name="chevronLeft" />
                    </button>
                    <h2 ref={headingRef} tabIndex={-1} className="m-0 min-w-0 flex-1 truncate text-[15px] font-semibold outline-none">
                      {section.label}
                    </h2>
                    {section.action}
                  </div>
                  {section.description && <p className="m-0 pl-3 text-xs text-muted-foreground">{section.description}</p>}
                </header>
                <ScrollArea className="min-h-0 flex-1" viewportClassName="px-5 pb-5">
                  {section.render()}
                </ScrollArea>
              </>
            )}
          </div>
        </div>
        <CloseButton />
      </DialogContent>
    </Dialog>
  );
}

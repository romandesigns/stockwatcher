import type { IconName } from "@stockwatcher/chart-core";
import { Icon } from "@stockwatcher/chart-web";
import { Tip, cn } from "@stockwatcher/chart-web/ui";
import { NAV_ITEMS, type PageId } from "./routes.ts";

// The main navigation, in the two shapes wavykit defines for it (H:\wavystack\wavykit
// registry/components/sidebar and tab-bar): a rail on the left on wider screens, a bottom bar on
// a phone. Ported rather than `kit add`ed: stockwatcher predates wavykit and names its tokens the
// shadcn way (bg-bg-raised, text-foreground, text-primary = the accent), so the markup is the
// kit's and the classes are this app's. Same rules as the kit:
// - no border or fill between the nav and the page; space separates them;
// - active = one surface step up with foreground text, the icon in the accent duotone (the only
//   accent the nav spends); idle = dim, a row surface on hover;
// - real links with aria-current="page", so pages open in new tabs and work from the keyboard.

/**
 * The desktop rail. Collapsed it is a 56 px icon column with a tip per item; expanded it shows the
 * labels. The labels stay in the DOM either way (they fade and narrow), so every link keeps its
 * accessible name, and the icons keep the same x so nothing jumps when it toggles.
 */
export function NavRail({ page, collapsed, onToggle }: { page: PageId; collapsed: boolean; onToggle: () => void }) {
  return (
    <nav
      aria-label="Main"
      data-collapsed={collapsed || undefined}
      className={cn("flex h-full shrink-0 flex-col gap-4 overflow-hidden p-2 transition-[width] duration-200 ease-out motion-reduce:transition-none", collapsed ? "w-14" : "w-48")}
    >
      <div className="flex h-10 items-center gap-2.5 px-1.5">
        {/* The brand mark: a small accent tile, as on the handbook and the app icon's colour. */}
        <span aria-hidden className="size-7 shrink-0 rounded-lg bg-primary/15 p-2">
          <span className="block size-full rounded-[3px] bg-primary" />
        </span>
        <span className={cn("truncate text-sm font-semibold tracking-tight transition-opacity duration-150", collapsed ? "opacity-0" : "opacity-100")}>StockWatcher</span>
      </div>
      <ul className="m-0 flex list-none flex-col gap-0.5 p-0">
        {NAV_ITEMS.map((item) => (
          <li key={item.id} className="flex min-w-0">
            <RailLink icon={item.icon} label={item.label} href={item.href} active={item.id === page} collapsed={collapsed} />
          </li>
        ))}
      </ul>
      <ul className="m-0 mt-auto flex list-none flex-col gap-0.5 p-0">
        <li className="flex min-w-0">
          <RailLink icon={collapsed ? "sidebarExpand" : "sidebarCollapse"} label={collapsed ? "Expand menu" : "Collapse menu"} onClick={onToggle} collapsed={collapsed} />
        </li>
      </ul>
    </nav>
  );
}

function RailLink({ icon, label, href, onClick, active, collapsed }: { icon: IconName; label: string; href?: string; onClick?: () => void; active?: boolean; collapsed: boolean }) {
  const cls = cn(
    "flex h-10 w-full min-w-0 items-center gap-2.5 rounded-lg px-3 text-sm no-underline outline-none select-none transition-colors duration-150 focus-visible:ring-2 focus-visible:ring-ring [&_svg]:size-4 [&_svg]:shrink-0",
    active ? "bg-bg-raised font-medium text-foreground [&_svg]:text-primary" : "text-muted-foreground hover:bg-bg-row hover:text-foreground",
  );
  const inner = (
    <>
      <Icon name={icon} />
      <span className={cn("truncate transition-opacity duration-150", collapsed ? "opacity-0" : "opacity-100")}>{label}</span>
    </>
  );
  const el =
    href !== undefined ? (
      <a href={href} aria-current={active ? "page" : undefined} className={cls}>
        {inner}
      </a>
    ) : (
      <button type="button" onClick={onClick} className={cls}>
        {inner}
      </button>
    );
  // The tip names the item only while the label is hidden.
  return collapsed ? (
    <Tip label={label} side="right">
      {el}
    </Tip>
  ) : (
    el
  );
}

/**
 * The phone's bottom bar: items share the width, icon over label, the active icon on a row-surface
 * pill. Padded by the home-indicator inset; fixed, so the page scrolls under nothing.
 */
export function NavTabBar({ page }: { page: PageId }) {
  return (
    <nav aria-label="Main" className="fixed inset-x-0 bottom-0 z-30 bg-card pb-[env(safe-area-inset-bottom)]">
      <ul className="m-0 flex list-none gap-1 p-1">
        {NAV_ITEMS.map((item) => {
          const active = item.id === page;
          return (
            <li key={item.id} className="flex min-w-0 flex-1">
              <a
                href={item.href}
                aria-current={active ? "page" : undefined}
                className={cn(
                  "flex w-full flex-col items-center justify-center gap-0.5 rounded-lg px-1 py-1.5 text-[11px] font-medium no-underline outline-none select-none transition-colors duration-150 focus-visible:ring-2 focus-visible:ring-ring",
                  active ? "text-foreground" : "text-muted-foreground hover:text-foreground",
                )}
              >
                <span className={cn("flex h-7 w-12 items-center justify-center rounded-full transition-colors duration-150 [&_svg]:size-5", active && "bg-bg-row text-primary")}>
                  <Icon name={item.icon} />
                </span>
                <span className="max-w-full truncate">{item.label}</span>
              </a>
            </li>
          );
        })}
      </ul>
    </nav>
  );
}

/** Height the fixed bottom bar takes (without the inset), for the page's bottom padding. */
export const TAB_BAR_HEIGHT = "4rem";

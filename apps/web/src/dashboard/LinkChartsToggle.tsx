import { Icon } from "@stockwatcher/chart-web";
import { cn, Tip, TOUCH_HIT } from "@stockwatcher/chart-web/ui";

/**
 * The panel's "link all charts" toggle (owner, 2026-11-15): a paperclip just left of the
 * multi-view control. While on, a ticker picked anywhere (search, a panel row, a notification)
 * loads into every chart shown instead of only the selected one(s); off is the usual one-chart
 * behaviour. Neutral at rest; on, its icon takes the accent duotone like every active control
 * (aria-pressed). No border: the surface step is the only frame. 32 px, 40 px with a 44 px touch
 * area on a coarse pointer, matching the multi-view control beside it so the header never shifts.
 */
export function LinkChartsToggle({ linked, onChange }: { linked: boolean; onChange: (linked: boolean) => void }) {
  return (
    <Tip label={linked ? "Linked: a ticker loads into every chart" : "Link all charts to the next ticker"}>
      <button
        type="button"
        aria-label="Link all charts"
        aria-pressed={linked}
        onClick={() => onChange(!linked)}
        className={cn(
          "relative inline-grid size-8 shrink-0 place-items-center rounded-lg bg-card text-muted-foreground outline-none transition-colors hover:bg-bg-row hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring aria-pressed:bg-bg-row aria-pressed:text-foreground aria-pressed:[&_svg]:text-primary pointer-coarse:size-10 [&_svg]:size-4",
          TOUCH_HIT,
        )}
      >
        <Icon name="paperclip" />
      </button>
    </Tip>
  );
}

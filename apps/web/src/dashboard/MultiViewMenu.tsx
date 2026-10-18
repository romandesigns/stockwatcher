import { MAX_DASHBOARD_CHARTS } from "@stockwatcher/app-state";
import { Icon } from "@stockwatcher/chart-web";
import { DropdownMenu, DropdownMenuContent, DropdownMenuRadioGroup, DropdownMenuRadioItem, DropdownMenuTrigger, Tip } from "@stockwatcher/chart-web/ui";

const COUNTS = Array.from({ length: MAX_DASHBOARD_CHARTS }, (_, i) => i + 1);

/**
 * The panel's multi-view control (owner, 2026-10-18): a screen icon × n, n being the charts shown.
 * It opens a menu of 1–4. Neutral at rest; while open its icon takes the accent duotone, like
 * every open menu button.
 */
export function MultiViewMenu({ count, onChange }: { count: number; onChange: (n: number) => void }) {
  return (
    <DropdownMenu>
      <Tip label="Charts in the panel">
        <DropdownMenuTrigger
          aria-label={`${count} ${count === 1 ? "chart" : "charts"} shown, change`}
          className="flex h-8 shrink-0 items-center gap-1 rounded-lg bg-card px-2.5 text-xs text-muted-foreground outline-none transition-colors hover:bg-bg-row hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring aria-expanded:bg-bg-row aria-expanded:text-foreground aria-expanded:[&_svg]:text-primary [&_svg]:size-4"
        >
          <Icon name="screen" />
          <span className="font-mono tabular-nums">×{count}</span>
        </DropdownMenuTrigger>
      </Tip>
      <DropdownMenuContent align="end" className="min-w-36">
        <DropdownMenuRadioGroup value={String(count)} onValueChange={(v) => onChange(Number(v))}>
          {COUNTS.map((n) => (
            // One choice, then out of the way (Base UI keeps a menu open after a radio pick by default).
            <DropdownMenuRadioItem key={n} value={String(n)} closeOnClick>
              {n} {n === 1 ? "chart" : "charts"}
            </DropdownMenuRadioItem>
          ))}
        </DropdownMenuRadioGroup>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

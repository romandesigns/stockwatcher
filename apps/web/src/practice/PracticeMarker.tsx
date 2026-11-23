import type { PaperState } from "@stockwatcher/chart-core";
import { Icon } from "@stockwatcher/chart-web";
import { cn, Popover, PopoverContent, PopoverTrigger, Tip, TOUCH_HIT } from "@stockwatcher/chart-web/ui";
import type { OffPanel } from "./offPanels.ts";

/**
 * The practice page's marker (owner, 2026-11-23: it must never be mistaken for live): the replay
 * mark and "Practice · <day>", in the quiet neutral chip the source chips use for replay. Never
 * the accent, which stays for the page's one primary action (60/30/10). It opens a short note on
 * what practice is and which live panels it does not have yet. "Practice" always reads in full;
 * the day joins it short from 1100 px and long from 1440 px (the replay bar always shows it).
 */
export function PracticeBadge({ day, dayShort, off, className }: { day: string | null; dayShort: string | null; off: readonly OffPanel[]; className?: string }) {
  return (
    <Popover>
      <PopoverTrigger
        aria-label={`Practice${day ? `, ${day}` : ""}: replay, not live. About practice`}
        className={cn(
          "relative inline-flex h-7 shrink-0 items-center gap-1.5 rounded-full bg-card px-2.5 text-xs font-semibold text-muted-foreground outline-none transition-colors hover:bg-bg-row hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring data-popup-open:bg-bg-row pointer-coarse:h-9 [&_svg]:size-3.5 [&_svg]:shrink-0",
          TOUCH_HIT,
          className,
        )}
      >
        <Icon name="replay" />
        <span className="shrink-0">Practice</span>
        {day && (
          <>
            <span className="hidden min-[1100px]:max-[1439px]:inline">· {dayShort ?? day}</span>
            <span className="hidden min-[1440px]:inline">· {day}</span>
          </>
        )}
      </PopoverTrigger>
      <PopoverContent side="bottom" align="start" className="w-72">
        <div className="flex flex-col gap-2 text-xs">
          <p className="m-0 text-sm font-semibold text-foreground">Practice{day ? ` · ${day}` : ""}</p>
          <p className="m-0 text-muted-foreground">A recorded day on the replay clock, with a simulated account that starts flat on every seek or day switch. Nothing on this page is live market data.</p>
          <p className="m-0 text-muted-foreground">Practice alerts show here only, marked Practice: never on your phone or as a system notification. Real alerts that fire meanwhile are marked Live.</p>
          <div className="flex flex-col gap-1">
            <p className="m-0 font-semibold text-foreground">Not in practice yet</p>
            <ul className="m-0 flex list-none flex-col gap-0.5 p-0 text-text-faint">
              {off.map((p) => (
                <li key={p.id}>{p.label}: live market only for now</li>
              ))}
              <li>Research shadow alerts (N1, c4, x-halt, x-float, gap/ctx, rw2, b-good)</li>
            </ul>
          </div>
        </div>
      </PopoverContent>
    </Popover>
  );
}

/**
 * "Not in practice yet: …": the panels a live session would show that this one cannot, said once
 * in a quiet line rather than as empty panels (a panel with no items is hidden, owner 2026-10-22).
 */
export function NotInPractice({ panels, className }: { panels: readonly OffPanel[]; className?: string }) {
  if (panels.length === 0) return null;
  const names = panels.map((p) => p.label).join(", ");
  return (
    <Tip label={`${names}: live market only for now. They open here once they run on the replay clock. Research shadow alerts are not in practice yet either.`}>
      <span className={cn("min-w-0 truncate text-xs text-text-faint", className)}>Not in practice yet: {names}</span>
    </Tip>
  );
}

const money = (v: number) => `${v < 0 ? "−" : v > 0 ? "+" : ""}$${Math.abs(v).toFixed(2)}`;

/**
 * The practice account at a glance: realized P&L (good/critical only because they carry meaning)
 * and how many positions are open. The chart's toolbar shows the shown ticker's position itself.
 */
export function PaperSummary({ state, className }: { state: PaperState | null; className?: string }) {
  if (!state) return null;
  const pnl = state.account.realizedPnl;
  const open = state.positions.filter((p) => p.qty !== 0).length;
  return (
    <Tip label="The practice account (simulated). It starts flat again on every seek or day switch.">
      <span className={cn("inline-flex shrink-0 items-baseline gap-1.5 text-xs", className)}>
        <span className="text-text-faint">Paper P&amp;L</span>
        <span className={cn("font-mono tabular-nums", pnl > 0 ? "text-good" : pnl < 0 ? "text-critical" : "text-muted-foreground")}>{money(pnl)}</span>
        {open > 0 && <span className="text-text-faint">· {open} open</span>}
      </span>
    </Tip>
  );
}

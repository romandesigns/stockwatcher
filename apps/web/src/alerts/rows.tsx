import type { AlertDisplayKind } from "@stockwatcher/app-state";
import type { AlertEventWire } from "@stockwatcher/shared";
import { formatEtTime } from "@stockwatcher/chart-core";
import { Icon } from "@stockwatcher/chart-web";
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuSeparator, DropdownMenuTrigger, cn } from "@stockwatcher/chart-web/ui";
import { TickerLabel } from "../lib/TickerLabel.tsx";

// The Alerts page's two rows (approved design, "Alerts manager"), also in wavykit as alert-row
// and activity-row: no borders, the row a step up from the page, the state as a soft chip whose
// accent is spent only on "Armed" (the thing still watching) and the good tone only on "Fired".

/** State chip colours by kind: tinted surfaces, never a border. */
const CHIP: Readonly<Record<AlertDisplayKind, string>> = {
  armed: "bg-primary/15 text-primary",
  cooling: "bg-bg-raised text-muted-foreground",
  snoozed: "bg-bg-raised text-text-faint",
  fired: "bg-good-bg text-good",
  expired: "bg-bg-raised text-text-faint",
  // The drawing is gone: a warning status (the alert silently watches nothing), worded.
  orphaned: "bg-warning-bg text-warning",
};

export interface AlertRowAction {
  label: string;
  onSelect: () => void;
  destructive?: boolean;
}

export interface AlertRowProps {
  symbol: string;
  /** "Price crosses above 2.50". */
  rule: string;
  /** "Once · pre, regular, after · expires Fri". */
  meta: string;
  state: { kind: AlertDisplayKind; label: string };
  actions: readonly (AlertRowAction | "separator")[];
  /** The row itself opens the alert (edit). */
  onOpen?: () => void;
  /** The ticker opens its chart (nav/openTicker.ts; owner, 2026-10-23: every ticker shown opens its own chart). */
  onOpenChart?: () => void;
}

export function AlertRow({ symbol, rule, meta, state, actions, onOpen, onOpenChart }: AlertRowProps) {
  return (
    <div className="grid grid-cols-[4.5rem_minmax(0,1fr)_auto_auto] items-center gap-3 rounded-[10px] bg-card py-2 pr-1.5 pl-3 @max-md/alerts:grid-cols-[minmax(0,1fr)_auto]">
      {onOpenChart ? (
        <TickerLabel symbol={symbol} onTap={onOpenChart} aria-label={`${symbol}, open chart`} className="justify-self-start font-mono text-[13.5px] @max-md/alerts:hidden" />
      ) : (
        <span className="font-mono text-[13.5px] font-semibold @max-md/alerts:hidden">{symbol}</span>
      )}
      <button type="button" onClick={onOpen} className="flex min-w-0 flex-col items-start gap-0.5 rounded-md text-left outline-none focus-visible:ring-2 focus-visible:ring-ring">
        <span className="max-w-full truncate text-[13px]">
          <span className="mr-1.5 hidden font-mono font-semibold @max-md/alerts:inline">{symbol}</span>
          {rule}
        </span>
        <span className="max-w-full truncate text-[11.5px] text-text-faint">{meta}</span>
      </button>
      <span className={cn("inline-flex h-[22px] items-center rounded-full px-2.5 text-[11px] font-medium whitespace-nowrap @max-md/alerts:hidden", CHIP[state.kind])}>{state.label}</span>
      <DropdownMenu>
        <DropdownMenuTrigger
          aria-label={`Alert options for ${symbol}: ${rule}, ${state.label}`}
          className="inline-grid size-[30px] place-items-center rounded-md text-muted-foreground outline-none hover:bg-bg-row hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring [&_svg]:size-3.5"
        >
          <Icon name="more" />
        </DropdownMenuTrigger>
        <DropdownMenuContent align="end" className="w-48">
          {actions.map((a, i) =>
            a === "separator" ? (
              <DropdownMenuSeparator key={`s${i}`} />
            ) : (
              <DropdownMenuItem key={a.label} onClick={a.onSelect} className={a.destructive ? "text-critical" : undefined}>
                {a.label}
              </DropdownMenuItem>
            ),
          )}
        </DropdownMenuContent>
      </DropdownMenu>
    </div>
  );
}

/** One Activity line: time (ET), ticker, what happened, and how it was delivered or why it was held. */
export function ActivityRow({ event, onOpenChart }: { event: AlertEventWire; onOpenChart?: (symbol: string) => void }) {
  const held = event.kind === "held" || event.phone === "held" || event.phone === "failed";
  return (
    <div className="grid grid-cols-[3.9rem_4rem_minmax(0,1fr)] items-baseline gap-2.5 px-1 py-2">
      <span className="font-mono text-xs text-muted-foreground">{formatEtTime(event.t, true)}</span>
      {onOpenChart ? (
        <TickerLabel symbol={event.symbol} onTap={() => onOpenChart(event.symbol)} aria-label={`${event.symbol}, open chart`} className="justify-self-start font-mono text-[12.5px]" />
      ) : (
        <span className="font-mono text-[12.5px] font-semibold">{event.symbol}</span>
      )}
      <span className="flex min-w-0 flex-col gap-0.5">
        <span className="text-[12.5px]">{event.text}</span>
        <span className={cn("text-[11px]", held ? "text-warning" : "text-text-faint")}>{event.note}</span>
      </span>
    </div>
  );
}

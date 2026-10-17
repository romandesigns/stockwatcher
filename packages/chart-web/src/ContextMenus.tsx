import { formatEtTime, formatPrice, formatSize, LAYER_IDS, LAYER_INFO, LAYER_SWATCHES, type ChartLayers, type FillMark, type LayerId } from "@stockwatcher/chart-core";
import type { ReactNode } from "react";
import { Button } from "./ui/button.tsx";
import { DropdownMenu, DropdownMenuCheckboxItem, DropdownMenuContent, DropdownMenuGroup, DropdownMenuLabel, DropdownMenuTrigger } from "./ui/dropdown-menu.tsx";
import { ScrollArea } from "./ui/scroll-area.tsx";
import { Tip } from "./ui/tooltip.tsx";
import { cn } from "./ui/utils.ts";

const icon = (children: ReactNode) => (
  <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" aria-hidden="true">
    {children}
  </svg>
);

/** Levels: three stacked lines, the dashed one a level. */
const LevelsIcon = icon(
  <>
    <path d="M1.5 4h13" strokeDasharray="2 2" />
    <path d="M1.5 8h13" />
    <path d="M1.5 12h13" strokeOpacity="0.5" />
  </>,
);

const SWATCH_COLOR = { warning: "#e0a63a", dim: "#98a2ae", text: "#e4e8ed", good: "#3fb97a", critical: "#f0645c" } as const;

/** The swatch each layer row shows: how that layer looks on the chart (shapes shared with the phone). */
function LayerSwatch({ id }: { id: LayerId }) {
  return (
    <svg viewBox="0 0 24 16" width="24" height="16" aria-hidden="true">
      {LAYER_SWATCHES[id].map((g, i) =>
        g.paint === "fill" ? (
          <path key={i} d={g.d} fill={SWATCH_COLOR[g.color]} fillOpacity={g.opacity} />
        ) : (
          <path key={i} d={g.d} fill="none" stroke={SWATCH_COLOR[g.color]} strokeOpacity={g.opacity} strokeWidth="1.5" strokeDasharray={g.dash?.join(" ")} />
        ),
      )}
    </svg>
  );
}

/**
 * The chart's context layers, each a switch: halts, LULD bands, the key
 * levels (premarket, day range, open, previous close) and your fills. Every
 * row shows how the layer looks on the chart, so the menu is also its key.
 */
export function LayersMenu({ layers, ids = LAYER_IDS, onChange }: { layers: ChartLayers; ids?: readonly LayerId[]; onChange: (l: ChartLayers) => void }) {
  const on = ids.filter((id) => layers[id]).length;
  return (
    <DropdownMenu>
      <Tip label="Levels">
        <DropdownMenuTrigger aria-label={`Levels, ${on} of ${ids.length} on`} render={<Button variant="ghost" size="sm" className="h-7 min-w-7 rounded-md px-1.5 text-muted-foreground [&_svg]:size-3.5" />}>
          {LevelsIcon}
        </DropdownMenuTrigger>
      </Tip>
      <DropdownMenuContent align="end" aria-label="Levels" className="w-80 p-1.5">
        {/* Base UI requires a group around a menu label: without it the menu throws and takes the app down. */}
        <DropdownMenuGroup>
        <DropdownMenuLabel className="flex justify-between px-1.5 pt-1.5 pb-1 text-[11px] font-medium text-muted-foreground">
          On the chart
          <span className={cn("tabular-nums", on === ids.length ? "text-foreground" : "text-text-faint")}>{`${on}/${ids.length}`}</span>
        </DropdownMenuLabel>
        {ids.map((id) => (
          <DropdownMenuCheckboxItem key={id} checked={layers[id]} onCheckedChange={(v) => onChange({ ...layers, [id]: !!v })} closeOnClick={false} className="gap-2.5 py-1.5 pr-9">
            <span className={cn("grid h-8 w-10 shrink-0 place-items-center rounded-lg", layers[id] ? "bg-bg-raised" : "bg-bg-row opacity-60")}>
              <LayerSwatch id={id} />
            </span>
            <span className="flex min-w-0 flex-col gap-0.5">
              <span className="text-[13px] font-medium">{LAYER_INFO[id].name}</span>
              <span className="truncate text-[11px] text-text-faint">{LAYER_INFO[id].description}</span>
            </span>
          </DropdownMenuCheckboxItem>
        ))}
        </DropdownMenuGroup>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

const ROLE: Record<string, string> = { take_profit: "TP", stop_loss: "SL", entry: "Entry" };

/**
 * Your simulated fills for this symbol, newest first, with realized P&L:
 * the bottom of the Trade widget, under its action buttons. The same fills
 * are drawn on the chart as arrows (the "My fills" layer).
 */
export function TradeHistory({ fills, realized, className }: { fills: readonly FillMark[]; realized: number; className?: string }) {
  const list = [...fills].reverse();
  return (
    <section aria-label="Trade history" className={cn("flex flex-col gap-1", className)}>
      <div className="flex items-baseline justify-between">
        <h3 className="m-0 text-[11px] font-medium text-muted-foreground">Trades</h3>
        <span className="text-[11px] text-muted-foreground">
          Realized{" "}
          <span className={cn("font-mono tabular-nums", realized > 0 ? "text-good" : realized < 0 ? "text-critical" : "text-foreground")}>
            {realized >= 0 ? "+" : "\u2212"}${Math.abs(realized).toFixed(2)}
          </span>
        </span>
      </div>
      {list.length === 0 ? (
        <p className="m-0 text-[11px] text-text-faint">No fills yet.</p>
      ) : (
        <ScrollArea viewportClassName="max-h-36">
        <ul className="m-0 flex list-none flex-col gap-0.5 p-0 pr-2">
          {list.map((f, i) => (
            <li key={`${f.orderId}-${i}`} className="grid grid-cols-[52px_32px_minmax(0,1fr)_auto] items-baseline gap-1.5 rounded-md px-1 py-0.5 font-mono text-[11px] tabular-nums odd:bg-bg-row/50">
              <span className="text-muted-foreground">{formatEtTime(f.t, true)}</span>
              <span className={f.side === "buy" ? "text-good" : "text-critical"}>{f.side === "buy" ? "Buy" : "Sell"}</span>
              <span className="truncate">
                {formatSize(f.qty)} @ {formatPrice(f.price)}
              </span>
              <span className="font-sans text-[10px] text-text-faint">{f.role ? ROLE[f.role] : ""}</span>
            </li>
          ))}
        </ul>
        </ScrollArea>
      )}
    </section>
  );
}

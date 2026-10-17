import { formatPrice, formatSize, orderTypeAt, priceDecimals, type OrderSide } from "@stockwatcher/chart-core";
import { useMemo, type RefObject } from "react";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuGroup,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuSeparator,
} from "./ui/dropdown-menu.tsx";
import { cn } from "./ui/utils.ts";

export const TRADE_MENU_SIZES = [100, 500, 1000] as const;

export interface TradeMenuProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** Viewport (client) px: exactly where the right-click happened. */
  anchor: { x: number; y: number } | null;
  symbol: string;
  /** The price under the cursor, already on the tick grid. */
  price: number;
  /** Last trade, which decides limit vs stop for each side. */
  last: number | null;
  qty: number;
  onQtyChange: (qty: number) => void;
  /** Place at once: a resting order at `price`, or market when type is "market". */
  onOrder: (side: OrderSide, type: "limit" | "stop" | "market") => void;
  /** Open the full order ticket at `price`. */
  onTicket: () => void;
  /** Drop a long or short position plan with its entry at this point. */
  onPosition: (kind: "long" | "short") => void;
  finalFocus?: RefObject<HTMLElement | null>;
}

const TYPE_LABEL = { limit: "limit", stop: "stop", market: "market" } as const;

/**
 * The right-click trade menu (owner, 2026-10-17: practise by right-clicking
 * the chart). It opens exactly where the click was, with the price at that
 * height. One click places a simulated order at that price: the resting
 * order it means for each side (below the market a buy limit or a sell
 * stop, above it a sell limit or a buy stop, Robinhood Legend's rule), or a
 * market order; the share size is picked once and remembered. It also
 * opens the full ticket, or drops a position plan, at the same point.
 */
export function TradeMenu({ open, onOpenChange, anchor, symbol, price, last, qty, onQtyChange, onOrder, onTicket, onPosition, finalFocus }: TradeMenuProps) {
  const virtual = useMemo(() => {
    if (!anchor) return null;
    const { x, y } = anchor;
    return { getBoundingClientRect: () => DOMRect.fromRect({ x, y, width: 0, height: 0 }) };
  }, [anchor]);
  const buyType = last === null ? "limit" : orderTypeAt("buy", price, last);
  const sellType = last === null ? "limit" : orderTypeAt("sell", price, last);
  const p = formatPrice(price);
  const vs = last === null ? null : price < last ? "below" : price > last ? "above" : "at";

  return (
    <DropdownMenu open={open && virtual !== null} onOpenChange={(o) => onOpenChange(o)}>
      <DropdownMenuContent anchor={virtual} side="bottom" align="start" sideOffset={2} finalFocus={finalFocus} aria-label={`Trade ${symbol} at ${p}`} className="w-64 p-1.5">
        {/* Same header as the phone's quick-trade sheet and the order ticket. */}
        <div className="flex items-center gap-1.5 px-1.5 pt-1 pb-1.5">
          <span className="text-[13px] font-semibold">Trade {symbol}</span>
          <span className="rounded bg-background px-1.5 py-0.5 font-mono text-[11px]">{p}</span>
          <span className="rounded bg-warning-bg px-1.5 py-0.5 text-[9px] font-semibold tracking-wider text-warning">SIMULATED</span>
        </div>
        {vs && (
          <p className="m-0 px-1.5 pb-1.5 text-[11px] text-text-faint">
            {vs === "at" ? "At the last trade" : `${formatPrice(Math.abs(price - (last ?? price)), priceDecimals(price))} ${vs} the last trade`}
          </p>
        )}
        {/* Size first: it is what the items below place. */}
        <DropdownMenuRadioGroup value={String(qty)} onValueChange={(v) => onQtyChange(Number(v))}>
          <DropdownMenuLabel className="px-1.5 pt-1 pb-1 text-[11px] font-medium text-muted-foreground">Shares</DropdownMenuLabel>
          <div className="grid grid-cols-3 gap-1 px-1 pb-1">
            {TRADE_MENU_SIZES.map((n) => (
              <DropdownMenuRadioItem
                key={n}
                value={String(n)}
                closeOnClick={false}
                className={cn("justify-center rounded-md bg-bg-row py-1 pr-1.5 pl-1.5 font-mono text-xs data-highlighted:bg-bg-raised [&_[data-slot=dropdown-menu-radio-item-indicator]]:hidden", qty === n && "bg-bg-raised text-foreground")}
              >
                {formatSize(n)}
              </DropdownMenuRadioItem>
            ))}
          </div>
        </DropdownMenuRadioGroup>
        <DropdownMenuSeparator />
        <DropdownMenuGroup>
          <DropdownMenuItem onClick={() => onOrder("buy", buyType)} className="gap-2">
            <span aria-hidden="true" className="size-2 rounded-full bg-good" />
            <span>
              Buy {TYPE_LABEL[buyType]} <span className="font-mono">{formatSize(qty)}</span> @ <span className="font-mono">{p}</span>
            </span>
          </DropdownMenuItem>
          <DropdownMenuItem onClick={() => onOrder("sell", sellType)} className="gap-2">
            <span aria-hidden="true" className="size-2 rounded-full bg-critical" />
            <span>
              Sell {TYPE_LABEL[sellType]} <span className="font-mono">{formatSize(qty)}</span> @ <span className="font-mono">{p}</span>
            </span>
          </DropdownMenuItem>
        </DropdownMenuGroup>
        <DropdownMenuSeparator />
        <DropdownMenuGroup>
          <DropdownMenuItem onClick={() => onOrder("buy", "market")} className="text-muted-foreground">
            Buy <span className="font-mono">{formatSize(qty)}</span> at market
          </DropdownMenuItem>
          <DropdownMenuItem onClick={() => onOrder("sell", "market")} className="text-muted-foreground">
            Sell <span className="font-mono">{formatSize(qty)}</span> at market
          </DropdownMenuItem>
        </DropdownMenuGroup>
        <DropdownMenuSeparator />
        <DropdownMenuGroup>
          <DropdownMenuItem onClick={onTicket}>Order ticket at {p}…</DropdownMenuItem>
          <DropdownMenuItem onClick={() => onPosition("long")}>
            Long position here
          </DropdownMenuItem>
          <DropdownMenuItem onClick={() => onPosition("short")}>
            Short position here
          </DropdownMenuItem>
        </DropdownMenuGroup>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

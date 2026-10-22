import { formatPrice, orderTypeAt, snapPrice, tickSize, type FillMark, type OrderPlace, type OrderSide, type OrderType } from "@stockwatcher/chart-core";
import { TradeHistory } from "./ContextMenus.tsx";
import { useEffect, useMemo, useState, type RefObject } from "react";
import { Button } from "./ui/button.tsx";
import { Input } from "./ui/input.tsx";
import { Popover, PopoverContent } from "./ui/popover.tsx";
import { ToggleGroup, ToggleGroupItem } from "./ui/toggle-group.tsx";
import { cn } from "./ui/utils.ts";

export type OrderTicketOrder = Omit<OrderPlace, "type" | "clientOrderId">;

export interface OrderTicketProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** Viewport (client) px to open at: the "+" button, or the Trade button. */
  anchor: { x: number; y: number } | null;
  symbol: string;
  /** Last trade, for the side/type default and the market estimate. */
  last: number | null;
  /** Price the ticket was opened at (the "+" button); null opens a market order. */
  price: number | null;
  onSubmit: (order: OrderTicketOrder) => void;
  finalFocus?: RefObject<HTMLElement | null>;
  /** This symbol's simulated fills and realized P&L: the history at the bottom of the widget. */
  fills?: readonly FillMark[];
  realized?: number;
}

const TYPES: readonly { v: Exclude<OrderType, "stop_limit">; label: string }[] = [
  { v: "market", label: "Market" },
  { v: "limit", label: "Limit" },
  { v: "stop", label: "Stop" },
];

const QUICK_QTY = [100, 500, 1000] as const;

// 44 px and 16 px text on touch: a finger-size field, and iOS does not zoom into a field of 16 px or more.
const inputClass = "h-8 w-full rounded-md bg-bg-row px-2 font-mono text-sm tabular-nums text-foreground outline-none focus-visible:ring-2 focus-visible:ring-ring/50 pointer-coarse:h-11 pointer-coarse:text-base";

/**
 * The simulated order ticket. Opened at a price (the "+" button), it starts
 * as the resting order that price means (Robinhood Legend's rule: below the
 * market a buy limit or sell stop, above it a sell limit or buy stop) and
 * follows the side until the type is picked by hand. Prices snap to the tick
 * grid the broker accepts. Nothing here talks to a server: it hands the order
 * to onSubmit.
 */
export function OrderTicket({ open, onOpenChange, anchor, symbol, last, price, onSubmit, finalFocus, fills, realized = 0 }: OrderTicketProps) {
  const [side, setSide] = useState<OrderSide>("buy");
  const [type, setType] = useState<Exclude<OrderType, "stop_limit">>("market");
  const [typeByHand, setTypeByHand] = useState(false);
  const [qty, setQty] = useState("100");
  const [px, setPx] = useState("");

  // Each opening starts from where it was opened.
  useEffect(() => {
    if (!open) return;
    setSide("buy");
    setTypeByHand(false);
    if (price !== null && last !== null) {
      setType(orderTypeAt("buy", price, last));
      setPx(formatPrice(snapPrice(price)));
    } else {
      setType("market");
      setPx(last !== null ? formatPrice(snapPrice(last)) : "");
    }
  }, [open, price, last]);

  const changeSide = (s: OrderSide) => {
    setSide(s);
    const p = Number(px);
    if (!typeByHand && type !== "market" && last !== null && Number.isFinite(p)) setType(orderTypeAt(s, p, last));
  };

  const virtual = useMemo(() => {
    if (!anchor) return null;
    const { x, y } = anchor;
    return { getBoundingClientRect: () => DOMRect.fromRect({ x, y, width: 0, height: 0 }) };
  }, [anchor]);

  const n = Number(qty);
  const qtyOk = Number.isInteger(n) && n > 0 && n <= 1_000_000;
  const p = Number(px);
  const priceOk = type === "market" || (Number.isFinite(p) && p > 0);
  const snapped = priceOk && type !== "market" ? snapPrice(p) : null;
  const est = type === "market" ? last : snapped;

  const submit = () => {
    if (!qtyOk || !priceOk) return;
    const order: OrderTicketOrder = { symbol, side, orderType: type, qty: n };
    if (type === "limit" && snapped !== null) order.limitPrice = snapped;
    if (type === "stop" && snapped !== null) order.stopPrice = snapped;
    onSubmit(order);
    onOpenChange(false);
  };

  return (
    <Popover open={open && virtual !== null} onOpenChange={(o) => onOpenChange(o)}>
      <PopoverContent anchor={virtual} finalFocus={finalFocus} aria-label={`Simulated order, ${symbol}`}>
        <form
          className="flex flex-col gap-2.5"
          onSubmit={(e) => {
            e.preventDefault();
            submit();
          }}
        >
          {/* Same layout as the phone's ticket: "Buy SMPL" with a simulated chip, recessed controls, one shares row. */}
          <div className="flex items-center gap-2">
            <span className="text-sm font-semibold">
              {side === "buy" ? "Buy" : "Sell"} {symbol}
            </span>
            <span className="rounded bg-warning-bg px-1.5 py-0.5 text-[9px] font-semibold tracking-wider text-warning">SIMULATED</span>
          </div>
          <ToggleGroup aria-label="Side" size="sm" spacing={0.5} variant="segmented" className="w-full bg-background p-0.5" value={[side]} onValueChange={(v) => v[0] && changeSide(v[0] as OrderSide)}>
            <ToggleGroupItem value="buy" className="flex-1 aria-pressed:bg-good-bg aria-pressed:text-good">
              Buy
            </ToggleGroupItem>
            <ToggleGroupItem value="sell" className="flex-1 aria-pressed:bg-critical-bg aria-pressed:text-critical">
              Sell
            </ToggleGroupItem>
          </ToggleGroup>
          <ToggleGroup
            aria-label="Order type"
            size="sm"
            spacing={0.5}
            variant="segmented"
            className="w-full bg-background p-0.5"
            value={[type]}
            onValueChange={(v) => {
              if (!v[0]) return;
              setType(v[0] as typeof type);
              setTypeByHand(true);
            }}
          >
            {TYPES.map((t) => (
              <ToggleGroupItem key={t.v} value={t.v} className="flex-1 text-xs">
                {t.label}
              </ToggleGroupItem>
            ))}
          </ToggleGroup>
          <div className="flex flex-col gap-1">
            <label htmlFor="ticket-qty" className="text-[11px] font-medium text-muted-foreground">
              Shares
            </label>
            <div className="flex gap-1">
              <Input id="ticket-qty" className={cn(inputClass, "min-w-0 flex-1")} inputMode="numeric" value={qty} onChange={(e) => setQty(e.target.value.replace(/[^\d]/g, ""))} aria-invalid={!qtyOk} />
              {QUICK_QTY.map((q) => (
                <Button
                  key={q}
                  type="button"
                  variant="ghost"
                  size="sm"
                  aria-label={`${q} shares`}
                  className={cn("h-8 w-10 shrink-0 rounded-md bg-bg-row font-mono text-[11px] text-muted-foreground", qty === String(q) && "bg-bg-raised text-foreground")}
                  onClick={() => setQty(String(q))}
                >
                  {q === 1000 ? "1K" : q}
                </Button>
              ))}
            </div>
          </div>
          {type !== "market" && (
            <label className="flex flex-col gap-1 text-[11px] text-muted-foreground">
              {type === "limit" ? "Limit price" : "Stop price"}
              <Input
                className={inputClass}
                inputMode="decimal"
                value={px}
                onChange={(e) => setPx(e.target.value.replace(/[^\d.]/g, ""))}
                onBlur={() => Number.isFinite(p) && p > 0 && setPx(formatPrice(snapPrice(p)))}
                aria-invalid={!priceOk}
                step={Number.isFinite(p) ? tickSize(p) : 0.01}
              />
            </label>
          )}
          <div className="flex items-baseline justify-between text-[11px] text-muted-foreground">
            <span>{side === "buy" ? "Est. cost" : "Est. proceeds"}</span>
            <span className="font-mono text-foreground tabular-nums">{est !== null && qtyOk ? `$${(est * n).toLocaleString("en-US", { minimumFractionDigits: 2, maximumFractionDigits: 2 })}` : "—"}</span>
          </div>
          <Button type="submit" className={cn("h-9 w-full rounded-lg text-[13px] font-semibold")} disabled={!qtyOk || !priceOk}>
            {side === "buy" ? "Buy" : "Sell"} {qtyOk ? n.toLocaleString("en-US") : ""} {symbol} · {type === "market" ? "Market" : `${type === "limit" ? "Limit" : "Stop"} ${snapped !== null ? formatPrice(snapped) : ""}`}
          </Button>
          <p className="m-0 text-center text-[10px] leading-snug text-text-faint">Fills against the replay tape · no money, no broker</p>
        </form>
        {fills && <TradeHistory fills={fills} realized={realized} className="mt-2.5 rounded-lg bg-background p-2.5" />}
      </PopoverContent>
    </Popover>
  );
}

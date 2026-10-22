import { flipPosition, formatPrice, formatSize, positionPlan, type Drawing } from "@stockwatcher/chart-core";
import { useEffect, useState } from "react";
import { Button } from "./ui/button.tsx";
import { Input } from "./ui/input.tsx";
import { ToggleGroup, ToggleGroupItem } from "./ui/toggle-group.tsx";
import { cn } from "./ui/utils.ts";

export interface PositionPlanCardProps {
  /** The selected long or short drawing. */
  drawing: Drawing;
  onChange: (d: Drawing) => void;
  /** Place the plan as a simulated bracket; absent = planning only (no trading on this chart). */
  onPlace?: () => void;
  className?: string;
}

// 44 px and 16 px text on touch: a finger-size field, and iOS does not zoom into a field of 16 px or more.
const inputClass = "h-8 w-full rounded-md bg-bg-row px-2 font-mono text-[13px] tabular-nums text-foreground outline-none focus-visible:ring-2 focus-visible:ring-ring/50 pointer-coarse:h-11 pointer-coarse:text-base";
const money = (v: number) => `$${Math.abs(v).toLocaleString("en-US", { minimumFractionDigits: 2, maximumFractionDigits: 2 })}`;
// Stats are glanceable: whole dollars from $100 up, so "−$100" fits its tile.
const short = (v: number) => `$${Math.abs(v) >= 100 ? Math.round(Math.abs(v)).toLocaleString("en-US") : Math.abs(v).toFixed(2)}`;

/**
 * The position tool's plan: side, risk budget or share count, the three legs
 * with the order each becomes, reward, risk, R:R and value, and Place
 * bracket (design canvas https://claude.ai/artifact/UkBuQdEMyZjAYmdMsTzatW).
 * Edits write straight back into the drawing, so the zones on the chart and
 * this card are always the same plan.
 */
export function PositionPlanCard({ drawing, onChange, onPlace, className }: PositionPlanCardProps) {
  const plan = positionPlan(drawing);
  const long = plan.side === "long";
  // Inputs keep what is being typed; the drawing gets a value only when it parses.
  const [risk, setRisk] = useState(String(plan.riskBudget));
  const [qty, setQty] = useState(String(plan.qty));
  useEffect(() => setRisk(String(plan.riskBudget)), [plan.riskBudget]);
  useEffect(() => setQty(String(plan.qty)), [plan.qty]);

  const legs: { label: string; price: number; swatch: string }[] = [
    { label: `Target · ${long ? "sell" : "buy"} limit`, price: plan.target, swatch: "bg-good" },
    { label: `Entry · ${long ? "buy" : "sell"} limit`, price: plan.entry, swatch: "bg-foreground" },
    { label: `Stop · ${long ? "sell" : "buy"} stop`, price: plan.stop, swatch: "bg-critical" },
  ];

  return (
    <section aria-label="Position plan" className={cn("flex w-64 flex-col gap-2.5 rounded-xl bg-card p-3 text-card-foreground shadow-lg transition-[opacity,translate] duration-150 starting:-translate-y-1 starting:opacity-0", className)}>
      <div className="flex items-baseline justify-between">
        <h2 className="m-0 text-xs font-medium text-muted-foreground">Position plan</h2>
        <span className="text-[10px] font-medium tracking-wide text-warning">SIMULATED</span>
      </div>
      <ToggleGroup aria-label="Side" size="sm" spacing={0.5} variant="segmented" className="w-full" value={[plan.side]} onValueChange={(v) => v[0] && v[0] !== plan.side && onChange(flipPosition(drawing))}>
        <ToggleGroupItem value="long" className="flex-1 aria-pressed:text-good">
          Long
        </ToggleGroupItem>
        <ToggleGroupItem value="short" className="flex-1 aria-pressed:text-critical">
          Short
        </ToggleGroupItem>
      </ToggleGroup>
      <div className="grid grid-cols-2 gap-2">
        <label className="flex flex-col gap-1 text-[11px] text-muted-foreground">
          Risk per trade
          <Input
            className={inputClass}
            inputMode="decimal"
            value={risk}
            onChange={(e) => {
              const v = e.target.value.replace(/[^\d.]/g, "");
              setRisk(v);
              const n = Number(v);
              // A new risk budget re-sizes the position, so it drops any share override.
              if (Number.isFinite(n) && n > 0) onChange({ ...drawing, risk: n, qty: null });
            }}
          />
        </label>
        <label className="flex flex-col gap-1 text-[11px] text-muted-foreground">
          Shares
          <Input
            className={inputClass}
            inputMode="numeric"
            value={qty}
            onChange={(e) => {
              const v = e.target.value.replace(/[^\d]/g, "");
              setQty(v);
              const n = Number(v);
              if (Number.isInteger(n) && n > 0) onChange({ ...drawing, qty: n });
            }}
          />
        </label>
      </div>
      <div className="grid grid-cols-[auto_minmax(0,1fr)_auto] items-center gap-x-2.5 gap-y-1.5 text-xs">
        {legs.map((l) => (
          <div key={l.label} className="contents">
            <span aria-hidden="true" className={cn("size-2 rounded-[2px]", l.swatch)} />
            <span className="text-muted-foreground">{l.label}</span>
            <span className="font-mono tabular-nums">{formatPrice(l.price)}</span>
          </div>
        ))}
      </div>
      <div className="grid grid-cols-3 gap-1.5">
        <Stat label="Reward" value={`+${short(plan.reward)}`} tone="text-good" />
        <Stat label="Risk" value={`−${short(plan.risk)}`} tone="text-critical" />
        <Stat label="R:R" value={plan.rr.toFixed(2)} />
      </div>
      <div className="flex justify-between text-[11px] text-muted-foreground">
        <span>{formatSize(plan.qty)} shares · value</span>
        <span className="font-mono text-foreground tabular-nums">{money(plan.value)}</span>
      </div>
      {onPlace && (
        <Button className="h-8 w-full text-xs" disabled={!plan.valid} onClick={onPlace}>
          Place bracket
        </Button>
      )}
      <p className="m-0 text-[10px] leading-snug text-text-faint">
        {onPlace
          ? `${long ? "Buys" : "Sells"} ${formatSize(plan.qty)} at ${formatPrice(plan.entry)} or better. Once filled, the target and stop work as one order: whichever fills first cancels the other.`
          : "Planning only: trading is off on this chart."}
      </p>
    </section>
  );
}

function Stat({ label, value, tone }: { label: string; value: string; tone?: string }) {
  return (
    <div className="flex flex-col gap-0.5 rounded-lg bg-bg-row p-2">
      <span className="text-[10px] text-muted-foreground">{label}</span>
      <span className={cn("font-mono text-[13px] font-medium tabular-nums", tone)}>{value}</span>
    </div>
  );
}

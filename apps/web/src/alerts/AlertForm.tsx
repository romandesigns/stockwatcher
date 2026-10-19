import {
  ALERT_SESSIONS,
  EXPIRY_LABELS,
  GAP_CHOICES,
  MOVE_WINDOW_LABELS,
  MOVE_WINDOWS,
  SESSION_LABELS,
  editedAlert,
  expiresAtFor,
  parseSymbol,
  type AlertCondition,
  type AlertExpiry,
  type AlertRule,
  type AlertSession,
  type MoveWindow,
} from "@stockwatcher/app-state";
import { formatPrice, snapPrice } from "@stockwatcher/chart-core";
import { Icon } from "@stockwatcher/chart-web";
import { Dialog, DialogContent, DialogTitle, cn } from "@stockwatcher/chart-web/ui";
import { useState, useSyncExternalStore, type ReactNode } from "react";
import { getAppStore } from "../state/store.ts";
import { alertForm, createAlert, type AlertFormState } from "./actions.ts";

type Tab = "price" | "move" | "signal";
type Signal = "halt" | "luld" | "hod" | "pressure";

const tabOf = (c: AlertCondition): Tab => (c.kind === "price" ? "price" : c.kind === "move" ? "move" : "signal");

const fieldClass = "h-[34px] w-full rounded-lg bg-background px-2.5 text-[13px] text-foreground outline-none focus-visible:ring-2 focus-visible:ring-ring";
const labelClass = "flex min-w-0 flex-col gap-1.5 text-[11.5px] text-muted-foreground";

/** A segmented control: one choice, the active segment raised (no accent: the design's segmented rule). */
function Segmented<T extends string>({ label, value, options, onChange }: { label: string; value: T; options: readonly { v: T; label: string }[]; onChange: (v: T) => void }) {
  return (
    <div role="radiogroup" aria-label={label} className="inline-flex gap-0.5 self-start rounded-lg bg-background p-0.5">
      {options.map((o) => (
        <button
          key={o.v}
          type="button"
          role="radio"
          aria-checked={value === o.v}
          onClick={() => onChange(o.v)}
          className={cn("h-7 rounded-md px-3 text-xs font-medium outline-none transition-colors focus-visible:ring-2 focus-visible:ring-ring", value === o.v ? "bg-bg-raised text-foreground" : "text-muted-foreground hover:text-foreground")}
        >
          {o.label}
        </button>
      ))}
    </div>
  );
}

/** A toggle chip (sessions, notify): pressed = raised, never accent. */
function Chip({ pressed, onClick, children }: { pressed: boolean; onClick: () => void; children: ReactNode }) {
  return (
    <button
      type="button"
      aria-pressed={pressed}
      onClick={onClick}
      className={cn("h-[26px] rounded-full px-2.5 text-[11.5px] font-medium outline-none transition-colors focus-visible:ring-2 focus-visible:ring-ring", pressed ? "bg-bg-raised text-foreground" : "bg-bg-row text-muted-foreground hover:text-foreground")}
    >
      {children}
    </button>
  );
}

function Fieldset({ legend, children }: { legend: string; children: ReactNode }) {
  return (
    <fieldset className="m-0 flex min-w-0 flex-col gap-1.5 border-0 p-0">
      <legend className="mb-1.5 p-0 text-[11.5px] text-muted-foreground">{legend}</legend>
      {children}
    </fieldset>
  );
}

/**
 * The full alert form (approved design, "Create a price alert"): Price / % move / Signal, when it
 * fires, the sessions, the expiry and where it notifies. One form for the chart's "More…", the
 * Alerts page's New alert and Edit. Rendered once by the Shell; opened through actions.ts.
 */
export function AlertForm() {
  const state = useSyncExternalStore(alertForm.subscribe, alertForm.get);
  const close = () => alertForm.set({ open: false });
  return (
    <Dialog open={state.open} onOpenChange={(o) => !o && close()}>
      <DialogContent className="top-1/2 left-1/2 flex max-h-[calc(100dvh-2rem)] w-[min(400px,calc(100vw-1.5rem))] -translate-x-1/2 -translate-y-1/2 flex-col gap-3.5 overflow-y-auto bg-bg-row p-4">
        {/* Keyed by the opening: each one starts from its own draft (state initialised from it). */}
        {state.draft && <FormBody key={state.seq} state={state} d={state.draft} close={close} />}
      </DialogContent>
    </Dialog>
  );
}

function FormBody({ state, d, close }: { state: AlertFormState; d: AlertRule; close: () => void }) {
  const r = d.rule;
  const [symbol, setSymbol] = useState(d.symbol);
  const [tab, setTab] = useState<Tab>(tabOf(r));
  const [dir, setDir] = useState<"above" | "below">(r.kind === "price" ? r.dir : "above");
  // A new alert from the Alerts page has no price yet (its draft's is a placeholder).
  const [price, setPrice] = useState(r.kind === "price" && d.symbol ? formatPrice(r.price) : "");
  const [moveDir, setMoveDir] = useState<"up" | "down" | "either">(r.kind === "move" ? r.dir : "up");
  const [pct, setPct] = useState(r.kind === "move" ? String(r.pct) : "5");
  const [win, setWin] = useState<MoveWindow>(r.kind === "move" ? r.window : "5m");
  const [signal, setSignal] = useState<Signal>(r.kind === "halt" || r.kind === "hod" || r.kind === "luld" || r.kind === "pressure" ? r.kind : "halt");
  const [side, setSide] = useState<"up" | "down" | "either">(r.kind === "luld" ? r.side : "up");
  const [bandPct, setBandPct] = useState(r.kind === "luld" ? String(r.pct) : "2");
  const [pSide, setPSide] = useState<"buy" | "sell" | "either">(r.kind === "pressure" ? r.side : "buy");
  const [pMin, setPMin] = useState(r.kind === "pressure" ? String(r.min) : "80");
  const [fire, setFire] = useState<"once" | "every">(d.fire);
  const [gap, setGap] = useState(d.gapMin);
  const [sessions, setSessions] = useState<AlertSession[]>(d.sessions);
  const [expiry, setExpiry] = useState<AlertExpiry>(d.expiry);
  const [notify, setNotify] = useState(d.notify);

  const sym = parseSymbol(symbol);
  const condition = ((): AlertCondition | null => {
    if (tab === "price") {
      const p = Number(price);
      return Number.isFinite(p) && p > 0 ? { kind: "price", dir, price: snapPrice(p) } : null;
    }
    if (tab === "move") {
      const n = Number(pct);
      return Number.isFinite(n) && n > 0 ? { kind: "move", dir: moveDir, pct: n, window: win } : null;
    }
    if (signal === "pressure") {
      const n = Number(pMin);
      return Number.isInteger(n) && n >= 1 && n <= 100 ? { kind: "pressure", side: pSide, min: n } : null;
    }
    if (signal === "luld") {
      const n = Number(bandPct);
      return Number.isFinite(n) && n > 0 ? { kind: "luld", side, pct: n } : null;
    }
    return { kind: signal };
  })();
  const valid = !!sym && !!condition && sessions.length > 0;
  const priceNum = Number(price);
  const hint = tab === "price" && state.last && Number.isFinite(priceNum) && priceNum > 0 ? `${priceNum >= state.last ? "+" : "−"}${Math.abs(((priceNum - state.last) / state.last) * 100).toFixed(1)}% from ${formatPrice(state.last)}` : null;

  const submit = () => {
    if (!valid || !d || !condition || !sym) return;
    const now = Date.now();
    const patch = { symbol: sym, rule: condition, fire, gapMin: gap, sessions, expiry, expiresAt: expiresAtFor(expiry, now), notify };
    const app = getAppStore().getState();
    if (state.editId) app.saveAlert(editedAlert(d, patch, now));
    else createAlert({ ...d, ...patch, createdAt: now, editedAt: now });
    close();
  };

  return (
    <form
      className="flex flex-col gap-3.5"
      onSubmit={(e) => {
        e.preventDefault();
        submit();
      }}
    >
      <div className="flex items-center gap-2">
        <DialogTitle className="not-sr-only m-0 text-sm font-semibold">
          {state.editId ? "Edit alert" : "New alert"}
          {sym && !state.editId ? <> · <span className="font-mono">{sym}</span></> : null}
        </DialogTitle>
        <button type="button" aria-label="Close" onClick={close} className="ml-auto inline-grid size-7 place-items-center rounded-md text-muted-foreground outline-none hover:bg-bg-raised hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring [&_svg]:size-3.5">
          <Icon name="clear" />
        </button>
      </div>
      {(!d?.symbol || state.editId) && (
        <label className={labelClass}>
          Ticker
          <input value={symbol} onChange={(e) => setSymbol(e.target.value.toUpperCase())} placeholder="XLAB" autoComplete="off" spellCheck={false} className={cn(fieldClass, "font-mono uppercase")} />
        </label>
      )}
      <Segmented label="Alert type" value={tab} onChange={setTab} options={[{ v: "price", label: "Price" }, { v: "move", label: "% move" }, { v: "signal", label: "Signal" }]} />
      {tab === "price" && (
        <>
          <div className="grid grid-cols-2 gap-2.5">
            <label className={labelClass}>
              When the price
              <select value={dir} onChange={(e) => setDir(e.target.value as "above" | "below")} className={fieldClass}>
                <option value="above">Crosses above</option>
                <option value="below">Crosses below</option>
              </select>
            </label>
            <label className={labelClass}>
              Price
              <input value={price} onChange={(e) => setPrice(e.target.value)} inputMode="decimal" className={cn(fieldClass, "font-mono")} />
            </label>
          </div>
          {hint && <span className="-mt-1 text-[11.5px] text-text-faint">{hint} · drag the line on the chart to change it</span>}
        </>
      )}
      {tab === "move" && (
        <>
          <div className="grid grid-cols-2 gap-2.5">
            <label className={labelClass}>
              Moves
              <select value={moveDir} onChange={(e) => setMoveDir(e.target.value as "up" | "down" | "either")} className={fieldClass}>
                <option value="up">Up</option>
                <option value="down">Down</option>
                <option value="either">Either way</option>
              </select>
            </label>
            <label className={labelClass}>
              By (%)
              <input value={pct} onChange={(e) => setPct(e.target.value)} inputMode="decimal" className={cn(fieldClass, "font-mono")} />
            </label>
          </div>
          <label className={labelClass}>
            Within
            <select value={win} onChange={(e) => setWin(e.target.value as MoveWindow)} className={fieldClass}>
              {MOVE_WINDOWS.map((w) => (
                <option key={w} value={w}>
                  {MOVE_WINDOW_LABELS[w]}
                </option>
              ))}
            </select>
          </label>
        </>
      )}
      {tab === "signal" && (
        <>
          <label className={labelClass}>
            Signal
            <select value={signal} onChange={(e) => setSignal(e.target.value as Signal)} className={fieldClass}>
              <option value="halt">Halted or resumed</option>
              <option value="luld">Near the limit-up/down band (modelled)</option>
              <option value="hod">New high of day</option>
              <option value="pressure">Halt watcher pressure</option>
            </select>
          </label>
          {signal === "luld" && (
            <>
              <div className="grid grid-cols-2 gap-2.5">
                <label className={labelClass}>
                  Band
                  <select value={side} onChange={(e) => setSide(e.target.value as "up" | "down" | "either")} className={fieldClass}>
                    <option value="up">Limit up</option>
                    <option value="down">Limit down</option>
                    <option value="either">Either</option>
                  </select>
                </label>
                <label className={labelClass}>
                  Within (%)
                  <input value={bandPct} onChange={(e) => setBandPct(e.target.value)} inputMode="decimal" className={cn(fieldClass, "font-mono")} />
                </label>
              </div>
              <span className="-mt-1 text-[11.5px] text-text-faint">The band is our model of the LULD plan, not the exchange's numbers.</span>
            </>
          )}
          {signal === "pressure" && (
            <>
              <div className="grid grid-cols-2 gap-2.5">
                <label className={labelClass}>
                  Side
                  <select value={pSide} onChange={(e) => setPSide(e.target.value as "buy" | "sell" | "either")} className={fieldClass}>
                    <option value="buy">Buy pressure</option>
                    <option value="sell">Sell pressure</option>
                    <option value="either">Either</option>
                  </select>
                </label>
                <label className={labelClass}>
                  Reaches (0–100)
                  <input value={pMin} onChange={(e) => setPMin(e.target.value)} inputMode="numeric" className={cn(fieldClass, "font-mono")} />
                </label>
              </div>
              <span className="-mt-1 text-[11.5px] text-text-faint">The Halt watcher's score: how one-sided and how heavy the last minute was.</span>
            </>
          )}
        </>
      )}
      <Fieldset legend="Fires">
        <div className="flex flex-wrap items-center gap-2">
          <Segmented label="Fires" value={fire} onChange={setFire} options={[{ v: "once", label: "Once" }, { v: "every", label: "Every time" }]} />
          {fire === "every" && (
            <label className="flex items-center gap-1.5 text-[11.5px] text-muted-foreground">
              at least
              <select value={gap} onChange={(e) => setGap(Number(e.target.value))} aria-label="Minimum gap" className={cn(fieldClass, "h-7 w-auto")}>
                {GAP_CHOICES.map((g) => (
                  <option key={g} value={g}>
                    {g} min
                  </option>
                ))}
              </select>
              apart
            </label>
          )}
        </div>
      </Fieldset>
      <Fieldset legend="Sessions">
        <div className="flex flex-wrap gap-1.5">
          {ALERT_SESSIONS.map((s) => (
            <Chip key={s} pressed={sessions.includes(s)} onClick={() => setSessions((cur) => (cur.includes(s) ? cur.filter((x) => x !== s) : ALERT_SESSIONS.filter((x) => x === s || cur.includes(x))))}>
              {SESSION_LABELS[s]}
            </Chip>
          ))}
        </div>
      </Fieldset>
      <div className="grid grid-cols-2 gap-2.5">
        <label className={labelClass}>
          Expires
          <select value={expiry} onChange={(e) => setExpiry(e.target.value as AlertExpiry)} className={fieldClass}>
            {(["week", "day", "never"] as const).map((x) => (
              <option key={x} value={x}>
                {EXPIRY_LABELS[x]}
              </option>
            ))}
          </select>
        </label>
        <Fieldset legend="Notify">
          <div className="flex gap-1.5">
            <Chip pressed={notify.app} onClick={() => setNotify((n) => ({ ...n, app: !n.app }))}>
              In app
            </Chip>
            <Chip pressed={notify.phone} onClick={() => setNotify((n) => ({ ...n, phone: !n.phone }))}>
              Phone
            </Chip>
          </div>
        </Fieldset>
      </div>
      <div className="flex gap-2 pt-0.5">
        <button type="submit" disabled={!valid} className="h-9 flex-1 rounded-lg bg-primary text-[13px] font-semibold text-primary-foreground outline-none hover:bg-primary/90 focus-visible:ring-2 focus-visible:ring-ring disabled:opacity-50">
          {state.editId ? "Save alert" : "Create alert"}
        </button>
        <button type="button" onClick={close} className="h-9 rounded-lg bg-bg-raised px-3.5 text-[13px] font-medium outline-none hover:bg-background focus-visible:ring-2 focus-visible:ring-ring">
          Cancel
        </button>
      </div>
    </form>
  );
}

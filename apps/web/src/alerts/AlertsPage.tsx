import {
  alertDisplay,
  conditionText,
  etDayKey,
  isActiveAlert,
  metaText,
  newAlert,
  PHONE_BUDGETS,
  type AlertPrefs,
  type AlertRule,
} from "@stockwatcher/app-state";
import { etToUtc, etParts, formatEtDate } from "@stockwatcher/chart-core";
import { Icon } from "@stockwatcher/chart-web";
import { Popover, PopoverContent, PopoverTrigger, ScrollArea, Slider, Switch, cn } from "@stockwatcher/chart-web/ui";
import type { DrawingTone } from "@stockwatcher/chart-core";
import { useEffect, useMemo, useState } from "react";
import { ConnectionGate } from "../ConnectionGate.tsx";
import { TimeSelect } from "../lib/TimeSelect.tsx";
import { NavToggle } from "../nav/NavToggle.tsx";
import { openTickerChart } from "../nav/openTicker.ts";
import { useHasRail } from "../nav/navRail.ts";
import { SettingsButton } from "../nav/settings.tsx";
import { getAppStore, useApp } from "../state/store.ts";
import { deleteAlert, openAlertForm } from "./actions.ts";
import { ActivityRow, AlertRow, type AlertRowAction } from "./rows.tsx";
import { markAlertsSeen } from "./seen.ts";
import { playAlertTone, unlockAlertSound } from "./sound.ts";

/**
 * The Alerts page (approved design, "Alerts manager"): every alert with its state, and Activity,
 * every fire and every hold-back with its reason. It needs no market data, but it connects like
 * every page (the live market's socket), which is what brings Activity and the phone count.
 */
export function AlertsPage() {
  return <ConnectionGate source="live">{() => <Alerts />}</ConnectionGate>;
}

const DAY_MS = 86_400_000;

/** Tomorrow 04:00 ET (the pre-market open): "Snooze until tomorrow". */
function tomorrowMorning(now: number): number {
  const p = etParts(now + DAY_MS);
  return etToUtc(p.year, p.month, p.day, 4);
}

function Alerts() {
  const hasRail = useHasRail();
  const rules = useApp((s) => s.alerts);
  const states = useApp((s) => s.alertStates);
  const activity = useApp((s) => s.alertActivity);
  const status = useApp((s) => s.alertStatus);
  const prefs = useApp((s) => s.alertPrefs);
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const t = setInterval(() => setNow(Date.now()), 30_000);
    return () => clearInterval(t);
  }, []);
  // Opening the page reads every fire so far (the rail badge counts the ones after).
  useEffect(() => markAlertsSeen(), [activity]);

  const app = getAppStore().getState();
  const active = rules.filter((r) => isActiveAlert(r, states[r.id], now)).length;
  // Watching first (newest first), then fired and expired ones (newest first).
  const ordered = useMemo(() => {
    const live = rules.filter((r) => isActiveAlert(r, states[r.id], now));
    const done = rules.filter((r) => !isActiveAlert(r, states[r.id], now));
    return [...live.reverse(), ...done.reverse()];
  }, [rules, states, now]);
  const days = useMemo(() => {
    const out: { key: string; label: string; events: typeof activity }[] = [];
    const today = etDayKey(now);
    for (const e of activity) {
      const key = etDayKey(e.t);
      let g = out.at(-1);
      if (!g || g.key !== key) {
        g = { key, label: key === today ? "Activity · today" : `Activity · ${formatEtDate(e.t)}`, events: [] };
        out.push(g);
      }
      g.events.push(e);
    }
    return out;
  }, [activity, now]);

  const actionsFor = (r: AlertRule): (AlertRowAction | "separator")[] => {
    const d = alertDisplay(r, states[r.id], now);
    const out: (AlertRowAction | "separator")[] = [{ label: "Open chart", onSelect: () => openTickerChart(r.symbol) }, { label: "Edit…", onSelect: () => openAlertForm(r, { editId: r.id }) }];
    if (app.drawingFires[r.id]) out.push({ label: "Acknowledge fire", onSelect: () => app.acknowledgeDrawingFire(r.id) });
    if (d.kind === "fired" || d.kind === "expired") out.push({ label: "Re-arm", onSelect: () => app.updateAlert(r.id, { expiry: r.expiry }) });
    if (d.kind === "snoozed") out.push({ label: "Unsnooze", onSelect: () => app.snoozeAlert(r.id, null) });
    else if (d.kind !== "fired" && d.kind !== "expired") {
      out.push({ label: "Snooze until tomorrow", onSelect: () => app.snoozeAlert(r.id, tomorrowMorning(Date.now())) });
      out.push({ label: "Snooze for a week", onSelect: () => app.snoozeAlert(r.id, Date.now() + 7 * DAY_MS) });
    }
    out.push("separator", { label: "Delete", destructive: true, onSelect: () => deleteAlert(r.id) });
    return out;
  };

  const newOne = () => openAlertForm(newAlert("", { kind: "price", dir: "above", price: 1 }, prefs));
  const phoneToday = status?.phoneToday ?? 0;
  const budget = status?.phoneBudget ?? prefs.phoneBudget;

  return (
    <div className="@container/alerts flex h-full min-h-0 flex-col gap-4 p-3 text-foreground @3xl/alerts:px-6 @3xl/alerts:py-4">
      <header className="flex flex-wrap items-center gap-x-3 gap-y-2">
        {hasRail && <NavToggle />}
        <h1 className="m-0 text-base font-semibold tracking-tight">Alerts</h1>
        <span className="text-xs text-text-faint">{active} active · synced on every device</span>
        <div className="ml-auto flex items-center gap-2.5">
          <SoundPrefs prefs={prefs} onPrefs={app.setAlertPrefs} />
          <PhoneBudget today={phoneToday} budget={budget} devices={status?.devices ?? 0} prefs={prefs} onPrefs={app.setAlertPrefs} />
          <button type="button" onClick={newOne} className="inline-flex h-8 items-center gap-1.5 rounded-lg bg-primary pointer-coarse:h-11 px-3 text-[12.5px] font-semibold text-primary-foreground outline-none hover:bg-primary/90 focus-visible:ring-2 focus-visible:ring-ring [&_svg]:size-3.5">
            <Icon name="plus" />
            New alert
          </button>
          {!hasRail && <SettingsButton />}
        </div>
      </header>
      <ScrollArea className="flex-1" viewportClassName="grid grid-cols-1 gap-6 @4xl/alerts:grid-cols-[minmax(0,1.35fr)_minmax(0,1fr)] @4xl/alerts:gap-8">
        <section aria-labelledby="alerts-active" className="flex min-w-0 flex-col gap-2">
          <h2 id="alerts-active" className="m-0 mb-1 text-xs font-medium tracking-wider text-muted-foreground uppercase">
            Alerts
          </h2>
          {ordered.length === 0 ? (
            <p className="m-0 text-[13px] text-text-faint">No alerts yet. Hold the chart for half a second, use the bell beside the + on the price axis, or New alert.</p>
          ) : (
            ordered.map((r) => <AlertRow key={r.id} symbol={r.symbol} rule={conditionText(r.rule)} meta={metaText(r, now)} state={alertDisplay(r, states[r.id], now)} actions={actionsFor(r)} onOpen={() => openAlertForm(r, { editId: r.id })} onOpenChart={() => openTickerChart(r.symbol)} />)
          )}
        </section>
        <section aria-label="Activity" className="flex min-w-0 flex-col gap-1">
          {days.length === 0 && (
            <>
              <h2 className="m-0 mb-2 text-xs font-medium tracking-wider text-muted-foreground uppercase">Activity · today</h2>
              <p className="m-0 text-[13px] text-text-faint">Fired alerts and every hold-back, with the reason, show here for 30 days.</p>
            </>
          )}
          {days.map((g) => (
            <div key={g.key} className="flex flex-col">
              <h2 className="m-0 mt-1 mb-1 text-xs font-medium tracking-wider text-muted-foreground uppercase">{g.label}</h2>
              {g.events.map((e) => (
                <ActivityRow key={e.id} event={e} onOpenChart={openTickerChart} />
              ))}
            </div>
          ))}
          <p className="m-0 mt-3 text-[11px] text-text-faint">Alerts watch the live market only. Delivery to a phone cannot be guaranteed (the phone, its OS and the push service decide); Activity is the record.</p>
        </section>
      </ScrollArea>
    </div>
  );
}

const TEST_TONES: readonly { tone: DrawingTone; label: string }[] = [
  { tone: "up", label: "Up" },
  { tone: "down", label: "Down" },
  { tone: "stop", label: "Stop" },
];

/**
 * Alert sounds (drawing alerts' tones; owner decision 2026-11-09): the global switch, the volume
 * and a test of each tone. Synced with the alert preferences, so muting here mutes every device
 * (a phone's push then arrives on the plain alerts channel). Turning it on or testing is the user
 * gesture that lets the browser play sound.
 */
function SoundPrefs({ prefs, onPrefs }: { prefs: AlertPrefs; onPrefs: (p: AlertPrefs) => void }) {
  const { on, volume } = prefs.sound;
  const test = (tone: DrawingTone) => {
    unlockAlertSound();
    // The context resumes asynchronously on the first gesture: try again a beat later.
    if (!playAlertTone(tone, volume)) setTimeout(() => playAlertTone(tone, volume), 60);
  };
  return (
    <Popover>
      <PopoverTrigger aria-label={on ? "Alert sounds on" : "Alert sounds off"} className="inline-grid size-8 place-items-center rounded-lg text-muted-foreground outline-none pointer-coarse:size-11 hover:bg-bg-row hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring [&_svg]:size-4">
        <Icon name={on ? "sound" : "soundOff"} />
      </PopoverTrigger>
      <PopoverContent side="bottom" align="end" className="w-72">
        <div className="flex flex-col gap-3 text-[12.5px]">
          <label className="flex items-center justify-between gap-3">
            <span>Alert sounds</span>
            <Switch
              checked={on}
              onCheckedChange={(v) => {
                if (v) unlockAlertSound();
                onPrefs({ ...prefs, sound: { ...prefs.sound, on: v } });
              }}
            />
          </label>
          <div className={cn("flex flex-col gap-0.5 transition-opacity", !on && "pointer-events-none opacity-50")}>
            <span className="text-muted-foreground">Volume · {Math.round(volume * 100)}%</span>
            <Slider
              aria-label="Alert volume"
              min={0}
              max={100}
              step={5}
              value={Math.round(volume * 100)}
              onValueChange={(v) => onPrefs({ ...prefs, sound: { ...prefs.sound, volume: (Array.isArray(v) ? (v[0] ?? 0) : v) / 100 } })}
              getAriaValueText={(_, v) => `${v}%`}
              trackStyle={{ background: `linear-gradient(to right, var(--color-muted-foreground) ${volume * 100}%, var(--color-bg-row) ${volume * 100}%)` }}
              thumbStyle={{ background: "var(--color-foreground)" }}
            />
            <div role="group" aria-label="Test a tone" className="flex gap-1.5">
              {TEST_TONES.map((t) => (
                <button key={t.tone} type="button" onClick={() => test(t.tone)} className="h-7 flex-1 rounded-md bg-bg-row text-xs text-muted-foreground outline-none pointer-coarse:h-11 hover:bg-bg-raised hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring">
                  {t.label}
                </button>
              ))}
            </div>
          </div>
          <span className="text-[11px] text-text-faint">Drawing alerts play a tone once when they fire: rising when the price comes up into a drawing, falling when it comes down, a low triple for a stop. Each alert has its own Sound chip; quiet hours silence them.</span>
        </div>
      </PopoverContent>
    </Popover>
  );
}

/** "Phone notifications today 7 / 20" with the budget and quiet hours behind it. */
function PhoneBudget({ today, budget, devices, prefs, onPrefs }: { today: number; budget: number; devices: number; prefs: AlertPrefs; onPrefs: (p: AlertPrefs) => void }) {
  const share = Math.min(1, budget ? today / budget : 0);
  return (
    <Popover>
      <PopoverTrigger className="flex items-center gap-2 rounded-lg px-1.5 py-1 text-xs pointer-coarse:min-h-11 pointer-coarse:px-3 text-muted-foreground outline-none hover:bg-bg-row focus-visible:ring-2 focus-visible:ring-ring">
        <span className="hidden @2xl/alerts:inline">Phone today</span>
        <span role="meter" aria-label="Phone notifications used today" aria-valuemin={0} aria-valuemax={budget} aria-valuenow={today} className="block h-1.5 w-20 overflow-hidden rounded-full bg-bg-row">
          <span className="block h-full rounded-full bg-primary transition-[width] duration-300 motion-reduce:transition-none" style={{ width: `${share * 100}%` }} />
        </span>
        <span className="font-mono text-foreground">{`${today} / ${budget}`}</span>
      </PopoverTrigger>
      <PopoverContent side="bottom" align="end" className="w-72">
        <div className="flex flex-col gap-3 text-[12.5px]">
          <div className="flex flex-col gap-1.5">
            <span className="text-muted-foreground">Phone notifications a day</span>
            <div role="radiogroup" aria-label="Daily phone budget" className="inline-flex gap-0.5 self-start rounded-lg bg-background p-0.5">
              {PHONE_BUDGETS.map((b) => (
                <button key={b} type="button" role="radio" aria-checked={prefs.phoneBudget === b} onClick={() => onPrefs({ ...prefs, phoneBudget: b })} className={cn("h-7 rounded-md px-3 font-mono text-xs outline-none pointer-coarse:h-11 focus-visible:ring-2 focus-visible:ring-ring", prefs.phoneBudget === b ? "bg-bg-raised text-foreground" : "text-muted-foreground hover:text-foreground")}>
                  {b}
                </button>
              ))}
            </div>
            <span className="text-[11px] text-text-faint">Past it, alerts still show in the app and in Activity. At most one per stock every 5 minutes.</span>
          </div>
          <label className="flex items-center justify-between gap-3">
            <span>Quiet hours (in app only)</span>
            <Switch checked={prefs.quietHours.on} onCheckedChange={(on) => onPrefs({ ...prefs, quietHours: { ...prefs.quietHours, on } })} />
          </label>
          {prefs.quietHours.on && (
            <div className="flex items-center gap-2 text-muted-foreground">
              <TimeSelect label="Quiet from (ET)" value={prefs.quietHours.start} onChange={(start) => onPrefs({ ...prefs, quietHours: { ...prefs.quietHours, start } })} />
              to
              <TimeSelect label="Quiet until (ET)" value={prefs.quietHours.end} onChange={(end) => onPrefs({ ...prefs, quietHours: { ...prefs.quietHours, end } })} />
              <span className="text-[11px]">ET</span>
            </div>
          )}
          <span className="text-[11px] text-text-faint">{devices === 0 ? "No phone registered yet: open the app on your phone and allow notifications." : `${devices} phone${devices === 1 ? "" : "s"} registered.`}</span>
        </div>
      </PopoverContent>
    </Popover>
  );
}

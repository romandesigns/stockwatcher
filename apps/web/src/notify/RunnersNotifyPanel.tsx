import type { RunnersNotifyKinds } from "@stockwatcher/shared";
import { Icon } from "@stockwatcher/chart-web";
import { Button, Switch, cn } from "@stockwatcher/chart-web/ui";
import { useId, useState, useSyncExternalStore } from "react";
import type { RunnersNotifier } from "./runnersNotify.ts";
import { RUNNERS_SWITCHES } from "./runnersSwitches.ts";

/**
 * Settings → Runners' panel (runnersSection.tsx): the master switch (on the web, the "Turn on
 * desktop alerts" button that asks for permission, from a click only) and the phone's six kinds.
 */
export function RunnersNotifyPanel({ notifier }: { notifier: RunnersNotifier }) {
  const prefs = useSyncExternalStore(notifier.subscribe, notifier.prefs, notifier.prefs);
  const permission = useSyncExternalStore(notifier.subscribe, notifier.permission, notifier.permission);
  const [busy, setBusy] = useState(false);
  const on = prefs.enabled && permission === "granted";
  const statusId = useId();

  const turnOn = () => {
    // A click, so the browser may show its permission prompt (never asked on load).
    setBusy(true);
    void notifier.enable().finally(() => setBusy(false));
  };
  const setKind = (key: keyof RunnersNotifyKinds, v: boolean) => notifier.setPrefs({ ...prefs, kinds: { ...prefs.kinds, [key]: v } });

  const status =
    permission === "unsupported"
      ? "This browser cannot show notifications. The Runners board still toasts while the dashboard is open."
      : permission === "denied"
        ? "Notifications are blocked for this site in the browser's settings. Allow them there, then turn this on."
        : on
          ? `${notifier.desktop ? "Shown while the app is open, minimized or behind other windows." : "Shown while this tab is open, also in the background; a closed tab gets nothing."} At most one per stock per 10 min (a burst: 2 min). No daily limit. The alerts' quiet hours apply. Click one to open its chart.`
          : "Off on this device: the Runners board toasts new flags and breakouts while the dashboard is open.";

  return (
    <div className="flex flex-col gap-1">
      <div className="flex min-h-11 items-center gap-3 rounded-lg px-2 py-1.5">
        <span className={cn("grid size-8 shrink-0 place-items-center rounded-lg bg-bg-row transition-colors duration-150 [&_svg]:size-4", on ? "text-primary" : "text-muted-foreground")}>
          <Icon name="bell" />
        </span>
        <span className="flex min-w-0 flex-1 flex-col">
          <span className="text-[13px] text-foreground">{notifier.desktop ? "System notifications" : "Desktop alerts"}</span>
          <span id={statusId} className="text-[11px] text-text-faint" aria-live="polite">
            {status}
          </span>
        </span>
        {on || notifier.desktop ? (
          <Switch checked={on} disabled={permission !== "granted"} onCheckedChange={(v) => (v ? turnOn() : notifier.setPrefs({ ...prefs, enabled: false }))} aria-label="Runners notifications on this device" aria-describedby={statusId} />
        ) : (
          <Button size="sm" disabled={busy || permission === "denied" || permission === "unsupported"} onClick={turnOn} aria-describedby={statusId}>
            Turn on desktop alerts
          </Button>
        )}
      </div>
      <div role="group" aria-label="Which Runners events notify" className={cn("flex flex-col gap-0.5 transition-opacity duration-150", !on && "opacity-50")}>
        {RUNNERS_SWITCHES.map((s) => (
          <KindRow key={s.key} item={s} checked={prefs.kinds[s.key]} disabled={!on} onChange={(v) => setKind(s.key, v)} />
        ))}
      </div>
    </div>
  );
}

function KindRow({ item, checked, disabled, onChange }: { item: (typeof RUNNERS_SWITCHES)[number]; checked: boolean; disabled: boolean; onChange: (v: boolean) => void }) {
  const descId = useId();
  return (
    <label className={cn("flex min-h-11 items-center gap-3 rounded-lg px-2 py-1.5 transition-colors", disabled ? "cursor-default" : "cursor-pointer hover:bg-bg-row")}>
      <span className={cn("grid size-8 shrink-0 place-items-center rounded-lg bg-bg-row transition-[opacity,color] duration-150 [&_svg]:size-4", checked ? "text-foreground" : "text-muted-foreground opacity-50")}>
        <Icon name={item.icon} />
      </span>
      <span className="flex min-w-0 flex-1 flex-col">
        <span className={cn("text-[13px] transition-colors", checked ? "text-foreground" : "text-foreground/70")}>{item.name}</span>
        <span id={descId} className="truncate text-[11px] text-text-faint">
          {item.description}
        </span>
      </span>
      <Switch checked={checked} disabled={disabled} onCheckedChange={onChange} aria-label={item.name} aria-describedby={descId} />
    </label>
  );
}

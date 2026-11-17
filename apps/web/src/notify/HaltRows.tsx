import { HALT_SOUND_KEYS, HALT_SOUND_LABELS, type HaltEvent, type HaltSoundKey } from "@stockwatcher/app-state";
import { Icon } from "@stockwatcher/chart-web";
import { Button, OptionSelect, Switch, Tip, cn, type SelectOption } from "@stockwatcher/chart-web/ui";
import type { AlertSwitch } from "@stockwatcher/shared";
import { useEffect, useId, useRef, useState, useSyncExternalStore } from "react";
import { alertSoundUnlocked, onAlertSoundState, playHaltSound, unlockAlertSound } from "../alerts/sound.ts";
import { useApp } from "../state/store.ts";
import type { RunnersNotifier, RunnersNotifyPrefs } from "./runnersNotify.ts";

/** The picker's options: every built-in sound, "None" last (app-state HALT_SOUND_KEYS). */
const SOUND_OPTIONS: readonly SelectOption<HaltSoundKey>[] = HALT_SOUND_KEYS.map((k) => ({ value: k, label: HALT_SOUND_LABELS[k].name }));

/**
 * Settings → Alerts → Halts (plan "halt sounds", 2026-11-16): the exchange's halts and resumptions
 * of a covered stock, one row per event with its switch, this device's sound, a ▶ preview and
 * "Send test" (a real notice from the server, to this connection only). Unlike the rest of the
 * panel they work without system notifications (runnersNotify.ts): the sound and an in-app toast.
 * The sound is per device (localStorage, with the switches); the volume and the global mute are
 * the alert sounds' (synced, the Alerts page's speaker).
 *
 * Layout: each row is its own container; the sound controls sit under the name, indented to its
 * text, and wrap below 20rem rather than squeezing the picker.
 */
export function HaltRows({ items, prefs, notifier }: { items: readonly AlertSwitch[]; prefs: RunnersNotifyPrefs; notifier: RunnersNotifier }) {
  const unlocked = useSyncExternalStore(onAlertSoundState, alertSoundUnlocked, () => false);
  const sound = useApp((s) => s.alertPrefs.sound);
  return (
    <div className="flex flex-col gap-0.5">
      {items.map((item) => {
        const event = item.key as HaltEvent;
        return <HaltRow key={item.key} item={item} event={event} checked={prefs.kinds[event]} sound={prefs.haltSounds[event]} volume={sound.volume} notifier={notifier} onChecked={(v) => notifier.setPrefs({ ...prefs, kinds: { ...prefs.kinds, [event]: v } })} onSound={(k) => notifier.setPrefs({ ...prefs, haltSounds: { ...prefs.haltSounds, [event]: k } })} />;
      })}
      {!unlocked ? (
        <div role="status" className="flex min-h-11 flex-wrap items-center gap-x-3 gap-y-1 rounded-lg px-2 py-1.5">
          <span className="min-w-0 flex-1 text-[11px] text-text-faint">This page cannot play sounds until it is clicked once.</span>
          <Button variant="secondary" size="sm" onClick={() => void unlockAlertSound()}>
            Allow sounds
          </Button>
        </div>
      ) : !sound.on ? (
        <p className="m-0 px-2 py-1.5 text-[11px] text-text-faint">Alert sounds are muted (the speaker on the Alerts page): halts still notify, silently.</p>
      ) : null}
    </div>
  );
}

function HaltRow({ item, event, checked, sound, volume, notifier, onChecked, onSound }: { item: AlertSwitch; event: HaltEvent; checked: boolean; sound: HaltSoundKey; volume: number; notifier: RunnersNotifier; onChecked: (v: boolean) => void; onSound: (k: HaltSoundKey) => void }) {
  const descId = useId();
  const [note, setNote] = useState<string | null>(null);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  useEffect(() => () => void (timer.current && clearTimeout(timer.current)), []);
  const flash = (text: string) => {
    setNote(text);
    if (timer.current) clearTimeout(timer.current);
    timer.current = setTimeout(() => setNote(null), 4000);
  };
  const preview = (key: HaltSoundKey = sound) => {
    // On the page's first gesture the audio context is still resuming: play once it has.
    if (!playHaltSound(key, volume)) void unlockAlertSound().then((ok) => ok && playHaltSound(key, volume));
  };
  const test = () => {
    void unlockAlertSound();
    flash(notifier.sendHaltTest(event) ? "Test sent: it arrives in a moment." : "Not connected to the server.");
  };
  const name = HALT_SOUND_LABELS[sound].name;
  return (
    <div role="group" aria-label={`${item.name} alerts`} className="@container/halt flex flex-col gap-1 rounded-lg px-2 py-1.5">
      <label className="flex min-h-11 cursor-pointer items-center gap-3">
        <span className={cn("grid size-8 shrink-0 place-items-center rounded-lg bg-bg-row transition-[opacity,color] duration-150 [&_svg]:size-4", checked ? "text-foreground" : "text-muted-foreground opacity-50")}>
          <Icon name={item.icon} />
        </span>
        <span className="flex min-w-0 flex-1 flex-col">
          <span className={cn("text-[13px] transition-colors", checked ? "text-foreground" : "text-foreground/70")}>{item.name}</span>
          <span id={descId} className="text-[11px] text-text-faint">
            {item.description}
          </span>
        </span>
        <Switch checked={checked} onCheckedChange={onChecked} aria-label={item.name} aria-describedby={descId} />
      </label>
      <div className={cn("flex flex-wrap items-center gap-1.5 pl-11 transition-opacity duration-150", !checked && "opacity-50")}>
        <OptionSelect
          aria-label={`${item.name} sound`}
          size="sm"
          className="min-w-[8.5rem] flex-1 @xs/halt:max-w-48"
          options={SOUND_OPTIONS}
          value={sound}
          onValueChange={(k) => {
            onSound(k);
            // Picking a sound plays it: the choice is heard, not guessed from its name.
            preview(k);
          }}
          disabled={!checked}
        />
        <Tip label={sound === "none" ? "No sound" : `Play ${name}`}>
          <Button variant="secondary" size="icon" className="size-8 pointer-coarse:size-11 [&_svg]:size-3.5" aria-label={`Play the ${item.name} sound (${name})`} disabled={!checked || sound === "none"} onClick={() => preview()}>
            <Icon name="play" />
          </Button>
        </Tip>
        <Button variant="secondary" size="sm" className="h-8 pointer-coarse:h-11" disabled={!checked} onClick={test}>
          Send test
        </Button>
      </div>
      <span aria-live="polite" className={cn("pl-11 text-[11px] text-text-faint transition-opacity duration-150", note ? "opacity-100" : "h-0 opacity-0")}>
        {note}
      </span>
    </div>
  );
}

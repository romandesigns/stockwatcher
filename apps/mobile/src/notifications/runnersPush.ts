import { useSyncExternalStore } from "react";

import { getPreference, setPreference } from "../storage/preferences";
import { readRunnersPush, registerFieldsOf, RUNNERS_PUSH_DEFAULTS, type RunnersPushPrefs, type RunnersRegisterFields } from "./runnersPushModel";

export * from "./runnersPushModel";

// Push opt-ins on this phone (Settings → Alerts; owner, 2026-10-22, widened 2026-10-23 to Ross,
// Signals and the Halt watcher). Per device, not synced: they say which phone makes noise, like
// the push token itself. The server keeps its copy on the device record
// (crates/server/src/alerts/push.rs `RunnersOptIn`), sent with every `push.register`
// (src/feed/serverFeed.ts) and again whenever a switch changes. The data (defaults, old stored
// values, the register fields) is in runnersPushModel.ts.
//
// Defaults match the server's: new flags ON (the reason this exists: a +95 % move in 30 minutes
// that was only on a closed app's screen), breakouts OFF (several a day per flag at most, but the
// owner chooses), buying bursts ON (2026-10-22: the owner asked to be alerted when massive buying
// hits the tape; ~20 a day), running entries and "Run +20%" entries ON (2026-10-22: "I don't
// wanna lose opportunities because I'm away from my phone"), "Fast move" entries ON too.
// 2026-10-23 ("every panel event backed by evidence must notify"): Ross and Signals crossings,
// volume-shelf entries and Runners follow-ups ON; Ross/Signals "At trigger" and the Halt
// watcher's gauge OFF (opt-in). A value saved before a switch existed takes its default.

const KEY = "runners.push";

let prefs: RunnersPushPrefs = RUNNERS_PUSH_DEFAULTS;
const listeners = new Set<(p: RunnersPushPrefs) => void>();
const emit = () => {
  for (const l of listeners) l(prefs);
};
void getPreference<Partial<RunnersPushPrefs>>(KEY)
  .then((v) => {
    if (!v || typeof v !== "object") return;
    prefs = readRunnersPush(v);
    emit();
  })
  .catch(() => {});

export function runnersPushPrefs(): RunnersPushPrefs {
  return prefs;
}

export function subscribeRunnersPush(fn: (p: RunnersPushPrefs) => void): () => void {
  listeners.add(fn);
  return () => {
    listeners.delete(fn);
  };
}

export function useRunnersPush(): RunnersPushPrefs {
  return useSyncExternalStore(subscribeRunnersPush, runnersPushPrefs);
}

export async function setRunnersPush(change: Partial<RunnersPushPrefs>): Promise<void> {
  prefs = { ...prefs, ...change };
  emit();
  await setPreference(KEY, prefs).catch(() => {});
}

/** What `push.register` carries for the opt-ins now. */
export function runnersRegisterFields(): RunnersRegisterFields {
  return registerFieldsOf(prefs);
}

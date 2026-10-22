import { useSyncExternalStore } from "react";

import { getPreference, setPreference } from "../storage/preferences";

// Runners push opt-ins on this phone (Settings → Runners; owner, 2026-10-22). Per device, not
// synced: they say which phone makes noise, like the push token itself. The server keeps its copy
// on the device record (crates/server/src/alerts/push.rs `RunnersOptIn`), sent with every
// `push.register` (src/feed/serverFeed.ts) and again whenever a switch changes.
//
// Defaults match the server's: new flags ON (the reason this exists: a +95 % move in 30 minutes
// that was only on a closed app's screen), breakouts OFF (several a day per flag at most, but the
// owner chooses), buying bursts ON (2026-10-22: the owner asked to be alerted when massive buying
// hits the tape; ~20 a day), running entries OFF (2026-10-22: 10–40 a day, a move already under
// way rather than a forecast; the owner opts a phone in).

export interface RunnersPushPrefs {
  pushNew: boolean;
  pushBreakout: boolean;
  pushBurst: boolean;
  pushRunning: boolean;
}

const KEY = "runners.push";
export const RUNNERS_PUSH_DEFAULTS: RunnersPushPrefs = { pushNew: true, pushBreakout: false, pushBurst: true, pushRunning: false };

let prefs: RunnersPushPrefs = RUNNERS_PUSH_DEFAULTS;
const listeners = new Set<(p: RunnersPushPrefs) => void>();
const emit = () => {
  for (const l of listeners) l(prefs);
};
void getPreference<Partial<RunnersPushPrefs>>(KEY)
  .then((v) => {
    if (!v || typeof v !== "object") return;
    prefs = {
      pushNew: typeof v.pushNew === "boolean" ? v.pushNew : RUNNERS_PUSH_DEFAULTS.pushNew,
      pushBreakout: typeof v.pushBreakout === "boolean" ? v.pushBreakout : RUNNERS_PUSH_DEFAULTS.pushBreakout,
      // Saved before bursts existed: the default (on), like the server's.
      pushBurst: typeof v.pushBurst === "boolean" ? v.pushBurst : RUNNERS_PUSH_DEFAULTS.pushBurst,
      // Saved before running entries existed: the default (off), like the server's.
      pushRunning: typeof v.pushRunning === "boolean" ? v.pushRunning : RUNNERS_PUSH_DEFAULTS.pushRunning,
    };
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

/** What `push.register` carries for Runners (the server's field names). */
export function runnersRegisterFields(): { runnersNew: boolean; runnersBreakout: boolean; runnersBurst: boolean; runnersRunning: boolean } {
  return { runnersNew: prefs.pushNew, runnersBreakout: prefs.pushBreakout, runnersBurst: prefs.pushBurst, runnersRunning: prefs.pushRunning };
}

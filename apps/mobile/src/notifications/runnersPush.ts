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
// hits the tape; ~20 a day), running entries and "Run +20%" entries ON (2026-10-22: "I don't
// wanna lose opportunities because I'm away from my phone"; a move already under way, ~50 and
// ~25 a day, held to one push per stock per 10 minutes and dropped first past the daily cap).
// "Fast move" entries ON too (2026-10-22: +8 % in 60 s or +12 % in 15 s, held, with real
// trading; the same limits).

export interface RunnersPushPrefs {
  pushNew: boolean;
  pushBreakout: boolean;
  pushBurst: boolean;
  pushRunning: boolean;
  pushRun: boolean;
  pushFast: boolean;
}

const KEY = "runners.push";
export const RUNNERS_PUSH_DEFAULTS: RunnersPushPrefs = { pushNew: true, pushBreakout: false, pushBurst: true, pushRunning: true, pushRun: true, pushFast: true };

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
      // Saved before running entries existed: the default (on), like the server's.
      pushRunning: typeof v.pushRunning === "boolean" ? v.pushRunning : RUNNERS_PUSH_DEFAULTS.pushRunning,
      // Saved before "Run +20%" entries existed: the default (on), like the server's.
      pushRun: typeof v.pushRun === "boolean" ? v.pushRun : RUNNERS_PUSH_DEFAULTS.pushRun,
      // Saved before "Fast move" entries existed: the default (on), like the server's.
      pushFast: typeof v.pushFast === "boolean" ? v.pushFast : RUNNERS_PUSH_DEFAULTS.pushFast,
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
export function runnersRegisterFields(): { runnersNew: boolean; runnersBreakout: boolean; runnersBurst: boolean; runnersRunning: boolean; runnersRun: boolean; runnersFast: boolean } {
  return { runnersNew: prefs.pushNew, runnersBreakout: prefs.pushBreakout, runnersBurst: prefs.pushBurst, runnersRunning: prefs.pushRunning, runnersRun: prefs.pushRun, runnersFast: prefs.pushFast };
}

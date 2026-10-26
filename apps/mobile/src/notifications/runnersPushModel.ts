import type { RunnersNotifyKinds } from "@stockwatcher/shared";

// The phone's push opt-ins as data (pure, so bun tests it without storage or mocks); the store
// and the reasons behind each default are in runnersPush.ts.

export interface RunnersPushPrefs {
  pushNew: boolean;
  pushBreakout: boolean;
  pushBurst: boolean;
  pushRunning: boolean;
  pushRun: boolean;
  pushFast: boolean;
  pushShelf: boolean;
  pushFollowUp: boolean;
  pushRossCrossing: boolean;
  pushSignalsCrossing: boolean;
  pushRossTrigger: boolean;
  pushSignalsTrigger: boolean;
  pushHalt: boolean;
  pushPullback: boolean;
}

export const RUNNERS_PUSH_DEFAULTS: RunnersPushPrefs = {
  pushNew: true,
  pushBreakout: false,
  pushBurst: true,
  pushRunning: true,
  pushRun: true,
  pushFast: true,
  pushShelf: true,
  pushFollowUp: true,
  pushRossCrossing: true,
  pushSignalsCrossing: true,
  pushRossTrigger: false,
  pushSignalsTrigger: false,
  pushHalt: false,
  // "Pullback setting up": on, the owner asked for it (2026-10-25); the server's default too.
  pushPullback: true,
};

/** The phone pref behind each shared switch (packages/shared `ALERT_SWITCH_SECTIONS`). */
export const PUSH_PREF_OF: Readonly<Record<keyof RunnersNotifyKinds, keyof RunnersPushPrefs>> = {
  new: "pushNew",
  breakout: "pushBreakout",
  burst: "pushBurst",
  running: "pushRunning",
  run: "pushRun",
  fast: "pushFast",
  shelf: "pushShelf",
  followUp: "pushFollowUp",
  rossCrossing: "pushRossCrossing",
  signalsCrossing: "pushSignalsCrossing",
  rossTrigger: "pushRossTrigger",
  signalsTrigger: "pushSignalsTrigger",
  halt: "pushHalt",
  pullback: "pushPullback",
};

/**
 * A stored value as a full set of prefs: each switch saved before it existed (or unreadable)
 * takes its default, like the server's device record.
 */
export function readRunnersPush(v: unknown): RunnersPushPrefs {
  const o = v && typeof v === "object" ? (v as Record<string, unknown>) : {};
  const out = { ...RUNNERS_PUSH_DEFAULTS };
  for (const k of Object.keys(out) as (keyof RunnersPushPrefs)[]) if (typeof o[k] === "boolean") out[k] = o[k];
  return out;
}

/** What `push.register` carries for the opt-ins (the server's field names). */
export interface RunnersRegisterFields {
  runnersNew: boolean;
  runnersBreakout: boolean;
  runnersBurst: boolean;
  runnersRunning: boolean;
  runnersRun: boolean;
  runnersFast: boolean;
  runnersShelf: boolean;
  runnersFollowUp: boolean;
  rossCrossing: boolean;
  signalsCrossing: boolean;
  rossTrigger: boolean;
  signalsTrigger: boolean;
  haltWatch: boolean;
  runnersPullback: boolean;
}

export function registerFieldsOf(p: RunnersPushPrefs): RunnersRegisterFields {
  return {
    runnersNew: p.pushNew,
    runnersBreakout: p.pushBreakout,
    runnersBurst: p.pushBurst,
    runnersRunning: p.pushRunning,
    runnersRun: p.pushRun,
    runnersFast: p.pushFast,
    runnersShelf: p.pushShelf,
    runnersFollowUp: p.pushFollowUp,
    rossCrossing: p.pushRossCrossing,
    signalsCrossing: p.pushSignalsCrossing,
    rossTrigger: p.pushRossTrigger,
    signalsTrigger: p.pushSignalsTrigger,
    haltWatch: p.pushHalt,
    runnersPullback: p.pushPullback,
  };
}

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
  /** Exchange halts / resumptions of a covered stock (plan "halt sounds", 2026-11-16). */
  pushHalted: boolean;
  pushResumed: boolean;
  /** The N1 shadow detector (crates/server/src/n1_live, 2026-11-17): research measured beside production. */
  pushN1: boolean;
  /** Engine A T2-c4's F1u and HALT add-on (crates/server/src/c4_live, 2026-11-18). */
  pushF1u: boolean;
  pushHaltBull: boolean;
  /** research v2 b-good-2's model picks (crates/server/src/bgood, 2026-11-22). */
  pushModelPick: boolean;
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
  // Exchange halts: on (the server's default too). Release 1 rings with the phone's default
  // sound; the picked sound needs release 2's APK (its halt channels).
  pushHalted: true,
  pushResumed: true,
  // N1: on since T2-c6 was confirmed (2026-11-17; the server's default too).
  pushN1: true,
  // T2-c4's add-ons: on (owner, 2026-11-18; the server's default too).
  pushF1u: true,
  pushHaltBull: true,
  // research v2's proven wins go live (owner, 2026-11-22; the server's default too).
  pushModelPick: true,
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
  halted: "pushHalted",
  resumed: "pushResumed",
  n1: "pushN1",
  f1u: "pushF1u",
  haltBull: "pushHaltBull",
  modelPick: "pushModelPick",
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
  halted: boolean;
  resumed: boolean;
  n1Shadow: boolean;
  f1u: boolean;
  haltBull: boolean;
  modelPick: boolean;
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
    halted: p.pushHalted,
    resumed: p.pushResumed,
    n1Shadow: p.pushN1,
    f1u: p.pushF1u,
    haltBull: p.pushHaltBull,
    modelPick: p.pushModelPick,
  };
}

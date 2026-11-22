// The phone's push opt-ins (Settings → Alerts): defaults, old stored values and what
// `push.register` sends (crates/server/src/alerts/push.rs `RunnersOptIn` reads these names).
import { expect, test } from "bun:test";
import { ALERT_SWITCH_SECTIONS, DEFAULT_RUNNERS_NOTIFY } from "@stockwatcher/shared";
import { PUSH_PREF_OF, readRunnersPush, registerFieldsOf, RUNNERS_PUSH_DEFAULTS } from "./runnersPushModel";

test("the defaults are the server's and the desk clients': evidence on; breakouts, at-trigger, Halt watcher off", () => {
  for (const [k, pref] of Object.entries(PUSH_PREF_OF)) expect([k, RUNNERS_PUSH_DEFAULTS[pref]]).toEqual([k, DEFAULT_RUNNERS_NOTIFY[k as keyof typeof DEFAULT_RUNNERS_NOTIFY]]);
  expect(registerFieldsOf(RUNNERS_PUSH_DEFAULTS)).toEqual({
    runnersNew: true,
    runnersBreakout: false,
    runnersBurst: true,
    runnersRunning: true,
    runnersRun: true,
    runnersFast: true,
    runnersShelf: true,
    runnersFollowUp: true,
    rossCrossing: true,
    signalsCrossing: true,
    rossTrigger: false,
    signalsTrigger: false,
    haltWatch: false,
    runnersPullback: true,
    halted: true,
    resumed: true,
    n1Shadow: true,
    f1u: true,
    haltBull: true,
    resumedUp: true,
    xFloat: true,
    xGap: true,
    xCtx: true,
    modelPick: true,
    vwapReclaim: true,
    levelBreak: true,
  });
});

test("a value saved before the new switches existed keeps its own and takes the defaults for the rest", () => {
  const old = { pushNew: false, pushBreakout: true, pushBurst: false, pushRunning: true, pushRun: true, pushFast: true };
  expect(readRunnersPush(old)).toEqual({ ...RUNNERS_PUSH_DEFAULTS, pushNew: false, pushBreakout: true, pushBurst: false });
  expect(readRunnersPush({ pushHalt: "yes" })).toEqual(RUNNERS_PUSH_DEFAULTS);
  expect(readRunnersPush(null)).toEqual(RUNNERS_PUSH_DEFAULTS);
});

test("every shared switch has a phone pref, and a switched pref is what push.register sends", () => {
  for (const s of ALERT_SWITCH_SECTIONS) for (const i of s.items) expect(PUSH_PREF_OF[i.key]).toBeDefined();
  expect(registerFieldsOf({ ...RUNNERS_PUSH_DEFAULTS, pushHalt: true, pushRossCrossing: false })).toMatchObject({ haltWatch: true, rossCrossing: false, signalsCrossing: true });
});

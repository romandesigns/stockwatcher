import { expect, test } from "bun:test";
import { createTapHold } from "./tapHold.ts";

/** A recognizer on a fake clock: `at(ms)` runs every timer due by then. */
function rig() {
  let now = 0;
  let timers: { at: number; fn: () => void; id: number }[] = [];
  let next = 1;
  const log: string[] = [];
  const th = createTapHold(
    { onTap: () => log.push("tap"), onHold: () => log.push("hold") },
    {
      doubleTapMs: 300,
      holdMs: 450,
      setTimer: (fn, ms) => {
        const id = next++;
        timers.push({ at: now + ms, fn, id });
        return id;
      },
      clearTimer: (h) => (timers = timers.filter((t) => t.id !== h)),
    },
  );
  const at = (ms: number) => {
    now = ms;
    for (const t of [...timers].sort((a, b) => a.at - b.at)) {
      if (t.at <= now && timers.includes(t)) {
        timers = timers.filter((x) => x !== t);
        t.fn();
      }
    }
  };
  return { th, log, at };
}

test("a single tap fires after the double-tap window", () => {
  const { th, log, at } = rig();
  th.down();
  at(80);
  th.up();
  at(200);
  expect(log).toEqual([]);
  at(380);
  expect(log).toEqual(["tap"]);
});

test("tap, then press and hold: copies once, and no tap", () => {
  const { th, log, at } = rig();
  th.down();
  at(60);
  th.up();
  at(200);
  th.down();
  at(500);
  expect(log).toEqual([]);
  at(650);
  expect(log).toEqual(["hold"]);
  expect(th.firedHold()).toBe(true);
  at(900);
  th.up();
  at(2000);
  expect(log).toEqual(["hold"]);
});

test("a plain double tap is one tap", () => {
  const { th, log, at } = rig();
  th.down();
  at(50);
  th.up();
  at(150);
  th.down();
  at(220);
  th.up();
  expect(log).toEqual(["tap"]);
  at(2000);
  expect(log).toEqual(["tap"]);
});

test("only the second press holds: a long first press is still a tap", () => {
  const { th, log, at } = rig();
  th.down();
  at(1000);
  th.up();
  at(1400);
  expect(log).toEqual(["tap"]);
});

test("a second press too late starts over; a cancelled press fires nothing", () => {
  const { th, log, at } = rig();
  th.down();
  at(50);
  th.up();
  at(400); // window over: the first tap fired
  th.down();
  at(1000); // held, but as a FIRST press
  th.up();
  at(1400);
  expect(log).toEqual(["tap", "tap"]);
  th.down();
  th.cancel();
  at(5000);
  expect(log).toEqual(["tap", "tap"]);
});

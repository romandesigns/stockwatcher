import { expect, test } from "bun:test";
import { createTapHold, type TapHoldOptions } from "@stockwatcher/shared";

function clock() {
  let now = 0;
  let next = 0;
  const timers = new Map<number, { at: number; run: () => void }>();
  const options: TapHoldOptions = {
    setTimer(run, delay) { const id = ++next; timers.set(id, { at: now + delay, run }); return id; },
    clearTimer(id) { timers.delete(id as number); },
  };
  return { options, advance(to: number) {
    now = to;
    for (const [id, timer] of [...timers].sort((a, b) => a[1].at - b[1].at)) {
      if (timer.at <= now && timers.delete(id)) timer.run();
    }
  } };
}

// Detection rows are keyed by transport epoch. React removes the old row on a seek; the
// ticker hook's cleanup calls cancel(). Verify that cleanup while a real timer is pending,
// rather than merely cancelling a press before a timer exists.
test("a removed epoch's pending tap cannot navigate after the replacement row mounts", () => {
  const c = clock();
  const picked: string[] = [];
  const old = createTapHold({ onTap: () => picked.push("epoch-A"), onHold() {} }, c.options);
  old.down(); old.up();
  c.advance(200);
  old.cancel();
  const current = createTapHold({ onTap: () => picked.push("epoch-B"), onHold() {} }, c.options);
  c.advance(1000);
  expect(picked).toEqual([]);
  current.down(); current.up();
  c.advance(1300);
  expect(picked).toEqual(["epoch-B"]);
});

test("a removed detail/row cannot finish a pending second-press copy", () => {
  const c = clock();
  const copied: string[] = [];
  const old = createTapHold({ onTap() {}, onHold: () => copied.push("old") }, c.options);
  old.down(); old.up();
  c.advance(100); old.down();
  c.advance(200); old.cancel();
  const current = createTapHold({ onTap() {}, onHold: () => copied.push("current") }, c.options);
  c.advance(1000);
  expect(copied).toEqual([]);
  current.down(); current.up();
  c.advance(1100); current.down();
  c.advance(1550);
  expect(copied).toEqual(["current"]);
});

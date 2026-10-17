import { expect, test } from "bun:test";
import { emptyHistory, HISTORY_LIMIT, record, redo, undo } from "./history.ts";

test("undo walks back, redo forward, and a new change clears redo", () => {
  let h = emptyHistory<string>();
  let cur = "a";
  for (const next of ["b", "c"]) {
    h = record(h, cur);
    cur = next;
  }
  const u1 = undo(h, cur)!;
  expect(u1.state).toBe("b");
  const u2 = undo(u1.history, u1.state)!;
  expect(u2.state).toBe("a");
  expect(undo(u2.history, u2.state)).toBeNull();
  const r = redo(u2.history, u2.state)!;
  expect(r.state).toBe("b");
  const branched = record(r.history, r.state);
  expect(redo(branched, "x")).toBeNull();
});

test("history is capped", () => {
  let h = emptyHistory<number>();
  for (let i = 0; i < HISTORY_LIMIT + 20; i++) h = record(h, i);
  expect(h.past.length).toBe(HISTORY_LIMIT);
  expect(h.past[0]).toBe(20);
});

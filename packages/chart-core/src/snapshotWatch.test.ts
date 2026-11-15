import { expect, test } from "bun:test";
import { createSnapshotWatch } from "./snapshotWatch.ts";

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

test("a slot that hears nothing is reported overdue once", async () => {
  const overdue: string[] = [];
  const w = createSnapshotWatch((id) => overdue.push(id), 30);
  w.expect("d0");
  await sleep(80);
  expect(overdue).toEqual(["d0"]);
});

test("a snapshot in time, or the chart closing, cancels it", async () => {
  const overdue: string[] = [];
  const w = createSnapshotWatch((id) => overdue.push(id), 30);
  w.expect("d0");
  w.expect("d1");
  w.seen("d0");
  w.forget("d1");
  await sleep(80);
  expect(overdue).toEqual([]);
});

test("expecting again restarts the wait instead of stacking a second report", async () => {
  const overdue: string[] = [];
  const w = createSnapshotWatch((id) => overdue.push(id), 40);
  w.expect("");
  await sleep(25);
  w.expect("");
  await sleep(25);
  expect(overdue).toEqual([]);
  await sleep(40);
  expect(overdue).toEqual([""]);
  w.clear();
});

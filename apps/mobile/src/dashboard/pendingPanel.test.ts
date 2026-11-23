// The phone's News pending card wiring (a bun test cannot render native components; the model is
// packages/shared/src/pending.test.ts).
import { expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { join } from "node:path";

const read = (p: string) => readFileSync(join(import.meta.dir, p), "utf8");

test("the card collapses at 0 cards and sits first in the dashboard scroll", () => {
  expect(read("PendingPanel.tsx")).toContain("<Collapse open={cards.length > 0} reducedMotion={reducedMotion} gap={gap}>");
  const screen = read("../screens/DashboardScreen.tsx");
  expect(screen).toContain("<PendingPanel channel={feed.pending}");
  expect(screen.indexOf("<PendingPanel")).toBeLessThan(screen.indexOf("<RossPanel"));
});

test("the accent is spent only on the reopen countdown", () => {
  const src = read("PendingPanel.tsx");
  expect(src.match(/UI\.accent/g)?.length).toBe(1);
  expect(src).toContain("counting ? UI.accent");
});

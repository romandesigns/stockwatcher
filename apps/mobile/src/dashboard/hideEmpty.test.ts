// The hide-when-empty rule on the phone (owner, 2026-10-22: "If a panel has 0 items then we hide
// the panel", and empty sections inside a panel too). What counts as an item is the shared
// model's (packages/shared/src/panels.test.ts); these pin the phone-side wiring, which a bun test
// cannot render without a native renderer.
import { expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { join } from "node:path";

const read = (p: string) => readFileSync(join(import.meta.dir, p), "utf8");

test("every dashboard card and the Halt watcher row collapse at 0 items", () => {
  expect(read("RossPanel.tsx")).toContain("<Collapse open={rossItemCount(state) > 0} reducedMotion={reducedMotion} gap={gap}>");
  expect(read("RocketPanel.tsx")).toContain("<Collapse open={counts.bursts + counts.flags > 0} reducedMotion={reducedMotion} gap={gap}>");
  expect(read("DetectionPanel.tsx")).toContain("<Collapse open={counts.active + counts.recent > 0} reducedMotion={reducedMotion} gap={gap}>");
  expect(read("HaltWatcher.tsx")).toContain("<Collapse open={board.length > 0} reducedMotion={reducedMotion} gap={gap}>");
  // Top Gainers and Hot stocks: the shared count (Bull and Bear together for Hot stocks).
  expect(read("MoversPanel.tsx")).toContain("<Collapse open={moversCount(state, kind) > 0} reducedMotion={reducedMotion} gap={gap}>");
});

test("Hot stocks: Bull and Bear each collapse at 0", () => {
  const src = read("MoversPanel.tsx");
  expect(src).toContain("<Collapse open={rows.length > 0} reducedMotion={reducedMotion} gap={CARD_GAP}>");
  expect(src.indexOf('list="bull"')).toBeLessThan(src.indexOf('list="bear"'));
});

test("no quiet, off or connecting lines left to draw", () => {
  const all = ["RossPanel.tsx", "RocketPanel.tsx", "DetectionPanel.tsx", "HaltWatcher.tsx", "MoversPanel.tsx"].map(read).join("\n");
  for (const s of ["Connecting…", "Off on this server", "No stock at 5/5", "Nothing flagged right now", "No buying bursts yet today", "NO_BUILDUPS", "Nothing developing right now"]) expect(all).not.toContain(s);
});

test("Runners: the Bursts section and the Flags section each collapse at 0; the Flags heading only beside Bursts", () => {
  const src = read("RocketPanel.tsx");
  expect(src).toContain("<Collapse open={burstCount > 0} reducedMotion={reducedMotion} gap={CARD_GAP}>");
  expect(src).toContain("<Collapse open={rows.length > 0} reducedMotion={reducedMotion} gap={CARD_GAP}>");
  const flags = src.slice(src.indexOf('accessibilityLabel="Flags"'));
  expect(flags.indexOf("<Collapse open={burstCount > 0} reducedMotion={reducedMotion}>")).toBeLessThan(flags.indexOf("Flags & running\n"));
  // Bursts come before the flags.
  expect(src.indexOf('accessibilityLabel="Bursts"')).toBeLessThan(src.indexOf('accessibilityLabel="Flags"'));
});

test("the dashboard passes its gap so a closed card takes no space; cards stay mounted (subscriptions, banners)", () => {
  const screen = read("../screens/DashboardScreen.tsx");
  for (const c of ["<HaltWatcher ", "<RossPanel ", "<DetectionPanel ", "<RocketPanel ", '<MoversPanel kind="gainers" ', '<MoversPanel kind="hot" ']) {
    const at = screen.indexOf(c);
    expect(at).toBeGreaterThan(0);
    expect(screen.slice(at, screen.indexOf("/>", at))).toContain("gap={GAP}");
    // Rendered unconditionally: the card itself decides, so its channel stays subscribed.
    expect(screen.slice(at - 3, at)).not.toContain("&&");
  }
});

test("Collapse animates real height (taps stay true), honours reduced motion, keeps the last rows while closing", () => {
  const src = read("../motion/Collapse.tsx");
  expect(src).not.toMatch(/LinearTransition|layout=\{/);
  expect(src).toContain("height: height.get() * progress.get()");
  expect(src).toContain("reducedMotion ? (open ? 1 : 0) : withTiming(open ? 1 : 0, collapseTiming)");
  expect(src).toContain("if (open && held !== children) setHeld(children);");
  expect(src).toContain('pointerEvents={open ? "auto" : "none"}');
  expect(src).toContain("marginBottom: -gap");
  expect(src).not.toMatch(/border(Width|Color|Top|Bottom|Left|Right)/);
});

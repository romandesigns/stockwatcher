// The lighter rows on the phone (owner, 2026-10-26, approved design canvas). The marks themselves
// are the shared model's (packages/shared/src/rowMarks.test.ts); these pin the phone-side wiring a
// bun test cannot render: every panel joins the Runners channel, press-and-hold reads the marks
// out without breaking tap or tap-then-hold, and the marks are drawn without borders.
import { expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { join } from "node:path";

const read = (p: string) => readFileSync(join(import.meta.dir, p), "utf8");

test("Ross and Favorites join the Runners channel for the path and the flame", () => {
  const screen = read("../screens/DashboardScreen.tsx");
  for (const panel of ["<RossPanel ", "<FavoritesPanel "]) {
    const at = screen.indexOf(panel);
    expect(screen.slice(at, screen.indexOf("/>", at))).toContain("runners={feed.rocket}");
  }
  expect(read("RossPanel.tsx")).toContain("rossRowMarks(w, byRunner.get(w.symbol), nowMs, formatEtTime)");
  expect(read("FavoritesPanel.tsx")).toContain("favoriteRowMarks(w, byRunner.get(s), nowMs, hhmm)");
  expect(read("RocketPanel.tsx")).toContain("rocketRowMarks(w, now, formatEtTime)");
});

test("every row reads its marks out on press and hold, keeping tap and tap-then-hold copy", () => {
  for (const f of ["RossPanel.tsx", "RocketPanel.tsx", "FavoritesPanel.tsx"]) {
    const src = read(f);
    expect(src).toContain("useReadoutPress(useTickerPress(");
    expect(src).toContain("<Flame flame=");
    expect(src).toMatch(/<MarksLine\s+marks=/);
  }
  const marks = read("RowMarks.tsx");
  // A hold right after a tap is the copy gesture: no read-out then.
  expect(marks).toContain("downAt.current - lastTap.current < 450");
  expect(marks).toContain("showToast(readout, { ms: 6000, lines: 8 })");
});

test("the demand-zone mark: the Demand zone icon, its label, fainter once tested, read out on hold", () => {
  const marks = read("RowMarks.tsx");
  expect(marks).toContain('accessibilityLabel={zone.label} style={zone.tested ? styles.tested : undefined} testID="mark-zone"');
  expect(marks).toContain('<Icon name="demand"');
  // The read-out (marksReadout, shared) includes it; the Runners row builds it from its wire.
  expect(read("../../../../packages/shared/src/rowMarks.ts")).toContain("zone: zoneMark(r.zone)");
});

test("marks are SVG and filled views: no borders, the flame in its own token, a ticking clock", () => {
  const marks = read("RowMarks.tsx");
  expect(marks).not.toMatch(/border(Width|Color|Top|Bottom|Left|Right)/);
  expect(marks).toContain("colors.flame");
  expect(marks).toContain("setInterval(() => setWall(Date.now()), ms)");
  expect(read("../theme.ts")).toContain('flame: "#ff8a3d"');
});

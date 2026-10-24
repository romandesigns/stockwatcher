import { describe, expect, test } from "bun:test";
import { etToUtc, sessionBands, type Clock, type ReplayAlert, type ReplayBarLayout } from "@stockwatcher/chart-core";
import { renderToStaticMarkup } from "react-dom/server";
import { ReplayBar, type ReplayBarProps } from "./ReplayBar.tsx";

// The replay bar, "B · Two-tier scrubber" (owner-approved 2026-10-24). The maths is tested in
// chart-core replayBar.test.ts; these check what the markup exposes: the three arrangements, the
// speed segments, the jumps' enabled state and names, the alert dots and the slider.

const bands = sessionBands({ year: 2026, month: 10, day: 23 });
const at = (h: number, m = 0, s = 0) => etToUtc(2026, 10, 23, h, m, s);
const clock: Clock = { mode: "replay", now: at(11, 11, 35), playing: false, speed: 5 };
const alerts: ReplayAlert[] = [
  { t: at(9, 41, 15), kind: "fast", symbol: "LABT", price: 0.8877 },
  { t: at(13, 0), kind: "run", symbol: "LABT", price: 1.1 },
  { t: at(10, 0), kind: "r2", symbol: "XXII" },
];

function render(p: Partial<ReplayBarProps> = {}, layout: ReplayBarLayout = "desktop") {
  return renderToStaticMarkup(
    <ReplayBar layout={layout} clock={clock} onPlayPause={() => {}} onSpeedChange={() => {}} onSeek={() => {}} bands={bands} candles={[{ t: at(9, 31), v: 1000 }]} symbol="LABT" price="0.9100" onTickerClick={() => {}} alerts={alerts} {...p} />,
  );
}

const count = (html: string, s: string) => html.split(s).length - 1;

describe("ReplayBar", () => {
  test("layouts: the artboard is named on the root, ticks hourly / 2-hourly / 4-hourly", () => {
    const desktop = render();
    expect(desktop).toContain('data-layout="desktop"');
    expect(desktop).toContain("Jump to");
    expect(desktop).toContain(">Pre-market<");
    expect(desktop).toContain(">05:00<");
    expect(desktop).not.toContain(">20:00<");
    const fold = render({}, "fold");
    expect(fold).toContain('data-layout="fold"');
    expect(fold).toContain(">06:00<");
    expect(fold).not.toContain(">05:00<");
    expect(fold).toContain("‹ Prev alert");
    const cover = render({}, "cover");
    expect(cover).toContain('data-layout="cover"');
    expect(cover).toContain(">Pre<");
    expect(cover).toContain(">08:00<");
    expect(cover).not.toContain(">06:00<");
    expect(cover).toContain(">20:00<");
  });

  test("top row: play as the accent button, the clock with its session and time in, the ticker", () => {
    const html = render();
    expect(html).toContain('aria-label="Play replay"');
    expect(html).toContain("bg-primary");
    expect(html).toContain("11:11:35");
    expect(html).toContain("Regular · 1h 41m in");
    expect(html).toContain('aria-label="LABT, 0.9100: search tickers"');
    expect(render({ clock: { ...clock, playing: true } })).toContain('aria-label="Pause replay"');
  });

  test("speed: five segments 1× 2× 5× 10× 30×, the current one pressed", () => {
    const html = render();
    expect(html).toContain('aria-label="Replay speed"');
    for (const s of ["1×", "2×", "5×", "10×", "30×"]) expect(html).toContain(`aria-label="Speed ${s}"`);
    expect(html).toMatch(/aria-pressed="true"[^>]*aria-label="Speed 5×"|aria-label="Speed 5×"[^>]*aria-pressed="true"/);
    expect(count(html, 'aria-pressed="true"')).toBe(1);
  });

  test("alert dots: the shown ticker's, each a named button; jumps to the previous and next one", () => {
    const html = render();
    expect(html).toContain('aria-label="Fast move at 09:41:15, $0.8877, jump there"');
    expect(html).toContain('aria-label="Run +20% at 13:00:00, $1.10, jump there"');
    expect(html).not.toContain("R2 flag at 10:00:00");
    expect(html).toContain('aria-label="Previous alert: LABT Fast move at 09:41:15, $0.8877"');
    expect(html).toContain('aria-label="Next alert: LABT Run +20% at 13:00:00, $1.10"');
    // After the last one: Next is disabled and says so.
    const late = render({ clock: { ...clock, now: at(15) } });
    expect(late).toMatch(/<button[^>]*disabled=""[^>]*aria-label="No later alert"|<button[^>]*aria-label="No later alert"[^>]*disabled=""/);
  });

  test("a ticker without alerts that day jumps across every ticker", () => {
    const html = render({ symbol: "QUIET" });
    expect(html).toContain("Next alert · any");
    expect(html).toContain('aria-label="Next alert: LABT Run +20% at 13:00:00, $1.10"');
    expect(html).toContain('aria-label="Previous alert: XXII R2 flag at 10:00:00"');
    expect(html).not.toContain("jump there");
    // No alerts at all: plain labels, both disabled.
    const none = render({ alerts: [] });
    expect(none).not.toContain("· any");
    expect(none).toContain('aria-label="No earlier alert"');
  });

  test("the scrubber is a keyboard slider over 04:00–20:00", () => {
    const html = render();
    expect(html).toContain('role="slider"');
    expect(html).toContain(`aria-valuemin="${at(4)}"`);
    expect(html).toContain(`aria-valuemax="${at(20)}"`);
    expect(html).toContain(`aria-valuenow="${at(11, 11, 35)}"`);
    expect(html).toContain('aria-valuetext="11:11:35 Eastern, regular session"');
    expect(html).toContain('tabindex="0"');
  });

  test("live or before the clock: nothing to control", () => {
    const live = render({ clock: { ...clock, mode: "live" } });
    expect(live).toContain('aria-disabled="true"');
    const none = render({ clock: null });
    expect(none).toContain("--:--:--");
    expect(none).toContain("Clock not started");
  });

  test("no native controls, no borders", () => {
    for (const layout of ["desktop", "fold", "cover"] as const) {
      const html = render({}, layout);
      expect(html).not.toMatch(/<(select|input|option|progress|meter)[\s>]/);
      expect(html).not.toMatch(/\bborder(-[a-z])?\b(?!-)/);
    }
  });
});

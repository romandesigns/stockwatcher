import { describe, expect, test } from "bun:test";
import { EMPTY_ROCKET, EMPTY_ROSS, rossDemoPanel, type RocketState, type RossState } from "@stockwatcher/shared";
import { renderToStaticMarkup } from "react-dom/server";
import { RossPanelView } from "./RossPanel.tsx";

// The lighter rows on the Ross Panel (owner, 2026-10-26, approved design canvas): marks instead
// of sentences, each named in its label, and the flame joined from the Runners channel.

const T_MS = 1_792_075_200_000;
const demo = rossDemoPanel(T_MS * 1_000_000);
const sym = demo.rows[0]!.symbol;
const priceDollars = (demo.rows[0]!.price ?? 0) / 1_000_000;
const ross: RossState = { ...EMPTY_ROSS, enabled: true, connected: true, panel: demo };

const runners = (burstAt: number, alertPrice: number): RocketState => ({
  ...EMPTY_ROCKET,
  enabled: true,
  connected: true,
  receivedAt: T_MS,
  panel: {
    t: T_MS,
    enabled: true,
    strategy: null,
    mode: "budget",
    budget: 10,
    threshold: 1,
    today: { date: "2026-10-26", alerts: 1, hits: 0, hits30: 0, known: 0 },
    days: [],
    rows: [{ id: "1", kind: "burst", s: sym, at: burstAt, p: alertPrice, last: priceDollars, pc: null, score: 1, reasons: [], outcome: { state: "pending", gain: null, drawdown: null, partial: false } }],
  },
});

const render = (r?: RocketState) => renderToStaticMarkup(<RossPanelView state={ross} onPick={() => {}} runners={r} />);

describe("Ross rows as lighter rows", () => {
  test("two lines of marks, each named; the old sentences are gone", () => {
    const html = render();
    expect(html).toContain('data-testid="row-marks"');
    expect(html).toContain('data-testid="mark-pillars"');
    expect(html).toMatch(/aria-label="Five pillars · \d of 5/);
    // Not drawn as text any more (the row's spoken summary still says it).
    const visible = html.replace(/aria-label="[^"]*"/g, "");
    expect(visible).not.toContain("close pending");
    expect(visible).not.toContain("over low");
    // No Runners row joined: no path, no flame.
    expect(html).not.toContain('data-testid="mark-path"');
    expect(html).not.toContain('data-testid="row-flame"');
  });

  test("a fresh Buying burst holding its alert price lights the flame and names the path", () => {
    const html = render(runners(T_MS - 2 * 60_000, priceDollars * 0.9));
    expect(html).toContain('data-testid="row-flame"');
    expect(html).toContain("Look now: Buying burst 2m ago");
    expect(html).toContain("text-flame");
    expect(html).toContain('data-testid="mark-path" data-kind="burst"');
  });

  test("the flame goes out after 5 minutes, and under the alert price", () => {
    expect(render(runners(T_MS - 5 * 60_000, priceDollars * 0.9))).not.toContain('data-testid="row-flame"');
    expect(render(runners(T_MS - 60_000, priceDollars * 1.1))).not.toContain('data-testid="row-flame"');
  });
});

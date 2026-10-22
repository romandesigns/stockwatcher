import { describe, expect, test } from "bun:test";
import type { RocketPanelWire, RocketState } from "@stockwatcher/shared";
import { renderToStaticMarkup } from "react-dom/server";
import { RocketPanelView } from "./RocketPanel.tsx";

const T = 1_792_075_200_000;

const panel: RocketPanelWire = {
  t: T,
  enabled: true,
  strategy: { id: "placeholder-b2", version: "1", placeholder: true },
  mode: "budget",
  budget: 10,
  threshold: 1.2,
  today: { date: "2026-10-15", alerts: 2, hits: 1, hits30: 1, known: 2 },
  days: [],
  rows: [
    { id: "b", s: "FRSH", at: T - 10_000, p: 2, last: 2.1, pc: 2, score: 1.5, reasons: ["volume 8.2× 30-min avg"], outcome: { state: "pending", gain: null, drawdown: null, partial: false } },
    { id: "a", s: "ROCK", at: T - 30 * 60_000, p: 1, last: 1.4, pc: 1, score: 2, reasons: ["grinding: 7 higher closes", "near premarket high", "third"], outcome: { state: "rocketed", gain: 34.2, drawdown: -1, partial: false } },
  ],
};

const render = (state: RocketState) => renderToStaticMarkup(<RocketPanelView state={state} wallNow={state.receivedAt} onPick={() => {}} />);

describe("Runners panel", () => {
  test("header, experimental chip, record, rows with reasons, fresh accent, outcome tag", () => {
    const html = render({ enabled: true, panel, receivedAt: 5, connected: true });
    expect(html).toContain("Runners");
    expect(html).toContain("Experimental");
    expect(html).toContain("1 of 2 rocketed today");
    expect(html).toContain("placeholder-b2 v1 · budget 10/day (placeholder)");
    expect(html).toContain("FRSH");
    expect(html).toContain("now");
    expect(html).toContain("volume 8.2× 30-min avg");
    expect(html).toContain("+34% ✓");
    expect(html).toContain("30m");
    // Two reason chips at most.
    expect(html).not.toContain("third");
    // The accent only on the fresh row's age; green only on the outcome.
    expect(html.match(/text-primary/g)?.length).toBe(1);
    expect(html.match(/text-good/g)?.length).toBe(1);
  });
  test("empty, off, connecting, reconnecting", () => {
    expect(render({ enabled: true, panel: { ...panel, rows: [] }, receivedAt: 0, connected: true })).toContain("Nothing flagged right now");
    expect(render({ enabled: false, panel: null, receivedAt: 0, connected: true })).toContain("Off on this server");
    expect(render({ enabled: null, panel: null, receivedAt: 0, connected: false })).toContain("Connecting…");
    expect(render({ enabled: true, panel, receivedAt: 0, connected: false })).toContain("Reconnecting");
  });
});

describe("Runners momentum rows (2026-10-22)", () => {
  const rows: RocketPanelWire["rows"] = [
    { id: "w", s: "WAIT", at: T - 20 * 60_000, p: 1, last: 1, pc: 1, score: 1, reasons: [], outcome: { state: "faded", gain: 2, drawdown: -3, partial: false, gain_2h: 43.4 }, state: "watching" },
    { id: "b", s: "BRKO", at: T - 5 * 60_000, p: 1, last: 1.2, pc: 1, score: 1, reasons: [], outcome: { state: "pending", gain: null, drawdown: null, partial: false }, state: "breaking_out", provisional: true, state_since: T - 30_000, from_flag_pct: 20, from_high_pct: 0, move_1m_pct: 3.1, day_line_start: 1, day_line_from: 0, day_line: [1_000_000, 1_100_000] },
    { id: "p", s: "PULL", at: T - 9 * 60_000, p: 1, last: 1.1, pc: 1, score: 1, reasons: [], outcome: { state: "pending", gain: null, drawdown: null, partial: false }, state: "pullback", from_high_pct: -4, pullback_depth_pct: 4, levels: { state: "forming", trigger: 1_150_000, invalidation: 1_050_000, meter_milli: 500 } },
    { id: "h", s: "HLTD", at: T - 9 * 60_000, p: 1, last: 1.1, pc: 1, score: 1, reasons: [], outcome: { state: "pending", gain: null, drawdown: null, partial: false }, state: "halted" },
  ];
  const html = render({ enabled: true, panel: { ...panel, today: { ...panel.today, ran_later: 1 }, rows }, receivedAt: 5, connected: true });
  test("state labels, order, edge, halted badge, meter, day line, ran later", () => {
    expect(html.indexOf("BRKO")).toBeLessThan(html.indexOf("HLTD"));
    expect(html.indexOf("HLTD")).toBeLessThan(html.indexOf("PULL"));
    expect(html.indexOf("PULL")).toBeLessThan(html.indexOf("WAIT"));
    expect(html).toContain("Breaking out");
    expect(html).toContain("provisional");
    expect(html).toContain("inset_3px_0_0_var(--accent)");
    expect(html).toContain("HALTED");
    expect(html).toContain("−4.0% off high · trigger 1.15");
    expect(html).toContain("of the way from the pullback low to the trigger");
    expect(html).toContain('data-testid="rocket-day-line"');
    expect(html).toContain("ran later +43%");
    expect(html).toContain("1 ran ≥ +15% later");
    expect(html).toContain("1 building or breaking out");
    // Still no green but the strict outcome tag (none here), and the pulse stills with reduced motion.
    expect(html).not.toContain("text-good");
    expect(html).toContain("motion-reduce:animate-none");
  });
});

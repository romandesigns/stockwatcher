import { describe, expect, test } from "bun:test";
import type { BurstOutcomeWire, RocketPanelWire, RocketState } from "@stockwatcher/shared";
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

describe("Buying bursts (2026-10-22)", () => {
  const outcome: BurstOutcomeWire = { state: "pending", c10: false, c20: false, c30: false, stopped: false, mfe: null, mae: null, settled: false };
  const burstRow = (id: string, s: string, at: number, o: Partial<BurstOutcomeWire> = {}): RocketPanelWire["rows"][number] => ({
    id,
    s,
    kind: "burst",
    at,
    p: 2,
    last: 2.1,
    pc: 1.5,
    score: 0.61,
    reasons: ["$ rate 6.4× base", "12 new highs in 15 s", "3 sweeps"],
    outcome: { state: "pending", gain: null, drawdown: null, partial: false },
    burst: { start: at - 6 * 60_000, start_price: 1.7, gain_pct: 18, move_pct: 4.2, rate_x: 6.4, new_highs: 12, sweeps: 3, buy_pct: 88, outcome: { ...outcome, ...o } },
  });
  const detector: RocketPanelWire["burst"] = { enabled: true, threshold: 0.42, per_day: 20, today: 2, typical: { mfe30_med_pct: 13.4, mae_med_pct: -2.8, cont10_pct: 37, cont20_pct: 23, fires_after_min: [5, 9] } };
  const runner = { ...panel.rows[1]!, id: "r", s: "FLAG", at: T - 40 * 60_000 };

  test("a Bursts section on top, newest first, then the flags; the accent only on the live burst; strict tags coloured", () => {
    const rows = [runner, burstRow("old", "OLDB", T - 20 * 60_000, { state: "stopped", stopped: true }), burstRow("new", "NEWB", T - 60_000)];
    const html = render({ enabled: true, panel: { ...panel, burst: detector, rows }, receivedAt: 5, connected: true });
    expect(html).toContain("Bursts");
    expect(html).toContain("2 today");
    expect(html).toContain("typically 5–9 min into a move (about halfway)");
    expect(html).toContain("Typical from here: +13% max in 30 min (median), 1 in 4 reach +20% before −5%");
    expect(html.indexOf("NEWB")).toBeLessThan(html.indexOf("OLDB"));
    expect(html.indexOf("OLDB")).toBeLessThan(html.indexOf("Flags"));
    expect(html.indexOf("Flags")).toBeLessThan(html.indexOf("FLAG<"));
    expect(html).toContain("Buying burst");
    expect(html).toContain("+18.0% since burst start · 7m ago");
    expect(html).toContain("+4.2% last 30 s");
    expect(html).toContain("−5% first");
    expect(html).toContain("text-critical");
    // Green only on the runner's strict tag; no "buy" advice anywhere.
    expect(html.match(/text-good/g)?.length).toBe(1);
    expect(html).not.toMatch(/\bBuy\b|\bbuy\b/);
    // The live burst (NEWB) has the accent headline and edge; the stopped one does not.
    const newb = html.slice(html.indexOf("NEWB"), html.indexOf("OLDB"));
    expect(newb).toContain("text-primary");
    const oldb = html.slice(html.indexOf("OLDB"), html.indexOf("Flags"));
    expect(oldb).not.toContain("text-primary");
  });

  test("detector on, nothing yet: an empty Bursts section; no detector: no section", () => {
    expect(render({ enabled: true, panel: { ...panel, burst: { ...detector, today: 0 } }, receivedAt: 5, connected: true })).toContain("No buying bursts yet today");
    expect(render({ enabled: true, panel, receivedAt: 5, connected: true })).not.toContain("Bursts");
  });
});

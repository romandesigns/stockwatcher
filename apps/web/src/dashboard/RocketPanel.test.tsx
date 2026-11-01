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

const render = (state: RocketState) => renderToStaticMarkup(<RocketPanelView state={state} onPick={() => {}} />);
/**
 * Green outside the day line's last-minute tail. The lighter rows (owner-approved design,
 * 2026-10-26) colour that tail by its direction on every row; elsewhere green is still only the
 * strict outcome tag.
 */
const green = (html: string) => html.replace(/<path[^>]*data-testid="row-line-tail"[^>]*><\/path>/g, "").match(/text-good/g)?.length ?? 0;

describe("Runners panel", () => {
  test("unknown exact history stays in inspection details without adding visible row text", () => {
    const rows = [{ ...panel.rows[0]!, breakout_tracking: { status: "unknown" as const, reason: "missing_creation_suffix", policy_id: "rocket-momentum-15s-exact-after-v1" } }];
    const html = render({ enabled: true, panel: { ...panel, rows }, receivedAt: 5, connected: true });
    expect(html).toContain("FRSH");
    expect(html).toContain("Exact breakout history unavailable · missing creation suffix");
    // The original compact row stays intact; hover/accessibility retain the diagnostic.
    expect(html.replace(/<[^>]*>/g, "")).not.toContain("Exact breakout history unavailable");
  });
  test("a disconnected header does not claim building or breakout attention", () => {
    const p = { ...panel, rows: panel.rows.map((r) => ({ ...r, state: "building" as const })) };
    expect(render({ enabled: true, panel: p, receivedAt: 5, connected: true })).toContain("2 building or breaking out");
    const stale = render({ enabled: true, panel: p, receivedAt: 5, connected: false });
    expect(stale).toContain("Reconnecting");
    expect(stale).not.toContain("2 building or breaking out");
  });
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
    // The accent only on the fresh row's pulsing dot; green only on the outcome.
    expect(html.match(/text-primary/g)).toBeNull();
    expect(html.match(/bg-primary/g)?.length).toBe(1);
    expect(html).toContain('aria-label="Flagged under a minute ago"');
    expect(green(html)).toBe(1);
  });
  test("running entries (2026-10-22): a quiet source chip on every row, strongest first, measured from the entry", () => {
    const running = { id: "r", s: "TANH", kind: "running" as const, at: T - 5 * 60_000, p: 10.4, last: 11.2, pc: 8.8, score: 1.3, reasons: ["+30% off the 60-min low (needs 22%)"], outcome: { state: "pending" as const, gain: null, drawdown: null, partial: false }, state: "running" as const, from_flag_pct: 7.7 };
    const html = render({ enabled: true, panel: { ...panel, strategy: { id: "precursors-R2", version: "1", placeholder: false }, rows: [...panel.rows, running] }, receivedAt: 5, connected: true });
    // Running (accent state) is listed above the watched flags.
    expect(html.indexOf("TANH")).toBeLessThan(html.indexOf("FRSH"));
    expect(html).toContain('data-kind="running"');
    // Lighter rows (2026-10-26): the source is the path mark, named with its age and move in its label.
    expect(html).toContain('aria-label="Runners: Running at');
    expect(html).toContain('aria-label="Runners: R2 flag at');
    expect(html).toContain("+7.7% since");
    expect(html).not.toContain("from entry, from high, last minute");
  });
  test("Run +20% entries (2026-10-22): their own quiet chip beside the others, never accent, measured from the entry", () => {
    const run = { id: "g", s: "LEGS", kind: "run" as const, at: T - 5 * 60_000, p: 1.22, last: 1.3, pc: 0.9, score: 1.1, reasons: ["+22% in 5 min off $1.00"], outcome: { state: "pending" as const, gain: null, drawdown: null, partial: false }, state: "watching" as const, from_flag_pct: 6.6 };
    const html = render({ enabled: true, panel: { ...panel, strategy: { id: "precursors-R2", version: "1", placeholder: false }, rows: [...panel.rows, run] }, receivedAt: 5, connected: true });
    expect(html).toContain('data-kind="run"');
    const mark = /<span class="([^"]*)" role="img" aria-label="Runners: Run \+20% at[^"]*" data-testid="mark-path" data-kind="run"/.exec(html);
    expect(mark).not.toBeNull();
    expect(mark?.[1]).not.toMatch(/primary|accent/);
    expect(html).toContain("+6.6% since");
  });
  test("Fast move entries (2026-10-22): their own quiet chip, never accent, measured from the entry", () => {
    const fast = { id: "z", s: "ZAP", kind: "fast" as const, at: T - 60_000, p: 2.41, last: 2.5, pc: 2.0, score: 1.05, reasons: ["+8% in 60 s (held)", "52 trades, $71k in 60 s"], outcome: { state: "pending" as const, gain: null, drawdown: null, partial: false }, state: "watching" as const, from_flag_pct: 3.7 };
    const html = render({ enabled: true, panel: { ...panel, strategy: { id: "precursors-R2", version: "1", placeholder: false }, rows: [...panel.rows, fast] }, receivedAt: 5, connected: true });
    expect(html).toContain('data-kind="fast"');
    const mark = /<span class="([^"]*)" role="img" aria-label="Runners: Fast move at[^"]*" data-testid="mark-path" data-kind="fast"/.exec(html);
    expect(mark).not.toBeNull();
    expect(mark?.[1]).not.toMatch(/primary|accent/);
    expect(html).toContain("+3.7% since");
  });
  test("no quiet, off or connecting lines (the tab is hidden then); reconnecting keeps the rows, dimmed", () => {
    const empty = render({ enabled: true, panel: { ...panel, rows: [] }, receivedAt: 0, connected: true });
    expect(empty).not.toContain("Nothing flagged");
    expect(empty).not.toContain("Flags");
    expect(render({ enabled: false, panel: null, receivedAt: 0, connected: true })).not.toContain("Off on this server");
    expect(render({ enabled: null, panel: null, receivedAt: 0, connected: false })).not.toContain("Connecting");
    const lost = render({ enabled: true, panel, receivedAt: 0, connected: false });
    expect(lost).toContain("Reconnecting");
    expect(lost).toContain("FRSH");
    expect(lost).toContain("opacity-60");
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
    // The setup as a stage mark (its meter and trigger), named in its label.
    expect(html).toContain('data-stage="armed"');
    expect(html).toContain('aria-label="Setup armed · trigger 1.15 · stop 1.05"');
    expect(html).toContain('data-testid="row-line"');
    expect(html).toContain('data-testid="row-line-tail"');
    expect(html).toContain("ran later +43%");
    expect(html).toContain("1 ran ≥ +15% later");
    expect(html).toContain("1 building or breaking out");
    // Still no green but the strict outcome tag (none here), and the pulse stills with reduced motion.
    expect(green(html)).toBe(0);
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
    expect(html).toContain("Research ($0.15–$25): +13% max in 30 min (median), 1 in 4 reach +20% before −5%");
    expect(html.indexOf("NEWB")).toBeLessThan(html.indexOf("OLDB"));
    expect(html.indexOf("OLDB")).toBeLessThan(html.indexOf("Flags"));
    expect(html.indexOf("Flags")).toBeLessThan(html.indexOf("FLAG<"));
    expect(html).toContain("Buying burst");
    expect(html).toContain("+18.0% since burst start · 7m ago");
    expect(html).toContain("+4.2% last 30 s");
    expect(html).toContain("−5% first");
    expect(html).toContain("text-critical");
    // Green only on the runner's strict tag; no "buy" advice anywhere.
    expect(green(html)).toBe(1);
    expect(html).not.toMatch(/\bBuy\b|\bbuy\b/);
    // The live burst (NEWB) has the accent headline and edge; the stopped one does not.
    const newb = html.slice(html.indexOf("NEWB"), html.indexOf("OLDB"));
    expect(newb).toContain("text-primary");
    const oldb = html.slice(html.indexOf("OLDB"), html.indexOf("Flags"));
    expect(oldb).not.toContain("text-primary");
  });

  test("empty sections are not drawn, headings included (owner, 2026-10-22)", () => {
    // Detector on but no burst yet: no Bursts section and no Flags heading, just the flags.
    const noBursts = render({ enabled: true, panel: { ...panel, burst: { ...detector, today: 0 } }, receivedAt: 5, connected: true });
    expect(noBursts).not.toContain("Bursts");
    expect(noBursts).not.toContain("No buying bursts");
    expect(noBursts).not.toContain(">Flags &amp; running<");
    expect(noBursts).toContain("FRSH");
    expect(render({ enabled: true, panel, receivedAt: 5, connected: true })).not.toContain("Bursts");
    // Bursts but no flags: the Bursts section alone, no Flags heading or quiet line.
    const onlyBursts = render({ enabled: true, panel: { ...panel, burst: detector, rows: [burstRow("new", "NEWB", T - 60_000)] }, receivedAt: 5, connected: true });
    expect(onlyBursts).toContain("Bursts");
    expect(onlyBursts).toContain("NEWB");
    expect(onlyBursts).not.toContain(">Flags &amp; running<");
    expect(onlyBursts).not.toContain("Nothing flagged");
  });
});

describe("Volume shelves on a row (2026-10-22)", () => {
  const inShelf = { state: "in_shelf" as const, lo: 1.3, hi: 1.38, entry: 1.35, at: T - 60_000, resolved: null };
  test("in the shelf: the range and the muted base rate, no green and no extra accent", () => {
    const plain = render({ enabled: true, panel, receivedAt: 5, connected: true });
    const html = render({ enabled: true, panel: { ...panel, zones: { ROCK: { bands: [], event: inShelf } } }, receivedAt: 5, connected: true });
    expect(html).toContain("In volume shelf $1.30–$1.38");
    expect(html).toContain("about 1 in 3 hold, 1 in 2 fail");
    expect(html).toContain('data-zone="in_shelf"');
    expect(html.match(/text-good/g)?.length).toBe(plain.match(/text-good/g)?.length);
    expect(html.match(/text-primary/g)?.length).toBe(plain.match(/text-primary/g)?.length);
    // Inside a <button> row: phrasing content only.
    const row = html.slice(html.lastIndexOf("<button", html.indexOf("In volume shelf")), html.indexOf("In volume shelf"));
    expect(row).not.toContain("<div");
  });
  test("held / lost are stated plainly; expired, no zones and garbage show nothing", () => {
    const at = (state: "held" | "lost" | "expired") => render({ enabled: true, panel: { ...panel, zones: { ROCK: { bands: [], event: { ...inShelf, state, resolved: T } } } }, receivedAt: 5, connected: true });
    expect(at("held")).toContain("Shelf held");
    expect(at("lost")).toContain("Shelf lost");
    expect(at("expired")).not.toContain("data-zone");
    expect(render({ enabled: true, panel, receivedAt: 5, connected: true })).not.toContain("data-zone");
    expect(render({ enabled: true, panel: { ...panel, zones: { ROCK: 7 } as never }, receivedAt: 5, connected: true })).not.toContain("data-zone");
  });

  test("a row whose price is in a demand zone shows the zone mark, named with its edges and hold rate", () => {
    const rows = panel.rows.map((r) => (r.s === "ROCK" ? { ...r, zone: { lo: 1.31, hi: 1.42, tested: true, formed_t: T - 20 * 60_000 } } : r));
    const html = render({ enabled: true, panel: { ...panel, rows }, receivedAt: 5, connected: true });
    expect(html.match(/data-testid="mark-zone"/g)?.length).toBe(1);
    expect(html).toContain('aria-label="In demand zone $1.31–$1.42 · zones held about 6 in 10 retests (5 in 10 at random levels)"');
    // Fainter once price has been back in it; no zone, no mark.
    expect(html).toMatch(/opacity-60[^>]*data-testid="mark-zone"[^>]*data-tested="true"/);
    expect(render({ enabled: true, panel, receivedAt: 5, connected: true })).not.toContain('data-testid="mark-zone"');
  });
});

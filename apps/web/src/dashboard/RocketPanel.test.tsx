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

describe("Rocket watch panel", () => {
  test("header, experimental chip, record, rows with reasons, fresh accent, outcome tag", () => {
    const html = render({ enabled: true, panel, receivedAt: 5, connected: true });
    expect(html).toContain("Rocket watch");
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

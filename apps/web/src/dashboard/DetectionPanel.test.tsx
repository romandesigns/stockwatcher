import { describe, expect, test } from "bun:test";
import { detectionSnapshotView, EMPTY_DETECTION, type DetectionCoreEpisode, type DetectionMessage, type DetectionState } from "@stockwatcher/shared";
import { renderToStaticMarkup } from "react-dom/server";
import { DetectionPanelView } from "./DetectionPanel.tsx";

const MIN = 60_000_000_000;
const NOW = 1_792_075_200_000_000_000;
const episode = (symbol: string, state: string, patch: Partial<DetectionCoreEpisode> = {}, near: boolean | null = null): DetectionCoreEpisode => {
  const id = `live/${symbol}/micro/1`;
  return {
    episode_id: id, family: "micro_pullback", variant: "micro_pullback_v0", policy: "detection-v2@test", symbol, instrument: symbol, trading_date: "2026-10-22", segment: "2026-10-22/regular", state, default_visible: true,
    first_detected_at: NOW - 5 * MIN, first_bar_end: NOW - 5 * MIN, last_material_seq: 1, last_material_at: NOW - 2 * MIN, base_secs: 15, fidelity: "Live", warmup: false, parent_episode_id: null, related_episode_ids: [], context: null,
    levels: [{ level_id: `${id}:trigger`, kind: "trigger", version: 1, price: 3_150_000, known_at: NOW - 4 * MIN, armed_at: NOW - 4 * MIN, formed_from: [1, 2], reason: "impulse_high" }, { level_id: `${id}:invalidation`, kind: "invalidation", version: 1, price: 3_020_000, known_at: NOW - 4 * MIN, armed_at: NOW - 4 * MIN, formed_from: [1, 2], reason: "pullback_low" }],
    anchors: null, recognition: null, checks: [], current: { price: 3_120_000, price_exec: NOW, price_age_ms: 200, stale: false, ...(near === null ? {} : { near_trigger: near }) }, resolution: null, ...patch,
  };
};
const message = (active: DetectionCoreEpisode[], recent: DetectionCoreEpisode[] = [], monitored = 1284): DetectionMessage => ({
  type: "detection.snapshot", enabled: true, source: "live", runEpoch: "live:1", cursor: 1,
  snapshot: { schema: 1, run_id: "live", source: "live", fidelity: "Live", policy: "detection-v2@test", experimental: true, alerts_enabled: false, cursor: 1, bar_end: NOW, cutoff: NOW, as_of: NOW, symbols: active.map((e) => ({ symbol: e.symbol, dominant_episode_id: e.episode_id, active_episode_ids: [e.episode_id], interrupted: null })), active, recent, data_status: { global_interruption: null, disabled_reason: null }, telemetry: { transitions_dropped: 0 }, monitored: { symbols_seen: monitored, symbols_today: monitored, symbols_active: active.length, symbols_interrupted: 0 } },
  transitions: [], history: { capacity: 2048, dropped: 0, first_sequence: null, last_sequence: null }, telemetry: { engine: { transitions_dropped: 0, recent_dropped: 0 }, journal_queue_dropped: 0, journal_write_errors: 0 },
  profile: { price_min_micros: 100_000, price_max_micros: 25_000_000, research_min_micros: 150_000, research_max_micros: 25_000_000 }, experimental: true, alertsEnabled: false,
});
const stateOf = (m: DetectionMessage, connected = true): DetectionState => ({ ...EMPTY_DETECTION, enabled: true, connected, snapshot: detectionSnapshotView(m), frame: m });
const render = (s: DetectionState) => renderToStaticMarkup(<DetectionPanelView state={s} onPick={() => {}} />);

describe("Signals (design canvas, 2026-10-22)", () => {
  const html = render(stateOf(message([episode("KLMN", "crossing_provisional", { last_material_at: NOW - 10_000_000_000 }), episode("PQRS", "armed", {}, true), episode("XYZA", "pullback_forming"), episode("TUVW", "trigger_observed")])));

  test("honest stage labels with level notes and ages; Active / Recent segmented", () => {
    for (const s of ["KLMN", "Crossing", "3.15 · close pending", "At trigger", "Pullback", "Broke trigger", "now", "2m", "Micro-pullback", "Active", "Recent", "Filters"]) expect(html).toContain(s);
    expect(html).toContain('aria-pressed="true"');
    expect(html).toContain('role="group" aria-label="Show"');
  });

  test("accent only on the crossing; amber edge at the trigger; no green, no borders, no native controls", () => {
    expect(html).toContain("shadow-[inset_3px_0_0_var(--accent)]");
    expect(html).toContain("shadow-[inset_3px_0_0_var(--warning)]");
    expect(html).not.toMatch(/text-good|bg-good/);
    expect(html).not.toMatch(/class="[^"]*\bborder/);
    expect(html).not.toMatch(/<select|<details|title="/);
    // The crossing's label, its pulse and its meter are the only accent text on a quiet panel.
    expect(html.match(/text-primary/g)?.length).toBe(2);
  });

  test("the trigger meter only on live pullback stages", () => {
    expect(html.match(/from the pullback low to the trigger/g)?.length).toBe(3);
  });

  test("quiet: nothing developing, with what is watched", () => {
    const quiet = render(stateOf(message([])));
    expect(quiet).toContain("Nothing developing right now");
    expect(quiet).toContain("watching 1,284 symbols · $0.10–$25");
  });

  test("reconnecting: last known rows dim, nothing pulses or claims to be at the trigger", () => {
    const lost = render(stateOf(message([episode("KLMN", "crossing_provisional"), episode("PQRS", "armed", {}, true)]), false));
    expect(lost).toContain("Reconnecting · last");
    expect(lost).toContain("opacity-60");
    expect(lost).not.toContain("animate-pulse");
    expect(lost).not.toContain("At trigger");
  });
});

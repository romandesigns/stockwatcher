import { describe, expect, test } from "bun:test";
import { detectionSnapshotView, EMPTY_DETECTION, type DetectionCoreEpisode, type DetectionMessage, type DetectionState } from "./detection.ts";
import { detectionItemCount, detectionView, detectionViewCounts, panelsSettled, pickSideTab, rocketItemCount, rocketSectionCounts, rossItemCount, visibleSideTabs } from "./panels.ts";
import { EMPTY_ROCKET, type RocketPanelWire, type RocketState } from "./rocket.ts";
import { EMPTY_ROSS, type RossState } from "./ross.ts";
import type { RossPanelWire } from "./protocol.ts";

const ross = (rows: number, patch: Partial<RossState> = {}): RossState => ({ ...EMPTY_ROSS, enabled: true, connected: true, panel: { rows: Array.from({ length: rows }, () => ({})) } as unknown as RossPanelWire, ...patch });
const rocketRows = (bursts: number, flags: number) => [
  ...Array.from({ length: bursts }, (_, i) => ({ id: `b${i}`, kind: "burst", burst: {} })),
  ...Array.from({ length: flags }, (_, i) => ({ id: `f${i}` })),
];
const rocket = (bursts: number, flags: number, patch: Partial<RocketState> = {}): RocketState => ({ ...EMPTY_ROCKET, enabled: true, connected: true, panel: { rows: rocketRows(bursts, flags) } as unknown as RocketPanelWire, ...patch });

const episode = (symbol: string, state: string, terminal = false): DetectionCoreEpisode => ({
  episode_id: `fixture/${symbol}/micro/1`, family: "micro_pullback", variant: "micro_pullback_v0", policy: "detection-v0@test", symbol, instrument: symbol, trading_date: "2026-10-16", segment: "2026-10-16/regular", state, default_visible: true,
  first_detected_at: 100, first_bar_end: 98, last_material_seq: 1, last_material_at: 100, base_secs: 15, fidelity: "Synthetic", warmup: false, parent_episode_id: null, related_episode_ids: [], context: null, levels: [], anchors: null, recognition: null, checks: [],
  current: { price: 3_340_000, price_exec: 101, price_age_ms: 25, stale: false }, resolution: terminal ? { state, at: 100, reason: "done" } as unknown as DetectionCoreEpisode["resolution"] : null,
});
const detection = (active: DetectionCoreEpisode[], recent: DetectionCoreEpisode[], patch: Partial<DetectionState> = {}): DetectionState => {
  const m: DetectionMessage = { type: "detection.snapshot", enabled: true, source: "fixture", runEpoch: "fixture:1", cursor: 1, snapshot: { schema: 1, run_id: "fixture", source: "fixture", fidelity: "Synthetic", policy: "detection-v0@test", experimental: true, alerts_enabled: false, cursor: 1, bar_end: 98, cutoff: 100, as_of: 102, symbols: [], active, recent, data_status: { global_interruption: null, disabled_reason: null }, telemetry: { transitions_dropped: 0 } }, transitions: [], history: { capacity: 2048, dropped: 0, first_sequence: null, last_sequence: null }, telemetry: { engine: { transitions_dropped: 0, recent_dropped: 0 }, journal_queue_dropped: 0, journal_write_errors: 0 }, profile: { price_min_micros: 100_000, price_max_micros: 25_000_000, research_min_micros: 150_000, research_max_micros: 25_000_000 }, experimental: true, alertsEnabled: false };
  return { ...EMPTY_DETECTION, enabled: true, connected: true, snapshot: detectionSnapshotView(m), frame: m, ...patch };
};

describe("hide-when-empty item counts (owner, 2026-10-22)", () => {
  test("Ross: rows only while on; off, unknown and empty are all 0", () => {
    expect(rossItemCount(ross(3))).toBe(3);
    expect(rossItemCount(ross(0))).toBe(0);
    expect(rossItemCount(EMPTY_ROSS)).toBe(0);
    expect(rossItemCount({ ...EMPTY_ROSS, enabled: false })).toBe(0);
  });

  test("reconnecting keeps the last known rows counted, so a panel does not flash out", () => {
    expect(rossItemCount(ross(2, { connected: false }))).toBe(2);
    expect(rocketItemCount(rocket(1, 1, { connected: false }))).toBe(2);
    expect(detectionItemCount(detection([episode("AAA", "armed")], [], { connected: false, resyncing: true }))).toBe(1);
  });

  test("Runners: bursts and flags counted apart, each section hidden at 0", () => {
    expect(rocketSectionCounts(rocket(2, 0))).toEqual({ bursts: 2, flags: 0 });
    expect(rocketSectionCounts(rocket(0, 3))).toEqual({ bursts: 0, flags: 3 });
    expect(rocketItemCount(rocket(0, 0))).toBe(0);
    expect(rocketItemCount({ ...EMPTY_ROCKET, enabled: false })).toBe(0);
  });

  test("Signals: Active and Recent counted apart; off is 0", () => {
    const s = detection([episode("AAA", "armed")], [episode("BBB", "invalidated", true)]);
    const c = detectionViewCounts(s);
    expect(c.active).toBe(1);
    expect(c.recent).toBe(1);
    expect(detectionItemCount({ ...s, enabled: false })).toBe(0);
    expect(detectionItemCount(EMPTY_DETECTION)).toBe(0);
  });

  test("Signals' view toggle only while both views have rows; an empty view falls to the other", () => {
    expect(detectionView(true, { active: 2, recent: 1 })).toEqual({ recent: true, showToggle: true });
    expect(detectionView(false, { active: 2, recent: 1 })).toEqual({ recent: false, showToggle: true });
    expect(detectionView(true, { active: 2, recent: 0 })).toEqual({ recent: false, showToggle: false });
    expect(detectionView(false, { active: 0, recent: 3 })).toEqual({ recent: true, showToggle: false });
    expect(detectionView(false, { active: 0, recent: 0 }).showToggle).toBe(false);
  });
});

describe("side tabs (web)", () => {
  test("a tab with 0 items is not offered; order is fixed", () => {
    expect(visibleSideTabs({ ross: 0, rocket: 2, signals: 1, favorites: 0 })).toEqual(["rocket", "signals"]);
    expect(visibleSideTabs({ ross: 1, rocket: 0, signals: 0, favorites: 3 })).toEqual(["ross", "favorites"]);
    expect(visibleSideTabs({ ross: 0, rocket: 0, signals: 0, favorites: 0 })).toEqual([]);
  });

  test("the picked tab while it has items, else the first non-empty one, else none", () => {
    expect(pickSideTab(["ross", "rocket", "signals"], "signals")).toBe("signals");
    expect(pickSideTab(["ross", "rocket"], "signals")).toBe("ross");
    expect(pickSideTab(["rocket", "signals"], "ross")).toBe("rocket");
    expect(pickSideTab([], "ross")).toBeUndefined();
  });

  test("settled once every present channel has said whether it is on", () => {
    expect(panelsSettled([true, false, undefined])).toBe(true);
    expect(panelsSettled([true, null])).toBe(false);
    expect(panelsSettled([])).toBe(true);
  });
});

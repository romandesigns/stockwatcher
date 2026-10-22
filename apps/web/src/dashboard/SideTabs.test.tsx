import { describe, expect, test } from "bun:test";
import { detectionSnapshotView, EMPTY_DETECTION, EMPTY_ROCKET, EMPTY_ROSS, type DetectionChannel, type DetectionCoreEpisode, type DetectionMessage, type DetectionState, type RocketChannel, type RocketPanelWire, type RocketState, type RossChannel, type RossState, rossDemoPanel } from "@stockwatcher/shared";
import { renderToStaticMarkup } from "react-dom/server";
import { SideTabs } from "./SideTabs.tsx";

// The hide-when-empty rule on the web side column (owner, 2026-10-22). Static renders: effects
// (the settle timer, the fallback's memory) do not run, which is the first frame a viewer sees.

const NOW = 1_792_075_200_000_000_000;
const MIN = 60_000_000_000;
const channel = <S,>(s: S) => ({ get: () => s, subscribe: () => () => {} });


const demo = rossDemoPanel(NOW);
const ross = (rows: string[], patch: Partial<RossState> = {}): RossState => ({ ...EMPTY_ROSS, enabled: true, connected: true, panel: { ...demo, rows: demo.rows.slice(0, rows.length) }, ...patch });
const rocket = (rows: string[], patch: Partial<RocketState> = {}): RocketState => ({
  ...EMPTY_ROCKET,
  enabled: true,
  connected: true,
  receivedAt: 5,
  panel: { t: 1, enabled: true, strategy: null, mode: "budget", budget: 10, threshold: 1, today: { date: "2026-10-22", alerts: rows.length, hits: 0, hits30: 0, known: 0 }, days: [], rows: rows.map((s, i) => ({ id: `${i}`, s, at: 0, p: 1, last: 1, pc: 1, score: 1, reasons: [], outcome: { state: "pending", gain: null, drawdown: null, partial: false } })) } as unknown as RocketPanelWire,
  ...patch,
});
const episode = (symbol: string): DetectionCoreEpisode => ({
  episode_id: `live/${symbol}/micro/1`, family: "micro_pullback", variant: "micro_pullback_v0", policy: "detection-v2@test", symbol, instrument: symbol, trading_date: "2026-10-22", segment: "2026-10-22/regular", state: "crossing_provisional", default_visible: true,
  first_detected_at: NOW - 5 * MIN, first_bar_end: NOW - 5 * MIN, last_material_seq: 1, last_material_at: NOW - MIN, base_secs: 15, fidelity: "Live", warmup: false, parent_episode_id: null, related_episode_ids: [], context: null,
  levels: [], anchors: null, recognition: null, checks: [], current: { price: 3_120_000, price_exec: NOW, price_age_ms: 200, stale: false }, resolution: null,
});
const detection = (symbols: string[], patch: Partial<DetectionState> = {}): DetectionState => {
  const active = symbols.map(episode);
  const m: DetectionMessage = {
    type: "detection.snapshot", enabled: true, source: "live", runEpoch: "live:1", cursor: 1,
    snapshot: { schema: 1, run_id: "live", source: "live", fidelity: "Live", policy: "detection-v2@test", experimental: true, alerts_enabled: false, cursor: 1, bar_end: NOW, cutoff: NOW, as_of: NOW, symbols: active.map((e) => ({ symbol: e.symbol, dominant_episode_id: e.episode_id, active_episode_ids: [e.episode_id], interrupted: null })), active, recent: [], data_status: { global_interruption: null, disabled_reason: null }, telemetry: { transitions_dropped: 0 } },
    transitions: [], history: { capacity: 2048, dropped: 0, first_sequence: null, last_sequence: null }, telemetry: { engine: { transitions_dropped: 0, recent_dropped: 0 }, journal_queue_dropped: 0, journal_write_errors: 0 },
    profile: { price_min_micros: 100_000, price_max_micros: 25_000_000, research_min_micros: 150_000, research_max_micros: 25_000_000 }, experimental: true, alertsEnabled: false,
  };
  return { ...EMPTY_DETECTION, enabled: true, connected: true, snapshot: detectionSnapshotView(m), frame: m, ...patch };
};

const render = (r: RossState | undefined, k: RocketState | undefined, d: DetectionState | undefined) =>
  renderToStaticMarkup(
    <SideTabs
      ross={r && (channel(r) as RossChannel)}
      rocket={k && (channel(k) as RocketChannel)}
      detection={d && (channel(d) as DetectionChannel)}
      onPick={() => {}}
      onEpisodePick={() => {}}
    />,
  );
const tabState = (html: string, id: string) => html.match(new RegExp(`data-reveal="(open|closed)"[^>]*>(?:<[^>]*>){1,2}<button[^>]*id="side-tab-${id}"`))?.[1] ?? "absent";

describe("side column: a panel with 0 items is hidden (owner, 2026-10-22)", () => {
  test("every panel has rows: three tabs, the first open", () => {
    const html = render(ross(["AAAA"]), rocket(["BBBB"]), detection(["CCCC"]));
    expect(html).toContain('data-side-column="open"');
    for (const s of ["Ross", "Runners", "Signals"]) expect(html).toContain(`>${s}`);
    expect(html).toContain('id="side-panel-ross"');
    expect(html).toContain("Ross Panel");
  });

  test("a tab whose panel is empty, off or not offered is not drawn", () => {
    const html = render(ross(["AAAA"]), rocket([]), detection(["CCCC"]));
    expect(html).not.toContain('id="side-tab-rocket"');
    expect(html).toContain('id="side-tab-signals"');
    const off = render(ross(["AAAA"]), { ...EMPTY_ROCKET, enabled: false }, { ...EMPTY_DETECTION, enabled: false });
    expect(off).not.toContain('id="side-tab-rocket"');
    expect(off).not.toContain('id="side-tab-signals"');
  });

  test("the open tab empties: the first non-empty tab is shown instead", () => {
    // No storage in the test runner: the remembered tab is Ross, and Ross is empty.
    const html = render(ross([]), rocket(["BBBB"]), detection(["CCCC"]));
    expect(html).toContain('id="side-panel-rocket"');
    expect(html).toContain('aria-selected="true"');
    expect(html).toMatch(/id="side-tab-rocket" aria-controls="side-panel-rocket" aria-selected="true"/);
    expect(html).toContain("BBBB");
    expect(html).not.toContain("Ross Panel");
  });

  test("one panel with rows: shown alone, no tab strip", () => {
    const html = render(ross([]), rocket([]), detection(["CCCC"]));
    expect(html).not.toContain('role="tablist"');
    expect(html).not.toContain('role="tabpanel"');
    expect(html).toContain('aria-label="Signals"');
    expect(html).toContain("CCCC");
  });

  test("every panel empty: the column closes (inert, hidden) and draws nothing", () => {
    const html = render(ross([]), rocket([]), detection([]));
    expect(html).toContain('data-side-column="closed"');
    expect(html).toContain('aria-hidden="true"');
    expect(html).toContain("@4xl/panel:w-0");
    expect(html).not.toContain("Ross Panel");
    expect(html).not.toContain("Runners");
    expect(html).not.toContain("Connecting");
    // No channels at all (the fixture feed): closed too.
    expect(render(undefined, undefined, undefined)).toContain('data-side-column="closed"');
  });

  test("first snapshot still loading: the column stays closed instead of growing a tab at a time", () => {
    const html = render(ross(["AAAA"]), { ...EMPTY_ROCKET }, detection(["CCCC"]));
    expect(html).toContain('data-side-column="closed"');
    expect(html).not.toContain("Ross Panel");
  });

  test("reconnecting: last known rows stay up, dimmed; nothing is hidden by the drop", () => {
    const html = render(ross(["AAAA"], { connected: false }), rocket(["BBBB"], { connected: false }), detection(["CCCC"], { connected: false }));
    expect(html).toContain('data-side-column="open"');
    expect(html).toContain('id="side-tab-rocket"');
    expect(html).toContain('id="side-tab-signals"');
    expect(html).toContain("opacity-60");
    // Counts claim attention only from current data.
    expect(html).not.toContain("at the trigger or crossing it");
    const live = render(ross(["AAAA"]), rocket(["BBBB"]), detection(["CCCC"]));
    expect(live).toContain('aria-label="1 at the trigger or crossing it"');
  });

  test("changes animate by real size and stop under reduced motion; no borders, no native controls", () => {
    const html = render(ross(["AAAA"]), rocket(["BBBB"]), detection(["CCCC"]));
    expect(html).toContain("transition-[grid-template-rows,width,margin,opacity]");
    expect(html).toContain("animate-panel-in");
    expect(html).toContain("motion-reduce:transition-none");
    expect(html).not.toMatch(/class="[^"]*\bborder/);
    expect(html).not.toMatch(/<select|<details|<dialog/);
    expect(tabState(html, "ross")).toBe("open");
  });
});

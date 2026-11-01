import { describe, expect, test } from "bun:test";
import { detectionSnapshotView, EMPTY_DETECTION, EMPTY_MOVERS, EMPTY_PRESSURE_STATUS, EMPTY_ROCKET, EMPTY_ROSS, type DetectionChannel, type DetectionCoreEpisode, type DetectionMessage, type DetectionState, type PressureCard, type PressureChannel, type MoverRowWire, type MoversChannel, type MoversState, type PressureStatus, type RocketChannel, type RocketPanelWire, type RocketState, type RossChannel, type RossState, rossDemoPanel } from "@stockwatcher/shared";
import { renderToStaticMarkup } from "react-dom/server";
import { DashboardPanels } from "./DashboardPanels.tsx";

// The dashboard layout (owner, 2026-10-30) and its hide-when-empty rule (owner, 2026-10-22).
// Static renders: effects (the settle timer, the presence timers) do not run, which is the first
// frame a viewer sees.

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
const card = (symbol: string): PressureCard => ({ symbol, price: 3.1, changePct: 24, risk: 70, side: "bull", rise: 40, riseSecs: 30, band: 3.3, toBand: 1.2, pressure: 80, buyShare: 0.8, pace: 3 });
const pressureChannel = (cards: PressureCard[], status: PressureStatus = { connected: true, asOf: 1 }): PressureChannel => ({ get: () => cards, status: () => status, subscribe: () => () => {} });
const mover = (symbol: string, price: number, chg: number, dv: number): MoverRowWire => ({ symbol, price: price * 1_000_000, ref: price * 1_000_000, chg, dv, volume: Math.round(dv / price), trades: 500 });
// The server ranks (crates/server/src/movers.rs); the panels show its lists as they come.
const moversState = (lists: { gainers?: MoverRowWire[]; bull?: MoverRowWire[]; bear?: MoverRowWire[] }): MoversState => {
  const all = [...(lists.gainers ?? []), ...(lists.bull ?? []), ...(lists.bear ?? [])];
  return { gainers: (lists.gainers ?? []).map((r) => r.symbol), bull: (lists.bull ?? []).map((r) => r.symbol), bear: (lists.bear ?? []).map((r) => r.symbol), rows: new Map(all.map((r) => [r.symbol, r])), t: 1, connected: true };
};
const moversChannel = (s: MoversState): MoversChannel => ({ get: () => s, subscribe: () => () => {} });

interface Channels {
  r?: RossState;
  k?: RocketState;
  d?: DetectionState;
  p?: PressureChannel;
  q?: MoversChannel;
}
const render = ({ r, k, d, p, q }: Channels) =>
  renderToStaticMarkup(
    <DashboardPanels ross={r && (channel(r) as RossChannel)} rocket={k && (channel(k) as RocketChannel)} detection={d && (channel(d) as DetectionChannel)} pressure={p} movers={q} onPick={() => {}} onEpisodePick={() => {}}>
      <div data-testid="charts" />
    </DashboardPanels>,
  );
const group = (html: string, g: "side" | "bottom") => html.match(new RegExp(`data-panel-group="${g}" data-open="(open|closed)"`))?.[1];
const slots = (html: string) => [...html.matchAll(/data-slot-panel="(\w+)"/g)].map((m) => m[1]);

// GAIN: +40% on $3M; HOTL: +1% on $9M (a gainer and a bull); DUMP: -20% on $5M (bear).
const GAIN = mover("GAIN", 3, 40, 3_000_000);
const HOTL = mover("HOTL", 10, 1, 9_000_000);
const DUMP = mover("DUMP", 2, -20, 5_000_000);
const movers = moversChannel(moversState({ gainers: [GAIN, HOTL], bull: [HOTL, GAIN], bear: [DUMP] }));
const everything = (): Channels => ({ r: ross(["AAAA"]), k: rocket(["BBBB"]), d: detection(["CCCC"]), p: pressureChannel([card("HALT")]), q: movers });

describe("dashboard layout: right column and bottom row (owner, 2026-10-30)", () => {
  test("every panel with rows: Signals and Top Gainers on the right, the four others along the bottom, in order", () => {
    const html = render(everything());
    expect(group(html, "side")).toBe("open");
    expect(group(html, "bottom")).toBe("open");
    // Favorites has no list in the test store, so it is hidden; the rest keep their fixed order.
    expect(slots(html)).toEqual(["signals", "gainers", "ross", "rocket", "halt", "hot"]);
    expect(html.indexOf('data-testid="charts"')).toBeGreaterThan(-1);
    expect(html.indexOf('data-testid="charts"')).toBeLessThan(html.indexOf('data-panel-group="side"'));
    for (const t of ["Top Gainers", "Hot stocks", "Ross Panel", "Runners", "Halt watcher"]) expect(html).toContain(t);
    // No tabs any more: every panel is on screen at once.
    expect(html).not.toContain('role="tablist"');
    expect(html).not.toContain('role="tab"');
  });

  test("Top Gainers shows the server's list; Hot stocks splits into Bull and Bear, each with its count", () => {
    const html = render(everything());
    const gainers = html.slice(html.indexOf('data-panel="gainers"'), html.indexOf('data-slot-panel="ross"'));
    expect(gainers).toContain("GAIN");
    expect(gainers).toContain("+40.0%");
    expect(gainers).toContain("$3M");
    expect(gainers).not.toContain("DUMP");
    const hot = html.slice(html.indexOf('data-panel="hot"'));
    const bull = hot.slice(hot.indexOf('data-section="bull"'), hot.indexOf('data-section="bear"'));
    const bear = hot.slice(hot.indexOf('data-section="bear"'));
    // Sorted by dollar volume by default: $9M before $3M.
    expect(bull.indexOf("HOTL")).toBeLessThan(bull.indexOf("GAIN"));
    expect(bull).not.toContain("DUMP");
    expect(bear).toContain("DUMP");
    expect(bear).toContain("−20.0%");
    // The sort is a segmented control, never a native select.
    expect(hot).toContain('aria-label="Sort Hot stocks"');
  });

  test("an empty section closes; an empty Hot stocks hides", () => {
    const html = render({ ...everything(), q: moversChannel(moversState({ gainers: [GAIN], bull: [GAIN] })) });
    const hot = html.slice(html.indexOf('data-panel="hot"'));
    expect(hot).toContain('data-reveal="closed"');
    expect(hot).not.toContain('data-section="bear"');
    expect(slots(render({ ...everything(), q: moversChannel(moversState({ gainers: [GAIN] })) }))).not.toContain("hot");
  });

  test("a panel with 0 items is not drawn; the others share the space", () => {
    const html = render({ ...everything(), k: rocket([]), p: pressureChannel([]) });
    expect(slots(html)).toEqual(["signals", "gainers", "ross", "hot"]);
    const off = render({ ...everything(), d: { ...EMPTY_DETECTION, enabled: false } });
    expect(slots(off)).not.toContain("signals");
  });

  test("an odd group's lone last panel spans the pair when stacked; an even one does not", () => {
    const three = render({ ...everything(), p: pressureChannel([]) });
    expect(slots(three)).toEqual(["signals", "gainers", "ross", "rocket", "hot"]);
    expect(three).toMatch(/data-slot-panel="hot"[^>]*class="[^"]*@2xl\/panel:col-span-2/);
    expect(three).not.toMatch(/data-slot-panel="ross"[^>]*class="[^"]*col-span-2/);
    expect(three).not.toMatch(/data-slot-panel="gainers"[^>]*class="[^"]*col-span-2/);
  });

  test("every right-column panel empty: the column closes and the charts take its width", () => {
    const html = render({ ...everything(), d: detection([]), q: moversChannel(moversState({ bull: [HOTL] })) });
    expect(group(html, "side")).toBe("closed");
    expect(html).toContain("--side-w:0px");
    expect(group(html, "bottom")).toBe("open");
    expect(slots(html)).toEqual(["ross", "rocket", "halt", "hot"]);
  });

  test("every bottom panel empty: the row closes", () => {
    const html = render({ d: detection(["CCCC"]), r: ross([]), k: rocket([]), p: pressureChannel([]), q: moversChannel(moversState({})) });
    expect(group(html, "bottom")).toBe("closed");
    expect(html).toContain("--bottom-h:0px");
    expect(html).not.toContain('aria-label="Resize the bottom panels"');
    expect(slots(html)).toEqual(["signals"]);
  });

  test("nothing anywhere (a feed with no channels): both groups closed, only the charts", () => {
    const html = render({});
    expect(group(html, "side")).toBe("closed");
    expect(group(html, "bottom")).toBe("closed");
    expect(slots(html)).toEqual([]);
    expect(html).toContain('data-testid="charts"');
    expect(html).not.toContain("Connecting");
  });

  test("first snapshots still loading: nothing opens a panel at a time", () => {
    // Runners has not said whether it is on; the movers and the Halt watcher have no frame yet.
    expect(slots(render({ ...everything(), k: { ...EMPTY_ROCKET } }))).toEqual([]);
    expect(slots(render({ ...everything(), q: moversChannel(EMPTY_MOVERS) }))).toEqual([]);
    expect(slots(render({ ...everything(), p: pressureChannel([], EMPTY_PRESSURE_STATUS) }))).toEqual([]);
  });

  test("reconnecting: last known rows stay up, dimmed; nothing is hidden by the drop", () => {
    const html = render({ ...everything(), r: ross(["AAAA"], { connected: false }), k: rocket(["BBBB"], { connected: false }), d: detection(["CCCC"], { connected: false }), p: pressureChannel([card("HALT")], { connected: false, asOf: 1 }) });
    expect(slots(html)).toEqual(["signals", "gainers", "ross", "rocket", "halt", "hot"]);
    expect(html).toContain("opacity-60");
    expect(html).toContain("Reconnecting");
  });

  test("changes animate by real size and stop under reduced motion; no borders, no native controls", () => {
    const html = render(everything());
    expect(html).toContain("transition-[grid-template-columns,grid-template-rows]");
    expect(html).toContain("transition-[grid-template-rows,flex-grow,opacity,padding]");
    expect(html).toContain("starting:grid-rows-[0fr]");
    expect(html).toContain("motion-reduce:transition-none");
    expect(html).not.toMatch(/class="[^"]*\bborder/);
    expect(html).not.toMatch(/<select|<details|<dialog/);
    // The bottom row's top edge resizes it, reachable by keyboard.
    expect(html).toMatch(/role="separator"[^>]*aria-orientation="horizontal"[^>]*tabindex="0"/);
    // The right column's left edge resizes its width (280–640px), and the boundary between two
    // of its panels their split; the first panel has no boundary above it.
    expect(html).toMatch(/role="separator" aria-orientation="vertical" aria-label="Resize the right column" aria-valuenow="400" aria-valuemin="280" aria-valuemax="640"/);
    expect(html).toContain('aria-label="Resize Top Gainers against the panel above"');
    expect(html).not.toContain('aria-label="Resize Signals against the panel above"');
  });
});

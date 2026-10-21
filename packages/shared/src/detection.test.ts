import { describe, expect, test } from "bun:test";
import { createDetectionChannel, detectionBlocked, detectionMatches, detectionOverlays, detectionPrice, detectionSnapshotView, groupDetections, type DetectionCoreEpisode, type DetectionMessage } from "./detection.ts";
import { parseServerMessage } from "./protocol.ts";
import { createWsClient, type WebSocketLike } from "./ws.ts";

const episode = (id = "fixture/MPXA/micro/1", symbol = "MPXA"): DetectionCoreEpisode => ({ episode_id: id, family: "micro_pullback", variant: "micro_pullback_v0", policy: "detection-v0@test", symbol, instrument: symbol, trading_date: "2026-10-16", segment: "2026-10-16/regular", state: "armed", default_visible: true, first_detected_at: 100, first_bar_end: 98, last_material_seq: 1, last_material_at: 100, base_secs: 15, fidelity: "Synthetic", warmup: false, parent_episode_id: null, related_episode_ids: [], context: null, levels: [{ level_id: `${id}:trigger`, kind: "trigger", version: 1, price: 3_400_000, known_at: 100, armed_at: 100, formed_from: [50, 98], reason: "impulse_high" }, { level_id: `${id}:invalidation`, kind: "invalidation", version: 1, price: 3_320_000, known_at: 100, armed_at: 100, formed_from: [50, 98], reason: "pullback_low" }], anchors: { pullback_low: 3_320_000 }, recognition: { bar_end: 98, cutoff: 100, range_expansion_15pct: "PASS", body_expansion_15pct: "FAIL" }, checks: [{ name: "depth", state: "PASS", value: 200, threshold: 500, units: "thousandths", reason: "within_limit" }], current: { price: 3_340_000, price_exec: 101, price_age_ms: 25, stale: false }, resolution: null });
const frame = (cursor = 1): DetectionMessage => ({ type: "detection.snapshot", enabled: true, source: "fixture", runEpoch: "fixture:1", cursor, snapshot: { schema: 1, run_id: "fixture", source: "fixture", fidelity: "Synthetic", policy: "detection-v0@test", experimental: true, alerts_enabled: false, cursor: 1, bar_end: 98, cutoff: 100, as_of: 102, symbols: [{ symbol: "MPXA", dominant_episode_id: "fixture/MPXA/micro/1", active_episode_ids: ["fixture/MPXA/micro/1"], interrupted: null }], active: [episode()], recent: [], data_status: { global_interruption: null, disabled_reason: null }, telemetry: { transitions_dropped: 0 } }, transitions: [], history: { capacity: 2048, dropped: 0, first_sequence: null, last_sequence: null }, telemetry: { engine: { transitions_dropped: 0, recent_dropped: 0 }, journal_queue_dropped: 0, journal_write_errors: 0 }, profile: { price_min_micros: 100_000, price_max_micros: 25_000_000, research_min_micros: 150_000, research_max_micros: 25_000_000 }, experimental: true, alertsEnabled: false });

describe("raw Rust -> display projection", () => {
  test("keeps body/range observations, nullable context and each tested level identity distinct", () => {
    const input = frame();
    const view = detectionSnapshotView(input)!;
    const e = view.episodes[0]!;
    expect(e.evidence.find((x) => x.name === "recognition.range_expansion_15pct")?.value).toBe("PASS");
    expect(e.evidence.find((x) => x.name === "recognition.body_expansion_15pct")?.value).toBe("FAIL");
    expect(e.evidence.find((x) => x.name === "anchors.pullback_low")?.units).toBe("micros");
    expect(e.price_at).toBe(101);
    expect(e.price_age_ms).toBe(25);
    expect(e.ross_member).toBeNull();
    expect(e.level?.invalidation_level_id).toBe("fixture/MPXA/micro/1:invalidation");
    expect(e.raw).toBe(input.snapshot!.active[0]!);
    expect(detectionOverlays(view, "MPXA", "missing")).toEqual([]);
    expect(detectionOverlays(view, "MPXA")[0]!.levels[0]!.knownAt).toBe(100 / 1_000_000);
  });

  test("uses core dominant episode but orders instrument rows by material events, never price", () => {
    const f = frame();
    const ignition = { ...episode("fixture/MPXA/ignition/1"), family: "ignition", state: "recognized", last_material_at: 200 };
    const hidden = { ...episode("fixture/MPXA/acceleration/1"), family: "upward_acceleration", default_visible: false, last_material_at: 300 };
    f.snapshot!.active.push(ignition, hidden, episode("fixture/OTHER/micro/1", "OTHER"));
    const v = detectionSnapshotView(f)!;
    expect(groupDetections(v)[0]!.selected.episode_id).toBe("fixture/MPXA/micro/1");
    expect(groupDetections(v)[0]!.lastMaterialAt).toBe(200);
    expect(groupDetections(v)[0]!.episodes).toHaveLength(2);
    expect(groupDetections(v, "momentum")[0]!.selected.family).toBe("upward_acceleration");
    v.episodes[0]!.price = 24_000_000;
    expect(groupDetections(v)[0]!.selected.episode_id).toBe("fixture/MPXA/micro/1");
  });

  test("global disabled/interruption reasons survive even with zero symbols and detections", () => {
    const f = frame(); f.snapshot!.active = []; f.snapshot!.symbols = [];
    f.snapshot!.data_status = { disabled_reason: "bar_secs_mismatch", global_interruption: { from: 100, to: null, reason: "link_down" } };
    expect(detectionSnapshotView(f)!.unavailable.map((x) => x.reason)).toEqual(["link_down", "bar_secs_mismatch"]);
  });

  test("uses authoritative monitoring, interruption, requirement and proximity metadata", () => {
    const f = frame();
    f.snapshot!.monitored = { symbols_seen: 7, symbols_today: 5, symbols_active: 1, symbols_interrupted: 1 };
    f.snapshot!.data_status.interrupted_symbols = [{ symbol: "OTHER", reason: "symbol_gap" }];
    f.snapshot!.active[0]!.checks.push({ name: "volume_condition", state: "UNKNOWN", value: null, threshold: null, units: "milli", reason: "measured_not_required", required: false });
    f.snapshot!.active[0]!.current!.near_trigger = false;
    let view = detectionSnapshotView(f)!;
    expect(view.monitored_symbols).toBe(5);
    expect(view.unavailable).toContainEqual({ reason: "symbol_gap", symbols: 1 });
    expect(detectionBlocked(view.episodes[0]!)).toEqual([]);
    expect(detectionMatches(view.episodes[0]!, "near")).toBe(false);
    f.snapshot!.active[0]!.current!.near_trigger = true;
    view = detectionSnapshotView(f)!;
    expect(detectionMatches(view.episodes[0]!, "near")).toBe(true);
    expect(detectionPrice(3_400_001)).toBe("$3.400001");
    expect(detectionPrice(125_001)).toBe("$0.125001");
  });

  test("certainty survives restored replay history and recognized ignition is not a trigger", () => {
    const f = frame(); const raw = f.snapshot!.active[0]!;
    f.transitions.push({ sequence: 1, emitted_at: 105, input_watermark: 105, restored: true, transition: { seq: 1, transition_id: "t1", episode_id: raw.episode_id, symbol: raw.symbol, family: raw.family, variant: raw.variant, from: "armed", to: "crossing_provisional", event: "crossing_provisional", reason: "post_arm_print_above_trigger", certainty: "provisional", observed_at: 104, bar_end: null, cutoff: null, trade: null, features: null, level: raw.levels[0]!, anchors: null, correction_of: null, warmup: false, default_visible: true } });
    const e = detectionSnapshotView(f)!.episodes[0]!;
    expect(e.status).toBe("provisional"); expect(e.restored).toBe(true);
    expect(e.timeline[0]!.warmup).toBe(false);
    expect(detectionMatches({ ...e, family: "ignition", state: "recognized" }, "triggered")).toBe(false);
    expect(detectionMatches({ ...e, family: "range_breakout", state: "confirmed" }, "triggered")).toBe(false);
    expect(detectionMatches({ ...e, state: "trigger_observed" }, "triggered")).toBe(true);
  });
});

describe("snapshot cursor, freshness and source isolation", () => {
  test("reconnect remains last-known until a frame; retired epochs and duplicate updates cannot replace it", () => {
    let subscriptions = 0;
    const c = createDetectionChannel({ source: "replay", subscribe: () => subscriptions++, unsubscribe() {} });
    c.apply(frame()); expect(c.get().connected).toBe(true);
    c.setConnected(false); c.setConnected(true); expect(c.get().connected).toBe(false);
    c.apply(frame()); expect(c.get().connected).toBe(true);
    const next = { ...frame(), runEpoch: "fixture:2" }; c.apply(next);
    c.apply({ ...frame(99), type: "detection.update" });
    expect(c.get().frame!.runEpoch).toBe("fixture:2");
    c.apply({ ...next, type: "detection.update", cursor: 0 });
    expect(c.get().frame!.cursor).toBe(1);
    expect(subscriptions).toBe(0);
  });

  test("fixture is admissible on replay, rejected on live; unsupported schema requests explicit resync", () => {
    let n = 0;
    const c = createDetectionChannel({ source: "live", subscribe: () => n++, unsubscribe() {} });
    c.apply(frame()); expect(c.get().error).toBe("source_mismatch"); expect(c.get().snapshot).toBeNull();
    const p = createDetectionChannel({ source: "replay", subscribe: () => n++, unsubscribe() {} });
    const f = frame(); f.snapshot!.schema = 9; p.apply(f);
    expect(p.get().error).toBe("unsupported_schema"); expect(n).toBe(2);
  });

  test("nested real drops are disclosed; normal terminal-history retention is separate", () => {
    const c = createDetectionChannel({ subscribe() {}, unsubscribe() {} });
    const f = frame(); f.history.dropped = 100; f.telemetry = { engine: { transitions_dropped: 0, recent_dropped: 100 } };
    c.apply(f); expect(c.get().coverageLost).toBe(false);
    c.apply({ ...f, cursor: 2, telemetry: { engine: { transitions_dropped: 1 } } }); expect(c.get().coverageLost).toBe(true);
  });

  test("a missing significant-history prefix requests resync once instead of losing it silently", () => {
    let requests = 0;
    const c = createDetectionChannel({ subscribe: () => requests++, unsubscribe() {} });
    const f = frame(); f.history.last_sequence = 2; c.apply(f);
    const update = { ...frame(2), type: "detection.update" as const, history: { capacity: 2048, dropped: 3, first_sequence: 6, last_sequence: 7 } };
    c.apply(update); c.apply(update);
    expect(c.get().coverageLost).toBe(true); expect(requests).toBe(1);
    expect(c.get().snapshot!.episodes).toHaveLength(1);
    c.apply({ ...update, type: "detection.snapshot" });
    c.apply({ ...update, cursor: 3 });
    expect(c.get().coverageLost).toBe(true);
    c.apply({ ...frame(), runEpoch: "fixture:next" });
    expect(c.get().coverageLost).toBe(false);
  });
});

test("parser rejects incomplete envelopes and preserves a complete raw snapshot", () => {
  expect(parseServerMessage('{"type":"detection.snapshot"}')).toBeNull();
  expect(parseServerMessage(JSON.stringify(frame()))).toEqual(frame());
  const bad = frame(); (bad.snapshot!.active[0] as unknown as Record<string, unknown>).levels = null;
  expect(parseServerMessage(JSON.stringify(bad))).toBeNull();
  for (const field of ["segment", "related_episode_ids", "resolution", "checks", "current"]) {
    const incomplete = frame(); delete (incomplete.snapshot!.active[0] as unknown as Record<string, unknown>)[field];
    expect(parseServerMessage(JSON.stringify(incomplete))).toBeNull();
  }
});

class Socket implements WebSocketLike {
  readyState = 0; sent: string[] = [];
  onopen: WebSocketLike["onopen"] = null; onmessage: WebSocketLike["onmessage"] = null; onclose: WebSocketLike["onclose"] = null; onerror: WebSocketLike["onerror"] = null;
  send(s: string) { this.sent.push(s); } close() { this.readyState = 3; this.onclose?.({}); }
  open() { this.readyState = 1; this.onopen?.({}); }
  receive(x: unknown) { this.onmessage?.({ data: JSON.stringify(x) }); }
}
test("real WS client resubscribes detection and ignores a retired socket's queued callbacks", async () => {
  const sockets: Socket[] = []; const seen: string[] = [];
  const client = createWsClient({ url: "ws://fixture", source: "replay", baseDelayMs: 1, random: () => 0, createSocket: () => { const s = new Socket(); sockets.push(s); return s; }, onDetection: (m) => seen.push(m.runEpoch) });
  client.subscribeDetection();
  const welcome = { type: "welcome", version: "test", commit: "test", heartbeatSecs: 15 };
  sockets[0]!.open(); sockets[0]!.receive(welcome); sockets[0]!.receive(frame()); sockets[0]!.close();
  await new Promise((r) => setTimeout(r, 10));
  sockets[1]!.open(); sockets[1]!.receive(welcome); sockets[1]!.receive({ ...frame(), runEpoch: "fixture:2" });
  sockets[0]!.receive({ ...frame(), runEpoch: "retired-wrong-epoch" }); sockets[0]!.close();
  await new Promise((r) => setTimeout(r, 5));
  expect(seen).toEqual(["fixture:1", "fixture:2"]);
  expect(sockets).toHaveLength(2);
  expect(sockets[1]!.sent.some((x) => x === '{"type":"detection.subscribe"}')).toBe(true);
  client.close();
});

test("malformed detection delivery is explicit and requests one repair snapshot", () => {
  let repairs = 0;
  const channel = createDetectionChannel({ subscribe() {}, unsubscribe() {}, resync: () => repairs++ });
  const socket = new Socket();
  const client = createWsClient({ url: "ws://fixture", createSocket: () => socket, onDetection: channel.apply, onDetectionError: channel.reject });
  socket.open(); socket.receive(frame());
  socket.receive({ type: "detection.update", enabled: true });
  socket.receive({ type: "detection.update", enabled: true });
  expect(channel.get().error).toBe("invalid_detection_frame");
  expect(channel.get().resyncing).toBe(true); expect(channel.get().connected).toBe(false);
  expect(channel.get().snapshot!.episodes).toHaveLength(1); expect(repairs).toBe(1);
  socket.receive(frame()); expect(channel.get().error).toBeNull();
  client.close();
});

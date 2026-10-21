// The Detection Panel's presentation/transport contract. Rust owns detection, required checks,
// levels and transitions. This module only reconciles full frames and formats their evidence.
import type { MarketSource } from "./protocol.ts";

export type DetectionCheck = "PASS" | "FAIL" | "UNKNOWN";
export interface DetectionEvidence {
  name: string;
  value: number | string | boolean | null;
  units: string;
  state: DetectionCheck | null;
  required: boolean;
  rule: string | null;
  reason: string | null;
  source: string;
  as_of: number;
  observed_at: number;
}
export interface DetectionLevel {
  level_id: string;
  version: number;
  trigger: number;
  invalidation: number | null;
  known_at: number;
  armed_at: number | null;
  valid_to: number | null;
  formed_from: string[];
  kind: "trigger" | "range_high";
  invalidation_level_id: string | null;
  invalidation_version: number | null;
}
export interface DetectionTransition {
  transition_id: string;
  episode_id: string;
  sequence: number;
  previous_state: string | null;
  state: string;
  event: string;
  reason: string;
  observed_at: number;
  input_cutoff: number;
  bar_end: number | null;
  input_sequence: number | null;
  status: "provisional" | "confirmed" | "retracted" | "restored" | "terminal" | "correction" | "unknown";
  tested_level_id: string | null;
  tested_level_version: number | null;
  supersedes: string | null;
}
export interface DetectionEpisode {
  episode_id: string;
  instrument_id: string;
  symbol: string;
  run_id: string;
  source: string;
  fidelity: string;
  trading_date: string;
  segment: string;
  family: string;
  variant: string;
  policy_id: string;
  policy_hash: string;
  parent_episode_id: string | null;
  related_episode_ids: string[];
  first_detected_at: number;
  last_material_transition_at: number;
  input_cutoff: number;
  bar_end: number | null;
  state: string;
  status: DetectionTransition["status"];
  terminal: boolean;
  resolved_at: number | null;
  reason: string | null;
  price: number | null;
  price_at: number | null;
  level: DetectionLevel | null;
  level_history: DetectionLevel[];
  evidence: DetectionEvidence[];
  timeline: DetectionTransition[];
  coverage: string;
  default_visible: boolean;
  price_age_ms: number | null;
  price_stale: boolean;
  /** Original engine record, retained for full anchors/features/level provenance in details. */
  raw: DetectionCoreEpisode;
  /** Context badges are evidence, never detector admission gates or confidence. */
  ross_member: boolean | null;
  catalyst: "qualifying" | "none" | "unknown";
}
export interface DetectionSnapshot {
  schema: 1;
  run_id: string;
  source: string;
  fidelity: string;
  policy_id: string;
  policy_hash: string;
  experimental: true;
  base_timeframe_secs: number;
  cutoff: number;
  as_of: number;
  price_min_micros: number;
  price_max_micros: number;
  research_min_micros: number;
  research_max_micros: number;
  episodes: DetectionEpisode[];
  monitored_symbols: number;
  unavailable: { reason: string; symbols: number }[];
  dominant_episode_ids: Record<string, string | null>;
}
/** Direct Rust serde records. Keep the semantic record intact on the wire and in exports;
 * the normalized types above are DISPLAY views, not a second detection protocol. */
export interface DetectionCoreLevel { level_id: string; kind: "trigger" | "invalidation" | "range_high"; version: number; price: number; known_at: number; armed_at: number | null; formed_from: [number, number]; reason: string }
export interface DetectionCoreCheck { name: string; state: DetectionCheck; value: number | null; threshold: number | null; units: string; reason: string; required?: boolean }
export interface DetectionCoreFeatures { bar_end: number; cutoff: number; [name: string]: unknown }
export interface DetectionCoreTransition {
  seq: number; transition_id: string; episode_id: string; symbol: string; family: string; variant: string;
  from: string | null; to: string; event: string; reason: string; certainty: string; observed_at: number;
  bar_end: number | null; cutoff: number | null; trade: { trade_id: string; exec: number; admit: number; price: number } | null;
  features: DetectionCoreFeatures | null; level: DetectionCoreLevel | null; anchors: Record<string, unknown> | null;
  correction_of: string | null; warmup: boolean; default_visible: boolean;
}
export interface DetectionCoreEpisode {
  episode_id: string; family: string; variant: string; policy: string; symbol: string; instrument: string;
  trading_date: string; segment: string; state: string; default_visible: boolean; first_detected_at: number;
  first_bar_end: number | null; last_material_seq: number; last_material_at: number; base_secs: number;
  fidelity: string; warmup: boolean; parent_episode_id: string | null; related_episode_ids: string[];
  context: string | null; levels: DetectionCoreLevel[]; anchors: Record<string, unknown> | null;
  recognition: DetectionCoreFeatures | null; checks: DetectionCoreCheck[];
  current: { price: number | null; price_exec: number | null; price_age_ms: number | null; stale: boolean; [key: string]: unknown } | null;
  resolution: { state: string; reason: string; at: number; bar_end: number | null; transition_id: string } | null;
}
export interface DetectionCoreSnapshot {
  schema: number; run_id: string; source: string; fidelity: string; policy: string; experimental: true; alerts_enabled: false;
  cursor: number; bar_end: number | null; cutoff: number | null; as_of: number | null;
  symbols: { symbol: string; dominant_episode_id: string | null; active_episode_ids: string[]; interrupted: string | null }[];
  active: DetectionCoreEpisode[]; recent: DetectionCoreEpisode[];
  data_status: Record<string, unknown>; telemetry: Record<string, number | string | boolean | null>;
}
export interface DetectionRecordedTransition {
  sequence: number;
  emitted_at: number;
  input_watermark: number;
  restored: boolean;
  transition: DetectionCoreTransition;
}
export interface DetectionMessage {
  type: "detection.snapshot" | "detection.update";
  enabled: boolean;
  source: MarketSource | "fixture";
  runEpoch: string;
  cursor: number;
  snapshot: DetectionCoreSnapshot | null;
  transitions: DetectionRecordedTransition[];
  history: { capacity: number; dropped: number; first_sequence: number | null; last_sequence: number | null };
  telemetry: Record<string, unknown>;
  profile: { price_min_micros: number; price_max_micros: number; research_min_micros: number; research_max_micros: number };
  policyHash?: string | null;
  context?: Record<string, { ross_member: boolean | null; catalyst: DetectionEpisode["catalyst"] }>;
  experimental: true;
  alertsEnabled: false;
}
export interface DetectionState {
  enabled: boolean | null;
  connected: boolean;
  snapshot: DetectionSnapshot | null;
  frame: DetectionMessage | null;
  resyncing: boolean;
  coverageLost: boolean;
  error: string | null;
}
export const EMPTY_DETECTION: DetectionState = { enabled: null, connected: false, snapshot: null, frame: null, resyncing: false, coverageLost: false, error: null };
export interface DetectionChannel {
  get(): DetectionState;
  subscribe(fn: () => void): () => void;
}

const object = (x: unknown): x is Record<string, unknown> => typeof x === "object" && x !== null && !Array.isArray(x);
const finite = (x: unknown): x is number => typeof x === "number" && Number.isFinite(x);
/** Validate the fields the presentation adapter traverses. An unsupported schema is kept as a
 * recognized envelope so the channel can show its explicit resync/error state. */
export function isDetectionMessage(x: unknown): x is DetectionMessage {
  if (!object(x) || !["detection.snapshot", "detection.update"].includes(String(x.type)) || typeof x.enabled !== "boolean" || !["live", "replay", "fixture"].includes(String(x.source)) || typeof x.runEpoch !== "string" || !finite(x.cursor) || !object(x.history) || !Array.isArray(x.transitions) || !object(x.telemetry) || x.experimental !== true || x.alertsEnabled !== false) return false;
  if (!finite(x.history.dropped) || !finite(x.history.capacity)) return false;
  const transitionsValid = x.transitions.every((r) => object(r) && finite(r.sequence) && finite(r.emitted_at) && finite(r.input_watermark) && object(r.transition) && typeof r.transition.transition_id === "string" && typeof r.transition.episode_id === "string" && finite(r.transition.seq) && finite(r.transition.observed_at));
  if (!transitionsValid) return false;
  if (!x.enabled && x.snapshot === null) return true;
  const s = x.snapshot;
  if (!object(s) || !finite(s.schema)) return false;
  if (s.schema !== 1) return true;
  if (!object(x.profile) || !["price_min_micros", "price_max_micros", "research_min_micros", "research_max_micros"].every((k) => finite(x.profile && (x.profile as Record<string, unknown>)[k]))) return false;
  if (typeof s.policy !== "string" || !Array.isArray(s.active) || !Array.isArray(s.recent) || !Array.isArray(s.symbols) || !object(s.data_status)) return false;
  return [...s.active, ...s.recent].every((e) => object(e) && typeof e.episode_id === "string" && typeof e.symbol === "string" && typeof e.family === "string" && typeof e.state === "string" && finite(e.first_detected_at) && finite(e.last_material_at) && Array.isArray(e.checks) && e.checks.every((c) => object(c) && typeof c.name === "string" && ["PASS", "FAIL", "UNKNOWN"].includes(String(c.state))) && Array.isArray(e.levels) && e.levels.every((l) => object(l) && typeof l.level_id === "string" && finite(l.version) && finite(l.price) && finite(l.known_at) && Array.isArray(l.formed_from)));
}

function capturedLoss(telemetry: Record<string, unknown>): boolean {
  const engine = object(telemetry.engine) ? telemetry.engine : telemetry;
  // Recent/history retention is a bounded view, not an ingestion or persistence failure.
  return [engine.transitions_dropped, telemetry.journal_queue_dropped, telemetry.journal_write_errors].some((n) => finite(n) && n > 0);
}

/** Each update is a full replacement. Cursor jumps are therefore valid coalescing; a missing
 * significant transition is detected from its separate history sequence, never guessed from
 * the price-frame cadence. A reconnect does not replay notifications (there are none here). */
export function createDetectionChannel(io: { subscribe(): void; unsubscribe(): void; resync?(): void; source?: MarketSource }): DetectionChannel & { apply(m: DetectionMessage): void; setConnected(open: boolean): void } {
  let state = EMPTY_DETECTION;
  const listeners = new Set<() => void>();
  const retired: string[] = [];
  let requested: string | null = null;
  const emit = () => { for (const fn of listeners) fn(); };
  const resync = (m: DetectionMessage, reason: string) => {
    state = { ...state, connected: false, resyncing: true, error: reason };
    const key = `${m.runEpoch}/${reason}`;
    if (requested !== key) { requested = key; (io.resync ?? io.subscribe)(); }
    emit();
  };
  return {
    get: () => state,
    subscribe(fn) {
      listeners.add(fn);
      if (listeners.size === 1) io.subscribe();
      return () => { if (listeners.delete(fn) && !listeners.size) io.unsubscribe(); };
    },
    apply(m) {
      if (io.source && m.source !== io.source && !(io.source === "replay" && m.source === "fixture")) { resync(m, "source_mismatch"); return; }
      if (m.snapshot && m.snapshot.schema !== 1) { resync(m, "unsupported_schema"); return; }
      if (retired.includes(m.runEpoch)) return;
      const previous = state.frame;
      if ((!previous || previous.runEpoch !== m.runEpoch) && m.type !== "detection.snapshot") { resync(m, "snapshot_required"); return; }
      if (previous?.runEpoch === m.runEpoch && m.cursor < previous.cursor) return;
      if (previous?.runEpoch === m.runEpoch && m.cursor === previous.cursor && m.type !== "detection.snapshot") return;
      if (previous && previous.runEpoch !== m.runEpoch) {
        retired.push(previous.runEpoch);
        if (retired.length > 16) retired.shift();
      }
      const last = previous?.runEpoch === m.runEpoch ? previous.history.last_sequence : null;
      const historyGap = last !== null && last !== undefined && m.history.first_sequence !== null && m.history.first_sequence > last + 1;
      const coverageLost = historyGap || capturedLoss(m.telemetry);
      state = { enabled: m.enabled, connected: true, snapshot: m.enabled ? detectionSnapshotView(m) : null, frame: m, resyncing: false, coverageLost, error: coverageLost ? "transition_history_incomplete" : null };
      requested = null;
      emit();
      // Full state is usable, but a missed transition is still disclosed and a current snapshot
      // requested once. Repeating an identical snapshot must not create a resync loop.
      if (historyGap && m.type !== "detection.snapshot") resync(m, "transition_history_incomplete");
    },
    setConnected(open) {
      if (open || !state.connected) return;
      state = { ...state, connected: false };
      emit();
    },
  };
}

const certainty = (raw: string, warmup: boolean): DetectionTransition["status"] => warmup ? "restored" : ["provisional", "confirmed", "retracted", "terminal", "correction"].includes(raw) ? raw as DetectionTransition["status"] : "unknown";
const transitionView = (r: DetectionRecordedTransition): DetectionTransition => {
  const t = r.transition;
  return { transition_id: t.transition_id, episode_id: t.episode_id, sequence: t.seq, previous_state: t.from, state: t.to, event: t.event, reason: t.reason, observed_at: t.observed_at, input_cutoff: t.cutoff ?? t.observed_at, bar_end: t.bar_end, input_sequence: r.input_watermark, status: certainty(t.certainty, r.restored || t.warmup), tested_level_id: t.level?.level_id ?? null, tested_level_version: t.level?.version ?? null, supersedes: t.correction_of };
};

/** Alias an engine level into one display pair. Both original identities/versions remain
 * attached; this does not invent or revise a structural reference. v0 never revises after arm. */
function levelViews(e: DetectionCoreEpisode): DetectionLevel[] {
  const triggers = e.levels.filter((l) => l.kind === "trigger" || l.kind === "range_high").slice().sort((a, b) => a.known_at - b.known_at || a.version - b.version);
  return triggers.map((t, index) => {
    const invalidation = e.levels.filter((l) => l.kind === "invalidation" && l.known_at <= t.known_at && l.armed_at === t.armed_at).sort((a, b) => b.known_at - a.known_at || b.version - a.version)[0];
    return { level_id: t.level_id, version: t.version, kind: t.kind as DetectionLevel["kind"], trigger: t.price, invalidation: invalidation?.price ?? null, invalidation_level_id: invalidation?.level_id ?? null, invalidation_version: invalidation?.version ?? null, known_at: t.known_at, armed_at: t.armed_at, valid_to: triggers[index + 1]?.known_at ?? e.resolution?.at ?? null, formed_from: t.formed_from.map((x) => `bar_end:${x}`) };
  });
}

function featureEvidence(value: unknown, prefix: string, at: number, source: string, out: DetectionEvidence[], depth = 0): void {
  if (depth > 5 || out.length >= 256 || value === undefined) return;
  if (value !== null && typeof value === "object") {
    for (const [key, child] of Object.entries(value)) featureEvidence(child, prefix ? `${prefix}.${key}` : key, at, source, out, depth + 1);
    return;
  }
  if (typeof value !== "number" && typeof value !== "string" && typeof value !== "boolean" && value !== null) return;
  const leaf = prefix.split(".").at(-1) ?? prefix;
  const units = leaf.endsWith("_milli") ? "thousandths" : leaf.endsWith("_ppm") ? "ppm" : leaf.endsWith("_bp") ? "bp" : leaf.endsWith("_ms") ? "ms" : leaf.endsWith("_secs") ? "s" : ["open", "high", "low", "close", "c_prev", "price", "l0", "h0", "lp", "pullback_low", "increment"].includes(leaf) ? "micros" : /(_at|_exec|_end|_start)$/.test(leaf) || ["cutoff", "known_at", "armed_at"].includes(leaf) ? "UTC ns" : leaf === "dollar_volume" ? "USD" : leaf === "volume" ? "shares" : leaf === "trades" ? "trades" : "";
  out.push({ name: prefix, value, units, state: null, required: false, rule: null, reason: value === null ? "feature_unavailable" : null, source, as_of: at, observed_at: at });
}

/** Presentation-only projection of the untouched raw Rust snapshot. Measurements are copied,
 * never re-evaluated from client candles. In particular confidence and buyer pressure are absent. */
export function detectionSnapshotView(m: DetectionMessage): DetectionSnapshot | null {
  const s = m.snapshot;
  if (!s) return null;
  const [policyId, shortHash = ""] = s.policy.split("@");
  const timeline = m.transitions.map(transitionView);
  const episodes = [...s.active, ...s.recent].map((e): DetectionEpisode => {
    const levels = levelViews(e);
    const history = timeline.filter((t) => t.episode_id === e.episode_id);
    const last = history.at(-1);
    const at = s.cutoff ?? e.last_material_at;
    const evidence: DetectionEvidence[] = e.checks.map((c) => ({ name: c.name, value: c.value, units: c.units, state: c.state, required: c.required ?? true, rule: c.threshold === null ? null : String(c.threshold), reason: c.reason, source: "engine rule", as_of: at, observed_at: at }));
    featureEvidence(e.recognition, "recognition", e.recognition?.cutoff ?? at, "engine feature snapshot", evidence);
    featureEvidence(e.anchors, "anchors", e.first_detected_at, "engine causal anchor", evidence);
    evidence.push({ name: "quiet_context", value: e.context, units: "", state: null, required: false, rule: null, reason: null, source: "engine context", as_of: at, observed_at: at });
    const context = m.context?.[e.symbol];
    return { episode_id: e.episode_id, instrument_id: e.instrument, symbol: e.symbol, run_id: s.run_id, source: m.source, fidelity: e.fidelity, trading_date: e.trading_date, segment: e.segment, family: e.family, variant: e.variant, policy_id: policyId ?? s.policy, policy_hash: m.policyHash ?? shortHash, parent_episode_id: e.parent_episode_id, related_episode_ids: e.related_episode_ids, first_detected_at: e.first_detected_at, last_material_transition_at: e.last_material_at, input_cutoff: e.recognition?.cutoff ?? at, bar_end: e.first_bar_end, state: e.state, status: last?.status ?? (e.warmup ? "restored" : "unknown"), terminal: e.resolution !== null, resolved_at: e.resolution?.at ?? null, reason: e.resolution?.reason ?? null, price: e.current?.price ?? null, price_at: e.current?.price_exec ?? null, price_age_ms: e.current?.price_age_ms ?? null, price_stale: e.current?.stale ?? true, level: levels.at(-1) ?? null, level_history: levels, evidence, timeline: history, coverage: e.warmup ? "restored/reconstructed — forward coverage unverified" : "collection completeness unverified", ross_member: context?.ross_member ?? null, catalyst: context?.catalyst ?? "unknown", default_visible: e.default_visible, raw: e };
  });
  const unavailable = new Map<string, number>();
  for (const symbol of s.symbols) if (symbol.interrupted) unavailable.set(symbol.interrupted, (unavailable.get(symbol.interrupted) ?? 0) + 1);
  const global = s.data_status.global_interruption;
  if (object(global)) unavailable.set(typeof global.reason === "string" ? global.reason : "global_interruption", s.symbols.length);
  if (typeof s.data_status.disabled_reason === "string") unavailable.set(s.data_status.disabled_reason, s.symbols.length);
  return { schema: 1, run_id: s.run_id, source: m.source, fidelity: s.fidelity, policy_id: policyId ?? s.policy, policy_hash: m.policyHash ?? shortHash, experimental: true, base_timeframe_secs: 15, cutoff: s.cutoff ?? 0, as_of: s.as_of ?? s.cutoff ?? 0, ...m.profile, episodes, monitored_symbols: s.symbols.length, unavailable: [...unavailable].map(([reason, symbols]) => ({ reason, symbols })), dominant_episode_ids: Object.fromEntries(s.symbols.map((x) => [x.symbol, x.dominant_episode_id])) };
}

/** Structurally matches chart-core DetectionOverlay, without making shared depend on a chart.
 * The caller supplies its symbol and optional exact episode selection; a missing ID yields no
 * substitute annotation. ns/micros -> ms/dollars is the only transformation. */
export function detectionOverlays(snapshot: DetectionSnapshot | null, symbol: string, episodeId?: string) {
  const rows = snapshot?.episodes.filter((e) => e.symbol === symbol) ?? [];
  const preferred = episodeId ?? snapshot?.dominant_episode_ids[symbol];
  const e = preferred ? rows.find((x) => x.episode_id === preferred) : rows.filter((x) => !x.terminal && x.default_visible).sort((a, b) => b.last_material_transition_at - a.last_material_transition_at || a.episode_id.localeCompare(b.episode_id))[0];
  if (!e) return [];
  return [{ episodeId: e.episode_id, label: `${detectionFamilyLabel(e.family)} · ${detectionStateLabel(e.state)}`, recognizedAt: e.first_detected_at / 1_000_000, terminal: e.terminal, levels: e.level_history.map((l) => ({ id: l.level_id, version: l.version, knownAt: l.known_at / 1_000_000, armedAt: l.armed_at === null ? null : l.armed_at / 1_000_000, validTo: l.valid_to === null ? null : l.valid_to / 1_000_000, trigger: l.trigger / 1_000_000, invalidation: l.invalidation === null ? null : l.invalidation / 1_000_000 })) }];
}

export type DetectionFilter = "all" | "momentum" | "ignition" | "micro" | "near" | "triggered" | "ross" | "catalyst";
export const DETECTION_FILTERS: readonly { id: DetectionFilter; label: string }[] = [
  { id: "all", label: "All" }, { id: "momentum", label: "Momentum building" }, { id: "ignition", label: "Ignition / expansion" }, { id: "micro", label: "Micro-pullbacks" }, { id: "near", label: "Approaching / crossing" }, { id: "triggered", label: "Trigger observed" }, { id: "ross", label: "Ross members" }, { id: "catalyst", label: "Catalyst evidence" },
];
const FAMILY: Record<string, string> = { momentum_building: "Momentum building", upward_acceleration: "Upward acceleration", ignition: "Ignition", expansion: "Bullish expansion", micro_pullback: "Micro-pullback", range_breakout: "Range breakout" };
const STATE: Record<string, string> = { pullback_forming: "Pullback forming", impulse: "Impulse established", building: "Momentum building", ignited: "Linked ignition observed", faded: "Buildup faded", recognized: "Ignition recognized", completed: "Completed", observed: "Observed", crossing_wick: "Wick above range — provisional", armed: "Armed", near: "Approaching trigger", crossing_provisional: "Crossing — provisional", trigger_observed: "Trigger observed", confirmed: "Confirmed observation", invalidated: "Invalidated", expired: "Expired", interrupted: "Interrupted", retracted: "Crossing retracted", extended: "Extended", retained: "Retaining breakout", rejected: "Back inside range", superseded: "Superseded", unavailable: "Assessment unavailable", warming: "Warming" };
export const detectionFamilyLabel = (family: string) => FAMILY[family] ?? family.replace(/_/g, " ");
export const detectionStateLabel = (state: string) => STATE[state] ?? state.replace(/_/g, " ");
export function detectionMatches(e: DetectionEpisode, filter: DetectionFilter): boolean {
  switch (filter) {
    case "momentum": return e.family === "momentum_building" || e.family === "upward_acceleration";
    case "ignition": return e.family === "ignition" || e.family === "expansion" || e.family === "range_breakout";
    case "micro": return e.family === "micro_pullback";
    case "near": return ["armed", "near", "crossing_provisional", "crossing_wick"].includes(e.state);
    case "triggered": return ["trigger_observed", "confirmed", "recognized"].includes(e.state);
    case "ross": return e.ross_member === true;
    case "catalyst": return e.catalyst === "qualifying";
    default: return true;
  }
}
export interface DetectionGroup { symbol: string; selected: DetectionEpisode; episodes: DetectionEpisode[]; lastMaterialAt: number }
/** Grouping and stable material-event ordering only; price ticks do not affect this key. */
export function groupDetections(snapshot: DetectionSnapshot | null, filter: DetectionFilter = "all", history = false, segment = "all"): DetectionGroup[] {
  const groups = new Map<string, DetectionEpisode[]>();
  for (const e of snapshot?.episodes ?? []) {
    if (e.terminal !== history || !detectionMatches(e, filter) || (filter === "all" && !e.default_visible) || (segment !== "all" && e.segment !== segment && !e.segment.endsWith(`/${segment}`))) continue;
    const key = `${e.instrument_id}/${e.symbol}`;
    const rows = groups.get(key) ?? [];
    rows.push(e); groups.set(key, rows);
  }
  const order = (a: DetectionEpisode, b: DetectionEpisode) => b.last_material_transition_at - a.last_material_transition_at || a.episode_id.localeCompare(b.episode_id);
  return [...groups.values()].map((episodes) => { episodes.sort(order); const symbol = episodes[0]!.symbol; const selected = episodes.find((e) => e.episode_id === snapshot?.dominant_episode_ids[symbol]) ?? episodes[0]!; return { symbol, selected, episodes, lastMaterialAt: episodes[0]!.last_material_transition_at }; }).sort((a, b) => b.lastMaterialAt - a.lastMaterialAt || a.symbol.localeCompare(b.symbol) || a.selected.episode_id.localeCompare(b.selected.episode_id));
}
export interface DetectionDistance { dollars: number; percent: number; atOrAbove: boolean }
/** Geometry, not readiness. Price is an admitted Rust value; no chart candle is consulted. */
export function detectionDistance(price: number | null, level: number | null, kind: "trigger" | "invalidation"): DetectionDistance | null {
  if (price === null || level === null || price <= 0 || level <= 0) return null;
  const delta = kind === "trigger" ? level - price : price - level;
  return { dollars: Math.abs(delta) / 1_000_000, percent: Math.abs(delta) * 100 / price, atOrAbove: price >= level };
}
export function detectionBlocked(e: DetectionEpisode): DetectionEvidence[] { return e.evidence.filter((x) => x.required && x.state !== "PASS"); }
export function detectionPrice(micros: number | null): string { return micros === null ? "—" : `$${(micros / 1_000_000).toFixed(micros < 1_000_000 ? 4 : 2)}`; }
export function detectionEvidenceValue(e: DetectionEvidence): string {
  if (e.value === null) return `unavailable${e.reason ? ` · ${e.reason.replace(/_/g, " ")}` : ""}`;
  if (e.units === "micros" && typeof e.value === "number") return detectionPrice(e.value);
  return `${typeof e.value === "number" ? Number(e.value.toFixed(4)).toLocaleString("en-US") : String(e.value)}${e.units ? ` ${e.units}` : ""}`;
}

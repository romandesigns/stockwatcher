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
  status: "provisional" | "confirmed" | "retracted" | "terminal" | "correction" | "unknown";
  restored: boolean;
  warmup: boolean;
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
  restored: boolean;
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
  near_trigger: boolean | null;
  /** Original engine record, retained for full anchors/features/level provenance in details. */
  raw: DetectionCoreEpisode;
  /** Context badges are evidence, never detector admission gates or confidence. */
  ross_member: boolean | null;
  catalyst: "qualifying" | "none" | "unknown";
  /** The day's gain against the previous official close, basis points (the Ross rule); null
   * when the server knows no close for the symbol (or is older than this field). */
  gain_bp: number | null;
  /** The recognition's headline measurements (ignition, momentum); [] for the others or an
   * older server. Copied verbatim from the engine's checks: format them, never derive. */
  summary: DetectionSummaryFact[];
}
export interface DetectionSummaryFact { name: string; value: number; units: string; window_secs: number | null }
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
export interface DetectionCoreLevel { level_id: string; kind: "trigger" | "invalidation" | "range_high" | "range_low"; version: number; price: number; known_at: number; armed_at: number | null; formed_from: [number, number]; reason: string }
export interface DetectionCoreCheck { name: string; state: DetectionCheck; value: number | null; threshold: number | null; units: string; reason: string; required?: boolean }
export interface DetectionCoreFeatures { bar_end: number; cutoff: number; [name: string]: unknown }
export interface DetectionCoreTransition {
  seq: number; transition_id: string; episode_id: string; symbol: string; family: string; variant: string;
  from: string | null; to: string; event: string; reason: string; certainty: string; observed_at: number;
  bar_end: number | null; cutoff: number | null; trade: { trade_id: { source: string; exchange: string; seq: number }; exec: number; admit: number; price: number } | null;
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
  /** Additive (2026-10-22); absent from older servers. */
  summary?: DetectionSummaryFact[];
}
export interface DetectionCoreSnapshot {
  schema: number; run_id: string; source: string; fidelity: string; policy: string; experimental: true; alerts_enabled: false;
  cursor: number; bar_end: number | null; cutoff: number | null; as_of: number | null;
  monitored?: { symbols_seen: number; symbols_today: number; symbols_active: number; symbols_interrupted: number; symbols_history_pending?: number };
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
  /** Per symbol. `prev_close_micros`/`gain_bp` are additive (2026-10-22) and present only when the
   * server knows the symbol's previous close; an entry may then hold only those two. */
  context?: Record<string, { ross_member?: boolean | null; catalyst?: DetectionEpisode["catalyst"]; prev_close_micros?: number; gain_bp?: number | null }>;
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
const nullableNumber = (x: unknown) => x === null || finite(x);
const nullableString = (x: unknown) => x === null || typeof x === "string";
const strings = (x: unknown): x is string[] => Array.isArray(x) && x.every((s) => typeof s === "string");
// Runtime::reset's wire identity is `<boot identity>:<monotone generation>`; comparing this
// suffix prevents a delayed full snapshot reviving an old seek after bounded ID retention.
const epochPosition = (epoch: string) => {
  const cut = epoch.lastIndexOf(":"); const generation = epoch.slice(cut + 1);
  return cut > 0 && /^\d+$/.test(generation) ? { boot: epoch.slice(0, cut), generation: BigInt(generation) } : null;
};
const coreLevel = (x: unknown): boolean => object(x) && typeof x.level_id === "string" && ["trigger", "invalidation", "range_high", "range_low"].includes(String(x.kind)) && finite(x.version) && finite(x.price) && finite(x.known_at) && nullableNumber(x.armed_at) && Array.isArray(x.formed_from) && x.formed_from.length === 2 && x.formed_from.every(finite) && typeof x.reason === "string";
function coreEpisode(x: unknown): boolean {
  if (!object(x) || !["episode_id", "family", "variant", "policy", "symbol", "instrument", "trading_date", "segment", "state", "fidelity"].every((k) => typeof x[k] === "string") || !["first_detected_at", "last_material_seq", "last_material_at", "base_secs"].every((k) => finite(x[k])) || !nullableNumber(x.first_bar_end) || typeof x.default_visible !== "boolean" || typeof x.warmup !== "boolean" || !nullableString(x.parent_episode_id) || !nullableString(x.context) || !strings(x.related_episode_ids)) return false;
  if (!Array.isArray(x.levels) || !x.levels.every(coreLevel) || !Array.isArray(x.checks) || !x.checks.every((c) => object(c) && typeof c.name === "string" && ["PASS", "FAIL", "UNKNOWN"].includes(String(c.state)) && nullableNumber(c.value) && nullableNumber(c.threshold) && typeof c.units === "string" && typeof c.reason === "string" && (c.required === undefined || typeof c.required === "boolean"))) return false;
  if (!(x.anchors === null || object(x.anchors)) || !(x.recognition === null || (object(x.recognition) && finite(x.recognition.bar_end) && finite(x.recognition.cutoff)))) return false;
  if (!(x.current === null || (object(x.current) && nullableNumber(x.current.price) && nullableNumber(x.current.price_exec) && nullableNumber(x.current.price_age_ms) && typeof x.current.stale === "boolean"))) return false;
  return x.resolution === null || (object(x.resolution) && typeof x.resolution.state === "string" && typeof x.resolution.reason === "string" && typeof x.resolution.transition_id === "string" && finite(x.resolution.at) && nullableNumber(x.resolution.bar_end));
}
/** Validate the fields the presentation adapter traverses. An unsupported schema is kept as a
 * recognized envelope so the channel can show its explicit resync/error state. */
export function isDetectionMessage(x: unknown): x is DetectionMessage {
  if (!object(x) || !["detection.snapshot", "detection.update"].includes(String(x.type)) || typeof x.enabled !== "boolean" || !["live", "replay", "fixture"].includes(String(x.source)) || typeof x.runEpoch !== "string" || !finite(x.cursor) || !object(x.history) || !Array.isArray(x.transitions) || !object(x.telemetry) || x.experimental !== true || x.alertsEnabled !== false) return false;
  if (!finite(x.history.dropped) || !finite(x.history.capacity)) return false;
  const transitionsValid = x.transitions.every((r) => object(r) && finite(r.sequence) && finite(r.emitted_at) && finite(r.input_watermark) && typeof r.restored === "boolean" && object(r.transition) && ["transition_id", "episode_id", "to", "event", "reason", "certainty"].every((k) => typeof (r.transition as Record<string, unknown>)[k] === "string") && finite(r.transition.seq) && finite(r.transition.observed_at) && nullableNumber(r.transition.cutoff) && nullableNumber(r.transition.bar_end) && nullableString(r.transition.from) && nullableString(r.transition.correction_of) && typeof r.transition.warmup === "boolean" && (r.transition.level === null || coreLevel(r.transition.level)));
  if (!transitionsValid) return false;
  if (!x.enabled && x.snapshot === null) return true;
  const s = x.snapshot;
  if (!object(s) || !finite(s.schema)) return false;
  if (s.schema !== 1) return true;
  if (!object(x.profile) || !["price_min_micros", "price_max_micros", "research_min_micros", "research_max_micros"].every((k) => finite(x.profile && (x.profile as Record<string, unknown>)[k]))) return false;
  if (!["policy", "run_id", "source", "fidelity"].every((k) => typeof s[k] === "string") || !nullableNumber(s.cutoff) || !nullableNumber(s.as_of) || !Array.isArray(s.active) || !Array.isArray(s.recent) || !Array.isArray(s.symbols) || !object(s.data_status)) return false;
  if (!s.symbols.every((r) => object(r) && typeof r.symbol === "string" && nullableString(r.dominant_episode_id) && strings(r.active_episode_ids) && nullableString(r.interrupted))) return false;
  if (s.monitored !== undefined && (!object(s.monitored) || !["symbols_seen", "symbols_today", "symbols_active", "symbols_interrupted"].every((k) => finite((s.monitored as Record<string, unknown>)[k])))) return false;
  return [...s.active, ...s.recent].every(coreEpisode);
}

function capturedLoss(telemetry: Record<string, unknown>): boolean {
  const engine = object(telemetry.engine) ? telemetry.engine : telemetry;
  // Recent/history retention is a bounded view, not an ingestion or persistence failure.
  return [engine.transitions_dropped, telemetry.journal_queue_dropped, telemetry.journal_write_errors].some((n) => finite(n) && n > 0);
}

/** Each update is a full replacement. Cursor jumps are therefore valid coalescing; a missing
 * significant transition is detected from its separate history sequence, never guessed from
 * the price-frame cadence. A reconnect does not replay notifications (there are none here). */
export function createDetectionChannel(io: { subscribe(): void; unsubscribe(): void; resync?(): void; source?: MarketSource }): DetectionChannel & { apply(m: DetectionMessage): void; reject(reason?: string): void; setConnected(open: boolean): void } {
  let state = EMPTY_DETECTION;
  const listeners = new Set<() => void>();
  const retired = new Set<string>();
  const identity = (epoch: string) => { const p = epochPosition(epoch); return p ? `boot:${p.boot}` : `opaque:${epoch}`; };
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
    reject(reason = "invalid_detection_frame") {
      state = { ...state, connected: false, resyncing: true, error: reason };
      const key = `${state.frame?.runEpoch ?? "pending"}/${reason}`;
      if (requested !== key) { requested = key; (io.resync ?? io.subscribe)(); }
      emit();
    },
    subscribe(fn) {
      listeners.add(fn);
      if (listeners.size === 1) io.subscribe();
      return () => { if (listeners.delete(fn) && !listeners.size) io.unsubscribe(); };
    },
    apply(m) {
      if (io.source && m.source !== io.source && !(io.source === "replay" && m.source === "fixture")) { resync(m, "source_mismatch"); return; }
      if (m.snapshot && m.snapshot.schema !== 1) { resync(m, "unsupported_schema"); return; }
      if (retired.has(identity(m.runEpoch))) return;
      const previous = state.frame;
      const oldEpoch = previous && epochPosition(previous.runEpoch);
      const newEpoch = epochPosition(m.runEpoch);
      if (oldEpoch && newEpoch && oldEpoch.boot === newEpoch.boot && newEpoch.generation < oldEpoch.generation) return;
      if ((!previous || previous.runEpoch !== m.runEpoch) && m.type !== "detection.snapshot") { resync(m, "snapshot_required"); return; }
      if (previous?.runEpoch === m.runEpoch && m.cursor < previous.cursor) return;
      if (previous?.runEpoch === m.runEpoch && m.cursor === previous.cursor && m.type !== "detection.snapshot") return;
      if (previous && previous.runEpoch !== m.runEpoch) {
        const before = identity(previous.runEpoch);
        if (before !== identity(m.runEpoch)) {
          // Seeks within a boot need only its monotone generation. Retire the BOOT on a
          // process replacement, so repeated seeks never evict a prior server identity.
          // At the finite retention limit, require a fresh channel rather than forgetting
          // an identity and letting its delayed snapshot become current again.
          if (retired.size >= 64) { resync(m, "epoch_history_full_reload_required"); return; }
          retired.add(before);
        }
      }
      const last = previous?.runEpoch === m.runEpoch ? previous.history.last_sequence : null;
      const historyGap = last !== null && last !== undefined && m.history.first_sequence !== null && m.history.first_sequence > last + 1;
      const coverageLost = (previous?.runEpoch === m.runEpoch && state.coverageLost) || historyGap || capturedLoss(m.telemetry);
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

const certainty = (raw: string): DetectionTransition["status"] => ["provisional", "confirmed", "retracted", "terminal", "correction"].includes(raw) ? raw as DetectionTransition["status"] : "unknown";
const transitionView = (r: DetectionRecordedTransition): DetectionTransition => {
  const t = r.transition;
  return { transition_id: t.transition_id, episode_id: t.episode_id, sequence: t.seq, previous_state: t.from, state: t.to, event: t.event, reason: t.reason, observed_at: t.observed_at, input_cutoff: t.cutoff ?? t.observed_at, bar_end: t.bar_end, input_sequence: r.input_watermark, status: certainty(t.certainty), restored: r.restored, warmup: t.warmup, tested_level_id: t.level?.level_id ?? null, tested_level_version: t.level?.version ?? null, supersedes: t.correction_of };
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

/** The well-formed summary facts of an episode (an additive field: absent or malformed is []). */
const summaryFacts = (x: unknown): DetectionSummaryFact[] => Array.isArray(x) ? x.filter((f): f is DetectionSummaryFact => object(f) && typeof f.name === "string" && finite(f.value) && typeof f.units === "string" && (f.window_secs === null || finite(f.window_secs))) : [];

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
    const gain = context?.gain_bp;
    return { episode_id: e.episode_id, instrument_id: e.instrument, symbol: e.symbol, run_id: s.run_id, source: m.source, fidelity: e.fidelity, trading_date: e.trading_date, segment: e.segment, family: e.family, variant: e.variant, policy_id: policyId ?? s.policy, policy_hash: m.policyHash ?? shortHash, parent_episode_id: e.parent_episode_id, related_episode_ids: e.related_episode_ids, first_detected_at: e.first_detected_at, last_material_transition_at: e.last_material_at, input_cutoff: e.recognition?.cutoff ?? at, bar_end: e.first_bar_end, state: e.state, status: last?.status ?? "unknown", restored: last?.restored ?? false, terminal: e.resolution !== null, resolved_at: e.resolution?.at ?? null, reason: e.resolution?.reason ?? null, price: e.current?.price ?? null, price_at: e.current?.price_exec ?? null, price_age_ms: e.current?.price_age_ms ?? null, price_stale: e.current?.stale ?? true, near_trigger: typeof e.current?.near_trigger === "boolean" ? e.current.near_trigger : null, level: levels.at(-1) ?? null, level_history: levels, evidence, timeline: history, coverage: e.warmup ? "restored/reconstructed — forward coverage unverified" : "collection completeness unverified", ross_member: context?.ross_member ?? null, catalyst: context?.catalyst ?? "unknown", gain_bp: finite(gain) ? gain : null, summary: summaryFacts(e.summary), default_visible: e.default_visible, raw: e };
  });
  const unavailable = new Map<string, number>();
  const interrupted = s.data_status.interrupted_symbols;
  if (Array.isArray(interrupted)) {
    for (const gap of interrupted) if (object(gap) && typeof gap.reason === "string") unavailable.set(gap.reason, (unavailable.get(gap.reason) ?? 0) + 1);
  } else for (const symbol of s.symbols) if (symbol.interrupted) unavailable.set(symbol.interrupted, (unavailable.get(symbol.interrupted) ?? 0) + 1);
  // A loader may be waiting before a symbol's first print, with no episode to project.
  // Use the engine's pending set (or its authoritative count), never infer from rows.
  const pending = s.data_status.history_pending_symbols;
  const pendingCount = Array.isArray(pending) ? new Set(pending.filter((x): x is string => typeof x === "string")).size : s.monitored?.symbols_history_pending ?? 0;
  if (pendingCount > 0) unavailable.set("history_pending", pendingCount);
  const global = s.data_status.global_interruption;
  if (object(global)) unavailable.set(typeof global.reason === "string" ? global.reason : "global_interruption", (s.monitored?.symbols_today ?? s.symbols.length));
  if (typeof s.data_status.disabled_reason === "string") unavailable.set(s.data_status.disabled_reason, (s.monitored?.symbols_today ?? s.symbols.length));
  return { schema: 1, run_id: s.run_id, source: m.source, fidelity: s.fidelity, policy_id: policyId ?? s.policy, policy_hash: m.policyHash ?? shortHash, experimental: true, base_timeframe_secs: 15, cutoff: s.cutoff ?? 0, as_of: s.as_of ?? s.cutoff ?? 0, ...m.profile, episodes, monitored_symbols: s.monitored?.symbols_today ?? s.symbols.length, unavailable: [...unavailable].map(([reason, symbols]) => ({ reason, symbols })), dominant_episode_ids: Object.fromEntries(s.symbols.map((x) => [x.symbol, x.dominant_episode_id])) };
}

/** Structurally matches chart-core DetectionOverlay, without making shared depend on a chart.
 * The caller supplies its symbol and optional exact episode selection; a missing ID yields no
 * substitute annotation. ns/micros -> ms/dollars is the only transformation.
 *
 * A resolved (terminal) episode yields none: the chart marks are the selected row's LIVE
 * references (design canvas, 2026-10-22), so they leave the chart the moment it resolves. */
export function detectionOverlays(snapshot: DetectionSnapshot | null, symbol: string, episodeId?: string) {
  const rows = snapshot?.episodes.filter((e) => e.symbol === symbol) ?? [];
  const preferred = episodeId ?? snapshot?.dominant_episode_ids[symbol];
  const e = preferred ? rows.find((x) => x.episode_id === preferred) : rows.filter((x) => !x.terminal && x.default_visible).sort((a, b) => b.last_material_transition_at - a.last_material_transition_at || a.episode_id.localeCompare(b.episode_id))[0];
  if (!e || e.terminal) return [];
  return [{ episodeId: e.episode_id, label: `${detectionFamilyLabel(e.family)} · ${detectionStateLabel(e.state)}`, recognizedAt: e.first_detected_at / 1_000_000, terminal: e.terminal, stale: e.price_stale, levels: e.level_history.map((l) => ({ id: l.level_id, kind: l.kind, version: l.version, knownAt: l.known_at / 1_000_000, armedAt: l.armed_at === null ? null : l.armed_at / 1_000_000, validTo: l.valid_to === null ? null : l.valid_to / 1_000_000, trigger: l.trigger / 1_000_000, invalidation: l.invalidation === null ? null : l.invalidation / 1_000_000 })) }];
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
    case "near": return !e.terminal && !e.price_stale && (e.near_trigger === true || ["crossing_provisional", "crossing_wick"].includes(e.state));
    case "triggered": return e.state === "trigger_observed";
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
export function detectionPrice(micros: number | null): string {
  if (micros === null) return "—";
  const exact = (micros / 1_000_000).toFixed(6).replace(/0+$/, "");
  const decimals = Math.max(Math.abs(micros) < 1_000_000 ? 4 : 2, exact.split(".")[1]?.length ?? 0);
  return `$${(micros / 1_000_000).toFixed(decimals)}`;
}
export function detectionEvidenceValue(e: DetectionEvidence): string {
  if (e.value === null) return `unavailable${e.reason ? ` · ${e.reason.replace(/_/g, " ")}` : ""}`;
  if (e.units === "micros" && typeof e.value === "number") return detectionPrice(e.value);
  return `${typeof e.value === "number" ? Number(e.value.toFixed(4)).toLocaleString("en-US") : String(e.value)}${e.units ? ` ${e.units}` : ""}`;
}

/**
 * The Signals row (design canvas "Detection Panel", owner-approved 2026-10-21/22): one compact
 * row per instrument, shared by web and phone so they cannot disagree. The STAGE labels are the
 * Ross cue's (packages/shared/src/ross.ts): Pullback, At trigger, Crossing, Broke trigger,
 * Failed. They describe where the price stands against levels the engine names, not entry
 * signals: on the 2026-10-07..20 holdout a trigger observed failed about three times in four, so
 * "Broke trigger" reads in the normal text colour, never a success green, and never counts as
 * needing attention. Accent is spent only on a crossing in progress, amber on a price at the
 * trigger. Families without a pullback stage (momentum, ignition) keep their own state label,
 * muted. Nothing here derives a detection: every field is the engine's.
 */
export type DetectionStage = "crossing" | "near" | "triggered" | "forming" | "failed" | "other";
export type DetectionTone = "critical" | "warning" | "accent" | "text" | "muted" | "dim";
export interface DetectionRowView {
  symbol: string;
  price: string;
  /** The day's gain, as the Ross rows print it ("+38.4%"); "" when the server sent none. */
  gain: string;
  /** Green up / red down like the Ross and Runners gain; never a stage colour. */
  gainTone: "good" | "critical" | "muted";
  family: string;
  stage: DetectionStage;
  label: string;
  tone: DetectionTone;
  /** A crossing in progress pulses, only while the data is current. */
  pulse: boolean;
  /** At the trigger or crossing it: the tab/card count. */
  attention: boolean;
  note: string;
  /** 0 at the invalidation (pullback low), 1 at the trigger; null when there is nothing current to place. */
  meter: number | null;
  dimmed: boolean;
  ago: string;
  news: boolean;
  ross: boolean;
  aria: string;
}

const CROSSING = new Set(["crossing_provisional", "crossing_wick"]);
const FORMING = new Set(["pullback_forming", "armed", "near"]);
const BROKE = new Set(["trigger_observed", "confirmed", "retained"]);
const FAILED = new Set(["invalidated", "rejected"]);
/** Prices as the rows print them (the canvas): four decimals below $1, two from $1, no "$". */
const rowPrice = (micros: number | null) => (micros === null ? "—" : (micros / 1_000_000).toFixed(Math.abs(micros) < 1_000_000 ? 4 : 2));

/** Basis points as the Ross rows print a gain: one decimal, a sign, a real minus. */
export function detectionGain(bp: number | null): string {
  if (bp === null) return "";
  const v = bp / 100;
  const s = Math.abs(v).toFixed(1);
  return v > 0 ? `+${s}%` : v < 0 ? `−${s}%` : "0.0%";
}

const span = (secs: number | null) => (secs === null ? "" : secs >= 60 && secs % 60 === 0 ? ` in ${secs / 60}m` : ` in ${secs}s`);
const ratio = (milli: number) => `${(milli / 1000).toFixed(milli >= 10_000 ? 0 : 1)}×`;
const logPercent = (ppm: number) => {
  const p = (Math.exp(ppm / 1_000_000) - 1) * 100;
  return `${p >= 0 ? "+" : "−"}${Math.abs(p).toFixed(1)}%`;
};
/**
 * The row note of a family with no pullback stage (ignition, momentum): the engine's own
 * recognition measurements (`summary`, copied from its checks), formatted, never derived:
 * "range 3.2× · +2.1% in 15s", "$vol 4.1× · +1.2% in 30s". A fact of an unknown kind is skipped.
 */
export function detectionSummaryNote(facts: readonly DetectionSummaryFact[]): string {
  const parts: string[] = [];
  for (const f of facts) {
    if (f.name === "range_expansion" && f.units === "milli") parts.push(`range ${ratio(f.value)}`);
    else if (f.name === "dollar_volume_rate" && f.units === "milli") parts.push(`$vol ${ratio(f.value)}`);
    else if ((f.name === "return" || f.name === "rise") && f.units === "log_ppm") parts.push(`${logPercent(f.value)}${span(f.window_secs)}`);
  }
  return parts.join(" · ");
}

/** "now" under a minute, then minutes, then hours: since the instrument's last material change. */
export function detectionAgo(thenNs: number, nowNs: number): string {
  const s = Math.max(0, (nowNs - thenNs) / 1_000_000_000);
  return s < 60 ? "now" : s < 3600 ? `${Math.floor(s / 60)}m` : `${Math.floor(s / 3600)}h`;
}

/** An episode's stage. "At trigger" needs a current price: a stale one reads as Pullback. */
export function detectionStage(e: DetectionEpisode, current = true): DetectionStage {
  if (CROSSING.has(e.state)) return "crossing";
  if (BROKE.has(e.state)) return "triggered";
  if (FAILED.has(e.state)) return "failed";
  if (FORMING.has(e.state)) return current && !e.price_stale && !e.terminal && (e.near_trigger === true || e.state === "near") ? "near" : "forming";
  return "other";
}

/** The row for an instrument's selected episode. `current` is false while the feed is
 * reconnecting/resyncing: then nothing pulses, no distance is shown and the meter is withheld. */
export function detectionRowView(group: DetectionGroup, nowNs: number, current: boolean, formatTime: (epochNs: number) => string): DetectionRowView {
  const e = group.selected;
  const live = current && !e.price_stale && !e.terminal;
  const trigger = e.level?.trigger ?? null;
  const inval = e.level?.invalidation ?? null;
  const stage = detectionStage(e, current);
  const LOOK: Record<DetectionStage, [string, DetectionTone]> = {
    crossing: ["Crossing", "accent"],
    near: ["At trigger", "warning"],
    triggered: ["Broke trigger", "text"],
    forming: ["Pullback", "muted"],
    failed: ["Failed", "dim"],
    other: [detectionStateLabel(e.state), e.terminal ? "dim" : "muted"],
  };
  const [label, tone] = LOOK[stage];
  const distance = live && trigger !== null ? detectionDistance(e.price, trigger, "trigger") : null;
  const away = distance && !distance.atOrAbove ? ` · ${distance.percent.toFixed(1)}% away` : "";
  const when = e.resolved_at ?? e.last_material_transition_at;
  let note = "";
  if (stage === "crossing" && trigger !== null) note = `${rowPrice(trigger)} · close pending`;
  else if (stage === "near" && trigger !== null) note = `${rowPrice(trigger)}${away}`;
  else if (stage === "forming" && trigger !== null) note = `trigger ${rowPrice(trigger)}${away}`;
  else if (stage === "triggered" && trigger !== null) note = `${rowPrice(trigger)}${when ? ` · ${formatTime(when)}` : ""}`;
  else if (stage === "failed") note = e.state === "rejected" ? "back inside range" : inval !== null ? `below ${rowPrice(inval)}` : "";
  else if (stage === "other") note = detectionSummaryNote(e.summary);
  // The meter only where a pullback stage is live and both levels are known.
  const meter = live && (stage === "forming" || stage === "near" || stage === "crossing") && trigger !== null && inval !== null && e.price !== null && trigger > inval ? Math.max(0, Math.min(1, (e.price - inval) / (trigger - inval))) : null;
  const view: DetectionRowView = {
    symbol: e.symbol,
    price: rowPrice(e.price),
    gain: detectionGain(e.gain_bp),
    gainTone: e.gain_bp === null || e.gain_bp === 0 ? "muted" : e.gain_bp > 0 ? "good" : "critical",
    family: detectionFamilyLabel(e.family),
    stage,
    label,
    tone,
    pulse: stage === "crossing" && live,
    attention: live && (stage === "crossing" || stage === "near"),
    note,
    meter,
    dimmed: stage === "failed" || (e.terminal && stage === "other"),
    ago: detectionAgo(group.lastMaterialAt, nowNs),
    news: e.catalyst === "qualifying",
    ross: e.ross_member === true,
    aria: "",
  };
  view.aria = `${view.symbol} ${view.price}${view.gain ? ` ${view.gain}` : ""}, ${view.family}, ${label}${note ? ` ${note}` : ""}, ${view.ago}${view.news ? ", news" : ""}${view.ross ? ", on the Ross Panel" : ""}${e.price_stale ? ", price stale" : ""}`;
  return view;
}

/** Active instruments at the trigger or crossing it: the Signals tab's and card's count. */
export function detectionAttention(snapshot: DetectionSnapshot | null): number {
  return groupDetections(snapshot).filter((g) => {
    const s = detectionStage(g.selected);
    return !g.selected.terminal && !g.selected.price_stale && (s === "crossing" || s === "near");
  }).length;
}

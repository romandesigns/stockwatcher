// Frozen causal evidence from NoticeDetail::Breakout. Optional metadata never gates a notice.
// Epoch ms and integer micros are JS-safe; exact ns/admission ordinals stay decimal strings.
export interface BreakoutNotice {
  v: 1;
  rule: "post_flag_exact_high_v1";
  policy_id: "rocket-momentum-15s-exact-after-v1";
  timeframe_secs: 15;
  event_time_basis: "bar_end";
  volume_scope: "whole_15s_bar";
  source: "accepted_original_ohlc";
  exact_state: "complete" | "restored";
  bar_start_ms: number; bar_end_ms: number; event_at_ms: number; evaluated_at_ms: number;
  flag_at_ms: number; flag_known_at_ms: number; high_exec_ms: number;
  reference_known_at_ms: number;
  reference_basis: "flag_price" | "eligible_bar_high";
  reference_bar_start_ms: number | null; reference_bar_end_ms: number | null;
  reference_price_micros: number; high_price_micros: number; close_price_micros: number | null;
  high_receive_ms: number | null;
  flag_known_ns: string; evaluated_ns: string; high_exec_ns: string; high_receive_ns: string | null;
  reference_known_ns: string;
  flag_creation_admission: string; decision_admission: string; high_admission: string;
  volume: number; pace_slot: number; required_multiple: number; volume_ratio: number | null;
  zero_base_fallback: boolean;
}

export interface NoticeChartContext { event_id: string; symbol: string; breakout: BreakoutNotice }
export interface NoticeContextBinding<T> { feed: T; source: string; context: NoticeChartContext | undefined; invalidated: boolean }

/** A source change invalidates an inspection until another explicit event is opened. */
export function bindNoticeContext<T>(previous: NoticeContextBinding<T> | null, feed: T, source: string, context?: NoticeChartContext): NoticeContextBinding<T> {
  if (!previous || previous.context !== context) return { feed, source, context, invalidated: source !== "live" };
  if (previous.feed !== feed || previous.source !== source) return { feed, source, context, invalidated: true };
  return previous;
}
export const BREAKOUT_LABEL = "15s new high since alert";
const obj = (v: unknown): v is Record<string, unknown> => !!v && typeof v === "object" && !Array.isArray(v);
const integer = (v: unknown): v is number => typeof v === "number" && Number.isSafeInteger(v) && v >= 0;
const epochMs = (v: unknown): v is number => integer(v) && v <= 8_640_000_000_000_000;
const finite = (v: unknown): v is number => typeof v === "number" && Number.isFinite(v) && v >= 0;
const decimal = (v: unknown): v is string => typeof v === "string" && /^(0|[1-9]\d{0,29})$/.test(v);
const micros = (v: unknown): v is number => integer(v) && v > 0;

/** Unsupported or malformed optional evidence becomes unknown; its legacy notice remains readable. */
export function parseBreakoutNotice(v: unknown): BreakoutNotice | undefined {
  if (!obj(v) || v.v !== 1 || v.rule !== "post_flag_exact_high_v1" || v.policy_id !== "rocket-momentum-15s-exact-after-v1" || v.timeframe_secs !== 15 || v.event_time_basis !== "bar_end" || v.volume_scope !== "whole_15s_bar" || v.source !== "accepted_original_ohlc" || (v.exact_state !== "complete" && v.exact_state !== "restored")) return undefined;
  for (const k of ["bar_start_ms", "bar_end_ms", "event_at_ms", "evaluated_at_ms", "flag_at_ms", "flag_known_at_ms", "high_exec_ms", "reference_known_at_ms"] as const) if (!epochMs(v[k])) return undefined;
  for (const k of ["reference_price_micros", "high_price_micros"] as const) if (!micros(v[k])) return undefined;
  if (v.close_price_micros !== null && !micros(v.close_price_micros)) return undefined;
  for (const k of ["high_receive_ms", "reference_bar_start_ms", "reference_bar_end_ms"] as const) if (v[k] !== null && !epochMs(v[k])) return undefined;
  for (const k of ["flag_known_ns", "evaluated_ns", "high_exec_ns", "reference_known_ns", "flag_creation_admission", "decision_admission", "high_admission"] as const) if (!decimal(v[k])) return undefined;
  if (v.high_receive_ns !== null && !decimal(v.high_receive_ns)) return undefined;
  if (v.reference_basis !== "flag_price" && v.reference_basis !== "eligible_bar_high") return undefined;
  for (const k of ["volume", "pace_slot", "required_multiple"] as const) if (!finite(v[k])) return undefined;
  if (v.volume_ratio !== null && !finite(v.volume_ratio)) return undefined;
  if (typeof v.zero_base_fallback !== "boolean") return undefined;
  const b = v as unknown as BreakoutNotice;
  if (b.bar_end_ms - b.bar_start_ms !== 15_000 || b.event_at_ms !== b.bar_end_ms || b.evaluated_at_ms < b.bar_end_ms || b.high_exec_ms < b.bar_start_ms || b.high_exec_ms >= b.bar_end_ms || b.flag_known_at_ms > b.evaluated_at_ms || b.reference_known_at_ms > b.evaluated_at_ms || b.high_price_micros <= b.reference_price_micros || b.required_multiple !== 2 || b.volume <= 0) return undefined;
  if ((b.high_receive_ms === null) !== (b.high_receive_ns === null)) return undefined;
  if (b.reference_known_at_ms < b.flag_known_at_ms || (b.reference_basis === "flag_price" && b.reference_known_ns !== b.flag_known_ns)) return undefined;
  for (const [ns, ms] of [[b.flag_known_ns, b.flag_known_at_ms], [b.evaluated_ns, b.evaluated_at_ms], [b.high_exec_ns, b.high_exec_ms], [b.reference_known_ns, b.reference_known_at_ms], [b.high_receive_ns, b.high_receive_ms]] as const) if (ns !== null && BigInt(ns) / 1_000_000n !== BigInt(ms!)) return undefined;
  if (BigInt(b.high_exec_ns) <= BigInt(b.flag_known_ns) || BigInt(b.high_admission) <= BigInt(b.flag_creation_admission) || BigInt(b.high_admission) > BigInt(b.decision_admission)) return undefined;
  if (b.pace_slot === 0 ? !b.zero_base_fallback || b.volume_ratio !== null : b.zero_base_fallback || b.volume_ratio === null || b.volume < b.required_multiple * b.pace_slot) return undefined;
  if (b.pace_slot > 0) {
    const expectedRatio = b.volume / b.pace_slot;
    if (!Number.isFinite(expectedRatio) || Math.abs(b.volume_ratio! - expectedRatio) > 1e-9 * Math.max(1, Math.abs(expectedRatio))) return undefined;
  }
  if (b.reference_basis === "eligible_bar_high") {
    if (b.reference_bar_start_ms === null || b.reference_bar_end_ms === null || b.reference_bar_end_ms - b.reference_bar_start_ms !== 15_000 || b.reference_bar_end_ms > b.bar_start_ms || b.reference_known_at_ms < b.reference_bar_end_ms) return undefined;
  } else if (b.reference_bar_start_ms !== null || b.reference_bar_end_ms !== null) return undefined;
  // Whitelist fields; never retain arbitrary payload keys or unsafe nested geometry.
  const out: Record<string, unknown> = {};
  for (const k of ["v", "rule", "policy_id", "timeframe_secs", "event_time_basis", "volume_scope", "source", "exact_state", "bar_start_ms", "bar_end_ms", "event_at_ms", "evaluated_at_ms", "flag_at_ms", "flag_known_at_ms", "high_exec_ms", "reference_known_at_ms", "reference_basis", "reference_bar_start_ms", "reference_bar_end_ms", "reference_price_micros", "high_price_micros", "close_price_micros", "high_receive_ms", "flag_known_ns", "evaluated_ns", "high_exec_ns", "high_receive_ns", "reference_known_ns", "flag_creation_admission", "decision_admission", "high_admission", "volume", "pace_slot", "required_multiple", "volume_ratio", "zero_base_fallback"] as const) out[k] = b[k];
  return out as unknown as BreakoutNotice;
}

export function parseNoticeChartContext(raw: unknown, symbol?: string): NoticeChartContext | undefined {
  if (!obj(raw) || typeof raw.event_id !== "string" || !raw.event_id || raw.event_id.length > 256 || typeof raw.symbol !== "string" || !/^[A-Z][A-Z0-9.-]{0,11}$/.test(raw.symbol) || (symbol && symbol !== raw.symbol)) return undefined;
  const breakout = parseBreakoutNotice(raw.breakout);
  return breakout ? { event_id: raw.event_id, symbol: raw.symbol, breakout } : undefined;
}

/** The producer's immutable identity is required; do not invent one from a ticker or body. */
export function noticeChartContext(item: { event_id: string; symbol?: string; notice?: string; breakout?: BreakoutNotice }): NoticeChartContext | undefined {
  return item.notice === "runners.breakout" ? parseNoticeChartContext(item) : undefined;
}

export function runnersNoticeChartContext(item: { id: string; symbol: string; kind: string; data?: Record<string, unknown> }): NoticeChartContext | undefined {
  return item.kind === "runners.breakout" ? parseNoticeChartContext({ event_id: item.id, symbol: item.symbol, breakout: item.data?.breakout }) : undefined;
}

const price = (p: number) => `$${(p / 1_000_000).toFixed(6).replace(/0+$/, "").replace(/\.$/, "")}`;
export function breakoutSummary(b: BreakoutNotice): string {
  return `15s bar high ${price(b.high_price_micros)} > ${b.reference_basis === "flag_price" ? "alert price" : "earlier eligible high"} ${price(b.reference_price_micros)} · ${b.zero_base_fallback ? "positive-volume fallback (zero prior pace)" : `${b.volume_ratio!.toFixed(2)}× prior pace (required ${b.required_multiple}×)`} · volume uses whole 15s bar`;
}

/** Distinct clocks; no scheduled endpoint is described as detection or knowledge time. */
export function breakoutClockText(b: BreakoutNotice, format: (ms: number) => string): string {
  const clock = (ms: number) => `${format(ms)} +${String(ms % 1000).padStart(3, "0")} ms`;
  return `15s bar end ${clock(b.event_at_ms)} · evaluated ${clock(b.evaluated_at_ms)} · high execution ${clock(b.high_exec_ms)}${b.high_receive_ms === null ? " · high observed unavailable" : ` · high observed ${clock(b.high_receive_ms)}`} · comparison known ${clock(b.reference_known_at_ms)}`;
}

/** Presentation-only projection through the existing annotation renderer, unchanged by display TF. */
export function breakoutOverlays(context: NoticeChartContext | undefined, symbol: string) {
  if (!context || context.symbol !== symbol) return [];
  const b = context.breakout;
  return [{ episodeId: context.event_id, label: BREAKOUT_LABEL, recognizedAt: b.event_at_ms, terminal: false, levels: [
    { id: `${context.event_id}/comparison`, version: 1, knownAt: b.reference_known_at_ms, armedAt: null, validTo: b.evaluated_at_ms, trigger: b.reference_price_micros / 1_000_000, invalidation: null },
    { id: `${context.event_id}/high`, version: 1, knownAt: b.evaluated_at_ms, armedAt: null, validTo: null, trigger: b.high_price_micros / 1_000_000, invalidation: null },
  ] }];
}

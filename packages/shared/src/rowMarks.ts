// The lighter rows (owner, 2026-10-26, approved design canvas "StockWatcher lighter rows"): the
// facts a Ross, Runners or Favorites row used to spell out in sentences become small marks, each
// with its name and exact value on hover (web) or press-and-hold (phone). Text stays only for the
// ticker, the price and the day's change. This module derives every mark from what a row's wire
// already carries, ONE way for every panel and both clients; a mark whose data is absent is null
// and is not drawn (never a guessed value).

import type { FavoriteRowWire } from "./favorites.ts";
import type { RossRowWire } from "./protocol.ts";
import { rocketBreakoutTracking, type RocketRowWire, type RocketState } from "./rocket.ts";
import { rossLineScale, rossPrice, type RossLineView, type RossTone } from "./ross.ts";

/** The five pillars in their fixed order (stock-context PillarId). */
export const PILLAR_ORDER = ["price", "gain", "rvol", "float", "news"] as const;
export type PillarKey = (typeof PILLAR_ORDER)[number];
const PILLAR_NAME: Record<PillarKey, string> = { price: "price", gain: "gain", rvol: "relative volume", float: "float", news: "news" };

export interface PillarDot {
  /** The pillar, when the row knows which one this dot is (Ross rows); null for a count-only row. */
  pillar: PillarKey | null;
  state: "pass" | "fail" | "unknown";
}

export interface PillarsMark {
  dots: PillarDot[];
  passes: number;
  /** "Five pillars · 4 of 5 · missing: float". */
  label: string;
}

/**
 * The pillar dots from the pillars that do not pass (a Ross row's `missing`, or its membership's
 * `nonpassing`): every other pillar passes. Order is fixed so a dot always means the same pillar.
 */
export function pillarsFromMissing(missing: readonly { pillar: string; state: string }[]): PillarsMark {
  const by = new Map(missing.map((m) => [m.pillar, m.state === "FAIL" ? "fail" : "unknown"] as const));
  const dots: PillarDot[] = PILLAR_ORDER.map((p) => ({ pillar: p, state: by.get(p) ?? "pass" }));
  const passes = dots.filter((d) => d.state === "pass").length;
  const miss = dots.filter((d) => d.state !== "pass").map((d) => `${PILLAR_NAME[d.pillar!]}${d.state === "unknown" ? " (unknown)" : ""}`);
  return { dots, passes, label: `Five pillars · ${passes} of 5${miss.length ? ` · missing: ${miss.join(", ")}` : " · all pass"}` };
}

/**
 * The dots from a count alone (a favorite's `pillars {passes, of}`): which pillars pass is not on
 * that wire, so the dots are filled from the left and the label says only the count.
 */
export function pillarsFromCount(passes: number, of: number): PillarsMark | null {
  if (!Number.isFinite(passes) || !Number.isFinite(of) || of <= 0) return null;
  const p = Math.max(0, Math.min(of, Math.round(passes)));
  const dots: PillarDot[] = Array.from({ length: of }, (_, i) => ({ pillar: null, state: i < p ? "pass" : "fail" }));
  return { dots, passes: p, label: `Five pillars · ${p} of ${of} pass` };
}

export interface RvolMark {
  /** 1: under 2×, 2: 2–5×, 3: above 5×. */
  bars: 1 | 2 | 3;
  label: string;
}

/** Relative volume as 1–3 bars (`ratio` in ×); null when unknown. */
export function rvolMark(ratio: number | null | undefined): RvolMark | null {
  if (typeof ratio !== "number" || !Number.isFinite(ratio) || ratio < 0) return null;
  const bars = ratio >= 5 ? 3 : ratio >= 2 ? 2 : 1;
  return { bars, label: `Relative volume ${ratio.toFixed(1)}×` };
}

export interface NewsMark {
  /** An offering, warrants, a convertible or an ATM program (the headline class `financing`). */
  dilution: boolean;
  label: string;
}

/**
 * The headline classes the server assigns at load (market-archive `EVENT_RULES`, the research
 * news protocol's keyword rules). Only `financing` is unambiguous dilution: reverse splits and
 * delisting notices share `corporate_action` with mergers and buybacks, so that class stays
 * neutral rather than raising a false alarm.
 */
const NEWS_LABEL: Record<string, string> = {
  financing: "News: financing / offering (dilution risk)",
  regulatory_decision: "News: FDA / regulatory decision",
  clinical_result: "News: clinical result",
  earnings: "News: earnings",
  guidance: "News: guidance",
  corporate_action: "News: corporate action (merger, split, listing)",
  commercial_agreement: "News: agreement / partnership",
};

/** The news mark: only when the news pillar passes (`passes`), coloured for dilution. */
export function newsMark(passes: boolean, kind: string | null | undefined): NewsMark | null {
  if (!passes) return null;
  return { dilution: kind === "financing", label: (kind && NEWS_LABEL[kind]) ?? "News today" };
}

export type PathKind = "runner" | "burst" | "running" | "run" | "fast";
const PATH_NAME: Record<PathKind, string> = { runner: "R2 flag", burst: "Buying burst", running: "Running", run: "Run +20%", fast: "Fast move" };

export interface PathMark {
  kind: PathKind;
  /** "3m", "now", "1h 05m". */
  age: string;
  label: string;
}

/** "now" under a minute, else "3m", "1h 05m" (the Runners age format). */
function age(ms: number): string {
  const m = Math.floor(Math.max(0, ms) / 60_000);
  if (m < 1) return "now";
  if (m < 60) return `${m}m`;
  return `${Math.floor(m / 60)}h ${String(m % 60).padStart(2, "0")}m`;
}

/**
 * Today's Runners path for a stock: which path flagged it and how long ago, with the clock time
 * (`formatTime`, epoch ms → "9:42 AM") and any extra (`detail`, e.g. later paths) in the label.
 */
export function pathMark(kind: string | null | undefined, atMs: number | null | undefined, nowMs: number, formatTime: (epochMs: number) => string, detail = ""): PathMark | null {
  if (!kind || !(kind in PATH_NAME) || typeof atMs !== "number" || !Number.isFinite(atMs)) return null;
  const k = kind as PathKind;
  const a = age(nowMs - atMs);
  return { kind: k, age: a, label: `Runners: ${PATH_NAME[k]} at ${formatTime(atMs)} (${a === "now" ? "just now" : `${a} ago`})${detail ? ` · ${detail}` : ""}` };
}

export type StageKind = "forming" | "armed" | "near" | "crossing" | "triggered" | "invalidated";

export interface StageMark {
  stage: StageKind;
  tone: RossTone;
  /** Price between the stop (0) and the trigger (1), when known. */
  meter: number | null;
  /** "6.10", "" without a trigger. */
  trigger: string;
  label: string;
}

const STAGE: Record<StageKind, { name: string; tone: RossTone }> = {
  forming: { name: "Setup forming", tone: "dim" },
  armed: { name: "Setup armed", tone: "muted" },
  near: { name: "At the trigger", tone: "warning" },
  crossing: { name: "Crossing the trigger", tone: "accent" },
  triggered: { name: "Broke the trigger", tone: "text" },
  invalidated: { name: "Setup failed", tone: "dim" },
};

/**
 * The micro-pullback stage from a cue state (`forming`, `near`, `crossing`, `triggered`,
 * `invalidated`; `watching` and `halted` have none) and its levels (micros). A forming cue with a
 * trigger has armed: its levels are fixed. The meter is the cue's own `meter_milli`.
 */
export function stageMark(state: string | null | undefined, trigger: number | null | undefined, stop: number | null | undefined, meterMilli: number | null | undefined): StageMark | null {
  if (!state) return null;
  const hasTrigger = typeof trigger === "number" && trigger > 0;
  let stage: StageKind | null = null;
  if (state === "forming") stage = hasTrigger ? "armed" : "forming";
  else if (state === "near" || state === "crossing" || state === "triggered" || state === "invalidated") stage = state;
  if (!stage) return null;
  const look = STAGE[stage];
  const levels = [hasTrigger && `trigger ${rossPrice(trigger!)}`, typeof stop === "number" && stop > 0 && `stop ${rossPrice(stop)}`].filter(Boolean).join(" · ");
  const meter = stage === "invalidated" || typeof meterMilli !== "number" ? null : Math.max(0, Math.min(1, meterMilli / 1000));
  return { stage, tone: look.tone, meter, trigger: hasTrigger ? rossPrice(trigger!) : "", label: `${look.name}${levels ? ` · ${levels}` : ""}` };
}

export interface TailMark {
  tone: "good" | "critical" | "muted";
  label: string;
}

/** The sparkline's last-minute tail: its colour and label from the last minute's move (%). */
export function tailMark(pct: number | null | undefined): TailMark | null {
  if (typeof pct !== "number" || !Number.isFinite(pct)) return null;
  const r = Math.round(pct * 10) / 10;
  const s = `${r > 0 ? "+" : r < 0 ? "−" : ""}${Math.abs(r).toFixed(1)}%`;
  return { tone: r > 0 ? "good" : r < 0 ? "critical" : "muted", label: `Last minute ${s}` };
}

/** How long a Buying burst keeps the flame lit (owner, 2026-10-26: "look at me right now"). */
export const FLAME_MS = 5 * 60_000;

export interface FlameMark {
  lit: boolean;
  label: string;
}

/**
 * "Look now": lit while the stock's Buying burst Runners entry is under {@link FLAME_MS} old AND
 * the last price is still at or above the burst's alert price. Buying burst is the one detector
 * that beat its baselines on untested data (about 4 in 10 followed through): the strongest
 * evidence there is right now, not a verified trade. Re-evaluated against the clock, so it goes
 * out by itself. Prices in dollars, times in epoch ms (the Runners market clock).
 */
export function flameMark(burst: { at: number; p: number } | null | undefined, last: number | null | undefined, nowMs: number): FlameMark | null {
  if (!burst || !Number.isFinite(burst.at) || !(burst.p > 0) || typeof last !== "number" || !Number.isFinite(last)) return null;
  const old = nowMs - burst.at;
  if (old < 0 || old >= FLAME_MS || last < burst.p) return null;
  const mins = Math.floor(old / 60_000);
  return { lit: true, label: `Look now: Buying burst ${mins < 1 ? "under a minute" : `${mins}m`} ago, price ${last.toFixed(last < 1 ? 4 : 2)} still at or above its alert ${burst.p.toFixed(burst.p < 1 ? 4 : 2)}` };
}

/** Today's Runners row per symbol, for joining a Ross or Favorites row to its path and burst. */
export function runnersBySymbol(state: RocketState | null | undefined): Map<string, RocketRowWire> {
  const out = new Map<string, RocketRowWire>();
  for (const r of state?.panel?.rows ?? []) if (r && typeof r.s === "string" && !out.has(r.s)) out.set(r.s, r);
  return out;
}

/** The burst a Runners row stands for (its own alert time and price), else null. */
export function burstOf(r: RocketRowWire | null | undefined): { at: number; p: number } | null {
  return r && r.kind === "burst" && Number.isFinite(r.at) && r.p > 0 ? { at: r.at, p: r.p } : null;
}

/**
 * What a lighter row draws over its day line, in the line's own `w`×`h` box: the last minute as a
 * thick tail (the last three points, 30 s apart, so about one minute), and the setup's trigger as a
 * dotted level when it sits within the drawn range (a trigger far outside it would flatten the
 * shape, so it is left to the stage mark).
 */
export function rowLineOverlay(line: RossLineView, w: number, h: number, triggerMicros: number | null | undefined, pad = 2): { tail: string | null; triggerY: number | null } {
  const v = line.values;
  const n = v.length;
  const { y, lo, hi, slack } = rossLineScale(line, h, pad);
  const f = (x: number) => (Math.round(x * 10) / 10).toString();
  const tail = n >= 2 ? `M${v.slice(-3).map((p, i, a) => `${f(((n - a.length + i) / (n - 1)) * w)} ${f(y(p))}`).join(" L")}` : null;
  const t = typeof triggerMicros === "number" && triggerMicros >= lo - slack && triggerMicros <= hi + slack ? triggerMicros : null;
  return { tail, triggerY: t === null ? null : Math.round(Math.max(0, Math.min(h, y(t))) * 10) / 10 };
}

export interface ZoneMark {
  /** Price came back into the zone before (drawn fainter, like the chart's tested zones). */
  tested: boolean;
  label: string;
}

/** The zone's dollar price in the row's style: four places under $1, two above. */
const dollars = (p: number) => (p < 1 ? p.toFixed(4) : p.toFixed(2));

/**
 * The Runners row's demand-zone mark: the price sits inside a live automatic demand zone. The
 * label carries the measured hold rate (research/auto-zones-hold, 118 never-read days, Runners
 * stocks: retested zones held 59–67% vs 47–59% for random levels), so it reads as context, never
 * as a call. Null without a zone or with nonsense edges.
 */
export function zoneMark(zone: { lo: number; hi: number; tested: boolean } | null | undefined): ZoneMark | null {
  if (!zone || !Number.isFinite(zone.lo) || !Number.isFinite(zone.hi) || !(zone.hi > zone.lo) || !(zone.lo > 0)) return null;
  return { tested: zone.tested === true, label: `In demand zone $${dollars(zone.lo)}–$${dollars(zone.hi)} · zones held about 6 in 10 retests (5 in 10 at random levels)` };
}

/** Every mark one row can show; each is null when the row has no data for it. */
export interface RowMarks {
  flame: FlameMark | null;
  pillars: PillarsMark | null;
  rvol: RvolMark | null;
  news: NewsMark | null;
  path: PathMark | null;
  /** Runners rows only (the server computes it for them). */
  zone: ZoneMark | null;
  stage: StageMark | null;
  tail: TailMark | null;
}

/**
 * The phone's press-and-hold read-out of a row's marks (it has no hover): every mark's label, in
 * the order the row draws them.
 */
export function marksReadout(m: RowMarks): string {
  return [m.flame?.label, m.pillars?.label, m.rvol?.label, m.news?.label, m.path?.label, m.zone?.label, m.stage?.label, m.tail?.label].filter(Boolean).join("\n");
}

/** A Ross member's marks, joined to its Runners row (`runner`) when it has one today. */
export function rossRowMarks(r: RossRowWire, runner: RocketRowWire | null | undefined, nowMs: number, formatTime: (epochMs: number) => string): RowMarks {
  const missing = r.missing ?? r.membership.nonpassing.map(([pillar, state]) => ({ pillar, state }));
  const newsMissing = missing.some((m) => m.pillar === "news");
  const m1 = r.move_1m_bp ?? (r.momentum.status === "ready" ? r.momentum.r_primary_bp : null);
  const s = r.setup ?? null;
  return {
    flame: flameMark(burstOf(runner), r.price !== null ? r.price / 1_000_000 : null, nowMs),
    pillars: pillarsFromMissing(missing),
    rvol: rvolMark(typeof r.rvol_milli === "number" ? r.rvol_milli / 1000 : null),
    news: newsMark(!newsMissing && r.news_first_seen != null, r.news_kind),
    path: runner ? pathMark(runner.kind ?? "runner", runner.at, nowMs, formatTime) : null,
    zone: null,
    stage: s ? stageMark(s.state, s.trigger?.price, s.invalidation?.price, s.meter_milli) : null,
    tail: tailMark(typeof m1 === "number" ? m1 / 100 : null),
  };
}

/**
 * A Runners row's marks. It carries no pillars or news; its path is its own source, with the
 * later paths, the move since the flag and the strategy's reasons folded into that mark's label
 * (they used to be a line of numbers and chips).
 */
export function rocketRowMarks(r: RocketRowWire, nowMs: number, formatTime: (epochMs: number) => string): RowMarks {
  const extra: string[] = [];
  if (typeof r.from_flag_pct === "number") extra.push(`${r.from_flag_pct >= 0 ? "+" : "−"}${Math.abs(r.from_flag_pct).toFixed(1)}% since`);
  if (typeof r.from_high_pct === "number") extra.push(`${Math.abs(r.from_high_pct).toFixed(1)}% off the high`);
  for (const a of r.also ?? []) if (a && Number.isFinite(a.at)) extra.push(`then ${PATH_NAME[a.kind as PathKind] ?? a.kind} at ${formatTime(a.at)}`);
  // Two reasons at most, as the row's chips always showed (rocketRowView).
  if (r.reasons?.length) extra.push(r.reasons.slice(0, 2).join(", "));
  const tracking = rocketBreakoutTracking(r);
  if (tracking) extra.push(tracking);
  // What the tape saw below the $0.10 floor, when the row's entry came right after it rose
  // through it (server rocket/crossing.rs): the same line its notification carried.
  if (typeof r.crossing === "string" && r.crossing) extra.push(r.crossing);
  const lv = r.levels ?? null;
  return {
    flame: flameMark(burstOf(r), r.last, nowMs),
    pillars: null,
    rvol: null,
    news: null,
    path: pathMark(r.kind ?? "runner", r.at, nowMs, formatTime, extra.join(" · ")),
    zone: zoneMark(r.zone),
    stage: lv ? stageMark(lv.state, lv.trigger, lv.invalidation, lv.meter_milli) : null,
    tail: tailMark(r.move_1m_pct),
  };
}

/**
 * A favorite's marks, joined to its Runners row (`runner`) for the burst behind the flame. Its
 * last-minute tail is read off its own day line (the close two 30 s points back against the live
 * price); its pillars are a count (the favorites wire does not say which pass).
 */
export function favoriteRowMarks(r: FavoriteRowWire, runner: RocketRowWire | null | undefined, nowMs: number, formatTime: (epochMs: number) => string): RowMarks {
  const price = typeof r.price === "number" ? r.price : null;
  const line = r.day_line ?? [];
  const back = line.length >= 2 ? line[line.length - 2]! : null;
  const p = r.pillars ?? null;
  const fromWire = r.runners ? pathMark(r.runners.latest_kind || r.runners.kind, r.runners.latest_at ?? r.runners.at, nowMs, formatTime, r.runners.entries > 1 ? `${r.runners.entries} entries today` : "") : null;
  const lv = r.levels ?? null;
  // Levels exist only once a setup has armed, so a trigger without a state reads as armed.
  const lvState = lv ? (lv.state ?? (typeof lv.trigger === "number" ? "forming" : null)) : null;
  return {
    flame: flameMark(burstOf(runner), price !== null ? price / 1_000_000 : null, nowMs),
    pillars: p ? pillarsFromCount(p.passes, p.of) : null,
    rvol: rvolMark(p?.rvol),
    news: p ? newsMark(p.news === "pass", p.news_kind) : null,
    path: runner ? pathMark(runner.kind ?? "runner", runner.at, nowMs, formatTime) : fromWire,
    zone: null,
    stage: lv ? stageMark(lvState, lv.trigger, lv.invalidation, lv.meter_milli) : null,
    tail: price !== null && back ? tailMark((price / back - 1) * 100) : null,
  };
}

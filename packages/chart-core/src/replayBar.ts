import { formatVolume } from "./format.ts";
import { formatEtTime, sessionOf, type SessionBand } from "./time.ts";
import type { Candle, SessionKind } from "./types.ts";

// The replay bar, "B · Two-tier scrubber" (owner-approved 2026-10-24; canvas
// TwoTier / TwoTierPhone / TwoTierFold): a top row of transport, clock, day,
// ticker, jumps and speed, over a full-width scrubber of the trading day with
// the session bands at their true proportions, a volume histogram, the played
// tint, the playhead and the day's alert dots. Everything here is the maths
// both renderers share (chart-web ReplayBar, chart-native ReplayBar), so the
// web, the desktop shell and the phone place, seek and label identically.

/** The speed segments, left to right. The server accepts any whole multiplier 1–3600 (market-replay `Speed`). */
export const REPLAY_SPEEDS = [1, 2, 5, 10, 30] as const;

export const replaySpeedLabel = (s: number) => (s === 0 ? "Max" : `${s}×`);

/**
 * Which artboard a bar of this width uses: "cover" (< 600, the Fold's cover
 * screen and phone-width web: stacked), "fold" (600–1099, the Fold unfolded
 * and narrow windows: two rows around the scrubber) or "desktop" (≥ 1100,
 * where the one-row top tier fits: one row over it).
 */
export type ReplayBarLayout = "cover" | "fold" | "desktop";
export const REPLAY_BAR_FOLD_MIN = 600;
export const REPLAY_BAR_DESKTOP_MIN = 1100;

export function replayBarLayout(width: number): ReplayBarLayout {
  if (width < REPLAY_BAR_FOLD_MIN) return "cover";
  if (width < REPLAY_BAR_DESKTOP_MIN) return "fold";
  return "desktop";
}

/** Hours between scrubber ticks: hourly on the desktop, 2-hourly unfolded, 4-hourly on the cover screen. */
export const SCRUBBER_TICK_HOURS: Record<ReplayBarLayout, number> = { desktop: 1, fold: 2, cover: 4 };
/** Histogram bars across the 16-hour day (7.5, 10 and 15 minutes each), as in the artboards. */
export const SCRUBBER_BARS: Record<ReplayBarLayout, number> = { desktop: 128, fold: 96, cover: 64 };

/** The scrubber's extent: premarket open to after-hours close (04:00–20:00 ET). The overnight band is left out. */
export interface ScrubberSpan {
  start: number;
  end: number;
}

export function scrubberSpan(bands: readonly SessionBand[]): ScrubberSpan | null {
  const pre = bands.find((b) => b.kind === "pre");
  const after = bands.find((b) => b.kind === "after");
  if (!pre || !after || after.end <= pre.start) return null;
  return { start: pre.start, end: after.end };
}

/** t → 0..1 along the scrubber, clamped. */
export function scrubberFraction(span: ScrubberSpan, t: number): number {
  return Math.min(1, Math.max(0, (t - span.start) / (span.end - span.start)));
}

/** A pointer x (px from the scrubber's left edge, of `width` px) → the epoch ms it seeks to, clamped to the span, whole seconds. */
export function scrubberTimeAt(span: ScrubberSpan, width: number, x: number): number {
  if (width <= 0) return span.start;
  const f = Math.min(1, Math.max(0, x / width));
  return Math.round((span.start + f * (span.end - span.start)) / 1000) * 1000;
}

export interface ScrubberSegment {
  kind: Exclude<SessionKind, "overnight">;
  /** "Pre-market", "Regular", "After hours" */
  label: string;
  /** "Pre", "Regular", "After": the cover screen's labels. */
  short: string;
  /** Fractions of the span. */
  from: number;
  to: number;
}

const SEGMENT_LABEL = { pre: ["Pre-market", "Pre"], regular: ["Regular", "Regular"], after: ["After hours", "After"] } as const;

/** The three shaded session bands at their true proportions (5.5 h : 6.5 h : 4 h = 34.4 % : 40.6 % : 25 % on a normal day). */
export function scrubberSegments(bands: readonly SessionBand[]): ScrubberSegment[] {
  const span = scrubberSpan(bands);
  if (!span) return [];
  const out: ScrubberSegment[] = [];
  for (const b of bands) {
    if (b.kind === "overnight") continue;
    const [label, short] = SEGMENT_LABEL[b.kind];
    out.push({ kind: b.kind, label, short, from: scrubberFraction(span, b.start), to: scrubberFraction(span, b.end) });
  }
  return out;
}

export interface ScrubberTick {
  t: number;
  frac: number;
  /** "09:00" */
  label: string;
}

/** Ticks every `everyHours` from 04:00, the last one at 20:00 included when it lands there. */
export function scrubberTicks(span: ScrubberSpan, everyHours: number): ScrubberTick[] {
  const out: ScrubberTick[] = [];
  const step = everyHours * 3_600_000;
  for (let t = span.start; t <= span.end; t += step) out.push({ t, frac: scrubberFraction(span, t), label: formatEtTime(t) });
  return out;
}

export interface ScrubberBar {
  t: number;
  v: number;
  /** 0..1 of the tallest bar, square-root scaled (one burst would flatten the rest on a linear scale). */
  h: number;
}

/** The volume histogram: the candles' volume summed into `n` equal bars across the span. */
export function scrubberBars(candles: readonly Pick<Candle, "t" | "v">[], span: ScrubberSpan, n: number): ScrubberBar[] {
  const width = (span.end - span.start) / n;
  const sums = Array.from({ length: n }, () => 0);
  for (const c of candles) {
    if (c.t < span.start || c.t >= span.end) continue;
    sums[Math.min(n - 1, Math.floor((c.t - span.start) / width))]! += c.v;
  }
  const max = Math.max(0, ...sums);
  return sums.map((v, i) => ({ t: span.start + i * width, v, h: max > 0 ? Math.sqrt(v / max) : 0 }));
}

/** The volume of the bar holding `t` (the hover tooltip), or null outside the span. */
export function scrubberVolumeAt(bars: readonly ScrubberBar[], span: ScrubberSpan, t: number): number | null {
  if (!bars.length || t < span.start || t > span.end) return null;
  const i = Math.min(bars.length - 1, Math.floor(((t - span.start) / (span.end - span.start)) * bars.length));
  return bars[i]!.v;
}

const MINUTE = 60_000;

/**
 * Where a key moves the playhead (the scrubber is a slider): arrows 1 minute
 * (Shift 15), Page Up/Down 30 minutes, Home/End the span's ends. Null for any
 * other key, so the caller lets it through.
 */
export function scrubberKeySeek(key: string, shift: boolean, now: number, span: ScrubberSpan): number | null {
  const step = (shift ? 15 : 1) * MINUTE;
  const clamp = (t: number) => Math.min(span.end, Math.max(span.start, t));
  switch (key) {
    case "ArrowRight":
    case "ArrowUp":
      return clamp(now + step);
    case "ArrowLeft":
    case "ArrowDown":
      return clamp(now - step);
    case "PageUp":
      return clamp(now + 30 * MINUTE);
    case "PageDown":
      return clamp(now - 30 * MINUTE);
    case "Home":
      return span.start;
    case "End":
      return span.end;
    default:
      return null;
  }
}

const SESSION_NAME: Record<SessionKind, string> = { overnight: "Overnight", pre: "Pre-market", regular: "Regular", after: "After hours" };

/** 6_060_000 → "1h 41m"; under an hour "41m". */
export function formatElapsed(ms: number): string {
  const m = Math.max(0, Math.floor(ms / MINUTE));
  const h = Math.floor(m / 60);
  return h > 0 ? `${h}h ${m % 60}m` : `${m}m`;
}

/** "Regular · 1h 41m in": the session the clock is in and how far into it, from the day's bands. */
export function sessionProgress(now: number, bands: readonly SessionBand[]): { kind: SessionKind; name: string; elapsed: string; text: string } {
  const band = bands.find((b) => now >= b.start && now < b.end);
  const kind = band?.kind ?? sessionOf(now);
  const elapsed = band ? formatElapsed(now - band.start) : "";
  return { kind, name: SESSION_NAME[kind], elapsed, text: elapsed ? `${SESSION_NAME[kind]} · ${elapsed} in` : SESSION_NAME[kind] };
}

// ---------------------------------------------------------------------------
// Alert dots: GET /api/replay/alerts?day= (crates/server/src/replay_alerts.rs)

export type ReplayAlertKind = "fast" | "r2" | "run" | "burst" | "running" | "shelf";

export interface ReplayAlert {
  /** Epoch ms it fired. */
  t: number;
  kind: ReplayAlertKind;
  symbol: string;
  price?: number;
  reason?: string;
}

export const REPLAY_ALERT_LABEL: Record<ReplayAlertKind, string> = {
  fast: "Fast move",
  r2: "R2 flag",
  run: "Run +20%",
  burst: "Burst",
  running: "Running",
  shelf: "Volume shelf",
};

/**
 * One hue per kind, distinguishable from each other and from the played tint
 * (the accent), each above 3:1 against the bar's surfaces (non-text contrast).
 * The kind is always also named (aria label, tooltip), never colour alone.
 * The R2 flag is a sky blue, not the accent itself: an accent dot would
 * vanish into the played tint, and the accent stays for play and the tint.
 */
export const REPLAY_ALERT_COLOR: Record<ReplayAlertKind, string> = {
  fast: "#f5b14c",
  r2: "#5fb0ff",
  run: "#3ecf8e",
  burst: "#e07be8",
  running: "#ff8a65",
  shelf: "#c3c9d1",
};

const KINDS = new Set<string>(Object.keys(REPLAY_ALERT_LABEL));

/** The route's body → its alerts, ascending; anything malformed is dropped. Null when the body is not one. */
export function parseReplayAlerts(body: unknown): { day: string; alerts: ReplayAlert[] } | null {
  if (!body || typeof body !== "object") return null;
  const b = body as { day?: unknown; alerts?: unknown };
  if (typeof b.day !== "string" || !Array.isArray(b.alerts)) return null;
  const alerts: ReplayAlert[] = [];
  for (const a of b.alerts as unknown[]) {
    if (!a || typeof a !== "object") continue;
    const x = a as Record<string, unknown>;
    if (typeof x.t !== "number" || typeof x.kind !== "string" || !KINDS.has(x.kind) || typeof x.symbol !== "string") continue;
    alerts.push({
      t: x.t,
      kind: x.kind as ReplayAlertKind,
      symbol: x.symbol.toUpperCase(),
      ...(typeof x.price === "number" ? { price: x.price } : {}),
      ...(typeof x.reason === "string" ? { reason: x.reason } : {}),
    });
  }
  alerts.sort((p, q) => p.t - q.t);
  return { day: b.day, alerts };
}

/** The shown ticker's alerts. */
export function alertsForSymbol(alerts: readonly ReplayAlert[], symbol: string): ReplayAlert[] {
  const s = symbol.toUpperCase();
  return alerts.filter((a) => a.symbol === s);
}

export interface AlertDot {
  alert: ReplayAlert;
  frac: number;
  /** "Fast move at 09:41:15, $0.89" */
  label: string;
}

/** The dots to plot: the alerts inside the span, placed by time. */
export function alertDots(alerts: readonly ReplayAlert[], span: ScrubberSpan): AlertDot[] {
  return alerts
    .filter((a) => a.t >= span.start && a.t <= span.end)
    .map((a) => ({ alert: a, frac: scrubberFraction(span, a.t), label: alertLabel(a) }));
}

export function alertLabel(a: ReplayAlert, withSymbol = false): string {
  const price = a.price !== undefined ? `, $${a.price < 1 ? a.price.toFixed(4) : a.price.toFixed(2)}` : "";
  return `${withSymbol ? `${a.symbol} ` : ""}${REPLAY_ALERT_LABEL[a.kind]} at ${formatEtTime(a.t, true)}${price}`;
}

/**
 * How close to an alert the playhead counts as "on" it. Prev/Next seek to an
 * alert's second, and the clock then reads that second or a little after it
 * (it keeps ticking): without the tolerance Next would step onto the alert just
 * jumped to, and Prev would return to it instead of the one before.
 */
export const ALERT_ON_MS = 2_000;

/** The latest alert before the playhead (more than ALERT_ON_MS before it), or null. */
export function prevAlert(alerts: readonly ReplayAlert[], now: number): ReplayAlert | null {
  let out: ReplayAlert | null = null;
  for (const a of alerts) if (a.t < now - ALERT_ON_MS && (!out || a.t >= out.t)) out = a;
  return out;
}

/** The first alert after the playhead (more than ALERT_ON_MS after it), or null. */
export function nextAlert(alerts: readonly ReplayAlert[], now: number): ReplayAlert | null {
  let out: ReplayAlert | null = null;
  for (const a of alerts) if (a.t > now + ALERT_ON_MS && (!out || a.t < out.t)) out = a;
  return out;
}

/**
 * What the alert jumps act on: the shown ticker's alerts when it has any that
 * day, else every ticker's ("any ticker" mode: a jump then also opens that
 * ticker), so the owner can always practise on the moments alerts fired.
 * `any` is false when the day has no alerts at all, and the jumps disable.
 */
export function alertJumpScope(alerts: readonly ReplayAlert[], symbol: string): { alerts: ReplayAlert[]; any: boolean } {
  const own = alertsForSymbol(alerts, symbol);
  return own.length || !alerts.length ? { alerts: own, any: false } : { alerts: [...alerts], any: true };
}

/** The alert nearest pointer x within `px`, for the tooltip. */
export function alertNear(dots: readonly AlertDot[], width: number, x: number, px = 6): AlertDot | null {
  let best: AlertDot | null = null;
  let dist = px;
  for (const d of dots) {
    const dx = Math.abs(d.frac * width - x);
    if (dx <= dist) {
      dist = dx;
      best = d;
    }
  }
  return best;
}

/** The tooltip line: "12:58:20 · Vol 412K · Fast move". */
export function scrubberTooltip(t: number, volume: number | null, alert: ReplayAlert | null): { time: string; volume: string | null; alert: string | null } {
  return { time: formatEtTime(t, true), volume: volume === null ? null : `Vol ${formatVolume(volume)}`, alert: alert ? REPLAY_ALERT_LABEL[alert.kind] : null };
}

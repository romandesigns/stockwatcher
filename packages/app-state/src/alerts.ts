import { DRAWING_PART_LABELS, etParts, etToUtc, formatEtTime, formatPrice, parseDrawingTolerance, toleranceText, TIMEFRAMES, type Drawing, type DrawingAlertPart, type DrawingTolerance, type DrawingTone, type Timeframe } from "@stockwatcher/chart-core";
import type { AlertEventWire } from "@stockwatcher/shared";
import { parseSymbol } from "./namespaces.ts";

// Price alerts as every client stores and reads them (owner-approved design, 2026-10-19; the
// Strategy board is the spec, docs/architecture/chart-packages.md "Alerts"). The Rust twin is
// crates/server/src/alerts/model.rs: change both together.
//
// - `alerts`        (synced, client-owned) a map alertId → rule, written per entry (settings.patch)
//                   so two devices editing different alerts never overwrite each other.
// - `alerts.state`  (synced, SERVER-owned) the evaluator's verdict per alert: armed, cooling until
//                   t, done, expired. Clients only read it (the server refuses their writes).
// - `alerts.prefs`  (synced, client-owned) the daily phone budget, quiet hours, the sessions
//                   picked last (a new alert starts with them).
// Activity (fires and hold-backs) is not synced: the server keeps it and sends it on connect.
//
// The store keeps values opaque, so everything goes through a parser here, as for every other
// namespace: junk from another build falls back field by field, an alert that cannot be read is
// skipped.

export const ALERTS_NS = { rules: "alerts", state: "alerts.state", prefs: "alerts.prefs" } as const;

export type AlertSession = "pre" | "regular" | "after" | "overnight";
export const ALERT_SESSIONS: readonly AlertSession[] = ["pre", "regular", "after", "overnight"];
export const SESSION_LABELS: Readonly<Record<AlertSession, string>> = { pre: "Pre 4:00", regular: "Regular", after: "After", overnight: "Overnight" };
/** A new alert's sessions until the person picks others (the design's chips): not overnight. */
export const DEFAULT_ALERT_SESSIONS: readonly AlertSession[] = ["pre", "regular", "after"];

export type MoveWindow = "1m" | "5m" | "15m" | "open" | "prevClose";
export const MOVE_WINDOWS: readonly MoveWindow[] = ["1m", "5m", "15m", "open", "prevClose"];
export const MOVE_WINDOW_LABELS: Readonly<Record<MoveWindow, string>> = { "1m": "1 minute", "5m": "5 minutes", "15m": "15 minutes", open: "Since today's open", prevClose: "Since yesterday's close" };

export type AlertCondition =
  | { kind: "price"; dir: "above" | "below"; price: number }
  | { kind: "move"; dir: "up" | "down" | "either"; pct: number; window: MoveWindow }
  | { kind: "halt" }
  /** Within `pct`% of the MODELLED limit-up/down band: every text that shows it says so. */
  | { kind: "luld"; side: "up" | "down" | "either"; pct: number }
  | { kind: "hod" }
  /** The Halt watcher's pressure (0–100) reaches `min` on the buy side, the sell side, or either. */
  | { kind: "pressure"; side: "buy" | "sell" | "either"; min: number }
  /**
   * A drawing alert (2026-11-09): an eligible trade within `tol` of level `part` of drawing
   * `drawingId` (this symbol's synced drawings), on the `tf` chart's slots. The server reads the
   * drawing live, so editing the drawing moves the alert; deleting it orphans the alert.
   */
  | { kind: "drawing"; drawingId: string; part: DrawingAlertPart; tol: DrawingTolerance; tf: Timeframe };

export type AlertExpiry = "day" | "week" | "never";
export const EXPIRY_LABELS: Readonly<Record<AlertExpiry, string>> = { week: "End of week", day: "End of day", never: "Never" };

/** "Every time" gap bounds and default, minutes (the server clamps the same way). */
export const MIN_GAP_MIN = 5;
export const MAX_GAP_MIN = 60;
export const DEFAULT_GAP_MIN = 15;
export const GAP_CHOICES = [5, 10, 15, 30, 60] as const;
export const PHONE_BUDGETS = [5, 10, 20, 50] as const;
export const DEFAULT_PHONE_BUDGET = 20;

export interface AlertRule {
  id: string;
  symbol: string;
  rule: AlertCondition;
  fire: "once" | "every";
  gapMin: number;
  sessions: AlertSession[];
  expiry: AlertExpiry;
  /** Epoch ms, computed here from `expiry` when the alert is created or edited, so every device (and the server) agrees; null = never. */
  expiresAt: number | null;
  notify: { app: boolean; phone: boolean };
  /** The in-app tone for this alert (drawing alerts; default on). Off also sends its phone push without the tone. */
  sound: boolean;
  /** Epoch ms; while in the future the alert neither fires nor records anything. */
  snoozeUntil: number | null;
  createdAt: number;
  /** Bumped on every edit; the server starts an edited alert over armed (moving a finished alert re-arms it). */
  editedAt: number;
}

export type AlertStatus = "armed" | "cooling" | "done" | "expired";

/** One `alerts.state` entry (written by the server, crates/server/src/alerts/model.rs AlertState). */
export interface AlertStateWire {
  status: AlertStatus;
  until?: number;
  movedBack?: boolean;
  basis: number;
  firedAt?: number;
  firedPrice?: number;
  count: number;
  /**
   * The snooze is running, by the server's clock. Present on every entry a server with
   * `clock.v1` wrote: expiry, snooze and the cooling gap are then decided there (crates/server/
   * src/alerts/eval.rs `tick`) and shown as written, never judged against this device's clock.
   */
  snoozed?: boolean;
  /** A drawing alert whose drawing is gone (deleted on some device): it watches nothing until the drawing comes back. */
  orphaned?: true;
}

export interface AlertPrefs {
  phoneBudget: number;
  quietHours: { on: boolean; start: string; end: string };
  /** The sessions picked last: a new alert starts with them ("the sessions you picked last time"). */
  sessions: AlertSession[];
  /**
   * Alert sounds, everywhere (drawing alerts' tones): the global switch and the in-app volume
   * (0–1). Off also sends the phone's push on the plain alerts channel. Quiet hours silence them too.
   */
  sound: { on: boolean; volume: number };
  /**
   * "Hide repeat alerts" (owner, 2026-11-22), **default off**: the server holds a Runners alert for
   * a stock already sent on the same path less than 5 min earlier (crates/server/src/alerts/
   * runners.rs "Limits" 4, held `repeat`, still in the notification log). For every device (it
   * lives in the synced prefs, like quiet hours). Research v2 found it cuts alerts by 38 % but
   * lowers precision, because repeats land on real runners: a comfort choice, not a quality gain.
   */
  hideRepeats: boolean;
  /**
   * "Group repeat alerts" (owner, 2026-11-25, after AKAN 11-24 rang 47 times), **default on**: a
   * stock rings once per alert family (breakout, follow-up, MP5, shelf, halt watch) per session part
   * (pre-market, regular, after hours); its later alerts of that family only update the stock's card
   * ("12 alerts · 10:37", crates/server/src/alerts/runners.rs "Limits" 4a, held `grouped`). The
   * first alert's timing is unchanged. Off restores one notification per event.
   */
  groupRepeats: boolean;
}

export const DEFAULT_SOUND_VOLUME = 0.6;
export const DEFAULT_ALERT_PREFS: AlertPrefs = { phoneBudget: DEFAULT_PHONE_BUDGET, quietHours: { on: false, start: "22:00", end: "07:00" }, sessions: [...DEFAULT_ALERT_SESSIONS], sound: { on: true, volume: DEFAULT_SOUND_VOLUME }, hideRepeats: false, groupRepeats: true };

const isObj = (v: unknown): v is Record<string, unknown> => typeof v === "object" && v !== null && !Array.isArray(v);
const num = (v: unknown): number | null => (typeof v === "number" && Number.isFinite(v) ? v : null);
const pick = <T extends string>(v: unknown, list: readonly T[], d: T): T => ((list as readonly unknown[]).includes(v) ? (v as T) : d);

function parseSessions(v: unknown): AlertSession[] {
  if (!Array.isArray(v)) return [...DEFAULT_ALERT_SESSIONS];
  return ALERT_SESSIONS.filter((s) => v.includes(s));
}

export function parseCondition(v: unknown): AlertCondition | null {
  if (!isObj(v)) return null;
  switch (v.kind) {
    case "price": {
      const price = num(v.price);
      return price !== null && price > 0 ? { kind: "price", dir: v.dir === "below" ? "below" : "above", price } : null;
    }
    case "move": {
      const pct = num(v.pct);
      if (pct === null || pct <= 0 || pct > 1000) return null;
      return { kind: "move", dir: pick(v.dir, ["up", "down", "either"] as const, "up"), pct, window: pick(v.window, MOVE_WINDOWS, "5m") };
    }
    case "halt":
      return { kind: "halt" };
    case "luld": {
      const pct = num(v.pct);
      return pct !== null && pct > 0 && pct <= 100 ? { kind: "luld", side: pick(v.side, ["up", "down", "either"] as const, "up"), pct } : null;
    }
    case "hod":
      return { kind: "hod" };
    case "pressure": {
      const min = num(v.min);
      return min !== null && Number.isInteger(min) && min >= 1 && min <= 100 ? { kind: "pressure", side: pick(v.side, ["buy", "sell", "either"] as const, "buy"), min } : null;
    }
    case "drawing": {
      const id = typeof v.drawingId === "string" && v.drawingId.length > 0 && v.drawingId.length <= 128 ? v.drawingId : null;
      if (!id) return null;
      return { kind: "drawing", drawingId: id, part: pick(v.part, ["line", "entry", "target", "stop"] as const, "line"), tol: parseDrawingTolerance(v.tol), tf: pick(v.tf, TIMEFRAMES, "1m") };
    }
    default:
      return null;
  }
}

export function parseAlertRule(id: string, raw: unknown): AlertRule | null {
  if (!isObj(raw) || !id) return null;
  const symbol = parseSymbol(raw.symbol);
  const rule = parseCondition(raw.rule);
  if (!symbol || !rule) return null;
  const createdAt = num(raw.createdAt) ?? 0;
  const notify = isObj(raw.notify) ? raw.notify : {};
  const expiry = pick(raw.expiry, ["day", "week", "never"] as const, "week");
  return {
    id,
    symbol,
    rule,
    fire: raw.fire === "every" ? "every" : "once",
    gapMin: Math.min(MAX_GAP_MIN, Math.max(MIN_GAP_MIN, Math.round(num(raw.gapMin) ?? DEFAULT_GAP_MIN))),
    sessions: parseSessions(raw.sessions),
    expiry,
    expiresAt: expiry === "never" ? null : num(raw.expiresAt),
    notify: { app: notify.app !== false, phone: notify.phone !== false },
    sound: raw.sound !== false,
    snoozeUntil: num(raw.snoozeUntil),
    createdAt,
    editedAt: num(raw.editedAt) ?? createdAt,
  };
}

/** Every alert, oldest first. */
export function parseAlertRules(raw: unknown): AlertRule[] {
  if (!isObj(raw)) return [];
  const out: AlertRule[] = [];
  for (const [id, v] of Object.entries(raw)) {
    const r = parseAlertRule(id, v);
    if (r) out.push(r);
  }
  return out.sort((a, b) => a.createdAt - b.createdAt || a.id.localeCompare(b.id));
}

export function parseAlertStates(raw: unknown): Record<string, AlertStateWire> {
  const out: Record<string, AlertStateWire> = {};
  if (!isObj(raw)) return out;
  for (const [id, v] of Object.entries(raw)) {
    if (!isObj(v)) continue;
    const status = pick(v.status, ["armed", "cooling", "done", "expired"] as const, "armed");
    out[id] = {
      status,
      ...(num(v.until) !== null ? { until: num(v.until)! } : {}),
      ...(v.movedBack === true ? { movedBack: true } : {}),
      basis: num(v.basis) ?? 0,
      ...(num(v.firedAt) !== null ? { firedAt: num(v.firedAt)! } : {}),
      ...(num(v.firedPrice) !== null ? { firedPrice: num(v.firedPrice)! } : {}),
      count: num(v.count) ?? 0,
      ...(typeof v.snoozed === "boolean" ? { snoozed: v.snoozed } : {}),
      ...(v.orphaned === true ? { orphaned: true as const } : {}),
    };
  }
  return out;
}

const HHMM = /^([01]\d|2[0-3]):[0-5]\d$/;

export function parseAlertPrefs(raw: unknown): AlertPrefs {
  if (!isObj(raw)) return DEFAULT_ALERT_PREFS;
  const q = isObj(raw.quietHours) ? raw.quietHours : {};
  const start = typeof q.start === "string" && HHMM.test(q.start) ? q.start : DEFAULT_ALERT_PREFS.quietHours.start;
  const end = typeof q.end === "string" && HHMM.test(q.end) ? q.end : DEFAULT_ALERT_PREFS.quietHours.end;
  const sessions = Array.isArray(raw.sessions) ? parseSessions(raw.sessions) : [...DEFAULT_ALERT_SESSIONS];
  const snd = isObj(raw.sound) ? raw.sound : {};
  const vol = num(snd.volume);
  return {
    phoneBudget: (PHONE_BUDGETS as readonly unknown[]).includes(raw.phoneBudget) ? (raw.phoneBudget as number) : DEFAULT_PHONE_BUDGET,
    quietHours: { on: q.on === true, start, end },
    sessions: sessions.length ? sessions : [...DEFAULT_ALERT_SESSIONS],
    sound: { on: snd.on !== false, volume: vol === null ? DEFAULT_SOUND_VOLUME : Math.min(1, Math.max(0, vol)) },
    // Only an explicit true (the server's rule, model.rs parse_prefs): absent is off.
    hideRepeats: raw.hideRepeats === true,
    // Only an explicit false (model.rs parse_prefs): absent is on.
    groupRepeats: raw.groupRepeats !== false,
  };
}

/** Whether `now` falls in the quiet hours (ET wall time; start > end wraps midnight), as the server's QuietHours::contains. */
export function inQuietHours(prefs: Pick<AlertPrefs, "quietHours">, now = Date.now()): boolean {
  const q = prefs.quietHours;
  if (!q.on || !HHMM.test(q.start) || !HHMM.test(q.end)) return false;
  const m = (s: string) => Number(s.slice(0, 2)) * 60 + Number(s.slice(3, 5));
  const start = m(q.start);
  const end = m(q.end);
  if (start === end) return false;
  const p = etParts(now);
  const cur = p.hour * 60 + p.minute;
  return start < end ? cur >= start && cur < end : cur >= start || cur < end;
}

/**
 * The tone a fire should play in the app, or null for silence: only a drawing alert's fire (it
 * carries a tone), with its own sound chip on, the global switch on, and outside quiet hours.
 * Played once per event (hosts key it by the event id).
 */
export function alertSoundFor(event: Pick<AlertEventWire, "tone" | "alertId">, rule: Pick<AlertRule, "sound"> | undefined, prefs: AlertPrefs, now = Date.now()): DrawingTone | null {
  if (!event.tone || !rule?.sound || !prefs.sound.on || prefs.sound.volume <= 0) return null;
  if (inQuietHours(prefs, now)) return null;
  return event.tone;
}

/**
 * The three tones, one score for every client (owner decision 2026-11-09: distinct for up-touch,
 * down-touch and stop). Web plays them with WebAudio; the phone's channels use WAV files rendered
 * from the same notes (apps/mobile/scripts/tones.ts). Each note: frequency Hz, length ms.
 * Rising for up, falling for down, a repeated low pair for the stop, so they are told apart
 * without looking.
 */
export const ALERT_TONES: Readonly<Record<DrawingTone, readonly { hz: number; ms: number }[]>> = {
  up: [
    { hz: 660, ms: 110 },
    { hz: 880, ms: 160 },
  ],
  down: [
    { hz: 880, ms: 110 },
    { hz: 660, ms: 160 },
  ],
  stop: [
    { hz: 440, ms: 120 },
    { hz: 0, ms: 60 },
    { hz: 440, ms: 120 },
    { hz: 0, ms: 60 },
    { hz: 330, ms: 220 },
  ],
};

/** Activity, newest first, without duplicate ids (a resent snapshot replaces, an update replaces in place). */
export function mergeActivity(current: readonly AlertEventWire[], incoming: readonly AlertEventWire[], replace: boolean): AlertEventWire[] {
  const byId = new Map<string, AlertEventWire>();
  for (const e of replace ? [] : current) byId.set(e.id, e);
  for (const e of incoming) if (isObj(e) && typeof e.id === "string") byId.set(e.id, e);
  return [...byId.values()].sort((a, b) => b.at - a.at).slice(0, 500);
}

// --- time -----------------------------------------------------------------------------------

const DAY_MS = 86_400_000;
/** The end of a trading day for expiry: 20:00 ET, the end of the after-hours session. */
const DAY_END_HOUR = 20;

/** Epoch ms of 20:00 ET on the ET calendar day `daysAhead` after `t`'s. */
function endOfEtDay(t: number, daysAhead: number): number {
  const p = etParts(t + daysAhead * DAY_MS);
  return etToUtc(p.year, p.month, p.day, DAY_END_HOUR);
}

const WEEKDAY: Readonly<Record<string, number>> = { Sun: 0, Mon: 1, Tue: 2, Wed: 3, Thu: 4, Fri: 5, Sat: 6 };

/**
 * When an alert set at `now` expires: end of day = 20:00 ET today (after 20:00 or on a weekend,
 * the next weekday's); end of week = 20:00 ET Friday (from Friday 20:00 on, next week's). Market
 * holidays are not skipped: an alert ending on one simply ends then.
 */
export function expiresAtFor(expiry: AlertExpiry, now: number): number | null {
  if (expiry === "never") return null;
  const wd = WEEKDAY[etParts(now).weekday] ?? 1;
  if (expiry === "day") {
    let ahead = now >= endOfEtDay(now, 0) ? 1 : 0;
    while ([0, 6].includes((wd + ahead) % 7)) ahead++;
    return endOfEtDay(now, ahead);
  }
  let ahead = (5 - wd + 7) % 7;
  if (ahead === 0 && now >= endOfEtDay(now, 0)) ahead = 7;
  return endOfEtDay(now, ahead);
}

/** "Fri", "today 20:00", "Oct 24": when an expiry reads best. */
export function expiryText(at: number, now: number): string {
  if (at - now < DAY_MS && etParts(at).day === etParts(now).day) return `today ${formatEtTime(at)}`;
  if (at - now < 6 * DAY_MS) return etParts(at).weekday;
  return new Date(at).toLocaleDateString("en-US", { timeZone: "America/New_York", month: "short", day: "numeric" });
}

// --- creating ---------------------------------------------------------------------------------

let seq = 0;
export const newAlertId = (now = Date.now()) => `al-${now.toString(36)}-${(seq++).toString(36)}-${Math.floor(Math.random() * 1296).toString(36)}`;

/** Above the last price means "crosses above", below "crosses below" (the design's smart default). */
export function crossDirFor(price: number, last: number | null): "above" | "below" {
  return last === null || price >= last ? "above" : "below";
}

/** A new alert with the design's defaults: Once, end of this week, the sessions picked last, in app + phone. */
export function newAlert(symbol: string, rule: AlertCondition, prefs: AlertPrefs, now = Date.now(), id = newAlertId(now)): AlertRule {
  return {
    id,
    symbol: symbol.trim().toUpperCase(),
    rule,
    fire: "once",
    gapMin: DEFAULT_GAP_MIN,
    sessions: [...prefs.sessions],
    expiry: "week",
    expiresAt: expiresAtFor("week", now),
    notify: { app: true, phone: true },
    sound: true,
    snoozeUntil: null,
    createdAt: now,
    editedAt: now,
  };
}

/**
 * A new drawing alert with its defaults (owner decisions 2026-11-09): touch within one tick,
 * Once, the sessions picked last plus After (the after-hours mental stop is the reason it exists;
 * Overnight stays a choice), never expiring (a stop that silently expired is worse than one
 * that was never set), in app + phone, with its sound. `tf` is the chart the alert was set on:
 * its slots place a sloped line.
 */
export function newDrawingAlert(symbol: string, drawing: Pick<Drawing, "id">, part: DrawingAlertPart, tf: Timeframe, prefs: AlertPrefs, now = Date.now(), id = newAlertId(now)): AlertRule {
  const base = newAlert(symbol, { kind: "drawing", drawingId: drawing.id, part, tol: { unit: "ticks", value: 1 }, tf }, prefs, now, id);
  const sessions = ALERT_SESSIONS.filter((s) => s === "after" || base.sessions.includes(s));
  return { ...base, sessions, expiry: "never", expiresAt: null };
}

/** The alerts watching a drawing (any of its levels). */
export function alertsOnDrawing(rules: readonly AlertRule[], symbol: string, drawingId: string): AlertRule[] {
  return rules.filter((r) => r.symbol === symbol && r.rule.kind === "drawing" && r.rule.drawingId === drawingId);
}

/** A rule after an edit: `editedAt` moves on (the server re-arms it) and the expiry is recomputed from now. */
export function editedAlert(rule: AlertRule, patch: Partial<Omit<AlertRule, "id" | "createdAt" | "editedAt">>, now = Date.now()): AlertRule {
  const next = { ...rule, ...patch, editedAt: Math.max(now, rule.editedAt + 1) };
  if (patch.expiry !== undefined || patch.rule !== undefined) next.expiresAt = expiresAtFor(next.expiry, now);
  return next;
}

// --- text -------------------------------------------------------------------------------------

const p = (x: number) => formatPrice(x);
const pctText = (x: number) => `${Number.isInteger(x) ? x : x.toFixed(1)}%`;

/** "Price crosses above 2.50", "Up 5% in 5 minutes", "Within 2% of the limit-up band (modelled)". */
export function conditionText(c: AlertCondition): string {
  switch (c.kind) {
    case "price":
      return `Price crosses ${c.dir} ${p(c.price)}`;
    case "move": {
      const dir = c.dir === "up" ? "Up" : c.dir === "down" ? "Down" : "Up or down";
      const w = c.window === "open" ? "from today's open" : c.window === "prevClose" ? "from yesterday's close" : `in ${c.window === "1m" ? "1 minute" : c.window === "5m" ? "5 minutes" : "15 minutes"}`;
      return `${dir} ${pctText(c.pct)} ${w}`;
    }
    case "halt":
      return "Halted or resumed";
    case "luld":
      return `Within ${pctText(c.pct)} of the ${c.side === "up" ? "limit-up" : c.side === "down" ? "limit-down" : "limit-up/down"} band (modelled)`;
    case "hod":
      return "New high of day";
    case "pressure":
      return `${c.side === "buy" ? "Buy pressure" : c.side === "sell" ? "Sell pressure" : "Pressure"} ${c.min} or more (Halt watcher)`;
    case "drawing":
      return `Touches the drawing's ${c.part === "line" ? "line" : DRAWING_PART_LABELS[c.part].toLowerCase()} (${toleranceText(c.tol)}, ${c.tf} chart)`;
  }
}

/** The phone list's short form: "Above 2.50", "Up 5% in 5 min", "Near limit-up band". */
export function shortConditionText(c: AlertCondition): string {
  switch (c.kind) {
    case "price":
      return `${c.dir === "above" ? "Above" : "Below"} ${p(c.price)}`;
    case "move": {
      const dir = c.dir === "up" ? "Up" : c.dir === "down" ? "Down" : "Moves";
      const w = c.window === "open" ? "from open" : c.window === "prevClose" ? "from prev close" : `in ${c.window.replace("m", " min")}`;
      return `${dir} ${pctText(c.pct)} ${w}`;
    }
    case "halt":
      return "Halted or resumed";
    case "luld":
      return `Near ${c.side === "down" ? "limit-down" : "limit-up"} band`;
    case "hod":
      return "New high of day";
    case "pressure":
      return `Pressure ${c.min}+`;
    case "drawing":
      return c.part === "line" ? "Touches drawing" : `Touches ${c.part}`;
  }
}

export function fireText(r: Pick<AlertRule, "fire" | "gapMin">): string {
  return r.fire === "once" ? "Once" : `Every time, ${r.gapMin} min apart`;
}

/** "Once · pre, regular, after · expires Fri". */
export function metaText(r: AlertRule, now = Date.now()): string {
  const sessions = r.sessions.length === ALERT_SESSIONS.length ? "all sessions" : r.sessions.length ? r.sessions.join(", ") : "no sessions";
  const parts = [fireText(r), sessions];
  if (r.rule.kind === "luld") parts.push("modelled band");
  if (r.rule.kind === "drawing" && !r.sound) parts.push("sound off");
  if (r.snoozeUntil !== null && r.snoozeUntil > now) parts.push(`snoozed until ${expiryText(r.snoozeUntil, now)}`);
  else if (r.expiresAt !== null) parts.push(`expires ${expiryText(r.expiresAt, now)}`);
  return parts.join(" · ");
}

/** The chart line's tag: "above 2.50 · once", "below 2.10 · every time". */
export function lineLabel(r: AlertRule): string {
  if (r.rule.kind !== "price") return shortConditionText(r.rule);
  return `${r.rule.dir} ${p(r.rule.price)} · ${r.fire === "once" ? "once" : "every time"}`;
}

export type AlertDisplayKind = "armed" | "cooling" | "snoozed" | "fired" | "expired" | "orphaned";

/** The state chip: "Armed", "Cooling · 9 min", "Snoozed", "Fired 10:42", "Expired". */
export function alertDisplay(r: AlertRule, s: AlertStateWire | undefined, now = Date.now()): { kind: AlertDisplayKind; label: string } {
  // A state written for an older version of the rule does not apply: the server re-arms an edit.
  const st = s && s.basis >= r.editedAt ? s : undefined;
  // The server says the drawing is gone: whatever else holds, it watches nothing.
  if (st?.orphaned && st.status !== "expired") return { kind: "orphaned", label: "Drawing deleted" };
  // Decided by the server's clock: shown as written. `now` only measures the cooling countdown.
  if (st && typeof st.snoozed === "boolean") {
    if (st.status === "expired") return { kind: "expired", label: "Expired" };
    if (st.status === "done") return { kind: "fired", label: st.firedAt ? `Fired ${formatEtTime(st.firedAt)}` : "Fired" };
    if (st.snoozed) return { kind: "snoozed", label: "Snoozed" };
    if (st.status === "cooling") {
      const left = st.until !== undefined ? st.until - now : 0;
      return { kind: "cooling", label: left > 0 ? `Cooling · ${Math.max(1, Math.ceil(left / 60_000))} min` : "Waiting for a 0.5% move back" };
    }
    return { kind: "armed", label: "Armed" };
  }
  if (st?.status === "expired" || (r.expiresAt !== null && now >= r.expiresAt && st?.status !== "done")) return { kind: "expired", label: "Expired" };
  if (st?.status === "done") return { kind: "fired", label: st.firedAt ? `Fired ${formatEtTime(st.firedAt)}` : "Fired" };
  if (r.snoozeUntil !== null && r.snoozeUntil > now) return { kind: "snoozed", label: "Snoozed" };
  if (st?.status === "cooling") {
    const left = st.until !== undefined ? st.until - now : 0;
    if (left > 0) return { kind: "cooling", label: `Cooling · ${Math.max(1, Math.ceil(left / 60_000))} min` };
    return { kind: "cooling", label: "Waiting for a 0.5% move back" };
  }
  return { kind: "armed", label: "Armed" };
}

/** Still watching (armed or cooling, snoozed included): drawn on the chart, counted as active. */
export function isActiveAlert(r: AlertRule, s: AlertStateWire | undefined, now = Date.now()): boolean {
  const k = alertDisplay(r, s, now).kind;
  return k !== "fired" && k !== "expired" && k !== "orphaned";
}

/** Phone budget text: "7 / 20". */
export function budgetText(today: number, budget: number): string {
  return `${today} / ${budget}`;
}

/** The ET date key ("2026-10-19") of an instant, for grouping Activity by day. */
export function etDayKey(t: number): string {
  const q = etParts(t);
  return `${q.year}-${String(q.month).padStart(2, "0")}-${String(q.day).padStart(2, "0")}`;
}

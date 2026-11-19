// Runners (and, since 2026-10-23, Ross, Signals and Halt watcher) system notifications on the
// desktop and the web (owner, 2026-10-23: "the same notifications as the phone, and a click takes
// me to the chart"). The server decides once (crates/server/src/alerts/runners.rs `desk_frame`,
// the phone's limits with their own count) and sends every socket a `runners.notice` carrying the
// phone push's own title and body; a client that sent `runners.notify` shows it when its switch
// for that kind is on, once per `id`.

import { parseBreakoutNotice } from "./breakout.ts";

/**
 * `welcome.features` entry (crates/server/src/alerts/runners.rs `NOTICE_FEATURE`): this server
 * sends `runners.notice` to a connection that opted in with `runners.notify`. The phone rings its
 * local banner (no push token) from these frames when it sees it, and diffs Runners panels itself
 * only against an older server. The desktop and web always listen: servers sent the frames
 * before the flag existed.
 */
export const RUNNERS_NOTICE_FEATURE = "runners.notice.v1";

/**
 * The kinds a notice can be about (the phone push's `data.kind`). Since 2026-10-23 every panel
 * event backed by evidence notifies, not only Runners: Ross and Signals crossings and at-trigger,
 * a Runners stock entering a volume shelf, a later path on a stock already in Runners, and the
 * Halt watcher's gauge appearing.
 */
export const RUNNERS_NOTICE_KINDS = [
  "runners.new",
  "runners.breakout",
  "runners.burst",
  "runners.running",
  "runners.run",
  "runners.fast",
  "runners.shelf",
  "runners.followup",
  "ross.crossing",
  "signals.crossing",
  "ross.trigger",
  "signals.trigger",
  "halt.watch",
  // "Pullback setting up" and its crossing (owner, 2026-10-25): one switch, `data.stage`
  // "setup" | "crossing" (crates/server/src/cue_notices.rs, "Pullback").
  "runners.pullback",
  // Exchange halts and resumptions of a covered stock (plan "halt sounds", 2026-11-16;
  // crates/server/src/alerts/halts.rs): the SIP trading status, not the Halt watcher's forecast.
  "halt.halted",
  "halt.resumed",
  // The N1 "early leg" SHADOW detector (owner, 2026-11-17; crates/server/src/n1_live): research's
  // Engine A entrant running beside production to be measured, never instead of it. Its own
  // switch, off by default. An older client drops the kind (`parseRunnersNotice`).
  "shadow.n1",
  // Engine A's T2-c4 (owner, 2026-11-18; crates/server/src/c4_live): F1u "first pullback after
  // ignition", a research detector like N1, and the HALT add-on, a buying-side Halt watcher gauge
  // 9:30–11:30. Each its own switch, on by default.
  "shadow.f1u",
  "halt.bull",
] as const;
export type RunnersNoticeKind = (typeof RUNNERS_NOTICE_KINDS)[number];

/** server `runners.notice`: one notice the desk limits let through. */
export interface RunnersNoticeMessage {
  type: "runners.notice";
  /** The server's event id (`runners-<kind>-<alertId>`; a breakout adds its barrier second): what a client dedupes on. */
  id: string;
  alertId: string;
  symbol: string;
  kind: RunnersNoticeKind;
  /** Exactly the phone push's title and body. */
  title: string;
  body: string;
  /** The phone push's data `{symbol, kind, alert_id}`. */
  data?: Record<string, unknown>;
  /** Per-kind source/event clock, epoch ms (Breakout: scheduled bar end; never decision time). */
  at: number;
  /**
   * A Ross notice (`ross.crossing` / `ross.trigger`) about a stock the Signals panel shows too.
   * The server sends ONE notice for the one episode, never a Ross and a Signals one, so a client
   * with only the Signals switch on still shows it ([`wantsNotice`]).
   */
  alsoSignals?: boolean;
}

/** One client's switches (the phone's Settings → Alerts names). */
export interface RunnersNotifyKinds {
  new: boolean;
  breakout: boolean;
  burst: boolean;
  running: boolean;
  run: boolean;
  fast: boolean;
  /** A Runners stock came down into a volume shelf. */
  shelf: boolean;
  /** A later Runners path fired on a stock already listed. */
  followUp: boolean;
  rossCrossing: boolean;
  signalsCrossing: boolean;
  rossTrigger: boolean;
  signalsTrigger: boolean;
  /** The Halt watcher's gauge appeared for a stock. */
  halt: boolean;
  /** A Runners stock's first pullback within 30 min of its alert (or a Ross stock's first of the day) armed its trigger and stop, then its trigger crossing. */
  pullback: boolean;
  /** The exchange halted trading in a covered stock (on the Runners board, on a chart, or with a halt rule). */
  halted: boolean;
  /** The same stock's trading resumed. */
  resumed: boolean;
  /** The N1 detector fired: +8% off a 3-minute low on heavy trading, 9:30–11:30 ET (Engine A research, beside production). */
  n1: boolean;
  /** F1u fired: a stock's first pullback after a +10% ignition crossed back up, 9:30–11:30 ET (Engine A T2-c4). */
  f1u: boolean;
  /** The Halt watcher's gauge appeared on the buying side, 9:30–11:30 ET (Engine A T2-c4); sent once if `halt` is on too. */
  haltBull: boolean;
}

/**
 * The phone's defaults (crates/server/src/alerts/push.rs `register`): every evidence kind on;
 * breakouts, at-trigger and the Halt watcher off (opt-in).
 */
export const DEFAULT_RUNNERS_NOTIFY: Readonly<RunnersNotifyKinds> = Object.freeze({
  new: true,
  breakout: false,
  burst: true,
  running: true,
  run: true,
  fast: true,
  shelf: true,
  followUp: true,
  rossCrossing: true,
  signalsCrossing: true,
  rossTrigger: false,
  signalsTrigger: false,
  halt: false,
  // On: the owner asked to receive it (2026-10-25).
  pullback: true,
  // On: plan "halt sounds" (2026-11-16).
  halted: true,
  resumed: true,
  // On since T2-c6 was confirmed (owner, 2026-11-17: "push live").
  n1: true,
  // On: Engine A T2-c4 (owner, 2026-11-18).
  f1u: true,
  haltBull: true,
});

/** client `runners.notify`: this connection shows notifications for these kinds (`enabled: false` stops). */
export interface RunnersNotifyMessage extends Partial<RunnersNotifyKinds> {
  type: "runners.notify";
  enabled?: boolean;
}

/** A stored or partial value as a full set of switches (anything unreadable or missing takes the default). */
export function runnersNotifyKinds(v: unknown): RunnersNotifyKinds {
  const o = v && typeof v === "object" ? (v as Record<string, unknown>) : {};
  const out = { ...DEFAULT_RUNNERS_NOTIFY };
  for (const k of Object.keys(out) as (keyof RunnersNotifyKinds)[]) if (typeof o[k] === "boolean") out[k] = o[k];
  return out;
}

/** The switch each kind answers to. */
export const RUNNERS_NOTIFY_KEY: Readonly<Record<RunnersNoticeKind, keyof RunnersNotifyKinds>> = Object.freeze({
  "runners.new": "new",
  "runners.breakout": "breakout",
  "runners.burst": "burst",
  "runners.running": "running",
  "runners.run": "run",
  "runners.fast": "fast",
  "runners.shelf": "shelf",
  "runners.followup": "followUp",
  "ross.crossing": "rossCrossing",
  "signals.crossing": "signalsCrossing",
  "ross.trigger": "rossTrigger",
  "signals.trigger": "signalsTrigger",
  "halt.watch": "halt",
  "runners.pullback": "pullback",
  "halt.halted": "halted",
  "halt.resumed": "resumed",
  "shadow.n1": "n1",
  "shadow.f1u": "f1u",
  "halt.bull": "haltBull",
});

/** Whether a notice is an exchange halt or resumption (its own sound on the desktop and web). */
export function isHaltNotice(kind: string): kind is "halt.halted" | "halt.resumed" {
  return kind === "halt.halted" || kind === "halt.resumed";
}

/** The switch a notice's kind answers to. */
export function runnersNotifyKey(kind: RunnersNoticeKind): keyof RunnersNotifyKinds {
  return RUNNERS_NOTIFY_KEY[kind];
}

/**
 * Whether the switches want `notice`: its kind's switch or, for a Ross notice about a stock the
 * Signals panel shows too (`alsoSignals`), the Signals twin's switch.
 */
export function wantsNotice(notice: Pick<RunnersNoticeMessage, "kind" | "alsoSignals">, kinds: RunnersNotifyKinds): boolean {
  // One gauge, one notification: with "Gauge appears" on, its Halt watcher notice already says it
  // (the server's DeskKinds::wants applies the same rule).
  if (notice.kind === "halt.bull") return kinds.haltBull && !kinds.halt;
  if (kinds[runnersNotifyKey(notice.kind)]) return true;
  if (!notice.alsoSignals) return false;
  // A Runners stock's pullback crossing stands for the Signals crossing too, as a Ross one does.
  if (notice.kind === "ross.crossing" || notice.kind === "runners.pullback") return kinds.signalsCrossing;
  if (notice.kind === "ross.trigger") return kinds.signalsTrigger;
  return false;
}

/** A ticker as the wire spells it (the phone's route.ts rule). */
const SYMBOL = /^[A-Z][A-Z0-9.-]{0,11}$/;

/** A frame read defensively: null for anything that is not a well-formed notice. */
export function parseRunnersNotice(v: unknown): RunnersNoticeMessage | null {
  if (!v || typeof v !== "object") return null;
  const m = v as Record<string, unknown>;
  if (m.type !== "runners.notice") return null;
  const str = (k: string) => (typeof m[k] === "string" ? (m[k] as string) : "");
  const kind = str("kind") as RunnersNoticeKind;
  const symbol = str("symbol").trim().toUpperCase();
  const id = str("id");
  if (!id || !SYMBOL.test(symbol) || !RUNNERS_NOTICE_KINDS.includes(kind) || !str("title")) return null;
  const data = m.data && typeof m.data === "object" && !Array.isArray(m.data) ? { ...(m.data as Record<string, unknown>) } : undefined;
  if (data && "breakout" in data) {
    const breakout = kind === "runners.breakout" ? parseBreakoutNotice(data.breakout) : undefined;
    if (breakout) data.breakout = breakout;
    else delete data.breakout;
  }
  const out: RunnersNoticeMessage = { type: "runners.notice", id, alertId: str("alertId"), symbol, kind, title: str("title"), body: str("body"), data, at: typeof m.at === "number" ? m.at : 0 };
  if (m.alsoSignals === true || data?.also_signals === true) out.alsoSignals = true;
  return out;
}

/**
 * The ids already shown, so the same client never shows one twice (a reconnect, a second socket,
 * a frame replayed). Bounded: the oldest go first. `load`/`save` persist it (the web keeps it in
 * localStorage, so a reload or a second tab does not show it again either).
 */
export interface NoticeDedupe {
  /** True the first time `id` is seen (and records it); false after. */
  claim(id: string): boolean;
}

export function createNoticeDedupe(opts: { max?: number; load?: () => string[]; save?: (ids: string[]) => void } = {}): NoticeDedupe {
  const max = opts.max ?? 500;
  // The session's own memory: the ids even when storage fails (private mode, quota).
  const mem = new Set<string>();
  return {
    claim(id) {
      if (mem.has(id)) return false;
      // Re-read every time: another tab may have claimed it since.
      let ids: string[] = [];
      try {
        ids = opts.load?.() ?? [];
      } catch {
        // Unreadable storage: this session's memory still holds.
      }
      mem.add(id);
      if (ids.includes(id)) return false;
      ids.push(id);
      try {
        opts.save?.(ids.slice(-max));
      } catch {
        // Full or blocked storage: as above.
      }
      if (mem.size > max) mem.delete(mem.values().next().value as string);
      return true;
    },
  };
}

/** Whether this client shows `notice`: its switch is on and it was not shown before. */
export function shouldShowNotice(notice: RunnersNoticeMessage, kinds: RunnersNotifyKinds, dedupe: NoticeDedupe): boolean {
  return wantsNotice(notice, kinds) && dedupe.claim(notice.id);
}

/** One notification switch as every client shows it (icons are chart-core icon names). */
/**
 * The icons a notice kind can carry: a subset of chart-core's two-tone `IconName` (this package
 * does not depend on chart-core; the web and the phone pass these to their `Icon`, so tsc there
 * rejects a name chart-core lacks).
 */
export type AlertIcon = "bell" | "gauge" | "trend" | "hline" | "levels" | "plus" | "star" | "pause" | "play" | "insights" | "pathRunner" | "pathBurst" | "pathFast" | "pathRun" | "pathRunning";

/**
 * THE icon of each kind of notice, by the switch it answers to (owner, 2026-10-30: the
 * Insights log showed Follow-ups with the volume shelf's stacked lines and Pullback with the
 * crossings' slanted line, while Settings → Alerts gave them ⊕ and —•—). Settings → Alerts
 * ({@link ALERT_SWITCH_SECTIONS}) and the notification log ({@link noticeIcon}) both read this
 * one table on the web and the phone, so they cannot diverge again. The Runners paths use the
 * board's own path marks (RowMarks), so a log row, its switch and its board chip match.
 */
export const ALERT_SWITCH_ICON: Readonly<Record<keyof RunnersNotifyKinds, AlertIcon>> = Object.freeze({
  new: "pathRunner",
  breakout: "gauge",
  burst: "pathBurst",
  running: "pathRunning",
  run: "pathRun",
  fast: "pathFast",
  followUp: "plus",
  pullback: "hline",
  shelf: "levels",
  rossCrossing: "trend",
  rossTrigger: "hline",
  signalsCrossing: "trend",
  signalsTrigger: "hline",
  halt: "gauge",
  halted: "pause",
  resumed: "play",
  n1: "insights",
  f1u: "insights",
  haltBull: "gauge",
});

/** A Runners-pipeline notice kind's icon: its switch's ({@link ALERT_SWITCH_ICON}). */
export function runnersNoticeIcon(kind: RunnersNoticeKind): AlertIcon {
  return ALERT_SWITCH_ICON[runnersNotifyKey(kind)];
}

export interface AlertSwitch {
  key: keyof RunnersNotifyKinds;
  icon: AlertIcon;
  name: string;
  description: string;
}

/**
 * Settings → Alerts on every client (owner, 2026-10-23: every panel event backed by evidence
 * notifies on the phone, the watch, the desktop and the web), by panel, in the same order and
 * words so the three clients read the same. Descriptions say what happened, never what to do.
 */
export const ALERT_SWITCH_SECTIONS: readonly { id: "runners" | "ross" | "signals" | "halt" | "halts" | "shadow"; label: string; items: readonly AlertSwitch[] }[] = [
  {
    id: "runners",
    label: "Runners",
    items: [
      { key: "new", icon: ALERT_SWITCH_ICON.new, name: "New flags", description: "Ticker, price and the top reasons as soon as Runners flags it." },
      { key: "breakout", icon: ALERT_SWITCH_ICON.breakout, name: "Breakouts", description: "When a flagged stock turns breaking out, with its % from the flag." },
      { key: "burst", icon: ALERT_SWITCH_ICON.burst, name: "Buying bursts", description: "When heavy buying is confirmed on the tape, with how far the burst already went." },
      { key: "running", icon: ALERT_SWITCH_ICON.running, name: "Running entries", description: "Every stock that starts running by its own range, with its move and money." },
      { key: "run", icon: ALERT_SWITCH_ICON.run, name: "Run +20%", description: "A +20% leg within 15 minutes on real trades, with its window and start price." },
      { key: "fast", icon: ALERT_SWITCH_ICON.fast, name: "Fast move", description: "+8% in a minute or +12% in 15 s on real trades, with the move and its money." },
      { key: "followUp", icon: ALERT_SWITCH_ICON.followUp, name: "Follow-ups", description: "A later Runners path fires on a stock already listed, e.g. Run +20% after a fast move." },
      { key: "pullback", icon: ALERT_SWITCH_ICON.pullback, name: "Pullback setting up", description: "The first dip that fixes a trigger and stop within 30 min of a stock's Runners alert (or on a Ross stock), then its crossing. Most do not follow through." },
      { key: "shelf", icon: ALERT_SWITCH_ICON.shelf, name: "Volume shelf entry", description: "Price came into a volume shelf. About 1 in 3 hold, 1 in 2 fail." },
    ],
  },
  {
    id: "ross",
    label: "Ross",
    items: [
      { key: "rossCrossing", icon: ALERT_SWITCH_ICON.rossCrossing, name: "Crossing", description: "The pullback's trigger is being crossed (close pending). Most do not follow through." },
      { key: "rossTrigger", icon: ALERT_SWITCH_ICON.rossTrigger, name: "At trigger", description: "Price is just under the pullback's trigger." },
    ],
  },
  {
    id: "signals",
    label: "Signals",
    items: [
      { key: "signalsCrossing", icon: ALERT_SWITCH_ICON.signalsCrossing, name: "Crossing", description: "A Signals pullback's trigger is being crossed (close pending). Sent once if Ross has it too." },
      { key: "signalsTrigger", icon: ALERT_SWITCH_ICON.signalsTrigger, name: "At trigger", description: "Price is just under a Signals pullback's trigger." },
    ],
  },
  {
    id: "halt",
    label: "Halt watcher",
    items: [
      { key: "halt", icon: ALERT_SWITCH_ICON.halt, name: "Gauge appears", description: "Pressure building fast near the LULD band. A possible halt, not a forecast." },
      {
        key: "haltBull",
        icon: ALERT_SWITCH_ICON.haltBull,
        name: "Buying-side gauge, 9:30–11:30",
        description: "The gauge appears on the buying side of a $0.10–$25 stock between 9:30 and 11:30. About 1 in 5 reached +10% before −5% in Engine A's tests. Sent once if Gauge appears is on too.",
      },
    ],
  },
  {
    // The exchange's own halts (the web and desktop add a sound picker per row: notify/HaltsRows).
    id: "halts",
    label: "Halts",
    items: [
      { key: "halted", icon: ALERT_SWITCH_ICON.halted, name: "Halted", description: "The exchange halts a Runners stock, a charted one or one with a halt alert: reason and day's move." },
      { key: "resumed", icon: ALERT_SWITCH_ICON.resumed, name: "Resumed", description: "Trading resumes: how long it was halted and the move from the halt price." },
    ],
  },
  {
    // Engine A's research detectors, sent beside production (judge ruling J8), never instead of it.
    id: "shadow",
    label: "Shadow (research)",
    items: [
      {
        key: "n1",
        icon: ALERT_SWITCH_ICON.n1,
        name: "N1 · shadow",
        description: "+8% off a 3-minute low on heavy trading, 9:30–11:30. An early look at stocks that may run: about 1 in 8 reached +10% before −5% in Engine A's tests. On by default.",
      },
      {
        key: "f1u",
        icon: ALERT_SWITCH_ICON.f1u,
        name: "F1 · first pullback",
        description: "A stock's first dip after a +10% ignition crosses back over the dip's last high, 9:30–11:30, within 20% of the day's low. About 1 in 15 reached +10% before −5% in Engine A's tests. On by default.",
      },
    ],
  },
];

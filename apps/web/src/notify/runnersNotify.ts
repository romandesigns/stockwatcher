// Runners system notifications on the web and the desktop shell (owner, 2026-10-23: every Runners
// event the phone is pushed for also notifies here, with the same text, and a click opens that
// ticker's chart).
//
// - **What:** the server's `runners.notice` frames (crates/server/src/alerts/runners.rs
//   `desk_frame`): it already applied the phone's limits (10 min per stock, 2 for a burst, the
//   no daily cap, live market only) once for every open client, and the title and body are
//   the phone push's own, so nothing here re-derives text.
// - **Who:** this device's switches (Settings → Runners), the phone's defaults (everything on
//   but breakouts), kept in localStorage: per device, like the phone's, never synced. The
//   connection tells the server it listens (`runners.notify`), so a kind no open client wants
//   does not spend the stock's gap.
// - **Once:** each notice id is claimed in localStorage before it shows, so a reconnect, a
//   reload or a second tab never shows it twice (and the browser's `tag` collapses a race).
// - **How:** the desktop shell (Tauri) shows it through its one app command, `runners_notify`
//   (apps/desktop/src-tauri/src/notify.rs), which also brings the window back on a click; the
//   browser uses the Notification API while the tab is open, foreground or background.
//   Permission is asked from the Settings switch only, never on load.
// - **Toasts:** with system notifications on, the dashboard's own Runners toasts stand down
//   (dashboard/runnersToasts.ts), so one event never shows twice, focused or not.
//
// Closed-tab Web Push (a service worker + VAPID, RFC 8030/8292) is not built: see the commit.
//
// Exchange halts (plan "halt sounds", 2026-11-16): a `halt.halted` / `halt.resumed` notice also
// plays the sound this device picked for it (Settings → Alerts → Halts; per device, like the
// switches; app-state HALT_SOUNDS), at the alert sounds' synced volume, muted by their global
// switch. Halts do not need system notifications: with them off (or refused by the browser) the
// connection still opts in to the two halt kinds, and a halt plays its sound and shows an in-app
// toast. A browser that has not had its click yet cannot play audio: the toast then says so.

import { haltSounds, type HaltEvent, type HaltSoundKey } from "@stockwatcher/app-state";
import { createNoticeDedupe, DEFAULT_RUNNERS_NOTIFY, isHaltNotice, runnersNoticeChartContext, runnersNotifyKinds, shouldShowNotice, type ClientMessage, type NoticeChartContext, type RunnersNoticeMessage, type RunnersNotifyKinds, type RunnersNotifyMessage } from "@stockwatcher/shared";
import { alertSoundUnlocked, playHaltSound } from "../alerts/sound.ts";
import { showToast } from "../lib/toastStore.ts";
import { openTickerChart } from "../nav/openTicker.ts";
import { getAppStore } from "../state/store.ts";

const PREFS_KEY = "sw-runners-notify";
const SEEN_KEY = "sw-runners-notify:seen";
/** The stockwatcher mark (favicon.svg rendered to PNG: notifications do not take SVG everywhere). */
export const NOTIFY_ICON = "/notify-icon.png";

export interface RunnersNotifyPrefs {
  /** System notifications on for this device. */
  enabled: boolean;
  kinds: RunnersNotifyKinds;
  /** The sound this device plays for each halt event (absent in older stored prefs: the defaults). */
  haltSounds: Record<HaltEvent, HaltSoundKey>;
}

/** The surface the notifier needs from the page, so tests can supply their own. */
export interface NotifyEnv {
  storage: Pick<Storage, "getItem" | "setItem"> | null;
  /** The desktop shell's command bridge (window.__TAURI_INTERNALS__.invoke), when running in it. */
  invoke: ((cmd: string, args: Record<string, unknown>) => Promise<unknown>) | null;
  /** The browser's Notification constructor, when there is one. */
  Notification: typeof Notification | null;
  focus: () => void;
  openChart: (symbol: string, noticeContext?: NoticeChartContext) => void;
  /** Plays a halt sound at the alert sounds' volume; false when muted, "none", locked or unavailable. */
  playHalt?: (key: HaltSoundKey) => boolean;
  /** Whether the page may play audio yet (the browser's one click). */
  soundUnlocked?: () => boolean;
  /** An in-app toast for a halt notice shown without a system notification, or whose sound was locked. */
  toast?: (m: RunnersNoticeMessage, locked: boolean) => void;
}

type Tauri = { invoke: (cmd: string, args: Record<string, unknown>) => Promise<unknown> };

function browserEnv(): NotifyEnv {
  const w = typeof window === "undefined" ? undefined : (window as Window & { __TAURI_INTERNALS__?: Tauri });
  let storage: NotifyEnv["storage"] = null;
  try {
    storage = w?.localStorage ?? null;
  } catch {
    storage = null;
  }
  const tauri = w?.__TAURI_INTERNALS__;
  return {
    storage,
    invoke: tauri ? (cmd, args) => tauri.invoke(cmd, args) : null,
    Notification: typeof Notification === "undefined" ? null : Notification,
    focus: () => w?.focus(),
    openChart: openTickerChart,
    playHalt: (key) => {
      const { on, volume } = getAppStore().getState().alertPrefs.sound;
      return on && playHaltSound(key, volume);
    },
    soundUnlocked: alertSoundUnlocked,
    toast: (m, locked) => {
      const test = m.data?.test === true;
      showToast(`${m.title} · ${m.body}${locked ? " · click anywhere to allow alert sounds" : ""}`, { icon: "bell", ms: 8000, ...(test ? {} : { action: { label: "Chart", run: () => openTickerChart(m.symbol) } }) });
    },
  };
}

/** The halt event a notice is about. */
export const haltEventOf = (kind: string): HaltEvent => (kind === "halt.resumed" ? "resumed" : "halted");

/**
 * A click's destination: the dashboard, with the ticker loaded into its selected chart: the app's
 * one "open this ticker" action (nav/openTicker.ts), the same as clicking a ticker on the dashboard.
 */
export { openTickerChart };

/** The desktop shell's click target (notify.rs runs `window.__stockwatcherOpenChart(symbol)`). */
export function installOpenChartHook(): void {
  if (typeof window === "undefined") return;
  (window as Window & { __stockwatcherOpenChart?: (s: string, context?: NoticeChartContext) => void }).__stockwatcherOpenChart = openTickerChart;
}

export interface RunnersNotifier {
  prefs(): RunnersNotifyPrefs;
  setPrefs(next: RunnersNotifyPrefs): void;
  subscribe(cb: () => void): () => void;
  /** "granted" | "denied" | "default" | "unsupported"; the desktop shell is always "granted". */
  permission(): NotificationPermission | "unsupported";
  /** From a user action only (the Settings switch): asks the browser, then turns it on if granted. */
  enable(): Promise<boolean>;
  /** Shows system notifications now (on, and allowed). */
  active(): boolean;
  /** The opt-in a connection sends on welcome (null = none). */
  notifyMessage(): RunnersNotifyMessage | null;
  /** One `runners.notice` frame: shown when its switch is on and it is new here. */
  handle(m: RunnersNoticeMessage): boolean;
  /** Connections to re-send the opt-in to when the switches change, and to send `halts.test` on. */
  attach(refresh: () => void, send?: (m: ClientMessage) => boolean): () => void;
  /** Settings "Send test": asks the server for a real test notice of `event` (false: no connection). */
  sendHaltTest(event: HaltEvent): boolean;
  readonly desktop: boolean;
}

export function createRunnersNotifier(env: NotifyEnv = browserEnv()): RunnersNotifier {
  const desktop = env.invoke !== null;
  const read = (): RunnersNotifyPrefs => {
    let raw: unknown;
    try {
      const s = env.storage?.getItem(PREFS_KEY);
      raw = s ? JSON.parse(s) : undefined;
    } catch {
      raw = undefined;
    }
    const o = raw && typeof raw === "object" ? (raw as Record<string, unknown>) : {};
    // The desktop shell notifies by default (it needs no permission, like the phone); the browser
    // waits for the switch, because only a click may ask for its permission.
    return { enabled: typeof o.enabled === "boolean" ? o.enabled : desktop, kinds: runnersNotifyKinds(o.kinds), haltSounds: haltSounds(o.haltSounds) };
  };
  let prefs = read();
  const listeners = new Set<() => void>();
  const refreshers = new Set<() => void>();
  const senders = new Set<(m: ClientMessage) => boolean>();
  const dedupe = createNoticeDedupe({
    load: () => {
      const s = env.storage?.getItem(SEEN_KEY);
      const v: unknown = s ? JSON.parse(s) : [];
      return Array.isArray(v) ? v.filter((x): x is string => typeof x === "string") : [];
    },
    save: (ids) => env.storage?.setItem(SEEN_KEY, JSON.stringify(ids)),
  });
  const permission = (): NotificationPermission | "unsupported" => (desktop ? "granted" : env.Notification ? env.Notification.permission : "unsupported");
  const active = () => prefs.enabled && permission() === "granted";
  // Halts reach this device even without system notifications (module docs).
  const haltsWanted = () => prefs.kinds.halted || prefs.kinds.resumed;
  const changed = () => {
    for (const l of listeners) l();
    for (const r of refreshers) r();
  };

  // Every notice shown is its own notification, tagged by its own id. With "Group repeat alerts"
  // on (owner 2026-11-25; server runners.rs "Limits" 4a) the server sends a stock's repeats as
  // `runners.grouped` instead, which never reaches here (shouldShowNotice) and only updates the
  // stock's card on the page (noticeCards.ts): the desktop shell cannot update a shown one.
  const show = (m: RunnersNoticeMessage) => {
    const noticeContext = runnersNoticeChartContext(m);
    const halt = isHaltNotice(m.kind);
    const test = m.data?.test === true;
    if (env.invoke) {
      void env.invoke("runners_notify", { id: m.id, title: m.title, body: m.body, symbol: m.symbol, ...(noticeContext ? { noticeContext } : {}) }).catch(() => {});
      return;
    }
    const N = env.Notification;
    if (!N) return;
    try {
      // A halt plays its own sound (the device's pick), so the browser's is silenced.
      const n = new N(m.title, { body: m.body, icon: NOTIFY_ICON, badge: NOTIFY_ICON, tag: m.id, data: { symbol: m.symbol }, ...(halt ? { silent: true } : {}) });
      n.onclick = (e) => {
        e.preventDefault();
        env.focus();
        // The Settings test is about no real stock.
        if (!test) {
          if (noticeContext) env.openChart(m.symbol, noticeContext);
          else env.openChart(m.symbol);
        }
        n.close();
      };
    } catch {
      // A browser that only notifies from a service worker (Android Chrome): nothing to show here.
    }
  };

  return {
    desktop,
    prefs: () => prefs,
    setPrefs(next) {
      prefs = { enabled: next.enabled, kinds: { ...next.kinds }, haltSounds: { ...(next.haltSounds ?? prefs.haltSounds) } };
      try {
        env.storage?.setItem(PREFS_KEY, JSON.stringify(prefs));
      } catch {
        // Private mode: the choice lasts for this visit.
      }
      changed();
    },
    subscribe(cb) {
      listeners.add(cb);
      return () => listeners.delete(cb);
    },
    permission,
    async enable() {
      let p = permission();
      if (p === "default" && env.Notification) p = await env.Notification.requestPermission();
      if (p !== "granted") {
        changed();
        return false;
      }
      this.setPrefs({ ...prefs, enabled: true });
      return true;
    },
    active,
    notifyMessage: () => {
      if (active()) return { type: "runners.notify", enabled: true, ...prefs.kinds };
      // System notifications off: only the halts, for their sound and toast.
      if (!haltsWanted()) return null;
      const none = Object.fromEntries(Object.keys(prefs.kinds).map((k) => [k, false])) as unknown as RunnersNotifyKinds;
      return { type: "runners.notify", enabled: true, ...none, halted: prefs.kinds.halted, resumed: prefs.kinds.resumed };
    },
    handle(m) {
      const halt = isHaltNotice(m.kind);
      if (!active() && !(halt && haltsWanted())) return false;
      // A Settings test is shown whatever its switch says (its id is new every time).
      const test = halt && m.data?.test === true;
      if (!test && !shouldShowNotice(m, prefs.kinds, dedupe)) return false;
      if (!halt) {
        show(m);
        return true;
      }
      const key = prefs.haltSounds[haltEventOf(m.kind)];
      const played = env.playHalt?.(key) ?? false;
      // A sound that should have played, but the page has not had its click yet.
      const locked = !played && key !== "none" && env.soundUnlocked?.() === false;
      if (active()) {
        show(m);
        if (locked) env.toast?.(m, true);
      } else {
        env.toast?.(m, locked);
      }
      return true;
    },
    attach(refresh, send) {
      refreshers.add(refresh);
      if (send) senders.add(send);
      return () => {
        refreshers.delete(refresh);
        if (send) senders.delete(send);
      };
    },
    sendHaltTest(event) {
      for (const send of senders) if (send({ type: "halts.test", event })) return true;
      return false;
    },
  };
}

let shared: RunnersNotifier | null = null;

/** The page's notifier, created on first use; also exposes the click target to the desktop shell. */
export function getRunnersNotifier(): RunnersNotifier {
  if (!shared) {
    shared = createRunnersNotifier();
    // The desktop shell's click handler calls this after restoring the window (notify.rs).
    if (typeof window !== "undefined") {
      installOpenChartHook();
      const w = window as Window & { __stockwatcherNotice?: (m: RunnersNoticeMessage) => boolean };
      // Dev builds only: feed a notice by hand to check the notification and its click.
      const n = shared;
      if (import.meta.env?.DEV) w.__stockwatcherNotice = (m) => n.handle(m);
    }
  }
  return shared;
}

export { DEFAULT_RUNNERS_NOTIFY };

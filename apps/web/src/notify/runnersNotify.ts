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

import { createNoticeDedupe, DEFAULT_RUNNERS_NOTIFY, runnersNotifyKinds, shouldShowNotice, type RunnersNoticeMessage, type RunnersNotifyKinds, type RunnersNotifyMessage } from "@stockwatcher/shared";
import { openTickerChart } from "../nav/openTicker.ts";

const PREFS_KEY = "sw-runners-notify";
const SEEN_KEY = "sw-runners-notify:seen";
/** The stockwatcher mark (favicon.svg rendered to PNG: notifications do not take SVG everywhere). */
export const NOTIFY_ICON = "/notify-icon.png";

export interface RunnersNotifyPrefs {
  /** System notifications on for this device. */
  enabled: boolean;
  kinds: RunnersNotifyKinds;
}

/** The surface the notifier needs from the page, so tests can supply their own. */
export interface NotifyEnv {
  storage: Pick<Storage, "getItem" | "setItem"> | null;
  /** The desktop shell's command bridge (window.__TAURI_INTERNALS__.invoke), when running in it. */
  invoke: ((cmd: string, args: Record<string, unknown>) => Promise<unknown>) | null;
  /** The browser's Notification constructor, when there is one. */
  Notification: typeof Notification | null;
  focus: () => void;
  openChart: (symbol: string) => void;
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
  };
}

/**
 * A click's destination: the dashboard, with the ticker loaded into its selected chart: the app's
 * one "open this ticker" action (nav/openTicker.ts), the same as clicking a ticker on the dashboard.
 */
export { openTickerChart };

/** The desktop shell's click target (notify.rs runs `window.__stockwatcherOpenChart(symbol)`). */
export function installOpenChartHook(): void {
  if (typeof window === "undefined") return;
  (window as Window & { __stockwatcherOpenChart?: (s: string) => void }).__stockwatcherOpenChart = openTickerChart;
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
  /** Connections to re-send the opt-in to when the switches change. */
  attach(refresh: () => void): () => void;
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
    return { enabled: typeof o.enabled === "boolean" ? o.enabled : desktop, kinds: runnersNotifyKinds(o.kinds) };
  };
  let prefs = read();
  const listeners = new Set<() => void>();
  const refreshers = new Set<() => void>();
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
  const changed = () => {
    for (const l of listeners) l();
    for (const r of refreshers) r();
  };

  const show = (m: RunnersNoticeMessage) => {
    if (env.invoke) {
      void env.invoke("runners_notify", { id: m.id, title: m.title, body: m.body, symbol: m.symbol }).catch(() => {});
      return;
    }
    const N = env.Notification;
    if (!N) return;
    try {
      const n = new N(m.title, { body: m.body, icon: NOTIFY_ICON, badge: NOTIFY_ICON, tag: m.id, data: { symbol: m.symbol } });
      n.onclick = (e) => {
        e.preventDefault();
        env.focus();
        env.openChart(m.symbol);
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
      prefs = { enabled: next.enabled, kinds: { ...next.kinds } };
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
    notifyMessage: () => (active() ? { type: "runners.notify", enabled: true, ...prefs.kinds } : null),
    handle(m) {
      if (!active() || !shouldShowNotice(m, prefs.kinds, dedupe)) return false;
      show(m);
      return true;
    },
    attach(refresh) {
      refreshers.add(refresh);
      return () => refreshers.delete(refresh);
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

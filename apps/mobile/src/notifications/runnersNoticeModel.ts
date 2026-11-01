import { createNoticeDedupe, shouldShowNotice, type NoticeDedupe, type RunnersNoticeMessage, type RunnersNotifyKinds, type RunnersNotifyMessage } from "@stockwatcher/shared";

import { PUSH_PREF_OF, type RunnersPushPrefs } from "./runnersPushModel";

// The phone's Runners local banners from the server's `runners.notice` frames (pure, so bun tests
// it without expo; the store and the banner itself are in runnersNotices.ts).
//
// With a push token the server pushes this phone (crates/server/src/alerts/push.rs), in the
// foreground too, so nothing here rings. Without one (permission refused, a build without FCM)
// the dashboard's connection opts in with `runners.notify`, exactly as the desktop and web do
// (apps/web/src/notify/runnersNotify.ts), and rings each notice the server let through: the push's
// own title and body, under the same Settings → Alerts switches. Against a server without
// `runners.notice.v1` (older) the Runners card keeps diffing panels itself (RocketPanel.tsx,
// rocket.ts), which is why `serverSends` is asked before it does.

/** The phone's Settings → Alerts switches as the shared notification kinds. */
export function phoneNotifyKinds(p: RunnersPushPrefs): RunnersNotifyKinds {
  const out = {} as RunnersNotifyKinds;
  for (const k of Object.keys(PUSH_PREF_OF) as (keyof RunnersNotifyKinds)[]) out[k] = p[PUSH_PREF_OF[k]];
  return out;
}

export interface PhoneNotices {
  /** The dashboard connection's welcome listed `runners.notice.v1` (false on close or an older server). */
  setServerSends(on: boolean): void;
  /** The server rings for us: the Runners card must not diff panels for banners. */
  serverSends(): boolean;
  /** The opt-in the connection sends on welcome and on every change (null = none: a push token, or an older server). */
  notifyMessage(): RunnersNotifyMessage | null;
  /** One `runners.notice`: rung when this phone has no push, its switch is on and it is new here. */
  handle(m: RunnersNoticeMessage): boolean;
}

export function createPhoneNotices(env: { prefs: () => RunnersPushPrefs; token: () => string | null; ring: (m: RunnersNoticeMessage) => void; dedupe?: NoticeDedupe }): PhoneNotices {
  let sends = false;
  // In memory: the server never replays a notice (desk_frame: live news only), and a reconnect's
  // second copy of one in flight is caught here.
  const dedupe = env.dedupe ?? createNoticeDedupe();
  const listening = () => sends && !env.token();
  return {
    setServerSends(on) {
      sends = on;
    },
    serverSends: () => sends,
    notifyMessage: () => (listening() ? { type: "runners.notify", enabled: true, ...phoneNotifyKinds(env.prefs()) } : null),
    handle(m) {
      if (!listening()) return false;
      if (!shouldShowNotice(m, phoneNotifyKinds(env.prefs()), dedupe)) return false;
      env.ring(m);
      return true;
    },
  };
}

/**
 * What a rung notice's banner carries: the push's own data, so a tap routes exactly like the
 * push (route.ts `notificationTarget`: the ticker's chart, with the notice's context).
 */
export function noticeBannerData(m: RunnersNoticeMessage): Record<string, unknown> {
  return { ...(m.data ?? {}), symbol: m.symbol, kind: m.kind, alert_id: m.alertId, event_id: m.id };
}

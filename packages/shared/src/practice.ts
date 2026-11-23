// Practice notices (owner, 2026-11-23; crates/server/src/practice.rs): what the REPLAY market's
// detectors decided, sent as `practice.notice` on replay connections only (the practice page).
//
// They are in-app toasts labelled "Practice" and nothing else: never a system notification, an
// Expo local notification, a phone push or an alert sound. That is why they have their own frame
// type and their own parser, and why the clients' live notifiers (web notify/runnersNotify.ts,
// mobile notifications/runnersNotices.ts), which only ever take `runners.notice`, never see them:
// each client's feed takes the frame off the socket before anything else reads it.
//
// A practice-page connection also still receives the real live `runners.notice` frames (they are
// process-wide on the server). The practice page shows those too, labelled "Live", so the two can
// never be mistaken for each other ({@link practiceToast}).

import { parseRunnersNotice, wantsNotice, type RunnersNoticeKind, type RunnersNoticeMessage, type RunnersNotifyKinds } from "./runnersNotify.ts";

/** `welcome.features`: the server sends {@link PracticeNoticeMessage}s to replay connections. */
export const PRACTICE_NOTICE_FEATURE = "practice.notice.v1";

/** server `practice.notice`: a notice the replay market decided, with the live push's own text. */
export interface PracticeNoticeMessage {
  type: "practice.notice";
  practice: true;
  /** `practice-<live event id>`: never the id of a real notice. */
  id: string;
  alertId: string;
  symbol: string;
  kind: RunnersNoticeKind;
  title: string;
  body: string;
  /** The decision's time on the replay clock, epoch ms. */
  at: number;
  /** The replayed trading day ("YYYY-MM-DD"), when the replay names one. */
  day: string | null;
}

/** The frame's text off the socket, cheaply: the server writes `type` first. */
export const isPracticeNoticeFrame = (raw: string): boolean => raw.startsWith('{"type":"practice.notice"');

/** A frame read defensively: null for anything that is not a well-formed practice notice. */
export function parsePracticeNotice(v: unknown): PracticeNoticeMessage | null {
  if (!v || typeof v !== "object") return null;
  const m = v as Record<string, unknown>;
  if (m.type !== "practice.notice" || m.practice !== true) return null;
  // The same fields as a live notice: one validator, so the two cannot drift.
  const as = parseRunnersNotice({ ...m, type: "runners.notice" });
  if (!as || !as.id.startsWith("practice-")) return null;
  return { type: "practice.notice", practice: true, id: as.id, alertId: as.alertId, symbol: as.symbol, kind: as.kind, title: as.title, body: as.body, at: as.at, day: typeof m.day === "string" ? m.day : null };
}

/** What a toast on the practice page says, and which label it carries. */
export interface PracticeToast {
  /** "practice" for the replay's own notices, "live" for a real alert arriving meanwhile. */
  label: "practice" | "live";
  symbol: string;
  /** One line: the title and the body's first line. */
  text: string;
}

/** The chip text of each label. */
export const PRACTICE_TOAST_LABEL: Readonly<Record<PracticeToast["label"], string>> = Object.freeze({ practice: "Practice", live: "Live" });

/**
 * A notice as a practice-page toast, or null when this device's switches (Settings → Alerts, the
 * same switches as the live notifications) do not want its kind. A practice notice and a live one
 * get different labels; nothing else about them differs.
 */
export function practiceToast(m: PracticeNoticeMessage | RunnersNoticeMessage, kinds: RunnersNotifyKinds): PracticeToast | null {
  if (!wantsNotice(m, kinds)) return null;
  const first = m.body.split("\n")[0]?.trim() ?? "";
  return { label: m.type === "practice.notice" ? "practice" : "live", symbol: m.symbol, text: first ? `${m.title} · ${first}` : m.title };
}

/** A bounded set of ids already toasted on this page (a resent frame never shows twice). */
export function createToastOnce(max = 200): (id: string) => boolean {
  const seen: string[] = [];
  return (id) => {
    if (seen.includes(id)) return false;
    seen.push(id);
    if (seen.length > max) seen.shift();
    return true;
  };
}

/** A stream of one kind of notice frame (a client feed's `practiceNotices` / `liveNotices`). */
export interface NoticeStream<T> {
  subscribe(fn: (m: T) => void): () => void;
}

/** The two streams the practice page toasts from. */
export interface PracticeToastStreams {
  practiceNotices?: NoticeStream<PracticeNoticeMessage>;
  liveNotices?: NoticeStream<RunnersNoticeMessage>;
}

/**
 * Wires a practice connection's two notice streams to the page's toasts (web and phone alike):
 * each notice its switches want, once per id, labelled Practice or Live. `show` is the client's
 * in-app toast and nothing else; returns the unsubscribe.
 */
export function attachPracticeToasts(streams: PracticeToastStreams, env: { kinds: () => RunnersNotifyKinds; show: (t: PracticeToast) => void }): () => void {
  const once = createToastOnce();
  const take = (m: PracticeNoticeMessage | RunnersNoticeMessage) => {
    const t = practiceToast(m, env.kinds());
    if (t && once(m.id)) env.show(t);
  };
  const offPractice = streams.practiceNotices?.subscribe(take);
  const offLive = streams.liveNotices?.subscribe(take);
  return () => {
    offPractice?.();
    offLive?.();
  };
}

import { EMPTY_NOTICES, parseNoticeLog, serverUrl, type NoticeLogWire, type NoticesChannel, type NoticesState } from "@stockwatcher/shared";
import { useCallback, useEffect, useState, useSyncExternalStore } from "react";
import { getToken } from "../feed/token.ts";

export type NoticeLoad = { state: "loading" } | { state: "error"; message: string } | { state: "ok"; log: NoticeLogWire };

const LOADING: NoticeLoad = { state: "loading" };
const NOOP = () => {};

/**
 * The log for `day` (null = today, the server's ET date).
 *
 * Today, with `channel` (the feed's `notices`): the server pushes it (`notices.v1`,
 * crates/server/src/notices.rs), a snapshot then one append per ledger line, and there is no
 * HTTP at all; loading until the snapshot. The web always ships with its server, so the 15 s
 * refetch an older server needed is gone (the phone keeps its own for installed builds). A past
 * day, or today without a channel, is one GET /api/notices: a past day's file no longer changes.
 * A failed load reports the error.
 */
export function useNoticeLog(day: string | null, channel?: NoticesChannel): NoticeLoad {
  // Only today is pushed: a past day never subscribes.
  const pushable = day === null && channel !== undefined;
  const subscribe = useCallback((cb: () => void) => (pushable && channel ? channel.subscribe(cb) : NOOP), [pushable, channel]);
  const get = useCallback((): NoticesState => (pushable && channel ? channel.get() : EMPTY_NOTICES), [pushable, channel]);
  const pushed = useSyncExternalStore(subscribe, get, get);

  // Keyed by the day it is for, so switching day reads as loading without a reset in the effect.
  const [got, setGot] = useState<{ day: string | null; load: NoticeLoad } | null>(null);
  useEffect(() => {
    if (pushable) return;
    let alive = true;
    const token = getToken();
    const q = day ? `?day=${encodeURIComponent(day)}` : "";
    fetch(serverUrl(`/api/notices${q}`), { headers: token ? { Authorization: `Bearer ${token}` } : {} })
      .then(async (res) => {
        if (!res.ok) throw new Error(res.status === 401 ? "Sign in to see the notification log." : `The server answered ${res.status}.`);
        const log = parseNoticeLog(await res.json());
        if (!log) throw new Error("The server's answer was not a notification log.");
        if (alive) setGot({ day, load: { state: "ok", log } });
      })
      .catch((e: unknown) => {
        const message = e instanceof Error ? e.message : String(e);
        if (alive) setGot({ day, load: { state: "error", message } });
      });
    return () => {
      alive = false;
    };
  }, [day, pushable]);

  if (pushable) return pushed.log ? { state: "ok", log: pushed.log } : LOADING;
  return got && got.day === day ? got.load : LOADING;
}

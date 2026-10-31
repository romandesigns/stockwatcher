import { EMPTY_NOTICES, parseNoticeLog, serverUrl, type NoticeLogWire, type NoticesChannel, type NoticesState } from "@stockwatcher/shared";
import { useCallback, useEffect, useState, useSyncExternalStore } from "react";
import { getToken } from "../feed/token.ts";

/** How often the open log refetches today, only from a server that does not push it (no `notices.v1`). */
const REFRESH_MS = 15_000;

export type NoticeLoad = { state: "loading" } | { state: "error"; message: string } | { state: "ok"; log: NoticeLogWire };

const LOADING: NoticeLoad = { state: "loading" };
const NOOP = () => {};

/**
 * The log for `day` (null = today, the server's ET date).
 *
 * Today, with `channel` (the feed's `notices`) on a server that pushes it (`notices.v1`,
 * crates/server/src/notices.rs): the pushed log, a snapshot then one append per ledger line, and
 * no HTTP at all. Against an older server, GET /api/notices refetched every 15 s while the page is
 * visible (a hidden tab stops, and refetches at once when shown again), as before. A past day is
 * one fetch: its file no longer changes. A failed refresh keeps the last good log; only a day's
 * first load reports the error.
 */
export function useNoticeLog(day: string | null, channel?: NoticesChannel): NoticeLoad {
  // Only today is pushed: a past day never subscribes.
  const pushable = day === null && channel !== undefined;
  const subscribe = useCallback((cb: () => void) => (pushable && channel ? channel.subscribe(cb) : NOOP), [pushable, channel]);
  const get = useCallback((): NoticesState => (pushable && channel ? channel.get() : EMPTY_NOTICES), [pushable, channel]);
  const pushed = useSyncExternalStore(subscribe, get, get);
  const live = pushable && pushed.available === true;

  // Keyed by the day it is for, so switching day reads as loading without a reset in the effect.
  const [got, setGot] = useState<{ day: string | null; load: NoticeLoad } | null>(null);
  useEffect(() => {
    if (live) return;
    let alive = true;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const fetchOnce = () => {
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
          if (alive) setGot((prev) => (prev && prev.day === day && prev.load.state === "ok" ? prev : { day, load: { state: "error", message } }));
        })
        .finally(() => {
          // Only today changes; a past day is fetched once.
          if (alive && day === null && !document.hidden) timer = setTimeout(fetchOnce, REFRESH_MS);
        });
    };
    const onVisible = () => {
      if (!document.hidden && day === null) {
        clearTimeout(timer);
        fetchOnce();
      }
    };
    fetchOnce();
    document.addEventListener("visibilitychange", onVisible);
    return () => {
      alive = false;
      clearTimeout(timer);
      document.removeEventListener("visibilitychange", onVisible);
    };
  }, [day, live]);

  if (live && pushed.log) return { state: "ok", log: pushed.log };
  // Pushed but its snapshot not in yet: whatever the last fetch had for today, else loading.
  return got && got.day === day ? got.load : LOADING;
}

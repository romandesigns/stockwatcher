import { authenticatedFetch, EMPTY_NOTICES, parseNoticeLog, type NoticeLogWire, type NoticesChannel, type NoticesState } from "@stockwatcher/shared";
import { useCallback, useEffect, useState, useSyncExternalStore } from "react";
import { AppState } from "react-native";

import { SERVER_URL } from "../feed/server";
import { getAccessToken } from "../storage/secure";

/** How often the open log refetches today, only from a server that does not push it (no `notices.v1`). */
const REFRESH_MS = 15_000;

export type NoticeLoad = { state: "loading" } | { state: "error"; message: string } | { state: "ok"; log: NoticeLogWire };
const LOADING: NoticeLoad = { state: "loading" };
const NOOP = () => {};

/**
 * The log for `day` (null = today, the server's ET date), the web's useNoticeLog on the phone.
 *
 * Today, with `channel` (the dashboard feed's `notices`) on a server that pushes it
 * (`notices.v1`, crates/server/src/notices.rs): the pushed log, no HTTP. Otherwise GET
 * /api/notices, refetched every 15 s while the app is active and at once when it comes back, as
 * every older APK does. A past day is one fetch: its file no longer changes. A failed refresh keeps
 * the last good log; only a day's first load reports the error.
 */
export function useNoticeLog(day: string | null, channel?: NoticesChannel): NoticeLoad {
  // Only today is pushed: a past day never subscribes.
  const pushable = day === null && channel !== undefined;
  const subscribe = useCallback((cb: () => void) => (pushable && channel ? channel.subscribe(cb) : NOOP), [pushable, channel]);
  const get = useCallback((): NoticesState => (pushable && channel ? channel.get() : EMPTY_NOTICES), [pushable, channel]);
  const pushed = useSyncExternalStore(subscribe, get);
  const live = pushable && pushed.available === true;

  const [got, setGot] = useState<{ day: string | null; load: NoticeLoad } | null>(null);
  useEffect(() => {
    if (live) return;
    let alive = true;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const fetchOnce = async () => {
      try {
        const token = await getAccessToken();
        const q = day ? `?day=${encodeURIComponent(day)}` : "";
        const res = await authenticatedFetch(`${SERVER_URL}/api/notices${q}`, token);
        if (!res.ok) throw new Error(`The server answered ${res.status}.`);
        const log = parseNoticeLog(await res.json());
        if (!log) throw new Error("The server's answer was not a notification log.");
        if (alive) setGot({ day, load: { state: "ok", log } });
      } catch (e) {
        const message = e instanceof Error && e.name === "UnauthorizedError" ? "Sign in to see the notification log." : e instanceof Error ? e.message : String(e);
        if (alive) setGot((prev) => (prev && prev.day === day && prev.load.state === "ok" ? prev : { day, load: { state: "error", message } }));
      } finally {
        // Only today changes; a past day is fetched once.
        if (alive && day === null && AppState.currentState === "active") timer = setTimeout(() => void fetchOnce(), REFRESH_MS);
      }
    };
    const sub = AppState.addEventListener("change", (s) => {
      if (s === "active" && day === null) {
        clearTimeout(timer);
        void fetchOnce();
      }
    });
    void fetchOnce();
    return () => {
      alive = false;
      clearTimeout(timer);
      sub.remove();
    };
  }, [day, live]);

  if (live && pushed.log) return { state: "ok", log: pushed.log };
  // Pushed but its snapshot not in yet: whatever the last fetch had for today, else loading.
  return got && got.day === day ? got.load : LOADING;
}

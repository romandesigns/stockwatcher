import { parseNoticeLog, serverUrl, type NoticeLogWire } from "@stockwatcher/shared";
import { useEffect, useState } from "react";
import { getToken } from "../feed/token.ts";

/** How often the open log refetches its day (only while the page is visible). */
const REFRESH_MS = 15_000;

export type NoticeLoad = { state: "loading" } | { state: "error"; message: string } | { state: "ok"; log: NoticeLogWire };

const LOADING: NoticeLoad = { state: "loading" };

/**
 * GET /api/notices for `day` (null = today, the server's ET date), refetched every 15 s while the
 * page is visible; a hidden tab stops and refetches at once when shown again. A failed refresh
 * keeps the last good log: only a day's first load reports the error.
 */
export function useNoticeLog(day: string | null): NoticeLoad {
  // Keyed by the day it is for, so switching day reads as loading without a reset in the effect.
  const [got, setGot] = useState<{ day: string | null; load: NoticeLoad } | null>(null);
  useEffect(() => {
    let live = true;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const fetchOnce = () => {
      const token = getToken();
      const q = day ? `?day=${encodeURIComponent(day)}` : "";
      fetch(serverUrl(`/api/notices${q}`), { headers: token ? { Authorization: `Bearer ${token}` } : {} })
        .then(async (res) => {
          if (!res.ok) throw new Error(res.status === 401 ? "Sign in to see the notification log." : `The server answered ${res.status}.`);
          const log = parseNoticeLog(await res.json());
          if (!log) throw new Error("The server's answer was not a notification log.");
          if (live) setGot({ day, load: { state: "ok", log } });
        })
        .catch((e: unknown) => {
          const message = e instanceof Error ? e.message : String(e);
          if (live) setGot((prev) => (prev && prev.day === day && prev.load.state === "ok" ? prev : { day, load: { state: "error", message } }));
        })
        .finally(() => {
          if (live && !document.hidden) timer = setTimeout(fetchOnce, REFRESH_MS);
        });
    };
    const onVisible = () => {
      if (!document.hidden) {
        clearTimeout(timer);
        fetchOnce();
      }
    };
    fetchOnce();
    document.addEventListener("visibilitychange", onVisible);
    return () => {
      live = false;
      clearTimeout(timer);
      document.removeEventListener("visibilitychange", onVisible);
    };
  }, [day]);
  return got && got.day === day ? got.load : LOADING;
}

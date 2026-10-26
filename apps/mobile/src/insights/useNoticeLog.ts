import { authenticatedFetch, parseNoticeLog, type NoticeLogWire } from "@stockwatcher/shared";
import { useEffect, useState } from "react";
import { AppState } from "react-native";

import { SERVER_URL } from "../feed/server";
import { getAccessToken } from "../storage/secure";

/** How often the open log refetches its day (only while the app is in the foreground). */
const REFRESH_MS = 15_000;

export type NoticeLoad = { state: "loading" } | { state: "error"; message: string } | { state: "ok"; log: NoticeLogWire };
const LOADING: NoticeLoad = { state: "loading" };

/**
 * GET /api/notices for `day` (null = today, the server's ET date), the web's useNoticeLog on the
 * phone: refetched every 15 s while the app is active, at once when it comes back. A failed
 * refresh keeps the last good log; only a day's first load reports the error.
 */
export function useNoticeLog(day: string | null): NoticeLoad {
  const [got, setGot] = useState<{ day: string | null; load: NoticeLoad } | null>(null);
  useEffect(() => {
    let live = true;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const fetchOnce = async () => {
      try {
        const token = await getAccessToken();
        const q = day ? `?day=${encodeURIComponent(day)}` : "";
        const res = await authenticatedFetch(`${SERVER_URL}/api/notices${q}`, token);
        if (!res.ok) throw new Error(`The server answered ${res.status}.`);
        const log = parseNoticeLog(await res.json());
        if (!log) throw new Error("The server's answer was not a notification log.");
        if (live) setGot({ day, load: { state: "ok", log } });
      } catch (e) {
        const message = e instanceof Error && e.name === "UnauthorizedError" ? "Sign in to see the notification log." : e instanceof Error ? e.message : String(e);
        if (live) setGot((prev) => (prev && prev.day === day && prev.load.state === "ok" ? prev : { day, load: { state: "error", message } }));
      } finally {
        if (live && AppState.currentState === "active") timer = setTimeout(() => void fetchOnce(), REFRESH_MS);
      }
    };
    const sub = AppState.addEventListener("change", (s) => {
      if (s === "active") {
        clearTimeout(timer);
        void fetchOnce();
      }
    });
    void fetchOnce();
    return () => {
      live = false;
      clearTimeout(timer);
      sub.remove();
    };
  }, [day]);
  return got && got.day === day ? got.load : LOADING;
}

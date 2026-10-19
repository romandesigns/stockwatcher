// The replay day menu's data: GET /api/replay/state's `day` and `days`
// (crates/server/src/market.rs `state_json`). Fetched, not pushed: the list
// changes a few times a month, so the socket does not carry it. It is read
// once on start, again whenever the menu opens (a day imported since then
// shows up), and again whenever the clock's trading day changes, which is how
// a switch made from another client or device reaches this one's label.
import { parseReplayDayState, type ReplayDayState } from "@stockwatcher/chart-core";
import { useCallback, useEffect, useRef, useState } from "react";
import type { SourceChoice } from "./source.ts";
import { getToken } from "./token.ts";

export interface ReplayDays {
  /** null until the server has answered (and always for the in-browser fixture). */
  state: ReplayDayState | null;
  refresh: () => void;
}

export function useReplayDays(choice: SourceChoice, clockDay: string | null): ReplayDays {
  const [state, setState] = useState<ReplayDayState | null>(null);
  // Answers can come back out of order (a slow one, then a fast one): only the latest request
  // may set the state, or the menu could flash back to a stale day.
  const latest = useRef(0);
  const refresh = useCallback(() => {
    if (choice.kind !== "server") return;
    const id = ++latest.current;
    const token = getToken();
    // The replay market's days: only the practice page (replay) has a day menu.
    void fetch("/api/replay/state?source=replay", { headers: token ? { Authorization: `Bearer ${token}` } : {} })
      .then((res) => (res.ok ? res.json() : null))
      .then((body: unknown) => {
        if (id === latest.current && body) setState(parseReplayDayState(body));
      })
      .catch(() => {
        // Offline: keep the last list; the connection status says so.
      });
  }, [choice.kind]);
  useEffect(() => refresh(), [refresh, clockDay]);
  return { state, refresh };
}

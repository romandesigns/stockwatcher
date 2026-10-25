// The replay bar's alert dots: GET /api/replay/alerts?day= (crates/server/src/replay_alerts.rs),
// every Runners entry the live server logged on the replay day, all tickers (the bar filters to
// the shown one, and falls back to every ticker for its "any" jumps). Fetched once per day: a past
// day's log does not change, and the bar has no business on the socket.
import { parseReplayAlerts, type ReplayAlert } from "@stockwatcher/chart-core";
import { serverUrl } from "@stockwatcher/shared";
import { useEffect, useState } from "react";
import type { SourceChoice } from "./source.ts";
import { getToken } from "./token.ts";

const NONE: readonly ReplayAlert[] = [];

export function useReplayAlerts(choice: SourceChoice, day: string | null): readonly ReplayAlert[] {
  const [got, setGot] = useState<{ day: string; alerts: readonly ReplayAlert[] } | null>(null);
  useEffect(() => {
    if (choice.kind !== "server" || !day) return;
    let live = true;
    const token = getToken();
    void fetch(serverUrl(`/api/replay/alerts?day=${encodeURIComponent(day)}`), { headers: token ? { Authorization: `Bearer ${token}` } : {} })
      .then((res) => (res.ok ? res.json() : null))
      .then((body: unknown) => {
        const parsed = parseReplayAlerts(body);
        if (live && parsed) setGot(parsed);
      })
      .catch(() => {
        // Offline or an older server without the route: no dots, the jumps disable.
      });
    return () => {
      live = false;
    };
  }, [choice.kind, day]);
  // Another day's answer never shows on this one.
  return got && got.day === day ? got.alerts : NONE;
}

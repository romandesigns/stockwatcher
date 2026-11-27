// The replay bar's Play/Pause on a feed: a press shows at once and is sent (or held for the
// reconnect), the server's clock confirms it, and an unconfirmed press gives way to the server
// after chart-core's PLAY_INTENT_MS (replayPlay.ts, the Practice-page freeze of 2026-11-26).
import { PLAY_INTENT_MS, pressPlay, shownPlay, type Clock, type PlayIntent, type PlayShown } from "@stockwatcher/chart-core";
import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import type { ChartFeed } from "./types.ts";

export function useReplayPlay(feed: ChartFeed, clock: Clock | null): PlayShown & { press: () => void } {
  const server = clock ? clock.playing : null;
  const [intent, setIntent] = useState<PlayIntent | null>(null);
  // Confirmed: the server is the truth from here on, so a change made on another device a moment
  // later shows at once. Adjusted during render (React's "state from a prop change"), no effect.
  if (intent && server === intent.playing) setIntent(null);
  // An unconfirmed press expires here, so a render never reads the time: while it stands it is
  // shown, unless the server already agrees.
  useEffect(() => {
    if (!intent) return;
    const id = setTimeout(() => setIntent((cur) => (cur === intent ? null : cur)), Math.max(0, intent.at + PLAY_INTENT_MS - Date.now()));
    return () => clearTimeout(id);
  }, [intent]);
  const latest = useRef({ server, intent });
  useLayoutEffect(() => {
    latest.current = { server, intent };
  });
  const press = useCallback(() => {
    const { server: s, intent: i } = latest.current;
    const next = pressPlay(s, i, Date.now());
    setIntent(next.intent);
    feed.control({ action: next.action });
  }, [feed]);
  return { ...shownPlay(server, intent, intent?.at ?? 0), press };
}

import { afterEach, describe, expect, test } from "bun:test";
import { createAppStore } from "@stockwatcher/app-state";
import { currentReplayDay, parseReplayDayState, replayDayOptions, sessionBands, type ChartMessage, type Clock } from "@stockwatcher/chart-core";
import { ReplayBar } from "@stockwatcher/chart-web";
import { createSettingsSync } from "@stockwatcher/shared";
import { renderToStaticMarkup } from "react-dom/server";
import { createServerFeed } from "./serverFeed.ts";
import { chooseSource, followCandidates, initialSymbol, type ServerSymbols } from "./source.ts";

// Owner, 2026-10-23: "The replay feature appears to be broken. I cannot select a date nor a ticker
// so I can practice placing trades."
//
// Root cause: the chart page decides its source once, from GET /api/symbols?source=replay. While
// the replay market is still building its start (one to two minutes on the VPS after every
// deploy, and again on every day switch or seek), that answer is `status: "running"` with an
// archived `day` but no candidates yet. chooseSource read that as "server has no data" and put
// the page on the in-browser fixture for the rest of the session: no day menu (the fixture has no
// days) and only fixture tickers in the search. These tests walk both selections through the
// pieces the chart page (App.tsx) wires together: source choice, the socket, the day state, the
// replay bar and the ticker the chart shows.

/** /api/symbols?source=replay while the replay market rebuilds to REPLAY_START (measured locally on a production archive day). */
const building: ServerSymbols = { mode: "replay", status: "running", source: "archive 2026-10-23", day: "2026-10-23", fixture: false, symbols: [], screened: { total: 6208, candidates: 0 } };
/** The same route once the rebuild has reached the first in-band trades. */
const built: ServerSymbols = { ...building, symbols: ["AAL", "ACHR", "CJMB"], screened: { total: 6208, candidates: 3 } };
/** /api/replay/state?source=replay (crates/server/src/market.rs `state_json`), the day menu's data. */
const dayState = { day: "2026-10-23", days: [{ day: "2026-10-22", fixture: false, symbols: 6820 }, { day: "2026-10-23", fixture: false, symbols: 6208 }] };

const CLOCK: Clock = { mode: "replay", now: Date.UTC(2026, 9, 23, 13, 30), playing: false, speed: 10 };

class Socket {
  static instances: Socket[] = [];
  readyState = 0;
  onopen: ((ev: unknown) => void) | null = null;
  onmessage: ((ev: { data: unknown }) => void) | null = null;
  onclose: ((ev: unknown) => void) | null = null;
  onerror: ((ev: unknown) => void) | null = null;
  sent: Record<string, unknown>[] = [];
  listeners: ((ev: { data: unknown }) => void)[] = [];
  constructor() {
    Socket.instances.push(this);
  }
  addEventListener(_type: string, fn: (ev: { data: unknown }) => void) {
    this.listeners.push(fn);
  }
  send(data: string) {
    this.sent.push(JSON.parse(data));
  }
  close() {
    this.readyState = 3;
    this.onclose?.({});
  }
  open() {
    this.readyState = 1;
    this.onopen?.({});
  }
  receive(value: object) {
    for (const fn of this.listeners) fn({ data: JSON.stringify(value) });
  }
}

const realSocket = globalThis.WebSocket;
let cleanup: (() => void) | undefined;
afterEach(() => {
  cleanup?.();
  cleanup = undefined;
  globalThis.WebSocket = realSocket;
  Socket.instances = [];
});

/** The chart page's server feed on a fake socket, opened and welcomed. */
function openFeed() {
  globalThis.WebSocket = Socket as unknown as typeof WebSocket;
  const feed = createServerFeed({ url: "ws://local-fixture.invalid/ws", source: "replay" });
  cleanup = () => feed.close();
  const socket = Socket.instances[0]!;
  socket.open();
  socket.receive({ type: "welcome", version: "test", commit: "test", heartbeatSecs: 15 });
  return { feed, socket };
}

/** The replay bar as App.tsx renders it (days from /api/replay/state, the clock from the socket). */
function replayBar(clock: Clock | null, pendingDay: string | null = null, state = parseReplayDayState(dayState)) {
  return renderToStaticMarkup(
    <ReplayBar
      layout="desktop"
      clock={clock}
      onPlayPause={() => {}}
      onSpeedChange={() => {}}
      onSeek={() => {}}
      bands={sessionBands({ year: 2026, month: 10, day: 23 })}
      candles={[]}
      symbol="AAL"
      price="12.34"
      onTickerClick={() => {}}
      days={replayDayOptions(state.days)}
      day={currentReplayDay(state, clock)}
      onDayChange={() => {}}
      pendingDay={pendingDay}
    />,
  );
}

describe("replay practice: a page opened while the replay builds (owner, 2026-10-23)", () => {
  test("stays on the server instead of falling back to the in-browser fixture for good", () => {
    const c = chooseSource(undefined, { ok: true, data: building }, "SMPL", "replay");
    expect(c).toMatchObject({ kind: "server", symbols: [], fixtureData: false, label: "Server replay · 2026-10-23", market: "replay" });
    // Still the fixture when the server really has nothing to replay, or no day at all.
    expect(chooseSource(undefined, { ok: true, data: { ...building, status: "no_data" } }, "SMPL", "replay").kind).toBe("fixture");
    expect(chooseSource(undefined, { ok: true, data: { ...building, status: "idle", day: null } }, "SMPL", "replay").kind).toBe("fixture");
    expect(chooseSource(undefined, { ok: true, data: { ...building, day: null } }, "SMPL", "replay").kind).toBe("fixture");
  });

  test("date: the day menu appears when the rebuild's clock arrives, and a pick goes to the server", () => {
    // Before the clock: the bar says so, and there is nothing to pick yet.
    expect(replayBar(null)).toContain("Clock not started");
    expect(replayBar(null)).not.toContain("Replay day");

    const { feed, socket } = openFeed();
    let clock = null as Clock | null;
    const off = feed.subscribe({ symbol: "AAL", timeframe: "1m", profileBin: 0.01 }, (m: ChartMessage) => {
      if (m.type === "replay.clock") clock = m.clock;
    });
    // The rebuild ends with a clock frame (crates/server/src/market.rs FeedItem::Clock → Resync).
    socket.receive({ type: "replay.clock", clock: CLOCK });
    expect(clock).toEqual(CLOCK);
    const html = replayBar(clock);
    expect(html).toContain('aria-label="Replay day Friday, October 23, 2026, change"');
    expect(html).toContain("Oct 23");

    // Picking Thursday sends exactly the server's day control.
    feed.control({ action: "day", value: "2026-10-22" });
    expect(socket.sent).toContainEqual({ type: "replay.control", action: "day", value: "2026-10-22" });

    // The rebuild onto the new day takes a minute or two: the date says it is loading, rather than
    // showing the old day as if the pick had been ignored.
    const loading = replayBar(clock, "2026-10-22");
    expect(loading).toContain("Loading Oct 22…");
    expect(loading).toContain("loading Oct 22");
    // Done: the server names the new day and the label is the menu again.
    const after = replayBar({ ...CLOCK, now: CLOCK.now - 86_400_000 }, "2026-10-22", parseReplayDayState({ ...dayState, day: "2026-10-22" }));
    expect(after).not.toContain("Loading");
    expect(after).toContain('aria-label="Replay day Thursday, October 22, 2026, change"');
    off();
  });

  test("ticker: the search's list fills from the server and a pick is the chart's ticker", () => {
    const choice = chooseSource(undefined, { ok: true, data: building }, "SMPL", "replay");
    // The candidates the ticker search offers (App.tsx: useCandidates, starting from the choice).
    // The fixture's single ticker is never offered for a real replay day.
    let candidates = choice.symbols;
    expect(candidates).not.toContain("SMPL");
    // useCandidates' next poll, after the rebuild.
    candidates = chooseSource(undefined, { ok: true, data: built }, "SMPL", "replay").symbols;
    expect(candidates).toEqual(["AAL", "ACHR", "CJMB"]);

    // The chart page's ticker (App.tsx): the synced ticker through followCandidates.
    let n = 0;
    const sync = createSettingsSync({ initial: null, newOpId: () => `op-${++n}`, setTimer: () => 0, clearTimer: () => {} });
    const store = createAppStore({ sync });
    const shown = () => followCandidates(store.getState().symbol ?? initialSymbol(choice, "SMPL"), candidates, store.getState().picked);
    // A ticker this replay day does not have (yesterday's pick, another device's) gives way to a candidate.
    store.getState().openTicker("ZZZZ");
    expect(followCandidates("ZZZZ", candidates, [])).toBe("AAL");
    // The search's pick (TickerSearch onSelect = app.openTicker) is what the chart shows.
    store.getState().openTicker("acHR");
    expect(shown()).toBe("ACHR");

    // And the chart subscribes to it on the replay socket.
    const { feed, socket } = openFeed();
    const off = feed.subscribe({ symbol: shown(), timeframe: "1m", profileBin: 0.01 }, () => {});
    expect(socket.sent).toContainEqual({ type: "chart.subscribe", symbol: "ACHR", timeframe: "1m", profileBin: 0.01 });
    off();
  });
});

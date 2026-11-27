import { describe, expect, test } from "bun:test";
import { CONTROL_QUEUE_MS, PLAY_INTENT_MS, dueControls, playIntentDeadline, pressPlay, queueControl, shownPlay } from "./replayPlay.ts";

describe("the Play button's state machine", () => {
  test("with no press it shows the server, paused before any clock", () => {
    expect(shownPlay(null, null, 0)).toEqual({ playing: false, pending: false });
    expect(shownPlay(true, null, 0)).toEqual({ playing: true, pending: false });
    expect(shownPlay(false, null, 0)).toEqual({ playing: false, pending: false });
  });

  test("a press shows at once, pending, until the server's clock agrees", () => {
    const p = pressPlay(false, null, 1000);
    expect(p.action).toBe("play");
    expect(shownPlay(false, p.intent, 1001)).toEqual({ playing: true, pending: true });
    // The clock confirms: the button is the server again.
    expect(shownPlay(true, p.intent, 1200)).toEqual({ playing: true, pending: false });
  });

  test("a second press undoes the first even before the server answered (the freeze: pause, pause, pause)", () => {
    // The clock frame saying "paused" never came: the button must not keep sending "pause".
    const first = pressPlay(true, null, 0);
    expect(first.action).toBe("pause");
    const second = pressPlay(true, first.intent, 300);
    expect(second.action).toBe("play");
    const third = pressPlay(true, second.intent, 600);
    expect(third.action).toBe("pause");
  });

  test("an unconfirmed press gives way to the server after the window", () => {
    const p = pressPlay(false, null, 0);
    expect(shownPlay(false, p.intent, PLAY_INTENT_MS - 1).pending).toBe(true);
    expect(playIntentDeadline(false, p.intent, 10)).toBe(PLAY_INTENT_MS);
    expect(shownPlay(false, p.intent, PLAY_INTENT_MS)).toEqual({ playing: false, pending: false });
    expect(playIntentDeadline(false, p.intent, PLAY_INTENT_MS)).toBeNull();
    // And the next press then toggles the server's state, not the stale intent.
    expect(pressPlay(false, p.intent, PLAY_INTENT_MS).action).toBe("play");
  });

  test("another device's change shows as soon as no press of ours stands", () => {
    const p = pressPlay(false, null, 0);
    expect(shownPlay(true, p.intent, 10).playing).toBe(true);
    // Paused from elsewhere afterwards: our confirmed press no longer counts.
    expect(shownPlay(false, null, 20)).toEqual({ playing: false, pending: false });
  });
});

type Cmd = { action: string; value?: unknown };

describe("commands held for the reconnect", () => {
  test("the newest of each kind, oldest kind first", () => {
    let q = queueControl<Cmd>([], { action: "pause" }, 0);
    q = queueControl(q, { action: "speed", value: 5 }, 1);
    q = queueControl(q, { action: "play" }, 2);
    q = queueControl(q, { action: "seek", value: 10 }, 3);
    q = queueControl(q, { action: "day", value: "2026-11-23" }, 4);
    expect(dueControls(q, 5)).toEqual([{ action: "speed", value: 5 }, { action: "play" }, { action: "day", value: "2026-11-23" }]);
  });

  test("nothing older than the window is sent on the reconnect", () => {
    const q = queueControl<Cmd>(queueControl<Cmd>([], { action: "play" }, 0), { action: "speed", value: 2 }, CONTROL_QUEUE_MS);
    expect(dueControls(q, CONTROL_QUEUE_MS + 1)).toEqual([{ action: "speed", value: 2 }]);
  });
});

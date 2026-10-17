import { describe, expect, test } from "bun:test";

import { FOREGROUND_CHECK_INTERVAL_MS, shouldCheckForUpdate } from "./policy";

const idle = { lastCheckedAt: null, inFlight: false, updatePending: false };

describe("shouldCheckForUpdate", () => {
  test("checks when it never has", () => {
    expect(shouldCheckForUpdate(idle, 1_000)).toBe(true);
  });

  test("throttles inside the interval and checks after it", () => {
    const state = { ...idle, lastCheckedAt: 10_000 };
    expect(shouldCheckForUpdate(state, 10_000 + FOREGROUND_CHECK_INTERVAL_MS - 1)).toBe(false);
    expect(shouldCheckForUpdate(state, 10_000 + FOREGROUND_CHECK_INTERVAL_MS)).toBe(true);
  });

  test("never overlaps a running check", () => {
    expect(shouldCheckForUpdate({ ...idle, inFlight: true }, 1_000)).toBe(false);
  });

  test("does nothing while a downloaded update waits for restart", () => {
    expect(shouldCheckForUpdate({ ...idle, updatePending: true }, 1_000)).toBe(false);
  });

  test("a clock that went backwards does not block checks", () => {
    expect(shouldCheckForUpdate({ ...idle, lastCheckedAt: 50_000 }, 1_000)).toBe(true);
  });
});

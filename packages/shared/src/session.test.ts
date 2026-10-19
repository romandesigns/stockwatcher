import { describe, expect, test } from "bun:test";
import { lastSessionLabel, parseLiveSession } from "./session.ts";

describe("the live market's last-session label", () => {
  test("a past session is labelled with its trading date, provisional when recorded", () => {
    expect(lastSessionLabel({ day: "2026-10-16", current: false, provisional: false })).toBe("Last session · Fri Oct 16");
    expect(lastSessionLabel({ day: "2026-10-16", current: false, provisional: true })).toBe("Last session · Fri Oct 16 · provisional");
  });

  test("the current session, or none, needs no label", () => {
    expect(lastSessionLabel({ day: "2026-10-19", current: true, provisional: true })).toBeNull();
    expect(lastSessionLabel(null)).toBeNull();
  });

  test("parses the server's field and refuses anything else", () => {
    expect(parseLiveSession({ day: "2026-10-16", current: false, provisional: true })).toEqual({ day: "2026-10-16", current: false, provisional: true });
    expect(parseLiveSession(null)).toBeNull();
    expect(parseLiveSession({ day: "Oct 16" })).toBeNull();
  });
});

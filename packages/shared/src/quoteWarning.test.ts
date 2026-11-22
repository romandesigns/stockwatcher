import { describe, expect, test } from "bun:test";
import { parseRunnersNotice } from "./runnersNotify.ts";
import { quoteWarningView } from "./quoteWarning.ts";

describe("no live quote warning", () => {
  test("only an explicit true shows it", () => {
    expect(quoteWarningView(true)?.label).toBe("No live quote");
    expect(quoteWarningView(true)?.short).toBe("No quote");
    for (const v of [false, null, undefined, "true", 1]) expect(quoteWarningView(v)).toBeNull();
  });

  test("runners.notice carries noQuote only when true", () => {
    const frame = { type: "runners.notice", id: "runners-fast-WFF-1", alertId: "WFF-1", symbol: "WFF", kind: "runners.fast", title: "t", body: "b", at: 1 };
    expect(parseRunnersNotice({ ...frame, spreadBp: null, noQuote: true })!.noQuote).toBe(true);
    expect(parseRunnersNotice({ ...frame, noQuote: null })!.noQuote).toBeUndefined();
    expect(parseRunnersNotice(frame)!.noQuote).toBeUndefined();
  });
});

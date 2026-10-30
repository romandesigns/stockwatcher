import { describe, expect, test } from "bun:test";
import { alpacaUnavailable, parseBrokerStatus, parseTradingAccount } from "./trading.ts";

describe("trading account settings", () => {
  test("only an explicit alpaca_paper routes", () => {
    expect(parseTradingAccount({ live: "alpaca_paper" }).live).toBe("alpaca_paper");
    for (const v of [null, undefined, "alpaca_paper", { live: "alpaca" }, { live: "alpaca_live" }, {}]) expect(parseTradingAccount(v).live).toBe("simulator");
  });

  test("broker status parses and flags an unusable route", () => {
    const s = parseBrokerStatus({ alpacaPaper: { configured: false, state: "not_configured", message: "set keys" } });
    expect(s).toEqual({ configured: false, state: "not_configured", message: "set keys" });
    expect(alpacaUnavailable(s)).toBe(true);
    expect(alpacaUnavailable(parseBrokerStatus({ alpacaPaper: { configured: true, state: "live", message: "" } }))).toBe(false);
    expect(parseBrokerStatus(42).state).toBe("unknown");
  });
});

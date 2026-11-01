import { describe, expect, test } from "bun:test";
import { parseServerMessage } from "./protocol.ts";
import { wireMs } from "./wireTime.ts";

const nsJson = (ms: number) => (BigInt(ms) * 1_000_000n).toString();

describe("board frames prefer the exchange-ns clock (times.ns.v1)", () => {
  const t0 = Date.UTC(2026, 9, 15, 14, 0, 1);

  test("t comes from tNs; the ms field is only the fallback", () => {
    for (const type of ["quotes.snapshot", "movers.update", "pressure.snapshot", "rocket.update", "favorites.snapshot", "quotes.search.update"]) {
      const m = parseServerMessage(`{"type":"${type}","t":5,"tNs":${nsJson(t0)}}`) as unknown as { t: number };
      expect([type, m.t]).toEqual([type, t0]);
    }
  });

  test("an older server's frame keeps its ms t, and a null clock stays null", () => {
    expect((parseServerMessage(`{"type":"movers.snapshot","t":${t0}}`) as unknown as { t: number }).t).toBe(t0);
    expect((parseServerMessage(`{"type":"movers.snapshot","t":null,"tNs":null}`) as unknown as { t: null }).t).toBeNull();
  });

  test("whole-ms twins round-trip exactly despite JSON.parse rounding", () => {
    for (let ms = t0; ms < t0 + 3600_000; ms += 997) expect(wireMs(-1, JSON.parse(nsJson(ms)))).toBe(ms);
  });

  test("a sub-ms twin just under the next ms keeps the server's agreeing ms", () => {
    const ms = t0 + 999;
    const ns = JSON.parse(`${BigInt(ms) * 1_000_000n + 999_800n}`) as number;
    expect(wireMs(ms, ns)).toBe(ms);
  });
});

import { describe, expect, test } from "bun:test";
import { parseChartMessage, type ChartSnapshot } from "./types.ts";
import { wireMs, wireMsOrNull } from "./wireTime.ts";

// A twin as the server writes it: an exact decimal integer that JSON.parse rounds.
const nsJson = (ms: number, extraNs = 0n) => (BigInt(ms) * 1_000_000n + extraNs).toString();
const parsedNs = (ms: number, extraNs = 0n) => JSON.parse(nsJson(ms, extraNs)) as number;

describe("wireMs", () => {
  test("a whole-ms twin comes back as exactly the ms the server floored it to", () => {
    // Every second over a trading day and then some, plus random ms across years: the rounding
    // of an epoch-ns double goes both ways, and a plain floor would be a ms early on half of them.
    const day = Date.UTC(2026, 9, 15, 8, 0, 0);
    for (let ms = day; ms < day + 16 * 3600_000; ms += 1000) expect(wireMs(-1, parsedNs(ms))).toBe(ms);
    let x = 12345;
    for (let i = 0; i < 20_000; i++) {
      x = (x * 1103515245 + 12345) % 2147483648;
      const ms = Date.UTC(2020, 0, 1) + (x % (10 * 365 * 86_400_000 / 1000)) * 1000 + (x % 1000);
      expect(wireMs(-1, parsedNs(ms))).toBe(ms);
    }
  });

  test("a time inside a millisecond floors like the server's", () => {
    const ms = Date.UTC(2026, 9, 15, 14, 0, 1, 250);
    for (const extra of [1n, 250_000n, 500_000n, 998_000n]) expect(wireMs(-1, parsedNs(ms, extra))).toBe(ms);
  });

  test("absent, null, zero or junk twins fall back to the ms field", () => {
    for (const ns of [undefined, null, 0, -5, Number.NaN, "1793017380000000000"]) expect(wireMs(42, ns)).toBe(42);
    expect(wireMsOrNull(null, undefined)).toBeNull();
    expect(wireMsOrNull(null, null)).toBeNull();
    expect(wireMsOrNull(7, null)).toBe(7);
    expect(wireMsOrNull(null, parsedNs(1_793_017_380_000))).toBe(1_793_017_380_000);
  });
});

describe("parseChartMessage prefers the twins", () => {
  const t0 = Date.UTC(2026, 9, 15, 14, 0, 0);

  test("snapshot: every time from its twin; the ms fields only as the fallback", () => {
    // Twins 5 ms off their ms fields, so the test sees which one won.
    const raw = `{"type":"chart.snapshot","symbol":"SMPL","timeframe":"1m","session":{"id":"2026-10-15/regular","start":1,"startNs":${nsJson(t0 + 5)},"end":2,"endNs":${nsJson(t0 + 6)}},`
      + `"candles":[{"t":3,"tNs":${nsJson(t0 + 7)},"o":1,"h":1,"l":1,"c":1,"v":1,"n":1,"final":true},{"t":${t0 + 60_000},"o":1,"h":1,"l":1,"c":1,"v":1,"n":1,"final":false}],`
      + `"gaps":[{"t":4,"tNs":${nsJson(t0 + 8)},"end":5,"endNs":${nsJson(t0 + 9)},"reason":"no_trades","v":0,"intervals":1}],"profile":null,"quality":{},`
      + `"clock":{"mode":"replay","now":6,"nowNs":${nsJson(t0 + 10)},"speed":1,"playing":true},`
      + `"halts":[{"start":7,"startNs":${nsJson(t0 + 11, 999n)},"end":null,"endNs":null,"reason":"LUDP","code":"H"}],"luld":[{"t":8,"tNs":${nsJson(t0 + 12)},"ref":null,"up":null,"down":null}]}`;
    const s = parseChartMessage(raw) as ChartSnapshot;
    expect([s.session!.start, s.session!.end]).toEqual([t0 + 5, t0 + 6]);
    expect(s.candles.map((c) => c.t)).toEqual([t0 + 7, t0 + 60_000]);
    expect([s.gaps[0]!.t, s.gaps[0]!.end]).toEqual([t0 + 8, t0 + 9]);
    expect(s.clock!.now).toBe(t0 + 10);
    expect([s.halts![0]!.start, s.halts![0]!.end]).toEqual([t0 + 11, null]);
    expect(s.luld![0]!.t).toBe(t0 + 12);
  });

  test("increments, clock, tape, indicators, levels and zones", () => {
    const p = (raw: string) => parseChartMessage(raw) as unknown as Record<string, any>;
    expect(p(`{"type":"chart.update","symbol":"S","timeframe":"1m","session":"x","candle":{"t":1,"tNs":${nsJson(t0)},"o":1,"h":1,"l":1,"c":1,"v":1,"n":1,"final":false},"profileBin":0.01,"profile":[]}`).candle.t).toBe(t0);
    expect(p(`{"type":"chart.remove","symbol":"S","timeframe":"1m","session":"x","t":1,"tNs":${nsJson(t0)},"profileBin":0.01,"profile":[]}`).t).toBe(t0);
    const halt = p(`{"type":"chart.halt","symbol":"S","halt":{"start":1,"startNs":${nsJson(t0)},"end":2,"endNs":${nsJson(t0 + 90_000)},"reason":"LUDP","code":"H"}}`).halt;
    expect([halt.start, halt.end]).toEqual([t0, t0 + 90_000]);
    expect(p(`{"type":"replay.clock","clock":{"mode":"live","now":1,"nowNs":${nsJson(t0)},"speed":1,"playing":true}}`).clock.now).toBe(t0);
    expect(p(`{"type":"tape.trade","symbol":"S","t":1,"tsNs":${nsJson(t0, 123_456n)},"p":1,"s":1,"exchange":"V","seq":1,"conditions":[],"counted":"ohlc_and_volume","reason":null,"session":null}`).t).toBe(t0);
    expect(p(`{"type":"chart.indicators","symbol":"S","timeframe":"1m","rth":false,"t":[1,2],"tNs":[${nsJson(t0)},${nsJson(t0 + 60_000)}],"studies":[]}`).t).toEqual([t0, t0 + 60_000]);
    // A twin column that does not line up is ignored rather than half applied.
    expect(p(`{"type":"chart.indicators","symbol":"S","timeframe":"1m","rth":false,"t":[1,2],"tNs":[${nsJson(t0)}],"studies":[]}`).t).toEqual([1, 2]);
    const inc = p(`{"type":"chart.indicator","symbol":"S","timeframe":"1m","rth":false,"points":[{"t":1,"tNs":${nsJson(t0)},"v":{}}],"removed":[2],"removedNs":[${nsJson(t0 + 60_000)}]}`);
    expect([inc.points[0].t, inc.removed[0]]).toEqual([t0, t0 + 60_000]);
    const lv = p(`{"type":"chart.levels","symbol":"S","timeframe":"1m","day":"2026-10-15","pmHigh":null,"pmLow":null,"hod":null,"lod":null,"open":null,"prevClose":null,"openingRanges":[{"day":"2026-10-15","t":1,"tNs":${nsJson(t0)},"end":2,"endNs":${nsJson(t0 + 1)},"high":1,"low":1,"forming":true}]}`);
    expect([lv.openingRanges[0].t, lv.openingRanges[0].end]).toEqual([t0, t0 + 1]);
    expect(p(`{"type":"chart.zones","symbol":"S","timeframe":"1m","rth":false,"zones":[{"kind":"demand","lo":1,"hi":2,"t":1,"tNs":${nsJson(t0)},"tested":false}]}`).zones[0].t).toBe(t0);
  });

  test("frames from an older server (no twins) keep their ms times", () => {
    const up = parseChartMessage(`{"type":"chart.update","symbol":"S","timeframe":"1m","session":"x","candle":{"t":${t0},"o":1,"h":1,"l":1,"c":1,"v":1,"n":1,"final":false},"profileBin":0.01,"profile":[]}`) as unknown as { candle: { t: number } };
    expect(up.candle.t).toBe(t0);
    const h = parseChartMessage(`{"type":"chart.halt","symbol":"S","halt":{"start":${t0},"end":null,"reason":"LUDP","code":"H"}}`) as unknown as { halt: { start: number; end: null } };
    expect(h.halt).toMatchObject({ start: t0, end: null });
  });
});

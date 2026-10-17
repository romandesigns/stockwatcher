// Contract test against the Rust engine's own sources (read-only):
// - the golden JSON strings in crates/chart-engine/tests/wire.rs must parse
//   and apply through chart-core;
// - every serialized struct in crates/chart-engine/src/wire.rs (and
//   DataQuality in quality.rs) must have exactly the fields the TS types
//   declare. If the engine adds, drops or renames a field, this fails.
import { describe, expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describeCounted } from "./inspect.ts";
import { ChartStore } from "./store.ts";
import { fixtureSnapshot } from "./testFixture.ts";
import { parseChartMessage, type ChartUpdate, type TapeTrade } from "./types.ts";

const crate = join(import.meta.dir, "../../../crates/chart-engine");
const read = (p: string) => readFileSync(join(crate, p), "utf8");

/** Field names of `pub struct Name { … }` after serde camelCase and explicit renames. */
function rustFields(src: string, name: string): string[] {
  const m = new RegExp(`pub struct ${name} \\{([\\s\\S]*?)\\n\\}`).exec(src);
  if (!m) throw new Error(`struct ${name} not found`);
  const out: string[] = [];
  let rename: string | null = null;
  for (const line of m[1]!.split("\n")) {
    const r = /#\[serde\(rename = "([^"]+)"\)\]/.exec(line);
    if (r) rename = r[1]!;
    const f = /^\s*pub (\w+):/.exec(line);
    if (f) {
      out.push(rename ?? f[1]!.replace(/_([a-z])/g, (_, c: string) => c.toUpperCase()));
      rename = null;
    }
  }
  return out.sort();
}

// The TS side of each struct (keep in step with types.ts).
const TS_FIELDS: Record<string, string[]> = {
  CandleWire: ["t", "o", "h", "l", "c", "v", "n", "final"],
  GapWire: ["t", "end", "reason", "v", "intervals"],
  ProfileRow: ["p", "v"],
  ValueArea: ["low", "high", "v"],
  ProfileWire: ["bin", "anchor", "rows", "total", "poc", "valueArea"],
  SessionInfo: ["id", "start", "end"],
  ClockInfo: ["mode", "now", "speed", "playing"],
  ChartSnapshot: ["type", "symbol", "timeframe", "session", "candles", "gaps", "profile", "quality", "clock"],
  CandleUpdate: ["symbol", "timeframe", "session", "candle", "profileBin", "profile"],
  CandleRemove: ["symbol", "timeframe", "session", "t", "profileBin", "profile"],
  GapUpdate: ["symbol", "timeframe", "session", "gap", "profileBin", "profile"],
  TapeTrade: ["symbol", "t", "tsNs", "p", "s", "exchange", "seq", "conditions", "counted", "reason", "session"],
};

const QUALITY_FIELDS = Object.keys(fixtureSnapshot().quality).sort();

describe("wire contract with crates/chart-engine", () => {
  const wire = read("src/wire.rs");

  test("struct fields match the TS types", () => {
    for (const [name, fields] of Object.entries(TS_FIELDS)) expect([name, rustFields(wire, name)]).toEqual([name, [...fields].sort()]);
    expect(rustFields(read("src/quality.rs"), "DataQuality")).toEqual(QUALITY_FIELDS);
  });

  test("message type tags and gap reasons match", () => {
    for (const tag of ["chart.update", "chart.remove", "chart.gap", "tape.trade"]) expect(wire).toContain(`#[serde(rename = "${tag}")]`);
    expect(wire).toContain('"chart.snapshot"');
    expect(wire).toMatch(/enum GapReason \{[\s\S]*NoTrades,[\s\S]*VolumeOnlyTrades,/);
  });

  test("golden increments parse and apply", () => {
    const golden = read("tests/wire.rs");
    // at(10, 0, 1, 250) on 2026-10-15 (EDT) and its 15 s interval start.
    const t = Date.UTC(2026, 9, 15, 14, 0, 1, 250);
    const vars: Record<string, string> = { t: String(t), ns: (BigInt(t) * 1_000_000n).toString(), start: String(Date.UTC(2026, 9, 15, 14, 0, 0)) };
    const templates = [...golden.matchAll(/r#"(\{\{"type":[\s\S]*?\}\})"#/g)].map((m) => m[1]!);
    expect(templates.length).toBeGreaterThanOrEqual(2);
    const msgs = templates.map((tpl) => parseChartMessage(tpl.replace(/\{(\w+)\}/g, (all, k: string) => vars[k] ?? all).replaceAll("{{", "{").replaceAll("}}", "}")));
    const tape = msgs.find((m) => m?.type === "tape.trade") as TapeTrade;
    const up = msgs.find((m) => m?.type === "chart.update") as ChartUpdate;
    expect(tape).toMatchObject({ symbol: "SMPL", t, p: 3.18, s: 100, exchange: "V", seq: 7, counted: "ohlc_and_volume", reason: null, session: "2026-10-15/regular" });
    expect(describeCounted(tape).label).toBe("Regular sale");
    expect(up).toMatchObject({ timeframe: "15s", session: "2026-10-15/regular", profileBin: 0.01, profile: [{ p: 3.18, v: 100 }] });

    const store = new ChartStore();
    store.applySnapshot({ ...fixtureSnapshot("15s", 0, []), candles: [], gaps: [], profile: null });
    expect(store.applyIncrement(up)).toBe(true);
    const s = store.getSeries("SMPL", "15s")!;
    expect(s.candles).toEqual([up.candle]);
    expect(s.profile).toMatchObject({ anchor: "2026-10-15/regular", bin: 0.01, total: 100, poc: 3.18, valueArea: { low: 3.18, high: 3.18, v: 100 } });
  });
});

import { describe, expect, test } from "bun:test";
import { applyHodMessage, EMPTY_HOD, filterHodRows, fmtHodPct, fmtHodShares, hodTint, parseHodFilter, toggleHodFilter, type HodPolicyWire, type HodRowWire, type HodSnapshotMessage, type HodUpdateMessage } from "./hod.ts";
import { parseServerMessage } from "./protocol.ts";

const policy: HodPolicyWire = {
  id: "hod-1", version: 1, band: [0.1, 25], cooldown_secs: 60, float_low_max: 10e6, float_medium_max: 50e6,
  rvol_high_min: 5, rvol_medium_min: 3, rvol_sessions: 30, rvol_base_min: 1, price_high_min: 20, momo_run_min: 0.3, momo_sessions: 90,
  categories: [
    { id: "lf_hrv", name: "Low Float - High Rel Vol", default_on: true, needs_history: false },
    { id: "mf_hrv_u20", name: "Medium Float - High Rel Vol - Price under $20", default_on: false, needs_history: false },
    { id: "sq_5_5", name: "Squeeze Alert - Up 5% in 5min", default_on: true, needs_history: false },
  ],
};
const row = (id: number, cats: string[]): HodRowWire => ({ id, t: 1_700_000_000_000 + id, sym: `S${id}`, price: 2, vol: 1e6, float: 5e6, rvol: 6, rvol5: 2, gap: 0.1, chg: 0.3, chg5: 0.05, chg10: 0.1, cats });
const head = { enabled: true, on: true, date: "2026-11-23", complete: true, since_ns: 1, hi52_available: false, facts_loaded: true };
const snap = (gen: number, rows: HodRowWire[]): HodSnapshotMessage => ({ type: "hod.snapshot", ...head, gen, policy, rows });
const upd = (prev_gen: number, gen: number, rows: HodRowWire[], date = "2026-11-23"): HodUpdateMessage => ({ type: "hod.update", ...head, date, gen, prev_gen, rows });

describe("HOD scanner frames", () => {
  test("a snapshot replaces, a continuing update prepends, a gap asks for a snapshot", () => {
    let s = applyHodMessage(EMPTY_HOD, snap(2, [row(2, ["lf_hrv"]), row(1, ["sq_5_5"])])).state;
    expect(s.rows.map((r) => r.id)).toEqual([2, 1]);
    const next = applyHodMessage(s, upd(2, 3, [row(3, ["lf_hrv"])]));
    expect(next.resync).toBe(false);
    s = next.state;
    expect(s.rows.map((r) => r.id)).toEqual([3, 2, 1]);
    expect(applyHodMessage(s, upd(5, 6, [row(6, ["lf_hrv"])])).resync).toBe(true);
    expect(applyHodMessage(s, upd(3, 4, [row(4, ["lf_hrv"])], "2026-11-24")).resync).toBe(true);
    expect(applyHodMessage(EMPTY_HOD, upd(0, 1, [row(1, ["lf_hrv"])])).resync).toBe(true);
  });

  test("the shared parser knows the frames", () => {
    expect(parseServerMessage(JSON.stringify(snap(0, [])))?.type).toBe("hod.snapshot");
    expect(parseServerMessage(JSON.stringify(upd(0, 1, [])))?.type).toBe("hod.update");
  });
});

describe("HOD category filter", () => {
  test("defaults come from the policy; a stored list wins; unknown ids drop", () => {
    expect([...parseHodFilter(null, policy)].sort()).toEqual(["lf_hrv", "sq_5_5"]);
    expect([...parseHodFilter({ on: ["mf_hrv_u20", "gone"] }, policy)]).toEqual(["mf_hrv_u20"]);
    expect(toggleHodFilter(new Set(["lf_hrv"]), "sq_5_5")).toEqual({ on: ["lf_hrv", "sq_5_5"] });
    expect(toggleHodFilter(new Set(["lf_hrv"]), "lf_hrv")).toEqual({ on: [] });
  });

  test("rows show with their shown tags only", () => {
    const out = filterHodRows([row(1, ["mf_hrv_u20"]), row(2, ["lf_hrv", "mf_hrv_u20"])], new Set(["lf_hrv"]));
    expect(out.map((r) => [r.row.id, r.cats])).toEqual([[2, ["lf_hrv"]]]);
  });

  test("formatting and tints", () => {
    expect(fmtHodPct(0.123)).toBe("+12.3%");
    expect(fmtHodPct(null)).toBe("—");
    expect(fmtHodShares(4_200_000)).toBe("4.2M");
    expect(hodTint("lf_momo")).toBe("lowFloat");
    expect(hodTint("mf_mrv_20")).toBe("medFloat");
    expect(hodTint("sq_52wk")).toBe("squeeze");
  });
});

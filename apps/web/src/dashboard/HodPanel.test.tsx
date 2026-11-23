import { describe, expect, test } from "bun:test";
import { EMPTY_HOD, type HodPolicyWire, type HodRowWire, type HodState } from "@stockwatcher/shared";
import { renderToStaticMarkup } from "react-dom/server";
import { HodPanelView } from "./HodPanel.tsx";

const policy: HodPolicyWire = {
  id: "hod-1", version: 1, band: [0.1, 25], cooldown_secs: 60, float_low_max: 10e6, float_medium_max: 50e6,
  rvol_high_min: 5, rvol_medium_min: 3, rvol_sessions: 30, rvol_base_min: 1, price_high_min: 20, momo_run_min: 0.3, momo_sessions: 90,
  categories: [
    { id: "lf_hrv", name: "Low Float - High Rel Vol", default_on: true, needs_history: false },
    { id: "mf_hrv_u20", name: "Medium Float - High Rel Vol - Price under $20", default_on: false, needs_history: false },
  ],
};
const row = (id: number, sym: string, cats: string[]): HodRowWire => ({ id, t: 1_792_075_200_000, sym, price: 3.21, vol: 2_500_000, float: 4e6, rvol: 6.5, rvol5: 2.5, gap: 0.12, chg: 0.42, chg5: 0.06, chg10: null, cats });

describe("HOD panel", () => {
  test("shows the rows of the default categories with every column", () => {
    const state: HodState = { ...EMPTY_HOD, enabled: true, connected: true, policy, rows: [row(2, "LOWF", ["lf_hrv"]), row(1, "MEDF", ["mf_hrv_u20"])] };
    const html = renderToStaticMarkup(<HodPanelView state={state} onPick={() => {}} />);
    expect(html).toContain("HOD Momentum");
    expect(html).toContain("LOWF");
    expect(html).not.toContain("MEDF");
    for (const s of ["3.21", "2.5M", "4.0M", "6.50x", "250%", "+12.0%", "+42.0%", "+6.0%", "LF · High RV"]) expect(html).toContain(s);
    expect(html).toContain("Short interest: no data source yet");
  });

  test("says when the day was not watched whole", () => {
    const state: HodState = { ...EMPTY_HOD, enabled: true, connected: true, complete: false, sinceMs: 1_792_075_200_000, policy, rows: [row(1, "LOWF", ["lf_hrv"])] };
    expect(renderToStaticMarkup(<HodPanelView state={state} onPick={() => {}} />)).toContain("Watching since");
  });
});

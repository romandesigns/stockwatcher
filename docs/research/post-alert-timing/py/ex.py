"""RESEARCH ONLY (deliverable F): failure-analysis examples, B0 vs D8 (and the frozen rule).

  python -I ex.py ROWS_DIR OUT_JSON [N]

Categories (P1, both fills on valid quotes, one example per day):
- helps: D8 - B0 largest (B0 stopped out, the wait avoided it);
- hurts: B0 - D8 largest;
- missed_breakout: B0 won within 8 min (target before D8 could enter) and D8 lost;
- before_reversal: D8 stopped out within 3 min of its entry.
"""
from __future__ import annotations

import datetime as dt
import json
import os
import sys
from zoneinfo import ZoneInfo

NY = ZoneInfo("America/New_York")


def et(ns):
    return dt.datetime.fromtimestamp(ns / 1e9, NY).strftime("%H:%M:%S")


def main():
    rows_dir, out = sys.argv[1], sys.argv[2]
    n = int(sys.argv[3]) if len(sys.argv) > 3 else 5
    fz = json.load(open(os.path.join(os.path.dirname(out), "frozen.json")))["winner"]
    al = {}
    for f in sorted(os.listdir(rows_dir)):
        for l in open(os.path.join(rows_dir, f)):
            r = json.loads(l)
            if "p1" in r["pops"] and r["strat"] in ("B0", "D8", fz):
                al.setdefault((r["day"], r["sym"], r["t0"]), {})[r["strat"]] = r
    ok = [v for v in al.values() if all(v[s].get("ekind") == "q" and v[s].get("xkind") == "q"
                                        for s in ("B0", "D8"))]

    def desc(v):
        b, d = v["B0"], v["D8"]
        x = v.get(fz, {})
        return {"day": b["day"], "sym": b["sym"], "path": b["path"], "segment": b["segment"],
                "alert_et": et(b["t0"]), "p0": b["p0"],
                "B0": {"entry": round(b["epx"], 4), "half_spread_pct": round(100 * b["ehs"], 2),
                       "outcome": b["reason"], "exit_et": et(b["te"] + b["hold_s"] * 10**9),
                       "net_pct": round(100 * b["net"], 2)},
                "D8": {"entry_et": et(d["te"]), "entry": round(d["epx"], 4),
                       "half_spread_pct": round(100 * d["ehs"], 2), "outcome": d["reason"],
                       "exit_et": et(d["te"] + d["hold_s"] * 10**9), "net_pct": round(100 * d["net"], 2)},
                fz: ({"entry_et": et(x["te"]), "entry": round(x["epx"], 4), "outcome": x["reason"],
                      "net_pct": round(100 * x["net"], 2)} if x.get("entered") else "no entry")}

    def pick(cands, key):
        cands = sorted(cands, key=key)
        seen, outl = set(), []
        for v in cands:
            if v["B0"]["day"] in seen:
                continue
            seen.add(v["B0"]["day"])
            outl.append(desc(v))
            if len(outl) == n:
                break
        return outl

    res = {
        "helps": pick(ok, lambda v: -(v["D8"]["net"] - v["B0"]["net"])),
        "hurts": pick(ok, lambda v: -(v["B0"]["net"] - v["D8"]["net"])),
        "missed_breakout": pick([v for v in ok if v["B0"]["reason"] == "win" and v["B0"]["hold_s"] < 480
                                 and v["D8"]["reason"] == "loss"], lambda v: v["D8"]["net"]),
        "before_reversal": pick([v for v in ok if v["D8"]["reason"] == "loss" and v["D8"]["hold_s"] <= 180],
                                lambda v: v["D8"]["net"]),
    }
    # how often each category occurs (all eligible P1, not only quote-complete)
    allv = list(al.values())
    res["frequency"] = {
        "alerts": len(allv),
        "D8_better_than_B0": sum(1 for v in allv if v["D8"].get("net", 0) > v["B0"].get("net", 0)),
        "B0_win_within_8min": sum(1 for v in allv if v["B0"].get("reason") == "win" and v["B0"]["hold_s"] < 480),
        "of_which_D8_loss": sum(1 for v in allv if v["B0"].get("reason") == "win" and v["B0"]["hold_s"] < 480 and v["D8"].get("reason") == "loss"),
        "D8_stopped_within_3min": sum(1 for v in allv if v["D8"].get("reason") == "loss" and v["D8"]["hold_s"] <= 180),
    }
    json.dump(res, open(out, "w"), indent=1)
    print(json.dumps(res["frequency"]))


if __name__ == "__main__":
    main()

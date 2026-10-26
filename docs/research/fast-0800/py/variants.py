"""research/fast-0800, part 3 (descriptive, post-hoc): what two Fast-move variants do to the fires
in a FINRA-TRF-opening window, against the unchanged production rule.

Input: work-f08/<var>/<variant>/<day>/fires.jsonl from the paa harness (each path's first fire
per symbol-day, with ft = +10 % before -5 % within 30 min of the fire, no costs), for the stocks
that had a Fast fire near the window under the production rule.
Usage: python variants.py var 08:00 120   |   python variants.py var04 04:00 180
"""

import collections
import glob
import json
import os
import random
import sys
from datetime import datetime
from zoneinfo import ZoneInfo

ET = ZoneInfo("America/New_York")
W = "C:/research/stockwatcher/work-f08"


def load(var, v):
    out = {}
    for f in glob.glob(f"{W}/{var}/{v}/*/fires.jsonl"):
        for line in open(f):
            r = json.loads(line)
            out.setdefault((r["day"], r["s"]), {})[r["path"]] = r
    return out


def in_win(t, hh, mm, secs):
    e = datetime.fromtimestamp(t / 1e9, ET)
    s = (e.hour * 60 + e.minute) * 60 + e.second - (hh * 60 + mm) * 60
    return 0 <= s < secs + 1  # the fire time is the end of the second


def boot(vals_by_day, rng, b=2000):
    days = list(vals_by_day)
    if not days:
        return (float("nan"),) * 3
    allv = [v for d in days for v in vals_by_day[d]]
    pt = sum(allv) / len(allv)
    bs = []
    for _ in range(b):
        s = [v for d in (rng.choice(days) for _ in days) for v in vals_by_day[d]]
        bs.append(sum(s) / len(s))
    bs.sort()
    return pt, bs[int(0.025 * b)], bs[int(0.975 * b)]


def main():
    var, hhmm, secs = sys.argv[1], sys.argv[2], int(sys.argv[3])
    hh, mm = map(int, hhmm.split(":"))
    days_done = {os.path.basename(os.path.dirname(p)) for p in glob.glob(f"{W}/{var}/blackout/*/done")}
    base, drop, black = load(var, "base"), load(var, "drop_d_t"), load(var, "blackout")
    rng = random.Random(4)
    report = {"window": f"{hhmm} +{secs}s", "days": len(days_done)}
    for split, lo, hi in (("before 2026-03-30", "0000", "2026-03-30"), ("from 2026-03-30", "2026-03-30", "9999")):
        keys = [k for k in base if lo <= k[0] < hi and k[0] in days_done and "fast" in base[k] and in_win(base[k]["fast"]["t"], hh, mm, secs)]
        if not keys:
            continue
        res = {"symbol_days": len(keys)}
        bwin = collections.defaultdict(list)
        for k in keys:
            bwin[k[0]].append(base[k]["fast"]["ft"] == "win")
        res["base_fast_win"] = boot(bwin, rng)
        for name, var_fires in (("drop_d_t", drop), ("blackout", black)):
            kept, moved, lost = 0, [], 0
            vwin, lost_wins, lost_wins_uncaught = collections.defaultdict(list), 0, 0
            for k in keys:
                f = var_fires.get(k, {}).get("fast")
                b0 = base[k]["fast"]
                if f is None:
                    lost += 1
                    if b0["ft"] == "win":
                        lost_wins += 1
                        # Caught anyway by another path within 30 min of the original fire?
                        others = [r for p, r in var_fires.get(k, {}).items() if p != "fast" and abs(r["t"] - b0["t"]) <= 30 * 60 * 10**9]
                        if not others:
                            lost_wins_uncaught += 1
                    continue
                vwin[k[0]].append(f["ft"] == "win")
                if f["t"] == b0["t"]:
                    kept += 1
                else:
                    moved.append((f["t"] - b0["t"]) / 60e9)
            moved.sort()
            res[name] = {
                "same_fire": kept,
                "later_fire": len(moved),
                "median_delay_min": moved[len(moved) // 2] if moved else None,
                "no_fast_fire": lost,
                "lost_fires_that_were_wins": lost_wins,
                "lost_wins_not_caught_by_another_path_within_30min": lost_wins_uncaught,
                "variant_fast_win": boot(vwin, rng),
                "variant_fast_fires": sum(len(v) for v in vwin.values()),
            }
        report[split] = res
    print(json.dumps(report, indent=1, default=lambda x: round(x, 4) if isinstance(x, float) else x))
    json.dump(report, open(f"C:/research/stockwatcher/reports/fast-0800/variants-{hhmm.replace(':', '')}.json", "w"), indent=1)


if __name__ == "__main__":
    main()

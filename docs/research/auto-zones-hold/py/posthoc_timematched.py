"""RESEARCH ONLY, POST-HOC (not in the protocol; never verdict-bearing): does the zone's LEVEL matter,
or only the moment it forms? Twin = same symbol-day, timeframe, kind, height and FORMATION CANDLE as
the auto zone, but its distance from price taken from another auto zone of the same set, timeframe
and kind (shuffled), so momentum and time of day are matched and only the level is decoupled.
usage: python posthoc_timematched.py <explore|confirm> <n_symbol_days>"""
from __future__ import annotations

import collections
import json
import multiprocessing as mp
import os
import random
import sys

import numpy as np

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import azones as az  # noqa: E402
import candles as cnd  # noqa: E402

RS = "C:/research/stockwatcher"
ROOT = {"explore": f"{RS}/work-pat/series", "confirm": f"{RS}/work-pat/series-confirm"}


def zones_of(job):
    root, day, sym = job
    out = []
    for tf_name, tf in cnd.TFS.items():
        cs = cnd.candles(root, day, sym, tf)
        if not cs or len(cs) < az.R["ATR_N"] + 3:
            continue
        for z in az.formed_zones(cs):
            pf = cs[z["formed"]][4]
            if pf <= 0:
                continue
            d = (pf - z["hi"]) / pf if z["kind"] == "demand" else (z["lo"] - pf) / pf
            o = az.outcome(cs, z["kind"], z["lo"], z["hi"], z["formed"] + 1, cnd.K[tf_name], z["atr"])
            out.append(dict(day=day, sym=sym, tf=tf_name, kind=z["kind"], h=(z["hi"] - z["lo"]) / pf, d=d, f=z["formed"], atr=z["atr"],
                            tested=o["tested"], hold=o.get("res") == "hold"))
    return out


def twins(args):
    (root, day, sym), items = args
    res = []
    by_tf = collections.defaultdict(list)
    for it in items:
        by_tf[it["tf"]].append(it)
    for tf_name, its in by_tf.items():
        cs = cnd.candles(root, day, sym, cnd.TFS[tf_name])
        for it in its:
            p = cs[it["f"]][4]
            if it["kind"] == "demand":
                hi = p * (1 - it["d2"]); lo = hi - p * it["h"]
            else:
                lo = p * (1 + it["d2"]); hi = lo + p * it["h"]
            o = az.outcome(cs, it["kind"], lo, hi, it["f"] + 1, cnd.K[tf_name], it["atr"])
            res.append(dict(it, t_tested=o["tested"], t_hold=o.get("res") == "hold"))
    return res


def main():
    set_name, n = sys.argv[1], int(sys.argv[2])
    trig = [json.loads(l) for l in open(f"{RS}/work-pat/triggers-{set_name}.jsonl")]
    jobs = [(ROOT[set_name], r["day"], r["sym"]) for r in trig if r["is_first"]]
    jobs = random.Random(5).sample(jobs, min(n, len(jobs)))
    with mp.Pool(max(2, os.cpu_count() - 4)) as pool:
        zs = [z for rows in pool.imap_unordered(zones_of, jobs, chunksize=8) for z in rows]
    rng = random.Random(6)
    pools = collections.defaultdict(list)
    for z in zs:
        pools[(z["tf"], z["kind"])].append(z["d"])
    for z in zs:
        z["d2"] = rng.choice(pools[(z["tf"], z["kind"])])
    grouped = collections.defaultdict(list)
    for z in zs:
        grouped[(ROOT[set_name], z["day"], z["sym"])].append(z)
    with mp.Pool(max(2, os.cpu_count() - 4)) as pool:
        out = [r for rows in pool.imap_unordered(twins, list(grouped.items()), chunksize=8) for r in rows]
    for tf in cnd.TFS:
        rs = [r for r in out if r["tf"] == tf]
        days = sorted({r["day"] for r in rs})
        di = {d: i for i, d in enumerate(days)}
        M = np.zeros((len(days), 4))
        for r in rs:
            i = di[r["day"]]
            if r["tested"]:
                M[i, 0] += r["hold"]; M[i, 1] += 1
            if r["t_tested"]:
                M[i, 2] += r["t_hold"]; M[i, 3] += 1
        tot = M.sum(0)
        g = np.random.default_rng(1)
        S = M[g.integers(0, len(days), (2000, len(days)))].sum(1)
        d = S[:, 0] / S[:, 1] - S[:, 2] / S[:, 3]
        print(f"{set_name} {tf}: auto hold {tot[0]/tot[1]:.1%} (n {int(tot[1])})  time-matched twin hold {tot[2]/tot[3]:.1%} (n {int(tot[3])})  "
              f"diff {100*(tot[0]/tot[1]-tot[2]/tot[3]):+.1f} pp [{100*np.percentile(d,2.5):+.1f}, {100*np.percentile(d,97.5):+.1f}]")


if __name__ == "__main__":
    main()

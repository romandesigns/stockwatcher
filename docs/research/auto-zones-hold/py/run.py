"""RESEARCH ONLY: protocol §2-§4 over every symbol-day of a set and universe; one JSONL row per zone.
usage: python run.py <explore|confirm> <u1|u2> <out.jsonl>"""
from __future__ import annotations

import glob
import json
import multiprocessing as mp
import os
import random
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import azones as az  # noqa: E402
import candles as cnd  # noqa: E402

RS = "C:/research/stockwatcher"
SERIES = {("explore", "u1"): f"{RS}/work-pat/series", ("confirm", "u1"): f"{RS}/work-pat/series-confirm"}
U2 = f"{RS}/work-azh/series-u2"


def jobs(set_name, univ):
    if univ == "u1":
        out = []
        for line in open(f"{RS}/work-pat/triggers-{set_name}.jsonl"):
            r = json.loads(line)
            if r["is_first"]:
                out.append((SERIES[(set_name, "u1")], r["day"], r["sym"]))
        return out
    out = []
    for p in glob.glob(f"{U2}/*/*.bin"):
        day = os.path.basename(os.path.dirname(p))
        if (set_name == "confirm") == (day < "2026-03-23"):
            out.append((U2, day, os.path.basename(p)[:-4]))
    return out


def one(job):
    root, day, sym = job
    rows = []
    for tf_name, tf in cnd.TFS.items():
        cs = cnd.candles(root, day, sym, tf)
        if not cs or len(cs) < az.R["ATR_N"] + 3:
            continue
        tr = az._tr(cs)
        rng = random.Random(f"20261026:{day}:{sym}:{tf_name}")
        zones = az.formed_zones(cs)
        for z in zones:
            # Unbroken same-kind zones at formation (how many the layer would show at once, before its 3-per-side cap).
            for src, zz in (("auto", z), ("random", az.random_twin(cs, tr, z, rng))):
                if zz is None:
                    continue
                o = az.outcome(cs, zz["kind"], zz["lo"], zz["hi"], zz["formed"] + 1, cnd.K[tf_name], zz["atr"])
                pf = cs[zz["formed"]][4]
                rows.append(dict(day=day, sym=sym, tf=tf_name, src=src, kind=zz["kind"], formed_t=cs[zz["formed"]][0],
                                 height_pct=(zz["hi"] - zz["lo"]) / pf * 100 if pf else None,
                                 tested=o["tested"], res=o.get("res"), wait=(o["test_k"] - zz["formed"]) if o["tested"] else None,
                                 wait_min=((cs[o["test_k"]][0] - cs[zz["formed"]][0]) / 60000) if o["tested"] else None,
                                 bounce_atr=o.get("bounce_atr"), n_zones_day=len(zones)))
    return rows


def main():
    set_name, univ, out = sys.argv[1:4]
    js = jobs(set_name, univ)
    n = 0
    with mp.Pool(max(2, os.cpu_count() - 4)) as pool, open(out, "w") as f:
        for rows in pool.imap_unordered(one, js, chunksize=8):
            for r in rows:
                f.write(json.dumps(r) + "\n")
            n += 1
            if n % 1000 == 0:
                print(set_name, univ, n, "/", len(js), flush=True)
    print("done", set_name, univ, len(js))


if __name__ == "__main__":
    main()

"""RESEARCH ONLY: autoZones parity, Python port vs the shipped TypeScript (protocol §2).
usage: python test_parity.py   (needs bun on PATH; writes fixtures under work-azh/parity)"""
from __future__ import annotations

import json
import os
import random
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import azones  # noqa: E402
import candles as cnd  # noqa: E402

W = "C:/research/stockwatcher/work-azh/parity"
os.makedirs(W, exist_ok=True)


def synthetic(rng: random.Random, n: int):
    """A random walk with quiet bases and sharp legs injected, so the rule fires often."""
    out, p, t = [], 5.0, 1_700_000_000_000
    i = 0
    while i < n:
        if rng.random() < 0.08 and i > 20:
            for _ in range(rng.randint(1, 5)):  # base
                o = p
                c = p + rng.uniform(-0.01, 0.01) * p * 0.2
                h = max(o, c) + abs(rng.gauss(0, 0.002)) * p
                l = min(o, c) - abs(rng.gauss(0, 0.002)) * p
                out.append((t, o, h, l, c)); t += 60_000; p = c; i += 1
            sgn = rng.choice([1, -1])
            for _ in range(rng.randint(1, 3)):  # leg
                o = p
                c = p * (1 + sgn * rng.uniform(0.01, 0.05))
                h = max(o, c) * (1 + abs(rng.gauss(0, 0.002)))
                l = min(o, c) * (1 - abs(rng.gauss(0, 0.002)))
                out.append((t, o, h, l, c)); t += 60_000; p = c; i += 1
            continue
        o = p
        c = p * (1 + rng.gauss(0, 0.006))
        h = max(o, c) * (1 + abs(rng.gauss(0, 0.003)))
        l = min(o, c) * (1 - abs(rng.gauss(0, 0.003)))
        out.append((t, round(o, 4), round(h, 4), round(l, 4), round(c, 4))); t += 60_000; p = c; i += 1
    return out


def main():
    rng = random.Random(7)
    cases = [synthetic(rng, rng.randint(10, 400)) for _ in range(300)]
    # Real candle days, every timeframe (EXPLORE and CONFIRM series).
    trig = [json.loads(l) for l in open("C:/research/stockwatcher/work-pat/triggers-explore.jsonl") if '"is_first": true' in l]
    for r in random.Random(11).sample(trig, 15):
        for tf in cnd.TFS.values():
            cs = cnd.candles("C:/research/stockwatcher/work-pat/series", r["day"], r["sym"], tf)
            if cs:
                cases.append(cs)
    trig = [json.loads(l) for l in open("C:/research/stockwatcher/work-pat/triggers-confirm.jsonl") if '"is_first": true' in l]
    for r in random.Random(12).sample(trig, 10):
        for tf in cnd.TFS.values():
            cs = cnd.candles("C:/research/stockwatcher/work-pat/series-confirm", r["day"], r["sym"], tf)
            if cs:
                cases.append(cs)
    json.dump(cases, open(f"{W}/fixtures.json", "w"))
    subprocess.run(["bun", os.path.join(HERE, "parity.ts"), f"{W}/fixtures.json", f"{W}/ts.json"], check=True)
    ts = json.load(open(f"{W}/ts.json"))
    bad = 0
    nz = 0
    for i, cs in enumerate(cases):
        py = azones.auto_zones(cs)
        a = [(z["kind"], z["lo"], z["hi"], z["t"], z["tested"]) for z in py]
        b = [(z["kind"], z["lo"], z["hi"], z["t"], z["tested"]) for z in ts[i]]
        nz += len(b)
        if a != b:
            bad += 1
            if bad <= 3:
                print("MISMATCH case", i, "\n py", a, "\n ts", b)
    print(f"cases {len(cases)}  zones {nz}  mismatches {bad}")
    sys.exit(1 if bad else 0)


if __name__ == "__main__":
    main()

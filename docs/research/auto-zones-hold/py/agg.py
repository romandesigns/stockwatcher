"""RESEARCH ONLY: protocol §3-§5 tables, bootstrap and verdicts from run.py rows.
usage: python agg.py <out_dir>"""
from __future__ import annotations

import collections
import json
import os
import statistics as st
import sys

import numpy as np

W = "C:/research/stockwatcher/work-azh"
TFS = ["15s", "30s", "1m", "5m"]
B = 5000


def load(set_name, univ):
    p = f"{W}/rows-{set_name}-{univ}.jsonl"
    return [json.loads(l) for l in open(p)] if os.path.exists(p) else []


def describe(rows):
    n = len(rows)
    t = [r for r in rows if r["tested"]]
    res = collections.Counter(r["res"] for r in t)
    waits = [r["wait_min"] for r in t if r["wait_min"] is not None]
    bounce = [r["bounce_atr"] for r in t if r["bounce_atr"] is not None]
    nt = len(t)
    return dict(zones=n, tested=nt, tested_share=nt / n if n else None,
                hold=res["hold"] / nt if nt else None, brk=res["break"] / nt if nt else None, unresolved=res["unresolved"] / nt if nt else None,
                wait_min_median=st.median(waits) if waits else None, bounce_atr_median=st.median(bounce) if bounce else None)


def delta(rows, seed=20261026):
    """Day-clustered bootstrap of hold rate (auto) - hold rate (random), rates as ratios of sums."""
    by = collections.defaultdict(lambda: np.zeros(4))  # auto holds, auto tested, random holds, random tested
    for r in rows:
        if not r["tested"]:
            continue
        k = 0 if r["src"] == "auto" else 2
        by[r["day"]][k] += r["res"] == "hold"
        by[r["day"]][k + 1] += 1
    days = list(by)
    if not days:
        return None
    M = np.array([by[d] for d in days])
    tot = M.sum(0)
    point = tot[0] / tot[1] - tot[2] / tot[3]
    rng = np.random.default_rng(seed)
    idx = rng.integers(0, len(days), size=(B, len(days)))
    S = M[idx].sum(1)
    d = S[:, 0] / S[:, 1] - S[:, 2] / S[:, 3]
    lo, hi = np.percentile(d, [2.5, 97.5])
    return dict(point=float(point), lo=float(lo), hi=float(hi), days=len(days), auto_tested=int(tot[1]), random_tested=int(tot[3]))


def verdict(d):
    if d is None:
        return "NO DATA"
    if d["lo"] > 0 and d["point"] >= 0.05 and d["auto_tested"] >= 300:
        return "SUPPORTED"
    if d["point"] >= 0.05:
        return "PROMISING BUT INCONCLUSIVE"
    return "NO EDGE OVER RANDOM"


def one_in(p):
    return f"1 in {max(1, round(1 / p))}" if p and p > 0 else "never"


def main():
    out = sys.argv[1]
    os.makedirs(out, exist_ok=True)
    res = {}
    lines = []
    for set_name in ("explore", "confirm"):
        for univ in ("u1", "u2"):
            rows = load(set_name, univ)
            days = {(r["day"], r["sym"]) for r in rows}
            for tf in TFS:
                rs = [r for r in rows if r["tf"] == tf]
                a = describe([r for r in rs if r["src"] == "auto"])
                b = describe([r for r in rs if r["src"] == "random"])
                d = delta(rs)
                per_day = a["zones"] / len(days) if days else None
                # The layer's own count per symbol-day: rows are per zone, n_zones_day repeats per row.
                res[f"{set_name}/{univ}/{tf}"] = dict(auto=a, random=b, delta=d, verdict=verdict(d), zones_per_symbol_day=per_day)
                lines.append(f"| {set_name} | {univ} | {tf} | {a['zones']} | {per_day:.1f} | {a['tested_share']:.0%} | {a['hold']:.1%} | {b['hold']:.1%} | "
                             f"{d['point']*100:+.1f} [{d['lo']*100:+.1f}, {d['hi']*100:+.1f}] | {a['brk']:.1%} | {a['unresolved']:.1%} | "
                             f"{a['wait_min_median']:.1f} | {a['bounce_atr_median']:.2f} | {verdict(d)} |" if a["zones"] and d else f"| {set_name} | {univ} | {tf} | no data |")
    menu = {}
    for tf in TFS:
        r = res.get(f"confirm/u1/{tf}")
        if r and r["auto"]["hold"] is not None:
            menu[tf] = f"Held about {one_in(r['auto']['hold'])} times (random zones: {one_in(r['random']['hold'])})"
    res["menu_text_confirm_u1"] = menu
    json.dump(res, open(f"{out}/results.json", "w", encoding="utf-8"), indent=1)
    md = ["| set | universe | tf | zones | zones per symbol-day | tested | hold (auto) | hold (random) | Δ pp [95% CI] | break | unresolved | median wait min | median bounce ATR | verdict |",
          "|---|---|---|---|---|---|---|---|---|---|---|---|---|---|"] + lines
    md += ["", "Menu text candidates (CONFIRM, U1):"] + [f"- {tf}: {t}" for tf, t in menu.items()]
    open(f"{out}/tables.md", "w", encoding="utf-8").write("\n".join(md) + "\n")
    print("\n".join(md))


if __name__ == "__main__":
    main()

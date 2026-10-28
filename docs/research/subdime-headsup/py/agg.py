"""RESEARCH ONLY: sub-dime heads-up analysis (docs/research/subdime-headsup/protocol.md).

usage:
  python -I agg.py windows DEV|TEST   -> work-sd/windows-<split>.csv (quote windows for fetch.py)
  python -I agg.py freeze             -> frozen.json (range15 quintile edges from DEV heads-ups)
  python -I agg.py score DEV|TEST     -> reports/subdime-headsup/<split>.json + .md

Reads work-sd/out/{prod,sub}/<root>/<day>/{fires,random}.jsonl and quotes-sd/<day>/<SYM>.npz.
"""

import glob
import json
import os
import random
import statistics as st
import sys
from collections import defaultdict

import numpy as np

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import aq  # noqa: E402

R = "C:/research/stockwatcher"
W = f"{R}/work-sd"
Q = f"{R}/quotes-sd"
REP = f"{R}/reports/subdime-headsup"
HERE = os.path.dirname(os.path.abspath(__file__))
FROZEN = os.path.join(HERE, "..", "frozen.json")
SEC = 10**9
MIN = 60 * SEC


def split_of(day):
    if "2026-03-30" <= day <= "2026-07-31":
        return "DEV"
    if "2026-08-03" <= day <= "2026-10-23":
        return "TEST"
    return None


def rows(variant, kind, split):
    out = []
    for f in glob.glob(f"{W}/out/{variant}/*/*/{kind}.jsonl"):
        day = os.path.basename(os.path.dirname(f))
        if split_of(day) != split:
            continue
        with open(f) as fh:
            out += [json.loads(l) for l in fh if l.strip()]
    return out


def first_per_symday(rs, lo, hi):
    best = {}
    for r in rs:
        if not (lo <= r["p"] < hi):
            continue
        k = (r["day"], r["s"])
        if k not in best or r["t"] < best[k]["t"]:
            best[k] = r
    return list(best.values())


def units(split):
    sub = rows("sub", "fires", split)
    prod = rows("prod", "fires", split)
    hu = first_per_symday(sub, 0.05, 0.10)
    ref = first_per_symday(prod, 0.10, 0.5000001)
    prod_by = defaultdict(list)
    for r in prod:
        prod_by[(r["day"], r["s"])].append(r)
    for h in hu:
        later = [r for r in prod_by[(h["day"], h["s"])] if h["t"] <= r["t"] <= h["t"] + 60 * MIN]
        h["prod_t"] = min((r["t"] for r in later), default=None)
        h["lead_min"] = (h["prod_t"] - h["t"]) / MIN if h["prod_t"] else None
    return hu, ref, prod, sub


def matched(hu, ref):
    """Protocol 1: per day, the first n in-band units by time, n = heads-ups that day."""
    n = defaultdict(int)
    for h in hu:
        n[h["day"]] += 1
    by = defaultdict(list)
    for r in ref:
        by[r["day"]].append(r)
    out = []
    for d, k in n.items():
        out += sorted(by[d], key=lambda r: r["t"])[:k]
    return out


def windows(split):
    hu, ref, _, _ = units(split)
    with open(f"{W}/windows-{split}.csv", "w") as f:
        f.write("day,sym,lo,hi,split\n")
        for u in hu + matched(hu, ref):
            f.write(f'{u["day"]},{u["s"]},{u["t"] - 2 * MIN},{u["t"] + 31 * MIN},{split}\n')
    print(split, "heads-ups", len(hu), "matched refs", len(matched(hu, ref)))


_qcache = {}


def quotes(day, sym):
    k = (day, sym)
    if k not in _qcache:
        p = f"{Q}/{day}/{sym}.npz"
        _qcache[k] = aq.load(p) if os.path.exists(p) else None
    return _qcache[k]


def half_spread(day, sym, t):
    """Protocol "Outcomes": the quote prevailing at t (within 60 s), valid when bid > 0 and ask > bid."""
    q = quotes(day, sym)
    if q is None or len(q["t"]) == 0:
        return None
    i = int(np.searchsorted(q["t"], t, side="right")) - 1
    if i < 0 or t - q["t"][i] > 60 * SEC:
        return None
    bp, ap = int(q["bp"][i]), int(q["ap"][i])
    if bp <= 0 or ap <= bp:
        return None
    return (ap - bp) / 2 / ((ap + bp) / 2)


def enrich(us):
    for u in us:
        a = half_spread(u["day"], u["s"], u["t"])
        b = half_spread(u["day"], u["s"], u["t_exit"])
        u["hs_in"], u["hs_out"] = a, b
        u["qc"] = a is not None and b is not None
        u["net"] = u["ret"] - a - b if u["qc"] else None
        u["win"] = u["ft"] == "win"
    return us


def boot(groups, stat, n=5000, seed=7):
    """Day-clustered bootstrap of stat(list_of_rows) -> (point, lo, hi)."""
    days = sorted(groups)
    rng = random.Random(seed)
    pt = stat([r for d in days for r in groups[d]])
    bs = []
    for _ in range(n):
        pick = [rng.choice(days) for _ in days]
        v = stat([r for d in pick for r in groups[d]])
        if v is not None:
            bs.append(v)
    bs.sort()
    if not bs or pt is None:
        return (pt, None, None)
    return (pt, bs[int(0.025 * len(bs))], bs[int(0.975 * len(bs)) - 1])


def mean(xs):
    xs = [x for x in xs if x is not None]
    return sum(xs) / len(xs) if xs else None


def freeze():
    hu, _, _, _ = units("DEV")
    rr = sorted(h["range15"] for h in hu if h.get("range15") is not None)
    edges = [rr[int(len(rr) * q / 5)] for q in (1, 2, 3, 4)] if len(rr) >= 5 else []
    import subprocess

    commit = subprocess.run(["git", "rev-parse", "HEAD"], capture_output=True, text=True, cwd=HERE).stdout.strip()
    json.dump({"range15_quintile_edges": edges, "dev_headups": len(hu), "commit": commit}, open(FROZEN, "w"), indent=1)
    print("frozen", edges, len(hu))


def bucket(x, edges):
    if x is None:
        return None
    return sum(1 for e in edges if x >= e)


def score(split):
    fz = json.load(open(FROZEN))
    edges = fz["range15_quintile_edges"]
    hu, ref, prod, sub = units(split)
    ref_m = matched(hu, ref)
    enrich(hu)
    enrich(ref_m)
    rnd = rows("sub", "random", split)
    for r in rnd:
        r["win"] = r["ft"] == "win"
    days = sorted({h["day"] for h in hu} | {r["day"] for r in ref_m})
    ndays = len({d for d in os.listdir(f"{W}/out/sub/archive-sd") if split_of(d) == split} |
                {d for root in ("archive-hist", "archive-vps") for d in (os.listdir(f"{W}/out/sub/{root}") if os.path.isdir(f"{W}/out/sub/{root}") else []) if split_of(d) == split})
    rep = {"split": split, "days": ndays, "headups": len(hu), "headups_per_day": len(hu) / max(1, ndays),
           "matched_refs": len(ref_m), "frozen": fz}

    def g(us, cond=lambda u: True):
        d = defaultdict(list)
        for u in us:
            if cond(u):
                d[u["day"]].append(u)
        return d

    qc = lambda u: u["qc"]  # noqa: E731
    rate = lambda rs: (sum(r["win"] for r in rs) / len(rs)) if rs else None  # noqa: E731
    netm = lambda rs: mean([r["net"] for r in rs])  # noqa: E731
    rep["quote_complete_share_headups"] = sum(qc(u) for u in hu) / max(1, len(hu))
    rep["quote_complete_share_refs"] = sum(qc(u) for u in ref_m) / max(1, len(ref_m))
    rep["median_hs_in_headups"] = st.median([u["hs_in"] for u in hu if u["hs_in"] is not None]) if any(u["hs_in"] is not None for u in hu) else None
    rep["median_hs_in_refs"] = st.median([u["hs_in"] for u in ref_m if u["hs_in"] is not None]) if any(u["hs_in"] is not None for u in ref_m) else None
    for name, us in (("headups", hu), ("refs", ref_m)):
        rep[name + "_gross_win"] = boot(g(us), rate)
        rep[name + "_qc_n"] = sum(qc(u) for u in us)
        rep[name + "_qc_gross_win"] = boot(g(us, qc), rate)
        rep[name + "_qc_net_mean"] = boot(g(us, qc), netm)
        rep[name + "_qc_net_pos_share"] = boot(g(us, qc), lambda rs: (sum(r["net"] > 0 for r in rs) / len(rs)) if rs else None)

    # Primary: net-of-spread "win" = bracket win AND net > 0 after both half-spreads, quote-complete.
    netwin = lambda rs: (sum(r["win"] and r["net"] > 0 for r in rs) / len(rs)) if rs else None  # noqa: E731
    gh, gr = g(hu, qc), g(ref_m, qc)
    alld = sorted(set(gh) | set(gr))
    rng = random.Random(11)

    def diff(pick):
        a = [r for d in pick for r in gh.get(d, [])]
        b = [r for d in pick for r in gr.get(d, [])]
        x, y = netwin(a), netwin(b)
        return None if x is None or y is None else x - y

    pt = diff(alld)
    bs = sorted(v for v in (diff([rng.choice(alld) for _ in alld]) for _ in range(5000)) if v is not None)
    rep["primary_netwin_headups"] = netwin([r for d in alld for r in gh.get(d, [])])
    rep["primary_netwin_refs"] = netwin([r for d in alld for r in gr.get(d, [])])
    rep["primary_diff"] = (pt, bs[int(0.025 * len(bs))] if bs else None, bs[int(0.975 * len(bs)) - 1] if bs else None)

    # Random baseline, matched by range15 quintile (gross).
    rb = defaultdict(list)
    for r in rnd:
        b = bucket(r.get("range15"), edges)
        if b is not None:
            rb[b].append(r)
    hb = [h for h in hu if bucket(h.get("range15"), edges) is not None]

    def rand_diff(pick):
        hs = [h for d in pick for h in hb if h["day"] == d]
        if not hs:
            return None
        exp = []
        for h in hs:
            pool = rb.get(bucket(h["range15"], edges))
            if pool:
                exp.append(sum(r["win"] for r in pool) / len(pool))
        if not exp:
            return None
        return rate(hs) - sum(exp) / len(exp)

    hd = sorted({h["day"] for h in hb})
    pt = rand_diff(hd)
    bs = sorted(v for v in (rand_diff([rng.choice(hd) for _ in hd]) for _ in range(2000)) if v is not None)
    rep["random_n"] = len(rnd)
    rep["headups_minus_random_gross"] = (pt, bs[int(0.025 * len(bs))] if bs else None, bs[int(0.975 * len(bs)) - 1] if bs else None)

    # Lead.
    reach = [h for h in hu if h.get("reach10") is not None]
    leads = [h["lead_min"] for h in reach if h["lead_min"] is not None]
    rep["reach10_share"] = len(reach) / max(1, len(hu))
    rep["prod_alert_within_60_share_of_reachers"] = len(leads) / max(1, len(reach))
    rep["median_lead_min_reachers_with_prod_alert"] = st.median(leads) if leads else None
    rep["lead_quartiles"] = [float(np.percentile(leads, q)) for q in (25, 50, 75)] if leads else None
    # By path (heads-ups); Burst separately (trained on $0.15+).
    bypath = defaultdict(list)
    for h in hu:
        bypath[h["path"]].append(h)
    rep["by_path"] = {p: {"n": len(v), "gross_win": rate(v), "qc_n": sum(qc(u) for u in v),
                          "qc_net_mean": mean([u["net"] for u in v if u["qc"]])} for p, v in sorted(bypath.items())}
    # Verdict (protocol).
    lo = rep["primary_diff"][1]
    rlo = rep["headups_minus_random_gross"][1]
    lead = rep["median_lead_min_reachers_with_prod_alert"]
    nqc = rep["headups_qc_n"]
    if rep["quote_complete_share_headups"] < 0.70:
        verdict = "INCONCLUSIVE DUE TO DATA"
    elif lo is not None and lo > -0.03 and rlo is not None and rlo > 0 and lead is not None and lead >= 5 and nqc >= 100:
        verdict = "SUPPORTED"
    elif (rep["primary_diff"][0] or -1) > -0.03 and (rep["headups_minus_random_gross"][0] or -1) > 0 and (lead or 0) >= 5:
        verdict = "PROMISING BUT INCONCLUSIVE"
    else:
        verdict = "UNSUPPORTED"
    rep["verdict"] = verdict
    os.makedirs(REP, exist_ok=True)
    json.dump(rep, open(f"{REP}/{split}.json", "w"), indent=1, default=str)
    json.dump(rep, open(os.path.join(HERE, "..", f"results-{split}.json"), "w"), indent=1, default=str)
    print(json.dumps(rep, indent=1, default=str))


if __name__ == "__main__":
    cmd = sys.argv[1]
    if cmd == "windows":
        windows(sys.argv[2])
    elif cmd == "freeze":
        freeze()
    elif cmd == "score":
        score(sys.argv[2])

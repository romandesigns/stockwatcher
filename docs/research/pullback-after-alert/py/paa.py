"""RESEARCH ONLY (docs/research/pullback-after-alert/protocol.md): reader for the export.

  python -I paa.py select  DEV_ROOT OUT_JSON            # §5 selection on DEV (1 % scenario)
  python -I paa.py windows ROOT FROZEN_JSON OUT_CSV     # §7 quote windows (entry times only)
  python -I paa.py score   ROOT FROZEN_JSON QUOTES OUT  # §6 tables + bootstrap (ROOT = dev|test)

ROOT holds one directory per day with triggers.jsonl / entries.jsonl (the Rust export).
All days studied are EDT (2026-03-23..10-23), so ET = UTC - 4 h.
"""

from __future__ import annotations

import collections
import json
import math
import os
import sys

import numpy as np

SEC = 10**9
CAPS = {"running": 150, "fast": 200}
PATH_ORDER = ["burst", "run", "fast", "running"]
SCEN = {"0.5": 0.0025, "1": 0.005, "2": 0.01}  # round trip % -> half per side
BASE = ["B0", "B1", "B2", "B3"]


def et_minutes(ns: int) -> float:
    s = ns // SEC - 4 * 3600
    return (s % 86400) / 60.0


def session(ns: int) -> str:
    return "pre" if et_minutes(ns) < 9 * 60 + 30 else "rth"


def band(p: float) -> str:
    return "0.10-1" if p < 1 else ("1-5" if p < 5 else "5-25")


def days_of(root: str) -> list[str]:
    return sorted(d for d in os.listdir(root) if os.path.exists(os.path.join(root, d, "done")))


def load_day(root: str, day: str):
    """The day's triggers (first per symbol after caps) and their entry rows, keyed by rule."""
    trig = [json.loads(l) for l in open(os.path.join(root, day, "triggers.jsonl"))]
    trig.sort(key=lambda r: (r["t"], r["s"]))
    used = collections.Counter()
    capped = set()
    for r in trig:
        c = CAPS.get(r["path"])
        if c is None:
            continue
        used[r["path"]] += 1
        if used[r["path"]] > c:
            capped.add((r["s"], r["path"]))
    ent = collections.defaultdict(dict)  # (s, path, t0) -> rule -> row
    for l in open(os.path.join(root, day, "entries.jsonl")):
        e = json.loads(l)
        ent[(e["s"], e["path"], e["t0"])][e["rule"]] = e
    keys_by_sym = collections.defaultdict(list)
    for k in ent:
        if (k[0], k[1]) in capped:
            continue
        keys_by_sym[k[0]].append(k)
    out = []
    for s, ks in keys_by_sym.items():
        ks.sort(key=lambda k: (k[2], PATH_ORDER.index(k[1])))
        k = ks[0]
        rules = ent[k]
        if "B0" not in rules:
            continue
        out.append({"day": day, "s": s, "path": k[1], "t0": k[2], "p0": rules["B0"]["p0"],
                    "rules": rules})
    return out


def bracket(e: dict, h_in: float, h_out_fn) -> tuple[float, str, int]:
    """Net return of the +10 %/-5 % bracket from the cost-adjusted entry (protocol §6)."""
    pe = e["pe"]
    en = pe * (1 + h_in)
    up, dn = en * 1.10, en * 0.95
    for ms, p in e["ext"]:
        if p >= up:
            return 0.10 - h_out_fn(ms), "win", ms
        if p <= dn:
            return -0.05 - h_out_fn(ms), "loss", ms
    if e["last"] is not None:
        ms, p = e["last"]
        return p / en - 1 - h_out_fn(ms), "timeout", ms
    return pe / en - 1 - h_out_fn(0), "noprint", 0


def flat(e, h):
    return bracket(e, h, lambda _ms: h)


def mfe_mae(e):
    ps = [p for _, p in e["ext"]] + [e["pe"]]
    return max(ps) / e["pe"] - 1, min(ps) / e["pe"] - 1


def rmult(e):
    """Stop = recorded stop, target = 2R; timeouts marked to the last print (raw prices)."""
    st = e.get("stop")
    pe = e["pe"]
    if st is None or not (st < pe):
        return None
    r = pe - st
    for _ms, p in e["ext"]:
        if p >= pe + 2 * r:
            return 2.0
        if p <= st:
            return -1.0
    if e["last"] is not None:
        return (e["last"][1] - pe) / r
    return 0.0


def b1_rule(rules: dict) -> str | None:
    for k in rules:
        if k.startswith("B1_"):
            return k
    return None


def candidates(trigs) -> list[str]:
    s = set()
    for t in trigs:
        s.update(k for k in t["rules"] if k.startswith("C"))
    return sorted(s)


def n_params(rule: str) -> int:
    return 0 if rule == "C1" else (3 if rule.startswith("C2") else 4)


def depth_rank(rule: str) -> float:
    if rule == "C1":
        return 0
    lo, hi = rule.split("_d")[1].split("_")[0].split("-")
    return float(hi) - float(lo)


# ----------------------------------------------------------------------------- select
def cmd_select(root: str, out: str):
    days = days_of(root)
    blocks = np.array_split(np.array(days), 6)
    val_days = [d for b in blocks[1:] for d in b]
    val = set(val_days)
    by_day = {d: load_day(root, d) for d in days}
    h = SCEN["1"]
    rules = candidates([t for d in days for t in by_day[d]])
    table = []
    for r in rules:
        deltas, n = [], 0
        for d in val_days:
            for t in by_day[d]:
                c = t["rules"].get(r)
                if c is None:
                    continue
                n += 1
                deltas.append(flat(c, h)[0] - flat(t["rules"]["B0"], h)[0])
        per_day = n / len(val_days)
        table.append({"rule": r, "n_val": n, "per_day": round(per_day, 2),
                      "delta_mean": float(np.mean(deltas)) if deltas else None,
                      "eligible": per_day >= 8})
    elig = [x for x in table if x["eligible"] and x["delta_mean"] is not None]
    elig.sort(key=lambda x: (-x["delta_mean"], n_params(x["rule"]), depth_rank(x["rule"])))
    win = elig[0]["rule"]
    delays = [(t["rules"][win]["te"] - t["t0"]) / SEC for d in days for t in by_day[d]
              if win in t["rules"]]
    d_b1 = int(round(float(np.median(delays))))
    res = {"winner": win, "b1_delay_s": d_b1, "selection": table,
           "blocks": [[str(b[0]), str(b[-1]), len(b)] for b in blocks],
           "validation_days": len(val_days), "dev_days": len(days),
           "criterion": "pooled paired mean net delta vs B0, 1% RT scenario, val blocks 2-6, "
                        ">= 8 entries/val day"}
    json.dump(res, open(out, "w"), indent=1)
    table.sort(key=lambda x: -(x["delta_mean"] or -9))
    for x in table[:15]:
        print(x)
    print("WINNER", win, "B1 delay", d_b1)


# ----------------------------------------------------------------------------- windows
def cmd_windows(root: str, frozen: str, out: str, split: str):
    fz = json.load(open(frozen))
    win = fz["winner"]
    rows = []
    for d in days_of(root):
        for t in load_day(root, d):
            for k, e in t["rules"].items():
                if k == win or k in ("B0", "B2", "B3") or k.startswith("B1_"):
                    rows.append((d, t["s"], e["te"]))
    with open(out, "a") as f:
        for d, s, te in rows:
            f.write(f"{d},{s},{te - 2 * 60 * SEC},{te + 31 * 60 * SEC},{split}\n")
    print(len(rows), "windows")


# ----------------------------------------------------------------------------- score
class Quotes:
    """Half-spread at time t from the stored NBBO (protocol §7)."""

    def __init__(self, qroot: str):
        self.qroot = qroot
        self.cache = {}

    def get(self, day, sym):
        k = (day, sym)
        if k not in self.cache:
            p = os.path.join(self.qroot, day, f"{sym}.npz")
            if not os.path.exists(p):
                self.cache[k] = None
            else:
                with np.load(p) as z:
                    t = np.cumsum(z["t"])
                    bp, ap = z["bp"].astype(float) / 1e6, z["ap"].astype(float) / 1e6
                self.cache[k] = (t, bp, ap)
            if len(self.cache) > 400:
                self.cache.pop(next(iter(self.cache)))
        return self.cache[k]

    def half(self, day, sym, t_ns):
        q = self.get(day, sym)
        if q is None:
            return None
        t, bp, ap = q
        i = np.searchsorted(t, t_ns, side="right") - 1
        if i < 0 or t_ns - t[i] > 10 * SEC:
            return None
        b, a = bp[i], ap[i]
        if not (a > b > 0):
            return None
        mid = (a + b) / 2
        return (a - b) / 2 / mid


def boot(day_vals: dict, n=5000, seed=7):
    """Cluster bootstrap over days of the pooled mean; day_vals = day -> list of values."""
    days = [d for d in day_vals if day_vals[d]]
    if not days:
        return None, None, None
    sums = np.array([np.sum(day_vals[d]) for d in days])
    cnts = np.array([len(day_vals[d]) for d in days])
    pt = sums.sum() / cnts.sum()
    rng = np.random.default_rng(seed)
    idx = rng.integers(0, len(days), size=(n, len(days)))
    bs = sums[idx].sum(1) / np.maximum(cnts[idx].sum(1), 1)
    return float(pt), float(np.percentile(bs, 2.5)), float(np.percentile(bs, 97.5))


def cmd_score(root: str, frozen: str, qroot: str, out: str):
    fz = json.load(open(frozen))
    win = fz["winner"]
    Q = Quotes(qroot)
    days = days_of(root)
    trigs = [t for d in days for t in load_day(root, d)]
    ntr = len(trigs)
    rules = [win, "B0", "B1", "B2", "B3"] + sorted({k for t in trigs for k in t["rules"]
                                                     if k.startswith("C") and k != win})
    res = {"winner": win, "days": len(days), "triggers": ntr, "rules": {}}

    def rule_row(t, r):
        if r == "B1":
            k = b1_rule(t["rules"])
            return t["rules"].get(k) if k else None
        return t["rules"].get(r)

    def costs(t, e):
        """Per-entry net under quote costs (fallback 1 % scenario per missing side)."""
        hin = Q.half(t["day"], t["s"], e["te"])
        qin = hin is not None
        hin = hin if qin else SCEN["1"]
        state = {"q_out": True}

        def hout(ms):
            h = Q.half(t["day"], t["s"], e["te"] + ms * 10**6)
            if h is None:
                state["q_out"] = False
                return SCEN["1"]
            return h

        net, kind, ms = bracket(e, hin, hout)
        return net, kind, qin and state["q_out"], hin

    # Per trigger, per rule: metrics.
    per = collections.defaultdict(dict)
    for t in trigs:
        key = (t["day"], t["s"])
        for r in rules:
            e = rule_row(t, r)
            if e is None:
                continue
            m = {"day": t["day"], "sess": session(t["t0"]), "band": band(t["p0"]),
                 "path": t["path"], "scen": e["scen"], "delay": (e["te"] - t["t0"]) / SEC,
                 "s": t["s"], "t0": t["t0"]}
            for name, h in SCEN.items():
                m["net" + name], m["kind" + name], _ = flat(e, h)
            m["netq"], m["kindq"], m["qfull"], m["hin"] = costs(t, e)
            m["gross"], m["kindgross"], _ = flat(e, 0.0)
            m["mfe"], m["mae"] = mfe_mae(e)
            m["R"] = rmult(e)
            m["fwd"] = [f / e["pe"] - 1 for f in e["fwd"]]
            per[key][r] = m

    def summarize(rule, filt=lambda m: True, metric="netq"):
        rows = [v[rule] for v in per.values() if rule in v and filt(v[rule])]
        n = len(rows)
        if not n:
            return {"n": 0}
        kk = {"netq": "kindq", "gross": "kindgross"}.get(metric, "kind" + metric[3:])
        kinds = collections.Counter(x[kk] for x in rows)
        nd = len({x["day"] for x in rows})
        s = {"n": n, "per_day": round(n / len(days), 2),
             "coverage": round(n / ntr, 3) if ntr else None,
             "win": round(kinds["win"] / n, 3), "loss": round(kinds["loss"] / n, 3),
             "timeout": round((kinds["timeout"] + kinds["noprint"]) / n, 3),
             "mean_net": round(float(np.mean([x[metric] for x in rows])), 5),
             "median_net": round(float(np.median([x[metric] for x in rows])), 5),
             "median_delay_s": round(float(np.median([x["delay"] for x in rows])), 1),
             "days": nd}
        # paired deltas vs B0 and B1 on the same triggers
        for b in ("B0", "B1"):
            if rule == b:
                continue
            dv = collections.defaultdict(list)
            for v in per.values():
                if rule in v and b in v and filt(v[rule]):
                    dv[v[rule]["day"]].append(v[rule][metric] - v[b][metric])
            pt, lo, hi = boot(dv)
            s["d_vs_" + b] = [None if x is None else round(x, 5) for x in (pt, lo, hi)]
            s["n_pairs_" + b] = sum(len(x) for x in dv.values())
        return s

    for r in rules:
        res["rules"][r] = {m: summarize(r, metric=m) for m in ("netq", "net0.5", "net1", "net2")}
        res["rules"][r]["quote_only"] = summarize(r, lambda m: m["qfull"])
        res["rules"][r]["gross"] = summarize(r, metric="gross")
    splits = {
        "session": ("sess", ["pre", "rth"]),
        "scenario": ("scen", ["A", "B", "C", "D", "none"]),
        "band": ("band", ["0.10-1", "1-5", "5-25"]),
        "path": ("path", PATH_ORDER),
    }
    res["splits"] = {}
    for name, (fld, vals) in splits.items():
        res["splits"][name] = {}
        for r in [win] + BASE:
            res["splits"][name][r] = {v: summarize(r, lambda m, v=v, f=fld: m[f] == v)
                                      for v in vals}
    # Q5: half-spread at entry > 1 % by session (entries with an entry quote)
    q5 = {}
    for r in [win] + BASE:
        for sname in ("pre", "rth"):
            hs = [v[r]["hin"] for v in per.values() if r in v and v[r]["sess"] == sname]
            qs = [Q.half(v[r]["day"], v[r]["s"], 0) for v in []]
            del qs
            q5[f"{r}:{sname}"] = None if not hs else round(float(np.mean([h > 0.01 for h in hs])), 3)
    res["q5_halfspread_gt_1pct"] = q5
    # quote coverage
    cov = {}
    for r in [win] + BASE:
        rows = [v[r] for v in per.values() if r in v]
        cov[r] = None if not rows else round(float(np.mean([x["qfull"] for x in rows])), 3)
    res["quote_coverage_both_sides"] = cov
    # secondary
    sec = {}
    for r in [win] + BASE:
        rows = [v[r] for v in per.values() if r in v]
        if not rows:
            continue
        sec[r] = {
            "fwd_mean_1_3_5_10_15": [round(float(np.mean([x["fwd"][i] for x in rows])), 5)
                                     for i in range(5)],
            "mfe_med": round(float(np.median([x["mfe"] for x in rows])), 4),
            "mae_med": round(float(np.median([x["mae"] for x in rows])), 4),
            "R_mean": (round(float(np.mean([x["R"] for x in rows if x["R"] is not None])), 3)
                       if any(x["R"] is not None for x in rows) else None),
        }
    res["secondary"] = sec
    json.dump(res, open(out, "w"), indent=1)
    # per-entry detail for named cases / audits
    with open(out.replace(".json", "-entries.jsonl"), "w") as f:
        for (d, s), v in per.items():
            for r, m in v.items():
                if r in [win] + BASE:
                    f.write(json.dumps({"rule": r, **m}) + "\n")
    print(json.dumps({r: res["rules"][r]["netq"] for r in [win] + BASE}, indent=1))


if __name__ == "__main__":
    c = sys.argv[1]
    if c == "select":
        cmd_select(sys.argv[2], sys.argv[3])
    elif c == "windows":
        cmd_windows(sys.argv[2], sys.argv[3], sys.argv[4], sys.argv[5])
    elif c == "score":
        cmd_score(sys.argv[2], sys.argv[3], sys.argv[4], sys.argv[5])

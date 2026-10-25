"""RESEARCH ONLY (docs/research/post-alert-timing/protocol.md §7-§8, §10): tables, bootstrap,
selection and verdicts from pat.py rows.

  python -I agg.py select  ROWS_DIR OUT_JSON                 # §7 on EXPLORE
  python -I agg.py tables  ROWS_DIR FROZEN_JSON OUT_PREFIX [--confirm]   # every table (+ §8 verdicts)

Per-alert: an alert the strategy never entered counts 0. Per-trade: entered only. Eligible alerts:
P1 alerts whose symbol-day has at least one valid quote in the window (A1). Bootstrap: days
resampled with replacement, 5,000 times, seed 20261025.
"""
from __future__ import annotations

import collections
import json
import math
import os
import sys

import numpy as np

B = 5000
RNG_SEED = 20261025
DYN = ["S1_0.5", "S1_1.0", "S2_0.5", "S2_0.7", "L_S1_1.0", "L_S2_0.7"]
SIBLING = {"S1_0.5": "S1_1.0", "S1_1.0": "S1_0.5", "S2_0.5": "S2_0.7", "S2_0.7": "S2_0.5",
           "L_S1_1.0": "L_S2_0.7", "L_S2_0.7": "L_S1_1.0"}
SEGS = ["open_print", "early_pre", "late_pre", "mkt_open", "rth"]


def load(rows_dir: str, pop: str = "p1") -> dict:
    """alerts[key] = {strat: row}; key = (day, sym, t0)."""
    al = collections.defaultdict(dict)
    for f in sorted(os.listdir(rows_dir)):
        if not f.endswith(".jsonl"):
            continue
        for l in open(os.path.join(rows_dir, f)):
            r = json.loads(l)
            if pop not in r["pops"]:
                continue
            al[(r["day"], r["sym"], r["t0"])][r["strat"]] = r
    return al


def eligible(al: dict) -> list:
    return [k for k, v in al.items() if v["B0"]["quoted"] and v["B0"].get("entered")
            and not v["B0"].get("bad")]


def ret(r: dict, f: str = "net") -> float:
    if not r.get("entered") or r.get("bad"):
        return 0.0
    x = r.get(f)
    return 0.0 if x is None or (isinstance(x, float) and math.isnan(x)) else float(x)


def qc(r: dict) -> bool:
    return (not r.get("entered")) or (r.get("ekind") == "q" and r.get("xkind") == "q")


def boot(vals: list[float], days: list[str], seed: int = RNG_SEED) -> tuple[float, float, float]:
    if not vals:
        return (float("nan"),) * 3
    v = np.asarray(vals, float)
    du = sorted(set(days))
    ix = {d: i for i, d in enumerate(du)}
    di = np.array([ix[d] for d in days])
    s = np.bincount(di, weights=v, minlength=len(du))
    c = np.bincount(di, minlength=len(du)).astype(float)
    rng = np.random.default_rng(seed)
    pick = rng.integers(0, len(du), size=(B, len(du)))
    cnt = np.zeros((B, len(du)))
    for j in range(len(du)):
        cnt += 0  # keep shapes explicit
    # resampled sums: count how many times each day is drawn per replicate
    w = np.zeros((B, len(du)))
    np.add.at(w, (np.repeat(np.arange(B), len(du)), pick.ravel()), 1)
    num = w @ s
    den = w @ c
    est = num / np.where(den > 0, den, np.nan)
    lo, hi = np.nanpercentile(est, [2.5, 97.5])
    return float(v.mean()), float(lo), float(hi)


def pct(x):
    return None if x is None or (isinstance(x, float) and math.isnan(x)) else round(100 * x, 3)


def ci(t):
    return [pct(t[0]), pct(t[1]), pct(t[2])]


def summarize(al: dict, keys: list, st: str) -> dict:
    rows = [al[k][st] for k in keys if st in al[k]]
    ent = [r for r in rows if r.get("entered") and not r.get("bad")]
    days = [k[0] for k in keys if st in al[k]]
    pa = boot([ret(al[k][st]) for k in keys if st in al[k]], days)
    pt = boot([r["net"] for r in ent], [r["day"] for r in ent])
    reasons = collections.Counter(r.get("reason") for r in ent)
    q_hs = [r["ehs"] for r in ent if r.get("ekind") == "q"]
    out = {
        "alerts": len(rows), "trades": len(ent), "coverage": round(len(ent) / max(1, len(rows)), 4),
        "per_alert_net": ci(pa), "per_trade_net": ci(pt),
        "per_trade_gross_mean": pct(np.mean([r["gross"] for r in ent])) if ent else None,
        "win": round(reasons["win"] / max(1, len(ent)), 4),
        "loss": round(reasons["loss"] / max(1, len(ent)), 4),
        "timeout": round(reasons["timeout"] / max(1, len(ent)), 4),
        "median_delay_s": float(np.median([r["delay_s"] for r in ent])) if ent else None,
        "median_entry_half_spread_q": pct(float(np.median(q_hs))) if q_hs else None,
        "mean_entry_half_spread_q": pct(float(np.mean(q_hs))) if q_hs else None,
        "entry_quote_kind": dict(collections.Counter(r.get("ekind") for r in ent)),
        "fills_both_quoted": round(sum(1 for r in ent if r.get("ekind") == "q" and r.get("xkind") == "q") / max(1, len(ent)), 4),
        "ambiguous_seconds": sum(1 for r in ent if r.get("ambig")),
        "possible_halt": sum(1 for r in ent if r.get("halt")),
        "ask_usd_lt_2000": round(sum(1 for r in ent if r.get("ask_usd") is not None and r["ask_usd"] < 2000) / max(1, sum(1 for r in ent if r.get("ask_usd") is not None)), 4),
        "mfe_mean": pct(np.mean([r["mfe"] for r in ent])) if ent else None,
        "mae_mean": pct(np.mean([r["mae"] for r in ent])) if ent else None,
    }
    for mk in (5, 15, 30, 45):
        xs = [r[f"f{mk}"] for r in ent if r.get(f"f{mk}") is not None]
        out[f"fwd{mk}_mean"] = pct(np.mean(xs)) if xs else None
    # skipped alerts: what B0 earned on them (selection effect)
    sk = [k for k in keys if st in al[k] and not al[k][st].get("entered")]
    out["skipped"] = len(sk)
    out["b0_on_skipped_mean"] = pct(np.mean([ret(al[k]["B0"]) for k in sk])) if sk else None
    return out


def delta(al: dict, keys: list, a: str, b: str, f: str = "net", qconly: bool = False,
          drop_top: float = 0.0) -> dict:
    ks = [k for k in keys if a in al[k] and b in al[k]]
    if qconly:
        ks = [k for k in ks if qc(al[k][a]) and qc(al[k][b])]
    if drop_top > 0:
        xs = sorted(ks, key=lambda k: ret(al[k][a]), reverse=True)
        cut = int(round(drop_top * sum(1 for k in ks if al[k][a].get("entered"))))
        drop = set(xs[:cut])
        ks = [k for k in ks if k not in drop]
    d = [ret(al[k][a], f) - ret(al[k][b], f) for k in ks]
    t = boot(d, [k[0] for k in ks])
    return {"n": len(ks), "delta_pp": ci(t)}


def select(rows_dir: str, out: str) -> None:
    al = load(rows_dir)
    keys = eligible(al)
    tab = {}
    for st in DYN:
        s = summarize(al, keys, st)
        d = delta(al, keys, st, "D8")
        tab[st] = {"coverage": s["coverage"], "per_alert_delta_vs_D8": d["delta_pp"],
                   "per_alert_net": s["per_alert_net"]}
    ok = [st for st in DYN if tab[st]["coverage"] >= 0.60]
    order = {st: i for i, st in enumerate(DYN)}  # S before L, lower h / higher r handled below
    simp = {"S1_0.5": 0, "S1_1.0": 1, "S2_0.7": 2, "S2_0.5": 3, "L_S1_1.0": 4, "L_S2_0.7": 5}
    if ok:
        win = max(ok, key=lambda s: (tab[s]["per_alert_delta_vs_D8"][0], -simp[s]))
        note = "highest per-alert mean net delta vs D8 among coverage >= 60%"
    else:
        win = max(DYN, key=lambda s: tab[s]["coverage"])
        note = "no candidate reached 60% coverage: highest-coverage frozen"
    res = {"eligible_alerts": len(keys), "all_alerts": len(al), "candidates": tab,
           "winner": win, "rule": note, "sibling": SIBLING[win]}
    json.dump(res, open(out, "w"), indent=1)
    print(json.dumps(res, indent=1))


def split_tab(al, keys, field, strats, ref="B0", vals=None):
    groups = collections.defaultdict(list)
    for k in keys:
        groups[al[k]["B0"][field]].append(k)
    out = {}
    for g in (vals or sorted(groups)):
        ks = groups.get(g, [])
        if not ks:
            continue
        e = {"alerts": len(ks), "thin": len(ks) < 100}
        for st in strats:
            s = summarize(al, ks, st)
            e[st] = {"per_alert_net": s["per_alert_net"], "per_trade_net": s["per_trade_net"],
                     "coverage": s["coverage"],
                     "median_entry_half_spread_q": s["median_entry_half_spread_q"]}
            if st != ref:
                e[st]["delta_vs_" + ref] = delta(al, ks, st, ref)["delta_pp"]
                e[st]["gross_delta_vs_" + ref] = delta(al, ks, st, ref, "gross")["delta_pp"]
        out[str(g)] = e
    return out


def band(p):
    return "0.10-1" if p < 1 else ("1-5" if p < 5 else "5-25")


def tables(rows_dir: str, frozen_path: str, prefix: str, confirm: bool, pop: str = "p1") -> None:
    fz = json.load(open(frozen_path))
    W = fz["winner"]
    al = load(rows_dir, pop)
    keys = eligible(al)
    for k in keys:
        al[k]["B0"]["band"] = band(al[k]["B0"]["p0"])
    strats = sorted({s for k in keys for s in al[k]}, key=lambda s: (s != "B0", s))
    res = {"rows_dir": rows_dir, "frozen": fz, "all_alerts": len(al), "eligible_alerts": len(keys),
           "excluded_no_quotes": len(al) - len(keys)}
    res["strategies"] = {st: summarize(al, keys, st) for st in strats}
    res["delta_vs_B0"] = {st: {"primary": delta(al, keys, st, "B0"),
                               "quote_complete": delta(al, keys, st, "B0", qconly=True),
                               "gross": delta(al, keys, st, "B0", "gross")}
                          for st in strats if st != "B0"}
    res["delta_vs_D8"] = {st: {"primary": delta(al, keys, st, "D8"),
                               "quote_complete": delta(al, keys, st, "D8", qconly=True),
                               "gross": delta(al, keys, st, "D8", "gross")}
                          for st in strats if st != "D8"}
    # matched per-trade: only alerts the strategy entered (separates timing from abstention)
    def matched(st, ref):
        ks = [k for k in keys if al[k][st].get("entered") and not al[k][st].get("bad")]
        d = [al[k][st]["net"] - ret(al[k][ref]) for k in ks]
        g = [al[k][st]["gross"] - ret(al[k][ref], "gross") for k in ks]
        return {"n": len(ks), "net_pp": ci(boot(d, [k[0] for k in ks])),
                "gross_pp": ci(boot(g, [k[0] for k in ks]))}
    res["matched_per_trade"] = {st: {"vs_B0": matched(st, "B0"), "vs_D8": matched(st, "D8")}
                                for st in strats if st not in ("B0",)}
    core = ["B0", "D8", W]
    res["segments"] = split_tab(al, keys, "segment", core, vals=SEGS)
    # share of the waiting gain (sum of D8 - B0 per-alert) by segment
    tot = sum(ret(al[k]["D8"]) - ret(al[k]["B0"]) for k in keys)
    res["waiting_gain_share_by_segment"] = {
        sg: round(sum(ret(al[k]["D8"]) - ret(al[k]["B0"]) for k in keys if al[k]["B0"]["segment"] == sg) / tot, 4) if tot else None
        for sg in SEGS}
    res["waiting_gain_without_open_print"] = delta(al, [k for k in keys if al[k]["B0"]["segment"] != "open_print"], "D8", "B0")
    res["paths"] = split_tab(al, keys, "path", core)
    if pop == "p2":
        res["event_types"] = split_tab(al, keys, "event_type", core,
                                       vals=["first", "after_failed", "after_consolidation", "later_other"])
    res["bands"] = split_tab(al, keys, "band", core)
    res["halts_excluded"] = {st: delta(al, [k for k in keys if not al[k][st].get("halt") and not al[k]["B0"].get("halt")], st, "B0") for st in ["D8", W]}
    # robustness
    days = sorted({k[0] for k in keys})
    q = np.array_split(np.array(days), 4)
    res["quarters"] = {}
    for i, qd in enumerate(q):
        qs = set(qd.tolist())
        ks = [k for k in keys if k[0] in qs]
        res["quarters"][f"Q{i+1} {qd[0]}..{qd[-1]}"] = {
            "D8_vs_B0": delta(al, ks, "D8", "B0")["delta_pp"],
            f"{W}_vs_D8": delta(al, ks, W, "D8")["delta_pp"],
            f"{W}_vs_B0": delta(al, ks, W, "B0")["delta_pp"]}
    res["drop_top1pct"] = {"D8_vs_B0": delta(al, keys, "D8", "B0", drop_top=0.01),
                           f"{W}_vs_D8": delta(al, keys, W, "D8", drop_top=0.01)}
    res["sibling"] = {"name": SIBLING[W], "vs_D8": delta(al, keys, SIBLING[W], "D8")} if SIBLING[W] in strats else None
    # verdicts
    def verdict(a, b, extra_cov=None, sib=None):
        p = delta(al, keys, a, b)["delta_pp"]
        qcd = delta(al, keys, a, b, qconly=True)["delta_pp"]
        qpos = [v[a + "_vs_" + b if a + "_vs_" + b in v else list(v)[0]][0] for v in res["quarters"].values()] if False else None
        quarters = []
        for qd in q:
            qs = set(qd.tolist())
            ks = [k for k in keys if k[0] in qs]
            quarters.append(delta(al, ks, a, b)["delta_pp"][0])
        dt = delta(al, keys, a, b, drop_top=0.01)["delta_pp"][0]
        conds = {"lower_bound_gt_0": p[1] is not None and p[1] > 0,
                 "point_ge_0.5pp": p[0] is not None and p[0] >= 0.5,
                 "quote_complete_same_sign": qcd[0] is not None and p[0] is not None and (qcd[0] > 0) == (p[0] > 0),
                 "quarters_positive_ge_3_of_4": sum(1 for x in quarters if x is not None and x > 0) >= 3,
                 "drop_top1pct_positive": dt is not None and dt > 0}
        if extra_cov is not None:
            conds["coverage_ge_60pct"] = extra_cov >= 0.60
        if sib is not None:
            conds["sibling_same_sign"] = sib is not None and p[0] is not None and (sib > 0) == (p[0] > 0)
        if all(conds.values()):
            v = "Supported"
        elif conds["point_ge_0.5pp"]:
            v = "Promising but unconfirmed"
        else:
            v = "Unsupported"
        return {"delta_pp": p, "quote_complete_pp": qcd, "quarters_point_pp": quarters,
                "drop_top1pct_point_pp": dt, "conditions": conds, "verdict": v}
    qcov = np.mean([1.0 if (al[k]["B0"].get("ekind") == "q" and al[k]["B0"].get("xkind") == "q") else 0.0 for k in keys])
    sib = res["sibling"]["vs_D8"]["delta_pp"][0] if res["sibling"] else None
    res["verdicts"] = {
        "HA_D8_vs_B0": verdict("D8", "B0"),
        "HB_frozen_vs_D8": verdict(W, "D8", extra_cov=res["strategies"][W]["coverage"], sib=sib),
        "B0_fill_quote_coverage": round(float(qcov), 4),
        "data_limited": bool(qcov < 0.70),
    }
    res["H0_per_trade_net"] = {st: res["strategies"][st]["per_trade_net"] for st in ["B0", "D8", W]}
    res["H0_profitable"] = {st: (res["strategies"][st]["per_trade_net"][1] or -1) > 0 for st in ["B0", "D8", W]}
    json.dump(res, open(prefix + ".json", "w"), indent=1)
    print("wrote", prefix + ".json")


if __name__ == "__main__":
    a = sys.argv
    if a[1] == "select":
        select(a[2], a[3])
    elif a[1] == "tables":
        tables(a[2], a[3], a[4], "--confirm" in a, a[a.index("--pop") + 1] if "--pop" in a else "p1")

"""Pullback-context analysis (docs/research/pullback-context/protocol.md).

    pbc.py select   -> EXPLORE tables + the gate choice (selection-explore.json)
    pbc.py confirm  -> CONFIRM-PAST tables + the verdict (results-confirm.json); refuses to run
                       unless frozen.json exists

Reads the setup-research PBC rows (one per armed MP5 episode), the Runners entries of the timing
study, raw daily bars and daily closes. Gross continuation is primary; spread-adjusted figures are
on the quoted subset only (biased toward gated episodes, see the protocol).
"""
import bisect
import collections
import glob
import json
import os
import random
import sys
from datetime import datetime
from zoneinfo import ZoneInfo

import numpy as np

R = "C:/research/stockwatcher"
OUT = f"{R}/reports/pullback-context"
ET = ZoneInfo("America/New_York")
BUDGETS = (10, 20, 40)
PRIMARY = 20
BOOT = 5000
random.seed(20261025)


def et(ns):
    return datetime.fromtimestamp(ns / 1e9, ET)


def segment(ns):
    t = et(ns)
    m = t.hour * 60 + t.minute + t.second / 60
    if m < 4 * 60 + 2:
        return "open_print"
    if m < 7 * 60:
        return "early_pre"
    if m < 9 * 60 + 30:
        return "late_pre"
    if m < 10 * 60:
        return "mkt_open"
    return "rth"


def is0800(ns):
    t = et(ns)
    return t.hour == 8 and t.minute < 2


def load_rows(dirs):
    rows = []
    for d in dirs:
        for f in sorted(glob.glob(f"{R}/work-pbc/{d}/signals-*.jsonl")):
            for line in open(f):
                r = json.loads(line)
                t = et(r["armed_t"])
                m = t.hour * 60 + t.minute
                if r["in_band"] and 4 * 60 <= m < 15 * 60 + 45:
                    rows.append(r)
    return rows


def load_triggers(name):
    by = collections.defaultdict(list)
    for line in open(f"{R}/work-pat/triggers-{name}.jsonl"):
        r = json.loads(line)
        by[(r["day"], r["sym"])].append((r["t0"], r["path"]))
    for v in by.values():
        v.sort()
    return by


class Daily:
    """Raw daily bars (volume) and official closes from every archive, merged by date."""

    def __init__(self):
        self.files = {}
        for a in ("archive-past", "archive-hist", "archive-vps"):
            for f in glob.glob(f"{R}/{a}/daily-bars/raw/*.json"):
                self.files.setdefault(os.path.basename(f)[:-5], f)
        self.dates = sorted(self.files)
        self.cache = {}
        self.closes = {}
        for a in ("archive-past", "archive-hist", "archive-vps"):
            p = f"{R}/{a}/daily-closes.json"
            if os.path.exists(p):
                for d, m in json.load(open(p)).items():
                    self.closes.setdefault(d, {}).update(m)
        self.close_dates = sorted(self.closes)
        self.adv = {}

    def bars(self, d):
        if d not in self.cache:
            self.cache[d] = json.load(open(self.files[d]))["bars"]
        return self.cache[d]

    def adv30(self, day, sym):
        k = (day, sym)
        if k not in self.adv:
            i = bisect.bisect_left(self.dates, day)
            vols = []
            for d in reversed(self.dates[max(0, i - 30):i]):
                b = self.bars(d).get(sym)
                if b and b.get("v"):
                    vols.append(b["v"])
            self.adv[k] = sum(vols) / len(vols) if len(vols) >= 20 else None
        return self.adv[k]

    def prev_close(self, day, sym):
        i = bisect.bisect_left(self.close_dates, day)
        for d in reversed(self.close_dates[max(0, i - 5):i]):
            c = self.closes[d].get(sym)
            if c:
                return c
        return None


def gates(rows, trig, daily):
    """Adds g1 / g2 / pillar flags / the preceding Runners path to every row, causally."""
    for r in rows:
        entries = trig.get((r["day"], r["sym"]), [])
        before = [e for e in entries if e[0] <= r["armed_t"]]
        r["g1"] = bool(before)
        r["path"] = before[-1][1] if before else None
        before_b = [e for e in entries if r["cross_t"] is not None and e[0] <= r["cross_t"]]
        r["g1b"] = bool(before_b)
        px = r["last_at_arm"]
        price_ok = px is not None and 100_000 <= px <= 25_000_000
        pc = daily.prev_close(r["day"], r["sym"])
        gain = (px / 1e6 / pc - 1) if (px and pc) else None
        adv = daily.adv30(r["day"], r["sym"]) if r["g1"] else None
        rvol = (r["armed_vol"] / adv) if (adv and r["armed_vol"] is not None) else None
        r["gain"], r["rvol"] = gain, rvol
        r["rvol_unknown"] = rvol is None
        r["g2"] = r["g1"] and price_ok and gain is not None and gain >= 0.10 and rvol is not None and rvol >= 5
        r["seg"] = segment(r["armed_t"])
        r["c0800"] = is0800(r["armed_t"])


def notices(rows, gate, stage):
    """Per day: the gate's notices in time order. Stage A = arming, B = crossing."""
    by = collections.defaultdict(list)
    for r in rows:
        if gate != "g0" and not r[gate if stage == "A" or gate == "g2" else "g1b"]:
            continue
        if stage == "A":
            by[r["day"]].append((r["armed_t"], 1 if r["outcome"] == "continuation" else 0, r))
        elif r["cross_t"] is not None:
            by[r["day"]].append((r["cross_t"], 1 if r["outcome"] == "continuation" else 0, r))
    for v in by.values():
        v.sort(key=lambda x: x[0])
    return by


def capped(by, n):
    return {d: v[:n] for d, v in by.items()}


def rate(by, days=None):
    s = n = 0
    for d in days if days is not None else by:
        v = by.get(d, [])
        s += sum(x[1] for x in v)
        n += len(v)
    return (s / n if n else float("nan")), n


def boot_delta(a, b, days):
    pa, _ = rate(a, days)
    pb, _ = rate(b, days)
    out = []
    for _ in range(BOOT):
        smp = [random.choice(days) for _ in days]
        x, _ = rate(a, smp)
        y, _ = rate(b, smp)
        out.append(x - y)
    out.sort()
    return pa - pb, out[int(0.025 * BOOT)], out[int(0.975 * BOOT)]


def per_day(by, days):
    return sum(len(by.get(d, [])) for d in days) / max(1, len(days))


def tables(rows, days, label):
    t = {"label": label, "days": len(days), "episodes": len(rows)}
    for stage in ("A", "B"):
        for g in ("g0", "g1", "g2"):
            by = notices(rows, g, stage)
            t[f"{stage}_{g}_per_day_uncapped"] = per_day(by, days)
            t[f"{stage}_{g}_rate_uncapped"] = rate(by, days)
            for n in BUDGETS:
                t[f"{stage}_{g}_rate_b{n}"] = rate(capped(by, n), days)
        for g in ("g1", "g2"):
            for n in BUDGETS:
                d, lo, hi = boot_delta(capped(notices(rows, g, stage), n), capped(notices(rows, "g0", stage), n), days)
                t[f"{stage}_delta_{g}_vs_g0_b{n}"] = (d, lo, hi)
    # Crossing rate, time to cross, MFE/MAE by gate (uncapped, stage A population).
    for g in ("g0", "g1", "g2"):
        sel = [r for r in rows if g == "g0" or r[g]]
        cr = [r for r in sel if r["cross_t"] is not None]
        t[f"{g}_cross_rate"] = len(cr) / max(1, len(sel))
        t[f"{g}_secs_to_cross_median"] = float(np.median([(r["cross_t"] - r["armed_t"]) / 1e9 for r in cr])) if cr else None
        t[f"{g}_cont_given_cross"] = sum(r["outcome"] == "continuation" for r in cr) / max(1, len(cr))
        t[f"{g}_fail_given_cross"] = sum(r["outcome"] == "failure" for r in cr) / max(1, len(cr))
        t[f"{g}_mfe_med_bp"] = float(np.median([r["mfe_bp"] for r in sel if r["mfe_bp"] is not None])) if sel else None
        t[f"{g}_mae_med_bp"] = float(np.median([r["mae_bp"] for r in sel if r["mae_bp"] is not None])) if sel else None
    # Notices per day by segment, and the 08:00 cluster.
    for g in ("g0", "g1", "g2"):
        seg = collections.Counter(r["seg"] for r in rows if g == "g0" or r[g])
        t[f"{g}_per_day_by_segment"] = {k: v / len(days) for k, v in sorted(seg.items())}
        sel = [r for r in rows if (g == "g0" or r[g])]
        a = [r for r in sel if r["c0800"]]
        b = [r for r in sel if not r["c0800"]]
        t[f"{g}_0800"] = {"n": len(a), "stageA_rate": sum(r["outcome"] == "continuation" for r in a) / max(1, len(a)), "without_n": len(b), "without_stageA_rate": sum(r["outcome"] == "continuation" for r in b) / max(1, len(b))}
    # G1 by preceding path, and price band.
    for key, f in (("path", lambda r: r["path"]), ("band", lambda r: "0.1-1" if r["trigger"] < 1e6 else "1-5" if r["trigger"] < 5e6 else "5-25")):
        grp = collections.defaultdict(list)
        for r in rows:
            if r["g1"]:
                grp[f(r)].append(r["outcome"] == "continuation")
        t[f"g1_by_{key}"] = {k: (sum(v) / len(v), len(v)) for k, v in sorted(grp.items(), key=lambda x: str(x[0]))}
    t["g2_rvol_unknown_share_of_g1"] = sum(r["rvol_unknown"] for r in rows if r["g1"]) / max(1, sum(r["g1"] for r in rows))
    return t


def quoted_net(rows, qroot):
    """Half-spread-adjusted stage-A continuation on episodes whose crossing has a valid quote.
    Continuation pays the target minus both half-spreads; a failure the stop plus both; a timeout
    and a no-cross count as before (0). Reported, never verdict-bearing."""
    out = collections.defaultdict(lambda: [0, 0, 0.0])
    for r in rows:
        if r["cross_t"] is None:
            continue
        p = f"{qroot}/{r['day']}/{r['sym']}.npz"
        if not os.path.exists(p):
            continue
        with np.load(p) as z:
            t = np.cumsum(z["t"])
            bp, ap = z["bp"], z["ap"]
        i = int(np.searchsorted(t, r["cross_t"], side="right")) - 1
        if i < 0 or r["cross_t"] - t[i] > 10_000_000_000 or not (ap[i] > bp[i] > 0):
            continue
        hs = (ap[i] - bp[i]) / (ap[i] + bp[i])
        for g in ("g0", "g1", "g2"):
            if g != "g0" and not r[g]:
                continue
            o = out[g]
            o[0] += 1
            o[1] += r["outcome"] == "continuation"
            ret = {"continuation": 0.05, "failure": -0.03}.get(r["outcome"], 0.0)
            o[2] += ret - 2 * hs
    return {g: {"n": v[0], "cont_rate": v[1] / max(1, v[0]), "mean_net_bracket": v[2] / max(1, v[0])} for g, v in out.items()}


def main():
    cmd = sys.argv[1]
    os.makedirs(OUT, exist_ok=True)
    daily = Daily()
    if cmd == "select":
        rows = load_rows(["explore-hist", "explore-vps"])
        gates(rows, load_triggers("explore"), daily)
        days = sorted({r["day"] for r in rows})
        t = tables(rows, days, "EXPLORE")
        t["quoted"] = quoted_net(rows, f"{R}/quotes-pat/explore")
        cand = {}
        for g in ("g1", "g2"):
            cand[g] = {"rate_A_b20": t[f"A_{g}_rate_b20"][0], "per_day": t[f"A_{g}_per_day_uncapped"]}
        ok = {g: c for g, c in cand.items() if c["per_day"] >= 5}
        choice = max(ok, key=lambda g: (ok[g]["rate_A_b20"], g == "g1")) if ok else "g1"
        t["selection"] = {"candidates": cand, "choice": choice, "rule": "higher stage-A budget-20 rate with >= 5 notices/day; ties -> g1"}
        json.dump(t, open(f"{OUT}/selection-explore.json", "w"), indent=1, default=str)
        print(json.dumps(t["selection"], indent=1))
    elif cmd == "confirm":
        frozen = json.load(open(os.path.join(os.path.dirname(__file__), "..", "frozen.json")))
        g = frozen["gate"]
        rows = load_rows(["confirm"])
        gates(rows, load_triggers("confirm"), daily)
        days = sorted({r["day"] for r in rows})
        t = tables(rows, days, "CONFIRM-PAST")
        t["quoted"] = quoted_net(rows, f"{R}/quotes-pat/confirm")
        d, lo, hi = t[f"A_delta_{g}_vs_g0_b{PRIMARY}"]
        ex = json.load(open(f"{OUT}/selection-explore.json"))[f"A_delta_{g}_vs_g0_b{PRIMARY}"]
        q = [days[i * len(days) // 4:(i + 1) * len(days) // 4] for i in range(4)]
        a, b = capped(notices(rows, g, "A"), PRIMARY), capped(notices(rows, "g0", "A"), PRIMARY)
        quarters = [rate(a, qq)[0] - rate(b, qq)[0] for qq in q]
        n_gate = sum(len(v) for v in a.values())
        conds = {"lower_bound_gt_0": lo > 0, "delta_ge_3pp": d >= 0.03, "explore_same_sign": (ex[0] > 0) == (d > 0), "quarters_ge_3_positive": sum(x > 0 for x in quarters) >= 3}
        if n_gate < 300:
            verdict = "Inconclusive"
        elif all(conds.values()):
            verdict = "Supported"
        elif d >= 0.03 and sum(not v for v in conds.values()) == 1:
            verdict = "Promising but unconfirmed"
        else:
            verdict = "Unsupported"
        t["verdict"] = {"gate": g, "delta": (d, lo, hi), "explore_delta": ex, "quarters": quarters, "conditions": conds, "gate_notices": n_gate, "verdict": verdict}
        json.dump(t, open(f"{OUT}/results-confirm.json", "w"), indent=1, default=str)
        print(json.dumps(t["verdict"], indent=1, default=str))


if __name__ == "__main__":
    main()

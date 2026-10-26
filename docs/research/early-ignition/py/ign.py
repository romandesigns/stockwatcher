"""Early ignition study (protocol.md). Phases: `dev` (CV, selection, freeze) and `test` (frozen, once).

python -I ign.py dev    -> work-ign/selection-dev.json, frozen.json (+ model files)
python -I ign.py test   -> reports/early-ignition/test.json (refuses without a committed frozen.json)
"""
import glob
import hashlib
import json
import os
import sys
from datetime import datetime, timezone
from zoneinfo import ZoneInfo

import lightgbm as lgb
import numpy as np
import pandas as pd
from sklearn.linear_model import LogisticRegression

R = "C:/research/stockwatcher"
EXP = f"{R}/reports/burst/export"
EXPT = f"{R}/reports/burst/export-test"
WORK = f"{R}/work-ign"
OUT = f"{R}/reports/early-ignition"
HERE = os.path.dirname(os.path.abspath(__file__))
FROZEN = os.path.join(HERE, "..", "frozen.json")
ET = ZoneInfo("America/New_York")
FEATS = ["ld5", "ld15", "ld30", "rd5", "rd15", "rd30", "rt15", "radv30", "ret5", "ret15", "ret30", "brk", "buy5", "buy15", "upr", "nhd15", "nsw15", "vex15", "blk15", "odd15", "asz15", "logp", "sess", "tod", "gain", "gap", "rvol", "dhi", "vwap", "idle30", "rng30", "ldd", "ep", "age"]
BUDGETS = [5, 10, 20]
DEDUP_NS = 10 * 60 * 1_000_000_000
LGB_PARAMS = dict(objective="binary", num_leaves=15, max_depth=4, learning_rate=0.05, n_estimators=300, min_child_samples=200, subsample=0.8, subsample_freq=1, colsample_bytree=0.8, reg_lambda=1.0, random_state=7, deterministic=True, force_row_wise=True, n_jobs=8, verbose=-1)
CONFIGS = ["I1-5", "I1-15", "I2-5", "I3-5", "I3-15", "I4-5"]
SIMPLE = {"I1": 0, "I2": 1, "I3": 2, "I4": 3}


def days_in(d, kind, lo, hi):
    out = []
    for f in sorted(glob.glob(f"{d}/{kind}-*.csv")):
        day = os.path.basename(f)[len(kind) + 1:-4]
        if lo <= day <= hi:
            out.append(day)
    return out


def batch_window(alert_ns):
    t = pd.to_datetime(alert_ns, utc=True).dt.tz_convert(ET)
    m = t.dt.hour * 3600 + t.dt.minute * 60 + t.dt.second
    return (m >= 4 * 3600) & (m < 4 * 3600 + 180)


def load(d, kind, days):
    frames = []
    for day in days:
        f = f"{d}/{kind}-{day}.csv"
        if not os.path.exists(f):
            continue
        df = pd.read_csv(f, low_memory=False)
        df = df.loc[:, ~df.columns.duplicated()]
        frames.append(df)
    df = pd.concat(frames, ignore_index=True)
    t = "alert_ns" if "alert_ns" in df else "b"
    df = df[~batch_window(df[t])].reset_index(drop=True)
    return df


def sets(df):
    onset = df["age"] <= 60
    quiet = (df["idle30"] >= 0.5) | (df["rng30"] <= 0.03)
    brk = df["brk"] > 0
    gate = df["gate"] == 1
    return gate & onset, gate & onset & quiet, gate & onset & quiet & brk


# ---- alert selection ---------------------------------------------------------------------------

def dedup(sub):
    """Forward dedup in time order: first row per symbol per 10 min. sub sorted by alert_ns."""
    last = {}
    keep = []
    for i, s, t in zip(sub.index, sub["sym"].values, sub["alert_ns"].values):
        p = last.get(s)
        if p is None or t - p >= DEDUP_NS:
            last[s] = t
            keep.append(i)
    return keep


def alerts_topn(df, score, n):
    """Per day: the largest threshold whose forward dedup yields >= n alerts, then the n highest."""
    out = []
    d = df.assign(_s=score).dropna(subset=["_s"])
    for day, g in d.groupby("day"):
        g = g.sort_values("alert_ns")
        vals = np.sort(g["_s"].unique())[::-1]
        lo, hi, best = 0, len(vals) - 1, None
        while lo <= hi:
            mid = (lo + hi) // 2
            k = dedup(g[g["_s"] >= vals[mid]])
            if len(k) >= n:
                best = k
                hi = mid - 1
            else:
                lo = mid + 1
        if best is None:
            best = dedup(g)
        sel = g.loc[best].nlargest(n, "_s")
        out.append(sel)
    return pd.concat(out) if out else d.iloc[:0]


def alerts_theta(df, score, theta):
    d = df.assign(_s=score).dropna(subset=["_s"])
    d = d[d["_s"] >= theta]
    out = []
    for day, g in d.groupby("day"):
        g = g.sort_values("alert_ns")
        out.append(g.loc[dedup(g)])
    return pd.concat(out) if out else d.iloc[:0]


def theta_for(df, score, n, ndays):
    """The threshold giving a mean of n alerts/day in sample (search on the score quantiles)."""
    s = score.dropna()
    lo, hi = float(s.min()), float(s.max())
    for _ in range(40):
        mid = (lo + hi) / 2
        k = len(alerts_theta(df, score, mid)) / ndays
        if k > n:
            lo = mid
        else:
            hi = mid
    return hi


# ---- scorers -----------------------------------------------------------------------------------

class Model:
    def __init__(self, kind):
        self.kind = kind

    def fit(self, X, y):
        X = X[FEATS].astype(float)
        if self.kind == "lr":
            self.lo, self.hi = X.quantile(0.005), X.quantile(0.995)
            Xc = X.clip(self.lo, self.hi, axis=1)
            self.med = Xc.median()
            Xc = Xc.fillna(self.med)
            self.mu, self.sd = Xc.mean(), Xc.std().replace(0, 1)
            self.m = LogisticRegression(C=1.0, solver="lbfgs", max_iter=2000).fit(((Xc - self.mu) / self.sd).values, y)
        else:
            self.m = lgb.LGBMClassifier(**LGB_PARAMS).fit(X.values, y)
        return self

    def score(self, X):
        X = X[FEATS].astype(float)
        if self.kind == "lr":
            Xc = X.clip(self.lo, self.hi, axis=1).fillna(self.med)
            return self.m.predict_proba(((Xc - self.mu) / self.sd).values)[:, 1]
        return self.m.predict_proba(X.values)[:, 1]


def config_score(cfg, df, train=None):
    """Score of configuration cfg on df (NaN where it never alerts); fit on train when needed."""
    var, g = cfg.split("-")
    onset, quiet, brk = sets(df)
    s = pd.Series(np.nan, index=df.index)
    if var == "I1":
        s[brk] = df.loc[brk, "rd15"]
        return s, None
    mask = quiet if var == "I4" else onset
    m = None
    if train is not None:
        to, tq, _ = sets(train)
        tm = tq if var == "I4" else to
        m = Model("lr" if var == "I2" else "gbm").fit(train[tm], train.loc[tm, "c10"].astype(int).values)
    return s, m


def apply_model(cfg, df, m):
    var = cfg.split("-")[0]
    onset, quiet, brk = sets(df)
    mask = quiet if var == "I4" else onset
    s = pd.Series(np.nan, index=df.index)
    if mask.any():
        s[mask] = m.score(df[mask])
    return s


def burst_scorer():
    b = lgb.Booster(model_file=f"{R}/reports/burst/frozen/V3-5.lgb.txt")
    return lambda df: pd.Series(np.where(df["gate"] == 1, b.predict(df[FEATS].astype(float).values), np.nan), index=df.index)


# ---- metrics -----------------------------------------------------------------------------------

def load_triggers():
    by = {}
    for line in open(f"{R}/work-pat/triggers-explore.jsonl"):
        r = json.loads(line)
        by.setdefault((r["day"], r["sym"]), []).append(r["t0"])
    for v in by.values():
        v.sort()
    return by


def load_rockets(days):
    ev = []
    for d in days:
        f = f"{R}/reports/rockets/events/{d}.jsonl"
        if os.path.exists(f):
            for line in open(f):
                r = json.loads(line)
                ev.append((d, r["symbol"], r["start_bar_open_ns"], r["plus30_ns"]))
    return ev


def timing(al, trig):
    H = 60 * 60 * 1_000_000_000
    ahead, after, none, leads = 0, 0, 0, []
    for d, s, t in zip(al["day"], al["sym"], al["alert_ns"]):
        ts = trig.get((d, s), [])
        before = [x for x in ts if t - H <= x < t]
        nxt = [x for x in ts if t <= x <= t + H]
        if before:
            after += 1
        elif nxt:
            ahead += 1
            leads.append((nxt[0] - t) / 60e9)
        else:
            none += 1
    n = max(1, len(al))
    return {"ahead_of_runners": ahead / n, "after_runners": after / n, "no_runners_60m": none / n, "median_lead_min": float(np.median(leads)) if leads else None}


def move_share(al):
    L = al["last"] / ((1 + al["rng30"]) * np.exp(al["brk"]))
    P = al["pa"] * (1 + al["mfe"])
    ok = P > L
    sh = ((al["pa"] - L) / (P - L))[ok].clip(0, 1)
    return float(sh.median()) if len(sh) else None


def rocket_recall(al, rockets):
    if not rockets:
        return None
    idx = {}
    for d, s, t in zip(al["day"], al["sym"], al["alert_ns"]):
        idx.setdefault((d, s), []).append(t)
    hit = sum(1 for d, s, st, p30 in rockets if any(st - 600e9 <= t <= p30 for t in idx.get((d, s), [])))
    return hit / len(rockets)


def summarize(al, ndays, trig, rockets):
    n = len(al)
    r = {"n": n, "per_day": n / ndays}
    if n:
        r.update({"cont10": float(al["c10"].mean()), "cont20": float(al["c20"].mean()), "cont30": float(al["c30"].mean()), "mfe_med": float(al["mfe"].median()), "mae_med": float(al["mae"].median())})
        r.update(timing(al, trig))
        r["move_share_med"] = move_share(al)
        r["rocket_recall"] = rocket_recall(al, rockets)
    return r


def b1_draws(al, pool, seed):
    """Volatility-matched random: 5 draws per alert from the same day/session/bucket pool."""
    rng = np.random.default_rng(seed)
    hits, n = {}, {}
    grouped = {k: g for k, g in pool.groupby(["day", "sess", "bucket"])}
    for d, ss, b, rg in zip(al["day"], al["sess"], al["bucket"], al["range5_bp"]):
        g = grouped.get((d, ss, b))
        if g is None:
            continue
        cand = g[g["range5_bp"] >= max(300, rg if pd.notna(rg) else 300)]
        if not len(cand):
            continue
        pick = cand["c10"].values[rng.integers(0, len(cand), 5)]
        hits[d] = hits.get(d, 0) + pick.sum()
        n[d] = n.get(d, 0) + 5
    return hits, n


def per_day(al, col="c10"):
    g = al.groupby("day")[col].agg(["sum", "count"])
    return g["sum"].to_dict(), g["count"].to_dict()


def boot_diff(a, b, days, reps=2000, seed=11):
    """Day-clustered bootstrap of pooled precision(a) - precision(b); a, b = (hits, n) dicts by day."""
    rng = np.random.default_rng(seed)
    days = list(days)
    ah = np.array([a[0].get(d, 0) for d in days], float); an = np.array([a[1].get(d, 0) for d in days], float)
    bh = np.array([b[0].get(d, 0) for d in days], float); bn = np.array([b[1].get(d, 0) for d in days], float)
    pt = ah.sum() / max(1, an.sum()) - bh.sum() / max(1, bn.sum())
    res = []
    for _ in range(reps):
        i = rng.integers(0, len(days), len(days))
        res.append(ah[i].sum() / max(1, an[i].sum()) - bh[i].sum() / max(1, bn[i].sum()))
    res.sort()
    return {"diff": pt, "lo": res[int(0.025 * reps)], "hi": res[int(0.975 * reps)]}


# ---- phases ------------------------------------------------------------------------------------

def dev():
    os.makedirs(WORK, exist_ok=True)
    days = days_in(EXP, "rows5", "2026-03-30", "2026-07-31")
    r5 = load(EXP, "rows5", days)
    r15 = load(EXP, "rows15", days)
    data = {"5": r5, "15": r15}
    blocks = np.array_split(np.array(days), 6)
    cv = {}
    for cfg in CONFIGS:
        g = cfg.split("-")[1]
        df = data[g]
        hits = n = 0
        for k in range(1, 6):
            tr_days, va_days = set(np.concatenate(blocks[:k])), set(blocks[k])
            tr, va = df[df["day"].isin(tr_days)], df[df["day"].isin(va_days)].copy()
            s, m = config_score(cfg, va, train=tr if not cfg.startswith("I1") else None)
            if m is not None:
                s = apply_model(cfg, va, m)
            al = alerts_topn(va, s, 10)
            hits += al["c10"].sum()
            n += len(al)
        cv[cfg] = {"cont10_at10": hits / max(1, n), "alerts": int(n)}
        print(cfg, cv[cfg], flush=True)
    best = max(CONFIGS, key=lambda c: (round(cv[c]["cont10_at10"], 12), -SIMPLE[c.split("-")[0]], int(c.split("-")[1])))
    g = best.split("-")[1]
    df = data[g]
    s, m = config_score(best, df, train=df if not best.startswith("I1") else None)
    model_file = None
    if m is not None:
        s = apply_model(best, df, m)
        import pickle
        model_file = f"{WORK}/{best}.pkl"
        pickle.dump(m, open(model_file, "wb"))
    nd = len(days)
    thetas = {f"B{b}": theta_for(df, s, b, nd) for b in BUDGETS}
    # Baselines calibrated the same way on DEV.
    bu = burst_scorer()(r5)
    va_s = pd.Series(np.where(r5["gate"] == 1, r5["rd15"], np.nan), index=r5.index)
    b3_s = pd.to_numeric(r15["b3"], errors="coerce")
    base_thetas = {
        "BU": {f"B{b}": theta_for(r5, bu, b, nd) for b in BUDGETS},
        "VA": {f"B{b}": theta_for(r5, va_s, b, nd) for b in BUDGETS},
        "B3": {f"B{b}": theta_for(r15, b3_s, b, nd) for b in BUDGETS},
    }
    sel = {"cv": cv, "winner": best, "dev_days": days, "blocks": [list(b) for b in blocks]}
    json.dump(sel, open(f"{WORK}/selection-dev.json", "w"), indent=1)
    frozen = {
        "study": "early-ignition protocol v1", "winner": best, "thetas": thetas, "baseline_thetas": base_thetas,
        "model_file": model_file, "model_sha256": hashlib.sha256(open(model_file, "rb").read()).hexdigest() if model_file else None,
        "cv": cv, "dev_days": days, "time": datetime.now(timezone.utc).isoformat(),
    }
    json.dump(frozen, open(FROZEN, "w"), indent=1)
    json.dump(sel, open(os.path.join(HERE, "..", "selection-dev.json"), "w"), indent=1)
    print("winner", best, thetas, base_thetas)


def score_set(df_by_g, frozen, m, label, days, trig, rockets):
    best = frozen["winner"]
    g = best.split("-")[1]
    df = df_by_g[g]
    if best.startswith("I1"):
        s, _ = config_score(best, df)
    else:
        s = apply_model(best, df, m)
    r5, r15, pool = df_by_g["5"], df_by_g["15"], df_by_g["pool"]
    bu = burst_scorer()(r5)
    va_s = pd.Series(np.where(r5["gate"] == 1, r5["rd15"], np.nan), index=r5.index)
    b3_s = pd.to_numeric(r15["b3"], errors="coerce")
    nd = len(days)
    out = {"days": nd}
    for b in BUDGETS:
        key = f"B{b}"
        cand = alerts_theta(df, s, frozen["thetas"][key])
        al = {"cand": cand, "BU": alerts_theta(r5, bu, frozen["baseline_thetas"]["BU"][key]), "VA": alerts_theta(r5, va_s, frozen["baseline_thetas"]["VA"][key]), "B3": alerts_theta(r15, b3_s, frozen["baseline_thetas"]["B3"][key])}
        res = {k: summarize(v, nd, trig, rockets) for k, v in al.items()}
        b1h, b1n = b1_draws(cand, pool, seed=b)
        res["B1"] = {"n_draws": int(sum(b1n.values())), "cont10": sum(b1h.values()) / max(1, sum(b1n.values()))}
        ca = per_day(cand)
        res["delta"] = {"B1": boot_diff(ca, (b1h, b1n), days), **{k: boot_diff(ca, per_day(al[k]), days) for k in ("B3", "VA", "BU")}}
        # Ahead-of-Runners difference vs Burst (for the "earlier" claim).
        def ahead(a):
            h, n = {}, {}
            for d, g2 in a.groupby("day"):
                t = timing(g2, trig)
                h[d] = t["ahead_of_runners"] * len(g2)
                n[d] = len(g2)
            return h, n
        res["ahead_vs_BU"] = boot_diff(ahead(cand), ahead(al["BU"]), days)
        out[key] = res
    return out


def test():
    frozen = json.load(open(FROZEN))
    import pickle, subprocess
    tracked = subprocess.run(["git", "-C", HERE, "log", "--oneline", "-1", "--", FROZEN], capture_output=True, text=True).stdout.strip()
    if not tracked:
        sys.exit("frozen.json is not committed: refusing to read TEST")
    m = pickle.load(open(frozen["model_file"], "rb")) if frozen["model_file"] else None
    trig = load_triggers()
    os.makedirs(OUT, exist_ok=True)
    result = {"frozen_commit": tracked}
    for label, d, lo, hi in [("TEST-H", EXPT, "2026-08-03", "2026-09-21"), ("TEST-V", EXP, "2026-09-22", "2026-10-20")]:
        days = days_in(d, "rows5", lo, hi)
        df_by_g = {"5": load(d, "rows5", days), "15": load(d, "rows15", days), "pool": load(d, "pool", days)}
        result[label] = score_set(df_by_g, frozen, m, label, days, trig, load_rockets(days))
        print(label, json.dumps(result[label]["B10"], default=str)[:1500], flush=True)
    json.dump(result, open(f"{OUT}/test.json", "w"), indent=1, default=str)
    json.dump(result, open(os.path.join(HERE, "..", "results-test.json"), "w"), indent=1, default=str)


if __name__ == "__main__":
    {"dev": dev, "test": test}[sys.argv[1]]()

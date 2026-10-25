"""RESEARCH ONLY (docs/research/post-alert-timing/protocol.md §9-§10): Runners lateness.

  python -I lateness.py analyse EXPORT_ROOT TRIGROOTS OUTDIR [QUOTES_ROOT]
  python -I lateness.py gallery EXPORT_ROOT OUTDIR            # needs matplotlib

EXPORT_ROOT holds <day>/alerts.jsonl + fires.jsonl from `running::lateness` (lateness.rs).
TRIGROOTS is a comma-separated list of dirs with <day>/triggers.jsonl (every path's first entry
per symbol, the Runners alert stream used for "followed by a Runners alert within 30 min").

Two kinds of quantity are kept apart everywhere:
- RETROSPECTIVE (hindsight, descriptive only): L* (lowest print in the 60 min before the alert),
  P* (highest print in the 60 min after), the retrospective ignition bar (found by scanning bars
  after L*, a time no real-time system could know was "after the low"), share of move before alert.
- CAUSAL: the early flag (completed-bar rule computable in real time), the alert itself, spreads
  and trade counts at a time.
"""

from __future__ import annotations

import collections
import datetime as dt
import json
import math
import os
import random
import sys
from zoneinfo import ZoneInfo

import numpy as np

ET = ZoneInfo("America/New_York")
SEC = 10**9
MIN = 60 * SEC
CAPS = {"running": 150, "fast": 200}
PATHS = ["burst", "run", "fast", "running"]
SEGS = ["open_print", "early_pre", "late_pre", "mkt_open", "rth"]


def et_min(ns: int) -> float:
    t = dt.datetime.fromtimestamp(ns / SEC, tz=ET)
    return t.hour * 60 + t.minute + t.second / 60


def segment(ns: int) -> str:
    m = et_min(ns)
    if m < 4 * 60 + 2:
        return "open_print"
    if m < 7 * 60:
        return "early_pre"
    if m < 9 * 60 + 30:
        return "late_pre"
    if m < 10 * 60:
        return "mkt_open"
    return "rth"


def band(p: float) -> str:
    return "0.10-1" if p < 1 else ("1-5" if p < 5 else "5-25")


def days_of(root: str) -> list[str]:
    return sorted(d for d in os.listdir(root) if os.path.exists(os.path.join(root, d, "done")))


# ---------------------------------------------------------------- quotes (optional)


class Quotes:
    def __init__(self, roots: list[str]):
        self.roots = roots
        self.cache: dict = {}

    def get(self, day, sym):
        k = (day, sym)
        if k not in self.cache:
            self.cache[k] = None
            for r in self.roots:
                p = os.path.join(r, day, f"{sym}.npz")
                if os.path.exists(p):
                    with np.load(p) as z:
                        t = np.cumsum(z["t"])
                        bp, ap = z["bp"].astype(float) / 1e6, z["ap"].astype(float) / 1e6
                    self.cache[k] = (t, bp, ap)
                    break
            if len(self.cache) > 300:
                self.cache.pop(next(iter(self.cache)))
        return self.cache[k]

    def half(self, day, sym, t_ns):
        """Half-spread / mid at t (valid quote: <= 10 s old, ask > bid > 0), else None."""
        q = self.get(day, sym)
        if q is None:
            return None
        t, bp, ap = q
        i = int(np.searchsorted(t, t_ns, side="right")) - 1
        if i < 0 or t_ns - t[i] > 10 * SEC:
            return None
        b, a = bp[i], ap[i]
        if not (a > b > 0):
            return None
        return (a - b) / (a + b)


# ---------------------------------------------------------------- per-alert measures


def bar_med(arr: np.ndarray, k: int, col: int) -> float:
    return float(np.median(arr[k - 30 : k, col]))


def measure(a: dict, fires: list[dict], q: Quotes | None, day: str, pc: float | None = None) -> dict:
    bars = np.asarray(a["bars"], dtype=float)  # o h l c v n
    t04, t0, p0 = a["t04"], a["t0"], a["p0"]
    rng = np.where(bars[:, 0] > 0, bars[:, 1] - bars[:, 2], 0.0)
    r = {"day": day, "s": a["s"], "path": a["path"], "t0": t0, "p0": p0,
         "seg": segment(t0), "band": band(p0), "ft": a["ft"],
         "n_before": a["n_before"], "first": a["first"]}
    kb = lambda t: int((t - t04) // MIN)  # bar index holding time t
    r["trades_alert_bar"] = float(bars[kb(t0) - 1, 5]) if kb(t0) >= 1 else None  # last completed
    # CAUSAL context: rise from the previous close, and from the session low before the alert.
    if pc:
        r["gap_prev_close"] = p0 / pc - 1
    k0 = kb(t0)
    lows = [b[2] for b in bars[:k0] if b[0] > 0]
    if a["lstar"]:
        lows.append(a["lstar"][1])
    if lows:
        dlo = min(lows)
        r["day_rise_before"] = p0 / dlo - 1
        # RETROSPECTIVE day-level share (sensitivity to the 60-min window): session low before
        # the alert to the highest bar high from the alert's bar to 20:00.
        hi_after = max([b[1] for b in bars[k0:] if b[0] > 0] or [0.0])
        if hi_after > dlo:
            r["day_share_before"] = min((p0 - dlo) / (hi_after - dlo), 1.0)
    if not a["lstar"]:
        return r
    tL, L = a["lstar"]
    r["tL"], r["L"] = tL, L
    r["min_L_to_alert"] = (t0 - tL) / MIN
    r["disp_before"] = p0 / L - 1
    r["trades_L_bar"] = float(bars[kb(tL), 5])
    if a["pstar"]:
        tP, P = a["pstar"]
        r["tP"], r["P"] = tP, P
        r["disp_after"] = P / p0 - 1
        if P > L:
            r["share_before"] = min((p0 - L) / (P - L), 1.0)
            r["no_new_high"] = P <= p0
    # RETROSPECTIVE ignition: first bar after the L* bar with range >= 3x and volume >= 3x the
    # medians of the 30 bars before it, within (t_L, t0 + 60 min].
    k_end = min(kb(t0 + 60 * MIN), len(bars) - 1)
    for k in range(max(kb(tL) + 1, 30), k_end + 1):
        if bars[k, 0] <= 0 or bars[k, 4] <= 0 or rng[k] <= 0:
            continue
        if rng[k] >= 3 * float(np.median(rng[k - 30 : k])) and bars[k, 4] >= 3 * bar_med(bars, k, 4):
            ts = t04 + k * MIN
            r["ign_t"] = ts
            r["ign_lead_min"] = (t0 - ts) / MIN  # >0: ignition bar STARTED before the alert
            r["ign_disp"] = bars[k, 3] / L - 1
            r["trades_ign_bar"] = float(bars[k, 5])
            if "P" in r and r["P"] > L:
                r["ign_share"] = (bars[k, 3] - L) / (r["P"] - L)
            break
    # CAUSAL early flag inside the same move: first fire with bar end in [t_L, t0].
    for strict in (False, True):
        tag = "flag" + ("_strict" if strict else "")
        fs = [f for f in fires if tL <= f["t"] <= t0 and (not strict or f["med"] > 0)]
        if fs:
            f = min(fs, key=lambda f: f["t"])
            r[tag + "_t"] = f["t"]
            r[tag + "_lead_min"] = (t0 - f["t"]) / MIN
            r[tag + "_disp"] = f["c"] / L - 1
            if "P" in r and r["P"] > L:
                r[tag + "_share"] = (f["c"] - L) / (r["P"] - L)
            if not strict:
                r["trades_flag_bar"] = f["n"]
    if q is not None:
        for name, t in (("L", tL), ("ign", r.get("ign_t", None) and r["ign_t"] + MIN),
                        ("flag", r.get("flag_t")), ("alert", t0)):
            if t is not None:
                r[f"half_{name}"] = q.half(day, a["s"], t)
    return r


# ---------------------------------------------------------------- triggers / fires


def triggers_of(roots: list[str], day: str) -> dict[str, list[int]]:
    """Every capped Runners path-first entry time per symbol for the day (sorted)."""
    for root in roots:
        p = os.path.join(root, day, "triggers.jsonl")
        if os.path.exists(p):
            trig = [json.loads(l) for l in open(p)]
            trig.sort(key=lambda r: (r["t"], r["s"]))
            used = collections.Counter()
            out = collections.defaultdict(list)
            for r in trig:
                c = CAPS.get(r["path"])
                if c is not None:
                    used[r["path"]] += 1
                    if used[r["path"]] > c:
                        continue
                out[r["s"]].append(r["t"])
            return out
    return {}


def fire_stats(fires: list[dict], trig: dict[str, list[int]], strict: bool) -> dict:
    fs = [f for f in fires if 0.10 <= f["c"] <= 25 and 4 * 60 + 30 <= et_min(f["t"]) <= 15 * 60 + 15
          and (not strict or f["med"] > 0)]
    n = len(fs)
    fol = before = fol_before = 0
    syms = set()
    for f in fs:
        ts = trig.get(f["s"], [])
        nxt = [t for t in ts if f["t"] < t <= f["t"] + 30 * MIN]
        first = min(ts) if ts else None
        b = first is None or f["t"] < first
        fol += bool(nxt)
        before += b
        fol_before += b and bool(nxt)
        syms.add(f["s"])
    return {"fires": n, "symbols": len(syms), "followed30": fol, "before_first_alert": before,
            "before_and_followed30": fol_before}


# ---------------------------------------------------------------- tables


def dist(xs):
    xs = [x for x in xs if x is not None and not (isinstance(x, float) and math.isnan(x))]
    if not xs:
        return {"n": 0}
    a = np.asarray(xs, dtype=float)
    p = np.percentile(a, [10, 25, 50, 75, 90])
    return {"n": len(a), "mean": float(a.mean()), "p10": float(p[0]), "p25": float(p[1]),
            "med": float(p[2]), "p75": float(p[3]), "p90": float(p[4])}


def boot_share(day_vals: dict, n=5000, seed=7):
    days = [d for d in day_vals if day_vals[d]]
    if not days:
        return None
    sums = np.array([np.sum(day_vals[d]) for d in days], dtype=float)
    cnts = np.array([len(day_vals[d]) for d in days], dtype=float)
    rng = np.random.default_rng(seed)
    idx = rng.integers(0, len(days), size=(n, len(days)))
    bs = sums[idx].sum(1) / np.maximum(cnts[idx].sum(1), 1)
    return [float(sums.sum() / cnts.sum()), float(np.percentile(bs, 2.5)), float(np.percentile(bs, 97.5))]


METRICS = ["gap_prev_close", "day_rise_before", "day_share_before", "disp_before", "share_before", "min_L_to_alert", "disp_after", "ign_lead_min",
           "ign_disp", "ign_share", "flag_lead_min", "flag_disp", "flag_share",
           "flag_strict_lead_min", "flag_strict_share", "trades_L_bar", "trades_ign_bar",
           "trades_flag_bar", "trades_alert_bar", "half_L", "half_ign", "half_flag", "half_alert"]


def table(rows: list[dict]) -> dict:
    out = {"n": len(rows)}
    for m in METRICS:
        out[m] = dist([r.get(m) for r in rows])
    has = lambda k: [r for r in rows if "L" in r]
    hl = has("L")
    for name, cond in (("with_flag_in_move", lambda r: "flag_t" in r),
                       ("with_flag_strict_in_move", lambda r: "flag_strict_t" in r),
                       ("ign_before_alert", lambda r: r.get("ign_lead_min", -1) > 0),
                       ("no_new_high_60m", lambda r: r.get("no_new_high", False)),
                       ("share_before_ge_50", lambda r: r.get("share_before", 0) >= 0.5),
                       ("follow_through", lambda r: r["ft"] == "win")):
        dv = collections.defaultdict(list)
        for r in (rows if name == "follow_through" else hl):
            dv[r["day"]].append(1.0 if cond(r) else 0.0)
        out[name] = boot_share(dv)
    return out


_R = os.path.join("C:" + os.sep, "research", "stockwatcher")
CLOSES = os.environ.get("LAT_CLOSES", ",".join(
    os.path.join(_R, a, "daily-closes.json") for a in ("archive-hist", "archive-vps", "archive-confirm")))


def load_closes() -> dict[str, dict[str, float]]:
    out: dict[str, dict[str, float]] = {}
    for p in CLOSES.split(","):
        if os.path.exists(p):
            for d, m in json.load(open(p)).items():
                out.setdefault(d, {}).update(m)
    return out


def prev_close_map(closes: dict, day: str) -> dict[str, float]:
    ds = [d for d in closes if d < day]
    return closes[max(ds)] if ds else {}


def cmd_analyse(export: str, trigroots: str, outdir: str, qroot: str | None):
    os.makedirs(outdir, exist_ok=True)
    closes = load_closes()
    roots = trigroots.split(",")
    q = Quotes(qroot.split(",")) if qroot else None
    rows, fday, fday_s = [], {}, {}
    for day in days_of(export):
        fires = [json.loads(l) for l in open(os.path.join(export, day, "fires.jsonl"))]
        by = collections.defaultdict(list)
        for f in fires:
            by[f["s"]].append(f)
        trig = triggers_of(roots, day)
        pcm = prev_close_map(closes, day)
        fday[day] = fire_stats(fires, trig, False)
        fday_s[day] = fire_stats(fires, trig, True)
        for l in open(os.path.join(export, day, "alerts.jsonl")):
            a = json.loads(l)
            rows.append(measure(a, by.get(a["s"], []), q, day, pcm.get(a["s"])))
    with open(os.path.join(outdir, "rows.jsonl"), "w") as f:
        for r in rows:
            f.write(json.dumps(r) + "\n")
    res = {"days": len(fday), "all": table(rows)}
    for key, vals in (("path", PATHS), ("seg", SEGS), ("band", ["0.10-1", "1-5", "5-25"])):
        res["by_" + key] = {v: table([r for r in rows if r[key] == v]) for v in vals}
    # Market-wide causal flag burden.
    for tag, fd in (("flag", fday), ("flag_strict", fday_s)):
        tot = {k: sum(d[k] for d in fd.values()) for k in next(iter(fd.values()))}
        tot["per_day"] = tot["fires"] / len(fd)
        tot["followed30_share"] = tot["followed30"] / max(tot["fires"], 1)
        tot["before_and_followed30_share_of_before"] = tot["before_and_followed30"] / max(tot["before_first_alert"], 1)
        tot["per_day_dist"] = dist([d["fires"] for d in fd.values()])
        res["burden_" + tag] = tot
    # §10 opening prints: is the trigger the session's first print? Follow-through with/without.
    op = [r for r in rows if r["seg"] == "open_print"]
    res["open_print"] = {
        "n": len(op), "share_of_alerts": len(op) / max(len(rows), 1),
        "trigger_is_first_print": boot_share(_dv(op, lambda r: r["first"] is not None and r["first"][0] >= r["t0"] - 2 * SEC)),
        "n_before_dist": dist([r["n_before"] for r in op]),
        "by_path_n": dict(collections.Counter(r["path"] for r in op)),
        "follow_through": {},
    }
    for p in PATHS + ["all"]:
        sel = lambda rs: [r for r in rs if p == "all" or r["path"] == p]
        res["open_print"]["follow_through"][p] = {
            "with": boot_share(_dv(sel(rows), lambda r: r["ft"] == "win")),
            "without_open_print": boot_share(_dv(sel([r for r in rows if r["seg"] != "open_print"]), lambda r: r["ft"] == "win")),
            "open_print_only": boot_share(_dv(sel(op), lambda r: r["ft"] == "win")),
            "n_with": len(sel(rows)), "n_open": len(sel(op)),
        }
    json.dump(res, open(os.path.join(outdir, "lateness.json"), "w"), indent=1)
    open(os.path.join(outdir, "lateness.md"), "w", encoding="utf-8").write(render(res))
    print("wrote", os.path.join(outdir, "lateness.md"), len(rows), "alerts")


def _dv(rows, cond):
    dv = collections.defaultdict(list)
    for r in rows:
        dv[r["day"]].append(1.0 if cond(r) else 0.0)
    return dv


def fmt(d: dict, k: str, pct=True, scale=100) -> str:
    x = d.get(k, {})
    if not x or x.get("n", 0) == 0:
        return "—"
    s = scale if pct else 1
    return f"{x['med']*s:.1f} [{x['p25']*s:.1f}, {x['p75']*s:.1f}] ({x['p10']*s:.1f}/{x['p90']*s:.1f})"


def pct3(v):
    return "—" if not v else f"{v[0]*100:.1f}% [{v[1]*100:.1f}, {v[2]*100:.1f}]"


def render(res: dict) -> str:
    L = [f"# Runners lateness ({res['days']} days)\n",
         "Distributions: median [p25, p75] (p10/p90). Shares: point [95% day-bootstrap].\n",
         "## CAUSAL context at the alert (known at alert time)\n",
         "| group | n | rise from previous close % | rise from session low before alert % |", "|---|---|---|---|"]
    groups = [("all", res["all"])] + [(f"{k[3:]}={v}", t) for k in ("by_path", "by_seg", "by_band") for v, t in res[k].items()]
    for name, t in groups:
        L.append(f"| {name} | {t['n']} | {fmt(t,'gap_prev_close')} | {fmt(t,'day_rise_before')} |")
    L += ["\n## RETROSPECTIVE move position (hindsight; L* = low of 60 min before, P* = high of 60 min after)\n",
         "| group | n | rise before alert % | share of move before alert % | min L*→alert | rise after alert to P* % | ≥50% of move before | no new high in 60 min | day-level share before (sensitivity) % |",
         "|---|---|---|---|---|---|---|---|---|"]
    for name, t in groups:
        L.append(f"| {name} | {t['n']} | {fmt(t,'disp_before')} | {fmt(t,'share_before')} | {fmt(t,'min_L_to_alert',False)} | "
                 f"{fmt(t,'disp_after')} | {pct3(t['share_before_ge_50'])} | {pct3(t['no_new_high_60m'])} | {fmt(t,'day_share_before')} |")
    L += ["\n## RETROSPECTIVE ignition (first 3x range + 3x volume bar after L*)\n",
          "| group | ignition before alert | lead min (start→alert) | rise at ignition close % | share of move at ignition % |",
          "|---|---|---|---|---|"]
    for name, t in groups:
        L.append(f"| {name} | {pct3(t['ign_before_alert'])} | {fmt(t,'ign_lead_min',False)} | {fmt(t,'ign_disp')} | {fmt(t,'ign_share')} |")
    L += ["\n## CAUSAL early flag (5x vol, +3% close, ≥5 trades, completed 1-min bar) inside [L*, alert]\n",
          "| group | alerts with a flag in the move | lead min | rise at flag % | share of move at flag % | strict (median>0): with flag | strict lead min |",
          "|---|---|---|---|---|---|---|"]
    for name, t in groups:
        L.append(f"| {name} | {pct3(t['with_flag_in_move'])} | {fmt(t,'flag_lead_min',False)} | {fmt(t,'flag_disp')} | "
                 f"{fmt(t,'flag_share')} | {pct3(t['with_flag_strict_in_move'])} | {fmt(t,'flag_strict_lead_min',False)} |")
    L += ["\n## Causal flag burden (market-wide, $0.10–25, 04:30–15:15)\n",
          "| variant | fires/day (median [p25,p75]) | fires total | followed by a Runners alert ≤30 min | fires before the symbol's first alert | …of those followed ≤30 min |",
          "|---|---|---|---|---|---|"]
    for tag in ("flag", "flag_strict"):
        b = res["burden_" + tag]
        d = b["per_day_dist"]
        L.append(f"| {tag} | {d['med']:.0f} [{d['p25']:.0f}, {d['p75']:.0f}] | {b['fires']} | {b['followed30_share']*100:.1f}% | "
                 f"{b['before_first_alert']} | {b['before_and_followed30_share_of_before']*100:.1f}% |")
    L += ["\n## Liquidity at each stage (trades per 1-min bar; half-spread % where quotes exist)\n",
          "| group | trades @L* bar | @ignition | @flag | @last bar before alert | half-spread @L* | @ignition | @flag | @alert |",
          "|---|---|---|---|---|---|---|---|---|"]
    for name, t in groups:
        L.append(f"| {name} | {fmt(t,'trades_L_bar',False)} | {fmt(t,'trades_ign_bar',False)} | {fmt(t,'trades_flag_bar',False)} | "
                 f"{fmt(t,'trades_alert_bar',False)} | {fmt(t,'half_L')} | {fmt(t,'half_ign')} | {fmt(t,'half_flag')} | {fmt(t,'half_alert')} |")
    o = res["open_print"]
    L += ["\n## Opening prints (alerts 04:00:00–04:02:00 ET)\n",
          f"n = {o['n']} ({o['share_of_alerts']*100:.1f}% of first alerts); by path {o['by_path_n']}; "
          f"trigger second holds the session's first print: {pct3(o['trigger_is_first_print'])}; "
          f"OHLC prints before the alert: median {o['n_before_dist'].get('med', 0):.0f} (p90 {o['n_before_dist'].get('p90', 0):.0f}).\n",
          "Scorecard follow-through (+10% before −5%, 30 min, from the first print at/after the alert, no costs):\n",
          "| path | all first alerts | without open_print | open_print only | n all / n open |", "|---|---|---|---|---|"]
    for p, v in o["follow_through"].items():
        L.append(f"| {p} | {pct3(v['with'])} | {pct3(v['without_open_print'])} | {pct3(v['open_print_only'])} | {v['n_with']} / {v['n_open']} |")
    return "\n".join(L) + "\n"


# ---------------------------------------------------------------- gallery


def cmd_gallery(export: str, outdir: str, rowsfile: str):
    import matplotlib

    matplotlib.use("Agg")
    import matplotlib.pyplot as plt

    os.makedirs(outdir, exist_ok=True)
    rows = {(r["day"], r["s"], r["t0"]): r for r in map(json.loads, open(rowsfile))}
    rnd = random.Random(20261025)
    by = collections.defaultdict(list)
    for k, r in rows.items():
        if "L" in r:
            by[r["path"]].append(k)
    pick = []
    for p in PATHS:
        ks = sorted(by[p])
        pick += rnd.sample(ks, min(10, len(ks)))
    want = set(pick)
    index = []
    for day in sorted({k[0] for k in want}):
        for l in open(os.path.join(export, day, "alerts.jsonl")):
            a = json.loads(l)
            k = (day, a["s"], a["t0"])
            if k not in want:
                continue
            r = rows[k]
            bars = np.asarray(a["bars"], dtype=float)
            t04 = a["t04"]
            k0 = max(int((r["tL"] - t04) // MIN) - 15, 0)
            k1 = min(int((a["t0"] - t04) // MIN) + 61, len(bars))
            fig, (ax, av) = plt.subplots(2, 1, figsize=(10, 5.5), sharex=True,
                                         gridspec_kw={"height_ratios": [3, 1]})
            for k in range(k0, k1):
                o, h, lo, c, v, n = bars[k]
                if o <= 0:
                    continue
                col = "#2e9d6a" if c >= o else "#d0555b"
                ax.plot([k, k], [lo, h], color=col, lw=0.8)
                ax.add_patch(plt.Rectangle((k - 0.35, min(o, c)), 0.7, max(abs(c - o), 1e-9), color=col))
                av.bar(k, v, color=col, width=0.7)
            km = lambda t: (t - t04) / MIN
            marks = [("L*", r["tL"], r["L"], "#555"), ("alert", a["t0"], a["p0"], "#c78a00")]
            if "P" in r:
                marks.append(("P*", r["tP"], r["P"], "#2e9d6a"))
            if "ign_t" in r:
                marks.append(("ignition (retro)", r["ign_t"] + MIN / 2, None, "#7a5cc7"))
            if "flag_t" in r:
                marks.append(("causal flag", r["flag_t"] - MIN / 2, None, "#1f77b4"))
            for name, t, p, col in marks:
                ax.axvline(km(t), color=col, ls="--", lw=1, label=name)
                if p is not None:
                    ax.plot(km(t), p, "o", color=col, ms=5)
            tt = dt.datetime.fromtimestamp(a["t0"] / SEC, tz=ET).strftime("%H:%M:%S")
            ax.set_title(f"{day} {a['s']} — {a['path']} alert {tt} ET @ {a['p0']:.3f}; "
                         f"share of move before alert {r.get('share_before', float('nan'))*100:.0f}%")
            ax.legend(fontsize=7, loc="upper left")
            ticks = list(range(k0, k1, 15))
            av.set_xticks(ticks)
            av.set_xticklabels([dt.datetime.fromtimestamp((t04 + t * MIN) / SEC, tz=ET).strftime("%H:%M") for t in ticks])
            fig.tight_layout()
            name = f"{a['path']}_{day}_{a['s']}.png"
            fig.savefig(os.path.join(outdir, name), dpi=90)
            plt.close(fig)
            index.append({"file": name, **{k2: r.get(k2) for k2 in ("day", "s", "path", "seg", "share_before", "disp_before", "ign_lead_min", "flag_lead_min", "ft")}})
    json.dump(index, open(os.path.join(outdir, "index.json"), "w"), indent=1)
    print(len(index), "charts")


if __name__ == "__main__":
    cmd, *a = sys.argv[1:]
    if cmd == "analyse":
        cmd_analyse(a[0], a[1], a[2], a[3] if len(a) > 3 else None)
    elif cmd == "gallery":
        cmd_gallery(a[0], a[1], a[2])

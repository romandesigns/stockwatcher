"""RESEARCH ONLY (protocol §2 feed check): how well the quote feed covers the trades.

  python -I feed.py TRIGGERS SERIES_ROOT QUOTES_ROOT OUT_JSON

For every P1 alert window [t0 - 60 min, t0 + 46 min]: the share of OHLC-counted prints (weighted
per print, resolved to the second) that have a valid NBBO (bid > 0, ask > bid) updated within 1 s
before the end of their second; the share of quote updates that are locked or crossed (ask <= bid);
whether the fetched intervals cover the window. By session segment.
"""
from __future__ import annotations

import collections
import json
import os
import sys

import numpy as np

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import series  # noqa: E402
from pat import Quotes  # noqa: E402

SEC = 10**9
MIN = 60 * SEC


def main():
    trig, sroot, qroot, out = sys.argv[1:5]
    acc = collections.defaultdict(lambda: collections.Counter())
    cache = {}
    for l in open(trig):
        r = json.loads(l)
        if not r["is_first"]:
            continue
        day, sym, t0 = r["day"], r["sym"], r["t0"]
        S = series.load(sroot, day, sym)
        Q = Quotes(os.path.join(qroot, day, f"{sym}.npz"))
        lo, hi = t0 - 60 * MIN, t0 + 46 * MIN
        a = acc[r["segment"]]
        a["alerts"] += 1
        a["window_covered"] += bool(len(Q.t)) and Q.covered(lo, hi)
        a["no_quotes"] += len(Q.t) == 0
        s_lo, s_hi = (lo - S.t04) // SEC, (hi - S.t04) // SEC
        m = (S.ps >= s_lo) & (S.ps < s_hi)
        secs, n = S.ps[m], S.n[S.n > 0][m]
        a["prints"] += int(n.sum())
        if len(Q.t) and len(secs):
            ends = S.t04 + (secs + 1) * SEC
            i = np.searchsorted(Q.t, ends, side="right") - 1
            ok = i >= 0
            ic = np.clip(i, 0, len(Q.t) - 1)
            good = ok & Q.valid[ic] & (ends - Q.t[ic] <= SEC)
            a["prints_quoted_1s"] += int(n[good].sum())
            good10 = ok & Q.valid[ic] & (ends - Q.t[ic] <= 10 * SEC)
            a["prints_quoted_10s"] += int(n[good10].sum())
            qa, qb = np.searchsorted(Q.t, [lo, hi])
            a["quote_updates"] += int(qb - qa)
            a["locked_crossed"] += int((Q.ap[qa:qb] <= Q.bp[qa:qb]).sum())
            a["zero_side"] += int(((Q.ap[qa:qb] <= 0) | (Q.bp[qa:qb] <= 0)).sum())
    res = {}
    tot = collections.Counter()
    for sg, a in acc.items():
        tot.update(a)
        res[sg] = dict(a)
    res["all"] = dict(tot)
    for sg, a in res.items():
        p = max(1, a.get("prints", 0))
        u = max(1, a.get("quote_updates", 0))
        a["share_prints_quoted_1s"] = round(a.get("prints_quoted_1s", 0) / p, 4)
        a["share_prints_quoted_10s"] = round(a.get("prints_quoted_10s", 0) / p, 4)
        a["share_locked_crossed"] = round(a.get("locked_crossed", 0) / u, 4)
        a["share_window_covered"] = round(a.get("window_covered", 0) / max(1, a["alerts"]), 4)
    json.dump(res, open(out, "w"), indent=1)
    print(json.dumps(res["all"], indent=1))


if __name__ == "__main__":
    main()

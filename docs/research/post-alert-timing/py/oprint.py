"""RESEARCH ONLY (protocol §10): 04:00-04:02 opening-print alerts.

  python -I oprint.py TRIGGERS SERIES_ROOT OUT_JSON

For every P1 alert: whether the session's first OHLC print (>= 04:00) is within 60 s before t0
(an alert fired on the opening print); scorecard-style follow-through from the first print at or
after t0 (+10 % before -5 % within 30 min, no costs), per path, with and without open_print alerts.
"""
from __future__ import annotations

import collections
import json
import sys
import os

import numpy as np

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import series  # noqa: E402

SEC = 10**9


def follow(S, t0):
    s0 = (t0 - S.t04) // SEC
    i = int(np.searchsorted(S.ps, s0, side="left"))
    if i >= len(S.ps):
        return None
    p = S.po[i]
    j = int(np.searchsorted(S.ps, s0 + 1800, side="right"))
    ph, pl = S.ph[i + 1:j], S.pl[i + 1:j]
    ht = np.flatnonzero(ph >= 1.10 * p)
    hs = np.flatnonzero(pl <= 0.95 * p)
    a = ht[0] if len(ht) else 10**9
    b = hs[0] if len(hs) else 10**9
    return a < b


def main():
    trig, sroot, out = sys.argv[1:4]
    rows = []
    for l in open(trig):
        r = json.loads(l)
        if not r["is_first"]:
            continue
        S = series.load(sroot, r["day"], r["sym"])
        if len(S.ps) == 0:
            continue
        first_ns = S.t04 + int(S.ps[0]) * SEC
        r["on_open_print"] = bool(0 <= r["t0"] - first_ns <= 60 * SEC)
        r["ft"] = follow(S, r["t0"])
        rows.append(r)
    op = [r for r in rows if r["segment"] == "open_print"]
    res = {"p1_alerts": len(rows), "open_print_alerts": len(op),
           "open_print_share": round(len(op) / max(1, len(rows)), 4),
           "open_print_on_session_first_print": round(sum(r["on_open_print"] for r in op) / max(1, len(op)), 4),
           "open_print_paths": dict(collections.Counter(r["path"] for r in op)),
           "follow_through": {}}
    for path in sorted({r["path"] for r in rows}) + ["all"]:
        def ft(rs):
            v = [r["ft"] for r in rs if r["ft"] is not None and (path == "all" or r["path"] == path)]
            return {"n": len(v), "rate": round(sum(v) / len(v), 4) if v else None}
        res["follow_through"][path] = {"with_open_print": ft(rows),
                                       "without_open_print": ft([r for r in rows if r["segment"] != "open_print"]),
                                       "open_print_only": ft(op)}
    json.dump(res, open(out, "w"), indent=1)
    print(json.dumps(res, indent=1))


if __name__ == "__main__":
    main()

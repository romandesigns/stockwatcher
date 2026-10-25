"""RESEARCH ONLY (docs/research/post-alert-timing/protocol.md §3, §9): P1 first alerts per day.

  python -I lateness_alerts.py paa   PAA_ROOT... OUTDIR    # from the pullback export (work-paa/dev, test)
  python -I lateness_alerts.py pat   TRIGGERS_JSONL OUTDIR # from the sibling's triggers-*.jsonl

Writes OUTDIR/<day>.jsonl with {"s","t0","p0","path"}: the first Runners entry per symbol-day
after the per-path daily caps, band $0.10-25, alert time 04:00-15:15 ET (DST-aware).
"""

from __future__ import annotations

import collections
import datetime as dt
import json
import os
import sys
from zoneinfo import ZoneInfo

ET = ZoneInfo("America/New_York")
SEC = 10**9


def et_min(ns: int) -> float:
    t = dt.datetime.fromtimestamp(ns / SEC, tz=ET)
    return t.hour * 60 + t.minute + t.second / 60


def keep(a: dict) -> bool:
    return 0.10 <= a["p0"] <= 25.0 and 240 <= et_min(a["t0"]) <= 15 * 60 + 15


def from_paa(roots: list[str]) -> dict[str, list[dict]]:
    sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "..", "pullback-after-alert", "py"))
    import paa  # noqa: E402  (the prior study's reader: caps, first per symbol)

    out = collections.defaultdict(list)
    for root in roots:
        for day in paa.days_of(root):
            for t in paa.load_day(root, day):
                a = {"s": t["s"], "t0": t["t0"], "p0": t["p0"], "path": t["path"]}
                if keep(a):
                    out[day].append(a)
    return out


def from_pat(path: str) -> dict[str, list[dict]]:
    out = collections.defaultdict(list)
    for l in open(path):
        r = json.loads(l)
        a = {"s": r["s"], "t0": r["t0"], "p0": r["p0"], "path": r["path"]}
        if r.get("p1", True) and keep(a):
            out[r["day"]].append(a)
    return out


def main() -> None:
    mode, *args = sys.argv[1:]
    outdir = args[-1]
    os.makedirs(outdir, exist_ok=True)
    days = from_paa(args[:-1]) if mode == "paa" else from_pat(args[0])
    n = 0
    for day, al in sorted(days.items()):
        with open(os.path.join(outdir, f"{day}.jsonl"), "w") as f:
            for a in sorted(al, key=lambda a: (a["t0"], a["s"])):
                f.write(json.dumps(a) + "\n")
                n += 1
    print(f"{len(days)} days, {n} alerts")


if __name__ == "__main__":
    main()

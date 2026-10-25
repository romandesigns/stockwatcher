"""RESEARCH ONLY (docs/research/post-alert-timing/protocol.md §3): alert populations.

  python -I trig.py syms   TRIG_ROOT OUT_DIR          # per day: symbols needing a series export
  python -I trig.py build  TRIG_ROOT SERIES_ROOT OUT  # triggers-<set>.jsonl (P1 + P2 events)

TRIG_ROOT holds <day>/triggers.jsonl from paa.rs (the unchanged production Runners replay: first
entry per path per symbol). Daily caps (Running 150, Fast 200) are applied in time order exactly as
the pullback study did. Times are converted with tzdata (America/New_York, DST-aware).
"""
from __future__ import annotations

import collections
import datetime as dt
import json
import os
import sys
from zoneinfo import ZoneInfo

import numpy as np

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import series  # noqa: E402

NY = ZoneInfo("America/New_York")
SEC = 10**9
CAPS = {"running": 150, "fast": 200}
PATH_ORDER = ["burst", "run", "fast", "running"]
EVENT_GAP = 15 * 60 * SEC


def et_of(ns: int) -> dt.datetime:
    return dt.datetime.fromtimestamp(ns / SEC, NY)


def t_et(day: str, h: int, m: int, s: int = 0) -> int:
    y, mo, d = map(int, day.split("-"))
    return int(dt.datetime(y, mo, d, h, m, s, tzinfo=NY).timestamp()) * SEC


def segment(day: str, ns: int) -> str:
    if ns < t_et(day, 4, 2):
        return "open_print"
    if ns < t_et(day, 7, 0):
        return "early_pre"
    if ns < t_et(day, 9, 30):
        return "late_pre"
    if ns < t_et(day, 10, 0):
        return "mkt_open"
    return "rth"


def days_of(root: str) -> list[str]:
    return sorted(d for d in os.listdir(root) if os.path.exists(os.path.join(root, d, "triggers.jsonl")))


def entries_of(root: str, day: str) -> list[dict]:
    """Capped-in Runners entries in band ($0.10-25) and window (04:00-15:15 ET), time order."""
    trig = [json.loads(l) for l in open(os.path.join(root, day, "triggers.jsonl"))]
    trig.sort(key=lambda r: (r["t"], r["s"], PATH_ORDER.index(r["path"])))
    used = collections.Counter()
    out = []
    lo, hi = t_et(day, 4, 0), t_et(day, 15, 15)
    for r in trig:
        c = CAPS.get(r["path"])
        if c is not None:
            used[r["path"]] += 1
            if used[r["path"]] > c:
                continue
        if not (0.10 <= r["p"] <= 25.0) or not (lo <= r["t"] <= hi):
            continue
        out.append(r)
    return out


def cmd_syms(root: str, out: str) -> None:
    os.makedirs(out, exist_ok=True)
    tot = 0
    for day in days_of(root):
        syms = sorted({r["s"] for r in entries_of(root, day)})
        tot += len(syms)
        with open(os.path.join(out, f"{day}.txt"), "w") as f:
            f.write(",".join(syms))
    print("symbol-days", tot)


def cmd_build(root: str, sroot: str, out: str) -> None:
    n = collections.Counter()
    with open(out, "w") as w:
        for day in days_of(root):
            by = collections.defaultdict(list)
            for r in entries_of(root, day):
                by[r["s"]].append(r)
            for s, rs in by.items():
                S = series.load(sroot, day, s)
                ev = -1
                prev_t = None
                prev_ev_t = None  # first entry of the previous event
                for i, r in enumerate(rs):
                    t0 = r["t"]
                    new = prev_t is None or t0 - prev_t >= EVENT_GAP
                    etype = None
                    if new:
                        ev += 1
                        if ev == 0:
                            etype = "first"
                        else:
                            etype = series.event_type(S, day, prev_ev_t, t0)
                        prev_ev_t = t0
                    prev_t = t0
                    row = {"day": day, "sym": s, "path": r["path"], "t0": t0, "p0": r["p"],
                           "segment": segment(day, t0), "is_first": i == 0,
                           "event_id": f"{day}:{s}:{ev}", "event_start": new,
                           "event_type": etype if new else "repeat"}
                    w.write(json.dumps(row) + "\n")
                    n["entries"] += 1
                    n["first"] += i == 0
                    n["events"] += new
                    n[f"type:{row['event_type']}"] += 1
    print(json.dumps(n))


if __name__ == "__main__" and sys.argv[1] != "windows":
    {"syms": lambda: cmd_syms(sys.argv[2], sys.argv[3]),
     "build": lambda: cmd_build(sys.argv[2], sys.argv[3], sys.argv[4])}[sys.argv[1]]()


def cmd_windows(root: str, out: str, split: str) -> None:
    """Quote windows (§2): P1 [t0 - 60 min, t0 + 46 min], P2 event starts [t0, t0 + 46 min]."""
    import csv
    n = 0
    with open(out, "a", newline="") as f:
        w = csv.writer(f)
        if f.tell() == 0:
            w.writerow(["day", "sym", "lo", "hi", "split"])
        for day in days_of(root):
            prev = {}
            seen = set()
            for r in entries_of(root, day):
                s, t0 = r["s"], r["t"]
                if s not in seen:
                    w.writerow([day, s, t0 - 60 * 60 * SEC, t0 + 46 * 60 * SEC, split])
                    seen.add(s)
                    n += 1
                elif t0 - prev[s] >= EVENT_GAP:
                    w.writerow([day, s, t0, t0 + 46 * 60 * SEC, split])
                    n += 1
                prev[s] = t0
    print("windows", n)


if __name__ == "__main__" and sys.argv[1] == "windows":
    cmd_windows(sys.argv[2], sys.argv[3], sys.argv[4])

"""research/fast-0800, part 2: the 08:00:00-08:02:00 ET cluster's share of first Runners alerts and
its follow-through, by path and date range, with day-clustered bootstrap intervals.

Input: the post-alert timing study's per-first-alert rows (lateness rows.jsonl: day, s, path, t0,
ft = win/loss/timeout/noprint for +10 % before -5 % within 30 min, no costs).
Usage: python effect.py [--at=H] CUTOFF [CUTOFF ...]   (ISO dates; ranges are [prev, cutoff));
--at=4 measures the 04:00:00-04:02:00 window instead of 08:00 (the TRF opening moved there on
2026-03-30).
"""

import collections
import json
import random
import sys
from datetime import datetime
from zoneinfo import ZoneInfo

ET = ZoneInfo("America/New_York")
R = "C:/research/stockwatcher/reports/post-alert-timing/lateness"
B = 2000


AT = 8


def in_cluster(t0):
    t = datetime.fromtimestamp(t0 / 1e9, ET)
    return t.hour == AT and t.minute < 2


def rows():
    out = []
    for s in ("confirm", "explore"):
        for line in open(f"{R}/{s}/rows.jsonl"):
            r = json.loads(line)
            if r["ft"] == "noprint":
                continue
            r["c08"] = in_cluster(r["t0"])
            r["win"] = r["ft"] == "win"
            out.append(r)
    return out


def boot_rate(rs, rng):
    """Win rate with a day-clustered 95 % interval."""
    if not rs:
        return (float("nan"), float("nan"), float("nan"), 0)
    by = collections.defaultdict(list)
    for r in rs:
        by[r["day"]].append(r["win"])
    days = list(by)
    pt = sum(r["win"] for r in rs) / len(rs)
    bs = []
    for _ in range(B):
        s = [w for d in (rng.choice(days) for _ in days) for w in by[d]]
        bs.append(sum(s) / len(s))
    bs.sort()
    return (pt, bs[int(0.025 * B)], bs[int(0.975 * B)], len(rs))


def boot_diff(rs, rng):
    """Cluster minus other win rate, day-clustered."""
    by = collections.defaultdict(list)
    for r in rs:
        by[r["day"]].append(r)
    days = list(by)

    def diff(sample):
        a = [r["win"] for r in sample if r["c08"]]
        b = [r["win"] for r in sample if not r["c08"]]
        return (sum(a) / len(a) - sum(b) / len(b)) if a and b else None

    pt = diff(rs)
    bs = [x for x in (diff([r for d in (rng.choice(days) for _ in days) for r in by[d]]) for _ in range(B)) if x is not None]
    bs.sort()
    return pt, bs[int(0.025 * len(bs))], bs[int(0.975 * len(bs))]


def main():
    global AT
    args = sys.argv[1:]
    for a in [a for a in args if a.startswith("--at=")]:
        AT = int(a[5:])
        args.remove(a)
    cuts = args
    rs = rows()
    edges = ["0000-00-00", *cuts, "9999-99-99"]
    rng = random.Random(8)
    out = []
    for lo, hi in zip(edges, edges[1:]):
        part = [r for r in rs if lo <= r["day"] < hi]
        if not part:
            continue
        days = sorted({r["day"] for r in part})
        for path in ("all", "fast", "burst", "run", "running"):
            sub = part if path == "all" else [r for r in part if r["path"] == path]
            if not sub:
                continue
            c = [r for r in sub if r["c08"]]
            o = [r for r in sub if not r["c08"]]
            rc, ro = boot_rate(c, rng), boot_rate(o, rng)
            d = boot_diff(sub, rng) if c and o else (float("nan"),) * 3
            out.append({
                "range": f"{days[0]}..{days[-1]}", "days": len(days), "path": path, "n": len(sub),
                "cluster_n": len(c), "cluster_share": len(c) / len(sub),
                "cluster_win": rc[:3], "other_win": ro[:3], "diff": d,
            })
    for o in out:
        print(f"{o['range']} ({o['days']}d) {o['path']:8s} n={o['n']:5d} cluster={o['cluster_n']:4d} ({o['cluster_share']:.1%}) "
              f"win cluster {o['cluster_win'][0]:.1%} [{o['cluster_win'][1]:.1%},{o['cluster_win'][2]:.1%}] "
              f"other {o['other_win'][0]:.1%} [{o['other_win'][1]:.1%},{o['other_win'][2]:.1%}] "
              f"diff {o['diff'][0]*100:+.1f} pp [{o['diff'][1]*100:+.1f},{o['diff'][2]*100:+.1f}]")
    json.dump(out, open(f"C:/research/stockwatcher/reports/fast-0800/effect-{AT:02d}00.json", "w"), indent=1)


if __name__ == "__main__":
    main()

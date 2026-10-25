"""RESEARCH ONLY: download SIP quotes for the pullback-after-alert entry windows (copied from
docs/research/rocket-quotes, windows given explicitly as lo/hi instead of around an anchor).

usage: python -I fetch.py WINDOWS_CSV OUT_ROOT SPLIT [MAX_REQUESTS]

WINDOWS_CSV columns: day,sym,lo,hi,split (paa.py windows). For every (day, symbol) of SPLIT,
fetches the union of [lo, hi) and stores OUT_ROOT/<day>/<SYM>.npz
(aq.save; extra array `iv` = the fetched intervals). Appends one manifest line per file to
OUT_ROOT/manifest.jsonl (quotes, requests, wire bytes, stored bytes, seconds). Files already present
are skipped (resumable). 8 worker threads share aq's paced bucket. Stops cleanly after
MAX_REQUESTS (default unlimited).
"""

import csv
import json
import os
import sys
import threading
import time
from concurrent.futures import ThreadPoolExecutor

import numpy as np

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import aq  # noqa: E402

MIN = 60 * 10**9


def subtract(need, have):
    """Sub-intervals of `need` not covered by any interval of `have` (all half-open)."""
    out = []
    for lo, hi in need:
        cur = [(lo, hi)]
        for a, b in have:
            nxt = []
            for x, y in cur:
                if b <= x or a >= y:
                    nxt.append((x, y))
                    continue
                if x < a:
                    nxt.append((x, a))
                if b < y:
                    nxt.append((b, y))
            cur = nxt
        out.extend([x, y] for x, y in cur if y > x)
    return out


def main():
    wcsv, root, split = sys.argv[1], sys.argv[2], sys.argv[3]
    cap = int(sys.argv[4]) if len(sys.argv) > 4 else 10**12
    jobs = {}
    with open(wcsv) as f:
        for r in csv.DictReader(f):
            if r["split"] != split:
                continue
            jobs.setdefault((r["day"], r["sym"]), []).append((int(r["lo"]), int(r["hi"])))
    work = []
    for (day, sym), ivs in sorted(jobs.items()):
        ivs.sort()
        merged = []
        for lo, hi in ivs:
            if merged and lo <= merged[-1][1]:
                merged[-1][1] = max(merged[-1][1], hi)
            else:
                merged.append([lo, hi])
        work.append((day, sym, merged))
    man = open(os.path.join(root, "manifest.jsonl"), "a")
    lock = threading.Lock()
    tot = {"files": 0, "reqs": 0, "wire": 0, "stored": 0, "quotes": 0}
    t0 = time.time()
    stop = threading.Event()

    def one(job):
        day, sym, ivs = job
        if stop.is_set():
            return
        d = os.path.join(root, day)
        os.makedirs(d, exist_ok=True)
        path = os.path.join(d, f"{sym}.npz")
        parts, have = [], []
        if os.path.exists(path):
            old = aq.load(path)
            have = [tuple(x) for x in old.pop("iv").tolist()]
            if all(any(a <= lo and hi <= b for a, b in have) for lo, hi in ivs):
                return
            parts.append(old)
            # fetch only the parts not already stored (no quote is stored twice)
            ivs = subtract(ivs, have)
        t = time.time()
        reqs, wire = 0, 0
        for lo, hi in ivs:
            a, r, b = aq.fetch_quotes(sym, lo, hi)
            parts.append(a)
            reqs += r
            wire += b
        arr = {k: np.concatenate([p[k] for p in parts]) for k in parts[0]}
        order = np.argsort(arr["t"], kind="stable")
        arr = {k: v[order] for k, v in arr.items()}
        allv = sorted(have + [tuple(x) for x in ivs])
        arr["iv"] = np.asarray(allv, dtype=np.int64)
        stored = aq.save(path, arr)
        rec = {"day": day, "sym": sym, "intervals": len(ivs), "quotes": int(len(arr["t"])),
               "requests": reqs, "wire_bytes": wire, "stored_bytes": stored,
               "secs": round(time.time() - t, 2), "split": split}
        with lock:
            man.write(json.dumps(rec) + "\n")
            man.flush()
            tot["files"] += 1
            tot["reqs"] += reqs
            tot["wire"] += wire
            tot["stored"] += stored
            tot["quotes"] += rec["quotes"]
            if tot["reqs"] >= cap:
                stop.set()
            if tot["files"] % 100 == 0:
                el = time.time() - t0
                print(f"{tot['files']}/{len(work)} files, {tot['reqs']} req, "
                      f"{tot['quotes']/1e6:.1f}M quotes, wire {tot['wire']/1e6:.0f} MB, "
                      f"stored {tot['stored']/1e6:.0f} MB, {el/60:.1f} min", flush=True)

    with ThreadPoolExecutor(8) as ex:
        for _ in ex.map(one, work):
            pass
    with open(os.path.join(root, f"conditions-{split}.json"), "w") as f:
        json.dump(aq.COND, f)
    el = time.time() - t0
    print("DONE", json.dumps(tot), f"{el/60:.1f} min", flush=True)


if __name__ == "__main__":
    main()

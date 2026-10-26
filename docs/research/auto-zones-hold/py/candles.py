"""RESEARCH ONLY: production-rule candles from pat.rs per-second series (protocol §1)."""
from __future__ import annotations

import os
import sys

import numpy as np

sys.path.insert(0, r"H:\wavystack\stockwatcher\.claude\worktrees\timing\docs\research\post-alert-timing\py")
import series as ser  # noqa: E402

TFS = {"15s": 15, "30s": 30, "1m": 60, "5m": 300}
K = {"15s": 20, "30s": 20, "1m": 20, "5m": 12}


def candles(root: str, day: str, sym: str, tf: int):
    """[(t_ms, o, h, l, c)] for intervals with at least one OHLC-priced second, 04:00-20:00 ET."""
    path = os.path.join(root, day, f"{sym}.bin")
    if not os.path.exists(path):
        return None
    S = ser.load(root, day, sym)
    if len(S.ps) == 0:
        return []
    b = S.ps // tf
    out = []
    # Runs of equal bucket ids (ps is sorted).
    cut = np.flatnonzero(np.diff(b)) + 1
    starts = np.concatenate(([0], cut))
    ends = np.concatenate((cut, [len(b)]))
    t04_ms = S.t04 // 1_000_000
    for s, e in zip(starts, ends):
        out.append((int(t04_ms + int(b[s]) * tf * 1000), float(S.po[s]), float(S.ph[s:e].max()), float(S.pl[s:e].min()), float(S.pc[e - 1])))
    return out

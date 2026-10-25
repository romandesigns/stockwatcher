"""RESEARCH ONLY: reader for pat.rs per-second series (<root>/<day>/<SYM>.bin, 60-byte records)."""
from __future__ import annotations

import datetime as dt
import os
from zoneinfo import ZoneInfo

import numpy as np

NY = ZoneInfo("America/New_York")
SEC = 10**9
DT = np.dtype([("sec", "<u4"), ("o", "<f8"), ("h", "<f8"), ("l", "<f8"), ("c", "<f8"),
               ("n", "<u4"), ("v", "<f8"), ("pv", "<f8"), ("nv", "<u4")])


def t04(day: str) -> int:
    y, m, d = map(int, day.split("-"))
    return int(dt.datetime(y, m, d, 4, 0, tzinfo=NY).timestamp()) * SEC


class Series:
    def __init__(self, a: np.ndarray, day: str):
        self.day = day
        self.t04 = t04(day)
        self.sec = a["sec"].astype(np.int64)
        self.o, self.h, self.l, self.c = a["o"], a["h"], a["l"], a["c"]
        self.n, self.v, self.pv, self.nv = a["n"], a["v"], a["pv"], a["nv"]
        # OHLC-priced seconds only, for price scans
        m = self.n > 0
        self.ps = self.sec[m]
        self.ph, self.pl, self.pc, self.po = self.h[m], self.l[m], self.c[m], self.o[m]

    def s_of(self, ns: int) -> int:
        """Second index (since 04:00) containing ns."""
        return (ns - self.t04) // SEC

    def first_priced_at(self, s: int) -> int:
        """Index into ps of the first priced second >= s."""
        return int(np.searchsorted(self.ps, s, side="left"))

    def last_price_before(self, s: int) -> float:
        i = int(np.searchsorted(self.ps, s, side="left")) - 1
        return float(self.pc[i]) if i >= 0 else float("nan")


def load(root: str, day: str, sym: str) -> Series:
    a = np.fromfile(os.path.join(root, day, f"{sym}.bin"), dtype=DT)
    return Series(a, day)


def event_type(S: Series, day: str, prev_ev_t: int, t0: int) -> str:
    """Protocol §3 P2 event type, causal at t0: after_failed / after_consolidation / later_other."""
    a, b = S.s_of(prev_ev_t), S.s_of(t0)
    i0, i1 = S.first_priced_at(a), S.first_priced_at(b)
    if i1 > i0:
        hi = np.maximum.accumulate(S.ph[i0:i1])
        if np.any(S.pl[i0:i1] <= 0.90 * hi):
            return "after_failed"
    j0 = S.first_priced_at(b - 15 * 60)
    if i1 > j0:
        h, l = float(S.ph[j0:i1].max()), float(S.pl[j0:i1].min())
        if l > 0 and h / l <= 1.08:
            return "after_consolidation"
    return "later_other"

"""RESEARCH ONLY (docs/research/post-alert-timing/protocol.md §5-§6, §10): strategy engine.

  python -I pat.py run TRIGGERS SERIES_ROOT QUOTES_ROOT OUT_DIR [--lat 1] [--strats a,b] [--jobs 20]

For every P1 alert (is_first) and every P2 event start, runs the strategies of §6 under the
alert-anchored lifecycle of §5 and writes one row per (alert, strategy) to OUT_DIR/<day>.jsonl
(`pop` = "p1" or "p2"; a P1 alert is also its symbol-day's first P2 event and is written once per
population). Decisions are taken on second boundaries from completed seconds only; fills are at the
NBBO prevailing at decision + latency. Nothing here aggregates or selects; agg.py does.
"""
from __future__ import annotations

import datetime as dt
import json
import os
import sys
from multiprocessing import Pool

import numpy as np

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import series  # noqa: E402

SEC = 10**9
MIN = 60 * SEC
ENTRY_WIN = 15 * MIN
LIFE = 45 * MIN
QVALID = 10 * SEC
Q60 = 60 * SEC
SIZE_SHARES_FROM = "2025-11-03"
SANITY = os.environ.get("PAT_A1_ONLY") != "1"  # A2 stub-quote filter (PAT_A1_ONLY=1 reproduces A1 rows)  # Alpaca quote sizes: round lots before, shares from this date

DELAYS = {"D30s": 30, "D1": 60, "D2": 120, "D3": 180, "D5": 300, "D8": 480, "D10": 600,
          "D12": 720, "D15": 900}
ALL = (["B0"] + list(DELAYS) + ["RC", "PB", "S1_0.5", "S1_1.0", "S2_0.5", "S2_0.7",
                                 "L_S1_1.0", "L_S2_0.7"])


def qload(path: str) -> dict:
    """aq.load without importing aq (which reads credentials): t is stored as deltas."""
    with np.load(path) as z:
        d = {k: z[k] for k in z.files}
    d["t"] = np.cumsum(d["t"])
    return d


class Quotes:
    def __init__(self, path: str | None):
        if path is None or not os.path.exists(path):
            self.t = np.zeros(0, np.int64)
            self.iv = np.zeros((0, 2), np.int64)
            return
        d = qload(path)
        self.t = d["t"]
        self.bp = d["bp"].astype(np.float64) / 1e6
        self.ap = d["ap"].astype(np.float64) / 1e6
        self.bs = d["bs"]
        self.as_ = d["as"]
        self.iv = d["iv"]
        self.valid = (self.bp > 0) & (self.ap > self.bp)
        with np.errstate(divide="ignore", invalid="ignore"):
            self.hs = np.where(self.valid, (self.ap - self.bp) / (self.ap + self.bp), np.nan)
        self.vi = np.flatnonzero(self.valid)
        self.tv = self.t[self.vi]

    def apply_sanity(self, D: "Dense") -> None:
        """A2: a quote is also invalid when its half-spread is > 20 % of the mid, or its mid is
        > 25 % away from the last OHLC print of the preceding 60 s (stub / erroneous quotes)."""
        if len(self.t) == 0:
            return
        S = D.S
        qs = (self.t - S.t04) // SEC
        j = np.searchsorted(S.ps, qs, side="right") - 1
        has = (j >= 0)
        jc = np.clip(j, 0, max(len(S.ps) - 1, 0))
        recent = has & (qs - S.ps[jc] <= 60) if len(S.ps) else np.zeros(len(qs), bool)
        lastp = np.where(recent, S.pc[jc] if len(S.ps) else 0.0, np.nan)
        mid = (self.ap + self.bp) / 2
        with np.errstate(divide="ignore", invalid="ignore"):
            off = np.abs(mid / lastp - 1)
        sane = (np.nan_to_num(self.hs, nan=9.0) <= 0.20) & ~(recent & (off > 0.25))
        self.n_insane = int((self.valid & ~sane).sum())
        self.valid = self.valid & sane
        self.hs = np.where(self.valid, self.hs, np.nan)
        self.vi = np.flatnonzero(self.valid)
        self.tv = self.t[self.vi]

    def covered(self, lo: int, hi: int) -> bool:
        merged: list[list[int]] = []
        for a, b in sorted(self.iv.tolist()):
            if merged and a <= merged[-1][1]:
                merged[-1][1] = max(merged[-1][1], b)
            else:
                merged.append([a, b])
        return any(a <= lo and hi <= b for a, b in merged)

    def prevailing(self, ns: int) -> int | None:
        if len(self.t) == 0:
            return None
        i = int(np.searchsorted(self.t, ns, side="right")) - 1
        if i < 0 or ns - self.t[i] > QVALID or not self.valid[i]:
            return None
        return i

    def last_valid_within(self, ns: int, w: int) -> int | None:
        if len(self.tv) == 0:
            return None
        j = int(np.searchsorted(self.tv, ns, side="right")) - 1
        if j < 0 or ns - self.tv[j] > w:
            return None
        return int(self.vi[j])

    def med_hs(self, lo: int, hi: int) -> float:
        if len(self.tv) == 0:
            return float("nan")
        a, b = np.searchsorted(self.tv, [lo, hi])
        if b <= a:
            return float("nan")
        return float(np.median(self.hs[self.vi[a:b]]))


class Dense:
    """Per-second dense views of one symbol-day (index = seconds since 04:00)."""

    def __init__(self, S: series.Series):
        n = 16 * 3600 + 1
        self.S = S
        last = np.full(n, np.nan)
        last[S.ps] = S.pc
        # forward-fill the last OHLC price
        idx = np.where(~np.isnan(last), np.arange(n), 0)
        np.maximum.accumulate(idx, out=idx)
        self.last = last[idx]
        self.last[: (S.ps[0] if len(S.ps) else n)] = np.nan
        v = np.zeros(n)
        pv = np.zeros(n)
        nv = np.zeros(n)
        v[S.sec] = S.v
        pv[S.sec] = S.pv
        nv[S.sec] = S.nv
        # cumulative through second k inclusive -> index k+1
        self.cv = np.concatenate([[0], np.cumsum(v)])
        self.cpv = np.concatenate([[0], np.cumsum(pv)])
        self.cnv = np.concatenate([[0], np.cumsum(nv)])
        # 1-min bars (prices from OHLC prints, volume from volume-counted prints)
        nb = 16 * 60
        m = S.ps // 60
        self.bn = np.bincount(m, minlength=nb)[:nb]
        self.bh = np.full(nb, -np.inf)
        np.maximum.at(self.bh, m, S.ph)
        self.bl = np.full(nb, np.inf)
        np.minimum.at(self.bl, m, S.pl)
        self.bo = np.full(nb, np.nan)
        self.bc = np.full(nb, np.nan)
        # first/last priced second per minute
        first = np.ones(len(m), bool)
        first[1:] = m[1:] != m[:-1]
        lastm = np.ones(len(m), bool)
        lastm[:-1] = m[1:] != m[:-1]
        self.bo[m[first]] = S.po[first]
        self.bc[m[lastm]] = S.pc[lastm]
        bv = np.zeros(nb)
        np.add.at(bv, (S.sec // 60).clip(0, nb - 1), S.v)
        self.bv = bv
        # EMA-9 at each minute boundary m (bars ending <= boundary), as paa.rs
        self.ema9 = np.full(nb + 1, np.nan)
        closes, s9, e = 0, 0.0, np.nan
        for k in range(1, nb + 1):
            if self.bn[k - 1] > 0:
                closes += 1
                c = self.bc[k - 1]
                if closes <= 9:
                    s9 += c
                    if closes == 9:
                        e = s9 / 9.0
                else:
                    e = 0.2 * c + 0.8 * e
            self.ema9[k] = e
        # prefix minimum of per-second lows (NaN-free: inf where no print)
        low = np.full(n, np.inf)
        low[S.ps] = S.pl
        self.low = low
        high = np.full(n, -np.inf)
        high[S.ps] = S.ph
        self.high = high


def fill(Q: Quotes, ns: int, side: str, ref: float, lo: int, hi: int):
    """(price, mid, half-spread, kind) of a fill at `ns` (§5)."""
    i = Q.prevailing(ns)
    if i is not None:
        mid = (Q.ap[i] + Q.bp[i]) / 2
        px = Q.ap[i] if side == "buy" else Q.bp[i]
        return float(px), float(mid), float(Q.hs[i]), "q", i
    j = Q.last_valid_within(ns, Q60)
    if j is not None:
        h = float(Q.hs[j])
        kind = "q60"
    else:
        h = Q.med_hs(lo, hi)
        kind = "med"
    sgn = 1 if side == "buy" else -1
    return ref * (1 + sgn * h), ref, h, kind, None


def s2_qualify(Q: Quotes, t0: int, end: int, r: float) -> np.ndarray:
    """Boolean per quote update in Q: qualifies for S2(r) (3 consecutive updates <= r x ref,
    spanning >= 10 s). Reference = prevailing valid half-spread at t0, else the first valid update
    after t0 (counting starts after it)."""
    q = np.zeros(len(Q.t), bool)
    if len(Q.t) == 0:
        return q
    i0 = Q.prevailing(t0)
    if i0 is not None:
        ref = Q.hs[i0]
        start = int(np.searchsorted(Q.t, t0, side="right"))
    else:
        a = int(np.searchsorted(Q.tv, t0, side="left"))
        if a >= len(Q.tv) or Q.tv[a] > end:
            return q
        k = int(Q.vi[a])
        ref = Q.hs[k]
        start = k + 1
    stop = int(np.searchsorted(Q.t, end, side="right"))
    if stop <= start:
        return q
    ok = Q.valid[start:stop] & (np.nan_to_num(Q.hs[start:stop], nan=9.0) <= r * ref)
    grp = np.cumsum(~ok)
    idx = np.arange(len(ok))
    # first index of each ok-run
    firsts = np.zeros(len(ok), np.int64)
    run_first = -1
    prev_g = -1
    # vectorised: for ok positions, the run start is the first position with same grp and ok
    posok = np.flatnonzero(ok)
    if len(posok) == 0:
        return q
    g_ok = grp[posok]
    newrun = np.ones(len(posok), bool)
    newrun[1:] = g_ok[1:] != g_ok[:-1]
    starts = np.maximum.accumulate(np.where(newrun, np.arange(len(posok)), 0))
    first_pos = posok[starts]
    count = np.arange(len(posok)) - starts + 1
    tt = Q.t[start:stop]
    span = tt[posok] - tt[first_pos]
    good = (count >= 3) & (span >= 10 * SEC)
    q[start + posok[good]] = True
    return q


def pb_decision(D: Dense, t0: int, end: int) -> int | None:
    """Frozen C2_k2_d25-65_none (b9ac39a) on per-second data: decision ns or None."""
    S = D.S
    t04 = S.t04
    s0 = (t0 - t04) // SEC
    frm = max(s0 - 300, 0)
    m_first = s0 // 60
    m = m_first
    while t04 + m * MIN <= end:
        bnd = m * 60  # boundary second
        # H over [t0 - 5 min, boundary): latest second holding the max
        a, b = frm, bnd
        if b > a:
            hh = D.high[a:b]
            if np.isfinite(hh).any():
                h = hh.max()
                hs_ = a + int(np.flatnonzero(hh == h)[-1])
                ll = D.low[frm:s0]
                if np.isfinite(ll).any():
                    L = ll.min()
                else:
                    L = D.last[frm - 1] if frm > 0 else np.nan
                hb = hs_ // 60
                bars = [k for k in range(hb + 1, m) if D.bn[k] > 0]
                fm = frm // 60
                imp = [k for k in range(fm, hb + 1) if D.bn[k] > 0]
                if len(bars) >= 2 and np.isfinite(L) and h > L and imp:
                    lp = min(D.bl[k] for k in bars)
                    x = max(D.bh[k] for k in bars)
                    depth = (h - lp) / (h - L)
                    pbv = np.mean([D.bv[k] for k in bars])
                    impv = np.mean([D.bv[k] for k in imp])
                    if 0.25 <= depth <= 0.65 and pbv < impv:
                        bs = max(bnd, s0)
                        be = (m + 1) * 60
                        seg = D.high[bs:be]
                        hit = np.flatnonzero(seg > x)
                        if len(hit):
                            dec = t04 + (bs + int(hit[0]) + 1) * SEC
                            if dec <= end:
                                return dec
                            return None
        m += 1
    return None


def rc_decision(D: Dense, t0: int, end: int) -> int | None:
    S = D.S
    t04 = S.t04
    m = (t0 - t04) // MIN
    while t04 + (m + 1) * MIN <= end:
        if D.bn[m] > 0 and D.bc[m] < D.bo[m]:
            bs = (m + 1) * 60
            be = (end - t04) // SEC
            seg = D.high[bs:be]
            hit = np.flatnonzero(seg > D.bh[m])
            if len(hit):
                dec = t04 + (bs + int(hit[0]) + 1) * SEC
                return dec if dec <= end else None
            return None
        m += 1
    return None


def dyn_decisions(D: Dense, Q: Quotes, t0: int, end: int) -> dict[str, int | None]:
    """S1/S2/L decisions, evaluated at each second boundary t0 + k s, k = 0..900."""
    t04 = D.S.t04
    ks = np.arange(0, ENTRY_WIN // SEC + 1)
    ns = t0 + ks * SEC
    out = {}
    if len(Q.t) == 0:
        return {k: None for k in ["S1_0.5", "S1_1.0", "S2_0.5", "S2_0.7", "L_S1_1.0", "L_S2_0.7"]}
    pi = np.searchsorted(Q.t, ns, side="right") - 1
    okp = (pi >= 0)
    pic = np.clip(pi, 0, len(Q.t) - 1)
    age_ok = okp & ((ns - Q.t[pic]) <= QVALID) & Q.valid[pic]
    hs_now = np.where(age_ok, Q.hs[pic], np.nan)
    s1 = {h: age_ok & (np.nan_to_num(hs_now, nan=9.0) <= h / 100) for h in (0.5, 1.0)}
    s2 = {}
    for r in (0.5, 0.7):
        qq = s2_qualify(Q, t0, end, r)
        s2[r] = age_ok & qq[pic]
    # structure + liquidity at each boundary: completed seconds < sb
    sb = (ns - t04) // SEC  # seconds strictly before sb are complete
    sbc = np.clip(sb, 1, len(D.last) - 1)
    lastp = D.last[sbc - 1]
    vw = np.where(D.cv[sbc] > 0, D.cpv[sbc] / np.maximum(D.cv[sbc], 1e-12), np.nan)
    mb = sbc // 60  # boundary minute index: bars < mb complete
    ema = D.ema9[np.clip(mb, 0, len(D.ema9) - 1)]
    s0 = (t0 - t04) // SEC
    # lowest low since t0 (seconds s0..sb-1), and last completed 1-min bar low
    lows_since = np.minimum.accumulate(D.low[s0: s0 + len(ks) + 1])
    lmin = np.array([lows_since[max(0, int(b - s0) - 1)] if b > s0 else np.inf for b in sb])
    lastbar = np.full(len(ks), np.nan)
    for i, b in enumerate(mb):
        k = int(b) - 1
        while k >= 0 and D.bn[k] == 0:
            k -= 1
        lastbar[i] = D.bl[k] if k >= 0 else np.nan
    struct = (lastp >= vw) & (lastp >= ema) & (lastbar >= lmin)
    tr60 = D.cnv[sbc] - D.cnv[np.clip(sbc - 60, 0, None)]
    usd60 = D.cpv[sbc] - D.cpv[np.clip(sbc - 60, 0, None)]
    liq = (tr60 >= 5) & (usd60 >= 10_000)

    def first(mask):
        w = np.flatnonzero(mask)
        return int(ns[w[0]]) if len(w) else None

    out["S1_0.5"] = first(s1[0.5])
    out["S1_1.0"] = first(s1[1.0])
    out["S2_0.5"] = first(s2[0.5])
    out["S2_0.7"] = first(s2[0.7])
    out["L_S1_1.0"] = first(s1[1.0] & struct & liq)
    out["L_S2_0.7"] = first(s2[0.7] & struct & liq)
    return out


def trade(D: Dense, Q: Quotes, t0: int, dec: int, lat: int, day: str) -> dict:
    """Fill at dec + lat, bracket +10 % / -5 % on prints, end at t0 + 45 min (§5)."""
    S = D.S
    t04 = S.t04
    lo_w, hi_w = t0 - 60 * MIN, t0 + 46 * MIN
    fns = dec + lat * SEC
    fs = (fns - t04) // SEC
    ref = D.last[min(max(fs - 1, 0), len(D.last) - 1)]
    if not np.isfinite(ref):
        ref = np.nan
    epx, emid, ehs, ekind, ei = fill(Q, fns, "buy", ref, lo_w, hi_w)
    endns = t0 + LIFE
    es = (endns - t04) // SEC
    out = {"te": int(fns), "epx": epx, "emid": emid, "ehs": ehs, "ekind": ekind}
    if ei is not None:
        sz = int(Q.as_[ei]) * (100 if day < SIZE_SHARES_FROM else 1)
        out["ask_usd"] = sz * epx
    if not np.isfinite(epx) or epx <= 0:
        out["bad"] = True
        return out
    tgt, stp = epx * 1.10, epx * 0.95
    a = int(np.searchsorted(S.ps, fs, side="right"))  # priced seconds strictly after fill second
    b = int(np.searchsorted(S.ps, es, side="left"))
    ph, pl, psec = S.ph[a:b], S.pl[a:b], S.ps[a:b]
    hit_t = np.flatnonzero(ph >= tgt)
    hit_s = np.flatnonzero(pl <= stp)
    it = int(hit_t[0]) if len(hit_t) else 10**9
    is_ = int(hit_s[0]) if len(hit_s) else 10**9
    ambig = it == is_ and it < 10**9
    if is_ <= it and is_ < 10**9:
        reason, k, level = "loss", is_, stp
    elif it < 10**9:
        reason, k, level = "win", it, tgt
    else:
        reason, k, level = "timeout", None, None
    if k is not None:
        xns = t04 + (int(psec[k]) + 1) * SEC + lat * SEC
        xref = level
        xsec = int(psec[k])
    else:
        xns = endns
        xref = D.last[min(es, len(D.last) - 1) - 1] if es > 0 else np.nan
        if not np.isfinite(xref):
            xref = epx
        xsec = es
    xpx, xmid, xhs, xkind, _ = fill(Q, xns, "sell", xref, lo_w, hi_w)
    out.update({"reason": reason, "ambig": bool(ambig), "xkind": xkind, "xhs": xhs,
                "net": xpx / epx - 1, "gross": xmid / emid - 1 if emid > 0 else np.nan,
                "hold_s": int((xns - fns) // SEC)})
    # halts: any >= 5 min gap without prints inside [fill, exit]
    ss = S.ps[a: int(np.searchsorted(S.ps, xsec, side="right"))]
    pts = np.concatenate([[fs], ss, [xsec]])
    out["halt"] = bool(np.any(np.diff(pts) >= 300))
    # forward and excursions (from entry mid)
    for mk in (5, 15, 30, 45):
        m_ns = t0 + mk * MIN
        if m_ns < fns:
            out[f"f{mk}"] = None
            continue
        p = D.last[min((m_ns - t04) // SEC, len(D.last) - 1) - 1]
        out[f"f{mk}"] = float(p / emid - 1) if np.isfinite(p) and emid > 0 else None
    if b > a:
        out["mfe"] = float(ph.max() / emid - 1)
        out["mae"] = float(pl.min() / emid - 1)
    else:
        out["mfe"] = out["mae"] = 0.0
    return out


def run_day(args):
    day, alerts, sroot, qroot, out, lat, strats = args
    path = os.path.join(out, f"{day}.jsonl")
    if os.path.exists(path):
        return day, -1
    rows = []
    bysym = {}
    for a in alerts:
        bysym.setdefault(a["sym"], []).append(a)
    for sym, al in bysym.items():
        S = series.load(sroot, day, sym)
        if len(S.ps) == 0:
            continue
        D = Dense(S)
        Q = Quotes(os.path.join(qroot, day, f"{sym}.npz"))
        if SANITY:
            Q.apply_sanity(D)
        for a in al:
            t0 = a["t0"]
            end = t0 + ENTRY_WIN
            base = {k: a[k] for k in ("day", "sym", "path", "t0", "p0", "segment", "event_id",
                                      "event_type")}
            base["pops"] = a["pops"]
            base["quoted"] = bool(np.isfinite(Q.med_hs(t0 - 60 * MIN, t0 + 46 * MIN)))
            base["qcov"] = bool(Q.covered(t0 - (60 * MIN if "p1" in a["pops"] else 0), t0 + 46 * MIN)) if len(Q.iv) else False
            decs = {"B0": t0}
            for k, d in DELAYS.items():
                decs[k] = t0 + d * SEC
            want = set(strats)
            if "RC" in want:
                decs["RC"] = rc_decision(D, t0, end)
            if "PB" in want:
                decs["PB"] = pb_decision(D, t0, end)
            if want & {"S1_0.5", "S1_1.0", "S2_0.5", "S2_0.7", "L_S1_1.0", "L_S2_0.7"}:
                decs.update(dyn_decisions(D, Q, t0, end))
            for st in strats:
                dec = decs.get(st)
                r = dict(base)
                r["strat"] = st
                if dec is None:
                    r["entered"] = False
                else:
                    r["entered"] = True
                    r["delay_s"] = (dec - t0) / SEC
                    r.update(trade(D, Q, t0, dec, lat, day))
                rows.append(r)
    tmp = path + ".tmp"
    with open(tmp, "w") as f:
        for r in rows:
            f.write(json.dumps(r, default=float) + "\n")
    os.replace(tmp, path)
    return day, len(rows)


def main():
    a = sys.argv
    trig, sroot, qroot, out = a[2], a[3], a[4], a[5]
    lat = int(a[a.index("--lat") + 1]) if "--lat" in a else 1
    strats = a[a.index("--strats") + 1].split(",") if "--strats" in a else ALL
    jobs = int(a[a.index("--jobs") + 1]) if "--jobs" in a else 20
    only = a[a.index("--days") + 1].split(",") if "--days" in a else None
    os.makedirs(out, exist_ok=True)
    byday: dict[str, list] = {}
    for l in open(trig):
        r = json.loads(l)
        pops = []
        if r["is_first"]:
            pops.append("p1")
        if r["event_start"]:
            pops.append("p2")
        if not pops:
            continue
        r["pops"] = pops
        byday.setdefault(r["day"], []).append(r)
    days = sorted(byday) if only is None else only
    work = [(d, byday[d], sroot, qroot, out, lat, strats) for d in days]
    with Pool(jobs) as p:
        for d, n in p.imap_unordered(run_day, work):
            print(d, n, flush=True)


if __name__ == "__main__":
    main()

"""RESEARCH ONLY: the shipped automatic supply/demand zone rule (packages/chart-core/src/autoZones.ts,
commit 991ff69) ported line for line, plus the protocol's causal formation, test/hold outcome and
matched random-zone baseline. Parity with the TypeScript function: parity.ts + test_parity.py."""
from __future__ import annotations

import math
import random

R = dict(ATR_N=14, BASE_MAX=5, BASE_RANGE_ATR=1.0, LEG_MAX=3, LEG_ATR=2.0, DECISIVE_BODY=0.6, PER_SIDE=3)


def _body(c):
    return abs(c[4] - c[1])


def _range(c):
    return c[2] - c[3]


def _tr(cs):
    out = []
    for i, c in enumerate(cs):
        if i == 0:
            out.append(_range(c))
        else:
            pc = cs[i - 1][4]
            out.append(max(_range(c), abs(c[2] - pc), abs(c[3] - pc)))
    return out


def scan(cs):
    """The rule's `found` list on candles cs = [(t, o, h, l, c), ...]: dicts with the zone, the
    departure's leg end, distal edge, base start index, the first leg candle where the reach reached
    LEG_ATR x ATR (formation), and the ATR."""
    n = len(cs)
    if n < R["ATR_N"] + 3:
        return []
    tr = _tr(cs)
    # Trailing sums so atr_before(i) is O(1); identical arithmetic order is not needed for the
    # comparisons below beyond float noise, so it is recomputed exactly where it decides.
    def atr_before(i):
        if i < R["ATR_N"]:
            return math.nan
        s = 0.0
        for k in range(i - R["ATR_N"], i):
            s += tr[k]
        return s / R["ATR_N"]

    def quiet(c, atr):
        return _range(c) > 0 and _body(c) <= _range(c) * 0.5 and _range(c) <= R["BASE_RANGE_ATR"] * atr

    found = []
    i = R["ATR_N"]
    while i < n - 1:
        atr = atr_before(i)
        if not (atr > 0) or not quiet(cs[i], atr):
            i += 1
            continue
        j = i
        while j + 1 < n and j - i + 1 < R["BASE_MAX"] and quiet(cs[j + 1], atr):
            j += 1
        base = cs[i : j + 1]
        base_high = max(c[2] for c in base)
        base_low = min(c[3] for c in base)
        body_top = max(max(c[1], c[4]) for c in base)
        body_bottom = min(min(c[1], c[4]) for c in base)
        first = cs[j + 1] if j + 1 < n else None
        made = False
        if first is not None and _body(first) >= R["DECISIVE_BODY"] * _range(first):
            up = first[4] > base_high
            down = first[4] < base_low
            if up or down:
                reach = 0.0
                end = j + 1
                formed = None
                for k in range(j + 1, min(n, j + 1 + R["LEG_MAX"])):
                    d = cs[k][2] - base_high if up else base_low - cs[k][3]
                    if d > reach:
                        reach = d
                        end = k
                    if formed is None and reach >= R["LEG_ATR"] * atr:
                        formed = k
                if reach >= R["LEG_ATR"] * atr:
                    if up:
                        zone = dict(kind="demand", lo=base_low, hi=body_top, t=base[0][0], tested=False)
                    else:
                        zone = dict(kind="supply", lo=body_bottom, hi=base_high, t=base[0][0], tested=False)
                    if zone["hi"] > zone["lo"]:
                        found.append(dict(zone=zone, leg_end=end, distal=base_low if up else base_high, base_i=i, formed=formed, atr=atr))
                    made = True
        i = j + 1 if made else i + 1
    return found


def auto_zones(cs):
    """autoZones(slots) exactly: active zones, nearest PER_SIDE per kind, sorted by t."""
    found = scan(cs)
    if not found:
        return []
    live = []
    for f in found:
        broken = False
        tested = False
        for k in range(f["leg_end"] + 1, len(cs)):
            c = cs[k]
            if (c[4] < f["distal"]) if f["zone"]["kind"] == "demand" else (c[4] > f["distal"]):
                broken = True
                break
            if (c[3] <= f["zone"]["hi"]) if f["zone"]["kind"] == "demand" else (c[2] >= f["zone"]["lo"]):
                tested = True
        if not broken:
            live.append(dict(f["zone"], tested=tested))
    kept = []
    for z in live:
        if not any(k["kind"] == z["kind"] and z["lo"] <= k["hi"] and z["hi"] >= k["lo"] for k in kept):
            kept.append(z)
    last = cs[-1][4]

    def dist(z):
        return min(abs(z["lo"] - last), abs(z["hi"] - last))

    out = []
    for kind in ("supply", "demand"):
        ks = [z for z in kept if z["kind"] == kind]
        ks.sort(key=dist)  # stable, like Array.prototype.sort in modern engines
        out += ks[: R["PER_SIDE"]]
    out.sort(key=lambda z: z["t"])
    return out


def atr_at(cs, tr, i):
    if i < R["ATR_N"]:
        return math.nan
    return sum(tr[i - R["ATR_N"] : i]) / R["ATR_N"]


def outcome(cs, kind, lo, hi, start, K, atr):
    """Protocol §3 from candle index `start` (the first candle after formation)."""
    test = None
    for k in range(start, len(cs)):
        c = cs[k]
        if (c[3] <= hi) if kind == "demand" else (c[2] >= lo):
            test = k
            break
    if test is None:
        return dict(tested=False)
    res = "unresolved"
    mfe = 0.0
    for k in range(test, min(len(cs), test + K)):
        c = cs[k]
        mfe = max(mfe, (c[2] - hi) if kind == "demand" else (lo - c[3]))
        if kind == "demand":
            if c[4] > hi:
                res = "hold"
                break
            if c[4] < lo:
                res = "break"
                break
        else:
            if c[4] < lo:
                res = "hold"
                break
            if c[4] > hi:
                res = "break"
                break
    return dict(tested=True, test_k=test, res=res, bounce_atr=(mfe / atr) if atr and atr > 0 else None)


def formed_zones(cs):
    """Every zone the rule forms, at its formation candle, with the overlap rule applied at
    formation (an older same-kind zone still unbroken at that candle and overlapping hides it)."""
    found = scan(cs)
    out = []
    for idx, f in enumerate(found):
        z = f["zone"]
        fi = f["formed"]
        hidden = False
        for g in found[:idx]:
            if g["zone"]["kind"] != z["kind"] or not (z["lo"] <= g["zone"]["hi"] and z["hi"] >= g["zone"]["lo"]):
                continue
            broken = False
            for k in range(g["leg_end"] + 1, fi + 1):
                c = cs[k]
                if (c[4] < g["distal"]) if g["zone"]["kind"] == "demand" else (c[4] > g["distal"]):
                    broken = True
                    break
            if not broken and g["formed"] <= fi:
                hidden = True
                break
        if not hidden:
            out.append(dict(kind=z["kind"], lo=z["lo"], hi=z["hi"], formed=fi, atr=f["atr"], base_i=f["base_i"]))
    return out


def random_twin(cs, tr, z, rng):
    """Protocol §4: same kind, height and distance as fractions of price, random formation candle."""
    pf = cs[z["formed"]][4]
    if pf <= 0:
        return None
    h = (z["hi"] - z["lo"]) / pf
    d = (pf - z["hi"]) / pf if z["kind"] == "demand" else (z["lo"] - pf) / pf
    lo_i, hi_i = R["ATR_N"], len(cs) - 2
    if hi_i < lo_i:
        return None
    r = rng.randint(lo_i, hi_i)
    p = cs[r][4]
    if z["kind"] == "demand":
        top = p * (1 - d)
        lo = top - p * h
        return dict(kind="demand", lo=lo, hi=top, formed=r, atr=atr_at(cs, tr, r))
    lo = p * (1 + d)
    return dict(kind="supply", lo=lo, hi=lo + p * h, formed=r, atr=atr_at(cs, tr, r))

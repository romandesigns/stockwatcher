"""RESEARCH ONLY (docs/research/rocket-quotes): Alpaca historical SIP quotes client.

Market-data GET only (`/v2/stocks/{sym}/quotes`); never an order or trading endpoint.
Credentials: ONLY ALPACA_API_KEY / ALPACA_API_SECRET are read from the repo's .env (the same
names swimport reads); their values are never printed, logged or written anywhere.

Pacing: a process-wide token bucket at <= 2,700 requests/min (budget 3,000/min; production's
nightly import shares the account's 10,000/min), and no request at all between 21:00 and 23:30
ET (that import's window): callers block until 23:30 ET.

Units: every day studied is after 2025-11-03, when Alpaca switched CTA/UTP quote sizes from
round lots to SHARES (rounded down to the symbol's round lot). bs/as are stored as returned.
"""

from __future__ import annotations

import datetime as dt
import gzip
import http.client
import json
import os
import ssl
import threading
import time
import urllib.parse

import numpy as np

ENV = os.environ.get("ROCKETQ_ENV", r"H:\wavystack\stockwatcher\.env")
HOST = "data.alpaca.markets"
RATE_PER_MIN = 2700.0


def _creds() -> tuple[str, str]:
    want = {"ALPACA_API_KEY", "ALPACA_API_SECRET"}
    got: dict[str, str] = {}
    with open(ENV, encoding="utf-8") as f:
        for line in f:
            line = line.strip()
            if not line or line.startswith("#") or "=" not in line:
                continue
            k, v = line.split("=", 1)
            k = k.strip()
            if k.startswith("export "):
                k = k[7:].strip()
            if k in want:
                got[k] = v.strip().strip('"').strip("'")
    if set(got) != want or not all(got.values()):
        raise SystemExit("ALPACA_API_KEY / ALPACA_API_SECRET missing from .env")
    return got["ALPACA_API_KEY"], got["ALPACA_API_SECRET"]


def et_now() -> dt.datetime:
    """Eastern time without tzdata: EDT (UTC-4) until 2026-11-01 06:00 UTC, else EST."""
    u = dt.datetime.now(dt.UTC)
    off = -4 if u < dt.datetime(2026, 11, 1, 6, tzinfo=dt.UTC) else -5
    return (u + dt.timedelta(hours=off)).replace(tzinfo=None)


def in_pause(t: dt.datetime | None = None) -> bool:
    t = t or et_now()
    m = t.hour * 60 + t.minute
    return 21 * 60 <= m < 23 * 60 + 30


class Bucket:
    def __init__(self, per_min: float):
        self.rate = per_min / 60.0
        self.tokens = 1.0
        self.last = time.monotonic()
        self.lock = threading.Lock()
        self.count = 0

    def take(self) -> None:
        while True:
            while in_pause():
                time.sleep(30)
            with self.lock:
                now = time.monotonic()
                self.tokens = min(5.0, self.tokens + (now - self.last) * self.rate)
                self.last = now
                if self.tokens >= 1.0:
                    self.tokens -= 1.0
                    self.count += 1
                    return
                wait = (1.0 - self.tokens) / self.rate
            time.sleep(wait)


BUCKET = Bucket(RATE_PER_MIN)
_KEY, _SECRET = _creds()
_local = threading.local()


def _conn() -> http.client.HTTPSConnection:
    c = getattr(_local, "c", None)
    if c is None:
        c = http.client.HTTPSConnection(HOST, timeout=60, context=ssl.create_default_context())
        _local.c = c
    return c


def get(path: str, params: dict) -> tuple[dict, int]:
    """One paced GET; returns (json, wire bytes). Retries 429/5xx/connection errors."""
    q = path + "?" + urllib.parse.urlencode(params)
    hdr = {
        "APCA-API-KEY-ID": _KEY,
        "APCA-API-SECRET-KEY": _SECRET,
        "Accept-Encoding": "gzip",
        "Accept": "application/json",
    }
    for attempt in range(8):
        BUCKET.take()
        try:
            c = _conn()
            c.request("GET", q, headers=hdr)
            r = c.getresponse()
            body = r.read()
            if r.status == 200:
                n = len(body)
                if r.getheader("Content-Encoding") == "gzip":
                    body = gzip.decompress(body)
                return json.loads(body), n
            if r.status in (429, 500, 502, 503, 504):
                time.sleep(min(60, 2 ** attempt))
                continue
            raise RuntimeError(f"HTTP {r.status} on {path}: {body[:300]!r}")
        except (http.client.HTTPException, OSError):
            _local.c = None
            time.sleep(min(60, 2 ** attempt))
    raise RuntimeError(f"gave up on {path}")


def rfc3339(ns: int) -> str:
    s, frac = divmod(ns, 1_000_000_000)
    return dt.datetime.fromtimestamp(s, dt.UTC).strftime("%Y-%m-%dT%H:%M:%S") + f".{frac:09d}Z"


_sec_cache: dict[str, int] = {}


def parse_ns(t: str) -> int:
    head = t[:19]
    base = _sec_cache.get(head)
    if base is None:
        base = int(dt.datetime.strptime(head, "%Y-%m-%dT%H:%M:%S").replace(tzinfo=dt.UTC).timestamp()) * 1_000_000_000
        if len(_sec_cache) > 200_000:
            _sec_cache.clear()
        _sec_cache[head] = base
    rest = t[19:]
    if rest.startswith("."):
        digits = rest[1:].rstrip("Z")
        if not digits.isdigit():
            raise ValueError(t)
        return base + int(digits.ljust(9, "0")[:9])
    if rest not in ("Z", ""):
        raise ValueError(t)
    return base


EXCH = {c: i for i, c in enumerate("ABCDEHIJKLMNPQSTUVWXYZ", start=1)}
# Quote condition code -> bit: fixed (one-character codes A-Z, 0-9 in that order); any other
# code sets bit 63. COND records every code actually seen (for the manifest).
_CODES = "ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789"
COND: dict[str, int] = {}


def _cond_bits(cs) -> int:
    b = 0
    for c in cs or ():
        bit = _CODES.find(c) if len(c) == 1 else -1
        if bit < 0:
            bit = 63
        if c not in COND:
            COND[c] = bit
        b |= 1 << bit
    return b


def fetch_quotes(sym: str, start_ns: int, end_ns: int, raw_first: list | None = None):
    """All SIP quotes of `sym` with start <= t < end, ascending. Returns (arrays, requests, bytes)."""
    path = "/v2/stocks/" + urllib.parse.quote(sym, safe="") + "/quotes"
    params = {
        "start": rfc3339(start_ns),
        "end": rfc3339(end_ns - 1),
        "limit": 10000,
        "feed": "sip",
        "sort": "asc",
    }
    cols = {k: [] for k in ("t", "bp", "ap", "bs", "as", "bx", "ax", "c", "z")}
    reqs = byts = 0
    tok = None
    while True:
        p = dict(params)
        if tok:
            p["page_token"] = tok
        js, n = get(path, p)
        reqs += 1
        byts += n
        qs = js.get("quotes") or ()
        if raw_first is not None and not raw_first and qs:
            raw_first.extend(qs[:5])
        for q in qs:
            cols["t"].append(parse_ns(q["t"]))
            cols["bp"].append(round(q.get("bp", 0.0) * 1e6))
            cols["ap"].append(round(q.get("ap", 0.0) * 1e6))
            cols["bs"].append(q.get("bs", 0))
            cols["as"].append(q.get("as", 0))
            cols["bx"].append(EXCH.get(q.get("bx", ""), 0))
            cols["ax"].append(EXCH.get(q.get("ax", ""), 0))
            cols["c"].append(_cond_bits(q.get("c")))
            cols["z"].append(ord((q.get("z") or "?")[0]))
        tok = js.get("next_page_token")
        if not tok:
            break
    arr = {
        "t": np.asarray(cols["t"], dtype=np.int64),
        "bp": np.asarray(cols["bp"], dtype=np.int64),
        "ap": np.asarray(cols["ap"], dtype=np.int64),
        "bs": np.asarray(cols["bs"], dtype=np.int64),
        "as": np.asarray(cols["as"], dtype=np.int64),
        "bx": np.asarray(cols["bx"], dtype=np.uint8),
        "ax": np.asarray(cols["ax"], dtype=np.uint8),
        "c": np.asarray(cols["c"], dtype=np.uint64),
        "z": np.asarray(cols["z"], dtype=np.uint8),
    }
    return arr, reqs, byts


def save(path: str, arr: dict) -> int:
    """Compact store (npz, zlib): t as deltas whose first element is the absolute time."""
    out = dict(arr)
    out["t"] = np.diff(arr["t"], prepend=np.int64(0))
    tmp = path + ".tmp.npz"
    np.savez_compressed(tmp, **out)
    os.replace(tmp, path)
    return os.path.getsize(path)


def load(path: str) -> dict:
    with np.load(path) as z:
        d = {k: z[k] for k in z.files}
    d["t"] = np.cumsum(d["t"])
    return d

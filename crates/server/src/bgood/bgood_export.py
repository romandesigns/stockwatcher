"""Regenerates `b-good-1.xgb.txt`, `b-good-2.xgb.txt` and their parity vectors (`xgb.rs`).

NOT run by the build: the files are committed. Kept so their provenance is checkable.

    CUDA_VISIBLE_DEVICES= python -I bgood_export.py FINALIST_DIR MODEL_DIR B_GOOD_OUT OUT_DIR

- FINALIST_DIR: `C:\\wavystack\\outputs\\stockwatcher-research-v2\\30-w2\\finalists` (the frozen
  `b-good-1.json` / `b-good-2.json`: feature order, θ, model sha256).
- MODEL_DIR: `E:\\sw-derived\\v2w2-b-good\\finalists` (the frozen `b-good-N.pkl`, research branch
  `research/v2-b-good` @ ef61aeb6, `finalize.py` / `finalize_u2.py freeze`). Each pickle is
  `{cfg, cols?, model: Booster.save_raw("ubj"), iso, rule?}`; its sha256 is checked against the
  finalist record before anything is read.
- B_GOOD_OUT: `E:\\sw-derived\\v2w2-b-good` (`u1/<day>.parquet`, `u2s/<day>.parquet`: the
  research's own feature rows, Train days only).

CPU only, never a fit: the booster is loaded and asked to predict, nothing else. The GPU is not
touched (run with `CUDA_VISIBLE_DEVICES=` empty; the booster's `device` is forced to `cpu`).

# The text model

XGBoost's own JSON dump (`save_raw("json")`) holds, per tree, `left_children`, `right_children`,
`split_indices`, `split_conditions` and `default_left`; a node with `left_children == -1` is a leaf
and its value is its `split_conditions` entry. They are float32 in the booster, so each is written
as `repr(float(np.float32(x)))` (the shortest decimal that round-trips the float32) and parsed in
Rust with `str::parse::<f32>` (correctly rounded): every threshold and leaf is the frozen float32.
(serde_json is avoided for the same reason as in `r2_export.py`: without `float_roundtrip` it can be
one ulp off.)

`base_score` is stored in probability space (`[2.0058489E-1]`); the booster's starting margin is
`ProbToMargin` of it in float32. Rather than re-deriving XGBoost's float arithmetic for that, the
margin is RECOVERED from the booster itself: `predict(output_margin=True)` minus the float32 sum of
the walked leaves would mix orders, so instead the script tries the obvious candidate
`np.float32(-log(1/b - 1))` and its neighbours and keeps the one whose Rust-order sum (below)
reproduces XGBoost's margins bit for bit on every parity row; it refuses to write anything if none
does.

# θ in margin space

The research flags `predict() >= θ` (θ is itself a float32 prediction of a calibration row). The
server compares the float32 MARGIN with `theta_margin` instead, so the platform's `expf` (MSVC here,
glibc on the VPS) can never flip a flag. `theta_margin` = the smallest float32 m with
`sigmoid32(m) >= θ`, found by bisection over float32 bit patterns with numpy float32 arithmetic,
then CHECKED: `margin >= theta_margin` must equal XGBoost's own `predict() >= θ` on every parity row
(including the calibration row whose prediction is exactly θ when it is in the set).

# Parity vectors

`b-good-N.parity.csv`: feature rows (the model's column order, float32 repr, `nan` for missing)
with XGBoost's own `output_margin` and `predict()` per row; `xgb.rs` tests demand bit-identical
margins and identical flags. Rows: b-good-1 = every U1 row (all B0 alerts) of PARITY_DAYS_U1;
b-good-2 = every u2s sample row of PARITY_DAYS. All are Train days (2021-10-29..2024-06-28);
the script refuses any other day.
"""
import hashlib
import json
import os
import pickle
import sys

import numpy as np
import polars as pl
import xgboost as xgb

TRAIN = ("2021-10-29", "2024-06-28")
PARITY_DAYS = ["2024-06-11", "2024-06-13", "2024-06-14", "2024-06-18", "2024-06-20", "2024-06-24", "2024-06-25",
               "2024-06-26", "2024-06-27", "2024-06-28"]
# The committed b-good-1 vectors are kept small (two days); the full ten-day check runs outside the
# repo (50-ship/bgood/PARITY.md).
PARITY_DAYS_U1 = ["2024-06-27", "2024-06-28"]
PARITY_DAYS_U2 = ["2024-06-27", "2024-06-28"]


def sha(p):
    h = hashlib.sha256()
    with open(p, "rb") as f:
        for b in iter(lambda: f.read(1 << 20), b""):
            h.update(b)
    return h.hexdigest()


def f32r(x):
    x = np.float32(x)
    return "nan" if np.isnan(x) else repr(float(x))


def check_days(days):
    for d in days:
        if not (TRAIN[0] <= d <= TRAIN[1]):
            raise SystemExit(f"refused: {d} is not a Train day")


def load(fin_dir, model_dir, n):
    fz = json.load(open(os.path.join(fin_dir, f"b-good-{n}.json")))
    mp = os.path.join(model_dir, f"b-good-{n}.pkl")
    if sha(mp) != fz["model_sha256"]:
        raise SystemExit(f"{mp}: sha256 differs from the finalist record; refused")
    o = pickle.load(open(mp, "rb"))
    b = xgb.Booster()
    b.load_model(bytearray(o["model"]))
    b.set_param({"device": "cpu", "nthread": 1})
    return fz, o, b


def trees_of(b):
    j = json.loads(b.save_raw("json"))
    L = j["learner"]
    assert L["objective"]["name"] == "binary:logistic", L["objective"]
    g = L["gradient_booster"]
    assert g["name"] == "gbtree", g["name"]
    m = g["model"]
    assert m["gbtree_model_param"]["num_parallel_tree"] == "1"
    assert all(t == 0 for t in m["tree_info"])
    out = []
    for t in m["trees"]:
        assert all(s == 0 for s in t["split_type"]), "categorical split"
        out.append((np.array(t["left_children"], np.int64), np.array(t["right_children"], np.int64),
                    np.array(t["split_indices"], np.int64), np.array(t["split_conditions"], np.float32),
                    np.array(t["default_left"], np.int64)))
    bs = L["learner_model_param"]["base_score"].strip("[]")
    return out, float(bs), int(L["learner_model_param"]["num_feature"])


def walk_sum(trees, base, X):
    """The Rust evaluator's arithmetic in numpy float32: margin = base, then += each tree's leaf,
    in tree order (XGBoost's CPU predictor order)."""
    n = X.shape[0]
    acc = np.full(n, base, np.float32)
    for (lc, rc, si, sc, dl) in trees:
        node = np.zeros(n, np.int64)
        while True:
            leaf = lc[node] == -1
            if leaf.all():
                break
            x = X[np.arange(n), si[node]]
            go_left = np.where(np.isnan(x), dl[node] == 1, x < sc[node])
            nxt = np.where(go_left, lc[node], rc[node])
            node = np.where(leaf, node, nxt)
        acc = (acc + sc[node]).astype(np.float32)
    return acc


def sigmoid32(m):
    m = np.float32(m)
    with np.errstate(over="ignore"):
        return np.float32(1.0) / (np.float32(1.0) + np.exp(-m, dtype=np.float32))


def theta_margin(theta):
    """smallest float32 m with sigmoid32(m) >= theta (bisection over ordered float32 bits)."""
    def key(f):  # float32 -> monotone int
        i = np.array([f], np.float32).view(np.int32)[0]
        return int(i) if i >= 0 else -int(i & 0x7FFFFFFF)

    def unkey(k):
        i = k if k >= 0 else -(k) | 0x80000000
        return np.array([i], np.uint32).view(np.float32)[0]

    lo, hi = key(np.float32(-20)), key(np.float32(20))
    assert sigmoid32(unkey(lo)) < theta <= sigmoid32(unkey(hi))
    while hi - lo > 1:
        mid = (lo + hi) // 2
        if sigmoid32(unkey(mid)) >= theta:
            hi = mid
        else:
            lo = mid
    return np.float32(unkey(hi))


def rows_u1(out_dir, cols, days):
    """The research's own U1 table rows (C.load_u1 layout): u1 features joined to the B0 table."""
    sys.path.insert(0, r"E:\sw-wt\v2-b-good\research\v2\bgood")
    import common as C  # noqa: E402  (read-only import of the frozen research code)
    d = C.load_u1().filter(pl.col("day").is_in(days))
    return d, d.select(cols).to_numpy().astype(np.float32)


def rows_u2(out_dir, cols, days):
    s = pl.concat([pl.read_parquet(f"{out_dir}/u2s/{d}.parquet") for d in days], how="vertical_relaxed")
    sys.path.insert(0, r"E:\sw-wt\v2-b-good\research\v2\bgood")
    import common as C  # noqa: E402
    s = s.with_columns(C.ses_code_expr())  # u2.load_sample's rebuild of the overwritten feature
    return s, s.select(cols).to_numpy().astype(np.float32)


def export(fin_dir, model_dir, out_dir, dest, n):
    fz, o, b = load(fin_dir, model_dir, n)
    cols = fz["features"]
    if "cols" in o:
        assert list(o["cols"]) == cols, "pickle and finalist record disagree on the feature order"
    trees, bs, nf = trees_of(b)
    assert nf == len(cols)
    days = PARITY_DAYS_U1 if n == 1 else PARITY_DAYS_U2
    check_days(days)
    df, X = (rows_u1 if n == 1 else rows_u2)(out_dir, cols, days)
    dm = xgb.DMatrix(X, missing=np.nan)
    m_ref = b.predict(dm, output_margin=True).astype(np.float32)
    p_ref = b.predict(dm).astype(np.float32)
    cand = np.float32(-np.log(1.0 / bs - 1.0))
    base = None
    for k in range(-4, 5):
        c = np.nextafter(cand, np.float32(np.inf if k > 0 else -np.inf), dtype=np.float32) if k else cand
        for _ in range(abs(k) - 1):
            c = np.nextafter(c, np.float32(np.inf if k > 0 else -np.inf), dtype=np.float32)
        if np.array_equal(walk_sum(trees, c, X).view(np.int32), m_ref.view(np.int32)):
            base = np.float32(c)
            break
    if base is None:
        raise SystemExit(f"b-good-{n}: no base margin reproduces XGBoost's margins bit for bit; refused")
    theta = float(fz["rule"]["theta"])
    tm = theta_margin(theta)
    # θ is a float64 holding a float32 prediction, so this is the research's own comparison
    # (`finalize.py apply`: `r >= rule["theta"]` on the float32 predictions).
    flags_ref = p_ref.astype(np.float64) >= theta
    flags_m = m_ref >= tm
    if not np.array_equal(flags_ref, flags_m):
        raise SystemExit(f"b-good-{n}: theta_margin disagrees with predict() >= theta on {int((flags_ref != flags_m).sum())} rows")
    p = os.path.join(dest, f"b-good-{n}.xgb.txt")
    with open(p, "w", newline="\n") as f:
        f.write("bgood-xgb 1\n")
        f.write(f"name b-good-{n}\n")
        f.write(f"finalist_sha256 {sha(os.path.join(fin_dir, f'b-good-{n}.json'))}\n")
        f.write(f"model_sha256 {fz['model_sha256']}\n")
        f.write(f"features {len(cols)} {' '.join(cols)}\n")
        f.write(f"base_margin {f32r(base)}\n")
        f.write(f"theta {repr(theta)}\n")
        f.write(f"theta_margin {f32r(tm)}\n")
        f.write(f"trees {len(trees)}\n")
        for (lc, rc, si, sc, dl) in trees:
            f.write(f"tree {len(lc)}\n")
            for i in range(len(lc)):
                f.write(f"{lc[i]} {rc[i]} {si[i]} {f32r(sc[i])} {dl[i]}\n")
    pp = os.path.join(dest, f"b-good-{n}.parity.csv")
    # b-good-1: every row. b-good-2: every 4th sample row plus every row with prob >= 0.1 (the
    # sample is 0.75 % GOOD and none of it clears θ, so this keeps the high-scoring rows while the
    # file stays small); the whole sample is checked outside the repo (50-ship/bgood/PARITY.md).
    keep = np.ones(X.shape[0], bool) if n == 1 else ((np.arange(X.shape[0]) % 4 == 0) | (p_ref >= 0.1))
    with open(pp, "w", newline="\n") as f:
        f.write(",".join(cols) + ",xgb_margin,xgb_prob\n")
        for i in np.flatnonzero(keep):
            f.write(",".join(f32r(v) for v in X[i]) + f",{f32r(m_ref[i])},{f32r(p_ref[i])}\n")
    print(f"b-good-{n}: {len(trees)} trees, base_margin {f32r(base)}, theta {theta}, theta_margin {f32r(tm)}, "
          f"{X.shape[0]} parity rows on {days}, flagged {int(flags_m.sum())}; {p} {sha(p)}")


if __name__ == "__main__":
    if os.environ.get("CUDA_VISIBLE_DEVICES", None) != "":
        raise SystemExit("set CUDA_VISIBLE_DEVICES= (empty): CPU only")
    fin_dir, model_dir, out_dir, dest = sys.argv[1:5]
    for n in (1, 2):
        export(fin_dir, model_dir, out_dir, dest, n)

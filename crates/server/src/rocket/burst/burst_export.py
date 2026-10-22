"""Writes burst_parity.csv: model-input rows and LightGBM's own scores for the burst model's
pure-Rust evaluator test (crates/server/src/rocket/burst/tests.rs `model_matches_lightgbm`).

Run with the research venv (lightgbm 4.7.0, pandas, numpy), on the research machine:

    C:\\research\\stockwatcher\\venv\\Scripts\\python.exe burst_export.py

Rows: the research's own TEST export (reports/burst/export-test/rows5-<day>.csv, gate rows),
read exactly as the research's common.load did (pandas, features as float32), 70 rows from each
of five TEST days spread over the day, plus up to 20 rows at or above the 20-a-day threshold per day. The score is
Booster(model_file=frozen V3-5).predict on the float32 matrix, as test.py computed it. Values are
written with repr() (shortest round trip), so Rust parses the same doubles.
"""
import os

import lightgbm as lgb
import numpy as np
import pandas as pd

ROOT = r"C:\research\stockwatcher\reports\burst"
MODEL = os.path.join(ROOT, "frozen", "V3-5.lgb.txt")
DAYS = ["2026-08-03", "2026-08-14", "2026-09-02", "2026-09-10", "2026-09-16"]
FEATS = ["ld5", "ld15", "ld30", "rd5", "rd15", "rd30", "rt15", "radv30", "ret5", "ret15", "ret30",
         "brk", "buy5", "buy15", "upr", "nhd15", "nsw15", "vex15", "blk15", "odd15", "asz15", "logp",
         "sess", "tod", "gain", "gap", "rvol", "dhi", "vwap", "idle30", "rng30", "ldd", "ep", "age"]
META = ["day", "sym", "t", "alert_ns", "sess_code", "bucket", "last", "pa", "gate", "b3", "ep_start"]
TAIL = ["range5_bp", "mfe", "mae", "c10", "c20", "c30", "t20"]
OUT = os.path.join(os.path.dirname(__file__), "burst_parity.csv")


def main():
    bst = lgb.Booster(model_file=MODEL)
    out = []
    for d in DAYS:
        x = pd.read_csv(os.path.join(ROOT, "export-test", f"rows5-{d}.csv"), header=None,
                        skiprows=1, names=META + FEATS + TAIL, dtype={"sym": str})
        x = x[x.gate == 1].reset_index(drop=True)
        for c in FEATS:
            x[c] = x[c].astype("float32")
        x["score"] = bst.predict(x[FEATS].to_numpy(np.float32))
        pick = set(np.linspace(0, len(x) - 1, 70).round().astype(int).tolist())
        hot = x.index[x.score >= 0.2334402054046066].tolist()
        pick |= set(hot[:: max(1, len(hot) // 20)][:20])
        out.append(x.loc[sorted(pick)])
    x = pd.concat(out, ignore_index=True)
    with open(OUT, "w", newline="\n") as f:
        f.write("day,sym,t," + ",".join(FEATS) + ",score\n")
        for _, r in x.iterrows():
            vals = ["" if np.isnan(r[c]) else repr(float(r[c])) for c in FEATS]
            f.write(f"{r.day},{r.sym},{r.t}," + ",".join(vals) + f",{repr(float(r.score))}\n")
    print(len(x), "rows ->", OUT)


if __name__ == "__main__":
    main()

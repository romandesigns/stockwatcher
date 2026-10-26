"""RESEARCH ONLY: protocol §2 causality check. For 50 sampled symbol-days per set and every
timeframe, each formed zone must be in the rule's found list run on the candles up to its formation
candle, and absent from the run one candle earlier. Prints the share that match."""
from __future__ import annotations

import json
import os
import random
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import azones as az  # noqa: E402
import candles as cnd  # noqa: E402

RS = "C:/research/stockwatcher"
for set_name, root in (("explore", f"{RS}/work-pat/series"), ("confirm", f"{RS}/work-pat/series-confirm")):
    trig = [json.loads(l) for l in open(f"{RS}/work-pat/triggers-{set_name}.jsonl") if '"is_first": true' in l]
    ok = tot = 0
    for r in random.Random(99).sample(trig, 50):
        for tf in cnd.TFS.values():
            cs = cnd.candles(root, r["day"], r["sym"], tf)
            if not cs:
                continue
            for z in az.formed_zones(cs):
                key = (z["kind"], z["lo"], z["hi"], z["base_i"])
                at = {(f["zone"]["kind"], f["zone"]["lo"], f["zone"]["hi"], f["base_i"]) for f in az.scan(cs[: z["formed"] + 1])}
                before = {(f["zone"]["kind"], f["zone"]["lo"], f["zone"]["hi"], f["base_i"]) for f in az.scan(cs[: z["formed"]])}
                tot += 1
                ok += key in at and key not in before
    print(f"{set_name}: {ok}/{tot} formed zones match a prefix run ({ok / max(1, tot):.4f})")

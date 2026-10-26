"""Predeclared paired comparison; reads only this run's outputs, writes JSON.

Gross price outcomes, not delivered notifications or executable returns. The
bootstrap samples complete days, preserving within-day dependence and pairing.
"""
from pathlib import Path
from collections import Counter
from datetime import datetime
from zoneinfo import ZoneInfo
import hashlib
import json
import random
import sys

UNKNOWN_OUTCOMES = {"censored", "retracted"}
ET = ZoneInfo("America/New_York")


def status(e):
    return (e.get("wick") or {}).get("status", "unknown")


def select(episodes):
    ordered = sorted(episodes, key=lambda e: (e["armed_at"], e["episode_id"]))
    baseline = ordered[0] if ordered else None
    candidate = next((e for e in ordered if status(e) != "flagged"), None)
    return baseline, candidate


def successful(e):
    return e is not None and e["outcome"] == "continuation"


def band(e):
    p = e.get("price_at_arm")
    if p is None:
        return "unknown"
    if 100_000 <= p < 150_000:
        return "0.10_to_below_0.15"
    if 150_000 <= p <= 25_000_000:
        return "0.15_to_25"
    return "outside_reported_bands"


def segment(e):
    t = datetime.fromtimestamp(e["armed_at"] / 1e9, ET)
    m = t.hour * 60 + t.minute
    return "04_00_to_07_00" if m < 420 else "07_00_to_09_30" if m < 570 else "09_30_to_10_00" if m < 600 else "10_00_onward"


def rate(successes, notices):
    return successes / notices if notices else None


def metrics(pairs, days):
    daily = {d: [0, 0, 0, 0] for d in days}
    counts = Counter()
    outcomes = Counter()
    classification = Counter()
    for day, b, c in pairs:
        counts["baseline_notices"] += 1
        counts["candidate_notices"] += c is not None
        counts["baseline_successes"] += successful(b)
        counts["candidate_successes"] += successful(c)
        counts["baseline_successes_discarded"] += status(b) == "flagged" and successful(b)
        counts["baseline_successes_lost_after_replacement"] += successful(b) and not successful(c)
        counts["replacements"] += c is not None and c["episode_id"] != b["episode_id"]
        counts["successful_replacements"] += c is not None and c["episode_id"] != b["episode_id"] and successful(c)
        counts["baseline_noncontinuations_removed"] += not successful(b) and (c is None or c["episode_id"] != b["episode_id"])
        counts["no_remaining_candidate"] += c is None
        classification[status(b)] += 1
        outcomes[b["outcome"]] += 1
        d = daily[day]
        d[0] += 1
        d[1] += successful(b)
        d[2] += c is not None
        d[3] += successful(c)
    bn, bs = counts["baseline_notices"], counts["baseline_successes"]
    cn, cs = counts["candidate_notices"], counts["candidate_successes"]
    br, cr = rate(bs, bn), rate(cs, cn)
    for key in ["baseline_notices", "candidate_notices", "baseline_successes", "candidate_successes",
                "baseline_successes_discarded", "baseline_successes_lost_after_replacement",
                "replacements", "successful_replacements", "baseline_noncontinuations_removed", "no_remaining_candidate"]:
        counts.setdefault(key, 0)
    rng = random.Random(20261026)
    cluster = list(daily.values())
    differences = []
    if cluster and pairs:
        for _ in range(5000):
            totals = [0, 0, 0, 0]
            for _ in cluster:
                d = cluster[rng.randrange(len(cluster))]
                for i in range(4):
                    totals[i] += d[i]
            if totals[0] and totals[2]:
                differences.append(totals[3] / totals[2] - totals[1] / totals[0])
    differences.sort()
    ci = [differences[int((len(differences) - 1) * q)] * 100 for q in [0.025, 0.975]] if differences else None
    return dict(counts) | {
        "days": len(days), "days_with_baseline_notices": sum(v[0] > 0 for v in daily.values()),
        "baseline_notices_per_day": bn / len(days) if days else None,
        "candidate_notices_per_day": cn / len(days) if days else None,
        "baseline_continuation_rate": br, "candidate_continuation_rate": cr,
        "difference_pp": (cr - br) * 100 if cr is not None and br is not None else None,
        "difference_pp_day_bootstrap_95pct": ci, "bootstrap_valid_resamples": len(differences),
        "notice_reduction": (bn - cn) / bn if bn else None,
        "baseline_classification": dict(classification), "baseline_outcomes": dict(outcomes),
    }


def self_test():
    def e(i, s, outcome="no_cross"):
        return {"episode_id": str(i), "armed_at": i, "wick": {"status": s}, "outcome": outcome}
    a, b, c = e(1, "flagged", "continuation"), e(2, "unknown"), e(3, "clear")
    assert select([c, b, a]) == (a, b)  # Unknown is retained, before spending allowance.
    assert select([a]) == (a, None)
    assert select([]) == (None, None)
    r = metrics([("d", a, b)], ["d"])
    assert r["baseline_successes_lost_after_replacement"] == 1
    assert r["candidate_notices"] == 1 and r["difference_pp"] == -100
    empty = metrics([], ["d"])
    assert empty["baseline_notices"] == 0 and empty["difference_pp"] is None


def main():
    self_test()
    root = Path(sys.argv[1])
    population_path = root / "population.json"
    population = json.loads(population_path.read_text())
    run = json.loads((root / (sys.argv[2] if len(sys.argv) > 2 else "run-receipt.json")).read_text())
    assert run["population_sha256"] == hashlib.sha256(population_path.read_bytes()).hexdigest()
    out = root / run["output"]
    completion = json.loads((out / "run-summary.json").read_text())
    assert completion["errors"] == 0 and completion["completed"] == len(population["jobs"])
    pairs, excluded = [], []
    counters = Counter()
    episode_status = Counter()
    no_episode = 0
    for job in population["jobs"]:
        data = json.loads((out / f"{job['day']}-{job['sym']}.json").read_text())
        assert "error" not in data, data
        assert data["semantic_mismatches"] == 0 and data["dropped_transitions"] == 0
        for k in ["events", "counted_prints", "receive_timestamps", "revisions", "compared_transitions"]:
            counters[k] += data[k]
        for e in data["episodes"]:
            episode_status[status(e)] += 1
        b, c = select(data["episodes"])
        if b is None:
            no_episode += 1
            continue
        if b["outcome"] in UNKNOWN_OUTCOMES or (c is not None and c["outcome"] in UNKNOWN_OUTCOMES):
            excluded.append({"day": job["day"], "sym": job["sym"], "baseline": b["outcome"], "candidate": c["outcome"] if c else None})
            continue
        pairs.append((job["day"], b, c))
    days = population["days"]
    half = len(days) // 2
    report = {
        "definition": "mp5-wick-v1 exploratory matched notice-opportunity replay",
        "population_sha256": run["population_sha256"], "rule": "mp5-wick-v1",
        "jobs": len(population["jobs"]), "unavailable_symbol_days": len(population["unavailable"]),
        "days": len(days), "from": days[0], "to": days[-1], "no_arming_in_context_window": no_episode,
        "paired_censored_or_retracted": excluded, "counts": dict(counters),
        "all_context_arming_classification": dict(episode_status), "overall": metrics(pairs, days),
        "bands_grouped_by_baseline_price_at_arm": {
            k: metrics([p for p in pairs if band(p[1]) == k], days)
            for k in ["0.10_to_below_0.15", "0.15_to_25", "unknown", "outside_reported_bands"]},
        "chronological_halves": {
            "first": metrics([p for p in pairs if p[0] in days[:half]], days[:half]),
            "second": metrics([p for p in pairs if p[0] in days[half:]], days[half:])},
        "session_segments": {k: metrics([p for p in pairs if segment(p[1]) == k], days)
                             for k in ["04_00_to_07_00", "07_00_to_09_30", "09_30_to_10_00", "10_00_onward"]},
        "blocking_filter_enabled": False,
        "limitations": ["EXPLORE only, not independent confirmation", "fixed historical Runners contexts, not new-discovery efficacy",
                        "Ross-only and device delivery policy excluded", "reconstructed admission and coverage", "gross price outcomes; costs unknown"]}
    (root / "comparison.json").write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print(json.dumps({k: report[k] for k in ["jobs", "days", "overall", "all_context_arming_classification"]}, indent=2))


if __name__ == "__main__":
    if len(sys.argv) > 1 and sys.argv[1] == "--self-test":
        self_test()
        print("Analysis selection and paired-loss checks passed")
    else:
        main()

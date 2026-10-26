"""Prepare a read-only historical population and the pre-observer control.

No outcomes are read. Never edits the archive, old studies or frozen worktrees.
The control keeps the current pillar policy/floor changes, and replaces only
the diagnostic's two core files and exports with their pre-observer versions.
"""
from pathlib import Path
from datetime import datetime
from zoneinfo import ZoneInfo
import hashlib
import json
import shutil
import subprocess
import sys

REPO = Path(__file__).resolve().parents[2]
TOOL = Path(__file__).resolve().parent
ROOT = Path("C:/research/stockwatcher")
OUT = Path(sys.argv[1])
BASE_REV = "a7a89b597388cf8e69c0e2d7f65bf8317546a6d4"
OUT.mkdir(parents=True, exist_ok=True)
baseline = TOOL / "baseline-stock-context"
if baseline.exists():
    raise SystemExit("Baseline exists; use the frozen snapshot, do not overwrite it")
shutil.copytree(REPO / "crates/stock-context/src", baseline / "src")
shutil.copytree(REPO / "crates/stock-context/policy", baseline / "policy")
for rel in ["src/detect/mod.rs", "src/detect/micro.rs", "src/lib.rs"]:
    data = subprocess.check_output(["git", "show", f"{BASE_REV}:crates/stock-context/{rel}"], cwd=REPO)
    (baseline / rel).write_bytes(data)
(baseline / "Cargo.toml").write_text('''[package]
name = "stock-context-baseline"
version = "0.1.0"
edition = "2024"
publish = false
[dependencies]
market-events = { path = "../../../crates/market-events" }
serde = { version = "1.0.229", features = ["derive"] }
serde_json = "1.0.154"
sha2 = "0.10.9"
chrono = { version = "0.4.45", default-features = false, features = ["std", "serde"] }
''', encoding="utf-8")
for rel in ["src/detect/mod.rs", "src/detect/micro.rs", "src/lib.rs"]:
    expected = subprocess.check_output(["git", "show", f"{BASE_REV}:crates/stock-context/{rel}"], cwd=REPO)
    assert (baseline / rel).read_bytes() == expected

export = ROOT / "work-pat/triggers-explore.jsonl"
first = {}
et = ZoneInfo("America/New_York")
for line in export.read_text(encoding="utf-8").splitlines():
    r = json.loads(line)
    if not "2026-03-23" <= r["day"] <= "2026-10-23":
        continue
    t = datetime.fromtimestamp(r["t0"] / 1e9, et)
    if not 240 <= t.hour * 60 + t.minute <= 915:
        continue
    if not 0.10 <= r["p0"] <= 25:
        continue
    key = (r["day"], r["sym"])
    first[key] = min(first.get(key, r["t0"]), r["t0"])

manifests = {}
manifest_hashes = {}
jobs, unavailable = [], []
for (day, sym), at in sorted(first.items()):
    found = None
    for folder in ["archive-hist", "archive-vps"]:
        for live in [False, True]:
            p = ROOT / folder / day
            if live:
                p = p / "live"
            mp = p / "manifest.json"
            if mp not in manifests:
                raw = mp.read_bytes() if mp.exists() else None
                manifests[mp] = json.loads(raw) if raw else None
                manifest_hashes[mp] = hashlib.sha256(raw).hexdigest() if raw else None
            m = manifests[mp]
            if not m or sym not in m.get("symbols", {}) or not (p / f"{sym}.swa").exists():
                continue
            if live and not m.get("live", {}).get("sealed", False):
                continue
            found = {"day": day, "sym": sym, "first_trigger_ns": at,
                     "archive": str(ROOT / folder), "live": live,
                     "bytes": (p / f"{sym}.swa").stat().st_size,
                     "manifest_sha256": manifest_hashes[mp]}
            break
        if found:
            break
    if found:
        jobs.append(found)
    else:
        unavailable.append({"day": day, "sym": sym, "reason": "no_replayable_symbol"})
jobs.sort(key=lambda j: (-j["bytes"], j["day"], j["sym"]))
frozen = {"protocol_sha256": hashlib.sha256((REPO / "docs/research/mp5-wick-v1-protocol.md").read_bytes()).hexdigest(),
          "historical_trigger_sha256": hashlib.sha256(export.read_bytes()).hexdigest(),
          "base_revision": BASE_REV, "jobs": jobs, "unavailable": unavailable,
          "days": sorted({d for d, _ in first}),
          "source_sha256": {str(p.relative_to(REPO)): hashlib.sha256(p.read_bytes()).hexdigest()
                            for p in (REPO / "crates/stock-context/src").rglob("*.rs")},
          "baseline_sha256": {str(p.relative_to(baseline)): hashlib.sha256(p.read_bytes()).hexdigest()
                              for p in baseline.rglob("*.rs")}}
(OUT / "population.json").write_text(json.dumps(frozen, indent=2) + "\n", encoding="utf-8")
print(json.dumps({"jobs": len(jobs), "days": len(frozen["days"]), "unavailable": len(unavailable)}))

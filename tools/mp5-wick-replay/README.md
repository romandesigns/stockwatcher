# MP5 wick diagnostic replay

Offline research only. See `docs/research/mp5-wick-v1-protocol.md` for the frozen
candidate, baseline, population, clock amendment and limits on interpretation.
No production crate depends on this package. It cannot push notifications or
place orders. Archives and earlier experiments are read only.

From the repo root, on the machine with the existing research archive copies:

```powershell
python tools/mp5-wick-replay/prepare.py H:/wavystack/outputs/mp5-wick-NEW
cargo build --manifest-path tools/mp5-wick-replay/Cargo.toml --release --offline
tools/mp5-wick-replay/target/release/mp5-wick-replay.exe H:/wavystack/outputs/mp5-wick-NEW/population.json H:/wavystack/outputs/mp5-wick-NEW/replay-v1 8
```

`prepare.py` creates an ignored control crate. Its `micro.rs`, detector record
definitions and exports come from `a7a89b5`, before the observer. The other
current engine/pillar files are retained so the control includes the same $0.10
floor changes. Each replay step compares the entire serialized transition with
the control after removing only `anchors.wick`; a mismatch is an error. Source
and input-manifest hashes record the precise snapshots used.

The initial recorded run used `CARGO_TARGET_DIR` pointing to an existing local
release cache. Its exact binary path/hash, protocol amendment hash and population
hash are in `outputs/mp5-wick-20261026/run-receipt.json`. A rerun must create an
equivalent receipt before `analyze.py`, whose default output is `replay-v1`:

```powershell
python tools/mp5-wick-replay/analyze.py --self-test
python tools/mp5-wick-replay/analyze.py H:/wavystack/outputs/mp5-wick-20261026 run-receipt-v2.json
```

The analysis refuses incomplete/error runs and preserves unknown/censored
outcomes. It simulates a wick-filtered first-notice allowance, not device
delivery limits, orders, costs or predictive profitability. Production keeps
the hard filter off; the candidate cannot be enabled merely by this analysis.

The corrected run is `replay-v2`/`run-receipt-v2.json`. A2 corrects an adapter
measurement bug: a bar closing back below the trigger records a retraction, but
does not censor the observed first-crossing price outcome. A source crossing
print that is actually revised is different and remains censored. The original
`replay-v1`, its receipt, and `comparison-v1-preaudit.json` are retained. See the
protocol amendment; the initial comparison was already read before this fix.

# Automatic supply / demand zones: how often do they hold? — protocol v1

Pre-registered 2026-10-26, committed before any zone outcome is computed. Research only: no
production change. Amendments go at the end (date, reason, commit), never motivated by a CONFIRM
result.

Question (owner, 2026-10-26): the Levels menu's "Supply / demand zones" layer (commit 991ff69,
`packages/chart-core/src/autoZones.ts`, `AUTO_ZONE_RULES` unchanged) is labelled untested. How
often does a zone hold when price comes back to it, compared with a zone placed at random? The
answer becomes the menu's hold-rate text, as for volume shelves.

## 1. Data

- Candles: per-second OHLC series exported by `pat.rs` with the chart engine's counting rules
  (`work-pat/series`, `series-confirm`), aggregated to 15 s / 30 s / 1 min / 5 min candles by the
  production rule: a candle exists for an interval with at least one OHLC-priced second; open = the
  first such second's open, high/low = extremes, close = the last second's close. Intervals without
  prices are gap slots, which `autoZones()` skips. 04:00–20:00 ET.
- **EXPLORE** = 149 days (archive-hist + archive-vps, 2026-03-23..10-23). Descriptive.
- **CONFIRM** = 118 days (archive-past, 2025-10-01..2026-03-20). Scored once, after this commit.
- Universes, reported separately:
  - **U1 Runners**: every symbol-day with a Runners first alert (`work-pat/triggers-*.jsonl`, P1) —
    the stocks the owner actually charts. Primary.
  - **U2 Other active**: per day, 5 symbols drawn with seed 20261026 from symbols with a daily close
    in $0.10–$25 and daily volume ≥ 500,000 shares, excluding that day's U1 symbols. Exported with the
    same `pat.rs` harness. Secondary.

## 2. Zones (the shipped rule, unchanged)

`autoZones()` is ported to Python (`py/azones.py`) and must reproduce the TypeScript function's
output exactly on a parity fixture (synthetic sequences plus real candle days, every timeframe) run
through bun; the parity test is committed and must pass before any outcome is computed.

Causality. A zone is **formed** at the close of the first candle at which the shipped rule, given
only the candles up to then, has its base followed by a decisive first departure candle and a reach
of ≥ 2 ATR within the leg window. Its edges and kind are those `autoZones()` gives. The overlap rule
(an overlapping same-kind zone keeps the older) is applied at formation. The display filter
(three nearest per side) is NOT applied: every zone the rule forms is studied, and how many are
shown at once is reported. Equality with prefix runs of the rule is checked on 50 sampled
symbol-days per set (the share of zones whose formation, edges and kind match a prefix run is
reported).

## 3. Outcomes

After formation, the zone's **test** is the first later candle that trades into it (demand: low ≤
zone top; supply: high ≥ zone bottom). From the test candle (inclusive), within K candles:

- **hold**: demand — a candle closes above the zone top before any candle closes below the zone
  bottom (supply mirrored: closes below the bottom before any close above the top);
- **break**: the opposite close comes first;
- **unresolved**: neither within K.

K = 20 candles at 15 s (5 min), 20 at 30 s (10 min), 20 at 1 min (20 min), 12 at 5 min (60 min).
A zone untested by 20:00 ET is **untested**. Also reported: share tested, time to test, bounce =
maximum favourable move within K candles after the test in units of the formation ATR (demand: the
highest high minus the zone top), break rate.

## 4. Baselines

- **R random zones** (matched geometry): for every auto zone, one random zone on the same
  symbol-day and timeframe, of the same kind, height as a fraction of price and distance from the
  formation close as a fraction of price, formed at a random candle drawn uniformly from the
  candles that have the 14-candle ATR history (seed 20261026). Same test and outcome rules.
- **Volume shelves**: the server's shelves are produced only by the live Runners pipeline and are
  logged only for recorded days (from 2026-10-22); they cannot be replayed over these archives with
  the shipped code. Not compared here; stated as a limitation, with the earlier zone bake-off's shelf
  figures quoted for context only.

## 5. Primary estimand and verdict

Per timeframe, on U1: **Δ = hold rate (auto) − hold rate (random)**, hold rate = holds / tested
zones. Day-clustered bootstrap, 5,000 resamples, 95% percentile interval. No parameter is chosen:
the rule is frozen as shipped.

- **SUPPORTED** — CONFIRM Δ lower bound > 0 and point Δ ≥ +5 pp.
- **PROMISING BUT INCONCLUSIVE** — point ≥ +5 pp but the interval crosses 0, or fewer than 300
  tested zones.
- **NO EDGE OVER RANDOM** — otherwise.

The menu text candidate per timeframe is written from CONFIRM numbers in the form "Held about 1 in
N times (random zones: 1 in M)", rounded to the nearest whole N, and is reported whatever the verdict.

## Amendments

(none)

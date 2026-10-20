# Ross Panel: membership and momentum order (ross-panel-v1)

Owner product policy (Roman, 2026-10-19), implemented in `crates/stock-context` (`panel.rs`,
`engine.rs`), policy `crates/stock-context/policy/ross-panel-v1.json`, tagged
`ross-panel-v1@<sha256>` on every panel snapshot. Not a sourced Ross rule, not a validated
ranking. On the server behind `STOCK_CONTEXT=1` (section "On the server"); the five pillars it
uses are `pillars-v2` ([pillars-v1.md](pillars-v1.md) documents both).

## Membership: `five_pillars_membership_v1`

Pillar qualification and panel membership are separate outputs.

- **Admission**: all five pillars PASS in one decision (not five passes collected at different
  times) while the stock is platform-eligible.
- **Retention**: at least four pillars PASS and still platform-eligible. An UNKNOWN never counts
  as a pass. Four plus one FAIL or four plus one UNKNOWN stays, with the non-passing pillar,
  state and reason in `membership.nonpassing`.
- **Removal**: below four passes, recorded as `known_failures`, `insufficient_evidence` (every
  non-pass UNKNOWN) or `failures_and_unknowns`; or ineligible: `left_platform_band` (fresh price
  outside $0.10–$25) or `ineligible` (no fresh price, unsupported session).
- **Re-entry** needs 5/5 again and opens a new episode `<run>:<date>:<symbol>:<n>`; earlier
  episodes stay with their exit time and reason.
- **Platform eligibility** is not a sixth pillar: a fresh price inside the platform band in a
  pre/regular/after session. The band is production's quote band ($0.10–$25); the reference price
  pillar ($2–$20) may fail while a member is retained, leaving the platform band removes it.
- **Scope**: run, policy and SIP trading date. A new trading date (20:00 ET) starts with no
  members; the previous date stays in the decision log. Membership is derived from the decision
  stream, so a replay or a restart that re-applies the day reproduces it; it is never inferred from
  a current 4/5 snapshot.
- No score-based entry, no grace periods, no mandatory pillar, no rank-based removal, no relaxed
  admission when the panel is empty.

## Order: `bullish_momentum_order_v1`

Stronger current upward movement first; the pillar count, day gain, float, news age and
admission time do not order the panel.

For bar end `b`, `P(e)` is the latest OHLC print executed before `e` and admitted by the barrier,
valid only if at most 15 s older than `e` and in the same session segment as `b`.

1. `r_60 = P(b) / P(b−60 s)` descending.
2. `r_300 = P(b) / P(b−300 s)` descending; missing after observed.
3. `a_30 = P(b)·P(b−60 s) / P(b−30 s)²` descending (the change between the last two 30 s
   returns); missing after observed.
4. Symbol ascending (stable ties).

Every comparison cross-multiplies integer prices (exact; no floating logarithm), so display
rounding cannot reorder rows. Returns are displayed as simple-return basis points.

A member without a valid one-minute endpoint (no print within 15 s of it, a different session
segment, halted, or the run's coverage known partial) is **unranked**: listed after the ranked rows
by symbol, never given a zero return, and still a member. Dollar-volume participation (the
brief's fourth key) is not used: no comparable measurement is defined yet.

Each row also shows the 30 s / 1 min / 5 min returns, the acceleration and the distance below the
day's high. Panels are recomputed at every 15 s barrier while there are members (the window slides
even without prints) and published, with the decision revision they reflect, only when they
change. No rank hysteresis.

Limitation: a one-minute-first order does not recognize healthy trend structure and makes no claim
to be the most profitable order. It is the baseline later orders are measured against.

## Tests and cost (2026-10-20)

`cargo test -p stock-context`: membership unit tests (5 in / 4 stay / exits by cause / re-entry /
band), momentum unit tests (A/B order, stale and missing endpoints, halted, exact ordering), and
`tests/panel.rs`: the brief's example (A +45 % on the day but −2 % in the last minute, 5/5; B +18 %,
+3 %, retained 4/5 with float failing: B ranks first), unranked thin member, band exit and
re-entry, halted, empty panel, new trading date, identical replay of decisions and panels.

`cargo run --release -p stock-context --example bench -- 3000 1000000` (synthetic, Windows
desktop, engine only; 55 members at the close): 4.39 s wall for 1,000,000 prints, of which 4.04 s
is spent in 1,561 barrier evaluations (~2.6 ms each, ~4 µs per symbol decision); per-print
`apply` p50 200 ns, p99 600 ns. The per-print percentiles hide the barriers: the integration
benchmark must measure barrier time, transport and fan-out on the real host.

## On the server (`STOCK_CONTEXT=1`, off by default)

`crates/server/src/context.rs`: one engine per market `Core` (live and replay never share
state; a replay seek rebuilds it with the candles). Every print is applied once, on arrival:
with the chart engine's verdict when the symbol is loaded, with its own conditions when its
history is still being loaded (the chart engine applies it later; the context engine never sees
it twice). Admission time is the print's receive time when known. The server runs
**`pillars-v2`** (`Policy::current()`: price pillar on the app's own $0.10–$25 band, 120 s news
coverage lag), `ross-panel-v1` and the EXPERIMENTAL `setups-v0`; it refuses to start context
when either band differs from `PRICE_BAND_MIN` / `MAX`.

Evidence (`crates/server/src/evidence.rs`, every `CONTEXT_EVIDENCE_CHECK_SECS` = 15 s, off the
market path): float observations and news items written by `swimport evidence`, market-wide
news coverage, and the previous close and 30-session daily baseline built from the stored daily
bars and the corporate-action checks `swimport maintain` writes (`market_archive::baseline`).
Each record joins decisions only from its own `observed_at`. The raw `daily-closes.json` close is
still offered as a fallback (it can never pass the gain pillar; it keeps the stored-close
diagnostic where no baseline exists). With the collector running and the maintainer's
pre-session announcement check done, a stock can reach 5/5 in production.

Wire: `ross.subscribe` / `ross.snapshot` / `ross.update` and `context.get` / `context.decision`
(see the `ws.rs` header). The run's numerator coverage is reported `unverified` until the
recorder's coverage records are consulted (a PASS on an incomplete count still stands: missing
prints only lower it).

## Not here yet

Append-only decision log on disk, Signals UI, ignition. Per-member setup monitoring exists as
the EXPERIMENTAL `bull_flag_v0` family ([setups-v0.md](setups-v0.md)): each row carries `setup`,
plus `move_1m_bp`, `from_high_bp`, `rvol_milli`, `float_shares`, `news_first_seen`, `missing`
(non-passing pillars) and `spark` (the last 20 completed 15 s bar closes); the snapshot carries
`setup_policy` and `attention`. With `pillars-v2` the price pillar uses the app's own $0.10–$25
band, so the reference $2–$20 price failure described above applies to v1 only.

Known limits of the evidence today:

- **Band entrants.** A stock that enters the band intraday without an in-band close in the last
  5 dates has no split snapshot or announcement check until a warmup runs, and the server does
  not file warmup requests yet (`market_archive::warmup`): its gain and RVOL stay UNKNOWN that
  day. The float collector's own warmup covers its float.
- **Announcement freshness.** The coverage policy calls a check stale after 90 min, but the
  engine does not re-grade a delivered close: it uses the latest one. Live, a new check every
  30 min supersedes it; if the maintainer stops, the last "no known action" stays in use.
- **Replay.** A replayed day builds its baselines as of its first announcement check, and raw
  date files are re-fetched nightly, so older replayed days usually see no baseline (their
  `observed_at` is after the day). That is the causal rule, not a gap.
- **Renames.** Baselines are built for symbols in the previous day's raw bars; a ticker renamed
  overnight has none under its new symbol.

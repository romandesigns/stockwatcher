# Ross Panel: membership and momentum order (ross-panel-v1)

Owner product policy (Roman, 2026-10-19), implemented in `crates/stock-context` (`panel.rs`,
`engine.rs`), policy `crates/stock-context/policy/ross-panel-v1.json`, tagged
`ross-panel-v1@<sha256>` on every panel snapshot. Not a sourced Ross rule, not a validated
ranking. On the server behind `STOCK_CONTEXT=1` (section "On the server"); the five pillars it
uses are `pillars-v2` ([pillars-v1.md](pillars-v1.md) documents both); since 2026-11-22 the server
runs `pillars-v6` (SEC EDGAR filings as a second news source, [float-news-plan.md](float-news-plan.md)
section 10) and, while the owner's switch makes news optional, `ross-panel-v2` (section "News
optional" below).

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

## News optional: `ross-panel-v2` (owner decision 2026-11-22)

The synced switch **"Ross Panel: require a news catalyst"** (`detectors` entry
`ross-require-news`, default **on**; Settings on web and phone, every device, no restart) picks the
panel policy every context board runs: on, `ross-panel-v1` above; off,
`crates/stock-context/policy/ross-panel-v2.json` (`four_pillars_news_optional_membership_v1`,
`membership.news_optional: true`). Both are frozen by id and the snapshot names the one in force
(`panel_policy`, `news_optional`; the panel header says "news optional").

- **Counting.** The news pillar is left out: admit when price, gain, RVOL and float all PASS in one
  decision (4/4), stay while at least 3 of those 4 pass. Removal reasons are judged over the same
  four. The news pillar is still decided and stays in `nonpassing`.
- **Rows.** A member whose news pillar is not passing carries `news_unconfirmed` and shows
  "No news confirmed": its catalyst is the owner's to check. An SEC offering filing still shows
  "Offering filed".
- **Near-qualifying.** News no longer makes a symbol near or keeps it out: one blocked only by
  news is a member and leaves the list; float-only near misses stay.
- **Switching.** `Engine::set_panel_policy` marks every symbol for a fresh decision, so the change
  applies at the next 15 s barrier to the membership as it stands: an active episode continues
  under the new rule. Switching back on, a member admitted without news stays while 4/5 pass (v1's
  retention: its four other pillars), with its news pillar shown as missing; a new entry needs 5/5.
- **Restart.** A restart's replay of the day runs the policy in force at restart for the whole
  day; a switch flipped mid-day is not replayed at its own time.

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
coverage lag), `ross-panel-v1` and the EXPERIMENTAL `detection-v2` for the row cues (section
"Row cues" below); it refuses to start context when either band differs from `PRICE_BAND_MIN` /
`MAX`. Halts, link drops and provider errors reach the cue detector whether or not
`DETECTION_PANEL` is on, so a gap ends a member's episode exactly as it ends the Detection
Panel's.

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

## Row cues: one pullback detector (owner decision 2026-10-21)

Each row's `setup` is read from the engine's **`micro_pullback`** episodes under the product
detection policy **`detection-v2`** (variant `micro_pullback_mp5`; `stock_context::cue`,
[detection-core.md](detection-core.md) "One pullback detector"). It replaced `bull_flag_v0`
([setups-v0.md](setups-v0.md)), whose code is kept for research reproducibility only. The Ross
Panel and the Detection Panel show the same episodes, levels, states and ids, so they cannot
disagree. With `DETECTION_PANEL=0` (production) the engine runs only this family and only for
the panel's members: a member is stepped from its first member barrier (the 20-bar lookback
reaches a pullback already under way) and its episode is interrupted (`left_panel`) when it
leaves. Measured on this machine with 10 members (`examples/ross_cue_cost.rs`): about +26 µs
per barrier, +6–8 ns per print and +41 KiB held, inside the noise of a 3,000-symbol barrier.

**Why.** A 21-day comparison (pre-registered protocol and frozen choice on the
`research/setup-compare` branch, `docs/research/setup-compare/`) scored each detector's
"+3 % before −1.5 %" outcome after its trigger. On the 2026-10-07..20 holdout, micro-pullback MP5
(impulse ≥ 5 %, depth ≤ 38.2 %, impulse coverage ≥ 75 %) reached a precision of **0.183**,
bull_flag_v0 BF1 **0.163**, confidence intervals overlapping: MP5 is slightly ahead, not shown to
be better, and a triggered cue **failed about three times in four**.

**So the cues describe stages; they are not validated entry signals.** Labels (web and phone
share `packages/shared/src/ross.ts`):

| Detector state | `setup.state` (wire) | Shown as | Tone |
|---|---|---|---|
| `pullback_forming`, `armed` | `forming` | Pullback (with "trigger X" once armed) | muted |
| `armed`, price within 1 % below the trigger | `near` | At trigger | amber, edge |
| `crossing_provisional` | `crossing` | Crossing (a full bar must still close above) | accent, pulse, edge |
| `trigger_observed`, held 120 s | `triggered` | Broke trigger | normal text, never green, no edge |
| `invalidated`, held 120 s | `invalidated` | Failed | dimmed |

`attention` counts `near` and `crossing` only: a broke-trigger is history, not a call to act.
The trigger meter runs from the pullback low (the failure level) to the trigger, whose end mark is
neutral. Notes name levels and distances ("5.30 · 0.8% away · 1.1% over low"), never a stop or an
entry. The wire keeps the earlier coarse tokens so an older client still decodes; new fields are
`stage` (the detector state), `variant`, `episode_id`, `impulse_bp`, `depth_milli`,
`pullback_bars`, `near_trigger`, `stale`, `to_invalidation_bp` and `resolution`; the snapshot's
`setup_policy` is `detection-v2@<hash>`.

## Not here yet

Append-only decision log on disk, ignition. Each row carries `setup` (above), plus `move_1m_bp`, `from_high_bp`, `rvol_milli`, `float_shares`, `news_first_seen`, `news_evidence` (the source that passed news: Benzinga, or an SEC 8-K/6-K with its acceptance time and filing link; pillars-v6), `offering` (an SEC offering filing within 24 h), `news_unconfirmed` (news-optional panel only), `missing`
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

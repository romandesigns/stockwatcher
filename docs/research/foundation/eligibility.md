# Historical eligibility policy (research policy `eligibility-research-v1`)

Date: 2026-10-18. Base: origin/main e14e9ab. Code: `crates/research-features/src/eligibility.rs`
(band membership, tracker), `corporate.rs` (splits, renames, censoring). Production's band
(`market_events::PriceBand`, **$0.10**-$25.00) is unchanged.

## Why a research policy

The baseline audit (§5.6) found the universe built from today's active assets and band membership
decided with the same day's close: both are future information at any intraday decision time.
Research rebuilds membership point in time, from what was observable at `t` only.

## Rules

| Topic | Rule (v1) |
|---|---|
| Band | Inclusive `[min, max]`, research default **$0.15-$25.00**, configurable (`EligibilityConfig::with_band`). Compared in micro-dollars, no floats. |
| Price used | The **last eligible print observed by `t`**: the newest trade with `event_ts <= t` whose `conditions-research-v1` effect updates the **last** price, executed in a SIP session (pre/regular/after). Odd lots, average-price, prior-reference, out-of-sequence, official open/close records and every unresolved print can never move it. Never the day's close, high, low or anything later than `t`. |
| Prices | **Raw** (unadjusted) for membership: it is about the price the stock trades at, then. |
| Staleness | A last eligible print older than X cannot make a symbol currently eligible: **X = 30 min pre-market, 10 min regular, 30 min after hours**, using the session of `t` (inclusive: exactly X old is still fresh). |
| Trading-date boundary | A price **never carries across trading dates**: at 04:00 every symbol starts with no current price and needs its first eligible print of the day. (Yesterday's price is on yesterday's share basis and may be pre-split.) |
| Sessions | Decisions only in SIP sessions, 04:00-20:00 ET (early-close days end at 17:00, per `market_events::session`). At an overnight / closed `t` membership is `Closed`. Overnight (BOATS) prints never set the SIP last price; BOATS is a separate venue with its own (thin) prints, and the archive has none. A future overnight study gets its own policy, not this one. |
| Entry | Membership becomes `Eligible` (first of the trading date: `Entry`, later ones: `ReEntry`). |
| Exit | Membership stops being `Eligible`: out of band, stale, or session closed. |
| Re-entry | Allowed any number of times, no cool-down, history kept. Entries/exits per date are counted (`entries_today`, `exits_today`). |
| Hysteresis | **None.** See below. |
| Warmup history | Every symbol's data is retained from its trading date's **04:00 ET** (`history_start`), whether or not it is eligible. Cross-date history (previous close, multi-day warmup) is split-adjusted (below). |
| Determinism | Membership is a pure function `membership_at(last eligible print, t, config)`; the tracker only remembers the last print and the previous membership. Transitions are stamped with the evaluation time (a trade's time or the decision time passed to `advance`), so evaluating on a fixed grid (e.g. every 15 s interval end) gives identical transitions on every run. |

### Why these choices

- **Last eligible print, not the last print of any kind.** An odd lot or an average-price print is
  not a market price at `t` (production's chart excludes them from OHLC for the same reason). A
  symbol must not enter the band on a $0.16 odd lot while round lots trade at $0.12.
- **Staleness 10 min regular / 30 min extended.** The band asks "does this stock trade in the
  band now?". In the regular session, a candidate with no qualifying print for 10 minutes is not
  moving (the strategy looks at 15 s-5 m charts). Pre-market and after-hours prints are much
  sparser even for active small caps, so 30 minutes avoids dropping a stock that is quietly
  building a pre-market range. Both are configurable and should be revisited with the pilot's
  inter-trade-time distribution (a stock with a 10-min gap in the regular session is rare among
  the candidates this product cares about; the pilot can count how rare).
- **Halts.** No trades during a halt, so a halt longer than X makes the symbol stale; it re-enters
  on the first print after the resumption with no delay. Historical halts are not available
  (baseline audit §3), so this is the only consistent rule; features that care about halts must
  not read "stale" as "halted".
- **No hysteresis.** Hysteresis (e.g. enter at $0.16, leave at $0.14) would make membership depend
  on the path, adding a parameter to every result. It is needed only when leaving the set is
  costly, and here it is not: history retention does not depend on membership, so a stock flapping
  around $0.15 loses no state, it only produces entry/exit events, which are counted and can be
  filtered later (e.g. "first entry of the day only") without changing the definition.
- **Warmup from 04:00.** The chart's VWAP runs from the day's first candle and its EMA never
  resets, so a symbol entering at 10:42 must have its whole day already: a research feature must
  equal what Roman sees on the chart at that moment (`indicators.md`). Keeping a day of prints for
  every traded symbol is cheap compared with getting this wrong. Overnight is not included because
  the SIP archive has no overnight prints (see `indicators.md` on what that means for the chart's
  live VWAP).

## Splits, renames, delistings (`corporate.rs`)

- **Contemporaneous values use raw prices** (membership, intraday candles, VWAP, any indicator
  anchored at the day's start): within a trading date every print is on one share basis.
- **Cross-session values are split-adjusted**: previous close, gap %, multi-day returns, and any
  indicator warmup carried from an earlier date. A split with ex-date in `(earlier date, later
  date]` multiplies earlier prices by `old_rate / new_rate` and volumes by `new_rate / old_rate`
  (Alpaca's corporate-actions convention: `new_rate` shares per `old_rate`). Exact rationals;
  prices rounded to the micro-dollar, half away from zero.
- **A split day must not manufacture a setup.** `cross_session_reference` returns the previous
  close on the current date's basis, or **nothing**: when corporate actions do not cover
  `(previous date, date]` the reference is unavailable and every feature that needs it is missing
  for that symbol-date. It never falls back to the raw close. (A 1-for-10 reverse split of a $0.08
  stock opens around $0.80: raw, that is a +900 % gap, the most attractive "setup" of the day;
  adjusted, it is 0 %.) Coverage is explicit (`CorporateActions::covered`): "no split in the data"
  only means "no split" inside the covered range. Every symbol-date with a split ex-date also
  carries `split_day = true`, so studies can stratify or exclude them.
- **Membership on a split day** uses the raw post-split price, as on any day: a reverse split can
  legitimately move a stock into the band. That is a real eligibility change; only cross-session
  comparisons are guarded.
- **Renames** link history: on its rename date `NEW`'s previous close is `OLD`'s
  (`predecessor_on`), and splits recorded under the old symbol still apply across the rename.
  Identity across renames is by the corporate-actions record, not by name; symbol reuse by an
  unrelated company after a delisting is a different instrument as long as there is no rename
  record linking them.
- **Delistings**: a symbol's data ends at its last print; nothing is forward-filled, and a label
  horizon past the last available print is **censored** (`censored`), never filled with the last
  price.

### Limitations (stated, not solved)

1. **Survivorship.** The current archive's universe comes from Alpaca's *current* asset list, so a
   symbol delisted before the import never appears on earlier dates. Results over the archive are
   conditional on survival to the import date. Fixing it needs a point-in-time universe (e.g.
   every symbol with SIP trades on the date, or Alpaca's corporate-actions delistings joined back),
   which is a data-acquisition task, not a policy.
2. **Corporate-action coverage** depends on what is fetched; an uncovered date makes cross-session
   features missing, by design.
3. **No historical halts**: staleness during a halt is indistinguishable from no trading.
4. **No arrival times** in history: "observed by `t`" means `event_ts <= t` (reconstructed-history
   fidelity, baseline audit §5.3). A late-reported print (`L`, `Z`, `U`) counts from its execution
   time, which a live consumer could only know after the report arrived; `Z` and `U` cannot set the
   last price anyway (volume only), `L` can.

## Tests

`cargo test -p research-features` covers: inclusive band edges and configuration
(`band_is_inclusive_and_configurable`); every membership state including staleness boundaries in
pre and regular, a future print, a previous-date price, overnight and closed times
(`membership_rules`); which prints may set the price (`only_last_qualifying_sip_prints_set_the_price`);
entry, staleness exit, re-entry, band exit, close and day reset (`tracker_entry_exit_reentry_and_odd_lots_do_not_move_it`);
history start (`history_starts_at_04_00_et`); reverse and forward splits, rounding, coverage gaps,
renames carrying splits, censoring (`corporate::tests`).

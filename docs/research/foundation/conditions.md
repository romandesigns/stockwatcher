# Trade-condition policy audit (research policy `conditions-research-v1`)

Date: 2026-10-18. Base: origin/main e14e9ab. Code: `crates/research-features/src/conditions.rs`
(policy, audit table, tests), `condition_impact.rs` (quantification), `rf` CLI. Production
(`crates/market-events/src/conditions.rs`) is **unchanged**; this document records where the
research policy and production agree, where they intentionally differ, and why.

## Sources (read 2026-10-18)

| Tape | Source | Where |
|---|---|---|
| A, B (CTA) | *Consolidated Tape System (CTS) Pillar Multicast Output Binary Specification*, v2.11b, 2026-01-29 ([PDF](https://www.ctaplan.com/publicdocs/ctaplan/CTS_Pillar_Output_Specification.pdf)) | Field Descriptions, *Sale Condition*: p. 82 table (columns CONSOLIDATED Update LAST, HIGH/LOW; PARTICIPANT Update OPEN, LAST, HIGH/LOW; VOLUME Update), p. 83 notes #2-#4 and "Multiple sale condition" rules, p. 84 category layout, p. 85 combination notes; version history 2.10b (`N` reserved), the history entry replacing `@` with Space |
| C (UTP) | *UTP Data Feed Services Specification*, v4.1, September 2026 ([PDF](https://utpplan.com/DOC/UtpBinaryOutputSpec.pdf)) | §3.14.1 Sale Condition Modifiers (p. 47), §3.14.2 UTP Sale Condition Matrix (p. 48: Consolidated Update High/Low, Update Last; Market Center Update High/Low, Update Last; Update Volume), footnotes 1-3 and notes (p. 49), §3.14.3 combination table (p. 50), §3.14.5 notes 1-8 (pp. 51-52) |
| all | Alpaca Market Data FAQ, "How are bars aggregated?" ([page](https://docs.alpaca.markets/us/docs/market-data-faq#how-are-bars-aggregated)) | the condition table, minute (`M`) column: open/close, high/low, volume; "the strictest rule is applied" |
| O (OTC) | FINRA TDDS 2.1 | **not audited** (out of the research universe; see "Tape O") |

Alpaca's machine-readable tables (`GET /v2/stocks/meta/conditions/trade?tape=A|B|C`) were to be
saved by a second agent at `C:\research\stockwatcher\reports\probes\alpaca-conditions-<tape>.json`.
**They were not present when this audit was written** (the probes folder held only FMP profiles),
so the audit cites the documents above. When they appear, check that every code they list is a
row below (a code they list that is not here would be "not defined for tape" in v1, i.e. excluded).

## Method

For **every code per tape family** the audit records, separately, whether the code updates

- the **last** (open/close),
- the **high/low**,
- the **volume** (consolidated),

as (a) the SIP spec's *consolidated* columns, (b) Alpaca's *minute-bar* column, (c) production
(`market_events::condition_rule`), and (d) research v1. The SIP consolidated columns describe
**day-level** statistics (the official last sale, the day's high/low); several of their cells
carry day-level footnotes ("only if first or only qualifying last of the day", "only before
16:00:10"). Intraday bars have no such notion, which is why Alpaca publishes a separate minute
column. Research features are intraday, so for every code the spec defines, v1 takes **the intraday
reading**: Alpaca's minute column where Alpaca lists the code, otherwise the SIP matrix. Every
place where that differs from a SIP consolidated cell is listed under "Intentional deviations from
the SIP consolidated columns".

Classification compares production with the audited rule:

- **matches**: production's effective behaviour equals the audited rule for all three fields;
- **differs** (production looser / stricter): it does not;
- **unresolved**: the spec marks the code reserved, placeholder or TBD, or the tape's SIP does not
  define the code at all. v1 keeps the raw print, flags it (`ResearchEffect::unresolved`) and
  **excludes its price and volume** from every research feature.

Production collapses last and high/low into one "OHLC" flag; v1 keeps them apart (they are equal for
every resolved code in v1, so this costs nothing today and makes a future v2 that separates them a
data change, not a schema change).

## Results

68 audited (tape family, code) rows: 34 CTA (tapes A/B), 34 UTP (tape C).

| Class | Rows |
|---|---|
| matches | **56** |
| differs (production looser or stricter) | **0** |
| unresolved | **12** |

**No resolved code differs.** Every code whose meaning the specs settle gets the same answer from
production and v1. The 12 unresolved rows are the whole difference:

| Tape | Code | Why unresolved | Production does | v1 does |
|---|---|---|---|---|
| A/B | `N` | CTS: Reserved (was "Next Day" until 2.10b), volume TBD | volume only (as Alpaca's "Next Day") | excludes |
| A/B | `8` | CTS: Reserved, volume TBD | unrecognized → full | excludes |
| A/B | `A` `D` `Y` | UTP-only codes, not CTS codes | full (UTP meaning) | excludes |
| A/B | `G` `W` | UTP-only codes | volume only (UTP meaning) | excludes |
| A/B | `S` `1` | UTP-only codes | unrecognized → full | excludes |
| C | `E` | UTP: Placeholder, all TBD | full (CTA's "Automatic Execution" meaning, applied regardless of tape) | excludes |
| C | `N` | UTP: Reserved, volume TBD | volume only | excludes |
| C | `8` | UTP: Placeholder For 611 Exempt, all TBD | unrecognized → full | excludes |

Outside the 68 rows:

- **Any other code** (on any tape) is "not defined for tape": production treats it as a regular sale
  and reports it in `Effect::unrecognized`; v1 excludes it.
- **Tape O** (OTC, FINRA TDDS): not audited. v1 flags and excludes **every** tape O print, with or
  without codes (`"tape:O"`); production applies its single table.
- **Unknown tape** (e.g. a BOATS print whose tape is not reported): v1 resolves a code only when
  the CTA and UTP readings agree; codes whose meaning depends on the tape (`B` average price vs
  bunched, `E` auto-execution vs placeholder, and the UTP-only `A D G W Y S 1`) are excluded as
  `AmbiguousTape`. Production treats `B` on an unknown tape as average price (volume only) and the
  rest per its single table.

### Intentional deviations from the SIP consolidated columns (all match production and Alpaca)

| Code(s) | SIP consolidated | Intraday rule (v1 = production = Alpaca 1m) | Why |
|---|---|---|---|
| `T` (A/B "Extended Hours Trade", C "Form T") | last N, high/low N, volume Y | full | The official last sale and day high/low exclude extended hours; intraday candles cannot (pre-market would have no candles, and the strategy is built on pre-market). |
| `L` (A/B note #3; C footnote 2) | last conditional, high/low Y | full | The condition is about the official consolidated last (same participant / listing market / before 16:00:10), a day-level rule. |
| `P` `Z` `4` (A/B note #2; C footnote 1), `G` (C footnote 1) | last conditional (only/first last of the day), high/low **Y** | volume only | The price belongs to an earlier time (prior reference, out of sequence) or is not a market price (derivatively priced, bunched sold); in a 15 s-5 m bar it would plant an extreme at the wrong time. The SIP's day-level high/low may include it; a minute bar should not. |
| `9` | last Y, high/low Y, volume N | nothing | A post-close administrative correction of the day's close, zero volume (UTP §3.14.5 note 8). Not an execution inside any bar. |

These are recorded so a future reader comparing with a SIP consolidated feed (e.g. a daily high
that includes a `P` print) knows the difference is by design.

### Intentional differences between research v1 and production

1. **Unresolved codes**: production counts them (full or volume only); v1 excludes price and
   volume and flags the print. Conservative: a feature built on a print nobody can interpret is
   indefensible; the cost is measured below.
2. **Tape O**: production applies its table; v1 excludes every print.
3. **Unknown-tape ambiguous codes**: production picks one meaning; v1 excludes.
4. **Provider `u` markers** (historical `canceled` / `incorrect`): production's `Trade::effect`
   always drops them (hindsight). Research makes it explicit (`UpdateMarkerMode`): **features** use
   `AsDelivered` (the print counts, because live consumers saw it and history does not record when
   the cancel arrived), **labels and clean reference bars** may use `Hindsight` and must say so.

Multiple conditions: the strictest rule wins per field (CTS p. 83 "the NO takes precedence"; UTP
p. 49 "a 'no' update for any one of the statistics will take precedence"); any unresolved code makes
the whole print excluded. Codes are compared after trimming whitespace; `" "` and `""` are the
regular sale (CTS Space; a space-filled UTP byte carries no condition).

## How much it matters (quantification)

`rf conditions <archive-root> [--day D]... [--symbol S]...` runs `ImpactReport` over every print:
per `tape:code` prints, shares, and how many v1 excluded; total flagged prints/shares; prints whose
price or volume eligibility differs between production and v1; flagged prints per symbol.

| Data | Symbol-days | Prints | Flagged (v1 excluded) | Price disagreements | Volume disagreements | Codes seen |
|---|---|---|---|---|---|---|
| Real: MDXH 2026-10-16 (`data/archive-real`) | 1 | 19,781 | **0** | 0 | 0 | C: `@` 19,781, `I` 4,478, `F` 3,619, `T` 1,828, `4` 61, `W` 12, `M` 2, `Q` 2, `X` 2, `O` 1, `6` 1 |
| Fixture 2026-10-15 (`data/archive`; engineering only, never market evidence) | 4 | 114,008 | 0 | 0 | 0 | C: `@`, `I`, `T`, `W`, `M`, `Q` |

One real Nasdaq symbol-day carries none of the unresolved codes, as expected (they are reserved or
placeholders). **The pilot data at `C:\research\stockwatcher\data` did not exist when this was
written**, so the count over the pilot is not done. When it exists, run (from the repo root):

```sh
cargo run -p research-features --bin rf --release -- conditions C:/research/stockwatcher/data/<archive-root> > C:/research/stockwatcher/reports/condition-impact-v1.json
```

(`<archive-root>` is the directory holding the `YYYY-MM-DD/manifest.json` day folders.) Tape A/B
symbols are where the CTA-only exclusions (`A D G S W Y 1 N 8` on A/B) could appear; the report's
`codes` map shows each one's prints and shares, and `flagged_by_symbol` shows whether exclusions
cluster on a few names.

## The full audit table

Generated by `rf audit-table` from the same rows the tests check
(`every_resolved_audit_row_matches_production_and_v1_agrees_with_the_table`). SIP columns are the
consolidated ones; `cond #n` / `cond fn n` cite the CTS note / UTP footnote; `-` = not defined by
that SIP. Alpaca: G updates, R does not.

| Tape | Code | Description | SIP last | SIP high/low | SIP volume | Alpaca 1m (o/c, h/l, v) | Production | Research v1 | Class | Note |
|---|---|---|---|---|---|---|---|---|---|---|
| A/B | ` ` | Regular Sale | Y | Y | Y | G G G | full | full | matches |  |
| A/B | `@` | Regular Sale (legacy CTS code) | - | - | - | not listed | full | full | matches | CTS replaced `@` with Space for Regular Sale (version history); treated as Space |
| A/B | `B` | Average Price Trade | N | N | Y | R R G | volume only | volume only | matches |  |
| A/B | `C` | Cash Trade (Same Day Clearing) | N | N | Y | R R G | volume only | volume only | matches |  |
| A/B | `E` | Automatic Execution | Y | Y | Y | G G G | full | full | matches |  |
| A/B | `F` | Inter-market Sweep Order | Y | Y | Y | G G G | full | full | matches |  |
| A/B | `H` | Price Variation Trade | N | N | Y | R R G | volume only | volume only | matches |  |
| A/B | `I` | Odd Lot Trade | N | N | Y | R R G | volume only | volume only | matches |  |
| A/B | `K` | Rule 127 (NYSE) / Rule 155 (NYSE American) | Y | Y | Y | G G G | full | full | matches |  |
| A/B | `L` | Sold Last (Late Reporting) | cond #3 | Y | Y | G G G | full | full | matches | CTS last only if only qualifying last / same participant / listing market (a day-level last-sale rule); intraday follows Alpaca: full |
| A/B | `M` | Market Center Official Close | N | N | N | R R R | nothing | nothing | matches |  |
| A/B | `N` | Reserved (was Next Day; reserved since CTS 2.10b) | N | N | TBD | R R G | volume only | **excluded** (Reserved) | **unresolved** | reserved, volume TBD: unresolved; production counts volume |
| A/B | `O` | Market Center Opening Trade | Y | Y | Y | G G G | full | full | matches |  |
| A/B | `P` | Prior Reference Price | cond #2 | Y | Y | R R G | volume only | volume only | matches | CTS high/low YES is day-level (the price belongs to an earlier time); intraday: no price, as Alpaca's minute bars |
| A/B | `Q` | Market Center Official Open | N | N | N | R R R | nothing | nothing | matches |  |
| A/B | `R` | Seller | N | N | Y | R R G | volume only | volume only | matches |  |
| A/B | `T` | Extended Hours Trade | N | N | Y | G G G | full | full | matches | SIP consolidated last/high/low exclude extended hours; Alpaca minute bars (and production) use them, or pre-market would have no candles. Intentional: intraday features need them |
| A/B | `U` | Extended Hours Sold (Out Of Sequence) | N | N | Y | R R G | volume only | volume only | matches |  |
| A/B | `V` | Contingent Trade | N | N | Y | R R G | volume only | volume only | matches |  |
| A/B | `X` | Cross / Periodic Auction Trade | Y | Y | Y | G G G | full | full | matches |  |
| A/B | `Z` | Sold (Out Of Sequence) | cond #2 | Y | Y | R R G | volume only | volume only | matches | as P: intraday no price |
| A/B | `4` | Derivatively Priced | cond #2 | Y | Y | R R G | volume only | volume only | matches | as P: intraday no price |
| A/B | `5` | Market Center Reopening Trade | Y | Y | Y | G G G | full | full | matches |  |
| A/B | `6` | Market Center Closing Trade | Y | Y | Y | G G G | full | full | matches |  |
| A/B | `7` | Qualified Contingent Trade | N | N | Y | R R G | volume only | volume only | matches |  |
| A/B | `8` | Reserved | N | N | TBD | not listed | unrecognized → full | **excluded** (Reserved) | **unresolved** | reserved: unresolved; production does not know it and counts it fully |
| A/B | `9` | Corrected Consolidated Close (per Listing Market) | Y | Y | N | R R R | nothing | nothing | matches | a post-close correction of the day's close, zero volume; not an intraday execution |
| A/B | `A` | (UTP Acquisition) | - | - | - | not listed | full | **excluded** (NotDefinedForTape) | **unresolved** | not a CTS code; production applies the UTP meaning (full) |
| A/B | `D` | (UTP Distribution) | - | - | - | not listed | full | **excluded** (NotDefinedForTape) | **unresolved** | not a CTS code; production applies the UTP meaning (full) |
| A/B | `G` | (UTP Bunched Sold Trade) | - | - | - | not listed | volume only | **excluded** (NotDefinedForTape) | **unresolved** | not a CTS code; production applies the UTP meaning (volume only) |
| A/B | `S` | (UTP Split Trade) | - | - | - | not listed | unrecognized → full | **excluded** (NotDefinedForTape) | **unresolved** | not a CTS code; production: unrecognized, full |
| A/B | `W` | (UTP Average Price Trade) | - | - | - | not listed | volume only | **excluded** (NotDefinedForTape) | **unresolved** | not a CTS code; production applies the UTP meaning (volume only) |
| A/B | `Y` | (UTP Yellow Flag Regular Trade) | - | - | - | not listed | full | **excluded** (NotDefinedForTape) | **unresolved** | not a CTS code; production applies the UTP meaning (full) |
| A/B | `1` | (UTP Stopped Stock) | - | - | - | not listed | unrecognized → full | **excluded** (NotDefinedForTape) | **unresolved** | not a CTS code; production: unrecognized, full |
| C | `@` | Regular Sale | Y | Y | Y | G G G | full | full | matches |  |
| C | ` ` | (space-filled byte: no condition) | - | - | - | not listed | full | full | matches | unused byte positions are space-filled (§3.14.3); no condition |
| C | `A` | Acquisition | Y | Y | Y | G G G | full | full | matches |  |
| C | `B` | Bunched Trade | Y | Y | Y | G G G | full | full | matches |  |
| C | `C` | Cash Sale | N | N | Y | R R G | volume only | volume only | matches |  |
| C | `D` | Distribution | Y | Y | Y | G G G | full | full | matches |  |
| C | `E` | Placeholder | TBD | TBD | TBD | not listed | full | **excluded** (Placeholder) | **unresolved** | placeholder, all TBD: unresolved; production applies the CTA meaning (full) |
| C | `F` | Intermarket Sweep | Y | Y | Y | G G G | full | full | matches |  |
| C | `G` | Bunched Sold Trade | cond fn 1 | Y | Y | R R G | volume only | volume only | matches | high/low Yes and last only if first/only last-sale trade of the day are day-level; intraday no price, as Alpaca's minute bars |
| C | `H` | Price Variation Trade | N | N | Y | R R G | volume only | volume only | matches |  |
| C | `I` | Odd Lot Trade | N | N | Y | R R G | volume only | volume only | matches |  |
| C | `K` | Rule 155 Trade (AMEX) | Y | Y | Y | G G G | full | full | matches |  |
| C | `L` | Sold Last | cond fn 2 | Y | Y | G G G | full | full | matches | consolidated last only before 16:00:10 (End of Last Sale Eligibility); intraday: full |
| C | `M` | Market Center Official Close | N | N | N | R R R | nothing | nothing | matches |  |
| C | `N` | Reserved | N | N | TBD | R R G | volume only | **excluded** (Reserved) | **unresolved** | reserved, volume TBD: unresolved; production counts volume |
| C | `O` | Opening Prints | Y | Y | Y | G G G | full | full | matches |  |
| C | `P` | Prior Reference Price | cond fn 1 | Y | Y | R R G | volume only | volume only | matches | as G: intraday no price |
| C | `Q` | Market Center Official Open | N | N | N | R R R | nothing | nothing | matches |  |
| C | `R` | Seller | N | N | Y | R R G | volume only | volume only | matches |  |
| C | `S` | Split Trade | Y | Y | Y | not listed | unrecognized → full | full | matches | not in Alpaca's table; production reports it unrecognized but counts it fully, as the matrix says |
| C | `T` | Form T | N | N | Y | G G G | full | full | matches | as CTA T: intentional, intraday features need extended-hours prices |
| C | `U` | Extended trading hours (Sold Out of Sequence) | N | N | Y | R R G | volume only | volume only | matches |  |
| C | `V` | Contingent Trade | N | N | Y | R R G | volume only | volume only | matches |  |
| C | `W` | Average Price Trade | N | N | Y | R R G | volume only | volume only | matches |  |
| C | `X` | Cross/Periodic Auction Trade | Y | Y | Y | G G G | full | full | matches |  |
| C | `Y` | Yellow Flag Regular Trade | Y | Y | Y | G G G | full | full | matches |  |
| C | `Z` | Sold (out of sequence) | cond fn 1 | Y | Y | R R G | volume only | volume only | matches | as G: intraday no price |
| C | `1` | Stopped Stock (Regular Trade) | Y | Y | Y | not listed | unrecognized → full | full | matches | not in Alpaca's table; production: unrecognized, full, as the matrix says |
| C | `4` | Derivatively priced | cond fn 1 | Y | Y | R R G | volume only | volume only | matches | as G: intraday no price |
| C | `5` | Re-Opening Prints | Y | Y | Y | G G G | full | full | matches |  |
| C | `6` | Closing Prints | Y | Y | Y | G G G | full | full | matches |  |
| C | `7` | Qualified Contingent Trade (QCT) | N | N | Y | R R G | volume only | volume only | matches |  |
| C | `8` | Placeholder For 611 Exempt | TBD | TBD | TBD | not listed | unrecognized → full | **excluded** (Placeholder) | **unresolved** | placeholder, all TBD: unresolved; production: unrecognized, full |
| C | `9` | Corrected Consolidated Close (per listing market) | Y | Y | N | R R R | nothing | nothing | matches | a post-close correction, zero volume (§3.14.5 note 8); not an intraday execution |

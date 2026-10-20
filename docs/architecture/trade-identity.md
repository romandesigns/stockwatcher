# Trade identity, duplicates and revision targeting

Decided 2026-10-19. Code: `crates/market-events/src/event.rs` (`TradeId`, `PrintId`, `TradeRef`,
`Resolution`), `crates/chart-engine/src/engine.rs` (`apply_trade`, `apply_cancel`,
`apply_correction`), `crates/provider-sim/src/{bars,rest}.rs`.

## The problem

The print identity used to be `(source, exchange, source_seq)`, i.e. Alpaca's `i` on venue `x`. The
chart engine dropped a second print with that identity as `excluded:duplicate`. On venue `D` that
drops real prints.

## Evidence (acquisition pilot, `research/acquisition` 7c91eb5, report `01-acquisition-pilot.md`)

The 12 pilot symbol-days hold 72,036 SIP prints, archived under `C:\research\stockwatcher\data\archive`.
Re-read for this decision:

| Measure | Count |
|---|---|
| Prints on `D` | 27,320 (38% of all prints) |
| `(symbol, exchange, id)` keys seen more than once | 1,806, **all on `D`** (1,671 twice, 135 three times) |
| Extra prints under a repeated key | 1,941 |
| Colliding pairs | 2,076 |
| Pairs with the same execution time | **0** |
| Pairs with the same price and size | 10 (9 also with the same conditions) |
| Pairs within 10 minutes of each other (chart-engine retention) | 269 |
| ...of which same price and size | **4**, all also same conditions |
| Repeats under `(source, exchange, id, event_ts)` | **0** |

- Tape is equal in every pair (all tape C in this sample).
- Conditions differ in roughly two thirds of pairs.
- Per symbol, the `D` id sequence goes backwards dozens to hundreds of times a day (CONL: 487 times
  in 8,178 `D` prints). That is what interleaved independent counters look like.
- Ids repeat at most three times, matching the three FINRA trade reporting facilities: Nasdaq
  Carteret, Nasdaq Chicago and NYSE.

**What Alpaca gives us.** A trade message has exactly `T S i x p s c t z`. Historical responses add
`u`, as in the stored probe `alp-trades-boats-AAPL.json` and `import::alpaca::RawTrade`. There is
**no field naming the TRF**, so the facility can't be recovered from the message.

**How revisions name their target** (`alpaca_stream/protocol.rs`):

| Message | Fields |
|---|---|
| Correction `T:"c"` | `x`, `oi` (original id), `op`/`os`/`oc` (original price/size/conditions), `ci`/`cp`/`cs`/`cc` (corrected), `t`, `z` |
| Cancel/error `T:"x"` | `x`, `i`, `p`, `s`, `a`, `t`, `z` |

Neither carries the original print's execution time: `t` is when the revision was published. So on
`D`, `(x, id)` alone can name more than one print.

The pilot had no live corrections or cancels: 0 prints carried a historical `u` flag. The targeting
rule below therefore rests on the message format and on the collision statistics above. It has not
been checked against an observed TRF correction.

## The rule

1. **A print's identity is `(source, exchange, source_seq, event_ts)`** (`PrintId`, which equals the
   engine's `OrderKey`).
   - A redelivery repeats the message, so it repeats the nanosecond timestamp and is still dropped as
     a duplicate. Reconnect replay and a recording read twice are both covered by the tests.
   - Distinct TRF prints never shared a timestamp in the pilot, so all of them are kept.
   - Every archived trade already carries `event_ts`, so existing `.swa` archives and fixtures read
     and replay unchanged. There was no format or contract version bump, and no fallback was needed.
2. **`TradeId` stays the provider reference.** It is what a correction or cancel names, and the
   engine indexes retained prints by it, with several prints allowed per reference.
3. **A correction or cancel resolves to exactly one retained print, or to none** (`TradeRef::resolve`).
   - No retained print carries the id → `correctionsUnapplied` / `cancelsUnapplied`. This is
     unchanged.
   - The revision carries the original's price and size, which every provider message does. The
     target is then the one retained print with that id whose price **and** size match.
   - If several prints match, or none does, the revision is **not applied** and is counted as
     `correctionsAmbiguous` / `cancelsAmbiguous`.
   - "None matches" includes a single retained print with the id. On `D`, the named print may already
     be out of retention while another TRF's print with the same id is still held. Applying the
     revision to that print would be a guess.
   - A reference without a payload (size 0) is resolved by id alone, only when one print holds it.
     Only test fixtures send that.
4. **Consumers follow the engine.** The simulator's minute VWAP (`BarBook`) applies the engine's
   `Revision`, which now carries the revised print's execution time `ts`, instead of keeping its own
   id map. The simulator's historical trade list (`rest.rs`) uses the same resolver.

## Remaining ambiguity

- **Identical twins.** Two TRF prints with the same id, price and size inside retention can't be
  told apart by any field Alpaca sends. A revision naming them is counted as ambiguous and left
  unapplied, so a withdrawn print may stay in the candles. Expected rate: 4 such pairs in 72,036
  prints. Conditions (`oc`) are not used as a tie-break; in the pilot they would have separated none
  of the 4.
- **Same-nanosecond collision.** Two TRF prints with the same id **and** the same execution time
  would be merged as a duplicate. This was never seen (0 of 2,076 pairs).
- **Corrected ids on `D`** (`ci`) may also collide. The corrected print is indexed under its new id,
  and later revisions resolve among the holders of that id by the same rule.
- **Price/size mismatch on unique venues.** A cancel whose price or size disagrees with the only
  print holding its id is now treated as ambiguous rather than applied, on every venue. Alpaca
  defines `p`/`s` and `op`/`os` as the original's values, so this should not occur. If it shows up in
  the `*Ambiguous` counters off `D`, revisit the rule.
- **To close this out:** find a symbol-day with real TRF corrections or cancels (live, or historical
  `u` flags), then check how often revisions resolve uniquely.

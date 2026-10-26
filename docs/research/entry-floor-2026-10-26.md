# Current entry floor: $0.10

Owner instruction, 2026-10-26: lower all current live entry floors from $0.15 to
$0.10, retaining the definitions actually used by historical studies.

## Result

- Precursor scoring (including R2), Buying bursts and volume shelves now use
  the inclusive $0.10–$25 application band. Running, Fast and Run +20% already
  used that minimum.
- Discovery eligibility uses the new `pillars-v3` policy at $0.10. Its other
  rules equal `pillars-v2`; both earlier pillar policies are unchanged.
- Shared precursor, burst and shelf entry constants derive from
  `market_events::PriceBand::DEFAULT`. Precursor/burst alert logs and shelf run
  logs carry the active band alongside their model/strategy identity.
- The model weights, score thresholds, liquidity checks, readiness guards,
  cooldowns and budgets are unchanged. Being eligible does not guarantee an alert.
- Canonical research rocket outcomes and earlier studies retain their $0.15
  definitions. The new price range has not acquired their validation. Burst
  pushes below $0.15 omit the historical odds; the shared section explicitly
  labels the original study's $0.15–$25 range.

This note records the original local floor-change verification. The subsequent
MP5 implementation and replay assessment are in `mp5-wick-v1-results.md`;
publication and runtime verification are recorded separately by the release.

## Is the MP5 wick-rejection check worth implementing?

Worth implementing as a diagnostic experiment first. MP5 already identifies
the frozen impulse-high candle, but presently does not require it to close
strongly. Recording upper-wick fraction and close location would make the
suspected rejection measurable. Use that actual frozen candle and only evidence
available at the arming cutoff. Missing or incomplete evidence remains UNKNOWN.

A hard rejection rule should follow a paired replay comparison: how many failed
notices it removes, how many successful continuations it loses, and whether that
tradeoff holds across sessions and price ranges. WFF alone cannot settle that
question. `wff-followup-2026-10-26.md` describes the proposed experiment; its
numerical cutoffs are unvalidated candidates. The follow-up now implements the
observation, with hard rejection left off after the replay assessment. That
behavior is separate from this price-floor change.

The broader literature reinforces the need to define and test the exact rule:
the [Lo, Mamaysky and Wang study](https://www.nber.org/papers/w7613) evaluates
specified patterns statistically, while a
[study of candlestick holding strategies](https://www.sciencedirect.com/science/article/pii/S0378426615002678)
finds different results under different holding strategies. Neither study
validates an intraday MP5 wick filter or this new sub-$0.15 population. The
diagnostic-first recommendation above is an engineering judgment based on that
evidence limit and the current implementation.

## Validation

Boundary checks cover inclusive $0.10 admission, rejection below the minimum,
unchanged upper bounds, sub-dime precursor history at crossing, liquidity guards,
shelf cooldowns, discovery policy identity, and suppression of out-of-range
historical burst odds. Frozen policy and model files were checked against HEAD.
Detailed command outputs are under `H:/wavystack/outputs/entry-floor-20261026`.

- Rust: 1,050 executable tests passed, 0 failed; 14 existing opt-in tests ignored.
- Rust documentation: 1 test passed. The pinned toolchain lacked `rustdoc`; the
  already installed stable toolchain supplied the exact same Rust 1.98.0 compiler
  build (`88d9e12ae`). `RUSTDOC` was set only for that command. No installation
  or persistent toolchain setting was changed.
- Rustfmt and CI's nine-crate Clippy command with warnings denied: passed.
- Shared: 211 tests, typecheck and lint passed.
- Web: 87 tests and production TypeScript/Vite build passed.
- Mobile: 87 tests and typecheck passed.
- Web and mobile lint exited 0 with the existing unrelated warnings; the web
  build retains the existing bundle-size warning.
- `git diff --check`: passed.

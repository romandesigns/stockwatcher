//! # research-features
//!
//! Research-only definitions for StockWatcher's setup research. **Production never depends on this
//! crate**: it changes nothing the server, the chart engine or any client does. Every policy here
//! is versioned and the id is recorded with anything computed under it:
//!
//! | Module | Policy / definition | Doc |
//! |---|---|---|
//! | [`conditions`] | `conditions-research-v1`: per-tape trade-condition audit; unresolved codes excluded | `docs/research/foundation/conditions.md` |
//! | [`condition_impact`] | counts what v1 changes on real prints | same |
//! | [`eligibility`] | `eligibility-research-v1`: point-in-time $0.15-$25 band membership | `docs/research/foundation/eligibility.md` |
//! | [`corporate`] | split adjustment and renames for cross-session values | same |
//! | [`indicators`] | `chart_*`: exact Rust port of `packages/chart-core/src/indicators.ts`, plus `vwap_trade` | `docs/research/foundation/indicators.md` |
//!
//! The `rf` binary runs the condition-impact count over an archive and produces the candle /
//! indicator JSON for the TypeScript parity check.

pub mod condition_impact;
pub mod conditions;
pub mod corporate;
pub mod eligibility;
pub mod indicators;
pub mod parity;

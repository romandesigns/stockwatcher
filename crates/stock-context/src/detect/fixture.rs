//! Deterministic adversarial fixtures for the detection families: small synthetic event streams
//! with fictional symbols, for tests, the `detection_demo` example, and the server's labelled
//! fixture/replay demonstration. A fixture run is `Fidelity::SyntheticFixture` with source
//! `fixture`; its quiet references are `synthetic: true`. Nothing here is market data and no
//! fixture is evidence of a detector's usefulness.

use super::{DetectionPolicy, DetectionSnapshot, DetectionTransition, QuietReference};
use crate::{Coverage, Engine, Fidelity, Policy, PrintRevision, RevisionTarget, RunInfo, Verdict};
use chrono::NaiveDate;
use market_events::session::{SessionKind, et};
use market_events::{Price, Source, Trade, TradeId, UnixNanos};
use serde::Serialize;

pub const SEC: i64 = 1_000_000_000;
const MS: i64 = 1_000_000;
const BOTH: Verdict = Verdict {
    ohlc: true,
    volume: true,
};

/// One input to an engine, in the order a run would apply it.
#[derive(Clone, Debug)]
pub enum FixtureStep {
    Print {
        symbol: String,
        exec: i64,
        admit: i64,
        price: i64,
        size: u64,
        seq: u64,
    },
    /// A cancel of an earlier fixture print (by its seq, price and size).
    Cancel {
        symbol: String,
        seq: u64,
        price: i64,
        size: u64,
        admit: i64,
    },
    Advance(i64),
    QuietReference(QuietReference),
    Interrupt {
        symbol: Option<String>,
        at: i64,
        reason: &'static str,
    },
    Resume {
        symbol: Option<String>,
        at: i64,
    },
}

/// What a fixture run produced.
#[derive(Clone, Debug, Serialize)]
pub struct FixtureRun {
    pub scenario: String,
    pub transitions: Vec<DetectionTransition>,
    pub snapshot: DetectionSnapshot,
}

/// The fixture trading day (a full Friday session).
pub fn day() -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 10, 16).expect("valid date")
}

/// ET wall clock on the fixture day, ns.
pub fn at(h: u32, m: u32, s: u32) -> i64 {
    et(day(), h, m, s, 0).expect("valid time").nanos()
}

/// End of the k-th 15 s slot after 09:30.
pub fn end(k: i64) -> i64 {
    at(9, 30, 0) + (k + 1) * 15 * SEC
}

/// The barrier cutoff of slot k.
pub fn cutoff(k: i64) -> i64 {
    end(k) + 2 * SEC
}

/// A script under construction.
#[derive(Clone, Debug, Default)]
pub struct Script {
    pub steps: Vec<FixtureStep>,
    seq: u64,
}

impl Script {
    pub fn new() -> Script {
        Script::default()
    }

    /// One print at `exec` (admitted 100 ms later), price in cents. Returns its seq.
    pub fn print(&mut self, sym: &str, exec: i64, cents: i64, size: u64) -> u64 {
        self.print_admitted(sym, exec, exec + 100 * MS, cents * 10_000, size)
    }

    /// One print with an explicit admission time, price in micro-dollars. Returns its seq.
    pub fn print_admitted(
        &mut self,
        sym: &str,
        exec: i64,
        admit: i64,
        micros: i64,
        size: u64,
    ) -> u64 {
        self.seq += 1;
        self.steps.push(FixtureStep::Print {
            symbol: sym.into(),
            exec,
            admit,
            price: micros,
            size,
            seq: self.seq,
        });
        self.seq
    }

    /// Prints making slot k exactly `[open, high, low, close]` (cents): open at +1 s, high at
    /// +4 s, low at +8 s, close at +12 s.
    pub fn bar(&mut self, sym: &str, k: i64, ohlc: [i64; 4], size: u64) {
        let s = end(k) - 15 * SEC;
        let [o, h, l, c] = ohlc;
        self.print(sym, s + SEC, o, size);
        self.print(sym, s + 4 * SEC, h, size);
        self.print(sym, s + 8 * SEC, l, size);
        self.print(sym, s + 12 * SEC, c, size);
    }

    /// Quiet two-price bars alternating `a`/`b` (cents) over slots `ks`.
    pub fn quiet(&mut self, sym: &str, ks: std::ops::Range<i64>, a: i64, b: i64) {
        for k in ks {
            let (o, c) = if k % 2 == 0 { (b, a) } else { (a, b) };
            self.bar(sym, k, [o, o.max(c), o.min(c), c], 100);
        }
    }

    pub fn advance(&mut self, t: i64) {
        self.steps.push(FixtureStep::Advance(t));
    }

    pub fn cancel(&mut self, sym: &str, seq: u64, cents: i64, size: u64, admit: i64) {
        self.steps.push(FixtureStep::Cancel {
            symbol: sym.into(),
            seq,
            price: cents * 10_000,
            size,
            admit,
        });
    }

    /// A synthetic prior-session quietness reference for `sym` (never a real one).
    pub fn quiet_reference(&mut self, sym: &str, q_low_ppm: i64, d_low_ppm: i64) {
        self.steps.push(FixtureStep::QuietReference(QuietReference {
            id: format!("fixture-quiet:{sym}"),
            version: 1,
            symbol: sym.into(),
            trading_date: day(),
            segment: SessionKind::Regular,
            q_low_ppm,
            d_low_ppm,
            percentile: 30,
            prior_sessions: 10,
            prior_through: NaiveDate::from_ymd_opt(2026, 10, 15).expect("valid date"),
            basis: "synthetic".into(),
            short_history: false,
            synthetic: true,
            provenance: "fixture: synthetic prior-session percentiles".into(),
            observed_at: at(8, 0, 0),
        }));
    }
}

/// A fixture engine: current pillar and detection policies, labelled synthetic.
pub fn engine(run_id: &str) -> Engine {
    Engine::new(
        Policy::current(),
        RunInfo {
            run_id: run_id.into(),
            source: "fixture".into(),
            fidelity: Fidelity::SyntheticFixture,
            numerator_coverage: Coverage::Complete,
        },
    )
    .with_detection_policy(DetectionPolicy::current())
}

fn fixture_id(seq: u64) -> TradeId {
    TradeId {
        source: Source::Fixture,
        exchange: String::new(),
        seq,
    }
}

/// Applies a script and returns every transition it produced, in order.
pub fn drive(e: &mut Engine, steps: &[FixtureStep]) -> Vec<DetectionTransition> {
    let mut out = Vec::new();
    for s in steps {
        match s {
            FixtureStep::Print {
                symbol,
                exec,
                admit,
                price,
                size,
                seq,
            } => {
                let t = Trade::new(
                    symbol.as_str(),
                    UnixNanos(*exec),
                    Price::from_micros(*price),
                    *size,
                    *seq,
                );
                e.apply(&t, BOTH, UnixNanos(*admit));
            }
            FixtureStep::Cancel {
                symbol,
                seq,
                price,
                size,
                admit,
            } => {
                let rev = PrintRevision {
                    symbol: symbol.clone(),
                    target: RevisionTarget {
                        id: fixture_id(*seq),
                        price: *price,
                        size: *size,
                    },
                    corrected: None,
                    last: None,
                    day_high: None,
                };
                e.apply_revision(&rev, UnixNanos(*admit));
            }
            FixtureStep::Advance(t) => {
                e.advance_to(UnixNanos(*t));
            }
            FixtureStep::QuietReference(r) => e.add_quiet_reference(r.clone()),
            FixtureStep::Interrupt { symbol, at, reason } => {
                e.interrupt_detection(symbol.as_deref(), UnixNanos(*at), reason);
            }
            FixtureStep::Resume { symbol, at } => {
                e.resume_detection(symbol.as_deref(), UnixNanos(*at));
            }
        }
        out.extend(e.take_detection_transitions());
    }
    out
}

/// Runs a named scenario on a fresh fixture engine.
pub fn run(name: &str) -> Option<FixtureRun> {
    let steps = script(name)?;
    let mut e = engine(&format!("fixture-{name}"));
    let transitions = drive(&mut e, &steps);
    Some(FixtureRun {
        scenario: name.into(),
        transitions,
        snapshot: e.detection_snapshot().expect("detection on"),
    })
}

pub fn scenario_names() -> &'static [&'static str] {
    &[
        "ignition_no_precursor",
        "quiet_build_ignition",
        "wick_only",
        "sparse_prints",
        "micro_valid",
        "micro_crossing_before_arming",
        "micro_retracted",
        "micro_invalidated",
        "micro_depth_exceeded",
        "micro_crossing_cancelled",
        "micro_halt_interrupt",
    ]
}

/// IGNB: 40 quiet adjacent bars, then a 5.00 → 5.08 bar (Z ≈ 5.4, r = 1.59 %, E = 5.5).
fn ignition_base(s: &mut Script, sym: &str) {
    s.quiet(sym, 0..40, 500, 501);
    s.bar(sym, 40, [500, 510, 499, 508], 400);
    for k in 41..52 {
        s.bar(sym, k, [508, 509, 507, 508], 100);
    }
}

/// MPXA-style impulse 3.00 → 3.40 with a two-bar retreat to 3.32 (d = 200 milli), armed at the
/// cutoff of slot 11 with T = 3.40, I = 3.32.
fn micro_base(s: &mut Script, sym: &str) {
    for k in 0..6 {
        s.bar(sym, k, [300, 300, 300, 300], 100);
    }
    s.bar(sym, 6, [300, 302, 300, 302], 200);
    s.bar(sym, 7, [302, 310, 302, 310], 300);
    s.bar(sym, 8, [310, 325, 310, 325], 300);
    s.bar(sym, 9, [325, 340, 325, 338], 400);
    s.bar(sym, 10, [338, 338, 333, 335], 150);
    s.bar(sym, 11, [335, 336, 332, 334], 120);
}

pub fn script(name: &str) -> Option<Vec<FixtureStep>> {
    let mut s = Script::new();
    match name {
        "ignition_no_precursor" => {
            ignition_base(&mut s, "IGNB");
            s.advance(cutoff(60));
        }
        "quiet_build_ignition" => {
            s.quiet_reference("QBLD", 10_000, 5_000);
            s.quiet("QBLD", 0..40, 299, 300);
            s.bar("QBLD", 40, [300, 300, 300, 300], 150);
            s.bar("QBLD", 41, [300, 301, 300, 301], 200);
            s.bar("QBLD", 42, [301, 302, 301, 302], 250);
            s.bar("QBLD", 43, [302, 305, 302, 305], 300);
            s.bar("QBLD", 44, [305, 307, 305, 307], 300);
            s.bar("QBLD", 45, [307, 318, 306, 317], 900);
            for k in 46..56 {
                s.bar("QBLD", k, [317, 318, 316, 317], 200);
            }
            s.advance(cutoff(64));
        }
        "wick_only" => {
            s.quiet("WCKX", 0..40, 500, 501);
            s.bar("WCKX", 40, [501, 520, 500, 502], 400);
            s.bar("WCKX", 41, [502, 502, 501, 501], 100);
            s.advance(cutoff(50));
        }
        "sparse_prints" => {
            for k in (0..60).step_by(4) {
                s.bar("SPRS", k, [400, 400, 400, 400], 100);
            }
            s.bar("SPRS", 70, [400, 440, 400, 440], 500);
            s.advance(cutoff(90));
        }
        "micro_valid" => {
            micro_base(&mut s, "MPXA");
            // Bar 12 straddles armed_at (cutoff(11)): its +1 s print is pre-arm, the 3.41 at
            // +4 s is the first post-arm crossing; its close cannot confirm.
            s.bar("MPXA", 12, [335, 341, 335, 342], 300);
            s.bar("MPXA", 13, [342, 344, 339, 343], 300);
            s.advance(cutoff(20));
        }
        "micro_crossing_before_arming" => {
            micro_base(&mut s, "MPXB");
            // Executed and admitted after bar 11's end but before its cutoff: before the levels
            // exist. Price above H0 at arming → superseded, never a trigger.
            s.print_admitted(
                "MPXB",
                end(11) + 1_500 * MS,
                end(11) + 1_900 * MS,
                3_410_000,
                100,
            );
            s.bar("MPXB", 12, [341, 343, 340, 343], 300);
            s.bar("MPXB", 13, [343, 345, 342, 344], 300);
            s.advance(cutoff(20));
        }
        "micro_retracted" => {
            micro_base(&mut s, "MPXC");
            s.bar("MPXC", 12, [335, 341, 334, 338], 300);
            s.bar("MPXC", 13, [338, 339, 335, 337], 200);
            s.advance(cutoff(30));
        }
        "micro_invalidated" => {
            micro_base(&mut s, "MPXD");
            s.bar("MPXD", 12, [334, 335, 331, 331], 300);
            s.advance(cutoff(20));
        }
        "micro_depth_exceeded" => {
            for k in 0..6 {
                s.bar("MPXE", k, [300, 300, 300, 300], 100);
            }
            s.bar("MPXE", 6, [300, 302, 300, 302], 200);
            s.bar("MPXE", 7, [302, 310, 302, 310], 300);
            s.bar("MPXE", 8, [310, 325, 310, 325], 300);
            s.bar("MPXE", 9, [325, 340, 325, 338], 400);
            s.bar("MPXE", 10, [338, 338, 333, 335], 150);
            s.print("MPXE", end(10) + SEC, 330, 100);
            s.bar("MPXE", 11, [330, 330, 315, 318], 300);
            s.advance(cutoff(20));
        }
        "micro_crossing_cancelled" => {
            micro_base(&mut s, "MPXF");
            let sq = s.print("MPXF", end(11) + 3 * SEC, 341, 100);
            s.print("MPXF", end(11) + 5 * SEC, 338, 100);
            s.cancel("MPXF", sq, 341, 100, end(11) + 6 * SEC);
            s.advance(cutoff(30));
        }
        "micro_halt_interrupt" => {
            micro_base(&mut s, "MPXH");
            s.steps.push(FixtureStep::Interrupt {
                symbol: Some("MPXH".into()),
                at: end(11) + 5 * SEC,
                reason: "halt",
            });
            s.steps.push(FixtureStep::Resume {
                symbol: Some("MPXH".into()),
                at: end(20),
            });
            s.bar("MPXH", 21, [334, 345, 334, 344], 300);
            s.advance(cutoff(30));
        }
        _ => return None,
    }
    Some(s.steps)
}

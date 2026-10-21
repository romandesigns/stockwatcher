//! Runs the deterministic detection fixtures (`stock_context::detect::fixture`) through a real
//! engine and prints what the detectors logged. Synthetic, fictional symbols: an engineering
//! demonstration, never market evidence.
//!
//! ```text
//! cargo run -p stock-context --example detection_demo -- all            # JSON lines
//! cargo run -p stock-context --example detection_demo -- micro_valid --summary
//! ```
//!
//! JSON mode prints one `{"scenario","kind":"transition"|"snapshot",...}` object per line.

use stock_context::detect::fixture;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let summary = args.iter().any(|a| a == "--summary");
    let wanted: Vec<&str> = args
        .iter()
        .filter(|a| !a.starts_with("--"))
        .map(String::as_str)
        .collect();
    let names: Vec<&str> = if wanted.is_empty() || wanted == ["all"] {
        fixture::scenario_names().to_vec()
    } else {
        wanted
    };
    for name in names {
        let Some(run) = fixture::run(name) else {
            eprintln!(
                "unknown scenario {name}; known: {:?}",
                fixture::scenario_names()
            );
            std::process::exit(2);
        };
        if summary {
            println!("== {name}");
            for t in &run.transitions {
                println!(
                    "  #{:<3} {:<22} {:<20} {:?} -> {:?} [{}] {} {:?}",
                    t.seq,
                    t.family.as_str(),
                    t.event,
                    t.from,
                    t.to,
                    t.reason,
                    t.observed_at,
                    t.level.as_ref().map(|l| (l.kind, l.price, l.version)),
                );
            }
            println!(
                "  active={} recent={} dropped={}",
                run.snapshot.active.len(),
                run.snapshot.recent.len(),
                run.snapshot.telemetry.transitions_dropped
            );
            continue;
        }
        for t in &run.transitions {
            let mut v = serde_json::to_value(t).expect("serializable");
            v["scenario"] = name.into();
            v["kind"] = "transition".into();
            println!("{v}");
        }
        let mut v = serde_json::to_value(&run.snapshot).expect("serializable");
        v["scenario"] = name.into();
        v["kind"] = "snapshot".into();
        println!("{v}");
    }
}

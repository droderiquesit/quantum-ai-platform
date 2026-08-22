//! The Deep Brain node.
//!
//! Research, causal reasoning, simulation, optimisation and learning: the path
//! that may take minutes to months and may call a language model.
//!
//! It runs the intelligence loop. What it does *not* do is reach a venue: order
//! submission belongs to the execution path, and the agents this node hosts are
//! checked at start-up to confirm none of them holds a market-touching
//! capability.

use qip_core::error::{Error, Result};
use qip_core::{Clock, SystemClock};
use qip_financial::universe::Universe;
use qip_investment_agents::manifests;
use qip_kernel::{Platform, PlatformConfig};
use qip_observability::Telemetry;
use qip_risk::limits::LimitSet;
use std::sync::Arc;

fn main() {
    if let Err(error) = run() {
        eprintln!("qip-deepbrain: {}", error.message());
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);
    let now = clock.now();

    // The check: nothing this node hosts may reach a market. The execution
    // agent runs on the execution path, not here.
    let roster = manifests::roster(now);
    for manifest in roster.iter() {
        if manifest.id == manifests::ids::EXECUTION {
            continue;
        }
        for capability in manifest.capabilities.iter() {
            if capability.touches_market() {
                return Err(Error::denied(format!(
                    "{} holds {capability}; the deep brain hosts no agent that can reach a venue",
                    manifest.id
                )));
            }
        }
    }

    let config = PlatformConfig::default();
    let context = qip_core::Context::new(clock.clone(), config.seed);
    let ceiling = config.autonomy_ceiling;
    let mut platform = Platform::new(
        config,
        context,
        Telemetry::new("qip-deepbrain", clock.clone()),
        Universe::new(),
        LimitSet::conservative_default(),
    )?;

    println!("qip-deepbrain");
    println!("  autonomy ceiling: {ceiling}");
    println!("  agents:           {}", platform.organisation().len());
    println!(
        "  live trading:     {}",
        if platform.is_live_capable() {
            "reachable"
        } else {
            "unreachable in this deployment"
        }
    );

    let report = platform.run_cycle(clock.now());
    println!();
    println!("{}", report.summarise());
    Ok(())
}

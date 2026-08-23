//! The Deep Brain node.
//!
//! Research, causal reasoning, simulation, optimisation and learning: the path
//! that may take minutes to months and may call a language model.
//!
//! It runs the intelligence loop. What it does *not* do is reach a venue: order
//! submission belongs to the execution path, and the agents this node hosts are
//! checked at start-up to confirm none of them holds a market-touching
//! capability.
//!
//! This node runs a cycle and exits, which makes it the binary with the most to
//! lose from having nowhere to write: without a configured store, every run
//! starts from nothing and the research it did is gone the moment it finishes.
//! What it keeps is the event log's hash chain, appended after the cycle so
//! successive runs accumulate into one record. What it does not keep is the
//! world model and the agent working state — those are derived from the chain
//! and from the universe, and a half-restored model is harder to reason about
//! than one rebuilt.

use qip_core::error::{Error, Result};
use qip_core::{Clock, SystemClock};
use qip_financial::universe::Universe;
use qip_investment_agents::manifests;
use qip_kernel::{Platform, PlatformConfig};
use qip_observability::Telemetry;
use qip_risk::limits::LimitSet;
use qip_storage::ChainArchive;
use qip_storage::settings::StorageSettings;
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

    // Before the platform is built, so a node deployed against a store it
    // cannot write to fails without having done any research it would then
    // throw away.
    let storage = StorageSettings::from_env()?;
    storage.preflight()?;
    let archive = ChainArchive::open(storage.key_value("event-log")?)?;

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
    for line in storage.banner_lines(
        &["the event log's hash chain, appended after the cycle"],
        &[
            "the world model and every agent's working state",
            "the opportunity queue",
        ],
    ) {
        println!("{line}");
    }
    println!("  event chain:      {}", archive.describe());

    let report = platform.run_cycle(clock.now());
    println!();
    println!("{}", report.summarise());

    // After the cycle, not during it: a disk on the path of every event would
    // put a storage system's latency inside the loop. What that costs is the
    // events of a cycle that was interrupted.
    let archived = archive.absorb(platform.event_log().records())?;
    println!();
    println!(
        "archived {archived} event(s); the chain now holds {}",
        archive.describe()
    );
    Ok(())
}

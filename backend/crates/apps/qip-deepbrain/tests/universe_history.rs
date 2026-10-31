//! R12-TICK-2090: the evolution engine judges a candidate against the
//! platform's historical universe — the names and venues that existed at the
//! backtest's own instants — and not against the present universe alone.
//! `EvolutionEngine::turn` takes the platform's history at the start of every
//! round; this is the pin on that handoff, which `node::run_with` is the only
//! production caller of.

#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_core::error::Result;
use qip_core::{Clock, Context, Duration, ManualClock, Timestamp};
use qip_deepbrain::config::DeepBrainConfig;
use qip_deepbrain::evolution::{EvolutionConfig, EvolutionEngine};
use qip_deepbrain::node::{self, Loops};
use qip_deepbrain::reference::synthetic_universe;
use qip_deepbrain::status::NodeStatus;
use qip_kernel::{Platform, PlatformConfig};
use qip_market_ingestion::synthetic::{EnvironmentConfig, SyntheticEnvironment};
use qip_observability::Telemetry;
use qip_risk::limits::LimitSet;
use qip_storage::{ChainArchive, MemoryKeyValueStore};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

fn start() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

/// Mutation: drop the `self.universe_history = platform.universe_history()
/// .clone()` line at the top of `EvolutionEngine::turn`; the engine's history
/// stays empty after the rounds and the final assertion fails.
#[test]
fn an_evolution_round_judges_candidates_against_the_platforms_historical_universe() -> Result<()> {
    let manual = Arc::new(ManualClock::new(start()));
    let clock: Arc<dyn Clock> = manual.clone();
    let config = PlatformConfig::default();
    let seed = config.seed;
    let environment = SyntheticEnvironment::demo(
        start(),
        EnvironmentConfig {
            seed,
            step: Duration::from_mins(1),
            ..EnvironmentConfig::default()
        },
    );
    let universe = synthetic_universe(&environment, start())?;
    let expected: Vec<String> = universe.ids().map(|id| id.as_str().to_string()).collect();
    assert!(
        !expected.is_empty(),
        "the premise: the synthetic universe lists instruments"
    );
    let context = Context::new(clock.clone(), seed);
    let mut platform = Platform::new(
        config,
        context,
        Telemetry::silent(),
        universe,
        LimitSet::conservative_default(),
    )?;
    let listed: Vec<String> = platform
        .universe_history()
        .members_as_of(start())
        .iter()
        .map(|listing| listing.object_id.as_str().to_string())
        .collect();
    assert_eq!(
        listed, expected,
        "the premise: the platform lists every assembled instrument from the assembly instant"
    );

    let node_config = DeepBrainConfig {
        cycle_interval: Duration::from_millis(1),
        max_cycles: Some(12),
        ..DeepBrainConfig::default()
    };
    let mut engine = EvolutionEngine::over_synthetic(
        EvolutionConfig {
            every_cycles: 1,
            minimum_bars: 2,
            candidates: 2,
            challengers: 1,
            ..EvolutionConfig::default()
        },
        environment,
        seed,
        start(),
    )?;
    assert!(
        engine.universe_history().members_as_of(start()).is_empty(),
        "the premise: before a round the engine holds no history"
    );
    let roster = qip_deepbrain::roster::clear(start())?;
    let status = Arc::new(Mutex::new(NodeStatus::opening(
        &roster,
        &node_config,
        start(),
    )));
    let archive = ChainArchive::open(Arc::new(MemoryKeyValueStore::default()))?;
    let mut turned = 0;
    node::run_with(
        &mut platform,
        &archive,
        &node_config,
        &status,
        &Arc::new(AtomicBool::new(false)),
        &clock,
        0,
        Some(&mut engine),
        Loops {
            desk: None,
            research: None,
            operations: None,
            tick: None,
            permissions: None,
            ambient: None,
            agency: None,
            warehouse: None,
            backlog: None,
        },
        |outcome| {
            manual.advance(Duration::from_mins(1));
            if outcome.evolution.is_some() {
                turned += 1;
            }
        },
    )?;
    assert!(
        turned >= 1,
        "the premise: at least one evolution round turned"
    );
    let held: Vec<String> = engine
        .universe_history()
        .members_as_of(start())
        .iter()
        .map(|listing| listing.object_id.as_str().to_string())
        .collect();
    assert_eq!(
        held, expected,
        "the engine did not take the platform's historical universe for its round"
    );
    Ok(())
}

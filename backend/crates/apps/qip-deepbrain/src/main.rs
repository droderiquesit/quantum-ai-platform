//! The Deep Brain node.
//!
//! Research, causal reasoning, simulation, optimisation and learning: the path
//! that may take minutes and may call a language model.
//!
//! It runs the intelligence loop. What it does *not* do is reach a venue: order
//! submission belongs to the execution path, and the agents this node hosts are
//! checked at start-up to confirm none of them holds a market-touching
//! capability. That check is the first thing this binary does and everything
//! else happens behind it — the node validates, and only then runs.
//!
//! It also refuses to start without a store it can actually write to. The
//! configuration is resolved and proven before the listener is bound, because a
//! deployment that believed it was durable and was not passes every smoke test
//! it has and discovers the truth at the restart.
//!
//! # What it does once it is running
//!
//! Runs a cycle, hands the event log to the chain archive, waits out the rest
//! of the cadence, and repeats. The cadence is minutes rather than
//! milliseconds, and unlike the fast brain this node has *no ceiling on a
//! cycle*: a long cycle here is a deep analysis, so an overrun is counted and
//! printed and is never a fault, never a reason to fail a probe, and never a
//! reason to leave rotation. What can take it out of rotation is having
//! produced nothing at all — see `qip_deepbrain::status::Unready`.
//!
//! The health surface is started *before* the platform is assembled, which is
//! the reverse of the fast brain's order and is deliberate: assembling this
//! platform is not instant, and an orchestrator that probed a node during its
//! own start-up should be told it is alive and warming rather than getting a
//! refused connection.
//!
//! # Stopping it
//!
//! `POST /quiesce` from the node itself, or a configured cycle or time bound.
//! Either way the loop finishes the cycle in flight, stops, and hands the event
//! log to the chain archive. The wait between cycles is interruptible, so a
//! quiesce lands within the cycle in flight rather than within the cadence —
//! which at five minutes would outlast the pod's termination grace period.
//! There is no signal handler: this build has no dependency that could install
//! one, so a `SIGTERM` ends the process where it stands and whatever has not
//! reached the archive is lost. That is why the archive runs after every cycle,
//! and why a pre-stop hook should quiesce.

use qip_core::error::{Error, Result};
use qip_core::{Clock, SystemClock};
use qip_deepbrain::config::DeepBrainConfig;
use qip_deepbrain::{health, node, roster};
use qip_financial::universe::Universe;
use qip_kernel::{Platform, PlatformConfig};
use qip_observability::Telemetry;
use qip_risk::limits::LimitSet;
use qip_risk_engine::autonomy::AutonomyLevel;
use qip_storage::ChainArchive;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

/// Exit code for a configuration problem, matching `sysexits.h`.
///
/// Distinct from a general failure so an orchestrator can tell "this node was
/// deployed wrong" from "this node broke", and stop restarting the first.
const EX_CONFIG: i32 = 78;

fn main() {
    match run() {
        Ok(()) => {}
        Err(error) if error.message().starts_with("configuration:") => {
            eprintln!("qip-deepbrain: {}", error.message());
            std::process::exit(EX_CONFIG);
        }
        Err(error) => {
            eprintln!("qip-deepbrain: {}", error.message());
            std::process::exit(1);
        }
    }
}

fn run() -> Result<()> {
    // The clock is read once, here, at the boundary. Everything inside takes a
    // timestamp as a parameter, which is what makes a session replayable.
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);
    let started = clock.now();

    // First, and before anything else exists to be undone.
    let cleared = roster::clear(started)?;

    let config = DeepBrainConfig::from_env()?;
    config
        .storage
        .preflight()
        .map_err(|error| Error::invalid(format!("configuration: {}", error.message())))?;
    let archive = ChainArchive::open(config.storage.key_value("event-log")?)?;

    // Bound before the platform is assembled: a busy port is a deployment
    // mistake, and finding it after building a platform wastes the start-up.
    let listener = health::bind(&config.health_address)?;
    let bound = listener
        .local_addr()
        .map_err(|error| Error::io(format!("the health listener has no address: {error}")))?;

    // Built here rather than inline at `Platform::new` below, because the
    // health thread starts before the platform does and the scrape surface it
    // serves has to read the same registry the cycle will write to. A second
    // registry made for the health thread would answer every scrape with an
    // empty surface forever, while the platform recorded diligently into one
    // nothing could reach.
    let telemetry = Telemetry::new("qip-deepbrain", clock.clone());
    let metrics = telemetry.metrics.clone();

    let status = Arc::new(Mutex::new(
        qip_deepbrain::status::NodeStatus::opening(&cleared, &config, started)
            .with_metrics(metrics),
    ));
    let stop = Arc::new(AtomicBool::new(false));

    // Serving starts here, before the platform exists. Until the first cycle
    // lands the status reports `warming`, which is exactly what an orchestrator
    // should see: alive, not yet worth consulting.
    {
        let status = status.clone();
        let stop = stop.clone();
        let clock = clock.clone();
        std::thread::Builder::new()
            .name("qip-deepbrain-health".to_string())
            .spawn(move || health::serve(&listener, &status, &stop, &clock))
            .map_err(|error| Error::io(format!("cannot start the health thread: {error}")))?;
    }

    // The kernel is told where its event log goes, so a deployment with a
    // durable path gets a chain that continues across a restart of this process
    // rather than one that begins again at sequence one.
    // The ceiling, read here for the first time — see the same block in
    // qip-fastbrain: `deepbrain.yaml` set QIP_AUTONOMY_CEILING and this binary
    // never read it, so the ConfigMap presented a control that did nothing.
    // `deployable` refuses a live level rather than quietly lowering it.
    let platform_config = PlatformConfig::default()
        .with_event_log(config.event_log.clone())
        .with_live_ceiling(AutonomyLevel::deployable(
            std::env::var("QIP_AUTONOMY_CEILING").ok().as_deref(),
        )?);
    let context = qip_core::Context::new(clock.clone(), platform_config.seed);
    let ceiling = platform_config.autonomy_ceiling.to_string();
    let mut platform = Platform::new(
        platform_config,
        context,
        telemetry,
        Universe::new(),
        LimitSet::conservative_default(),
    )?;

    // The trust root, before anything is served: install the operator's
    // envelope key when the deployment provides one, and refuse to run
    // live-capable on the seed-derived default. See `trust.rs` for why a
    // refusal and not a warning.
    //
    // Read through `qip_core::secret`, so the deployment may supply the key in
    // a file rather than in the process environment. That is what the Secret
    // Manager CSI driver projects into the pod, and a signing key in
    // `/proc/<pid>/environ` is one every child process and every crash dump
    // also has.
    let envelope_key =
        qip_core::secret::from_environment(qip_deepbrain::trust::ENVELOPE_KEY_VARIABLE)
            .map_err(|error| Error::invalid(format!("configuration: {}", error.message())))?;
    let provenance =
        qip_deepbrain::trust::harden_central(&mut platform, envelope_key.as_deref())
            .map_err(|error| Error::invalid(format!("configuration: {}", error.message())))?;

    // The durable trial book, on the same storage the event log archives to.
    // The factory the plane was built with charges holdout evaluations to an
    // in-process book, so until this call every restart forgot every
    // family's lifetime trial count — the per-run accounting the deflated
    // Sharpe gate is corrected against and the one the blueprint forbids.
    // Opened after the plane is hardened, and carried across by
    // `set_central` if it ever were not. A journal that does not verify
    // stops the process here: a count rebuilt over a broken chain is the
    // understated count the chain exists to catch.
    platform
        .open_trial_book(config.storage.key_value("trial-book")?, "trial-book")
        .map_err(|error| Error::invalid(format!("configuration: {}", error.message())))?;

    // Read once, immediately after assembly, and carried through the run. It is
    // the boundary between what this process inherited from a previous run's
    // log and what it is itself accountable for handing to the archive.
    let inherited = node::restored_through(platform.event_log().records());

    banner(
        provenance, &config, &cleared, &platform, &ceiling, bound, &archive, inherited,
    );

    // The evolution engine, and the research node's first data source. The
    // adapter is the synthetic exchange, seeded from the platform so a session
    // reproduces; QIP_DEEPBRAIN_REPLAY_PATH swaps in a recording. The engine
    // also feeds Platform::observe — before it, this node ran every cycle
    // blind and its own cycle lines said so.
    let evolution_config =
        qip_deepbrain::evolution::EvolutionConfig::from_lookup(&|name| std::env::var(name).ok())
            .map_err(|error| Error::invalid(format!("configuration: {}", error.message())))?;
    // The match produces the engine rather than a boxed adapter, because the
    // synthetic branch needs the environment *before* it is boxed: the
    // reference universe is derived from the exchange's own instrument list,
    // and once the environment is behind `dyn DataAdapter` that list is
    // unreachable.
    let mut evolution = match std::env::var("QIP_DEEPBRAIN_REPLAY_PATH").ok().as_deref() {
        Some(path) => qip_deepbrain::evolution::EvolutionEngine::new(
            evolution_config,
            Box::new(qip_market_ingestion::replay::ReplayAdapter::open(
                "replay", path,
            )?),
            platform.config().seed,
            // The replay path has no reference-data source — a tape carries
            // bars, not listings — so this is empty and the loop's backtests
            // refuse every candidate with "no fill" rather than register a
            // flat equity curve as evidence. That refusal is the point:
            // before it, an empty universe rejected every order silently and
            // the gate scored the resulting flat line as a real holdout. A
            // reference source derived from the tape's own instruments is
            // what would turn the loop on here, and until then the replay
            // path is visibly off rather than invisibly producing nothing.
            Universe::new(),
        )?,
        None => {
            // The bar interval must match the step, or a fast cadence
            // closes a bar every sixty cycles and the node runs blind for
            // hours while looking configured — the trap the fast brain's
            // feed documents, walked into here once before this comment.
            let synthetic = qip_market_ingestion::synthetic::EnvironmentConfig {
                seed: platform.config().seed,
                step: config.cycle_interval,
                bar_interval: if config.cycle_interval < qip_core::Duration::from_mins(1) {
                    qip_market::bar::Interval::Second
                } else {
                    qip_market::bar::Interval::Minute
                },
                ..qip_market_ingestion::synthetic::EnvironmentConfig::default()
            };
            // One instrument list for prices and reference data alike, with
            // provenance and licensing stamped synthetic. Deriving the
            // universe from the environment rather than declaring a second
            // one is what keeps the two from drifting — a listing the
            // exchange does not price, or a price the universe does not
            // list, would each turn the loop quietly off again.
            qip_deepbrain::evolution::EvolutionEngine::over_synthetic(
                evolution_config,
                qip_market_ingestion::synthetic::SyntheticEnvironment::demo(clock.now(), synthetic),
                platform.config().seed,
                clock.now(),
            )?
        }
    };
    println!(
        "  evolution:        {}",
        if evolution.enabled() {
            "searching on its cadence; candidates register at the bottom rung and never promote themselves"
        } else {
            "disabled (QIP_DEEPBRAIN_EVOLUTION_EVERY=0)"
        }
    );

    let summary = node::run(
        &mut platform,
        &archive,
        &config,
        &status,
        &stop,
        &clock,
        inherited,
        Some(&mut evolution),
        |outcome| {
            println!();
            println!("{}", outcome.report.summarise());
            if let Some(round) = &outcome.evolution {
                println!("  {}", round.describe());
            }
            if let Some(round) = &outcome.learning {
                println!("  {}", round.describe());
                // Named individually, not summed. An operator needs to know
                // *which* model has moved away from what it was fitted on, and
                // which feature carried it there.
                for observation in &round.drift {
                    if observation.above_threshold {
                        println!(
                            "    drift: {} is at {:.3} on {}, past its threshold",
                            observation.reference,
                            observation.population_stability_index,
                            observation.worst_feature
                        );
                    }
                }
            }
            println!(
                "  {:>10} {:>4}  {}s against a {}s cadence{}",
                "elapsed",
                "",
                outcome.elapsed.as_secs_f64(),
                config.cycle_interval.as_secs_f64(),
                if outcome.overran_the_interval {
                    "  (over the cadence; the next cycle starts immediately)"
                } else {
                    ""
                }
            );
        },
    )?;

    println!();
    println!(
        "qip-deepbrain stopping: {}",
        summary.stopped_because.as_str()
    );
    println!(
        "  cycles:           {} ({} did not traverse every stage)",
        summary.cycles, summary.failed_cycles
    );
    println!(
        "  cadence:          {} cycle(s) ran past the {}s interval, longest {}s",
        summary.overruns,
        config.cycle_interval.as_secs_f64(),
        summary.longest_cycle.as_secs_f64()
    );
    println!(
        "  archived so far:  {} record(s) handed over between cycles",
        summary.archived_while_running
    );

    let flushed = node::flush(
        &platform,
        &archive,
        config.storage.is_durable(),
        config.shutdown_budget,
        inherited,
    )?;
    println!("  shutdown:         {}", flushed.describe());
    Ok(())
}

/// What this process will do, before it does any of it.
///
/// Everything an operator would otherwise have to infer from behaviour: which
/// guarantee was checked, what this node will not do, whether the run stops on
/// its own, where the evidence goes, and what a restart takes away.
fn banner(
    provenance: qip_deepbrain::trust::KeyProvenance,
    config: &DeepBrainConfig,
    cleared: &roster::ClearedRoster,
    platform: &Platform,
    ceiling: &str,
    bound: std::net::SocketAddr,
    archive: &ChainArchive,
    inherited: u64,
) {
    println!("qip-deepbrain health on {bound}");
    println!("  autonomy ceiling: {ceiling}");
    println!("  envelope key:     {}", provenance.describe());
    println!("  agents:           {}", platform.organisation().len());
    println!(
        "  live trading:     {}",
        if platform.is_live_capable() {
            "reachable"
        } else {
            "unreachable in this deployment"
        }
    );
    println!(
        "  hosting:          {} agent(s), of which {} may consult a language model",
        cleared.agents.len(),
        cleared.model_callers()
    );
    println!(
        "  not hosting:      {} — this node reaches no venue",
        cleared.excluded.join(", ")
    );
    println!(
        "  cycle:            one every {}s, no ceiling — a long cycle here is research, not a \
         fault",
        config.cycle_interval.as_secs_f64()
    );
    println!(
        "  run:              {}",
        match (config.max_cycles, config.max_runtime) {
            (Some(cycles), _) => format!("stops after {cycles} cycle(s)"),
            (None, Some(runtime)) => format!("stops after {}s", runtime.as_secs_f64()),
            (None, None) => "until quiesced on loopback".to_string(),
        }
    );
    println!("  event log:        {}", config.event_log.describe());
    if inherited > 0 {
        println!(
            "  continuing:       {inherited} record(s) read back from the log; this run's chain \
             carries on from there rather than starting again"
        );
    }
    for line in config.storage.banner_lines(
        &["the event log's hash chain, after every cycle and once on the way out"],
        &[
            "the world model, the opportunity queue and every agent's working state, which are \
             rebuilt from the chain and the universe",
            "the cycle in flight, if this process is killed rather than quiesced",
        ],
    ) {
        println!("{line}");
    }
    println!("  event chain:      {}", archive.describe());
    if let Some(note) = config.durability_note() {
        println!("  note:             {note}");
    }
}

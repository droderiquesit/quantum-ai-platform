//! The Fast Brain node.
//!
//! Market data, microstructure, real-time risk and execution: the path that
//! must answer in microseconds to milliseconds.
//!
//! The rule this node exists to enforce is that **nothing on the fast path
//! waits for a language model**. It is checked at start-up rather than assumed:
//! the node refuses to run if any agent it would host holds
//! `call_language_model` or has a budget permitting one. A fast path that
//! blocks on a model call is not a fast path, and discovering that under load
//! is expensive. That check is the first thing this binary does and everything
//! else happens behind it — the node validates, and only then runs.
//!
//! It also refuses to start without a store it can actually write to. The
//! configuration is resolved and proven before the listener is bound, because a
//! deployment that believed it was durable and was not passes every smoke test
//! it has and discovers the truth at the restart.
//!
//! # What it does once it is running
//!
//! Polls a feed, hands the platform what passed validation, runs a cycle, and
//! repeats on a clock. Each cycle is timed against the fast-path ceiling — the
//! same ceiling the roster check enforces — because a guarantee checked once at
//! start-up is a guarantee that drifts. A breach is counted and printed; a run
//! of them takes the node out of rotation without taking it down, which is the
//! honest response to a node that is alive and is not fast.
//!
//! # Stopping it
//!
//! `POST /quiesce` from the node itself, or a configured cycle or time bound.
//! Either way the loop finishes the cycle in flight, stops, and hands the event
//! log to the chain archive. There is no signal handler: this build has no
//! dependency that could install one, so a `SIGTERM` ends the process where it
//! stands and whatever has not reached the archive is lost. That is why the
//! archive also runs between cycles, and why a pre-stop hook should quiesce.

use qip_core::error::{Error, Result};
use qip_core::{Clock, SystemClock};
use qip_fastbrain::config::FastBrainConfig;
use qip_fastbrain::feed::Feed;
use qip_fastbrain::{health, node, roster};
use qip_financial::universe::Universe;
use qip_kernel::{Platform, PlatformConfig};
use qip_observability::Telemetry;
use qip_risk::limits::LimitSet;
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
            eprintln!("qip-fastbrain: {}", error.message());
            std::process::exit(EX_CONFIG);
        }
        Err(error) => {
            eprintln!("qip-fastbrain: {}", error.message());
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

    let config = FastBrainConfig::from_env()?;
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

    let mut feed = Feed::open(
        config.replay_path.as_deref(),
        config.seed,
        config.cycle_interval,
        started,
    )
    .map_err(|error| Error::invalid(format!("configuration: {}", error.message())))?;

    let platform_config = PlatformConfig::default();
    let context = qip_core::Context::new(clock.clone(), platform_config.seed);
    let ceiling = platform_config.autonomy_ceiling.to_string();
    let mut platform = Platform::new(
        platform_config,
        context,
        Telemetry::new("qip-fastbrain", clock.clone()),
        Universe::new(),
        LimitSet::conservative_default(),
    )?;

    // The trust root, before anything is served: install the operator's
    // envelope key when the deployment provides one, and refuse to run
    // live-capable on the seed-derived default. See `trust.rs` for why a
    // refusal and not a warning.
    let provenance = qip_fastbrain::trust::harden_central(
        &mut platform,
        std::env::var(qip_fastbrain::trust::ENVELOPE_KEY_VARIABLE)
            .ok()
            .as_deref(),
    )
    .map_err(|error| Error::invalid(format!("configuration: {}", error.message())))?;

    let status = Arc::new(Mutex::new(qip_fastbrain::status::NodeStatus::opening(
        &cleared,
        &config,
        feed.descriptor().name,
        feed.is_production_grade(),
        started,
    )));
    let stop = Arc::new(AtomicBool::new(false));

    banner(
        provenance, &config, &cleared, &feed, &platform, &ceiling, bound, &archive,
    );

    // One thread for the listener, blocking, no async runtime. It reads the
    // status the loop writes and never takes a lock the loop holds for longer
    // than an update, so it answers while a cycle is running — which is the
    // only way it can report a cycle that is stuck.
    {
        let status = status.clone();
        let stop = stop.clone();
        let clock = clock.clone();
        std::thread::Builder::new()
            .name("qip-fastbrain-health".to_string())
            .spawn(move || health::serve(&listener, &status, &stop, &clock))
            .map_err(|error| Error::io(format!("cannot start the health thread: {error}")))?;
    }

    let summary = node::run(
        &mut platform,
        &mut feed,
        &archive,
        &config,
        &status,
        &stop,
        &clock,
        |outcome| {
            println!();
            println!("{}", outcome.report.summarise());
            println!(
                "  {:>10} {:>4}  {}us against a {}ms ceiling{}",
                "elapsed",
                "",
                outcome.elapsed.as_nanos() / 1_000,
                config.cycle_budget.as_millis(),
                if outcome.over_budget { "  BREACH" } else { "" }
            );
            for rejection in &outcome.rejections {
                println!("             !  {rejection}");
            }
        },
    )?;

    println!();
    println!(
        "qip-fastbrain stopping: {}",
        summary.stopped_because.as_str()
    );
    println!(
        "  cycles:           {} ({} record(s) observed, {} rejected)",
        summary.cycles, summary.observed, summary.rejected
    );
    println!(
        "  fast-path budget: {} breach(es), worst cycle {}us against a {}ms ceiling",
        summary.breaches,
        summary.worst_cycle.as_nanos() / 1_000,
        config.cycle_budget.as_millis()
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
    )?;
    println!("  shutdown:         {}", flushed.describe());
    Ok(())
}

/// What this process will do, before it does any of it.
///
/// Everything an operator would otherwise have to infer from behaviour: which
/// guarantee was checked, what the feed is and is not, whether the run stops on
/// its own, and what a restart takes away.
fn banner(
    provenance: qip_fastbrain::trust::KeyProvenance,
    config: &FastBrainConfig,
    cleared: &roster::ClearedRoster,
    feed: &Feed,
    platform: &Platform,
    ceiling: &str,
    bound: std::net::SocketAddr,
    archive: &ChainArchive,
) {
    println!("qip-fastbrain health on {bound}");
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
    for agent in &cleared.agents {
        println!(
            "  {}: {:?} budget, {} tool call(s), no language model",
            agent.id, agent.wall_time, agent.tool_calls
        );
    }
    println!("  every hosted agent is model-free and inside the fast-path budget");
    println!(
        "  cycle:            one every {}ms, ceiling {}ms, breach tolerance {}",
        config.cycle_interval.as_millis(),
        config.cycle_budget.as_millis(),
        config.breach_tolerance
    );
    println!(
        "  run:              {}",
        match (config.max_cycles, config.max_runtime) {
            (Some(cycles), _) => format!("stops after {cycles} cycle(s)"),
            (None, Some(runtime)) => format!("stops after {}s", runtime.as_secs_f64()),
            (None, None) => "until quiesced on loopback, or the feed runs out".to_string(),
        }
    );
    println!(
        "  feed:             {} ({})",
        feed.descriptor().name,
        if feed.is_production_grade() {
            "production-grade"
        } else {
            "NOT production-grade; no capital decision may rest on it"
        }
    );
    if let Some(requirement) = feed.production_requirement() {
        println!("  awaiting:         {requirement}");
    }
    for line in config.storage.banner_lines(
        &["the event log's hash chain, between cycles and once on the way out"],
        &[
            "the market view, price history and feature state, which are rebuilt from the feed",
            "any cycle since the last hand-over, if this process is killed rather than quiesced",
        ],
    ) {
        println!("{line}");
    }
    println!("  event chain:      {}", archive.describe());
}

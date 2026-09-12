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
use qip_kernel::central::{CentralConfig, HorizonPolicy};
use qip_kernel::{Platform, PlatformConfig};
use qip_observability::Telemetry;
use qip_risk::limits::LimitSet;
use qip_risk_engine::autonomy::AutonomyLevel;
use qip_storage::ChainArchive;
use qip_storage::settings::StorageSettings;
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
    // A second handle on the same three `Arc`s, taken for the same reason the
    // registry handle above is: the drain thread must read the registry the
    // cycle writes to. A `Telemetry::new` of its own would export an empty
    // surface for ever while the platform recorded into one nothing could
    // reach — the defect the comment above describes, one level up again.
    let telemetry_for_export = telemetry.clone();

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
    //
    // The §23.4 horizon policy, read here for the same reason: nothing in
    // this workspace ever called `PlatformConfig::with_central` outside a
    // test (`grep -n 'with_central' backend/crates/apps -r` found only
    // `qip-api/tests/mesh.rs`), so `CentralPlane::arm_horizons` refused to
    // arm on every deployed cycle for want of a policy rather than for want
    // of a claim. Overlaid onto `CentralConfig::default()` rather than
    // replacing it — this node states no view on the whole-book budget or
    // the drawdown schedule, and a file that had to restate every field of
    // `CentralConfig` to change one of them would be an invitation to drift
    // the two apart. Absent, this changes nothing; malformed, it stops the
    // process rather than arming the gate on a policy nobody actually
    // stated.
    let central = CentralConfig {
        horizons: load_central_horizons()?,
        ..CentralConfig::default()
    };
    let platform_config = PlatformConfig::default()
        .with_event_log(config.event_log.clone())
        .with_central(central)
        .with_live_ceiling(AutonomyLevel::deployable(
            std::env::var("QIP_AUTONOMY_CEILING").ok().as_deref(),
        )?);
    // A committed tape, opened before the platform exists because the
    // platform's clock has to be the tape's: opportunities expire at tape
    // time, and a router asked for a latency budget from the wall clock
    // against a deadline in 2025 would refuse every panel as already late.
    // Refused beside a replay — two recordings on two clocks — and refused
    // when it outlasts the roster's review interval, for the reason
    // `roster::refuse_tape_beyond_review` gives.
    let replay_path = std::env::var("QIP_DEEPBRAIN_REPLAY_PATH")
        .ok()
        .filter(|value| !value.trim().is_empty());
    let mut tape_feed = match (&config.tape_path, &replay_path) {
        (Some(_), Some(_)) => {
            return Err(Error::invalid(
                "configuration: both QIP_DEEPBRAIN_REPLAY_PATH and QIP_DEEPBRAIN_TAPE_PATH are \
                 set. A replay runs on the wall clock and a tape on its own; unset one of them",
            ));
        }
        (Some(path), None) => {
            let tape = qip_market_ingestion::tape::Tape::open(path)
                .map_err(|error| Error::invalid(format!("configuration: {}", error.message())))?;
            roster::refuse_tape_beyond_review(&tape, started)
                .map_err(|error| Error::invalid(format!("configuration: {}", error.message())))?;
            Some(qip_market_ingestion::tape::TapeFeed::new(tape))
        }
        (None, _) => None,
    };
    // Everything operational — the health surface, the run bound, telemetry
    // timestamps — stays on the wall clock, which is what an operator is on.
    let platform_clock: Arc<dyn Clock> = match &tape_feed {
        Some(feed) => feed.clock(),
        None => clock.clone(),
    };
    let context = qip_core::Context::new(platform_clock, platform_config.seed);
    let ceiling = platform_config.autonomy_ceiling.to_string();
    // The universe this node sizes against, read and journaled before the
    // platform exists — see `load_universe` for why an unset path is a
    // refusal and not an empty universe.
    let catalogue = load_universe(&config.storage, started)?;
    // The language model the organisation narrates through (ADR 0037): the
    // hosted adapter first when the configuration names one *and* the
    // preconditions in `language.rs` are met — a provider pinned, its terms
    // attested by a named operator, a credential resolved — and the
    // deterministic model in every case. Assembled here, in the one root
    // permitted to, and handed to the kernel rather than reached for inside
    // it, so the kernel's own constructor stays the deterministic one every
    // other root and every test uses.
    let language_model = qip_deepbrain::language::assemble(&config)?;
    let mut platform = Platform::with_language_model(
        platform_config,
        context,
        telemetry,
        catalogue.universe,
        LimitSet::conservative_default(),
        language_model.chain.clone(),
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

    // The OpenObserve drain (ADR 0028): absent configuration means this
    // process's telemetry stays where it already is, on /metrics on the health
    // port bound above. Set means a thread starts that POSTs this node's
    // metrics and spans on an interval. Refused here, before the banner, so a
    // deployment that got the configuration wrong exits 78 rather than
    // starting and draining nowhere; and see `openobserve`'s module doc for
    // why nothing this process can reach answers on the other end today.
    let openobserve_config = qip_deepbrain::openobserve::OpenObserveConfig::from_env()?;
    // Bound rather than discarded: `DrainHandle::drop` stops the loop, so a
    // `_` pattern here would start the thread and stop it on the same line.
    // This binding must outlive `node::run` below.
    let _openobserve_drain = match &openobserve_config {
        Some(config) => Some(qip_deepbrain::openobserve::spawn(
            telemetry_for_export,
            config.clone(),
            clock.clone(),
        )?),
        None => None,
    };

    // Read once, immediately after assembly, and carried through the run. It is
    // the boundary between what this process inherited from a previous run's
    // log and what it is itself accountable for handing to the archive.
    let inherited = platform.inherited_through();

    banner(
        provenance, &config, &cleared, &platform, &ceiling, bound, &archive, inherited,
    );
    // Which model is active, by name, and never the credential: the adapter's
    // token redacts in Debug and `describe` prints only what was configured.
    // When a provider was named and withheld, this line is where an operator
    // learns which precondition is missing and which variable supplies it.
    println!("  language model:   {}", language_model.describe());
    println!(
        "  universe:         {}; sector and country buckets are fed from it. Note ADR 0027: under the \
         conservative default the first desk order into an empty book is refused by \
         sector-concentration, and the decision is the risk desk's, not this process's",
        catalogue.manifest.describe()
    );
    match &openobserve_config {
        Some(config) => println!("  openobserve:      draining to {}", config.describe()),
        None => println!(
            "  openobserve:      not draining ({} is not set); telemetry stays local to \
             /metrics on the health port",
            qip_deepbrain::openobserve::URL_VARIABLE
        ),
    }

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
    let tape_summary = tape_feed.as_ref().map(|feed| {
        let tape = feed.tape();
        let (first, last) = tape
            .span()
            .map_or((String::new(), String::new()), |(first, last)| {
                (first.to_rfc3339(), last.to_rfc3339())
            });
        format!(
            "{} observation(s) across {} instrument(s) in {} period(s), {first} to {last}; tape \
             time drives the platform clock, one period per cycle, and the cadence wait is \
             skipped",
            tape.len(),
            tape.instruments().len(),
            tape.periods()
        )
    });
    let mut evolution = match (tape_feed.take(), replay_path.as_deref()) {
        // The tape carries the catalogue's own instruments, so the reference
        // universe is the catalogue, read again for the engine: `Universe`
        // moved into the platform above, and the manifest record it writes
        // a second time is the same record.
        (Some(feed), _) => qip_deepbrain::evolution::EvolutionEngine::new(
            evolution_config,
            Box::new(feed),
            platform.config().seed,
            load_universe(&config.storage, started)?.universe,
        )?,
        (None, Some(path)) => qip_deepbrain::evolution::EvolutionEngine::new(
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
        (None, None) => {
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
    if let Some(summary) = &tape_summary {
        println!("  tape:             {summary}");
    }

    // Source discovery (§7.4-§7.6.2): `Platform::assess_sources` wraps
    // `qip_data_finder::DataFinder::assess`, which was built, tested and
    // reached by nothing outside this crate's own tests and
    // `qip-acceptance`'s end-to-end suite before this. The candidate list is
    // stated by an operator, in the same shape the universe and the
    // capital-fabric declaration already are — this node discovers nothing
    // on its own — and the probe is `NetworkProbe`, which refuses every
    // call by name until a TLS-capable transport is authorised (ADR 0009).
    let discovery_config =
        qip_deepbrain::discovery::DiscoveryConfig::from_lookup(&|name| std::env::var(name).ok())
            .map_err(|error| Error::invalid(format!("configuration: {}", error.message())))?;
    let source_candidates = load_source_candidates()?;
    println!(
        "  discovery:        {}",
        if discovery_config.every_cycles == 0 {
            "disabled (QIP_DEEPBRAIN_DISCOVER_EVERY=0)".to_string()
        } else {
            format!(
                "every {} cycle(s) against {} declared candidate(s); every probe call refuses \
                 until a TLS-capable transport is linked in (ADR 0009), so a pass records why \
                 rather than nothing",
                discovery_config.every_cycles,
                source_candidates.len()
            )
        }
    );
    evolution = evolution.with_discovery(qip_deepbrain::discovery::DiscoveryDesk::new(
        discovery_config,
        source_candidates,
    ));

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
                // The champion/challenger contest, computed every round `turn`
                // runs and until now never printed: an operator watching the
                // cycle line saw candidates registered but never learned
                // whether any of them displaced the strategy that speaks for
                // an instrument, which is the question the succession desk
                // exists to answer.
                if let Some(challenge) = &round.challenge {
                    println!("  {}", challenge.describe());
                }
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
            if let Some(assessment) = &outcome.discovery {
                println!(
                    "  discovery: {} candidate(s) assessed, {} registered, {} catalogue \
                     problem(s)",
                    assessment.decisions.len(),
                    assessment.registered(),
                    assessment.catalogue_problems.len()
                );
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
/// The instrument universe, from the committed catalogue the deployment names.
///
/// Refused when unset. Every root used to assemble `Universe::new()`, so the
/// exposure buckets the kernel projects from the universe at assembly —
/// sector, country, asset class, venue — received nothing in any deployed
/// process and the two bucket limits in the default set could never fire;
/// an empty universe is the state that hid that, and a process that fell
/// back to one on a missing variable would hide it again. The catalogue's
/// hash is recorded in the `universe` namespace of the same storage the
/// event log archives to, under its hash and as `current`, so a run can say
/// which catalogue it sized against.
fn load_universe(
    storage: &StorageSettings,
    now: qip_core::Timestamp,
) -> Result<qip_financial::LoadedCatalogue> {
    let path = std::env::var("QIP_UNIVERSE_PATH")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            Error::invalid(
                "configuration: QIP_UNIVERSE_PATH is not set. Point it at the committed instrument \
                 catalogue — data/datasets/universe.json in the repository, mounted at \
                 /etc/qip/universe.json by the deployment; this process does not start on an \
                 empty universe, because an empty universe feeds no exposure bucket and \
                 nothing would say so",
            )
        })?;
    let text = std::fs::read_to_string(&path).map_err(|error| {
        Error::io(format!(
            "configuration: QIP_UNIVERSE_PATH names {path}, which cannot be read: {error}"
        ))
    })?;
    let catalogue = qip_financial::catalogue::load(&text, now)
        .map_err(|error| Error::invalid(format!("configuration: {}", error.message())))?;
    qip_financial::catalogue::record_manifest(
        storage.key_value("universe")?.as_ref(),
        &catalogue.manifest,
    )?;
    Ok(catalogue)
}

/// The desk's §23.4 horizon policy — how the whole-book risk budget divides
/// across the four blueprint horizons, and which horizon each strategy sits
/// at — from the file `QIP_CENTRAL_HORIZONS_PATH` names.
///
/// `None` where the variable is unset or empty, which is every deployment
/// today (`grep -rn QIP_CENTRAL_HORIZONS_PATH infrastructure/environments` —
/// wired nowhere) and is `CentralPlane::arm_horizons`'s own honest answer for
/// no policy stated: it refuses to arm rather than arming an empty one. A
/// file that is present but cannot be read or does not parse as a
/// [`HorizonPolicy`] stops the process instead — the same posture
/// `load_universe` takes, because a desk that believed a policy was armed
/// and was silently running on none would find out from a cycle report, at
/// the moment the gap costs something to have missed, rather than at start-up.
fn load_central_horizons() -> Result<Option<HorizonPolicy>> {
    let path = std::env::var("QIP_CENTRAL_HORIZONS_PATH")
        .ok()
        .filter(|value| !value.trim().is_empty());
    let Some(path) = path else {
        return Ok(None);
    };
    let text = std::fs::read_to_string(&path).map_err(|error| {
        Error::io(format!(
            "configuration: QIP_CENTRAL_HORIZONS_PATH names {path}, which cannot be read: {error}"
        ))
    })?;
    parse_central_horizons(&text, &path).map(Some)
}

/// The parsing half of [`load_central_horizons`], split out so it is
/// testable without an environment variable or a file on disk — this
/// workspace forbids `unsafe`, and Rust 2024 made `std::env::set_var` unsafe,
/// so a test cannot set the variable this function's caller reads.
fn parse_central_horizons(text: &str, path: &str) -> Result<HorizonPolicy> {
    serde_json::from_str(text).map_err(|error| {
        Error::invalid(format!(
            "configuration: {path} does not hold a valid §23.4 horizon policy: {error}"
        ))
    })
}

/// The source-discovery candidate list (§7.4-§7.6.2) `QIP_DEEPBRAIN_SOURCE_CANDIDATES_PATH`
/// names, or none where the variable is unset or empty — every deployment
/// today.
///
/// A candidate the finder has not yet assessed, not a registered source: the
/// crawl stage §7.4 asks for does not exist, so nothing in this workspace
/// discovers one on its own, and this file is an operator's list of hosts
/// worth asking about, in exactly the shape `DataFinder::assess` already
/// takes. Absent, this changes nothing (an empty list, the same as every
/// deployment runs on today); present but malformed, it stops the process
/// rather than running discovery against half the candidates.
fn load_source_candidates() -> Result<Vec<qip_data_finder::source::SourceCandidate>> {
    let path = std::env::var("QIP_DEEPBRAIN_SOURCE_CANDIDATES_PATH")
        .ok()
        .filter(|value| !value.trim().is_empty());
    let Some(path) = path else {
        return Ok(Vec::new());
    };
    let text = std::fs::read_to_string(&path).map_err(|error| {
        Error::io(format!(
            "configuration: QIP_DEEPBRAIN_SOURCE_CANDIDATES_PATH names {path}, which cannot be \
             read: {error}"
        ))
    })?;
    parse_source_candidates(&text, &path)
}

/// The parsing half of [`load_source_candidates`], split out for the same
/// reason [`parse_central_horizons`] is: no test here may set the
/// environment variable the caller reads.
fn parse_source_candidates(
    text: &str,
    path: &str,
) -> Result<Vec<qip_data_finder::source::SourceCandidate>> {
    serde_json::from_str(text).map_err(|error| {
        Error::invalid(format!(
            "configuration: {path} does not hold a valid source-candidate list: {error}"
        ))
    })
}

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

#[cfg(test)]
mod tests {
    //! `load_central_horizons` is the parser and the file read; this proves
    //! the field it fills actually changes what `Platform::arm_horizon_gate`
    //! does, not only that a document parses. Before this composition root
    //! read the variable, `grep -n with_central backend/crates/apps -r`
    //! found only `qip-api/tests/mesh.rs` — no production caller anywhere —
    //! so `CentralPlane::arm_horizons` refused to arm on every deployed
    //! cycle for want of a policy, which is the state `unconfigured` below
    //! reproduces as the premise the rest of the test contrasts against.

    // The workspace denies `panic_in_result_fn` for production code; in a
    // test the assertion is the deliverable and `?` keeps the setup readable.
    #![allow(clippy::panic_in_result_fn)]

    use super::*;
    use qip_core::Context;
    use qip_financial::universe::Universe;
    use qip_risk::limits::LimitSet;

    fn start() -> qip_core::Timestamp {
        qip_core::Timestamp::from_secs(1_760_000_000)
    }

    fn platform_with(central: CentralConfig) -> Result<Platform> {
        let config = PlatformConfig::default().with_central(central);
        let (context, _clock) = Context::deterministic(start(), config.seed);
        Platform::new(
            config,
            context,
            Telemetry::silent(),
            Universe::new(),
            LimitSet::conservative_default(),
        )
    }

    #[test]
    fn a_document_load_central_horizons_would_parse_arms_the_pool_gate() -> Result<()> {
        let mut unconfigured = platform_with(CentralConfig::default())?;
        assert!(
            unconfigured.arm_horizon_gate(start())?.is_none(),
            "the premise failed: an unconfigured platform already had a horizon policy"
        );

        // The document `load_central_horizons` reads and parses, written as
        // an operator would write it rather than built through the Rust
        // type — one bucket holding the whole default budget, and one claim,
        // because a policy naming no strategy is refused separately (§23.4:
        // every promotion to a capital-holding rung needs a claim to check
        // against). Decimals as bare JSON integers, matching the shape
        // `Decimal`'s own `Deserialize` accepts and `refuse_inexact_numbers`
        // polices elsewhere in this same declaration family.
        let text = r#"{
            "available_inventory": 10000000,
            "deployable_capital": 0,
            "capital_not_reserved_for_calls": 0,
            "reserved_capital": 0,
            "claims": [
                { "strategy": "strat-test", "source": "test", "horizon": "hours_to_days" }
            ],
            "despite": null
        }"#;
        let parsed = parse_central_horizons(text, "test-fixture")
            .expect("the document this test wrote parses");

        let mut configured = platform_with(CentralConfig {
            horizons: Some(parsed),
            ..CentralConfig::default()
        })?;
        assert!(
            configured.arm_horizon_gate(start())?.is_some(),
            "a stated horizon policy did not arm the gate"
        );
        Ok(())
    }

    #[test]
    fn a_horizon_document_that_is_not_json_is_refused_by_name_rather_than_silently_leaving_the_gate_unarmed()
     {
        let error = parse_central_horizons("not json", "/etc/qip/horizons.json")
            .expect_err("malformed JSON parsed as a horizon policy");
        assert!(
            error.message().contains("/etc/qip/horizons.json"),
            "the refusal does not name the file that failed to parse: {}",
            error.message()
        );
        assert!(
            error.message().contains("§23.4"),
            "the refusal does not say what kind of document was expected: {}",
            error.message()
        );
    }

    #[test]
    fn a_document_load_source_candidates_would_parse_reaches_assess_sources() -> Result<()> {
        use qip_contracts::governance::Usage;
        use qip_core::Currency;
        use qip_data_finder::coverage::{SourceCoverage, SourceRegion, UpdateFrequency};
        use qip_data_finder::endpoint::{AccessMechanism, AuthRequirement, SourceEndpoint};
        use qip_data_finder::legal::{LicensingPosture, SourceLicense};
        use qip_data_finder::quality::SourceCost;
        use qip_data_finder::source::{SourceCandidate, SourceIdentity};
        use qip_events::Topic;
        use qip_financial::asset_class::AssetClass;

        // Written through the Rust type and re-serialised, rather than typed
        // as a JSON literal, so this test cannot drift from `SourceCandidate`'s
        // own shape the way a hand-copied fixture could.
        let candidate = SourceCandidate::new(
            SourceIdentity::new("test-source", "test feed", "Example Data Ltd")?,
            SourceEndpoint::parse(
                "https://test-source.example/quotes",
                AccessMechanism::Rest {
                    auth: AuthRequirement::None,
                    incremental_parameter: None,
                    page_size: 100,
                },
            )?,
            SourceCoverage::new(
                [AssetClass::Equity],
                [SourceRegion::Europe],
                ["EU0001".to_string()],
                UpdateFrequency::Minutely,
            )?,
            LicensingPosture::declared(SourceLicense::new(
                "qip-discovery-test-terms",
                [Usage::Derive],
            )?),
            SourceCost::free(Currency::USD),
            SourceRegion::Europe,
            [Topic::MarketQuote],
            "test",
            start(),
        )?;
        let text = serde_json::to_string(&vec![candidate]).expect("a candidate list serialises");
        let parsed = parse_source_candidates(&text, "test-fixture")
            .expect("the document this test wrote parses");
        assert_eq!(
            parsed.len(),
            1,
            "one candidate went in; {} came out",
            parsed.len()
        );

        // The wiring claim: a parsed list actually reaches
        // `Platform::assess_sources`, through `DiscoveryDesk`, on its own
        // cadence -- one decision comes back per candidate even though the
        // network probe refuses every call, because a refusal is a decision
        // about the candidate and not the absence of one.
        let mut platform = platform_with(CentralConfig::default())?;
        let mut desk = qip_deepbrain::discovery::DiscoveryDesk::new(
            qip_deepbrain::discovery::DiscoveryConfig { every_cycles: 1 },
            parsed,
        );
        let assessment = desk
            .maybe_run(&mut platform, 1, start())?
            .ok_or_else(|| Error::not_found("a pass on its own cadence produced nothing"))?;
        assert_eq!(assessment.decisions.len(), 1);
        Ok(())
    }

    #[test]
    fn a_source_candidate_document_that_is_not_json_is_refused_by_name() {
        let error = parse_source_candidates("not json", "/etc/qip/candidates.json")
            .expect_err("malformed JSON parsed as a candidate list");
        assert!(
            error.message().contains("/etc/qip/candidates.json"),
            "the refusal does not name the file that failed to parse: {}",
            error.message()
        );
        assert!(
            error.message().contains("source-candidate"),
            "the refusal does not say what kind of document was expected: {}",
            error.message()
        );
    }
}

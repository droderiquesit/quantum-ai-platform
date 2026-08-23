//! The operator command line.
//!
//! Everything an operator needs to inspect a platform without an HTTP client:
//! run a cycle, print the loop's state, check the governance of the agent
//! roster, and verify the event log's hash chain.
//!
//! No subcommand here can raise the autonomy level. That is deliberate and
//! matches the API: enabling live trading requires two authenticated
//! operators, and a command line cannot establish two people.
//!
//! Every invocation builds a fresh platform and exits, so nothing this process
//! holds outlives the command — which makes the *archive* the only thing that
//! makes `qip cycle` more than a demonstration. `cycle` appends the event log's
//! hash chain to the configured store and `status` reads it back, so two
//! invocations against the same store are two runs of one platform rather than
//! two unrelated ones. Without a store configured they are unrelated, and
//! `status` says so rather than printing a zero it never observed.

use qip_core::error::{Error, Result};
use qip_core::{Clock, SystemClock};
use qip_financial::universe::Universe;
use qip_kernel::{Platform, PlatformConfig};
use qip_observability::Telemetry;
use qip_risk::limits::LimitSet;
use qip_storage::ChainArchive;
use qip_storage::settings::StorageSettings;
use std::sync::Arc;

fn main() {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let command = arguments.first().map(String::as_str).unwrap_or("help");

    let result = match command {
        "help" | "--help" | "-h" => {
            print_help();
            Ok(())
        }
        "status" => status(),
        "cycle" => cycle(arguments.get(1).and_then(|n| n.parse().ok()).unwrap_or(1)),
        "agents" => agents(),
        "governance" => governance(),
        "limits" => limits_command(),
        "storage" => storage_command(),
        other => Err(Error::invalid(format!(
            "unknown command: {other}. Run `qip help` for the list."
        ))),
    };

    if let Err(error) = result {
        eprintln!("qip: {}", error.message());
        std::process::exit(1);
    }
}

fn print_help() {
    println!("qip — operator command line");
    println!();
    println!("  status            the autonomy level, ceiling and kill switch");
    println!("  cycle [n]         run n cycles of the intelligence loop (default 1)");
    println!("  agents            the agent roster and each agent's grants");
    println!("  governance        run the roster's governance review");
    println!("  limits            the risk limits and their rationales");
    println!("  storage           the configured store, and what survives a restart");
    println!();
    println!("There is deliberately no command to raise the autonomy level:");
    println!("enabling live trading needs two authenticated operators, and a");
    println!("command line cannot establish two people.");
}

/// The configured store, proven writable.
///
/// Every command that reads or writes the archive goes through here, so a
/// misconfigured store fails the command outright. Returning an in-memory
/// store on a bad configuration would make `qip cycle` report archived records
/// that were never anywhere.
fn storage() -> Result<StorageSettings> {
    let settings = StorageSettings::from_env()?;
    settings.preflight()?;
    Ok(settings)
}

fn archive(settings: &StorageSettings) -> Result<ChainArchive> {
    ChainArchive::open(settings.key_value("event-log")?)
}

fn platform() -> Result<Platform> {
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);
    let config = PlatformConfig::default();
    let context = qip_core::Context::new(clock.clone(), config.seed);
    Platform::new(
        config,
        context,
        Telemetry::silent(),
        Universe::new(),
        LimitSet::conservative_default(),
    )
}

fn storage_command() -> Result<()> {
    let settings = storage()?;
    println!("target:    {}", settings.target().as_str());
    println!(
        "root:      {}",
        match settings.root().as_os_str().is_empty() {
            true => "not used by this target".to_string(),
            false => settings.root().display().to_string(),
        }
    );
    println!("rationale: {}", settings.target().rationale());
    println!(
        "durable:   {}",
        if settings.is_durable() {
            "yes; an acknowledged write survives a restart"
        } else {
            "NO; nothing this process writes survives it"
        }
    );
    println!("chain:     {}", archive(&settings)?.describe());
    Ok(())
}

fn status() -> Result<()> {
    let platform = platform()?;
    let controller = platform.autonomy();
    println!(
        "autonomy:  {} ({})",
        controller.level(),
        controller.level().describe()
    );
    println!("ceiling:   {}", controller.ceiling());
    println!(
        "live:      {}",
        if platform.is_live_capable() {
            "reachable"
        } else {
            "unreachable in this deployment"
        }
    );
    println!(
        "halted:    {}",
        if controller.kill_switch().is_globally_tripped() {
            "YES"
        } else {
            "no"
        }
    );
    println!("cycles:    {}", platform.cycle_count());
    println!("events:    {}", platform.event_log().len());
    println!(
        "log chain: {}",
        match platform.event_log().verify_chain() {
            Ok(()) => "intact".to_string(),
            Err(sequence) => format!("BROKEN at sequence {sequence}"),
        }
    );

    // The counts above describe a platform that was built one line ago, so
    // they are all but meaningless on their own. The archive is the part that
    // spans invocations, and it is reported separately rather than folded into
    // the same numbers — adding a restart's records to this run's would claim
    // this process had done work it has not.
    let settings = storage()?;
    println!("store:     {}", settings.target().as_str());
    println!("archived:  {}", archive(&settings)?.describe());
    if !settings.is_durable() {
        println!(
            "           nothing is being kept; set QIP_STORAGE_TARGET=engine and \
             QIP_STORAGE_ROOT to make successive commands one platform rather than many"
        );
    }
    Ok(())
}

fn cycle(count: u64) -> Result<()> {
    if count == 0 || count > 1000 {
        return Err(Error::invalid("run between 1 and 1000 cycles"));
    }
    let settings = storage()?;
    let archive = archive(&settings)?;
    let mut platform = platform()?;
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);
    for _ in 0..count {
        let report = platform.run_cycle(clock.now());
        println!("{}", report.summarise());
        println!();
    }

    // Once, after every cycle has run, rather than once per cycle: the archive
    // skips what it already holds, so the two are equivalent in what they
    // write, and doing it here keeps a storage failure from stopping a run of
    // cycles halfway through.
    let archived = archive.absorb(platform.event_log().records())?;
    println!(
        "archived {archived} event(s) to {}; {}",
        settings.target().as_str(),
        archive.describe()
    );
    Ok(())
}

fn agents() -> Result<()> {
    let platform = platform()?;
    for manifest in platform.organisation().roster().iter() {
        println!("{} — {}", manifest.id, manifest.name);
        println!("  role:   {}", manifest.role);
        println!("  owner:  {}", manifest.owner);
        println!("  grants: {}", manifest.capabilities.len());
        println!("  {}", manifest.purpose);
        for limitation in &manifest.limitations {
            println!("  ! {limitation}");
        }
        println!();
    }
    Ok(())
}

fn governance() -> Result<()> {
    let platform = platform()?;
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);
    let findings = platform.review_governance(clock.now());
    if findings.is_empty() {
        println!("the roster passes every governance rule");
        return Ok(());
    }
    for finding in &findings {
        println!(
            "[{}] {} — {}",
            match finding.severity {
                qip_agents::governance::Severity::Error => "error",
                qip_agents::governance::Severity::Warning => "warn",
            },
            finding.rule,
            finding.detail
        );
    }
    let errors = findings
        .iter()
        .filter(|f| f.severity == qip_agents::governance::Severity::Error)
        .count();
    if errors > 0 {
        return Err(Error::denied(format!(
            "{errors} governance error(s); the platform should not run"
        )));
    }
    Ok(())
}

fn limits_command() -> Result<()> {
    for limit in &LimitSet::conservative_default().limits {
        println!("{}", limit.name);
        println!("  {}", limit.rationale);
        println!();
    }
    Ok(())
}

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
//! `demo --live` is the one subcommand that opens a socket, and it keeps a
//! second rule of the same shape: it takes no address from anybody. Its three
//! peers are loopback servers this process binds on ephemeral ports and scripts
//! itself, and no flag, variable or file moves them — so it cannot become a way
//! to reach a venue the normal path would refuse. See [`qip_cli::demo`], which
//! is also where it says, at both ends of the run, that every fill it prints
//! was made up in this process.
//!
//! Every invocation builds a fresh platform and exits, so nothing this process
//! holds outlives the command — which makes the *archive* the only thing that
//! makes `qip cycle` more than a demonstration. `cycle` appends the event log's
//! hash chain to the configured store and `status` reads it back, so two
//! invocations against the same store are two runs of one platform rather than
//! two unrelated ones. Without a store configured they are unrelated, and
//! `status` says so rather than printing a zero it never observed.

use qip_cli::demo::{DemoSettings, LiveDemo};
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
        "demo" => demo(&arguments[1..]),
        "cycle" => cycle_count(arguments.get(1)).and_then(cycle),
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
    println!("  demo --live [n]   stand up loopback peers and walk the live path");
    println!("  cycle [n]         run n cycles of the intelligence loop (default 1)");
    println!("  agents            the agent roster and each agent's grants");
    println!("  governance        run the roster's governance review");
    println!("  limits            the risk limits and their rationales");
    println!("  storage           the configured store, and what survives a restart");
    println!();
    println!("`demo --live` binds a data vendor, a venue and a mesh peer on");
    println!("loopback, points the live adapters at them and prints what every");
    println!("layer did. It is a demonstration: every fill it reports is made up");
    println!("by a test double in this process, and it takes no address from");
    println!("anybody, so it cannot be pointed at a market.");
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

/// The `cycle` subcommand's count argument, refused rather than guessed.
///
/// `arguments.get(1).and_then(|n| n.parse().ok()).unwrap_or(1)` used to sit
/// here, so `qip cycle abc` silently ran one cycle instead of telling the
/// operator the argument was not a number — the same class of bug the house
/// rule against clamping exists to catch, just aimed at a CLI argument
/// instead of a domain value. A missing argument still means one cycle: that
/// is the documented default, not a value corrected from something else.
fn cycle_count(argument: Option<&String>) -> Result<u64> {
    match argument {
        None => Ok(1),
        Some(text) => text
            .parse::<u64>()
            .map_err(|_| Error::invalid(format!("{text:?} is not a number of cycles"))),
    }
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

/// Stand up the live path against loopback peers and walk it.
///
/// Everything this function does beyond argument handling belongs to
/// [`qip_cli::demo`], which is where it can be tested. What is here is the
/// shape of the command: refuse an invocation that does not name `--live`,
/// bound the cycle count, print the banner before anything runs, print each
/// cycle as it finishes, and print what the run was not on the way out.
///
/// `--live` is required rather than defaulted because the word is the only
/// thing separating this from a command that could be read as running the
/// platform for real. There is no other demonstration behind `qip demo`, and
/// naming the one there is costs an operator four keystrokes and buys the
/// reader of a shell history the knowledge that a socket was involved.
fn demo(arguments: &[String]) -> Result<()> {
    let mut positional = Vec::new();
    let mut live = false;
    for argument in arguments {
        match argument.as_str() {
            "--live" => live = true,
            other if other.starts_with("--") => {
                return Err(Error::invalid(format!(
                    "unknown option {other}. `qip demo --live [cycles]` is the only form"
                )));
            }
            other => positional.push(other),
        }
    }
    if !live {
        return Err(Error::invalid(
            "`qip demo` has one form: `qip demo --live [cycles]`. It binds a data vendor, a \
             venue and a mesh peer on loopback and walks the platform's live path against them. \
             Nothing it prints comes from a market",
        ));
    }
    let cycles = match positional.first() {
        None => DemoSettings::default().cycles,
        Some(text) => text
            .parse::<u64>()
            .map_err(|_| Error::invalid(format!("{text:?} is not a number of cycles")))?,
    };

    let mut demonstration = LiveDemo::stand_up(DemoSettings::default().with_cycles(cycles)?)?;
    for line in demonstration.banner_lines() {
        println!("{line}");
    }
    for _ in 0..cycles {
        println!();
        for line in demonstration.cycle()?.lines() {
            println!("{line}");
        }
    }
    println!();
    for line in demonstration.closing_lines() {
        println!("{line}");
    }
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

#[cfg(test)]
mod tests {
    use super::cycle_count;

    #[test]
    fn a_missing_cycle_count_defaults_to_one() {
        assert_eq!(
            cycle_count(None).expect("no argument is a legal invocation"),
            1
        );
    }

    #[test]
    fn a_numeric_cycle_count_is_accepted_as_written() {
        let text = "7".to_string();
        assert_eq!(
            cycle_count(Some(&text)).expect("7 is a legal cycle count"),
            7
        );
    }

    #[test]
    fn a_cycle_count_that_is_not_a_number_is_refused_rather_than_silently_run_once() {
        // Before this fix `qip cycle abc` ran one cycle without telling the
        // operator the argument was ignored — the exact failure mode the
        // house rule against clamping an invalid input exists to prevent.
        let text = "abc".to_string();
        let error =
            cycle_count(Some(&text)).expect_err("a non-numeric cycle count was silently accepted");
        assert!(
            error.message().contains("abc"),
            "the refusal does not name the argument that was rejected: {}",
            error.message()
        );
    }

    #[test]
    fn a_negative_cycle_count_is_refused_rather_than_silently_run_once() {
        let text = "-3".to_string();
        assert!(
            cycle_count(Some(&text)).is_err(),
            "a negative cycle count parsed as a u64 or was silently defaulted"
        );
    }
}

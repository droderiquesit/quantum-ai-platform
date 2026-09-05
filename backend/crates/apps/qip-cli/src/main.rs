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
use qip_kernel::config::EventLogDestination;
use qip_kernel::{Platform, PlatformConfig};
use qip_observability::Telemetry;
use qip_risk::limits::LimitSet;
use qip_storage::ChainArchive;
use qip_storage::settings::StorageSettings;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

/// The exit code of a command that ran and found nothing wrong.
const OK: u8 = 0;

/// The exit code of a command that could not answer at all: a bad argument,
/// a configuration that does not parse, a journal that is not one. Kept
/// distinct from the verdict codes — [`qip_cli::registrations::PENDING`] and
/// [`qip_cli::replay::DIFFERS`], both three — because a script that could
/// not tell "I could not look" from "I looked and it is wrong" would report
/// a broken invocation as a finding, or worse, the other way round.
const REFUSED: i32 = 1;

fn main() {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let command = arguments.first().map(String::as_str).unwrap_or("help");

    let result = match command {
        "help" | "--help" | "-h" => {
            print_help();
            Ok(OK)
        }
        "status" => status().map(|()| OK),
        "demo" => demo(&arguments[1..]).map(|()| OK),
        "cycle" => cycle_count(arguments.get(1)).and_then(cycle).map(|()| OK),
        "agents" => agents().map(|()| OK),
        "governance" => governance().map(|()| OK),
        "limits" => limits_command().map(|()| OK),
        "storage" => storage_command().map(|()| OK),
        "registrations" => registrations_command(&arguments[1..]),
        "replay" => replay_command(&arguments[1..]),
        other => Err(Error::invalid(format!(
            "unknown command: {other}. Run `qip help` for the list."
        ))),
    };

    match result {
        Err(error) => {
            eprintln!("qip: {}", error.message());
            std::process::exit(REFUSED);
        }
        // The verdict *is* the exit code for the two commands that have one,
        // so the process cannot report a clean run over a report that says
        // otherwise.
        Ok(code) => std::process::exit(i32::from(code)),
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
    println!("  registrations [--config <path>]");
    println!("                    every catalogued source's requirement and standing,");
    println!("                    and the exact secret-add command for a pending one");
    println!("  replay --journal <path> [--config <path>]");
    println!("                    verify a journal's hash chain and rebuild the");
    println!("                    eligibility, registration and fabric registries from");
    println!("                    it alone, against the platform this configuration");
    println!("                    assembles");
    println!();
    println!("`registrations` exits 3 while any catalogued source is still refused, and");
    println!("`replay` exits 3 if the chain is broken or a registry disagrees. Both exit");
    println!("1 when they could not answer at all. Neither prints a credential value:");
    println!("`registrations` prints the name a credential is read under and the command");
    println!("that puts a version behind it, which reads the value from stdin.");
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
    let settings = StorageSettings::from_env(&|name| std::env::var(name).ok())?;
    settings.preflight()?;
    Ok(settings)
}

fn archive(settings: &StorageSettings) -> Result<ChainArchive> {
    ChainArchive::open(settings.key_value("event-log")?)
}

fn platform() -> Result<Platform> {
    platform_from(PlatformConfig::default())
}

/// The platform a configuration assembles, on the host clock.
fn platform_from(config: PlatformConfig) -> Result<Platform> {
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);
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

// --- the two commands whose exit code is the answer -------------------------

/// The `--name <path>` options a command was given.
///
/// Refuses what it cannot act on rather than ignoring it: an option this
/// command does not take, an option with no path after it, and the same
/// option twice. The last is the one worth naming — `--journal a --journal
/// b` has an obvious reading and a wrong one, and a command that silently
/// picked either would verify a file the operator did not mean while
/// printing the name of the one they did.
fn options(arguments: &[String], permitted: &[&str]) -> Result<BTreeMap<String, PathBuf>> {
    let mut found: BTreeMap<String, PathBuf> = BTreeMap::new();
    let mut index = 0;
    while index < arguments.len() {
        let name = arguments[index].as_str();
        if !permitted.contains(&name) {
            return Err(Error::invalid(format!(
                "unknown argument {name}; this command takes {}",
                permitted.join(" and ")
            )));
        }
        let Some(value) = arguments.get(index + 1) else {
            return Err(Error::invalid(format!("{name} needs a path after it")));
        };
        if found
            .insert(name.to_string(), PathBuf::from(value))
            .is_some()
        {
            return Err(Error::invalid(format!(
                "{name} was given twice; which one is meant is not something this command will \
                 guess"
            )));
        }
        index += 2;
    }
    Ok(found)
}

/// The configuration a command runs against: the file if one was named, and
/// otherwise the shipped default.
///
/// A named file that does not exist is refused rather than falling back to
/// the default. The fallback is the dangerous one: `qip registrations
/// --config /etc/qip/confg.json` would report every source pending, name
/// nothing wrong, and exit 3 for a typo.
fn configuration(path: Option<&PathBuf>) -> Result<PlatformConfig> {
    let Some(path) = path else {
        return Ok(PlatformConfig::default());
    };
    let text = std::fs::read_to_string(path).map_err(|error| {
        Error::invalid(format!(
            "the configuration at {} could not be read: {error}",
            path.display()
        ))
    })?;
    serde_json::from_str(&text).map_err(|error| {
        Error::schema(format!(
            "the configuration at {} is not a platform configuration: {error}",
            path.display()
        ))
    })
}

/// `qip registrations [--config <path>]`.
fn registrations_command(arguments: &[String]) -> Result<u8> {
    let options = options(arguments, &["--config"])?;
    let mut config = configuration(options.get("--config"))?;
    // A read-only question must not append to a deployment's event log.
    // Assembling on a file destination journals every committed
    // registration again — correct for the platform resuming its own log,
    // and not something a command that only reports should do to the
    // evidence.
    config.event_log = EventLogDestination::InMemory;
    let now = SystemClock.now();
    let platform = platform_from(config)?;
    let view =
        qip_api::registration_views::registrations(&platform, now).map_err(Error::invalid)?;
    let report = qip_cli::registrations::report(&view.sources);
    for line in &report.lines {
        println!("{line}");
    }
    Ok(report.code)
}

/// `qip replay --journal <path> [--config <path>]`.
///
/// `--config` is not decoration: a deployment that commits venue
/// registrations assembles a platform holding them, and checking its
/// journal against the *default* configuration would report a divergence on
/// every one. The journal names the log; the configuration names the
/// platform it is being compared against.
fn replay_command(arguments: &[String]) -> Result<u8> {
    let options = options(arguments, &["--journal", "--config"])?;
    let Some(journal) = options.get("--journal") else {
        return Err(Error::invalid(
            "`qip replay` needs --journal <path>, the event log to check. There is no default: a \
             default would name a file the operator did not choose",
        ));
    };
    let config = configuration(options.get("--config"))?;
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);
    let verdict = qip_cli::replay::verify(journal, config, clock)?;
    for line in &verdict.lines {
        println!("{line}");
    }
    Ok(verdict.code)
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

//! `qip registrations`, run.
//!
//! Two properties, and they are different properties: that a pending source
//! is reported with the exact line an operator runs to move it, and that a
//! catalogue with nothing pending exits zero rather than three. The second
//! is the one that would rot silently — a command that always exits three is
//! a command every deployment script learns to ignore, and it would still
//! pass a test that only ever looked at the shipped table, where two sources
//! need an account and always will.
//!
//! Each test asserts its premise first. The exit code is meaningless without
//! the standing that produced it: "exits three" proves nothing if nothing was
//! pending, and "exits zero" proves nothing if the list was empty.

// In a test the assertion is the deliverable.
#![allow(clippy::panic_in_result_fn)]

use qip_api::registration_views::{RegistrationsView, StandingView};
use qip_cli::registrations::{ADMITTED, PENDING, report};
use qip_core::error::{Error, Result};
use qip_core::{Context, Timestamp};
use qip_financial::universe::Universe;
use qip_kernel::{Platform, PlatformConfig};
use qip_observability::Telemetry;
use qip_risk::limits::LimitSet;
use std::process::Command;

/// A source the shipped requirement table says needs an account and whose
/// manifest reads two credentials, so it is pending *and* has a command.
const PENDING_SOURCE: &str = "alpaca-daily-bars";

/// The two lines that put a version behind the slots that source reads. The
/// value arrives on stdin, so running either puts nothing in a shell
/// history — which is why the `--data-file=-` is part of what is asserted
/// rather than incidental to it.
const PRIMARY_COMMAND: &str = "gcloud secrets versions add qip-alpaca-api-secret-key --data-file=-";
const COMPANION_COMMAND: &str = "gcloud secrets versions add qip-alpaca-api-key-id --data-file=-";

fn start() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

/// The view the command renders, from a platform assembled exactly as the
/// command assembles one.
fn view() -> Result<RegistrationsView> {
    let config = PlatformConfig::default();
    let (context, _clock) = Context::deterministic(start(), config.seed);
    let platform = Platform::new(
        config,
        context,
        Telemetry::silent(),
        Universe::new(),
        LimitSet::conservative_default(),
    )?;
    qip_api::registration_views::registrations(&platform, start()).map_err(Error::invalid)
}

/// What the binary printed and what it exited with.
fn run() -> Result<(String, i32)> {
    let output = Command::new(env!("CARGO_BIN_EXE_qip"))
        .arg("registrations")
        .output()
        .map_err(|error| Error::io(format!("the qip binary did not run: {error}")))?;
    Ok((
        String::from_utf8_lossy(&output.stdout).to_string(),
        output.status.code().unwrap_or(-1),
    ))
}

#[test]
fn a_pending_source_is_printed_with_the_command_that_moves_it_and_the_run_exits_three() -> Result<()>
{
    // The premise, from the same registry the command reads: this source is
    // pending, and it is pending because nobody has registered rather than
    // because the catalogue forgot it. Without this the exit code below
    // could be three for any reason at all.
    let view = view()?;
    let source = view
        .sources
        .iter()
        .find(|source| source.source_id == PENDING_SOURCE)
        .ok_or_else(|| Error::invalid(format!("{PENDING_SOURCE} is not in the catalogue")))?;
    assert!(
        matches!(source.standing, StandingView::Pending { .. }),
        "{PENDING_SOURCE} is not pending, so this test proves nothing about a pending source: \
         {:?}",
        source.standing
    );
    assert_eq!(source.requirement.as_deref(), Some("account"));

    let (printed, code) = run()?;

    // The exact line, not a paraphrase of it. An operator copies this.
    assert!(
        printed.contains(PRIMARY_COMMAND),
        "the primary secret-add command is not in the output:\n{printed}"
    );
    assert!(
        printed.contains(COMPANION_COMMAND),
        "the companion secret-add command is not in the output, so an operator who ran what was \
         printed would have filled one of two slots:\n{printed}"
    );
    // Names, and only names. The command tells an operator which deployment
    // variable the credential is read under; the value is nowhere in this
    // process to be printed, and the line that adds it reads stdin.
    assert!(
        printed.contains("QIP_ALPACA_API_SECRET_KEY"),
        "the deployment variable the credential is read under is not named:\n{printed}"
    );
    assert_eq!(
        code,
        i32::from(PENDING),
        "a catalogue with a pending source exited {code}; a deployment gate reading this would \
         call an unregistered venue a clean run"
    );
    Ok(())
}

#[test]
fn a_catalogue_whose_every_source_is_keyless_exits_zero_and_prints_no_secret_command() -> Result<()>
{
    let view = view()?;
    let keyless: Vec<_> = view
        .sources
        .iter()
        .filter(|source| matches!(source.standing, StandingView::Keyless))
        .cloned()
        .collect();

    // Two premises, and the second is the one that matters. That the list is
    // non-empty stops this from being a test about an empty catalogue; that
    // the *unfiltered* list held something else stops it from being a test
    // in which the filter did nothing, which is the shape of test that
    // passes forever.
    assert!(
        !keyless.is_empty(),
        "no catalogued source is keyless, so this asserts nothing about a keyless catalogue"
    );
    assert!(
        keyless.len() < view.sources.len(),
        "every catalogued source is already keyless, so the filter removed nothing and this test \
         is the other one"
    );

    let report = report(&keyless);
    assert_eq!(
        report.code, ADMITTED,
        "a catalogue with nothing pending exited {}; a command that never exits zero is one no \
         deployment gate can use",
        report.code
    );
    assert!(
        !report.lines.iter().any(|line| line.contains("gcloud")),
        "a secret-add command was printed for a catalogue in which nothing needs a secret: {:?}",
        report.lines
    );
    // And it says so in words, not only in the exit code, because the exit
    // code is invisible to whoever is reading the terminal.
    assert!(
        report
            .lines
            .iter()
            .any(|line| line.contains("is admitted; nothing is waiting on a registration")),
        "the report does not say that nothing is pending: {:?}",
        report.lines
    );
    Ok(())
}

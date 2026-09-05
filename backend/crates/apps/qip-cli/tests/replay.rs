//! `qip replay --journal <path>`, run against journals a platform actually
//! wrote.
//!
//! The command's whole claim is that it can tell three things apart: a
//! journal the platform agrees with, a journal that has been altered, and a
//! journal holding decisions the platform is not acting on. A test that only
//! proved the first would pass against a command that printed `identical`
//! unconditionally, which is why every test here asserts the premise that
//! makes its verdict mean something — that the chain *was* intact before the
//! tamper, that the journal *does* hold the record it is said to diverge on.
//!
//! Every journal is written by assembling a real [`Platform`] on a file
//! event log and driving it through the kernel's own approval paths. None is
//! hand-built: a hand-built journal would prove the parser reads what the
//! test wrote and nothing about what the platform writes.

// In a test the assertion is the deliverable.
#![allow(clippy::panic_in_result_fn)]

use qip_capital::ledger::{
    Eligibility, EligibilityDecision, EligibilityTerms, Jurisdiction, UserId,
};
use qip_cli::replay::{AGREES, DIFFERS, IDENTICAL};
use qip_core::error::{Error, Result};
use qip_core::time::{Duration, Timestamp};
use qip_core::{Context, ObjectId, dec};
use qip_financial::asset_class::{InstrumentType, Sector};
use qip_financial::object::FinancialObject;
use qip_financial::quality::Provenance;
use qip_financial::universe::Universe;
use qip_kernel::{Platform, PlatformConfig};
use qip_market_ingestion::connector::manifest::SecretRef;
use qip_observability::Telemetry;
use qip_risk::limits::LimitSet;
use qip_risk_engine::autonomy::OperatorIdentity;
use std::path::{Path, PathBuf};
use std::process::Command;

const INSTRUMENT: &str = "obj-AAA";
/// The source the shipped table says needs an account, approved into the
/// journals below so that the divergence test has a real registration to
/// find rather than a synthesised one.
const ACCOUNT_SOURCE: &str = "alpaca-daily-bars";
const TERMS: &str = "https://alpaca.markets/terms-and-conditions";
const SLOT: &str = "QIP_ALPACA_API_SECRET_KEY";

/// The producers the kernel writes each registry's records under. Literals
/// here, as the kernel's own suites keep them, so a test reads the journal
/// the way an auditor would — by what the record says about itself.
const REGISTRATION_PRODUCER: &str = "kernel/registration";
const ELIGIBILITY_PRODUCER: &str = "kernel/eligibility";

fn start() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

fn universe() -> Result<Universe> {
    let mut universe = Universe::new();
    universe.insert(
        FinancialObject::builder(
            ObjectId::from_string(INSTRUMENT),
            "AAA",
            InstrumentType::CommonStock,
        )
        .venue("XNYS")
        .sector(Sector::InformationTechnology)
        .price(dec!("100"))
        .provenance(Provenance::synthetic("test", start()))
        .build(start())?,
    )?;
    Ok(universe)
}

/// A path in the temporary directory, emptied of any earlier run's file.
fn journal_path(label: &str) -> Result<PathBuf> {
    let path = std::env::temp_dir().join(format!(
        "qip-cli-replay-{label}-{}.jsonl",
        std::process::id()
    ));
    if path.exists() {
        std::fs::remove_file(&path)
            .map_err(|error| Error::io(format!("the previous journal survived: {error}")))?;
    }
    Ok(path)
}

/// Assemble a platform on `path` and let it write: `cycles` passes of the
/// loop, and — when `decisions` is set — one venue registration and one
/// eligibility decision through the kernel's own approval paths, so the
/// records are the ones a deployment would hold rather than ones this test
/// invented.
fn write_journal(path: &Path, decisions: bool, cycles: u64) -> Result<()> {
    let config = PlatformConfig::default().with_event_log_file(path);
    let (context, _clock) = Context::deterministic(start(), config.seed);
    let mut platform = Platform::new(
        config,
        context,
        Telemetry::silent(),
        universe()?,
        LimitSet::conservative_default(),
    )?;
    for _ in 0..cycles {
        platform.run_cycle(start());
    }
    if decisions {
        let operator = OperatorIdentity::verified("ops-dana", "hardware-token", start());
        platform.approve_registration(
            ACCOUNT_SOURCE,
            &operator,
            TERMS,
            SecretRef::new(SLOT)?,
            start(),
        )?;
        platform.decide_eligibility(
            &UserId::new("desk")?,
            EligibilityDecision::Granted {
                eligibility: Eligibility::new(EligibilityTerms {
                    verified_at: start(),
                    can_invest: true,
                    jurisdiction: Jurisdiction::new("GB")?,
                    expires_at: start().saturating_add(Duration::from_days(365)),
                })?,
            },
            &operator,
            "the desk is verified for this journal",
            start(),
        )?;
    }
    Ok(())
}

/// Every line of the journal, parsed. The test reads the file rather than
/// the log so that the positions it asserts are positions in the artefact an
/// auditor would open.
fn lines(path: &Path) -> Result<Vec<serde_json::Value>> {
    let text = std::fs::read_to_string(path)
        .map_err(|error| Error::io(format!("the journal could not be read: {error}")))?;
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            serde_json::from_str::<serde_json::Value>(line)
                .map_err(|error| Error::schema(format!("a journal line is not JSON: {error}")))
        })
        .collect()
}

/// The one-based position of the record a producer wrote, and the sequence
/// the chain committed it under.
fn position_of(records: &[serde_json::Value], producer: &str) -> Option<(usize, u64)> {
    records.iter().enumerate().find_map(|(index, record)| {
        let written_by = record.pointer("/event/lineage/producer")?.as_str()?;
        if written_by != producer {
            return None;
        }
        Some((index + 1, record.get("sequence")?.as_u64()?))
    })
}

/// What the binary printed and what it exited with.
fn run(arguments: &[&str]) -> Result<(String, String, i32)> {
    let output = Command::new(env!("CARGO_BIN_EXE_qip"))
        .args(arguments)
        .output()
        .map_err(|error| Error::io(format!("the qip binary did not run: {error}")))?;
    Ok((
        String::from_utf8_lossy(&output.stdout).to_string(),
        String::from_utf8_lossy(&output.stderr).to_string(),
        output.status.code().unwrap_or(-1),
    ))
}

/// The verdict on one line of the report, by its label — the text after the
/// first colon, trimmed. Compared exactly, so `identical` cannot be matched
/// by a line that merely contains the word.
fn verdict(printed: &str, label: &str) -> Option<String> {
    printed
        .lines()
        .find(|line| line.starts_with(&format!("{label}:")))
        .and_then(|line| line.split_once(':'))
        .map(|(_, verdict)| verdict.trim().to_string())
}

#[test]
fn a_clean_journal_replays_into_three_identical_registries_and_the_run_exits_zero() -> Result<()> {
    let path = journal_path("clean")?;
    write_journal(&path, false, 1)?;

    // Premise: there is a journal with records in it. The command refuses an
    // empty one precisely so that this test cannot pass against nothing.
    let records = lines(&path)?;
    assert!(
        !records.is_empty(),
        "the platform wrote no journal, so three `identical` lines would be three claims about \
         an empty file"
    );

    let (printed, stderr, code) = run(&["replay", "--journal", &path.display().to_string()])?;
    assert_eq!(
        verdict(&printed, "chain").as_deref(),
        Some("intact"),
        "the chain of a journal nothing touched is not intact:\n{printed}{stderr}"
    );
    for registry in ["eligibility", "registrations", "fabric"] {
        assert_eq!(
            verdict(&printed, registry).as_deref(),
            Some(IDENTICAL),
            "the {registry} registry the journal rebuilds is not the one the platform \
             holds:\n{printed}{stderr}"
        );
    }
    assert_eq!(
        code,
        i32::from(AGREES),
        "a journal the platform agrees with exited {code}:\n{printed}{stderr}"
    );

    // And the command left the evidence alone. A verifier that appends its
    // own assembly record to the file it was asked to check has changed the
    // thing under audit.
    assert_eq!(
        lines(&path)?.len(),
        records.len(),
        "the journal grew while it was being verified"
    );
    std::fs::remove_file(&path).ok();
    Ok(())
}

#[test]
fn a_journal_with_a_tampered_record_is_refused_by_position_and_the_run_exits_non_zero() -> Result<()>
{
    let path = journal_path("tampered")?;
    write_journal(&path, false, 1)?;
    let records = lines(&path)?;

    // Premise: before the tamper this journal passes. Without it, "exits
    // non-zero" is a claim about the tamper that any other fault would also
    // satisfy.
    let (before, stderr, code) = run(&["replay", "--journal", &path.display().to_string()])?;
    assert_eq!(
        verdict(&before, "chain").as_deref(),
        Some("intact"),
        "the journal was already broken before it was tampered with:\n{before}{stderr}"
    );

    // Alter the last record's committed hash by one character. Nothing else
    // in the file moves, so the only thing the command can be reacting to is
    // the chain.
    let position = records.len();
    let sequence = records
        .last()
        .and_then(|record| record.get("sequence")?.as_u64())
        .ok_or_else(|| Error::invalid("the last journal record carries no sequence"))?;
    let text = std::fs::read_to_string(&path)
        .map_err(|error| Error::io(format!("the journal could not be read: {error}")))?;
    let mut kept: Vec<String> = text.lines().map(str::to_string).collect();
    let last = kept
        .pop()
        .ok_or_else(|| Error::invalid("the journal has no last line"))?;
    let mut altered: serde_json::Value = serde_json::from_str(&last)
        .map_err(|error| Error::schema(format!("the last line is not JSON: {error}")))?;
    let hash = altered
        .get("record_hash")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| Error::invalid("the last record carries no hash"))?
        .to_string();
    // A hash is hexadecimal, so swapping a `0` for a `1` — or the reverse —
    // keeps it a well-formed hash of the right length and makes it the wrong
    // one. The record still parses; only the chain notices.
    let broken = match hash.starts_with('0') {
        true => format!("1{}", &hash[1..]),
        false => format!("0{}", &hash[1..]),
    };
    assert_ne!(broken, hash, "the tamper changed nothing");
    altered["record_hash"] = serde_json::Value::String(broken);
    kept.push(
        serde_json::to_string(&altered)
            .map_err(|error| Error::schema(format!("the altered record: {error}")))?,
    );
    std::fs::write(&path, format!("{}\n", kept.join("\n")))
        .map_err(|error| Error::io(format!("the tampered journal: {error}")))?;

    let (printed, stderr, code_after) = run(&["replay", "--journal", &path.display().to_string()])?;
    let chain = verdict(&printed, "chain")
        .ok_or_else(|| Error::invalid(format!("no chain line in:\n{printed}{stderr}")))?;
    assert!(
        chain.contains(&format!("record position {position} (sequence {sequence})")),
        "the refusal does not name the position of the record that was altered — an auditor \
         cannot open a line the report will not name: {chain}"
    );
    assert_ne!(
        code_after,
        i32::from(AGREES),
        "an altered journal exited zero:\n{printed}{stderr}"
    );
    assert_eq!(code_after, i32::from(DIFFERS), "{printed}{stderr}");
    assert_ne!(
        code, code_after,
        "the same code before and after the tamper, so the command is not reading the chain"
    );
    std::fs::remove_file(&path).ok();
    Ok(())
}

#[test]
fn a_journal_holding_decisions_this_configuration_does_not_commit_names_each_by_record_position()
-> Result<()> {
    let path = journal_path("diverging")?;
    write_journal(&path, true, 0)?;
    let records = lines(&path)?;

    // Premise: the journal holds one record from each registry, at positions
    // this test reads out of the file rather than assumes. The command is
    // then run against the *default* configuration, which commits neither —
    // so the platform it assembles is admitting nobody, and the journal says
    // somebody was admitted.
    let (registration_position, registration_sequence) =
        position_of(&records, REGISTRATION_PRODUCER).ok_or_else(|| {
            Error::invalid("the journal holds no registration record to diverge on")
        })?;
    let (eligibility_position, eligibility_sequence) = position_of(&records, ELIGIBILITY_PRODUCER)
        .ok_or_else(|| Error::invalid("the journal holds no eligibility record to diverge on"))?;

    let (printed, stderr, code) = run(&["replay", "--journal", &path.display().to_string()])?;

    // The chain is intact, so what follows is a disagreement about content
    // and not about the file having been altered.
    assert_eq!(
        verdict(&printed, "chain").as_deref(),
        Some("intact"),
        "{printed}{stderr}"
    );
    let registrations = verdict(&printed, "registrations")
        .ok_or_else(|| Error::invalid(format!("no registrations line in:\n{printed}{stderr}")))?;
    assert!(
        registrations.contains(&format!(
            "record position {registration_position} (sequence {registration_sequence})"
        )),
        "the registration divergence does not name the record it is about: {registrations}"
    );
    assert!(
        registrations.contains(ACCOUNT_SOURCE) && registrations.contains("ops-dana"),
        "the divergence names neither the source nor who registered it: {registrations}"
    );
    let eligibility = verdict(&printed, "eligibility")
        .ok_or_else(|| Error::invalid(format!("no eligibility line in:\n{printed}{stderr}")))?;
    assert!(
        eligibility.contains(&format!(
            "record position {eligibility_position} (sequence {eligibility_sequence})"
        )),
        "the eligibility divergence does not name the record it is about: {eligibility}"
    );
    assert_eq!(
        code,
        i32::from(DIFFERS),
        "a platform acting on neither of two recorded decisions exited {code}:\n{printed}{stderr}"
    );
    std::fs::remove_file(&path).ok();
    Ok(())
}

#[test]
fn a_missing_empty_or_malformed_journal_is_refused_by_name_rather_than_replayed_as_an_empty_one()
-> Result<()> {
    // Premise: a real journal at a path of the same shape is accepted, so
    // each refusal below is about the file and not about the command.
    let good = journal_path("refusals-good")?;
    write_journal(&good, false, 0)?;
    let (_, stderr, code) = run(&["replay", "--journal", &good.display().to_string()])?;
    assert_eq!(
        code,
        i32::from(AGREES),
        "the control journal was refused, so the refusals below prove nothing: {stderr}"
    );
    std::fs::remove_file(&good).ok();

    // Missing. `EventLog::open` would answer with an empty log, and every
    // comparison would then pass against nothing.
    let missing = journal_path("refusals-missing")?;
    let (_, stderr, code) = run(&["replay", "--journal", &missing.display().to_string()])?;
    assert_eq!(
        code, 1,
        "a missing journal did not stop the command: {stderr}"
    );
    assert!(
        stderr.contains(&missing.display().to_string()),
        "the refusal does not name the path it could not find: {stderr}"
    );

    // Present and empty. The dangerous one: it verifies, rebuilds three
    // empty registries, and would exit zero having proved nothing.
    let empty = journal_path("refusals-empty")?;
    std::fs::write(&empty, "").map_err(|error| Error::io(format!("the empty journal: {error}")))?;
    let (_, stderr, code) = run(&["replay", "--journal", &empty.display().to_string()])?;
    assert_eq!(code, 1, "an empty journal was replayed: {stderr}");
    assert!(
        stderr.contains(&empty.display().to_string()) && stderr.contains("holds no records"),
        "the refusal does not say the file was empty: {stderr}"
    );
    std::fs::remove_file(&empty).ok();

    // Present and not a log.
    let malformed = journal_path("refusals-malformed")?;
    std::fs::write(&malformed, "{\"sequence\": \"not a log record\"}\n")
        .map_err(|error| Error::io(format!("the malformed journal: {error}")))?;
    let (_, stderr, code) = run(&["replay", "--journal", &malformed.display().to_string()])?;
    assert_eq!(code, 1, "a malformed journal was replayed: {stderr}");
    assert!(
        stderr.contains(&malformed.display().to_string()),
        "the refusal does not name the file it could not read: {stderr}"
    );
    std::fs::remove_file(&malformed).ok();

    // And an argument the command does not take is refused rather than
    // ignored, for the same reason: a run that quietly dropped `--jrnal`
    // would verify the default of nothing.
    let (_, stderr, code) = run(&["replay", "--jrnal", "/tmp/whatever.jsonl"])?;
    assert_eq!(code, 1, "an unknown argument was ignored: {stderr}");
    assert!(
        stderr.contains("--jrnal"),
        "the refusal does not name the argument it did not understand: {stderr}"
    );
    Ok(())
}

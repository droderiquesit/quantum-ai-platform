//! The capital-fabric declaration: the composition root's one way of putting
//! a destination, a corridor or a transfer-gate assessment on the chain.
//!
//! Every test asserts its premise before its property, because the premise
//! here is the whole point: the platform under test starts with **no** fabric
//! records, and the failure this suite exists to prevent is exactly the one
//! that stood before the module existed — machinery that is built, tested and
//! reached by nothing, whose `/transfer-gate` answers `last_assessment: null`
//! for ever while reading as a control that is watching.
//!
//! So each test that proves a command reaches the chain first proves the
//! chain was empty, and the test that proves a refusal is a record first
//! proves the record count moved rather than only that an error was absent.

// The workspace denies `panic_in_result_fn` for production code, where an
// assertion that aborts a `Result`-returning function is a bug. In a test the
// assertion is the deliverable, and `?` is what keeps the setup readable.
#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_api::fabric::{Declaration, FABRIC_PATH_VARIABLE, FabricFeed, MAX_FABRIC_COMMANDS};
use qip_core::error::Result;
use qip_core::time::Timestamp;
use qip_core::{Context, ManualClock};
use qip_financial::universe::Universe;
use qip_kernel::config::PlatformConfig;
use qip_kernel::platform::Platform;
use qip_observability::Telemetry;
use qip_risk::limits::LimitSet;
use std::sync::Arc;

/// The address the fixtures declare. Named once so the test that proves no
/// refusal repeats the file can look for it.
const ADDRESS: &str = "treasury-account-0417";

fn start() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

fn platform() -> Result<Platform> {
    let config = PlatformConfig::default();
    let clock = Arc::new(ManualClock::new(start()));
    let context = Context::new(clock, config.seed);
    Platform::new(
        config,
        context,
        Telemetry::silent(),
        Universe::new(),
        LimitSet::conservative_default(),
    )
}

/// A directory of this test's own, so two tests running at once cannot see
/// each other's file.
fn fixture_dir(name: &str) -> std::path::PathBuf {
    let directory =
        std::env::temp_dir().join(format!("qip-api-fabric-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&directory).expect("the fixture directory is created");
    directory
}

fn write_fixture(directory: &std::path::Path, text: &str) -> String {
    let path = directory.join("fabric.json");
    std::fs::write(&path, text).expect("the fixture is written");
    path.display().to_string()
}

/// Move the file's modification time to `when`, so a change is visible
/// however coarse the filesystem's timestamps are.
fn touch(path: &str, when: std::time::SystemTime) {
    let file = std::fs::File::options()
        .write(true)
        .open(path)
        .expect("the fixture opens for writing");
    file.set_modified(when)
        .expect("the modification time moves");
}

/// One destination command, as the log writes it.
fn destination(action: &str, by: &str) -> String {
    format!(
        r#"{{"subject": "destination", "action": "{action}",
             "key": {{"asset": "USD", "address": "{ADDRESS}"}},
             "by": "{by}", "at": "{}"}}"#,
        start().to_rfc3339()
    )
}

fn declaration_of(commands: &[String]) -> String {
    format!(r#"{{"commands": [{}]}}"#, commands.join(","))
}

#[test]
fn a_declared_destination_reaches_the_chain_as_a_record() -> Result<()> {
    let mut platform = platform()?;
    // The premise, and the defect this whole module answers: nothing has
    // reached the fabric journal, which is the state every deployed process
    // was in.
    assert_eq!(
        platform.fabric_records(),
        0,
        "the platform starts with fabric records, so this test cannot show that the declaration \
         is what put one there"
    );

    let declaration =
        Declaration::parse(&declaration_of(&[destination("propose", "treasury-desk")]))?;
    let applied = declaration.apply_into(&mut platform, 0, start())?;

    assert_eq!(applied, 1, "the one declared command was not applied");
    assert_eq!(
        platform.fabric_records(),
        1,
        "the command was applied and the journal did not record it, so the chain does not carry \
         what the desk declared"
    );
    Ok(())
}

#[test]
fn a_corridor_declared_before_its_destination_is_refused_by_the_control_and_the_refusal_is_a_record()
-> Result<()> {
    let mut platform = platform()?;
    assert_eq!(
        platform.fabric_records(),
        0,
        "the premise is an empty chain"
    );

    // A corridor naming a destination no command has proposed. The control
    // refuses it, and the refusal is a decision: it belongs in the log, and a
    // module that turned it into an error would delete the only evidence the
    // attempt was made.
    let corridor = format!(
        r#"{{"subject": "corridor", "action": "step", "id": "treasury-sweep",
             "step": {{"step": "review", "by": "treasury-reviewer", "at": "{}"}}}}"#,
        start().to_rfc3339()
    );
    let declaration = Declaration::parse(&declaration_of(&[corridor]))?;

    let applied = declaration
        .apply_into(&mut platform, 0, start())
        .expect("a refusal by the control is a record, not an error the caller sees");
    assert_eq!(applied, 1);
    assert_eq!(
        platform.fabric_records(),
        1,
        "the control refused the command and nothing was written down; a refusal nobody can \
         replay is a decision that did not happen"
    );
    Ok(())
}

#[test]
fn a_fractional_json_number_is_refused_before_any_command_is_deserialised() -> Result<()> {
    // Written as a number rather than a string. `Decimal`'s deserialiser
    // falls through to `from_f64` for anything that is not an exact integer,
    // so this is the seam where a cap the desk wrote as 0.1 would become the
    // nearest binary double and the corridor would carry a ceiling nobody
    // wrote.
    let text = r#"{"commands": [{"subject": "destination", "action": "propose",
                    "key": {"asset": "USD", "address": "x"},
                    "by": "desk", "at": "2026-09-05T06:00:00Z", "cap": 0.1}]}"#;
    // The premise: the same document with the number as a string gets past
    // this check and is refused later, for being the wrong shape rather than
    // for carrying a float. Without this half, a test that only asserts a
    // refusal passes when everything is refused.
    let as_string = text.replace("0.1", "\"0.1\"");
    let string_error = Declaration::parse(&as_string)
        .expect_err("an unknown field is refused whichever way it is written");
    assert!(
        !string_error.message().contains("exact integer"),
        "the string form was refused by the number check, so this test cannot show the check is \
         about floats: {}",
        string_error.message()
    );

    let error = Declaration::parse(text).expect_err("a fractional JSON number is refused");
    assert!(
        error.message().contains("exact integer"),
        "the refusal does not name the rule it applied: {}",
        error.message()
    );
    assert!(
        error.message().contains("commands[0].cap"),
        "the refusal does not say where to look: {}",
        error.message()
    );
    Ok(())
}

#[test]
fn a_whole_json_number_is_admitted_because_it_crosses_to_a_decimal_exactly() -> Result<()> {
    // The other half of the gate above, and the half that distinguishes a
    // working check from one that refuses everything. An integer reaches
    // `Decimal` through the `i64` arm with no float in the path, so refusing
    // it would be refusing a figure that is exactly what was written.
    let text = format!(
        r#"{{"commands": [{{"subject": "destination", "action": "propose",
             "key": {{"asset": "USD", "address": "{ADDRESS}"}},
             "by": "desk", "at": "{}"}}], "trailing": 7}}"#,
        start().to_rfc3339()
    );
    let error = Declaration::parse(&text)
        .expect_err("an unknown top-level key is refused whatever it holds");
    assert!(
        !error.message().contains("exact integer"),
        "an integer was refused by the float check, so a cap written 7 could not be declared: {}",
        error.message()
    );
    assert!(
        error.message().contains("trailing"),
        "the refusal names something other than the key it refused: {}",
        error.message()
    );
    Ok(())
}

#[test]
fn a_field_the_command_does_not_have_is_refused_rather_than_dropped_in_silence() -> Result<()> {
    // The command types do not deny unknown fields, because they are the
    // log's own decode and a replay has to keep reading records written
    // before a field existed. So an operator's misspelling would be dropped
    // between the file and the chain, and the destination journalled would be
    // keyed on something nobody meant.
    //
    // The premise is the same document without the extra field, which must
    // parse: without it this test would pass on a parser that refused
    // everything.
    let sound = declaration_of(&[destination("propose", "treasury-desk")]);
    let parsed = Declaration::parse(&sound)?;
    assert_eq!(
        parsed.len(),
        1,
        "the premise fails: the command without the extra field does not parse"
    );

    let with_extra = sound.replace(
        r#""by": "treasury-desk""#,
        r#""by": "treasury-desk", "urgency": "high""#,
    );
    assert_ne!(
        with_extra, sound,
        "the premise fails: the extra field was not inserted"
    );
    let error =
        Declaration::parse(&with_extra).expect_err("a field the command does not have is refused");
    assert!(
        error.message().contains("urgency"),
        "the refusal does not name the field it refused, so an operator cannot find it: {}",
        error.message()
    );

    // And nested, one level down inside the destination key, because a
    // misspelling is at least as likely there as at the top.
    let nested = sound.replace(
        &format!(r#""address": "{ADDRESS}""#),
        &format!(r#""address": "{ADDRESS}", "chain": "mainnet""#),
    );
    let error = Declaration::parse(&nested)
        .expect_err("a field the destination key does not have is refused");
    assert!(
        error.message().contains("chain") && error.message().contains("key"),
        "the refusal does not say which field, in which part of the command: {}",
        error.message()
    );
    Ok(())
}

#[test]
fn an_empty_declaration_is_refused_rather_than_serving_as_one_that_declares_nothing() {
    let error =
        Declaration::parse(r#"{"commands": []}"#).expect_err("a declaration of nothing is refused");
    assert!(
        error.message().contains(FABRIC_PATH_VARIABLE),
        "the refusal does not say how to run with no declaration at all: {}",
        error.message()
    );
}

#[test]
fn a_declaration_past_the_burst_bound_is_refused_and_not_truncated() {
    let one = destination("propose", "desk");
    let commands: Vec<String> = std::iter::repeat_n(one, MAX_FABRIC_COMMANDS + 1).collect();
    let error = Declaration::parse(&declaration_of(&commands))
        .expect_err("a declaration past the bound is refused");
    assert!(
        error
            .message()
            .contains(&(MAX_FABRIC_COMMANDS + 1).to_string()),
        "the refusal does not name the count it refused, so an operator cannot tell how far past \
         the bound the file is: {}",
        error.message()
    );
}

#[test]
fn a_command_appended_while_the_process_serves_is_applied_and_the_prefix_is_not() -> Result<()> {
    let directory = fixture_dir("append");
    let first = destination("propose", "treasury-desk");
    let path = write_fixture(&directory, &declaration_of(std::slice::from_ref(&first)));
    let mut platform = platform()?;
    let mut feed = FabricFeed::open(&path)?;

    assert_eq!(
        platform.fabric_records(),
        0,
        "the premise is an empty chain"
    );
    assert_eq!(feed.apply_pending(&mut platform, start())?, 1);
    assert_eq!(platform.fabric_records(), 1);

    // The operator appends a second act. The first is history and must not be
    // offered to the control again.
    let second = destination("verify", "treasury-reviewer");
    std::fs::write(&path, declaration_of(&[first, second])).expect("the fixture is rewritten");
    touch(
        &path,
        std::time::SystemTime::now() + std::time::Duration::from_secs(60),
    );

    let appended = feed
        .refresh()?
        .expect("the file moved, so the refresh sees a change");
    assert_eq!(appended, 1, "the refresh counted the prefix as new work");
    assert_eq!(feed.apply_pending(&mut platform, start())?, 1);
    assert_eq!(
        platform.fabric_records(),
        2,
        "the appended command did not reach the chain, or the prefix reached it twice"
    );
    Ok(())
}

#[test]
fn an_edited_command_that_was_already_journalled_stops_the_feed() -> Result<()> {
    let directory = fixture_dir("amend");
    // Two approver names of the same length, so the edit does not move the
    // file's length and the prefix check is what catches it rather than the
    // fingerprint.
    let before = destination("propose", "treasury-desk-a");
    let path = write_fixture(&directory, &declaration_of(&[before]));
    let mut platform = platform()?;
    let mut feed = FabricFeed::open(&path)?;
    assert_eq!(feed.apply_pending(&mut platform, start())?, 1);
    assert_eq!(
        platform.fabric_records(),
        1,
        "the premise is one command already on the chain"
    );

    let after = destination("propose", "treasury-desk-b");
    std::fs::write(&path, declaration_of(&[after])).expect("the fixture is rewritten");
    touch(
        &path,
        std::time::SystemTime::now() + std::time::Duration::from_secs(60),
    );

    let error = feed
        .refresh()
        .expect_err("an amended act that is already on the chain stops the feed");
    assert!(
        error.message().contains("commands[0]"),
        "the refusal does not name the command that changed: {}",
        error.message()
    );
    assert!(
        error.message().contains("append"),
        "the refusal does not name the safe alternative, so an operator is told no and not told \
         what to do instead: {}",
        error.message()
    );
    assert_eq!(
        platform.fabric_records(),
        1,
        "the refused refresh journalled something anyway"
    );
    Ok(())
}

#[test]
fn a_removed_command_that_was_already_journalled_stops_the_feed() -> Result<()> {
    let directory = fixture_dir("shrink");
    let first = destination("propose", "treasury-desk");
    let second = destination("verify", "treasury-reviewer");
    let path = write_fixture(&directory, &declaration_of(&[first.clone(), second]));
    let mut platform = platform()?;
    let mut feed = FabricFeed::open(&path)?;
    assert_eq!(feed.apply_pending(&mut platform, start())?, 2);
    assert_eq!(feed.applied(), 2, "the premise is two commands applied");

    std::fs::write(&path, declaration_of(&[first])).expect("the fixture is rewritten");
    touch(
        &path,
        std::time::SystemTime::now() + std::time::Duration::from_secs(60),
    );

    let error = feed
        .refresh()
        .expect_err("removing an applied act does not remove its record");
    assert!(
        error.message().contains("append"),
        "the refusal does not name the safe alternative: {}",
        error.message()
    );
    Ok(())
}

#[test]
fn no_refusal_repeats_what_the_declaration_says() -> Result<()> {
    // A declaration names accounts and counterparties, and a refusal reaches
    // whoever can call the route, the process's stderr and whichever ticket
    // the line is pasted into. Every message names the position, the key or
    // the class and stops there.
    let malformed = format!(
        r#"{{"commands": [{{"subject": "destination", "action": "propose",
             "key": {{"asset": "USD", "address": "{ADDRESS}"}},
             "by": "treasury-desk"}}]}}"#
    );
    let error = Declaration::parse(&malformed).expect_err("a command missing `at` is refused");
    assert!(
        error.message().contains("commands[0]"),
        "the premise fails: the refusal does not name the position, so this test would pass on a \
         message that said nothing at all: {}",
        error.message()
    );
    assert!(
        !error.message().contains(ADDRESS),
        "the refusal repeats the address the declaration names: {}",
        error.message()
    );

    let unparseable = format!(r#"{{"commands": [{{"address": "{ADDRESS}""#);
    let error = Declaration::parse(&unparseable).expect_err("a truncated file is refused");
    assert!(
        error.message().contains("line"),
        "the premise fails: the refusal does not give a position: {}",
        error.message()
    );
    assert!(
        !error.message().contains(ADDRESS),
        "the syntax refusal quotes the bytes it stopped on, and those bytes are the desk's \
         document: {}",
        error.message()
    );
    Ok(())
}

#[test]
fn an_unset_variable_is_no_declaration_rather_than_a_refusal() -> Result<()> {
    let absent = FabricFeed::from_env(&|_| None)?;
    assert!(
        absent.is_none(),
        "an unset variable produced a feed, so a process that declares nothing would claim to \
         have declared something"
    );
    let blank = FabricFeed::from_env(&|_| Some("   ".to_string()))?;
    assert!(
        blank.is_none(),
        "a variable set to whitespace is the same statement as an unset one and must read the \
         same way"
    );
    Ok(())
}

#[test]
fn a_named_declaration_that_does_not_read_refuses_rather_than_falling_back_to_none() {
    let path = fixture_dir("missing")
        .join("not-written.json")
        .display()
        .to_string();
    let error = FabricFeed::from_env(&|_| Some(path.clone()))
        .expect_err("a named file that does not read is a refusal, not an absent declaration");
    assert!(
        error.message().contains(FABRIC_PATH_VARIABLE),
        "the refusal does not name the variable that caused it: {}",
        error.message()
    );
}

// --- the transfer gate, reached through the declaration -----------------------
//
// CAPITAL-030 and CAPITAL-031. The gate's seven checks are proven check by
// check in `qip-capital-fabric`'s own suite. What nothing proved is that an
// operator's file can put an assessment in front of them: until the
// declaration carried `corridor_policy`, no composition root declared a
// corridor policy, `Platform::decide_fabric` refused every gate command that
// named a proposed corridor, and the gate assessed nothing in any deployed
// process. These tests go in through the door a deployment uses — a file, a
// `FabricFeed`, `apply_pending` — and read the verdicts off the platform.
//
// The commands are built from the fabric's own types and serialised, rather
// than written out as JSON, because two of a gate command's three
// attestations are digests of the movement itself. The strategy fixture is
// mirrored from `mesh.rs` rather than shared with it, for the reason given
// there: a fixture crate would be a dependency.

use qip_capital_fabric::assessment::AssessmentId;
use qip_capital_fabric::corridor::{CorridorCaps, CorridorId, CorridorStage, PermittedHours};
use qip_capital_fabric::custody::{
    Attestation, CorridorKind, CustodyClass, CustodyPolicy, EnforcementPoint, EnforcementPoints,
    Identity, TransferAuthority,
};
use qip_capital_fabric::destination::{
    ACTIVATION_DELAY, Approver, Asset, DestinationKey, SignatureRecord,
};
use qip_capital_fabric::gate::{
    CarriedTransfer, FundingStanding, GateCheck, KillSwitchState, SourceBalances, StatedPurpose,
    TransferHistory, TransferIntent, VelocityState,
};
use qip_capital_fabric::journal::{
    CorridorAction, CorridorStep, DestinationAction, FabricCommand, GateCommand, GateVerdict,
};
use qip_capital_fabric::location::{CapitalLocation, Region};
use qip_contracts::capital::{CapitalEnvelope, Utilisation};
use qip_contracts::feature::FeatureKey;
use qip_contracts::gate::GateStage;
use qip_contracts::governance::Approval;
use qip_contracts::signal::{SignalKind, StrategyId};
use qip_contracts::venue::VenueId;
use qip_core::rng::{Rng, Xoshiro256};
use qip_core::time::Duration;
use qip_core::{Currency, Decimal, ObjectId, dec};
use qip_kernel::central::StrategyCandidate;
use qip_lifecycle::evidence::{
    CrossValidationRun, DatasetManifest, FeatureTiming, HoldoutEvidence, KillCondition,
    LeakageAudit, PaperEvidence, PilotEvidence, ScaledEvidence, ShadowDecision, ShadowEvidence,
    StrategyEvidence,
};
use qip_lifecycle::trials::StrategyFamily;
use qip_simulation_engine::validation::PurgedSplit;
use qip_strategy::catalogue::FeatureCatalogue;
use qip_strategy::compile::{CompiledStrategy, StrategyCompiler};
use qip_strategy::ir::{Expr, Rule, StrategySpec, Type};
use qip_strategy::program::Program;

/// The strategy the corridors fund.
const DESK: &str = "arb-desk";
const DESK_CELL: &str = "london-1";
/// The corridor's full ceiling, and the one it is narrowed to while the
/// strategy it funds is at pilot. Both are the desk's figures.
const CEILING: &str = "900";
const PILOT_CEILING: &str = "400";
/// The signed per-transfer cap, deliberately below the pilot ceiling so the
/// two bounds refuse different amounts.
const PER_TRANSFER: &str = "350";

fn desk() -> StrategyId {
    StrategyId::new(DESK)
}

fn at(offset: Duration) -> Timestamp {
    start().saturating_add(offset)
}

/// When the corridor's delay has elapsed.
fn activated_at() -> Timestamp {
    start().saturating_add(ACTIVATION_DELAY)
}

/// When the gate is asked: an hour after activation.
fn gate_time() -> Timestamp {
    activated_at().saturating_add(Duration::from_hours(1))
}

fn compile(id: &str) -> Result<(CompiledStrategy, Program)> {
    let subject = ObjectId::from_string(format!("obj-{id}"));
    let pressure = FeatureKey::new("book_pressure", subject.clone()).with("levels", 5);
    let mut catalogue = FeatureCatalogue::new();
    catalogue.declare(pressure.clone(), Type::Statistic)?;
    let spec = StrategySpec::new(StrategyId::new(id), subject, Duration::from_millis(250))
        .with_rule(Rule::new(
            "enter",
            SignalKind::Enter,
            Expr::feature(pressure).greater_than(Expr::Statistic(0.4)),
            Expr::Exact(Decimal::from_int(100)),
            Expr::Statistic(0.62),
            500,
        ));
    let mut compiler = StrategyCompiler::new(catalogue);
    let compiled = compiler.compile(&spec)?;
    Ok((compiled, compiler.into_program()))
}

fn good_returns(seed: u64, n: usize, drift: f64) -> Vec<f64> {
    let mut rng = Xoshiro256::seeded(seed);
    (0..n)
        .map(|_| {
            let u = rng.next_f64() + rng.next_f64() - 1.0;
            drift + u * 0.01
        })
        .collect()
}

fn honest_cross_validation(observations: usize) -> Result<CrossValidationRun> {
    let (folds, label_horizon, embargo) = (5, 10, 5);
    let splits = PurgedSplit::new(folds, label_horizon, embargo)?.split(observations)?;
    Ok(CrossValidationRun {
        folds,
        label_horizon,
        embargo,
        observations,
        purged: splits.iter().map(|s| s.purged).sum(),
        embargoed: splits.iter().map(|s| s.embargoed).sum(),
    })
}

fn dual_approval(subject: &str, at: Timestamp, rationale: &str) -> Result<Approval> {
    Approval::new(subject, "alice.chen", at, rationale)?.countersigned_by("bram.oduya")
}

fn full_evidence(id: &StrategyId, cell: &str) -> Result<StrategyEvidence> {
    let observations = 400;
    let holdout = HoldoutEvidence {
        holdout_returns: good_returns(1, observations, 0.0018),
        in_sample_folds: (0..5).map(|f| good_returns(10 + f, 80, 0.0020)).collect(),
        out_of_sample_folds: (0..5).map(|f| good_returns(20 + f, 80, 0.0018)).collect(),
        trials: 12,
        periods_per_year: 252.0,
        cross_validation: honest_cross_validation(observations)?,
        leakage: LeakageAudit {
            timings: (0..8)
                .map(|i| FeatureTiming {
                    feature: format!("feature-{i}"),
                    known_at: start(),
                    used_at: at(Duration::from_hours(1)),
                })
                .collect(),
            restated_without_snapshots: Vec::new(),
        },
    };
    let paper = PaperEvidence {
        against_live_data: true,
        assumed_cost_bps: 8.0,
        realised_cost_bps: (0..400).map(|i| 7.0 + f64::from(i % 5) * 0.2).collect(),
        peak_participation: 0.04,
        modelled_participation_limit: 0.10,
        unfillable_orders: 4,
        filled_orders: 400,
    };
    let shadow = ShadowEvidence {
        decisions: (0..400)
            .map(|i| ShadowDecision {
                at: at(Duration::from_mins(i)),
                object_id: ObjectId::from_string(format!("obj-{}", i % 20)),
                live: SignalKind::Enter,
                predicted: SignalKind::Enter,
                live_quantity: dec!("100"),
                predicted_quantity: dec!("100"),
            })
            .collect(),
        orders_reached_a_venue: false,
        decision_latency_p99: Duration::from_millis(40),
    };
    let proposed = CapitalEnvelope::new(
        id.clone(),
        cell,
        dec!("250000"),
        dec!("250000"),
        dec!("250000"),
        vec![VenueId::new("XNYS")],
        start(),
        at(Duration::from_days(14)),
        "alice.chen",
        "proposed-not-issued",
    )?;
    let pilot = PilotEvidence {
        approval: Some(dual_approval(
            &format!("{id} pilot"),
            start(),
            "shadow agreement held at 100% over 400 decisions",
        )?),
        envelope: Some(proposed),
        kill_conditions: vec![
            KillCondition::RealisedLoss(dec!("25000")),
            KillCondition::Drawdown(0.08),
            KillCondition::ConsecutiveLosingDays(5),
        ],
    };
    let scaled = ScaledEvidence {
        pilot_returns: good_returns(99, 120, 0.0030),
        pilot_started_at: start(),
        pilot_utilisation: Utilisation {
            gross_committed: dec!("180000"),
            realised_loss: dec!("0"),
            orders_sent: 5_400,
        },
        proposed_notional: dec!("1000000"),
        modelled_capacity: dec!("4000000"),
        pilot_approval: Some(dual_approval(
            &format!("{id} pilot"),
            start(),
            "shadow agreement held at 100% over 400 decisions",
        )?),
        scaling_approval: Some(dual_approval(
            &format!("{id} scaling"),
            at(Duration::from_days(120)),
            "ninety days at pilot returned a 0.7 Sharpe inside a quarter of capacity",
        )?),
    };
    Ok(StrategyEvidence::new()
        .with_holdout(holdout)
        .with_simulation(DatasetManifest::new(
            "obj-AAA",
            "XNYS",
            2_000,
            start().saturating_sub(Duration::from_days(2_000)),
            start(),
            qip_core::sha256_hex(b"recorded bars"),
        )?)
        .with_paper(paper)
        .with_shadow(shadow)
        .with_pilot(pilot)
        .with_scaled(scaled))
}

/// Register the desk's strategy with evidence that passes every gate and walk
/// it to pilot, so a corridor funding it is narrowed rather than suspended.
fn promote_the_desk_to_pilot(platform: &mut Platform) -> Result<()> {
    let id = desk();
    let (compiled, program) = compile(DESK)?;
    let candidate = StrategyCandidate::new(
        compiled,
        program,
        StrategyFamily::new("fabric-tests")?,
        DESK_CELL,
        VenueId::new("XNYS"),
        start(),
    )?
    .with_evidence(full_evidence(&id, DESK_CELL)?)
    .with_model("microprice-distilled@3")
    .with_evidence_artifacts(vec![
        format!("sha256:holdout-{id}"),
        format!("sha256:shadow-{id}"),
    ]);
    let factory = platform.central_mut().factory_mut();
    factory.register(candidate)?;
    for rung in [
        GateStage::Holdout,
        GateStage::Paper,
        GateStage::Shadow,
        GateStage::Pilot,
    ] {
        let approval = if rung.requires_human_approval() {
            Some(dual_approval(
                DESK,
                start(),
                "every gate check passed with the evidence attached",
            )?)
        } else {
            None
        };
        factory.promote(&id, approval, "the gate passed", start())?;
    }
    Ok(())
}

/// Where the declared corridors leave from. `venue` distinguishes the signed
/// corridor's source from the unsigned one's.
fn corridor_source(venue: &str) -> CapitalLocation {
    CapitalLocation::new(Region::new("home"), Currency::USD, VenueId::new(venue))
}

fn treasury_account() -> Result<DestinationKey> {
    DestinationKey::new(Asset::new("USD")?, ADDRESS)
}

fn signed_corridor() -> Result<CorridorId> {
    CorridorId::new("treasury-sweep")
}

fn unsigned_corridor() -> Result<CorridorId> {
    CorridorId::new("unsigned-sweep")
}

fn json_of(command: &FabricCommand) -> String {
    serde_json::to_string(command).expect("a fabric command serialises")
}

/// One policy subject, as an operator writes it.
fn policy_subject(source: &CapitalLocation, ceiling: &str, pilot_ceiling: &str) -> String {
    format!(
        r#"{{"route": {{"source": "{source}", "destination": "{ADDRESS}", "asset": "USD"}},
             "ceiling": "{ceiling}", "pilot_ceiling": "{pilot_ceiling}", "funds": ["{DESK}"]}}"#
    )
}

fn declaration_with_policy(policy: &[String], commands: &[String]) -> String {
    format!(
        r#"{{"corridor_policy": [{}], "commands": [{}]}}"#,
        policy.join(","),
        commands.join(",")
    )
}

fn propose_corridor(id: &CorridorId, source: CapitalLocation) -> Result<FabricCommand> {
    Ok(FabricCommand::Corridor(CorridorAction::Propose {
        id: id.clone(),
        source,
        source_class: CustodyClass::FiatAtInstitutionOfRecord,
        kind: CorridorKind::InstitutionApprovalFlow,
        destination: treasury_account()?,
        caps: CorridorCaps::new(
            Decimal::parse(PER_TRANSFER).expect("a literal cap"),
            dec!("600"),
            dec!("1000"),
            dec!("5000"),
            Duration::from_mins(15),
            PermittedHours::ALL_DAY,
        )?,
        purpose: "sweep realised cash to the treasury account".to_string(),
        by: Approver::new("alice")?,
        at: start(),
    }))
}

fn corridor_step(id: &CorridorId, step: CorridorStep) -> FabricCommand {
    FabricCommand::Corridor(CorridorAction::Step {
        id: id.clone(),
        step,
    })
}

/// The acts that put a usable destination, one corridor walked to active and
/// one corridor reviewed but never signed on the chain.
fn corridor_acts() -> Result<Vec<String>> {
    let key = treasury_account()?;
    let signed = signed_corridor()?;
    let unsigned = unsigned_corridor()?;
    let review = || -> Result<CorridorStep> {
        Ok(CorridorStep::Review {
            by: Approver::new("bob")?,
            at: start(),
        })
    };
    let commands = vec![
        FabricCommand::Destination(DestinationAction::Propose {
            key: key.clone(),
            by: Approver::new("alice")?,
            at: start(),
        }),
        FabricCommand::Destination(DestinationAction::Verify {
            key: key.clone(),
            by: Approver::new("bob")?,
            at: start(),
        }),
        FabricCommand::Destination(DestinationAction::RecordSignature {
            key,
            signature: SignatureRecord::new(Approver::new("carol")?, start(), "vault/dest/1")?,
        }),
        propose_corridor(&signed, corridor_source("simulated-venue"))?,
        corridor_step(&signed, review()?),
        corridor_step(
            &signed,
            CorridorStep::RecordSignature {
                signature: SignatureRecord::new(
                    Approver::new("carol")?,
                    start(),
                    "vault/corridor/1",
                )?,
            },
        ),
        corridor_step(&signed, CorridorStep::BeginDelay { now: start() }),
        corridor_step(
            &signed,
            CorridorStep::Activate {
                now: activated_at(),
            },
        ),
        // Drawn and reviewed, and nobody signed it.
        propose_corridor(&unsigned, corridor_source("other-venue"))?,
        corridor_step(&unsigned, review()?),
    ];
    Ok(commands.iter().map(json_of).collect())
}

/// A gate command for `amount` along `corridor`, with every input satisfied
/// that the caller did not vary: three independent attestations bound to this
/// movement, the blueprint's custody table, a full source balance, no
/// breaker tripped. The ruling is the one the platform derives, because that
/// is the only ruling `decide_fabric` will put to the gate.
fn gate_command(
    platform: &Platform,
    corridor: &CorridorId,
    source: CapitalLocation,
    destination: DestinationKey,
    amount: Decimal,
    history: TransferHistory,
) -> Result<String> {
    let now = gate_time();
    let custody = CustodyPolicy::blueprint();
    let mut points = EnforcementPoints::new();
    for (point, identity, reference) in [
        (
            EnforcementPoint::TransferGate,
            "gate-svc",
            AssessmentId::of(corridor, &source, &destination, amount, now).to_string(),
        ),
        (
            EnforcementPoint::CustodyPolicy,
            "custody-policy-svc",
            custody.fingerprint().to_string(),
        ),
        (
            EnforcementPoint::VenueAllowlist,
            "venue-ops-oob",
            format!("{}-record-1", EnforcementPoint::VenueAllowlist.as_str()),
        ),
    ] {
        points.attest(Attestation::new(
            point,
            Identity::new(identity)?,
            reference,
            start(),
        )?)?;
    }
    Ok(json_of(&FabricCommand::Gate(GateCommand {
        intent: TransferIntent::new(
            source,
            destination,
            amount,
            StatedPurpose::new(dec!("1000"), dec!("500"))?,
        )?,
        corridor: corridor.clone(),
        custody,
        authority: TransferAuthority::new(points, Identity::new("trading-svc")?),
        funding: platform.corridor_funding(corridor)?,
        history,
        balances: SourceBalances::new(dec!("10000"), dec!("0"), dec!("0"), dec!("0"))?,
        velocity: VelocityState::CLEAR,
        kill_switch: KillSwitchState::Armed,
        now,
    })))
}

/// A process serving over a declaration that states the policy and the
/// corridors, with the desk's strategy at pilot. Holds what a test needs to
/// append gate commands the way an operator would.
struct ServingDesk {
    platform: Platform,
    feed: FabricFeed,
    path: String,
    policy: Vec<String>,
    commands: Vec<String>,
}

impl ServingDesk {
    fn open(name: &str) -> Result<Self> {
        let mut platform = platform()?;
        promote_the_desk_to_pilot(&mut platform)?;
        let policy = vec![
            policy_subject(&corridor_source("simulated-venue"), CEILING, PILOT_CEILING),
            policy_subject(&corridor_source("other-venue"), CEILING, PILOT_CEILING),
        ];
        let commands = corridor_acts()?;
        let path = write_fixture(
            &fixture_dir(name),
            &declaration_with_policy(&policy, &commands),
        );
        let mut feed = FabricFeed::open(&path)?;
        feed.apply_pending(&mut platform, start())?;
        Ok(Self {
            platform,
            feed,
            path,
            policy,
            commands,
        })
    }

    /// Append commands to the file and let the feed pick them up, as an
    /// admitted cycle would.
    fn append(&mut self, appended: Vec<String>) -> Result<usize> {
        self.commands.extend(appended);
        std::fs::write(
            &self.path,
            declaration_with_policy(&self.policy, &self.commands),
        )
        .expect("the fixture is rewritten");
        touch(
            &self.path,
            std::time::SystemTime::now() + std::time::Duration::from_secs(60),
        );
        self.feed.refresh()?;
        self.feed.apply_pending(&mut self.platform, gate_time())
    }

    /// The verdict of each assessment made since `from`, as the check that
    /// vetoed it or `None` for an admission.
    fn verdicts_since(&self, from: usize) -> Vec<Option<GateCheck>> {
        self.platform.fabric_state().assessments()[from..]
            .iter()
            .map(|assessment| match &assessment.verdict {
                GateVerdict::Admitted(_) => None,
                GateVerdict::Vetoed(vetoed) => Some(vetoed.check),
            })
            .collect()
    }
}

#[test]
fn a_declared_corridor_policy_is_what_makes_a_declared_gate_command_assessable() -> Result<()> {
    // The premise, and the defect: the same acts with no `corridor_policy`.
    // The corridor is proposed and active, the strategy holds capital, and
    // the platform still has no ruling for the corridor — so a gate command
    // naming it could state none that `decide_fabric` would accept, which
    // stopped the feed in every deployment.
    let mut bare = platform()?;
    promote_the_desk_to_pilot(&mut bare)?;
    let acts = corridor_acts()?;
    let without = Declaration::parse(&declaration_of(&acts))?;
    assert_eq!(without.corridors_ruled(), 0);
    without.apply_into(&mut bare, 0, start())?;
    assert_eq!(
        bare.fabric_state()
            .corridor(&signed_corridor()?)
            .map(|corridor| corridor.stage()),
        Some(CorridorStage::Active),
        "the premise is an active corridor, so the refusal is about the policy"
    );
    let refusal = bare
        .corridor_funding(&signed_corridor()?)
        .expect_err("a corridor nobody declared a policy for was given a ruling");
    assert!(
        refusal
            .message()
            .contains("no corridor policy has been declared"),
        "{}",
        refusal.message()
    );

    // With the policy declared in the same file, the ruling exists, it is the
    // pilot ceiling because the strategy the corridor funds is at pilot, and
    // a gate command stating it is assessed and recorded.
    let mut desk = ServingDesk::open("policy")?;
    assert!(
        desk.platform
            .central()
            .factory()
            .holds_capital(&self::desk())
    );
    let funding = desk.platform.corridor_funding(&signed_corridor()?)?;
    assert_eq!(funding.standing(), FundingStanding::Narrowed);
    assert_eq!(funding.permitted(), dec!("400"));
    assert!(
        desk.platform.fabric_state().assessments().is_empty(),
        "the premise is that no assessment has been made"
    );

    let command = gate_command(
        &desk.platform,
        &signed_corridor()?,
        corridor_source("simulated-venue"),
        treasury_account()?,
        dec!("300"),
        TransferHistory::empty(),
    )?;
    assert_eq!(desk.append(vec![command])?, 1);
    assert_eq!(desk.verdicts_since(0), vec![None], "the gate did not admit");
    Ok(())
}

#[test]
fn a_declared_transfer_off_the_signed_corridor_set_is_refused_and_one_on_a_signed_corridor_passes()
-> Result<()> {
    let mut desk = ServingDesk::open("corridor-authority")?;
    let signed = signed_corridor()?;
    let unsigned = unsigned_corridor()?;
    // Premise: one corridor is active with a signature record, the other was
    // reviewed and never signed, and nothing has been assessed.
    let state = desk.platform.fabric_state();
    assert_eq!(
        state.corridor(&signed).map(|c| (c.stage(), c.is_signed())),
        Some((CorridorStage::Active, true))
    );
    assert_eq!(
        state
            .corridor(&unsigned)
            .map(|c| (c.stage(), c.is_signed())),
        Some((CorridorStage::Reviewed, false))
    );
    assert!(state.assessments().is_empty());
    let records_before = desk.platform.fabric_records();

    let elsewhere = DestinationKey::new(Asset::new("USD")?, "an-account-no-corridor-names")?;
    let never_proposed = CorridorId::new("never-proposed")?;
    let appended = vec![
        // On the signed corridor, along its own route.
        gate_command(
            &desk.platform,
            &signed,
            corridor_source("simulated-venue"),
            treasury_account()?,
            dec!("300"),
            TransferHistory::empty(),
        )?,
        // Naming the signed corridor, running somewhere it does not.
        gate_command(
            &desk.platform,
            &signed,
            corridor_source("simulated-venue"),
            elsewhere,
            dec!("300"),
            TransferHistory::empty(),
        )?,
        // Leaving from a source the signed corridor does not leave from.
        gate_command(
            &desk.platform,
            &signed,
            corridor_source("other-venue"),
            treasury_account()?,
            dec!("300"),
            TransferHistory::empty(),
        )?,
        // Along a corridor nobody signed.
        gate_command(
            &desk.platform,
            &unsigned,
            corridor_source("other-venue"),
            treasury_account()?,
            dec!("300"),
            TransferHistory::empty(),
        )?,
    ];
    assert_eq!(desk.append(appended)?, 4);

    assert_eq!(
        desk.verdicts_since(0),
        vec![
            None,
            Some(GateCheck::CorridorAuthority),
            Some(GateCheck::CorridorAuthority),
            Some(GateCheck::CorridorAuthority),
        ]
    );
    // Each veto is for the reason it should be, not merely at the same check.
    let reasons: Vec<&str> = desk.platform.fabric_state().assessments()[1..]
        .iter()
        .map(|assessment| match &assessment.verdict {
            GateVerdict::Vetoed(vetoed) => vetoed.reason.as_str(),
            GateVerdict::Admitted(_) => "admitted",
        })
        .collect();
    assert!(
        reasons[0].contains("only against the corridor it names"),
        "{}",
        reasons[0]
    );
    assert!(
        reasons[1].contains("only against the corridor it names"),
        "{}",
        reasons[1]
    );
    assert!(
        reasons[2].contains("is reviewed, not active"),
        "{}",
        reasons[2]
    );
    assert_eq!(desk.platform.fabric_records(), records_before + 4);

    // A route matching no corridor at all is refused as a record, and no
    // assessment is made of it. Its ruling cannot be derived — there is no
    // corridor to look one up by — so the command carries the signed
    // corridor's, which the platform does not check for a corridor that does
    // not exist and the journal never reaches.
    let assessed = desk.platform.fabric_state().assessments().len();
    let mut ghost: serde_json::Value = serde_json::from_str(&gate_command(
        &desk.platform,
        &signed,
        corridor_source("simulated-venue"),
        treasury_account()?,
        dec!("300"),
        TransferHistory::empty(),
    )?)
    .expect("a gate command is JSON");
    ghost["corridor"] = serde_json::to_value(&never_proposed).expect("an id serialises");
    assert_eq!(desk.append(vec![ghost.to_string()])?, 1);
    assert_eq!(
        desk.platform.fabric_state().assessments().len(),
        assessed,
        "a transfer along a corridor nobody proposed was assessed"
    );
    assert_eq!(desk.platform.fabric_records(), records_before + 5);
    Ok(())
}

#[test]
fn for_generated_declared_transfers_every_one_over_a_cap_is_vetoed_whole_and_every_one_under_all_of_them_is_admitted_at_the_amount_asked()
-> Result<()> {
    let mut desk = ServingDesk::open("caps")?;
    let signed = signed_corridor()?;
    let per_transfer = Decimal::parse(PER_TRANSFER).expect("a literal cap");
    // Premise: the corridor is narrowed to 400 and signed at 350 a transfer,
    // so the two bounds are different numbers and both can be the one that
    // refuses.
    assert_eq!(
        desk.platform.corridor_funding(&signed)?.permitted(),
        dec!("400")
    );
    assert!(desk.platform.fabric_state().assessments().is_empty());

    let mut rng = Xoshiro256::seeded(0x00CA_0031);
    let amounts: Vec<Decimal> = (0..60)
        .map(|_| Decimal::from_int(1 + rng.below(500) as i64))
        .collect();
    let mut appended = Vec::new();
    for amount in &amounts {
        appended.push(gate_command(
            &desk.platform,
            &signed,
            corridor_source("simulated-venue"),
            treasury_account()?,
            *amount,
            TransferHistory::empty(),
        )?);
    }
    assert_eq!(desk.append(appended)?, amounts.len());

    let assessments = desk.platform.fabric_state().assessments();
    assert_eq!(assessments.len(), amounts.len());
    let (mut admitted, mut over_transfer_cap, mut over_ceiling) = (0, 0, 0);
    for (amount, assessment) in amounts.iter().zip(assessments) {
        match &assessment.verdict {
            GateVerdict::Admitted(approved) => {
                assert!(
                    *amount <= per_transfer,
                    "{amount} is over the per-transfer cap and was admitted"
                );
                // Admitted at the amount asked: nothing was cut to fit.
                assert_eq!(approved.intent().amount(), *amount);
                admitted += 1;
            }
            GateVerdict::Vetoed(vetoed) => {
                assert!(
                    *amount > per_transfer,
                    "{amount} is under every cap and was vetoed: {}",
                    vetoed.reason
                );
                assert_eq!(vetoed.check, GateCheck::Caps, "{}", vetoed.reason);
                if *amount > dec!("400") {
                    over_ceiling += 1;
                } else {
                    over_transfer_cap += 1;
                }
            }
        }
    }
    // Premise: the sweep landed on both sides of both bounds.
    assert!(
        admitted > 10 && over_transfer_cap > 2 && over_ceiling > 5,
        "{admitted} {over_transfer_cap} {over_ceiling}"
    );

    // The rolling caps, which no single amount can reach: 400 carried in the
    // last hour against an hourly cap of 600. 250 would make 650 and is
    // vetoed whole; 200 makes exactly 600 and passes.
    let carried = || {
        TransferHistory::new(vec![
            CarriedTransfer {
                at: gate_time().saturating_sub(Duration::from_mins(50)),
                amount: dec!("200"),
            },
            CarriedTransfer {
                at: gate_time().saturating_sub(Duration::from_mins(20)),
                amount: dec!("200"),
            },
        ])
    };
    let from = desk.platform.fabric_state().assessments().len();
    let rolling = vec![
        gate_command(
            &desk.platform,
            &signed,
            corridor_source("simulated-venue"),
            treasury_account()?,
            dec!("250"),
            carried()?,
        )?,
        gate_command(
            &desk.platform,
            &signed,
            corridor_source("simulated-venue"),
            treasury_account()?,
            dec!("200"),
            carried()?,
        )?,
    ];
    assert_eq!(desk.append(rolling)?, 2);
    assert_eq!(desk.verdicts_since(from), vec![Some(GateCheck::Caps), None]);

    // The derived ceiling on its own. Above, every amount over the ceiling
    // was also over the signed per-transfer cap, so a gate that ignored the
    // ceiling would have refused the same transfers. The desk restates the
    // pilot ceiling at 250, below the signed 350, and now the amounts between
    // them are refused by the ceiling and by nothing else.
    desk.policy[0] = policy_subject(&corridor_source("simulated-venue"), CEILING, "250");
    assert_eq!(desk.append(Vec::new())?, 0);
    assert_eq!(
        desk.platform.corridor_funding(&signed)?.permitted(),
        dec!("250"),
        "the premise is that the restated ceiling is the one in force"
    );
    let from = desk.platform.fabric_state().assessments().len();
    let mut narrowed = Vec::new();
    for amount in &amounts {
        narrowed.push(gate_command(
            &desk.platform,
            &signed,
            corridor_source("simulated-venue"),
            treasury_account()?,
            *amount,
            TransferHistory::empty(),
        )?);
    }
    assert_eq!(desk.append(narrowed)?, amounts.len());
    let verdicts = desk.verdicts_since(from);
    let mut between = 0;
    for (amount, verdict) in amounts.iter().zip(&verdicts) {
        let expected = (*amount > dec!("250")).then_some(GateCheck::Caps);
        assert_eq!(*verdict, expected, "{amount} against a ceiling of 250");
        between += usize::from(*amount > dec!("250") && *amount <= per_transfer);
    }
    // Premise: some amounts sat between the two bounds, where only the
    // ceiling refuses.
    assert!(between > 2, "{between}");
    Ok(())
}

#[test]
fn a_corridor_policy_the_constructor_refuses_is_refused_by_position_without_repeating_the_route() {
    // A ceiling of zero deserialised into place would publish a corridor the
    // policy permits and the gate then refuses every transfer through, and
    // the operator would be sent to promote a strategy for a zero somebody
    // typed. The subject goes through its constructor, which refuses it.
    let acts = vec![destination("propose", "treasury-desk")];
    let source = corridor_source("simulated-venue");
    // Premise: the same subject with sound ceilings parses.
    let sound = declaration_with_policy(&[policy_subject(&source, "900", "400")], &acts);
    assert_eq!(
        Declaration::parse(&sound)
            .expect("a sound policy parses")
            .corridors_ruled(),
        1
    );

    for (ceiling, pilot) in [("0", "0"), ("400", "900")] {
        let refused = declaration_with_policy(&[policy_subject(&source, ceiling, pilot)], &acts);
        let error = Declaration::parse(&refused).expect_err("a refused subject parsed");
        assert!(
            error.message().contains("corridor_policy[0]"),
            "the refusal does not name the position: {}",
            error.message()
        );
        assert!(
            !error.message().contains(ADDRESS),
            "the refusal repeats the account the declaration names: {}",
            error.message()
        );
    }

    // An empty list is refused rather than read as "no policy": the platform
    // would go on holding the policy last declared while the file said none.
    let error = Declaration::parse(&declaration_with_policy(&[], &acts))
        .expect_err("an empty policy was taken as a statement");
    assert!(
        error.message().contains("corridor_policy is empty"),
        "{}",
        error.message()
    );

    // One route, two policies: refused naming both positions.
    let twice = declaration_with_policy(
        &[
            policy_subject(&source, "900", "400"),
            policy_subject(&source, "800", "300"),
        ],
        &acts,
    );
    let error = Declaration::parse(&twice).expect_err("one route was given two policies");
    assert!(
        error.message().contains("corridor_policy[1]")
            && error.message().contains("corridor_policy[0]"),
        "{}",
        error.message()
    );
    assert!(!error.message().contains(ADDRESS), "{}", error.message());
}

// --- the policy, restated while the process serves ----------------------------

use qip_api::auth::{Authenticator, Credential, RateLimiter, Role};
use qip_api::fabric::FabricRefresh;
use qip_api::http::{Handler, Method, Request};
use qip_api::routes::Api;
use std::collections::BTreeMap;
use std::sync::Mutex;

const ANALYST_TOKEN: &str = "analyst-token";

#[test]
fn a_restated_corridor_ceiling_reaches_the_platform_on_the_next_admitted_cycle_with_no_command_appended()
-> Result<()> {
    // The failure this prevents: the refresh middleware used to return as
    // soon as it saw no appended command, which was right while a declaration
    // was only acts. A ceiling is a statement, restated without an act, and a
    // desk that narrowed one would have gone on reading the old ruling — and
    // writing gate commands against it — until some unrelated command
    // happened to be appended.
    let config = PlatformConfig::default();
    let clock = Arc::new(ManualClock::new(start()));
    let context = Context::new(clock.clone(), config.seed);
    let mut platform = Platform::new(
        config,
        context,
        Telemetry::silent(),
        Universe::new(),
        LimitSet::conservative_default(),
    )?;
    promote_the_desk_to_pilot(&mut platform)?;

    let source = corridor_source("simulated-venue");
    let commands = corridor_acts()?;
    let stated = |pilot_ceiling: &str| {
        declaration_with_policy(
            &[
                policy_subject(&source, CEILING, pilot_ceiling),
                policy_subject(&corridor_source("other-venue"), CEILING, PILOT_CEILING),
            ],
            &commands,
        )
    };
    let path = write_fixture(&fixture_dir("restated"), &stated(PILOT_CEILING));
    // The root's own order: read, apply, then serve.
    let mut feed = FabricFeed::open(&path)?;
    feed.apply_pending(&mut platform, start())?;
    let applied = feed.applied();
    let platform = Arc::new(Mutex::new(platform));
    let authenticator = Arc::new(Authenticator::new(vec![Credential::from_token(
        "analyst@example.com",
        Role::Analyst,
        ANALYST_TOKEN.to_string(),
        start(),
        start().saturating_add(Duration::from_days(30)),
    )]));
    let handler = FabricRefresh::new(
        Api::new(
            platform.clone(),
            authenticator.clone(),
            Arc::new(RateLimiter::new(Duration::from_secs(60), 1000)),
            clock.clone(),
        ),
        Arc::new(Mutex::new(feed)),
        platform.clone(),
        authenticator,
        clock,
    );
    let permitted = |platform: &Arc<Mutex<Platform>>| -> Result<Decimal> {
        let platform = platform.lock().expect("the platform lock is not poisoned");
        Ok(platform.corridor_funding(&signed_corridor()?)?.permitted())
    };
    // Premise: the ruling is the ceiling first stated.
    assert_eq!(permitted(&platform)?, dec!("400"));
    let records = platform
        .lock()
        .expect("the platform lock is not poisoned")
        .fabric_records();

    // The desk narrows the pilot ceiling and appends nothing.
    std::fs::write(&path, stated("250")).expect("the fixture is rewritten");
    touch(
        &path,
        std::time::SystemTime::now() + std::time::Duration::from_secs(60),
    );
    let mut headers = BTreeMap::new();
    headers.insert(
        "authorization".to_string(),
        format!("Bearer {ANALYST_TOKEN}"),
    );
    let response = handler.handle(&Request {
        method: Method::Post,
        path: "/api/v1/cycle".to_string(),
        query: BTreeMap::new(),
        headers,
        body: Vec::new(),
        peer: "127.0.0.1:1".to_string(),
    });
    assert_ne!(
        response.status, 503,
        "the feed refused a declaration whose commands did not change"
    );

    assert_eq!(
        permitted(&platform)?,
        dec!("250"),
        "the restated ceiling did not reach the platform"
    );
    // And no act was journalled for it: the commands are as they were.
    let platform = platform.lock().expect("the platform lock is not poisoned");
    assert_eq!(platform.fabric_records(), records);
    assert_eq!(applied, commands.len());
    Ok(())
}

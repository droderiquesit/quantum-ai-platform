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

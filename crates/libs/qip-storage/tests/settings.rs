//! Storage configuration, and the refusals that keep a misconfigured
//! deployment from looking healthy.
//!
//! Every refusal here is tested against the case it is meant to be
//! distinguishable from: a test that only shows something was rejected cannot
//! tell a working guard from a function that rejects everything.

// The workspace denies `panic_in_result_fn` for production code, where an
// assertion that aborts a `Result`-returning function is a bug. In a test the
// assertion is the deliverable.
#![allow(clippy::panic_in_result_fn)]

use qip_storage::provider::StorageTarget;
use qip_storage::settings::{ROOT_VARIABLE, StorageSettings, TARGET_VARIABLE};
use std::sync::atomic::{AtomicU64, Ordering};

static DIRECTORY_COUNTER: AtomicU64 = AtomicU64::new(0);

fn temp_dir(label: &str) -> std::path::PathBuf {
    let unique = DIRECTORY_COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "qip-settings-{label}-{}-{unique}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("the test fixture directory is creatable");
    dir
}

// --- resolving the target ---------------------------------------------------

#[test]
fn an_unconfigured_process_resolves_to_memory_and_says_so() {
    let settings = StorageSettings::from_values(None, None).expect("an empty environment resolves");
    assert_eq!(settings.target(), StorageTarget::Memory);
    assert!(
        !settings.is_durable(),
        "memory must never report itself as durable"
    );
}

#[test]
fn a_target_name_is_read_the_way_an_operator_would_write_it() {
    // The premise: these differ from the canonical spelling only in case and
    // separator, so a parser that rejected them would be rejecting valid
    // configuration rather than catching a typo.
    for spelling in ["engine", "ENGINE", " Engine "] {
        let settings = StorageSettings::from_values(Some(spelling), Some("/tmp/qip"))
            .unwrap_or_else(|error| panic!("{spelling:?} was refused: {}", error.message()));
        assert_eq!(settings.target(), StorageTarget::Engine, "{spelling:?}");
    }
    for spelling in ["alloy_db", "alloy-db", "ALLOY-DB"] {
        let settings = StorageSettings::from_values(Some(spelling), None)
            .unwrap_or_else(|error| panic!("{spelling:?} was refused: {}", error.message()));
        assert_eq!(settings.target(), StorageTarget::AlloyDb, "{spelling:?}");
    }
}

#[test]
fn an_unrecognised_target_name_is_refused_and_the_error_names_every_valid_target() {
    // The premise: one character apart from a name that is accepted, so what
    // is being tested is the rejection and not the parser refusing everything.
    assert!(StorageSettings::from_values(Some("engine"), Some("/tmp/qip")).is_ok());

    let error = StorageSettings::from_values(Some("engin"), Some("/tmp/qip"))
        .expect_err("a misspelled target must not fall through to the default");
    for target in StorageTarget::ALL {
        assert!(
            error.message().contains(target.as_str()),
            "the error does not name {}: {}",
            target.as_str(),
            error.message()
        );
    }
}

// --- the root, and the silent-memory failure --------------------------------

#[test]
fn a_durable_target_without_a_root_is_refused_rather_than_given_a_default_directory() {
    // The premise: the same target with a root is admitted, so the refusal is
    // about the missing path and not about the target.
    assert!(StorageSettings::from_values(Some("engine"), Some("/tmp/qip")).is_ok());
    assert!(StorageSettings::from_values(Some("file"), Some("/tmp/qip")).is_ok());

    for target in ["engine", "file"] {
        let error = StorageSettings::from_values(Some(target), None)
            .expect_err("a durable target with no root must not start");
        assert!(
            error.message().contains(ROOT_VARIABLE),
            "the error does not name the variable to set: {}",
            error.message()
        );
    }
}

#[test]
fn a_root_configured_alongside_the_memory_target_is_refused_because_the_operator_expects_durability()
 {
    // The premise: this exact root is admitted for a durable target, so the
    // path is not what is being rejected — the combination is. An operator who
    // supplied a path believes the process persists, and a process that
    // started in memory anyway would pass every smoke test and lose everything
    // at the restart.
    let root = "/var/lib/qip";
    assert!(StorageSettings::from_values(Some("engine"), Some(root)).is_ok());

    let error = StorageSettings::from_values(Some("memory"), Some(root))
        .expect_err("a root with the memory target must not start silently");
    assert!(
        error.message().contains(TARGET_VARIABLE) && error.message().contains(ROOT_VARIABLE),
        "the error must name both variables so the operator knows which to change: {}",
        error.message()
    );
}

#[test]
fn an_unset_root_is_also_refused_when_the_target_variable_itself_is_absent() {
    // The specific deployment mistake: the root was templated in and the
    // target variable was forgotten. Defaulting to memory here is the silent
    // loss, so it is refused with the same message as an explicit `memory`.
    let error = StorageSettings::from_values(None, Some("/var/lib/qip"))
        .expect_err("a root with no target must not resolve to memory");
    assert!(
        error.message().contains(TARGET_VARIABLE),
        "{}",
        error.message()
    );
}

#[test]
fn an_empty_variable_is_treated_as_unset_rather_than_as_the_empty_path() {
    // A deployment template that expands a missing value to "" is common, and
    // reading that as "the operator asked for the empty path" would turn a
    // templating mistake into a directory named nothing.
    //
    // The premise: a non-empty root with the memory target *is* refused, so
    // these passing shows the empty string was treated as absent rather than
    // the check being absent.
    assert!(StorageSettings::from_values(Some("memory"), Some("/var/lib/qip")).is_err());

    for blank in ["", "   "] {
        let settings = StorageSettings::from_values(Some("memory"), Some(blank))
            .unwrap_or_else(|error| panic!("{blank:?} was refused: {}", error.message()));
        assert_eq!(settings.target(), StorageTarget::Memory);
        let settings = StorageSettings::from_values(Some(blank), None)
            .unwrap_or_else(|error| panic!("{blank:?} was refused: {}", error.message()));
        assert_eq!(settings.target(), StorageTarget::Memory);
    }
}

// --- preflight --------------------------------------------------------------

#[test]
fn a_configuration_that_can_be_written_passes_preflight() {
    // The premise every other preflight test rests on: preflight is capable of
    // succeeding, so a failure elsewhere is about that configuration.
    let root = temp_dir("preflight-ok");
    for target in ["file", "engine"] {
        let settings = StorageSettings::from_values(Some(target), root.to_str())
            .expect("the configuration resolves");
        settings
            .preflight()
            .unwrap_or_else(|error| panic!("{target} preflight failed: {}", error.message()));
    }
    StorageSettings::from_values(Some("memory"), None)
        .expect("memory resolves")
        .preflight()
        .expect("memory always preflights");
}

#[test]
fn a_managed_target_fails_preflight_naming_the_configuration_it_needs() {
    // The refusal that must never be softened into a fallback: a deployment
    // pointed at a managed service that quietly ran on local files would look
    // healthy and write to a disk nobody backs up.
    // With and without a root. A managed target is addressed by project and
    // instance rather than by path, so a root neither helps nor hurts — and an
    // operator who set one must still be told the credential is missing, not
    // sent to change the path. Reporting the wrong variable is how a
    // misconfiguration survives several deploys.
    let root = temp_dir("managed");
    for target in StorageTarget::ALL {
        if target.is_implemented() {
            continue;
        }
        let required = target
            .required_configuration()
            .expect("every managed target names what it needs");
        for supplied_root in [None, root.to_str()] {
            let settings = StorageSettings::from_values(Some(target.as_str()), supplied_root)
                .expect("a managed target resolves; it fails when it is used");
            let error = settings
                .preflight()
                .expect_err("a managed target must not preflight cleanly");
            assert!(
                error.message().contains(required),
                "{} with root {supplied_root:?} did not name what it needs: {}",
                target.as_str(),
                error.message()
            );
        }
    }
}

#[test]
fn preflight_fails_at_start_up_when_the_root_cannot_be_written_rather_than_at_the_first_record() {
    // A root that is a regular file rather than a directory constructs
    // cleanly enough to fool a start-up that only builds a store. The round
    // trip is what turns it into a start-up failure instead of a failure
    // during the first cycle, when the process is already being believed.
    let dir = temp_dir("preflight-blocked");
    let blocked = dir.join("not-a-directory");
    std::fs::write(&blocked, b"this is a file").expect("the fixture file is writable");

    // The premise: the enclosing directory works, so what fails below is the
    // unusable root and not the fixture.
    assert!(
        StorageSettings::from_values(Some("engine"), dir.to_str())
            .expect("resolves")
            .preflight()
            .is_ok()
    );

    for target in ["file", "engine"] {
        let settings = StorageSettings::from_values(Some(target), blocked.to_str())
            .expect("the configuration resolves; the filesystem is what refuses");
        assert!(
            settings.preflight().is_err(),
            "{target} preflighted against a root that is a regular file"
        );
    }
}

#[test]
fn preflight_leaves_nothing_of_its_own_behind() {
    // The probe is deleted on the way out, so an operator who finds one on
    // disk is looking at a process that died mid-preflight rather than at
    // ordinary debris.
    let root = temp_dir("preflight-clean");
    let settings = StorageSettings::from_values(Some("file"), root.to_str())
        .expect("the configuration resolves");
    settings.preflight().expect("preflight succeeds");

    let store = settings
        .key_value(qip_storage::settings::PREFLIGHT_NAMESPACE)
        .expect("the preflight namespace opens");
    assert!(
        store.is_empty().expect("the probe namespace is readable"),
        "preflight left its probe behind"
    );
}

// --- the banner -------------------------------------------------------------

#[test]
fn the_banner_states_where_state_goes_and_what_a_restart_takes_away() {
    let root = temp_dir("banner");
    let durable = StorageSettings::from_values(Some("engine"), root.to_str()).expect("resolves");
    let banner = durable.banner_lines(&["the event log's hash chain"], &["the order book"]);
    let text = banner.join("\n");
    assert!(text.contains("engine"), "{text}");
    assert!(text.contains(&root.display().to_string()), "{text}");
    assert!(text.contains("the event log's hash chain"), "{text}");
    assert!(text.contains("the order book"), "{text}");

    // The premise this is tested against: the same call on a memory
    // configuration must not claim anything persists, whatever the binary
    // passed in. A banner that reported a durable intent while running in
    // memory is the exact misreading the start-up refusals exist to prevent.
    let ephemeral = StorageSettings::from_values(None, None).expect("resolves");
    let text = ephemeral
        .banner_lines(&["the event log's hash chain"], &["the order book"])
        .join("\n");
    assert!(text.contains("NOTHING SURVIVES A RESTART"), "{text}");
    assert!(
        !text.contains("the event log's hash chain"),
        "a memory configuration must not claim the chain persists: {text}"
    );
}

//! CICD-096's start-up check, exercised through the library function
//! `main.rs` calls. These live outside `src/` because `manifest_wiring` reads
//! the binary's sources for the variables it reads, and a fixture variable
//! named in a unit test there is indistinguishable from a real read.
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report
use qip_api::config_signature::{canonical, check};

fn vars(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
        .collect()
}

const KEY: &str = "a-provisioned-test-key";

fn deployment() -> Vec<(String, String)> {
    vars(&[
        ("QIP_AUTONOMY__CEILING", "paper_trading"),
        ("QIP_STORAGE__TARGET", "memorystore"),
        ("PATH", "/usr/bin"),
    ])
}

#[test]
fn an_unset_signature_admits_without_reading_a_key() {
    assert!(check(deployment(), None, None).is_ok());
    assert!(check(deployment(), Some(""), None).is_ok());
}

#[test]
fn a_signature_with_no_key_is_refused_rather_than_checked_under_a_default() {
    let signature = canonical(deployment()).sign_with_key(KEY.as_bytes());
    let refused = check(deployment(), Some(&signature), None);
    let message = refused
        .err()
        .map(|e| e.message().to_string())
        .unwrap_or_default();
    assert!(
        message.contains("QIP_CONFIG_SIGNATURE_KEY is not"),
        "a signature with no key must be refused, naming the key: {message:?}"
    );
}

#[test]
fn a_signature_over_the_deployment_verifies_beside_itself_and_its_key_file() {
    let signature = canonical(deployment()).sign_with_key(KEY.as_bytes());
    // The environment the process sees holds the signature and the key's
    // file variable as well as the configuration it signed.
    let mut environment = deployment();
    environment.push(("QIP_CONFIG_SIGNATURE".into(), signature.clone()));
    environment.push((
        "QIP_CONFIG_SIGNATURE_KEY_FILE".into(),
        "/var/run/secrets/qip/config-signature-key".into(),
    ));
    assert!(
        check(environment, Some(&signature), Some(KEY)).is_ok(),
        "a signature over the deployment's own variables must verify once set beside them"
    );
}

#[test]
fn a_changed_variable_or_another_key_is_refused() {
    let signature = canonical(deployment()).sign_with_key(KEY.as_bytes());
    let mut changed = deployment();
    changed[0].1 = "observation".into();
    assert!(check(changed, Some(&signature), Some(KEY)).is_err());
    assert!(check(deployment(), Some(&signature), Some("another-key")).is_err());
    assert!(
        check(deployment(), Some(&signature), Some(KEY)).is_ok(),
        "premise: the unchanged deployment verifies"
    );
}

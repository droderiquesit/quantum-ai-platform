//! Configuration-level proofs for blueprint SEC controls that are Terraform
//! text rather than Rust behaviour.
//!
//! These read the committed configuration. They prove the control is declared
//! and cannot be silently dropped; they do not prove a deployed project has it
//! (nothing is applied), and the register says so.
#![allow(clippy::panic_in_result_fn)]

use qip_acceptance::read;

/// Configuration with comments removed, so a comment quoting a setting is not
/// mistaken for the setting.
fn code(path: &str) -> String {
    read(path)
        .lines()
        .map(|line| line.split('#').next().unwrap_or("").trim_end())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Whitespace-free form, so the checks do not depend on alignment.
fn compact(text: &str) -> String {
    text.split_whitespace().collect()
}

#[test]
fn every_internet_facing_cloud_armor_policy_enables_layer_7_adaptive_protection() {
    let modules = [
        "infrastructure/terraform/modules/public-edge/main.tf",
        "infrastructure/terraform/modules/iap-edge/main.tf",
    ];
    for path in modules {
        let text = compact(&code(path));
        assert!(
            text.contains("resource\"google_compute_security_policy\""),
            "{path} declares no Cloud Armor policy, so there is nothing for adaptive \
             protection to be on"
        );
        assert!(
            text.contains("adaptive_protection_config{layer_7_ddos_defense_config{enable=true}}"),
            "{path} does not enable layer-7 adaptive protection; a flood would be found on \
             the bill rather than blocked"
        );
    }
}

#[test]
fn secret_manager_and_kms_data_reads_are_audit_logged() {
    let text = compact(&code("infrastructure/terraform/modules/secrets/main.tf"));
    assert!(
        text.contains("resource\"google_project_iam_audit_config\""),
        "no audit config exists, so a secret's value can be read with no record"
    );
    for service in ["secretmanager.googleapis.com", "cloudkms.googleapis.com"] {
        assert!(
            text.contains(&format!("\"{service}\"")),
            "{service} is not audit-configured"
        );
    }
    for log_type in ["\"DATA_READ\"", "\"DATA_WRITE\"", "\"ADMIN_READ\""] {
        assert!(
            text.contains(log_type),
            "audit log type {log_type} is not enabled; DATA_READ is the only log that \
             records who read a secret's value"
        );
    }
}

#[test]
fn the_container_scanning_api_is_enabled_for_the_registry() {
    let services = code("infrastructure/terraform/modules/services/main.tf");
    assert!(
        services.contains("\"containeranalysis.googleapis.com\""),
        "premise: the always-on API list is the one this test reads"
    );
    assert!(
        services.contains("\"containerscanning.googleapis.com\" ="),
        "Artifact Analysis scanning is not enabled, so a vulnerability disclosed after a \
         push is never recorded against the digest"
    );
}

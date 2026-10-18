//! What each workload's machine identity may hold, and which compute may host
//! the execution plane (R12-SEC-10050, R12-GCP-8114).
//!
//! String checks on the HCL, like `infrastructure.rs`: they cannot evaluate
//! Terraform, but they fail when a role is added to a workload's module or
//! the execution plane stops being refused by the Cloud Run module, which is
//! the change that actually happens. The role lists are committed here, so
//! widening an identity is a diff to this file a reviewer reads, not only a
//! diff to a `main.tf`.
//!
//! Failure messages name files and roles, never an account.

use qip_acceptance::read;
use std::collections::BTreeSet;

/// Every `"roles/..."` literal outside a comment.
fn roles_in(text: &str) -> BTreeSet<String> {
    let mut roles = BTreeSet::new();
    for line in text.lines() {
        let code = line.split('#').next().unwrap_or("");
        let mut rest = code;
        while let Some(start) = rest.find("\"roles/") {
            let after = &rest[start + 1..];
            let end = after.find('"').unwrap_or(after.len());
            roles.insert(after[..end].to_string());
            rest = &after[end..];
        }
    }
    roles
}

fn set(roles: &[&str]) -> BTreeSet<String> {
    roles.iter().map(|r| (*r).to_string()).collect()
}

/// Each file that declares a workload identity and the roles it may bind,
/// with the reason the list is that long.
fn allow_lists() -> Vec<(&'static str, BTreeSet<String>)> {
    vec![
        (
            // Telemetry, the secrets and buckets the workload mounts, and
            // the reconciler's right to act as the account (granted on it,
            // not held by it).
            "infrastructure/terraform/modules/cloudrun/main.tf",
            set(&[
                "roles/iam.serviceAccountUser",
                "roles/logging.logWriter",
                "roles/monitoring.metricWriter",
                "roles/secretmanager.secretAccessor",
                "roles/storage.objectViewer",
            ]),
        ),
        (
            // Create-only evidence writes: a writer that can delete makes an
            // append-only store a store nobody has deleted from yet.
            "infrastructure/terraform/modules/execution-node/main.tf",
            set(&[
                "roles/logging.logWriter",
                "roles/monitoring.metricWriter",
                "roles/secretmanager.secretAccessor",
                "roles/storage.objectCreator",
            ]),
        ),
        (
            "infrastructure/terraform/modules/ledgerd/main.tf",
            set(&["roles/logging.logWriter", "roles/monitoring.metricWriter"]),
        ),
        (
            "infrastructure/terraform/fabric.tf",
            set(&["roles/logging.logWriter", "roles/monitoring.metricWriter"]),
        ),
    ]
}

/// The roles in `text` that `allowed` does not name.
fn extra_roles(text: &str, allowed: &BTreeSet<String>) -> Vec<String> {
    roles_in(text).difference(allowed).cloned().collect()
}

#[test]
fn every_workload_identity_holds_only_the_roles_its_allow_list_names() {
    for (file, allowed) in allow_lists() {
        let text = read(file);
        let held = roles_in(&text);
        assert!(!held.is_empty(), "premise: {file} binds at least one role");
        let extra = extra_roles(&text, &allowed);
        assert!(
            extra.is_empty(),
            "{file} binds roles its allow list does not name: {extra:?}"
        );
        let missing: Vec<_> = allowed.difference(&held).collect();
        assert!(
            missing.is_empty(),
            "{file} no longer binds {missing:?}; narrow the allow list in this file with it"
        );
    }
}

#[test]
fn a_role_added_to_a_workload_module_is_found_by_the_scan() {
    // The scan run on a mutated copy of the input: the committed file is
    // never touched. Both the broadest role an identity could be handed and
    // a narrow one a reviewer might wave through.
    for added in ["roles/owner", "roles/storage.objectAdmin"] {
        for (file, allowed) in allow_lists() {
            let mutated = format!(
                "{}\nresource \"google_project_iam_member\" \"extra\" {{\n  role = \"{added}\"\n}}\n",
                read(file)
            );
            assert_eq!(
                extra_roles(&mutated, &allowed),
                vec![added.to_string()],
                "{file}: the added role was not the one reported"
            );
        }
    }
}

#[test]
fn a_role_named_only_in_a_comment_is_not_a_grant() {
    let text = "# never roles/owner here\nrole = \"roles/logging.logWriter\"\n";
    assert_eq!(roles_in(text), set(&["roles/logging.logWriter"]));
}

#[test]
fn the_cloud_run_module_refuses_an_execution_plane_and_no_catalogue_workload_hosts_the_edge_node() {
    let variables = read("infrastructure/terraform/modules/cloudrun/variables.tf");
    let start = variables
        .find("variable \"plane\"")
        .expect("the module declares a plane variable");
    let block = &variables[start..];
    let block = &block[..block.find("\nvariable ").unwrap_or(block.len())];
    let allowed = block
        .split("contains([")
        .nth(1)
        .and_then(|rest| rest.split("], var.plane)").next())
        .expect("the plane validation lists the planes it admits");
    assert!(
        allowed.contains("\"cognition\""),
        "premise: the admitted list is the one parsed"
    );
    assert!(
        !allowed.contains("\"execution\""),
        "the Cloud Run module admits an execution plane"
    );
    assert!(
        block.contains("`execution` is not one of them"),
        "the refusal no longer says why the execution plane is refused"
    );

    // The plan-time proof this scan cannot give: a tftest that plans the
    // refusal. Its presence is pinned here; `terraform test` runs it.
    let tftest = read("infrastructure/terraform/modules/cloudrun/tests/plane.tftest.hcl");
    assert!(
        tftest.contains("plane = \"execution\"")
            && tftest.contains("expect_failures = [var.plane]"),
        "the plane tftest no longer plans the execution refusal"
    );

    let catalogue = read("infrastructure/terraform/catalogue.tf");
    let code: String = catalogue
        .lines()
        .map(|l| l.split('#').next().unwrap_or(""))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        code.contains("binary        = \"qip-api\""),
        "premise: the catalogue parsed"
    );
    assert!(
        !code.contains("qip-edge-node"),
        "a Cloud Run catalogue entry names the edge node"
    );
    assert!(
        !code.contains("plane         = \"execution\""),
        "a Cloud Run catalogue entry declares the execution plane"
    );
}

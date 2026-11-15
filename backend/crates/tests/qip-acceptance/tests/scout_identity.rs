//! The scout identity holds only its listed grants (R12-DATA-8112, ADR 0144).
//!
//! A compromised research crawler must not reach trading. ADR 0144 decision 2
//! says the scout's service account may create objects in its own bucket and
//! write logs, and nothing else, and that the deep brain may only read that
//! bucket. A comment saying so protects nothing; this reads the module and
//! refuses a grant that is not on the list, wherever in the repository it is
//! written.
//!
//! The module sits under `docs/blueprint/lanes/b52-14-scout-terraform/` until
//! a person adopts it (see the README there): placed under `infrastructure/` it
//! trips two other guards that need a decision, not an edit. `module_path`
//! moves with it.
//!
//! There is no Terraform binary in this environment, so this scans the text,
//! as `terraform_contract.rs` does. The two fixture tests at the end drive the
//! same scanner over a module carrying the defect and assert it is found: a
//! scan proven only by the tree it scans passes for ever once its parser stops
//! parsing.

#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use qip_acceptance::repository_root;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// The only roles the scout's own account may hold.
const SCOUT_ROLES: [&str; 2] = ["roles/storage.objectCreator", "roles/logging.logWriter"];

/// One IAM grant found in Terraform text: the resource, its role and member.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Grant {
    resource: String,
    role: String,
    member: String,
}

fn value_of(block: &str, key: &str) -> Option<String> {
    block.lines().find_map(|line| {
        let line = line.trim();
        let rest = line.strip_prefix(key)?.trim_start().strip_prefix('=')?;
        Some(rest.trim().trim_matches('"').to_string())
    })
}

/// Every `google_*_iam_*` resource block's role and member.
fn grants(text: &str) -> Vec<Grant> {
    let mut found = Vec::new();
    let mut rest = text;
    while let Some(at) = rest.find("resource \"google_") {
        rest = &rest[at..];
        let header_end = rest.find('{').unwrap_or(rest.len());
        let header = &rest[..header_end];
        let next = rest[1..].find("\nresource ").map_or(rest.len(), |n| n + 1);
        let block = &rest[..next];
        if header.contains("_iam_") {
            found.push(Grant {
                resource: header
                    .trim_start_matches("resource")
                    .trim()
                    .replace('"', "")
                    .replace(' ', "."),
                role: value_of(block, "role").unwrap_or_default(),
                member: value_of(block, "member").unwrap_or_default(),
            });
        }
        rest = &rest[next..];
    }
    found
}

fn problems(module_text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let all = grants(module_text);
    let scout: Vec<&Grant> = all
        .iter()
        .filter(|g| g.member.contains("google_service_account.scout.email"))
        .collect();
    for grant in &scout {
        if !SCOUT_ROLES.contains(&grant.role.as_str()) {
            out.push(format!(
                "the scout identity is granted {} by {}; ADR 0144 allows only {SCOUT_ROLES:?}",
                grant.role, grant.resource
            ));
        }
    }
    let held: BTreeSet<&str> = scout.iter().map(|g| g.role.as_str()).collect();
    for role in SCOUT_ROLES {
        if !held.contains(role) {
            out.push(format!("the scout identity no longer holds {role}"));
        }
    }
    for grant in &all {
        let public =
            grant.member.contains("allUsers") || grant.member.contains("allAuthenticatedUsers");
        if public {
            out.push(format!("{} grants to the public", grant.resource));
        }
        if grant.member.contains("var.deepbrain_service_account_email")
            && grant.role != "roles/storage.objectViewer"
        {
            out.push(format!(
                "the deep brain is granted {} on the scout bucket; it may only read",
                grant.role
            ));
        }
        for broad in ["owner", "editor", "admin", "Admin"] {
            if grant.role.contains(broad) {
                out.push(format!(
                    "{} grants the broad role {}",
                    grant.resource, grant.role
                ));
            }
        }
    }
    out
}

fn module_path() -> PathBuf {
    repository_root().join("docs/blueprint/lanes/b52-14-scout-terraform/scout/main.tf")
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk(&path, out);
        } else if path.extension().is_some_and(|e| e == "tf") {
            out.push(path);
        }
    }
}

#[test]
fn the_scout_identity_holds_only_its_listed_grants_and_the_deep_brain_only_reads() {
    let text = std::fs::read_to_string(module_path()).expect("the scout module exists");
    let found = grants(&text);
    assert!(
        found.len() >= 3,
        "premise: the scanner found the module's grants: {found:?}"
    );
    let problems = problems(&text);
    assert!(problems.is_empty(), "{problems:#?}");
}

/// Nothing outside the module may grant the scout identity a role: it is not
/// nameable from a caller, so a grant written elsewhere would be a grant the
/// list above never saw.
#[test]
fn no_terraform_outside_the_module_grants_a_role_to_the_scout_identity() {
    let mut files = Vec::new();
    walk(&repository_root().join("infrastructure"), &mut files);
    assert!(!files.is_empty(), "premise: terraform files were found");
    let module_dir = repository_root().join("docs/blueprint/lanes/b52-14-scout-terraform/scout");
    for file in files.iter().filter(|f| !f.starts_with(&module_dir)) {
        let text = std::fs::read_to_string(file).expect("readable");
        let outside = grants(&text)
            .into_iter()
            .filter(|g| g.member.contains("module.scout.service_account_email"))
            .collect::<Vec<_>>();
        assert!(
            outside.is_empty(),
            "{} grants to the scout identity: {outside:?}",
            file.display()
        );
    }
}

/// Fixture: a module whose scout account also holds a trading-adjacent role is
/// refused, and one that lost a listed role is too. Mutation proof for the
/// scanner: drop the `SCOUT_ROLES.contains` check and the first assertion
/// fails.
#[test]
fn a_scout_grant_that_is_not_on_the_list_is_found() {
    let real = std::fs::read_to_string(module_path()).expect("the scout module exists");
    let widened = format!(
        "{real}\nresource \"google_project_iam_member\" \"oops\" {{\n  project = var.project_id\n  \
         role    = \"roles/pubsub.publisher\"\n  member  = \"serviceAccount:${{google_service_account.scout.email}}\"\n}}\n"
    );
    let found = problems(&widened);
    assert!(
        found.iter().any(|p| p.contains("roles/pubsub.publisher")),
        "{found:?}"
    );
    let narrowed = real.replace("roles/logging.logWriter", "roles/logging.viewer");
    assert!(
        problems(&narrowed)
            .iter()
            .any(|p| p.contains("roles/logging.viewer") || p.contains("no longer holds")),
        "a changed role went unnoticed"
    );
    let deepbrain_writes = real.replace("roles/storage.objectViewer", "roles/storage.objectAdmin");
    assert!(
        problems(&deepbrain_writes)
            .iter()
            .any(|p| p.contains("it may only read")),
        "the deep brain gaining write went unnoticed"
    );
}

//! No warm service can starve another by scaling without bound
//! (R12-GCP-8116, as built for Cloud Run under ADR 0114 Amendment 1).
//!
//! The blueprint's node pools, taints and PriorityClasses keep research from
//! starving control on a shared cluster. The platform's services run on Cloud
//! Run instead, where the equivalent control is per service: a ceiling on
//! instances and on the requests one instance serves at once. A service
//! manifest with neither scales until the project's quota is spent, and the
//! next service to need an instance is the one starved. So every Cloud Run
//! service manifest in every environment must declare both, and both must be
//! positive.

use qip_acceptance::files_with_extension;

/// The integer after `key:` on a line of `document`, if any.
fn integer(document: &str, key: &str) -> Option<i64> {
    document.lines().find_map(|line| {
        let rest = line.trim().strip_prefix(key)?.strip_prefix(':')?;
        rest.trim().parse().ok()
    })
}

/// Every `kind: RunService` document under the GitOps environments, with
/// where it came from.
fn services() -> Vec<(String, String)> {
    let mut found = Vec::new();
    for file in files_with_extension("infrastructure/gitops/envs", "yaml") {
        let text = std::fs::read_to_string(&file).unwrap_or_default();
        for document in text.split("\n---") {
            // The document's own kind, at column zero: an IAM binding names
            // the service it grants on as an indented `kind: RunService`.
            if document.lines().any(|l| l == "kind: RunService") {
                found.push((file.display().to_string(), document.to_string()));
            }
        }
    }
    found
}

/// The refusals for one service document.
fn refusals(document: &str) -> Vec<&'static str> {
    let mut refused = Vec::new();
    if !integer(document, "maxInstanceCount").is_some_and(|n| n >= 1) {
        refused.push("no positive maxInstanceCount");
    }
    if !integer(document, "maxInstanceRequestConcurrency").is_some_and(|n| n >= 1) {
        refused.push("no positive maxInstanceRequestConcurrency");
    }
    refused
}

#[test]
fn every_cloud_run_service_in_every_environment_declares_an_instance_and_a_concurrency_ceiling() {
    let services = services();
    // Premise: the scan finds the warm services in more than one environment.
    assert!(
        services.len() >= 8,
        "premise: the scan found only {} RunService manifests",
        services.len()
    );
    let unbounded: Vec<String> = services
        .iter()
        .filter_map(|(file, document)| {
            let refused = refusals(document);
            (!refused.is_empty()).then(|| format!("{file}: {}", refused.join(", ")))
        })
        .collect();
    assert!(
        unbounded.is_empty(),
        "Cloud Run services that can scale without bound: {unbounded:#?}"
    );
}

#[test]
fn a_service_with_no_ceiling_or_a_zero_one_is_refused() {
    let bounded = "kind: RunService\n    scaling:\n      maxInstanceCount: 4\n    maxInstanceRequestConcurrency: 80\n";
    assert!(refusals(bounded).is_empty());
    assert_eq!(
        refusals("kind: RunService\n    maxInstanceRequestConcurrency: 80\n"),
        vec!["no positive maxInstanceCount"]
    );
    assert_eq!(
        refusals(
            "kind: RunService\n      maxInstanceCount: 0\n    maxInstanceRequestConcurrency: 80\n"
        ),
        vec!["no positive maxInstanceCount"]
    );
}

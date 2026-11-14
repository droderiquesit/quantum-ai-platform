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
    if integer(document, "maxInstanceCount").is_none_or(|n| n < 1) {
        refused.push("no positive maxInstanceCount");
    }
    if integer(document, "maxInstanceRequestConcurrency").is_none_or(|n| n < 1) {
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

/// Whether a service document runs a store in local mode on its own disk:
/// the `ZO_LOCAL_MODE_STORAGE` variable, on the line after its name, is
/// `disk`. The whole line is matched, not a substring of it.
fn keeps_data_on_local_disk(document: &str) -> bool {
    let mut lines = document.lines();
    while let Some(line) = lines.next() {
        if line.trim() == "- name: ZO_LOCAL_MODE_STORAGE" {
            return lines
                .next()
                .is_some_and(|value| value.trim() == "value: disk");
        }
    }
    false
}

// A store in local mode on disk keeps its data on the instance's own disk, so
// a second instance is a second store: a write lands in one and a query is
// answered by whichever the request reached. This has happened once: dev's
// OpenObserve declared a ceiling of four until 2026-11-14 while in exactly
// that mode.
#[test]
fn a_store_that_keeps_its_data_on_its_own_disk_is_never_more_than_one_instance() {
    let local: Vec<(String, String)> = services()
        .into_iter()
        .filter(|(_, document)| keeps_data_on_local_disk(document))
        .collect();
    // Premise: the scan finds such a store, or the rule below checks nothing.
    assert!(
        !local.is_empty(),
        "premise: no RunService sets ZO_LOCAL_MODE_STORAGE to disk"
    );
    for (file, document) in &local {
        assert_eq!(
            integer(document, "maxInstanceCount"),
            Some(1),
            "{file}: a store on its own disk may scale past one instance, and each instance \
             would be a separate store"
        );
    }
}

//! R12-REASON-8111: the deep brain's custom Rust solvers run on the deep
//! brain's Cloud Run capacity (ADR 0114 Amendment 1), so that capacity has to
//! be declared as compute-optimised and warm rather than inherited from the
//! request-serving default.
//!
//! Read from the GitOps manifest every environment's reconciler applies and
//! from the catalogue that renders it, since the Cloud Run module no longer
//! takes a CPU input. "Warm" is `cpuIdle: false` (CPU allocated outside
//! requests, which a solver running between cycles needs) beside a floor of
//! one instance; "compute-optimised" is at least four vCPU, the figure the
//! catalogue already gives it and the fast brain's two times over.

#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_acceptance::{read, repository_root};
use std::fs;

/// The first `key: value` under a manifest's `containers:` resources, as text.
fn manifest_value(manifest: &str, key: &str) -> Option<String> {
    manifest.lines().find_map(|line| {
        let (k, v) = line.trim().split_once(':')?;
        (k == key).then(|| v.trim().trim_matches('\'').to_string())
    })
}

fn vcpus(manifest: &str) -> f64 {
    manifest_value(manifest, "cpu")
        .and_then(|v| v.parse::<f64>().ok())
        .expect("the manifest declares a numeric cpu limit")
}

fn manifests() -> Vec<(String, String)> {
    let envs = repository_root().join("infrastructure/gitops/envs");
    let mut found: Vec<(String, String)> = fs::read_dir(&envs)
        .expect("the gitops envs directory exists")
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let path = entry.path().join("deepbrain.yaml");
            let text = fs::read_to_string(&path).ok()?;
            Some((entry.file_name().to_string_lossy().into_owned(), text))
        })
        .collect();
    found.sort();
    found
}

#[test]
fn the_deep_brain_declares_warm_compute_optimised_capacity_in_every_environment_that_runs_it() {
    let manifests = manifests();
    assert!(
        manifests.iter().any(|(env, _)| env == "dev"),
        "premise: dev runs the deep brain"
    );
    for (env, text) in &manifests {
        assert!(
            vcpus(text) >= 4.0,
            "{env}: the deep brain declares {} vCPU; solver work needs at least four",
            vcpus(text)
        );
        assert_eq!(
            manifest_value(text, "cpuIdle").as_deref(),
            Some("false"),
            "{env}: the deep brain's CPU is throttled outside requests, so a solver between cycles stalls"
        );
        let floor: u32 = manifest_value(text, "minInstanceCount")
            .and_then(|v| v.parse().ok())
            .expect("the manifest declares a scaling floor");
        assert!(
            floor >= 1,
            "{env}: the deep brain has a cold floor of {floor}"
        );
    }
}

#[test]
fn the_deep_brain_is_given_more_cpu_than_the_fast_brain_and_the_catalogue_agrees_with_the_manifest()
{
    let manifests = manifests();
    let (_, dev) = manifests
        .iter()
        .find(|(env, _)| env == "dev")
        .expect("dev runs the deep brain");
    let fast = read("infrastructure/gitops/envs/dev/fastbrain.yaml");
    assert!(
        vcpus(dev) > vcpus(&fast),
        "the deep brain ({}) is not given more CPU than the fast brain ({})",
        vcpus(dev),
        vcpus(&fast)
    );

    let catalogue = read("infrastructure/terraform/catalogue.tf");
    let entry = catalogue
        .split("    deepbrain = {")
        .nth(1)
        .expect("the catalogue has a deepbrain entry");
    let entry = &entry[..entry.find("\n    }").unwrap_or(entry.len())];
    let declared = entry
        .lines()
        .find_map(|line| {
            let (k, v) = line.split_once('=')?;
            (k.trim() == "cpu").then(|| v.trim().trim_matches('"').to_string())
        })
        .expect("the catalogue entry declares cpu");
    assert_eq!(
        declared.parse::<f64>().ok(),
        Some(vcpus(dev)),
        "the catalogue and the dev manifest disagree on the deep brain's CPU"
    );
}

#[test]
fn a_cold_or_small_deep_brain_manifest_is_found_by_the_scan() {
    // The scan run on mutated copies; the committed manifests are untouched.
    let (_, dev) = manifests().into_iter().next().expect("a manifest");
    assert!(
        vcpus(&dev) >= 4.0,
        "premise: unmutated, the deep brain is large"
    );
    let small = dev.replace("cpu: '4'", "cpu: '1'");
    assert!(vcpus(&small) < 4.0);
    let throttled = dev.replace("cpuIdle: false", "cpuIdle: true");
    assert_eq!(
        manifest_value(&throttled, "cpuIdle").as_deref(),
        Some("true")
    );
    let cold = dev.replace("minInstanceCount: 1", "minInstanceCount: 0");
    assert_eq!(
        manifest_value(&cold, "minInstanceCount").as_deref(),
        Some("0")
    );
}

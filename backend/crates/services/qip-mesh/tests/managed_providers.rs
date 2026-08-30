//! The managed-service adapters, which this build cannot supply.
//!
//! The property under test is the one `qip-storage`'s documentation calls out
//! as a hazard: a deployment pointed at a managed service must fail loudly
//! rather than quietly serving local files. A mesh that fell back would pass
//! its smoke tests, serve stale answers, and lose every write on restart.

#![allow(clippy::panic_in_result_fn)]

use qip_core::Duration;
use qip_core::error::{Error, Result};
use qip_mesh::{MeshPort, MeshProvider, MeshTarget};

const MANAGED: [MeshTarget; 6] = [
    MeshTarget::BigLakeIceberg,
    MeshTarget::BigQuery,
    MeshTarget::Bigtable,
    MeshTarget::Spanner,
    MeshTarget::SpannerGraph,
    MeshTarget::CloudStorageWorm,
];

/// Every way to obtain a store from a provider, so no path is left untested.
fn every_port(provider: &MeshProvider) -> Vec<(MeshPort, Result<()>)> {
    vec![
        (MeshPort::Lakehouse, provider.lakehouse().map(|_| ())),
        (MeshPort::Analytical, provider.analytics().map(|_| ())),
        (MeshPort::HotSeries, provider.hot_series().map(|_| ())),
        (MeshPort::MasterData, provider.master_data().map(|_| ())),
        (MeshPort::Graph, provider.graph().map(|_| ())),
        (MeshPort::Evidence, provider.evidence().map(|_| ())),
    ]
}

#[test]
fn an_unavailable_managed_adapter_errors_at_first_use_on_every_port() -> Result<()> {
    for target in MANAGED {
        assert!(
            !target.is_implemented(),
            "{target:?} claims to be implemented"
        );
        let provider = MeshProvider::new(target, "/nonexistent", Duration::from_hours(1));

        for (port, outcome) in every_port(&provider) {
            let error = outcome.expect_err(&format!(
                "{target:?} must not produce a working {} store",
                port.as_str()
            ));
            // `Unavailable` specifically: a caller distinguishing "not
            // configured here" from "broken" needs the code, not the prose.
            assert!(
                matches!(error, Error::Unavailable(_)),
                "{target:?}/{} returned {error:?}",
                port.as_str()
            );
        }
    }
    Ok(())
}

#[test]
fn the_error_names_the_exact_credential_and_project_that_are_missing() {
    for target in MANAGED {
        let provider = MeshProvider::new(target, "/nonexistent", Duration::from_hours(1));
        let Err(error) = provider.lakehouse() else {
            panic!("{target:?} must not produce a working lakehouse");
        };
        let message = error.message();
        let Some(required) = target.required_configuration() else {
            panic!("{target:?} must state what it requires");
        };
        // The message quotes the requirement verbatim, so an operator can act
        // on it without going and finding the enum.
        assert!(
            message.contains(required),
            "{target:?} does not name what it needs: {message}"
        );
        assert!(message.contains("GCP project"));
    }
}

#[test]
fn a_managed_provider_never_silently_falls_back_to_local_files() -> Result<()> {
    // The hazard in one test: point a provider at BigQuery with a perfectly
    // usable local directory underneath it, and it must still refuse.
    let root = std::env::temp_dir().join(format!("qip-mesh-fallback-{}", std::process::id()));
    std::fs::create_dir_all(&root)?;

    let provider = MeshProvider::new(MeshTarget::BigQuery, &root, Duration::from_hours(1));
    assert!(provider.analytics().is_err());
    assert!(provider.lakehouse().is_err());

    // Nothing was written, and no file was created on the way to failing.
    let entries = std::fs::read_dir(&root)?.count();
    assert_eq!(entries, 0);

    std::fs::remove_dir_all(&root)?;
    Ok(())
}

#[test]
fn the_local_targets_produce_working_stores() -> Result<()> {
    // The complement: the two implemented targets do work, so the refusals
    // above are about the managed services rather than about the provider.
    let memory = MeshProvider::in_memory(Duration::from_hours(1));
    assert!(MeshTarget::Memory.is_implemented());
    for (port, outcome) in every_port(&memory) {
        assert!(outcome.is_ok(), "memory/{} failed", port.as_str());
    }

    let root = std::env::temp_dir().join(format!("qip-mesh-local-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root)?;
    let filed = MeshProvider::new(MeshTarget::File, &root, Duration::from_hours(1));
    for (port, outcome) in every_port(&filed) {
        assert!(outcome.is_ok(), "file/{} failed", port.as_str());
    }
    std::fs::remove_dir_all(&root)?;
    Ok(())
}

#[test]
fn every_port_names_the_managed_service_the_architecture_intends() {
    // The mapping is data rather than prose, so a deployment guide and the
    // code cannot drift apart about which service serves which pattern.
    assert_eq!(MeshPort::all().len(), 6);
    for port in MeshPort::all() {
        let target = port.managed_target();
        assert!(!target.is_implemented());
        assert!(target.required_configuration().is_some());
        assert!(!target.rationale().is_empty());
    }
    assert_eq!(MeshPort::Analytical.managed_target(), MeshTarget::BigQuery);
    assert_eq!(
        MeshPort::Evidence.managed_target(),
        MeshTarget::CloudStorageWorm
    );
}

#[test]
fn a_providers_description_says_what_each_port_still_needs() {
    // For a start-up log line: an operator sees the whole mapping at once
    // rather than discovering it one failed call at a time.
    let unavailable =
        MeshProvider::new(MeshTarget::Spanner, "/nonexistent", Duration::from_hours(1));
    let described = unavailable.describe();
    assert_eq!(described.len(), 6);
    assert!(described.iter().all(|(_, _, missing)| missing.is_some()));

    let local = MeshProvider::in_memory(Duration::from_hours(1));
    assert!(
        local
            .describe()
            .iter()
            .all(|(_, _, missing)| missing.is_none())
    );
}

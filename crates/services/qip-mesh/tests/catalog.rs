//! The catalogue: what exists, what produced what, what it may be used for,
//! and whether it is currently trustworthy.
//!
//! The tests check the three questions whose answers a caller has to act on:
//! whether a lineage resolves or names the break, whether a quarantined
//! dataset can still be read, and what else is affected when one is found to
//! be wrong.

#![allow(clippy::panic_in_result_fn)]

use qip_contracts::governance::{Entitlement, Usage};
use qip_core::error::Result;
use qip_core::{Duration, Timestamp};
use qip_mesh::catalog::{Catalog, DatasetRegistration, QualityState};
use qip_mesh::MeshPort;

fn now() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

fn granted(dataset: &str, usage: Usage) -> Entitlement {
    Entitlement::Granted {
        dataset: dataset.to_string(),
        usage,
        expires_at: now().saturating_add(Duration::from_days(30)),
    }
}

fn dataset(name: &str, parents: &[&str]) -> Result<DatasetRegistration> {
    Ok(
        DatasetRegistration::new(name, "market-data", MeshPort::Lakehouse, now())?
            .produced_from(parents.iter().map(|p| (*p).to_string()).collect())
            .licensed(granted(name, Usage::Research))
            .licensed(granted(name, Usage::Derive)),
    )
}

/// Two vendor feeds, a feature set built from both, and a signal from that.
fn pipeline() -> Result<Catalog> {
    let mut catalog = Catalog::new();
    catalog.register(dataset("vendor.prices", &[])?)?;
    catalog.register(dataset("vendor.sentiment", &[])?)?;
    catalog.register(dataset("features.daily", &["vendor.prices", "vendor.sentiment"])?)?;
    catalog.register(dataset("signals.momentum", &["features.daily"])?)?;
    Ok(catalog)
}

#[test]
fn lineage_resolves_to_the_roots_a_dataset_was_built_from() -> Result<()> {
    let catalog = pipeline()?;
    let lineage = catalog.lineage_of("signals.momentum")?;
    lineage.require_resolved()?;

    assert!(lineage.is_resolved());
    assert_eq!(
        lineage.roots(),
        &["vendor.prices".to_string(), "vendor.sentiment".to_string()]
    );
    assert_eq!(lineage.depth(), 2);
    assert!(lineage.breaks().is_empty());
    assert!(lineage.cycles().is_empty());
    Ok(())
}

#[test]
fn a_broken_lineage_names_the_missing_parent_and_who_claimed_it() -> Result<()> {
    // The realistic break: a dataset was retired from the catalogue while
    // something downstream still declares it as a parent.
    let mut catalog = pipeline()?;
    catalog.register(dataset("features.daily", &["vendor.prices", "vendor.gone"])?)?;

    let lineage = catalog.lineage_of("signals.momentum")?;
    assert!(!lineage.is_resolved());
    assert_eq!(lineage.breaks().len(), 1);
    assert_eq!(lineage.breaks()[0].missing, "vendor.gone");
    assert_eq!(lineage.breaks()[0].referenced_by, "features.daily");

    let error = lineage
        .require_resolved()
        .expect_err("a lineage with a missing parent does not resolve");
    assert!(error.message().contains("vendor.gone"));
    assert!(error.message().contains("features.daily"));

    // And the whole catalogue can be swept for the same problem.
    let unresolved = catalog.unresolved();
    assert_eq!(unresolved.len(), 2);
    Ok(())
}

#[test]
fn a_dataset_cannot_be_registered_as_its_own_parent() {
    // Always a typo, and catching it here means the lineage walk's cycle
    // report is about real cycles.
    let mut catalog = Catalog::new();
    let Ok(registration) = dataset("features.daily", &["features.daily"]) else {
        panic!("the registration must build");
    };
    let Err(error) = catalog.register(registration) else {
        panic!("a self-parent must be refused");
    };
    assert!(error.message().contains("its own parent"));
}

#[test]
fn a_cyclical_lineage_is_reported_rather_than_walked_forever() -> Result<()> {
    // Two pipelines each believing they feed the other. Neither can be rebuilt
    // and the walk must say so instead of hanging.
    let mut catalog = Catalog::new();
    catalog.register(dataset("a", &["b"])?)?;
    catalog.register(dataset("b", &["a"])?)?;

    let lineage = catalog.lineage_of("a")?;
    assert!(!lineage.is_resolved());
    assert_eq!(lineage.cycles(), &["a".to_string()]);
    assert!(
        lineage
            .require_resolved()
            .expect_err("a cycle does not resolve")
            .message()
            .contains("cyclical")
    );
    Ok(())
}

#[test]
fn a_diamond_lineage_resolves_once_rather_than_twice() -> Result<()> {
    // One feed producing two features, both feeding a model, is normal.
    let mut catalog = Catalog::new();
    catalog.register(dataset("vendor.prices", &[])?)?;
    catalog.register(dataset("features.left", &["vendor.prices"])?)?;
    catalog.register(dataset("features.right", &["vendor.prices"])?)?;
    catalog.register(dataset("model.input", &["features.left", "features.right"])?)?;

    let lineage = catalog.lineage_of("model.input")?;
    lineage.require_resolved()?;
    assert_eq!(lineage.roots(), &["vendor.prices".to_string()]);
    Ok(())
}

#[test]
fn a_quarantined_dataset_cannot_be_read_however_well_licensed_it_is() -> Result<()> {
    // Quality is checked before licensing. A licence says what the platform
    // may do with correct data, not with data known to be wrong.
    let mut catalog = pipeline()?;
    assert!(catalog.usable_for("vendor.prices", Usage::Research, now()).is_ok());

    catalog.quarantine(
        "vendor.prices",
        "the vendor republished three days of adjusted closes without notice",
        now(),
    )?;

    let error = catalog
        .usable_for("vendor.prices", Usage::Research, now())
        .expect_err("a quarantined dataset must not be read");
    assert!(error.message().contains("quarantined"));
    assert!(error.message().contains("republished"));
    Ok(())
}

#[test]
fn a_degraded_dataset_is_still_usable_because_the_two_states_are_different() -> Result<()> {
    // Collapsing degraded into quarantined would mean either that every
    // imperfection stops the platform or that nothing does.
    let mut catalog = pipeline()?;
    catalog.set_quality(
        "vendor.sentiment",
        QualityState::Degraded {
            since: now(),
            reason: "coverage fell to 80% of the universe".to_string(),
        },
    )?;
    assert!(catalog.usable_for("vendor.sentiment", Usage::Research, now()).is_ok());
    Ok(())
}

#[test]
fn a_dataset_not_licensed_for_a_use_is_refused_by_the_catalogue_too() -> Result<()> {
    // The catalogue and `qip-compliance` speak the same vocabulary, so they
    // cannot disagree about what a licence says.
    let catalog = pipeline()?;
    assert!(catalog.usable_for("vendor.prices", Usage::Derive, now()).is_ok());

    let error = catalog
        .usable_for("vendor.prices", Usage::Trade, now())
        .expect_err("a dataset licensed for research is not licensed to trade on");
    assert!(error.message().contains("vendor.prices"));
    assert!(error.message().contains("trade"));
    Ok(())
}

#[test]
fn an_expired_licence_stops_being_usable_without_anything_else_changing() -> Result<()> {
    let catalog = pipeline()?;
    let later = now().saturating_add(Duration::from_days(60));
    assert!(catalog.usable_for("vendor.prices", Usage::Research, now()).is_ok());
    assert!(catalog.usable_for("vendor.prices", Usage::Research, later).is_err());
    Ok(())
}

#[test]
fn quarantining_a_feed_names_everything_computed_from_it() -> Result<()> {
    // What an incident actually needs: when a feed is found to be wrong, this
    // is the list of things downstream that are now also suspect.
    let catalog = pipeline()?;
    assert_eq!(
        catalog.impacted_by("vendor.prices"),
        vec!["features.daily".to_string(), "signals.momentum".to_string()]
    );
    assert_eq!(catalog.children_of("vendor.prices"), vec!["features.daily"]);
    assert!(catalog.impacted_by("signals.momentum").is_empty());
    Ok(())
}

#[test]
fn an_unowned_or_unnamed_dataset_cannot_be_registered() {
    // An unowned dataset is one nobody fixes.
    assert!(DatasetRegistration::new("", "team", MeshPort::Lakehouse, now()).is_err());
    assert!(DatasetRegistration::new("x", "  ", MeshPort::Lakehouse, now()).is_err());
}

#[test]
fn quarantining_without_a_reason_is_refused() -> Result<()> {
    let mut catalog = pipeline()?;
    assert!(catalog.quarantine("vendor.prices", "bad", now()).is_err());
    assert!(catalog.quarantine("no.such.dataset", "a perfectly good reason", now()).is_err());
    Ok(())
}

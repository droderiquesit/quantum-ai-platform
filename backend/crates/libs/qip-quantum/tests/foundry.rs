#![allow(clippy::panic_in_result_fn)]

use qip_core::error::Result;
use qip_quantum::foundry::{FamilyDeclaration, FamilyKind, FamilyStatus, Foundry};

fn complete() -> FamilyDeclaration {
    FamilyDeclaration {
        classical_baseline: Some("annealing".into()),
        objective: Some("variance".into()),
        promotion_threshold: Some(0.01),
    }
}

#[test]
fn no_blueprint_family_is_silently_absent_from_the_platform_foundry() -> Result<()> {
    let foundry = Foundry::platform()?;
    assert_eq!(FamilyKind::ALL.len(), 7, "premise: seven named families");
    assert!(
        foundry.unaccounted().is_empty(),
        "{:?}",
        foundry.unaccounted()
    );
    match foundry.status(FamilyKind::PortfolioAllocation) {
        Some(FamilyStatus::Registered(f)) => {
            assert!(!f.classical_baseline().is_empty() && !f.objective().is_empty());
            assert!(f.promotion_threshold() > 0.0);
        }
        other => panic!("portfolio allocation must be registered: {other:?}"),
    }
    for kind in FamilyKind::ALL {
        if kind != FamilyKind::PortfolioAllocation {
            assert!(
                matches!(foundry.status(kind), Some(FamilyStatus::NotBuilt { reason }) if !reason.is_empty()),
                "{kind:?}"
            );
        }
    }
    Ok(())
}

#[test]
fn an_empty_foundry_reports_every_family_as_unaccounted() {
    assert_eq!(Foundry::new().unaccounted().len(), FamilyKind::ALL.len());
}

#[test]
fn a_family_without_a_classical_baseline_is_refused() -> Result<()> {
    let mut f = Foundry::new();
    f.register(FamilyKind::GraphSearch, &complete())?; // premise: complete is admitted
    let d = FamilyDeclaration {
        classical_baseline: None,
        ..complete()
    };
    let e = f.register(FamilyKind::PortfolioAllocation, &d).unwrap_err();
    assert!(
        e.message().contains("classical baseline"),
        "{}",
        e.message()
    );
    let blank = FamilyDeclaration {
        classical_baseline: Some("  ".into()),
        ..complete()
    };
    assert!(f.register(FamilyKind::PortfolioAllocation, &blank).is_err());
    Ok(())
}

#[test]
fn a_family_without_a_shared_objective_is_refused() -> Result<()> {
    let mut f = Foundry::new();
    let d = FamilyDeclaration {
        objective: None,
        ..complete()
    };
    let e = f.register(FamilyKind::GraphSearch, &d).unwrap_err();
    assert!(e.message().contains("shared objective"), "{}", e.message());
    f.register(FamilyKind::GraphSearch, &complete())?;
    Ok(())
}

#[test]
fn a_family_without_a_usable_promotion_threshold_is_refused() -> Result<()> {
    let mut f = Foundry::new();
    for bad in [
        None,
        Some(0.0),
        Some(-0.1),
        Some(f64::NAN),
        Some(f64::INFINITY),
    ] {
        let d = FamilyDeclaration {
            promotion_threshold: bad,
            ..complete()
        };
        let e = f.register(FamilyKind::GraphSearch, &d).unwrap_err();
        assert!(
            e.message().contains("promotion threshold"),
            "{bad:?}: {}",
            e.message()
        );
    }
    assert!(f.status(FamilyKind::GraphSearch).is_none());
    f.register(FamilyKind::GraphSearch, &complete())?;
    Ok(())
}

#[test]
fn a_not_built_record_needs_a_reason_and_a_family_cannot_be_accounted_for_twice() -> Result<()> {
    let mut f = Foundry::new();
    assert!(f.record_not_built(FamilyKind::GraphSearch, " ").is_err());
    f.record_not_built(FamilyKind::GraphSearch, "no encoding yet")?;
    assert!(f.register(FamilyKind::GraphSearch, &complete()).is_err());
    Ok(())
}

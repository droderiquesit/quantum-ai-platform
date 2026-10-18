#![allow(clippy::panic_in_result_fn)]

use qip_core::error::Result;
use qip_quantum::foundry::{FamilyDeclaration, FamilyKind, FamilyStatus, Foundry};

fn complete() -> FamilyDeclaration {
    FamilyDeclaration {
        classical_baseline: Some("annealing".into()),
        objective: Some("variance".into()),
        promotion_threshold: Some(0.01),
        encoding: Some("PortfolioProblem::to_qubo".into()),
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

#[test]
fn a_job_of_each_eligible_class_is_admitted_and_anything_else_is_refused_naming_the_classes() {
    use qip_quantum::foundry::{JobClass, admit_job};
    assert_eq!(JobClass::ALL.len(), 3, "premise: three eligible classes");
    for class in JobClass::ALL {
        assert_eq!(admit_job(class.name(), false).ok(), Some(class));
    }
    // Substring trap: "optimisation" is a substring of "optimisation-live"
    // and must not match it.
    for bad in ["trading", "optimisation-live", "", "Optimisation"] {
        let refusal = admit_job(bad, false).unwrap_err();
        for class in JobClass::ALL {
            assert!(
                refusal.message().contains(class.name()),
                "{bad}: {}",
                refusal.message()
            );
        }
    }
    let sync = admit_job("optimisation", true).unwrap_err();
    assert!(sync.message().contains("synchronous"), "{}", sync.message());
    assert!(
        sync.message().contains("hypothesis-search"),
        "{}",
        sync.message()
    );
}

/// R12-QUANT-1076: a family is admitted only with an encoding, refused by
/// name without one, and the platform's portfolio family declares the QUBO
/// encoder. A blank string is no more an encoding than a missing one.
#[test]
fn a_family_without_an_encoding_is_refused_by_name() -> Result<()> {
    let mut f = Foundry::new();
    for missing in [None, Some("  ".to_string())] {
        let d = FamilyDeclaration {
            encoding: missing.clone(),
            ..complete()
        };
        let e = f.register(FamilyKind::PortfolioAllocation, &d).unwrap_err();
        assert!(
            e.message().contains("without an encoding"),
            "{missing:?}: {}",
            e.message()
        );
    }
    assert!(f.status(FamilyKind::PortfolioAllocation).is_none());

    match Foundry::platform()?.status(FamilyKind::PortfolioAllocation) {
        Some(FamilyStatus::Registered(family)) => assert_eq!(
            family.encoding(),
            qip_quantum::foundry::PORTFOLIO_ALLOCATION_ENCODING
        ),
        other => panic!("portfolio allocation must be registered: {other:?}"),
    }
    Ok(())
}

/// R12-QUANT-4040. The symbolic family has an encoder and a proven baseline in
/// the tree, so the generic "no encoding, baseline or benchmark exists"
/// reason would be a false statement about it - while registering it would
/// let `model_serving` promote a benefit nothing in the cycle can produce.
///
/// Mutation: record the symbolic family with the generic reason - the reason
/// no longer names the encoder and the first assertion fails. Mutation:
/// register it - the `NotBuilt` match fails.
#[test]
fn the_symbolic_family_is_not_routed_and_its_reason_names_the_encoder_that_exists() -> Result<()> {
    let foundry = Foundry::platform()?;
    let Some(FamilyStatus::NotBuilt { reason }) =
        foundry.status(FamilyKind::SymbolicCombinatorialSearch)
    else {
        panic!("the symbolic family must be accounted for as not built until the cycle routes it");
    };
    assert!(
        reason.contains("`Problem::to_qubo`") && reason.contains("`Problem::minimise`"),
        "the reason does not name the encoder and baseline that exist: {reason}"
    );
    let Some(FamilyStatus::NotBuilt { reason: generic }) = foundry.status(FamilyKind::GraphSearch)
    else {
        panic!("premise: graph search is not built");
    };
    assert_ne!(reason, generic, "premise: the symbolic reason is its own");
    Ok(())
}

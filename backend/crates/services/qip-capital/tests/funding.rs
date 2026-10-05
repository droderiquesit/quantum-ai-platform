//! The funding plan (CAPITAL-001): every requirement names its sources, the
//! sources sum exactly to it, and one no source can meet is refused whole.

#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_capital::funding::{
    FundingRequirement, FundingSource, RequirementKind, SourceBalance, plan_funding,
};
use qip_capital::treasury::{FinancingFunction, FinancingPermissions};
use qip_core::error::Result;
use qip_core::rng::{Rng, Xoshiro256};
use qip_core::{Decimal, dec};
use std::collections::BTreeMap;

const ENTITY: &str = "platform-ltd";
const JURISDICTION: &str = "GB";

fn own(book: &str, available: Decimal) -> SourceBalance {
    SourceBalance {
        source: FundingSource::OwnCapital { book: book.into() },
        available,
    }
}

fn credit_line(counterparty: &str, available: Decimal) -> SourceBalance {
    SourceBalance {
        source: FundingSource::Financing {
            function: FinancingFunction::CreditLine,
            counterparty: counterparty.into(),
        },
        available,
    }
}

fn requirement(id: &str, kind: RequirementKind, amount: Decimal) -> FundingRequirement {
    FundingRequirement {
        id: id.into(),
        kind,
        strategy: "mm-eu".into(),
        region: "eu".into(),
        amount,
    }
}

fn permitted(counterparties: &[&str]) -> FinancingPermissions {
    let mut permissions = FinancingPermissions::new();
    for counterparty in counterparties {
        permissions.permit(
            FinancingFunction::CreditLine,
            ENTITY,
            counterparty,
            JURISDICTION,
        );
    }
    permissions
}

#[test]
fn for_generated_requirements_every_funded_one_names_sources_summing_exactly_to_it_and_every_other_is_refused_whole()
-> Result<()> {
    let mut rng = Xoshiro256::seeded(0x00CA_0001);
    let (mut funded_total, mut refused_total, mut multi_source, mut financed) = (0, 0, 0, 0);
    for case in 0..300u64 {
        let balances = vec![
            own("treasury", Decimal::from_int(rng.below(4_000) as i64)),
            own("reserve", Decimal::from_int(rng.below(2_000) as i64)),
            credit_line("bank-a", Decimal::from_int(rng.below(3_000) as i64)),
            // Never permitted below, so its balance must never be drawn or
            // counted, however large.
            credit_line("bank-unlicensed", dec!("1000000")),
        ];
        let requirements: Vec<FundingRequirement> = (0..1 + rng.below(8))
            .map(|n| {
                let kind = if rng.below(2) == 0 {
                    RequirementKind::Grant
                } else {
                    RequirementKind::Placement
                };
                requirement(
                    &format!("r{n}"),
                    kind,
                    Decimal::from_int(1 + rng.below(3_000) as i64),
                )
            })
            .collect();

        let plan = plan_funding(
            &requirements,
            &balances,
            &permitted(&["bank-a"]),
            ENTITY,
            JURISDICTION,
        )?;

        // Every requirement is funded or refused, and none is both.
        assert_eq!(
            plan.funded.len() + plan.refused.len(),
            requirements.len(),
            "case {case}: a requirement was dropped or answered twice"
        );
        let available: BTreeMap<&FundingSource, Decimal> =
            balances.iter().map(|b| (&b.source, b.available)).collect();
        let mut drawn: BTreeMap<&FundingSource, Decimal> = BTreeMap::new();
        for funded in &plan.funded {
            let required = requirements
                .iter()
                .find(|r| r.id == funded.requirement)
                .expect("a funded requirement was asked for");
            // The sources sum exactly to the requirement.
            let sum = funded.draws.iter().fold(Decimal::ZERO, |s, d| s + d.amount);
            assert_eq!(
                sum, required.amount,
                "case {case}: {} is not funded exactly",
                required.id
            );
            assert!(!funded.draws.is_empty());
            for draw in &funded.draws {
                // Every draw names a source that was offered, and a positive amount.
                assert!(
                    available.contains_key(&draw.source),
                    "case {case}: unknown source"
                );
                assert!(draw.amount.is_positive());
                assert_ne!(
                    draw.source,
                    FundingSource::Financing {
                        function: FinancingFunction::CreditLine,
                        counterparty: "bank-unlicensed".into(),
                    },
                    "case {case}: an unpermitted financing source was drawn"
                );
                *drawn.entry(&draw.source).or_insert(Decimal::ZERO) += draw.amount;
                financed += usize::from(matches!(draw.source, FundingSource::Financing { .. }));
            }
            multi_source += usize::from(funded.draws.len() > 1);
        }
        // No source funds more than it has.
        for (source, total) in &drawn {
            assert!(
                *total <= available[source],
                "case {case}: {source:?} is overdrawn"
            );
        }
        // A refused requirement drew nothing: it appears in no funding.
        for refusal in &plan.refused {
            assert!(
                plan.funded
                    .iter()
                    .all(|f| f.requirement != refusal.requirement),
                "case {case}: {} was both refused and funded",
                refusal.requirement
            );
        }
        funded_total += plan.funded.len();
        refused_total += plan.refused.len();
    }
    // Premise: the sweep funded, refused, split a requirement across sources
    // and reached the financing source.
    assert!(
        funded_total > 50 && refused_total > 50 && multi_source > 20 && financed > 20,
        "{funded_total} {refused_total} {multi_source} {financed}"
    );
    Ok(())
}

#[test]
fn a_requirement_no_source_can_meet_is_refused_rather_than_partially_funded() -> Result<()> {
    let balances = [
        own("treasury", dec!("600")),
        credit_line("bank-a", dec!("300")),
    ];
    let requirements = [
        requirement("too-big", RequirementKind::Grant, dec!("901")),
        requirement("fits", RequirementKind::Placement, dec!("900")),
    ];
    let plan = plan_funding(
        &requirements,
        &balances,
        &permitted(&["bank-a"]),
        ENTITY,
        JURISDICTION,
    )?;

    // 901 against 900 on hand: refused, and it took nothing with it, which is
    // what lets the next requirement be funded in full.
    assert_eq!(plan.refused.len(), 1);
    assert_eq!(plan.refused[0].requirement, "too-big");
    assert!(
        plan.refused[0]
            .reason
            .contains("needs 901 and the permitted sources hold 900"),
        "{}",
        plan.refused[0].reason
    );
    assert_eq!(plan.funded.len(), 1);
    assert_eq!(plan.funded[0].requirement, "fits");
    // Own capital first, then the financing source for the remainder.
    let draws: Vec<(bool, Decimal)> = plan.funded[0]
        .draws
        .iter()
        .map(|d| {
            (
                matches!(d.source, FundingSource::OwnCapital { .. }),
                d.amount,
            )
        })
        .collect();
    assert_eq!(draws, vec![(true, dec!("600")), (false, dec!("300"))]);
    Ok(())
}

#[test]
fn a_financing_source_with_no_recorded_permission_funds_nothing() -> Result<()> {
    let balances = [
        own("treasury", dec!("100")),
        credit_line("bank-a", dec!("5000")),
    ];
    let requirements = [requirement("r", RequirementKind::Grant, dec!("500"))];
    // Premise: with the permission recorded the same requirement is funded,
    // so the refusal below is the permission's and not the arithmetic's.
    let with = plan_funding(
        &requirements,
        &balances,
        &permitted(&["bank-a"]),
        ENTITY,
        JURISDICTION,
    )?;
    assert_eq!(with.funded.len(), 1);

    let without = plan_funding(
        &requirements,
        &balances,
        &permitted(&[]),
        ENTITY,
        JURISDICTION,
    )?;
    assert!(without.funded.is_empty());
    assert!(
        without.refused[0]
            .reason
            .contains("1 financing source(s) left out"),
        "{}",
        without.refused[0].reason
    );
    Ok(())
}

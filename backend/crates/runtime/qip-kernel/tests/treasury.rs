//! The treasury as the kernel composes it (ADR 0388).
//!
//! `qip-treasury`'s own tests hold each book. These hold what the kernel adds
//! by putting them together: which sources a treasury counts at all, and the
//! two refusals that apply to a whole requirement after the planner has named
//! its draws.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic_in_result_fn)] // integration tests may unwrap and assert: a panic is the failure report

use qip_capital::allocation::{AllocationLimits, CapitalAllocator, DrawdownSchedule};
use qip_capital_fabric::forecast::{DemandForecast, DemandKind, Interval};
use qip_capital_fabric::location::{CapitalLocation, Region};
use qip_capital_fabric::plan::{PrePositioningPlan, PrePositioningPlanner, PrePositioningRequest};
use qip_capital_fabric::settlement::{SettlementBook, SettlementCalendar, SettlementConvention};
use qip_capital_fabric::transfer::{FundingCurve, FxRates, TransferCostModel};
use qip_contracts::venue::VenueId;
use qip_core::error::Result;
use qip_core::time::{Duration, Timestamp};
use qip_core::{Currency, Decimal, dec};
use qip_financial::costs::{LiquidityProfile, TransactionCostModel};
use qip_kernel::treasury::{Treasury, TreasuryDeclaration};
use qip_treasury::funding::{FundingRequirement, FundingSource, RequirementKind, SourceBalance};
use qip_treasury::treasury::{FinancingFunction, LeverageDecision};

const ENTITY: &str = "central";
const JURISDICTION: &str = "home";
const LENDER: &str = "repo-desk";

fn requirement(id: &str, amount: Decimal) -> FundingRequirement {
    FundingRequirement {
        id: id.to_owned(),
        kind: RequirementKind::Placement,
        strategy: "carry".to_owned(),
        region: "home".to_owned(),
        amount,
    }
}

fn own(available: Decimal) -> SourceBalance {
    SourceBalance {
        source: FundingSource::OwnCapital {
            book: ENTITY.to_owned(),
        },
        available,
    }
}

fn repo(available: Decimal) -> SourceBalance {
    SourceBalance {
        source: FundingSource::Financing {
            function: FinancingFunction::Repo,
            counterparty: LENDER.to_owned(),
        },
        available,
    }
}

fn ceiling(max_leverage: Decimal) -> LeverageDecision {
    LeverageDecision {
        strategy: "carry".to_owned(),
        region: "home".to_owned(),
        max_leverage,
        decided_at: Timestamp::from_secs(1),
    }
}

fn declaration(
    reserve_floor: Option<Decimal>,
    leverage: Vec<LeverageDecision>,
) -> TreasuryDeclaration {
    TreasuryDeclaration {
        reserve_floor,
        leverage,
        permissions: vec![(FinancingFunction::Repo, LENDER.to_owned())],
    }
}

#[test]
fn a_treasury_with_no_reserve_floor_counts_no_financing_source_however_permitted_and_says_why()
-> Result<()> {
    // The failure this prevents: a permission and a leverage ceiling recorded,
    // the floor never declared, and borrowed money planned against a reserve
    // nobody stated.
    let requirements = [requirement("a", dec!("150"))];
    let balances = [own(dec!("100")), repo(dec!("100"))];

    // Premise: with the floor declared the same ask is funded, and the repo
    // funds part of it. Without this the refusal below could be the
    // planner's, not the missing floor's.
    let whole = Treasury::declared(
        ENTITY,
        JURISDICTION,
        declaration(Some(Decimal::ZERO), vec![ceiling(dec!("2"))]),
    )?;
    let funded = whole.fund(&requirements, &balances)?;
    assert_eq!(
        funded.plan.funded.len(),
        1,
        "premise: fundable with a floor"
    );
    assert!(
        funded.plan.funded[0]
            .draws
            .iter()
            .any(|draw| matches!(draw.source, FundingSource::Financing { .. })),
        "premise: the repo is drawn on"
    );
    assert!(funded.financing_withheld.is_empty());

    let no_floor = Treasury::declared(
        ENTITY,
        JURISDICTION,
        declaration(None, vec![ceiling(dec!("2"))]),
    )?;
    let funding = no_floor.fund(&requirements, &balances)?;
    assert!(funding.plan.funded.is_empty(), "nothing is financed");
    assert_eq!(funding.plan.refused.len(), 1);
    assert_eq!(
        funding.financing_withheld.len(),
        1,
        "{:?}",
        funding.financing_withheld
    );
    assert!(
        funding.financing_withheld[0].starts_with("no reserve floor is declared"),
        "{:?}",
        funding.financing_withheld
    );

    // The treasury a composition root builds: both absences are named.
    let undeclared = Treasury::undeclared(ENTITY, JURISDICTION);
    let reasons = undeclared.financing_withheld();
    assert_eq!(reasons.len(), 2, "{reasons:?}");
    assert!(
        reasons[1].starts_with("no leverage ceiling is decided"),
        "{reasons:?}"
    );
    assert!(
        undeclared
            .fund(&requirements, &balances)?
            .plan
            .funded
            .is_empty()
    );
    Ok(())
}

#[test]
fn a_financed_requirement_past_its_leverage_decision_is_refused_whole_and_its_own_capital_funds_the_next()
-> Result<()> {
    // "a" needs 150: 100 of own capital and 50 borrowed, a leverage of 1.5.
    // "b" needs 100 and comes second, so the planner alone gives it the
    // repo's remainder only.
    let requirements = [requirement("a", dec!("150")), requirement("b", dec!("100"))];
    let balances = [own(dec!("100")), repo(dec!("100"))];

    // Premise: at a ceiling of 2 "a" is admitted and takes all the own
    // capital and half the repo, so "b" finds 50 where it needs 100 and is
    // the one the planner refuses.
    let wide = Treasury::declared(
        ENTITY,
        JURISDICTION,
        declaration(Some(Decimal::ZERO), vec![ceiling(dec!("2"))]),
    )?;
    let admitted = wide.fund(&requirements, &balances)?;
    assert_eq!(admitted.plan.funded.len(), 1);
    assert_eq!(admitted.plan.funded[0].requirement, "a");
    assert_eq!(admitted.plan.refused[0].requirement, "b");

    for leverage in [vec![ceiling(dec!("1.2"))], Vec::new()] {
        let decided = !leverage.is_empty();
        let narrow = Treasury::declared(
            ENTITY,
            JURISDICTION,
            declaration(Some(Decimal::ZERO), leverage),
        )?;
        let funding = narrow.fund(&requirements, &balances)?;
        assert_eq!(funding.plan.refused.len(), 1, "{:?}", funding.plan);
        assert_eq!(funding.plan.refused[0].requirement, "a");
        let expected = if decided {
            "above the recorded leverage of 1.2"
        } else {
            "no leverage decision recorded for carry/home"
        };
        assert!(
            funding.plan.refused[0].reason.contains(expected),
            "{}",
            funding.plan.refused[0].reason
        );
        // Refused whole: the 100 of own capital "a" had drawn is back, and
        // funds "b" with nothing borrowed.
        assert_eq!(funding.plan.funded.len(), 1);
        assert_eq!(funding.plan.funded[0].requirement, "b");
        assert_eq!(funding.plan.funded[0].draws.len(), 1);
        assert_eq!(funding.plan.funded[0].draws[0], {
            qip_treasury::funding::Draw {
                source: own(dec!("100")).source,
                amount: dec!("100"),
            }
        });
    }
    Ok(())
}

#[test]
fn a_declared_reserve_floor_refuses_the_own_capital_placement_that_would_breach_it_and_an_undeclared_one_applies_none()
-> Result<()> {
    let requirements = [requirement("a", dec!("60")), requirement("b", dec!("30"))];
    let balances = [own(dec!("100"))];

    // Premise: with no floor both are funded from own capital, and the
    // funding says no floor was applied rather than implying one of zero.
    let undeclared = Treasury::undeclared(ENTITY, JURISDICTION).fund(&requirements, &balances)?;
    assert_eq!(undeclared.plan.funded.len(), 2);
    assert!(!undeclared.reserve_floor_applied);
    assert!(
        undeclared
            .describe()
            .contains("no reserve floor applied to own capital, none being declared"),
        "{}",
        undeclared.describe()
    );

    // A floor of 30 on 100: "a" leaves 40 uncommitted, "b" would leave 10.
    let floored = Treasury::declared(
        ENTITY,
        JURISDICTION,
        TreasuryDeclaration {
            reserve_floor: Some(dec!("30")),
            ..TreasuryDeclaration::default()
        },
    )?
    .fund(&requirements, &balances)?;
    assert!(floored.reserve_floor_applied);
    assert_eq!(floored.plan.funded.len(), 1);
    assert_eq!(floored.plan.funded[0].requirement, "a");
    assert_eq!(floored.plan.refused.len(), 1);
    assert_eq!(floored.plan.refused[0].requirement, "b");
    assert!(
        floored.plan.refused[0].reason.contains("floor"),
        "{}",
        floored.plan.refused[0].reason
    );
    Ok(())
}

#[test]
fn a_negative_reserve_floor_is_refused_rather_than_read_as_no_floor() {
    let refused = Treasury::declared(
        ENTITY,
        JURISDICTION,
        TreasuryDeclaration {
            reserve_floor: Some(dec!("-1")),
            ..TreasuryDeclaration::default()
        },
    );
    let error = refused.expect_err("a negative floor is not a floor");
    assert!(error.message().contains("negative"), "{}", error.message());
}

/// A plan the fabric's own planner built, with one lane worth moving to: ten
/// million of margin forecast at a London venue four days out.
fn a_plan_with_a_move() -> Result<PrePositioningPlan> {
    let now = Timestamp::from_civil(2024, 3, 7).saturating_add(Duration::from_hours(9));
    let emea = Region::new("emea");
    let venue = CapitalLocation::new(emea.clone(), Currency::USD, VenueId::new("XLON"));
    let treasury =
        CapitalLocation::new(Region::new("namr"), Currency::USD, VenueId::new("TREASURY"));
    let mut settlement = SettlementBook::new();
    settlement.declare_jurisdiction(
        emea.clone(),
        SettlementCalendar::weekday(SettlementConvention::T1)?,
    )?;
    settlement.assign_venue(VenueId::new("XLON"), emea)?;
    let limit = dec!("100000000");
    let planner = PrePositioningPlanner::new(
        AllocationLimits::new(limit, limit, limit, limit)?,
        TransferCostModel::new(
            TransactionCostModel::listed(1.0),
            LiquidityProfile::listed(Decimal::from_int(5_000_000_000), 1.0),
            FundingCurve::flat(400.0)?,
            dec!("25"),
            300.0,
        )?,
        settlement,
    );
    let live = CapitalAllocator::new(planner.limits().clone(), DrawdownSchedule::default())
        .allocate(&[], 0.0, now)?;
    let request =
        PrePositioningRequest::new(treasury, dec!("50000000"), FxRates::new(Currency::USD))?
            .with_forecast(DemandForecast::new(
                venue,
                DemandKind::Margin,
                now,
                Duration::from_days(4),
                Interval::new(dec!("9000000"), dec!("10000000"), dec!("11000000"), 0.80)?,
                60,
            )?);
    planner.plan(&request, &live, now)
}

#[test]
fn every_move_of_a_pre_positioning_plan_is_sourced_whole_from_the_plans_own_budget_and_from_nothing_else()
-> Result<()> {
    // The failure this prevents: a plan reported with no source named for its
    // capital, or funded against a balance other than the budget the planner
    // was held to, so the plan and its funding disagree about one figure.
    let plan = a_plan_with_a_move()?;
    assert!(
        !plan.moves.is_empty(),
        "premise: the plan proposes a move, so there is something to fund"
    );
    let treasury = Treasury::undeclared(ENTITY, JURISDICTION);
    let funding = treasury.fund_plan(&plan, ENTITY)?;
    assert!(
        funding.plan.refused.is_empty(),
        "{:?}",
        funding.plan.refused
    );
    assert_eq!(funding.plan.funded.len(), plan.moves.len());
    for (index, (funded, planned)) in funding.plan.funded.iter().zip(&plan.moves).enumerate() {
        assert_eq!(
            funded.requirement,
            format!("pre-position:{}:{index}", plan.at.as_nanos())
        );
        assert_eq!(funded.draws.len(), 1, "{funded:?}");
        assert_eq!(funded.draws[0].source, own(Decimal::ZERO).source);
        assert_eq!(funded.draws[0].amount, planned.amount_base);
    }
    assert_eq!(
        funding.describe(),
        format!(
            "treasury: {} placement(s) sourced, 0 refused; no financing counted ({}); no reserve \
             floor applied to own capital, none being declared",
            plan.moves.len(),
            treasury.financing_withheld().join("; ")
        )
    );

    // The source's balance is the plan's budget: one unit short of the first
    // move and that move has no source.
    let mut short = plan.clone();
    short.budget = plan.moves[0].amount_base - dec!("1");
    let funding = treasury.fund_plan(&short, ENTITY)?;
    assert_eq!(funding.plan.refused.len(), plan.moves.len());
    assert!(
        funding.plan.refused[0]
            .reason
            .contains(&format!("the permitted sources hold {}", short.budget)),
        "{}",
        funding.plan.refused[0].reason
    );
    Ok(())
}

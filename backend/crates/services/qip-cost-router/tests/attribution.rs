//! R12-FINOPS-10136: cost is reported per strategy, model, data source and
//! region where attribution is possible.

#![allow(clippy::panic_in_result_fn)]

use qip_agents::Budget;
use qip_core::Decimal;
use qip_core::error::Result;
use qip_cost_router::{
    Attribution, ComputeLedger, CostDimension, IntelligenceTier, NO_MODEL, UNATTRIBUTED, report_by,
};

fn attributed(strategy: &str, source: &str, region: &str) -> Attribution {
    Attribution {
        strategy: Some(strategy.into()),
        data_source: Some(source.into()),
        region: Some(region.into()),
    }
}

/// Every entry carries all four keys, the report per strategy sums exactly
/// to each strategy's own ledgers, and a decision that names no strategy is
/// reported as unattributed rather than folded into one that does. The
/// failure this prevents is the monthly infrastructure line: spend nobody
/// can net against the edge that caused it.
#[test]
fn each_charge_carries_strategy_model_source_and_region_and_the_rollup_sums_per_strategy()
-> Result<()> {
    let mut momentum = ComputeLedger::new("momentum entry", Budget::deep_research())?
        .with_attribution(attributed("momentum", "vendor-a", "us-east"));
    momentum.charge(IntelligenceTier::DeterministicCode)?;
    momentum.charge(IntelligenceTier::DeepModel)?;
    let mut momentum_exit = ComputeLedger::new("momentum exit", Budget::research_default())?
        .with_attribution(attributed("momentum", "vendor-b", "eu-west"));
    momentum_exit.charge(IntelligenceTier::StatisticalModel)?;
    let mut carry = ComputeLedger::new("carry review", Budget::research_default())?
        .with_attribution(attributed("carry", "vendor-a", "us-east"));
    carry.charge(IntelligenceTier::Simulation)?;
    let mut cycle = ComputeLedger::new("cycle 7", Budget::research_default())?
        .with_attribution(Attribution::region("us-east"));
    cycle.charge(IntelligenceTier::DeterministicCode)?;

    // Premise: every ledger charged something, so the sums below are not of
    // empty ledgers that would agree with any report.
    for ledger in [&momentum, &momentum_exit, &carry, &cycle] {
        assert!(ledger.total_cost() > Decimal::ZERO, "{}", ledger.decision());
    }

    let entries = momentum.entries();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].strategy, "momentum");
    assert_eq!(entries[0].data_source, "vendor-a");
    assert_eq!(entries[0].region, "us-east");
    assert_eq!(entries[0].model, NO_MODEL);
    assert_eq!(
        entries[1].model,
        IntelligenceTier::DeepModel
            .model_tier()
            .expect("a deep model is a model")
            .as_str()
    );
    let cycle_entry = &cycle.entries()[0];
    assert_eq!(cycle_entry.strategy, UNATTRIBUTED);
    assert_eq!(cycle_entry.data_source, UNATTRIBUTED);
    assert_eq!(cycle_entry.region, "us-east");

    let ledgers = [&momentum, &momentum_exit, &carry, &cycle];
    let by_strategy = report_by(ledgers, CostDimension::Strategy);
    assert_eq!(
        by_strategy.get("momentum").copied(),
        Some(momentum.total_cost() + momentum_exit.total_cost())
    );
    assert_eq!(by_strategy.get("carry").copied(), Some(carry.total_cost()));
    assert_eq!(
        by_strategy.get(UNATTRIBUTED).copied(),
        Some(cycle.total_cost())
    );
    let by_region = report_by(ledgers, CostDimension::Region);
    assert_eq!(
        by_region.get("us-east").copied(),
        Some(momentum.total_cost() + carry.total_cost() + cycle.total_cost())
    );
    assert_eq!(
        by_region.get("eu-west").copied(),
        Some(momentum_exit.total_cost())
    );

    // Every cut of the same ledgers sums to the same total: no dimension
    // loses or invents spend.
    let total = ledgers
        .iter()
        .fold(Decimal::ZERO, |sum, ledger| sum + ledger.total_cost());
    for dimension in [
        CostDimension::Strategy,
        CostDimension::Model,
        CostDimension::DataSource,
        CostDimension::Region,
    ] {
        let sum = report_by(ledgers, dimension)
            .values()
            .fold(Decimal::ZERO, |sum, cost| sum + *cost);
        assert_eq!(sum, total, "{dimension:?}");
    }
    Ok(())
}

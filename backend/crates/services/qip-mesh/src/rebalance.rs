//! Inventory drift left by a distributed cycle, raised as work (MESH-033).
//!
//! Legs land wherever their venues are. If nothing converts the resulting
//! drift into a task, the next cycle starts from a position nobody chose and
//! pre-positioning (MESH-020) decays unseen. The task cites the epoch that
//! caused it, so the treasury can attribute the movement to a cycle.

use crate::peer::OpportunityEpoch;
use qip_core::Decimal;
use qip_core::error::{Error, Result};
use std::collections::{BTreeMap, BTreeSet};

/// A request to move `amount` of holdings at one `place` (region/venue) back
/// to what the placement wants. Positive `amount` means the place holds too
/// much and should shed it; negative means it holds too little.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RebalanceTask {
    pub place: String,
    pub amount: Decimal,
    pub opportunity: String,
    pub epoch_sequence: u64,
}

/// One task per place whose holding differs from its target by more than
/// `tolerance`, ordered by place. A place present on only one side counts
/// as holding zero on the other, so a target nobody filled is drift too.
pub fn rebalancing_tasks(
    epoch: &OpportunityEpoch,
    target: &BTreeMap<String, Decimal>,
    actual: &BTreeMap<String, Decimal>,
    tolerance: Decimal,
) -> Result<Vec<RebalanceTask>> {
    if tolerance.is_negative() {
        return Err(Error::invalid(
            "rebalance tolerance must not be negative; use zero to chase every unit",
        ));
    }
    let zero = Decimal::from_int(0);
    let places: BTreeSet<&String> = target.keys().chain(actual.keys()).collect();
    let mut tasks = Vec::new();
    for place in places {
        let held = actual.get(place).copied().unwrap_or(zero);
        let want = target.get(place).copied().unwrap_or(zero);
        let drift = held - want;
        if drift.abs() > tolerance {
            tasks.push(RebalanceTask {
                place: place.clone(),
                amount: drift,
                opportunity: epoch.opportunity.clone(),
                epoch_sequence: epoch.sequence,
            });
        }
    }
    Ok(tasks)
}

//! Treasury rebalancing, as intents (CAPITAL-020).
//!
//! [`crate::plan`] moves capital toward where demand is *forecast*. This
//! module answers the question that follows once a placement has been
//! decided: holdings have drifted from it, so what would bring them back, and
//! along which corridor.
//!
//! The answer is a list of [`TransferIntent`]s and nothing more. An intent is
//! a record (ADR 0021); each one still has to pass [`crate::gate::TransferGate`]
//! before it means anything, and there is no engine behind the gate. What
//! this module adds is that the intents exist at all, and that every one of
//! them names a corridor a person signed.
//!
//! **A drift with no active corridor is reported, never routed around.** The
//! failure this prevents is the helpful one: a planner that, finding no
//! signed route from A to B, proposes the transfer anyway and leaves the gate
//! to refuse it. A plan that reads as complete while half of it cannot be
//! carried is worse than a plan that says which half. So a location that
//! cannot be brought back along a corridor stays out of tolerance, in
//! [`RebalancePlan::unrouted`], with the reason.
//!
//! Only locations in the same currency are paired. Moving value across
//! currencies is a conversion and then a transfer, and pricing the two as one
//! is how the expensive half goes unnoticed ([`CapitalLocation::requires_conversion`]).

use crate::corridor::{Corridor, CorridorId, CorridorStage};
use crate::destination::DestinationKey;
use crate::gate::{StatedPurpose, TransferIntent};
use crate::location::CapitalLocation;
use qip_core::Decimal;
use qip_core::error::{Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// One rebalancing transfer and the corridor it runs along.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RebalanceMove {
    /// The corridor that carries it.
    pub corridor: CorridorId,
    /// The location holding more than its placement.
    pub from: CapitalLocation,
    /// The location holding less.
    pub to: CapitalLocation,
    /// The intent, for the gate.
    pub intent: TransferIntent,
}

/// A location left outside tolerance because no corridor could carry the
/// correction.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Unrouted {
    /// The location.
    pub location: CapitalLocation,
    /// Holding less placement, as it stands after the planned moves.
    pub deviation: Decimal,
    /// Why nothing could be planned for it.
    pub reason: String,
}

/// The transfers that would restore the placement, and what they cannot.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RebalancePlan {
    /// Transfers to make, in the order they were planned.
    pub moves: Vec<RebalanceMove>,
    /// Locations no corridor could correct.
    pub unrouted: Vec<Unrouted>,
}

/// Plan the transfers that bring every holding back within `tolerance` of the
/// decided `placement`.
///
/// `accounts` says which location each corridor destination credits. It is
/// passed in because the registry keys a destination by asset and address and
/// does not know where that address is; a caller that left an account out has
/// a corridor this plan will not use, which is the safe direction.
///
/// The location furthest from its placement is corrected first, against the
/// counterpart that takes the most of it. Each transfer is sized to bring one
/// of the two exactly back, so no transfer overshoots a placement.
pub fn rebalance(
    placement: &BTreeMap<CapitalLocation, Decimal>,
    holdings: &BTreeMap<CapitalLocation, Decimal>,
    tolerance: Decimal,
    corridors: &[Corridor],
    accounts: &BTreeMap<DestinationKey, CapitalLocation>,
) -> Result<RebalancePlan> {
    if tolerance.is_negative() {
        return Err(Error::invalid(format!(
            "a rebalancing tolerance is a distance and cannot be {tolerance}; state zero to \
             correct every drift"
        )));
    }
    let mut deviation: BTreeMap<CapitalLocation, Decimal> = BTreeMap::new();
    for (location, held) in holdings {
        *deviation.entry(location.clone()).or_insert(Decimal::ZERO) += *held;
    }
    for (location, target) in placement {
        *deviation.entry(location.clone()).or_insert(Decimal::ZERO) -= *target;
    }

    let mut plan = RebalancePlan {
        moves: Vec::new(),
        unrouted: Vec::new(),
    };
    let mut stuck: BTreeSet<CapitalLocation> = BTreeSet::new();
    loop {
        let Some((drifted, drift)) = deviation
            .iter()
            .filter(|(location, drift)| drift.abs() > tolerance && !stuck.contains(*location))
            .max_by(|a, b| a.1.abs().cmp(&b.1.abs()).then(b.0.cmp(a.0)))
            .map(|(location, drift)| (location.clone(), *drift))
        else {
            break;
        };
        // Every counterpart on the other side of its own placement, in the
        // same currency, that a corridor can actually reach.
        let counterpart = deviation
            .iter()
            .filter(|(location, other)| {
                location.currency == drifted.currency && other.signum() == -drift.signum()
            })
            .filter_map(|(location, other)| {
                let amount = drift.abs().min(other.abs());
                let (from, to) = if drift.is_positive() {
                    (&drifted, location)
                } else {
                    (location, &drifted)
                };
                route(corridors, accounts, from, to, amount)
                    .map(|corridor| (from.clone(), to.clone(), amount, corridor))
            })
            .max_by(|a, b| a.2.cmp(&b.2).then(b.1.cmp(&a.1)).then(b.0.cmp(&a.0)));

        let Some((from, to, amount, corridor)) = counterpart else {
            plan.unrouted.push(Unrouted {
                location: drifted.clone(),
                deviation: drift,
                reason: format!(
                    "{drifted} is {drift} from its placement and no active corridor under its \
                     per-transfer cap joins it to a {} location on the other side of its own; \
                     propose and sign one, or change the placement",
                    drifted.currency
                ),
            });
            stuck.insert(drifted);
            continue;
        };

        // The purpose the gate's fourth check reads: distance from the
        // placement across this currency, before and after this one transfer.
        let before = deviation
            .iter()
            .filter(|(location, _)| location.currency == from.currency)
            .fold(Decimal::ZERO, |sum, (_, drift)| sum + drift.abs());
        let after = before - amount - amount;
        let intent = TransferIntent::new(
            from.clone(),
            corridor.destination().clone(),
            amount,
            StatedPurpose::new(before, after)?,
        )?;
        for (location, change) in [(&from, -amount), (&to, amount)] {
            *deviation.entry(location.clone()).or_insert(Decimal::ZERO) += change;
        }
        plan.moves.push(RebalanceMove {
            corridor: corridor.id().clone(),
            from,
            to,
            intent,
        });
    }
    Ok(plan)
}

/// The corridor that carries `amount` from `from` to `to`, if one is active.
///
/// Three conditions and each is a refusal the gate would make anyway: the
/// corridor is active, it runs between exactly these two places, and the
/// amount is inside its per-transfer cap. Planning a transfer the gate is
/// certain to veto is not a plan.
fn route<'a>(
    corridors: &'a [Corridor],
    accounts: &BTreeMap<DestinationKey, CapitalLocation>,
    from: &CapitalLocation,
    to: &CapitalLocation,
    amount: Decimal,
) -> Option<&'a Corridor> {
    corridors.iter().find(|corridor| {
        corridor.stage() == CorridorStage::Active
            && corridor.source() == from
            && accounts.get(corridor.destination()) == Some(to)
            && amount <= corridor.caps().max_per_transfer()
    })
}

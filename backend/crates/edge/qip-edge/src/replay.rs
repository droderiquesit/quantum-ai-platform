//! Reading a journal back for the versions in force when each order was sent.
//!
//! An audit question about an order is "what was the cell running when it
//! did that" — the policy payload, the plan, the model weights — and the
//! chain answers it only if every one of those facts was recorded as an
//! entry of its own: `PolicyApplied` for the payload, `PlanAdmitted` for a
//! plan and the models it carries, `StrategyWithdrawn` for the end of one.
//! This reader walks the entries in sequence and names, at each
//! `OrderSent`, the state those entries had built up. It reads nothing but
//! the journal, so what it says is what a replay from the log alone would
//! say.

use crate::journal::{Decision, JournalEntry};
use qip_core::Timestamp;
use std::collections::BTreeMap;

/// The versions in force at one `OrderSent`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VersionsInForce {
    pub order_id: String,
    pub at: Timestamp,
    /// The sequence of the last policy payload applied before the order, or
    /// `None` if the journal holds no `PolicyApplied` before it — the reader
    /// does not guess a version the chain never recorded.
    pub policy_sequence: Option<u64>,
    /// Each strategy in force, by id, at its deployment digest.
    pub plans: BTreeMap<String, String>,
    /// Each model carried by a plan in force, by name, at its digest.
    pub models: BTreeMap<String, String>,
    /// The grants the order drew on, as `OrderSent` itself recorded them.
    pub grants: Vec<(String, String)>,
}

/// Name the versions in force at every `OrderSent` in `entries`.
///
/// `entries` must be in sequence order, as `Journal::entries` returns them.
/// A strategy admitted again replaces its earlier digest and models; one
/// withdrawn leaves the set. Model names are shared across plans only if the
/// digests agree, which `check_models_promoted` guarantees against the one
/// manifest, so the union by name is unambiguous.
pub fn versions_in_force(entries: &[JournalEntry]) -> Vec<VersionsInForce> {
    let mut policy_sequence: Option<u64> = None;
    let mut plans: BTreeMap<String, (String, Vec<(String, String)>)> = BTreeMap::new();
    let mut named = Vec::new();
    for entry in entries {
        match &entry.decision {
            Decision::PolicyApplied { sequence, .. } => policy_sequence = Some(*sequence),
            Decision::PlanAdmitted {
                strategy,
                digest,
                models,
            } => {
                plans.insert(strategy.clone(), (digest.clone(), models.clone()));
            }
            Decision::StrategyWithdrawn { strategy } => {
                plans.remove(strategy);
            }
            Decision::OrderSent {
                order_id, grants, ..
            } => named.push(VersionsInForce {
                order_id: order_id.clone(),
                at: entry.at,
                policy_sequence,
                plans: plans
                    .iter()
                    .map(|(strategy, (digest, _))| (strategy.clone(), digest.clone()))
                    .collect(),
                models: plans
                    .values()
                    .flat_map(|(_, models)| models.iter().cloned())
                    .collect(),
                grants: grants.clone(),
            }),
            _ => {}
        }
    }
    named
}

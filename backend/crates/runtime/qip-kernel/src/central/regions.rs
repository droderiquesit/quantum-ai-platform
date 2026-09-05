//! A region's grant, partitioned into disjoint per-cell shares before anything
//! ships (ADR 0039, option (a)).
//!
//! Every deployed node opens its region table over an amount its operator
//! typed, and two nodes under one regional grant hold two amounts nothing
//! sums — traceability F6's "operator discipline, not a structural
//! guarantee". The blueprint's grant is "per family per region", and seven
//! cells that decide alone while partitioned (ADR 0008) could each spend to
//! their own operator's number against a grant the centre had set aside once.
//!
//! [`partition`] is the centre's side of closing that. From the allocation
//! plan it already produces it takes each cell's gross — [`AllocationPlan::for_cell`],
//! the same number the cell's envelopes were issued against, so the share and
//! the envelopes are one fact from one source — and **refuses**, never
//! scales, a plan whose cells' shares would together exceed their region's
//! grant. A remainder left over after the shares stays unallocated rather
//! than being rounded onto anyone.
//!
//! # How a share reaches the cell
//!
//! Not as a number. The policy payload's `capital_grants` slot carries a
//! [`GrantManifest`]: the signatures of the grants the centre believes live
//! for that cell. The cell sums the `gross_limit` of the verified envelopes
//! the manifest names, and that sum is its share. For that to bound what the
//! partitioner computed, the manifest must never name envelopes whose gross
//! sums past the cell's share; a cell whose live envelopes already do — one
//! issued under an earlier, more generous plan — is **withheld** a manifest,
//! with the reason, and narrows to nothing until its grants are renewed
//! under the current plan. That is the fail-closed direction: a cell never
//! receives a share the centre had to guess at.
//!
//! # What this deliberately does not decide
//!
//! Where [`RegionMembership`] comes from. The ADR recommends operator-set
//! configuration beside the arbitrage policy and names the alternative —
//! deriving the grant from treasury on hand — as a different number with a
//! different owner. Until the owner decides, membership is an argument to
//! [`CentralPlane::region_shares`](super::CentralPlane::region_shares) and
//! nothing in the tree constructs one outside a test.

use qip_capital::allocation::AllocationPlan;
use qip_contracts::CapitalEnvelope;
use qip_contracts::policy::GrantManifest;
use qip_contracts::signal::StrategyId;
use qip_core::error::{Error, Result};
use qip_core::{Decimal, Timestamp};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Which cells make up which region, and what each region is granted.
///
/// Both maps are `BTreeMap` so the same membership partitions the same plan
/// identically on every machine. The only constructor validates: a grant is
/// positive, every cell's region has a grant, and no name is blank. A
/// membership that could name a cell in a region with no grant would be
/// answered by a share of nothing with no reason recorded, and a silent zero
/// is the number this module exists to remove.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegionMembership {
    /// Region to its grant, the most its cells may together be shared.
    grants: BTreeMap<String, Decimal>,
    /// Cell to the region it belongs to.
    cells: BTreeMap<String, String>,
}

impl RegionMembership {
    /// Refuses a non-positive grant, a cell in a region without a grant, and
    /// a blank name anywhere.
    pub fn new(grants: BTreeMap<String, Decimal>, cells: BTreeMap<String, String>) -> Result<Self> {
        for (region, grant) in &grants {
            if region.trim().is_empty() {
                return Err(Error::invalid(
                    "a region membership names a region with a blank name; every region needs \
                     a name its cells can be filed under",
                ));
            }
            if !grant.is_positive() {
                return Err(Error::invalid(format!(
                    "region {region} is granted {grant}; a grant is positive or the region is \
                     left out, because a region granted nothing partitions nothing"
                )));
            }
        }
        for (cell, region) in &cells {
            if cell.trim().is_empty() {
                return Err(Error::invalid(format!(
                    "a region membership files a cell with a blank name under {region}"
                )));
            }
            if !grants.contains_key(region) {
                return Err(Error::invalid(format!(
                    "cell {cell} is filed under region {region}, which has no grant; grant the \
                     region or leave the cell out, rather than sharing it a silent nothing"
                )));
            }
        }
        Ok(Self { grants, cells })
    }

    /// The region a cell belongs to, if it is in any.
    pub fn region_of(&self, cell: &str) -> Option<&str> {
        self.cells.get(cell).map(String::as_str)
    }

    /// A region's grant, if the region is known.
    pub fn grant(&self, region: &str) -> Option<Decimal> {
        self.grants.get(region).copied()
    }

    /// Every region and its grant, in name order.
    pub fn grants(&self) -> &BTreeMap<String, Decimal> {
        &self.grants
    }

    /// Every cell and its region, in cell order.
    pub fn cells(&self) -> &BTreeMap<String, String> {
        &self.cells
    }
}

/// One cell's disjoint share of its region's grant, and the manifest that
/// carries it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RegionShare {
    region: String,
    /// The share: the cell's gross in the plan, which is what its envelopes
    /// were issued against.
    amount: Decimal,
    /// The signatures of the cell's live grants, in strategy order. Their
    /// gross sums to at most `amount`; see the module doc.
    live_grants: Vec<String>,
    /// The gross those grants sum to — at most `amount` by construction, and
    /// what the cell will actually derive from the manifest.
    named_gross: Decimal,
}

impl RegionShare {
    pub fn region(&self) -> &str {
        &self.region
    }

    /// The share the partitioner computed from the plan.
    pub fn amount(&self) -> Decimal {
        self.amount
    }

    /// What the cell will sum from the manifest: the gross of the grants it
    /// names, which is at most [`Self::amount`].
    pub fn named_gross(&self) -> Decimal {
        self.named_gross
    }

    pub fn live_grants(&self) -> &[String] {
        &self.live_grants
    }

    /// The slot value the payload carries: the manifest naming this share's
    /// grants and nothing else.
    pub fn manifest(&self) -> GrantManifest {
        GrantManifest {
            live_grants: self.live_grants.clone(),
        }
    }
}

/// Every cell's share under one plan, and every cell that was withheld one.
///
/// A withheld cell is a cell the producer must ship an unproduced slot to,
/// saying why beside it; it is never a cell shipped a guessed share.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct RegionShares {
    shares: BTreeMap<String, RegionShare>,
    withheld: BTreeMap<String, String>,
}

impl RegionShares {
    /// The share for a cell, if one was computed.
    pub fn for_cell(&self, cell: &str) -> Option<&RegionShare> {
        self.shares.get(cell)
    }

    /// Every share, in cell order.
    pub fn shares(&self) -> &BTreeMap<String, RegionShare> {
        &self.shares
    }

    /// Every cell withheld a share, with why, in cell order.
    pub fn withheld(&self) -> &BTreeMap<String, String> {
        &self.withheld
    }

    /// The shares of one region summed — the number the region's grant
    /// bounds.
    pub fn region_total(&self, region: &str) -> Result<Decimal> {
        self.shares
            .values()
            .filter(|share| share.region == region)
            .try_fold(Decimal::ZERO, |total, share| {
                total.checked_add(share.amount).ok_or_else(|| {
                    Error::numeric(format!(
                        "the shares of region {region} cannot be summed past {total}"
                    ))
                })
            })
    }
}

/// Partition a plan into disjoint per-cell shares, refusing a plan whose
/// shares would exceed any region's grant.
///
/// `envelopes` is what the centre holds issued, keyed by cell and strategy in
/// that order so a manifest lists grants in strategy order on every machine.
/// A cell in the plan but in no region is withheld, not given a share of a
/// region it is not in; a cell in a region but absent from the plan gets a
/// share of zero and an empty manifest, which the cell reads as "nothing".
pub fn partition(
    plan: &AllocationPlan,
    membership: &RegionMembership,
    envelopes: &BTreeMap<(String, StrategyId), CapitalEnvelope>,
    now: Timestamp,
) -> Result<RegionShares> {
    // The invariant first, over every region, before any share exists: a
    // plan that over-commits one region is refused whole. Scaling it down
    // instead would ship every cell a number the allocator never produced.
    let mut totals: BTreeMap<&str, Decimal> = BTreeMap::new();
    for (cell, region) in membership.cells() {
        let amount = plan.for_cell(cell);
        if amount.is_negative() {
            return Err(Error::invalid(format!(
                "the plan allocates {amount} to cell {cell}; a negative allocation is the \
                 allocator's accounting gone wrong and is not partitioned"
            )));
        }
        let total = totals.entry(region.as_str()).or_insert(Decimal::ZERO);
        *total = total.checked_add(amount).ok_or_else(|| {
            Error::numeric(format!(
                "the plan's allocations to region {region} cannot be summed past {total}"
            ))
        })?;
    }
    for (region, total) in &totals {
        let grant = membership.grant(region).ok_or_else(|| {
            Error::invalid(format!(
                "region {region} has cells and no grant; the membership was not validated"
            ))
        })?;
        if *total > grant {
            return Err(Error::denied(format!(
                "the plan allocates {total} across the cells of region {region}, past its \
                 grant of {grant}; the plan is refused rather than scaled, because a share the \
                 allocator never produced is a number no envelope was issued against — narrow \
                 the plan or raise the region's grant"
            )));
        }
    }

    let mut shares = RegionShares::default();
    for (cell, region) in membership.cells() {
        let amount = plan.for_cell(cell);
        let mut live_grants = Vec::new();
        let mut named_gross = Decimal::ZERO;
        let mut overflowed = false;
        for envelope in envelopes
            .iter()
            .filter(|((owner, _), _)| owner == cell)
            .map(|(_, envelope)| envelope)
        {
            if !envelope.is_live(now) {
                continue;
            }
            match named_gross.checked_add(envelope.gross_limit()) {
                Some(sum) => named_gross = sum,
                None => {
                    overflowed = true;
                    break;
                }
            }
            live_grants.push(envelope.signature().to_string());
        }
        if overflowed {
            shares.withheld.insert(
                cell.clone(),
                format!(
                    "the live grants issued to {cell} cannot be summed; no manifest ships \
                     until they are renewed"
                ),
            );
            continue;
        }
        if named_gross > amount {
            shares.withheld.insert(
                cell.clone(),
                format!(
                    "the live grants issued to {cell} sum to {named_gross}, past its share of \
                     {amount} under the current plan; no manifest ships until they are renewed \
                     under it, so the cell narrows rather than holding a share the plan did \
                     not produce"
                ),
            );
            continue;
        }
        shares.shares.insert(
            cell.clone(),
            RegionShare {
                region: region.clone(),
                amount,
                live_grants,
                named_gross,
            },
        );
    }
    for allocation in &plan.allocations {
        if membership.region_of(&allocation.cell).is_none() {
            shares
                .withheld
                .entry(allocation.cell.clone())
                .or_insert_with(|| {
                    format!(
                        "cell {} is in no region, so it has no share of any region's grant; file \
                     it under a region or leave it ungranted",
                        allocation.cell
                    )
                });
        }
    }
    Ok(shares)
}

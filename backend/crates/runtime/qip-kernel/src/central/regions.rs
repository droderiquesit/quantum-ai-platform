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
//! # How the shares reach the payload
//!
//! [`CentralPlane::grant_manifests`](super::CentralPlane::grant_manifests)
//! is the producer's one call: it sizes the plan the plane already signs
//! envelopes against, partitions it, and answers for every configured cell
//! with a [`ManifestDecision`] — the manifest to ship in the `capital_grants`
//! slot, or the reason the slot ships unproduced. A cell absent from the
//! membership is withheld with that reason, never given a default region; a
//! plan the partitioner refuses withholds every cell, with the refusal, so a
//! payload never carries a manifest the plan did not produce.
//!
//! # What this deliberately does not decide
//!
//! Where [`RegionMembership`] comes from. The ADR recommends operator-set
//! configuration beside the arbitrage policy and names the alternative —
//! deriving the grant from treasury on hand — as a different number with a
//! different owner. Neither is taken here: membership is an argument to the
//! plane, not a `CentralConfig` field, and [`RegionMembership::parse`] reads
//! it from a committed declaration a composition root already holds, so a
//! root can construct one without this module deciding which configuration
//! that is.

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

    /// Read a membership from a committed declaration.
    ///
    /// The form is `region=grant:cell[,cell…]`, regions separated by `;`,
    /// so that one line beside the cell list a centre already reads names
    /// every region, its grant and its cells, and nothing is inferred from
    /// what a cell claims about itself. Refused: a region named twice, a
    /// cell filed under two regions, a grant that is not a positive decimal,
    /// and every malformed entry — each with what to write instead. A
    /// declaration that parses is validated by [`Self::new`] as well.
    pub fn parse(declaration: &str) -> Result<Self> {
        let mut grants = BTreeMap::new();
        let mut cells = BTreeMap::new();
        for entry in declaration.split(';') {
            let entry = entry.trim();
            if entry.is_empty() {
                continue;
            }
            let Some((region, rest)) = entry.split_once('=') else {
                return Err(Error::invalid(format!(
                    "region declaration `{entry}` is not `region=grant:cell,cell`; every region \
                     names its grant and its cells, so neither can be left to a default"
                )));
            };
            let Some((grant, members)) = rest.split_once(':') else {
                return Err(Error::invalid(format!(
                    "region declaration `{entry}` names no cells; write `region=grant:cell,cell`, \
                     or leave the region out rather than granting it to nobody"
                )));
            };
            let region = region.trim();
            let Some(amount) = Decimal::parse(grant.trim()) else {
                return Err(Error::invalid(format!(
                    "region {region} is granted `{}`, which is not a decimal amount; write the \
                     grant as digits with an optional fraction",
                    grant.trim()
                )));
            };
            if grants.insert(region.to_string(), amount).is_some() {
                return Err(Error::invalid(format!(
                    "region {region} is declared twice; one region has one grant, so merge the \
                     two entries"
                )));
            }
            for cell in members.split(',') {
                let cell = cell.trim();
                if cell.is_empty() {
                    return Err(Error::invalid(format!(
                        "region {region} lists an empty cell name; remove the stray comma"
                    )));
                }
                if let Some(elsewhere) = cells.insert(cell.to_string(), region.to_string()) {
                    return Err(Error::invalid(format!(
                        "cell {cell} is filed under both {elsewhere} and {region}; a cell has one \
                         region, or its share would be a piece of two grants"
                    )));
                }
            }
        }
        Self::new(grants, cells)
    }

    /// Refuse a configured cell this membership does not file anywhere.
    ///
    /// For a composition root to call with the cells it serves: a cell the
    /// centre ships payloads to but has no region for would be withheld
    /// every share, silently, for as long as the deployment ran. Refusing
    /// at start names the cell and what to write.
    pub fn covering<'a>(&self, cells: impl IntoIterator<Item = &'a str>) -> Result<()> {
        let missing: Vec<&str> = cells
            .into_iter()
            .filter(|cell| !self.cells.contains_key(*cell))
            .collect();
        if missing.is_empty() {
            return Ok(());
        }
        Err(Error::invalid(format!(
            "the region membership files no region for {}; add each to a region's cell list, \
             or remove it from the cells this centre serves, rather than leaving it a cell \
             that is shipped no share",
            missing.join(", ")
        )))
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

/// What the `capital_grants` slot of one cell's payload carries.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ManifestDecision {
    /// Ship the share's manifest, produced.
    Ship(RegionShare),
    /// Ship the slot unproduced, and say why beside the payload.
    Withhold(String),
}

impl ManifestDecision {
    /// The manifest to produce, or `None` for a slot that ships unproduced.
    pub fn manifest(&self) -> Option<GrantManifest> {
        match self {
            Self::Ship(share) => Some(share.manifest()),
            Self::Withhold(_) => None,
        }
    }

    /// One line for the producer's account, in the form the whitelist
    /// lines already take.
    pub fn describe(&self, cell: &str) -> String {
        match self {
            Self::Ship(share) => format!(
                "region share for {cell}: {} of {}'s grant, {} grant(s) named summing to {}",
                share.amount(),
                share.region(),
                share.live_grants().len(),
                share.named_gross()
            ),
            Self::Withhold(reason) => format!("region share for {cell}: not shipped, {reason}"),
        }
    }
}

/// Every configured cell's manifest decision under one plan, in cell order.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct GrantManifests {
    decisions: BTreeMap<String, ManifestDecision>,
}

impl GrantManifests {
    /// One decision per configured cell: a share from the partition, or the
    /// reason it was withheld. A refused plan withholds every cell with the
    /// refusal; a cell the partition neither shared nor withheld — one in no
    /// region and in no allocation — is withheld with that.
    pub fn decide<'a>(
        cells: impl IntoIterator<Item = &'a str>,
        partitioned: Result<RegionShares>,
    ) -> Self {
        let mut decisions = BTreeMap::new();
        match partitioned {
            Ok(shares) => {
                for cell in cells {
                    let decision = match (shares.for_cell(cell), shares.withheld().get(cell)) {
                        (Some(share), _) => ManifestDecision::Ship(share.clone()),
                        (None, Some(reason)) => ManifestDecision::Withhold(reason.clone()),
                        (None, None) => ManifestDecision::Withhold(format!(
                            "cell {cell} is in no region, so it has no share of any region's \
                             grant; file it under a region or leave it ungranted"
                        )),
                    };
                    decisions.insert(cell.to_string(), decision);
                }
            }
            Err(refusal) => {
                for cell in cells {
                    decisions.insert(
                        cell.to_string(),
                        ManifestDecision::Withhold(format!(
                            "the plan could not be partitioned, {}",
                            refusal.message()
                        )),
                    );
                }
            }
        }
        Self { decisions }
    }

    /// The decision for a cell, if it was among those decided for.
    pub fn for_cell(&self, cell: &str) -> Option<&ManifestDecision> {
        self.decisions.get(cell)
    }

    /// Every decision, in cell order.
    pub fn decisions(&self) -> &BTreeMap<String, ManifestDecision> {
        &self.decisions
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

#[cfg(test)]
// In a test the assertion is the deliverable; the workspace denies
// `panic_in_result_fn` for production code, where it would be a bug.
#[allow(clippy::panic_in_result_fn)]
mod tests {
    use super::*;
    use qip_core::dec;

    #[test]
    fn a_membership_is_read_from_a_committed_declaration_and_refuses_what_it_cannot_file()
    -> Result<()> {
        // The good declaration first, so the refusals below are known to be
        // refusals of the value and not of everything.
        let membership =
            RegionMembership::parse(" europe-west2 = 1000 : lon-1 , lon-2 ; us-east4=500:nyc-1 ;")?;
        assert_eq!(membership.grant("europe-west2"), Some(dec!("1000")));
        assert_eq!(membership.grant("us-east4"), Some(dec!("500")));
        assert_eq!(membership.region_of("lon-2"), Some("europe-west2"));
        assert_eq!(membership.region_of("nyc-1"), Some("us-east4"));
        assert_eq!(membership.cells().len(), 3, "a cell was dropped");
        membership.covering(["lon-1", "nyc-1"])?;

        // A configured cell the membership does not file is refused by name,
        // never given a region by default.
        let uncovered = match membership.covering(["lon-1", "tokyo-1"]) {
            Ok(()) => panic!("a cell in no region was accepted as covered"),
            Err(error) => error,
        };
        assert!(
            uncovered.message().contains("tokyo-1") && !uncovered.message().contains("lon-1"),
            "the refusal did not name exactly the uncovered cell: {}",
            uncovered.message()
        );

        for (declaration, expected) in [
            ("europe-west2:lon-1", "not `region=grant:cell,cell`"),
            ("europe-west2=1000", "names no cells"),
            ("europe-west2=lots:lon-1", "not a decimal amount"),
            ("europe-west2=0:lon-1", "positive"),
            (
                "europe-west2=1000:lon-1;europe-west2=5:lon-2",
                "declared twice",
            ),
            (
                "europe-west2=1000:lon-1;us-east4=5:lon-1",
                "filed under both",
            ),
            ("europe-west2=1000:lon-1,,lon-2", "empty cell name"),
        ] {
            let error = match RegionMembership::parse(declaration) {
                Ok(membership) => panic!("`{declaration}` was admitted as {membership:?}"),
                Err(error) => error,
            };
            assert!(
                error.message().contains(expected),
                "`{declaration}` was refused for another reason than `{expected}`: {}",
                error.message()
            );
        }
        Ok(())
    }

    #[test]
    fn a_refused_plan_withholds_every_cell_with_the_refusal_and_no_cell_gets_a_default_region() {
        let refused = GrantManifests::decide(
            ["lon-1", "lon-2"],
            Err(Error::denied(
                "the plan allocates 900 past its grant of 800",
            )),
        );
        assert_eq!(refused.decisions().len(), 2, "a cell was left undecided");
        for cell in ["lon-1", "lon-2"] {
            match refused.for_cell(cell) {
                Some(ManifestDecision::Withhold(reason)) => assert!(
                    reason.contains("could not be partitioned")
                        && reason.contains("past its grant"),
                    "the withholding did not carry the refusal: {reason}"
                ),
                other => panic!("{cell} was not withheld on a refused plan: {other:?}"),
            }
            assert!(
                refused
                    .for_cell(cell)
                    .is_some_and(|decision| decision.manifest().is_none()),
                "a withheld cell was given a manifest"
            );
        }

        // A partition that shared nothing and withheld nothing for a cell —
        // one in no region and in no allocation — is still a decision, and
        // it is a withholding, not a share of some region by default.
        let empty = GrantManifests::decide(["ghost-1"], Ok(RegionShares::default()));
        match empty.for_cell("ghost-1") {
            Some(ManifestDecision::Withhold(reason)) => {
                assert!(reason.contains("in no region"), "{reason}");
            }
            other => panic!("a cell in no region was decided as {other:?}"),
        }
    }
}

//! The producer of the shipping payload's causal digest — blueprint §8.3's
//! "compact digest of the relationships that currently matter" — as the
//! centre's own graph can honestly state it.
//!
//! # Why this slot, and why now
//!
//! [`crate::central::whitelist`]'s audit grouped this slot with the belief
//! priors and the episodic digest as "the kernel holds no belief engine, no
//! episodic store, and no causal edge". The other two stopped holding and
//! gained producers ([`super::belief`], [`super::episodic`]). The causal
//! third stopped holding under ADR 0054, when `Platform::discover_temporal_precedence`
//! began writing an edge from `stage_understand` whenever an instrument pair
//! cleared its bar; the audit's own bullet dated that and named what a
//! producer would then owe: "a digest producer that states its own
//! freshness rather than stamping a fresh instant on whatever the graph
//! happens to hold". This module is that producer, and nothing else.
//!
//! # What producing this slot does to a cell, said plainly
//!
//! It widens it, twice. `PolicyItem::CausalDigest` maps to
//! [`qip_contracts::degradation::Capability::CausalGraph`]; a fresh row 2
//! stops `DegradationState::sizing_multiplier` applying its causal-stale
//! factor, and moves `DegradationState::allocation_mode` from unconditional
//! to regime-conditional. So a slot produced on a graph that holds nothing,
//! or stamped with an instant the graph never absorbed evidence at, would
//! enlarge every receiving cell against a signed payload it cannot doubt.
//! Three refusals keep that from happening, each structural:
//!
//! * **A graph that has never absorbed a claim produces nothing at all.**
//!   Not an empty list: the slot ships unproduced and the cell narrows as
//!   every deployed cell does today. The gate is `CausalGraph::last_updated`,
//!   which is the same fact `Platform::central_degradation` reads for the
//!   centre's own §6.2 row 2, so the centre and the cell cannot disagree
//!   about whether a graph exists.
//! * **The slot is stamped with the graph's own newest absorption**, never
//!   with the issue instant. This is deliberately *not* [`super::belief`]'s
//!   rule — that map stamps its oldest member, because each prior is an
//!   independent belief and one instant asserted over many facts may
//!   overclaim none of them. A causal graph is judged on one instant by
//!   contract: `qip_contracts::degradation::CausalGraphFreshness::assess`
//!   reads `last_updated` and nothing else, and `CausalGraph::add` documents
//!   that instant as "the fact §6.2 row 2 is judged on". A digest stamped
//!   with any other instant would make the cell's row 2 and the centre's row
//!   2 two readings of one fact, and the louder one sizes the order.
//! * **A graph whose record contradicts itself is refused, not shipped.**
//!   An edge recorded after the graph's own `last_updated` cannot exist —
//!   `CausalGraph::add` moves that instant forward on every absorption — so
//!   one arriving here means two records of one fact have drifted. Same for
//!   a `last_updated` after the issue instant: a clock fault, named where it
//!   happened rather than stamped onto a signed wire.
//!
//! # What "active" means, and what it does not yet
//!
//! An edge is carried unless `CausalGraph::reestimate` has marked it decayed
//! — found no supporting claim inside the horizon on its last pass. That is
//! the only notion of "currently matters" the graph itself records, and this
//! module invents no second one: no confidence floor, no age cut, no
//! standing filter, because each would be a policy number chosen here and
//! read nowhere else. Say the honest limit out loud: `WorldModel::absorb_causal_support`
//! is the only caller of `reestimate` and it has no production caller, so on
//! a deployed platform no edge is ever decayed and the filter is structurally
//! right while doing nothing. When re-estimation gains a caller the digest
//! narrows with it and this module does not change.
//!
//! # The two horizons, named rather than reconciled
//!
//! The centre judges row 2 against `CAUSAL_GRAPH_HORIZON` (a quarter); the
//! cell judges the slot against `PolicyItem::CausalDigest::time_to_live()`
//! (a day). Stamped with `last_updated`, a graph whose newest edge is three
//! days old reads fresh at the centre and stale at the cell. That is a
//! disagreement, and it is left standing because it falls the safe way — a
//! cell narrows on a graph the centre would trust — and because the day is
//! the contract's number, not this producer's. Stamping `now` to close the
//! gap is exactly the overclaim the audit warned against.
//!
//! # What no cell reads
//!
//! `active_edges` is a manifest, in the same sense
//! [`qip_contracts::policy::GrantManifest`] is one: a sorted list of
//! `cause->effect:mechanism` keys, not a delivery path. No cell traverses it,
//! propagates along it or sizes on its contents; what a cell reads is the
//! slot's *freshness*, and that is the only behaviour producing it changes.
//! The list is here so an operator can reconcile what a cell was told against
//! what the centre held, and so a digest over "nothing" cannot be mistaken
//! for a digest over something.

use std::collections::BTreeSet;

use qip_contracts::policy::{CausalDigest, Slot};
use qip_core::error::{Error, Result};
use qip_core::time::Timestamp;
use qip_events::{EventBody, Topic};
use qip_world_model::causal::CausalEdge;
use serde::{Deserialize, Serialize};

/// Why a cell's causal slot carries what it carries.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CausalOutcome {
    /// The graph has never absorbed a claim, so there is nothing to state.
    /// `held` is how many edges the graph holds regardless — zero on every
    /// production path, and reported so a graph holding edges it never
    /// absorbed reads as the fault it is rather than as an empty graph.
    NeverAbsorbed { held: usize },
    /// The graph has absorbed claims and every edge it holds has since been
    /// found unsupported. Nothing currently matters, and saying so with an
    /// empty produced list would read at a cell as a fresh graph.
    NothingActive {
        held: usize,
        decayed: usize,
        last_updated: Timestamp,
    },
    /// A digest over the edges not marked decayed, current as of the graph's
    /// newest absorption.
    Produced {
        active: u64,
        decayed: usize,
        last_updated: Timestamp,
    },
}

/// One cycle's causal digest, and why — the record the journal keeps.
///
/// One per issue rather than one per cell: the graph is the platform's, so
/// every cell in a cycle receives the same digest, and journaling it per cell
/// would be seven records of one fact.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CausalIssue {
    pub issued_at: Timestamp,
    /// The digest and the instant it is current as of, together or not at
    /// all. Private because the pair is the invariant: a digest that could be
    /// stamped with an instant from anywhere else is the overclaim this
    /// module exists to refuse.
    produced: Option<(CausalDigest, Timestamp)>,
    pub outcome: CausalOutcome,
}

impl CausalIssue {
    /// Derive the digest from the edges the centre's graph holds at `now`.
    ///
    /// `edges` is `CausalGraph::edges()` and `last_updated` is
    /// `CausalGraph::last_updated()`. Two arguments rather than the graph
    /// itself so that this module needs nothing of the world model but the
    /// two facts, and so a test can drive the drifted and the future cases
    /// the graph's own writers will not produce.
    pub fn derive<'a>(
        edges: impl Iterator<Item = &'a CausalEdge>,
        last_updated: Option<Timestamp>,
        now: Timestamp,
    ) -> Result<Self> {
        let edges: Vec<&CausalEdge> = edges.collect();
        let Some(last_updated) = last_updated else {
            return Ok(Self {
                issued_at: now,
                produced: None,
                outcome: CausalOutcome::NeverAbsorbed { held: edges.len() },
            });
        };
        if last_updated > now {
            return Err(Error::invalid(format!(
                "the causal graph reports its newest absorption at {last_updated:?}, after the \
                 issue instant {now:?}; a claim cannot be absorbed in the future, so this is a \
                 clock fault rather than a digest to stamp"
            )));
        }

        let mut active: BTreeSet<String> = BTreeSet::new();
        let mut decayed = 0usize;
        for edge in &edges {
            if edge.recorded_at > last_updated {
                return Err(Error::invalid(format!(
                    "causal edge {}->{} is recorded at {:?} but the graph's newest absorption \
                     is {last_updated:?}; two records of one fact have drifted, and the newer \
                     is not evidence that the older is wrong",
                    edge.cause, edge.effect, edge.recorded_at
                )));
            }
            if edge.is_decayed() {
                decayed += 1;
                continue;
            }
            active.insert(format!(
                "{}->{}:{}",
                edge.cause,
                edge.effect,
                edge.mechanism.as_str()
            ));
        }

        if active.is_empty() {
            return Ok(Self {
                issued_at: now,
                produced: None,
                outcome: CausalOutcome::NothingActive {
                    held: edges.len(),
                    decayed,
                    last_updated,
                },
            });
        }

        Ok(Self {
            issued_at: now,
            outcome: CausalOutcome::Produced {
                active: active.len() as u64,
                decayed,
                last_updated,
            },
            produced: Some((
                CausalDigest {
                    active_edges: active.into_iter().collect(),
                },
                last_updated,
            )),
        })
    }

    /// The payload slot this issue ships, produced or not.
    ///
    /// The only route from here into a payload. A produced slot carries the
    /// graph's newest absorption, so the cell's §6.2 row 2 goes stale on the
    /// graph's silence rather than on the shipper's.
    pub fn slot(&self) -> Slot<CausalDigest> {
        match &self.produced {
            Some((digest, produced_at)) => Slot::produced(digest.clone(), *produced_at),
            None => Slot::unproduced(),
        }
    }

    /// The digest, where one was produced.
    pub fn digest(&self) -> Option<&CausalDigest> {
        self.produced.as_ref().map(|(digest, _)| digest)
    }

    /// The line an operator reads.
    pub fn describe(&self) -> String {
        match &self.outcome {
            CausalOutcome::NeverAbsorbed { held } => format!(
                "causal digest: not shipped, the causal graph has absorbed no claim in this \
                 process ({held} edge(s) held); every cell reads the slot unavailable, narrows \
                 by the causal-stale factor and allocates unconditionally"
            ),
            CausalOutcome::NothingActive {
                held,
                decayed,
                last_updated,
            } => format!(
                "causal digest: not shipped, all {held} edge(s) are decayed ({decayed} found \
                 unsupported on re-estimation, newest absorption {last_updated:?}); every cell \
                 reads the slot unavailable, narrows by the causal-stale factor and allocates \
                 unconditionally"
            ),
            CausalOutcome::Produced {
                active,
                decayed,
                last_updated,
            } => format!(
                "causal digest: {active} active edge(s), newest absorption at {last_updated:?}, \
                 which is the instant the slot's freshness is measured from; {decayed} decayed \
                 edge(s) left out"
            ),
        }
    }
}

impl EventBody for CausalIssue {
    // What the centre distributed as policy, recorded whether or not it was
    // anything: a graph that never reaches a cell is exactly the fact an
    // operator asking why every region allocates unconditionally has to find.
    const TOPIC: Topic = Topic::PolicyDistributed;
    const SCHEMA_VERSION: u32 = 1;
}

#[cfg(test)]
mod tests {
    use super::*;
    use qip_contracts::degradation::{AllocationMode, Capability, Freshness};
    use qip_contracts::policy::{PolicyItem, PolicyPayload};
    use qip_core::time::Duration;
    use qip_world_model::causal::Mechanism;

    fn at(secs: i64) -> Timestamp {
        Timestamp::from_secs(secs)
    }

    fn edge(cause: &str, effect: &str, recorded_at: Timestamp) -> CausalEdge {
        CausalEdge::new(
            cause,
            effect,
            Mechanism::SupplyChain,
            0.5,
            Duration::from_days(1),
            recorded_at,
        )
        .expect("a valid edge")
    }

    #[test]
    fn a_graph_that_has_absorbed_nothing_produces_no_slot_and_says_so() {
        // The state every deployed centre has shipped so far. An empty
        // produced list would read at a cell as a fresh graph over nothing.
        let issue =
            CausalIssue::derive([].iter(), None, at(10_000)).expect("nothing is not an error");
        assert_eq!(issue.slot(), Slot::unproduced());
        assert_eq!(issue.outcome, CausalOutcome::NeverAbsorbed { held: 0 });
        assert!(
            issue.describe().contains("not shipped"),
            "{}",
            issue.describe()
        );
    }

    #[test]
    fn the_slot_is_stamped_with_the_graphs_newest_absorption_and_never_the_issue_instant() {
        // The whole safety argument: stamped `now`, a graph that absorbed
        // its last claim last quarter would keep every cell regime-
        // conditional for as long as payloads kept being issued.
        let older = edge("AAA", "BBB", at(9_000));
        let newer = edge("CCC", "DDD", at(9_500));
        let edges = [newer, older];
        let issued_at = at(10_000);
        let issue = CausalIssue::derive(edges.iter(), Some(at(9_500)), issued_at)
            .expect("two evidenced edges");
        assert_eq!(issue.slot().produced_at(), Some(at(9_500)));
        assert_ne!(issue.slot().produced_at(), Some(issued_at));
        let digest = issue.digest().expect("produced");
        // Sorted on the key rather than left in the graph's order: the list
        // reaches a signed payload, and a replay that reorders is not one.
        assert_eq!(
            digest.active_edges,
            vec!["AAA->BBB:supply_chain", "CCC->DDD:supply_chain"]
        );
        assert_eq!(
            issue.outcome,
            CausalOutcome::Produced {
                active: 2,
                decayed: 0,
                last_updated: at(9_500),
            }
        );
    }

    #[test]
    fn a_decayed_edge_is_left_out_and_a_graph_of_only_decayed_edges_ships_nothing() {
        // "Currently matters" is the graph's own mark and nothing invented
        // here. A graph whose every edge was found unsupported has absorbed
        // claims and holds no relationship that matters — and an empty
        // produced list for it would read at a cell as a fresh graph.
        let mut decayed = edge("AAA", "BBB", at(9_000));
        decayed.decayed_at = Some(at(9_400));
        let live = edge("CCC", "DDD", at(9_500));
        let mixed = [decayed.clone(), live];
        let issue =
            CausalIssue::derive(mixed.iter(), Some(at(9_500)), at(10_000)).expect("one live edge");
        assert_eq!(
            issue.digest().expect("produced").active_edges,
            vec!["CCC->DDD:supply_chain"]
        );
        assert_eq!(
            issue.outcome,
            CausalOutcome::Produced {
                active: 1,
                decayed: 1,
                last_updated: at(9_500),
            }
        );

        let only_decayed = [decayed];
        let none = CausalIssue::derive(only_decayed.iter(), Some(at(9_400)), at(10_000))
            .expect("an all-decayed graph is not an error");
        assert_eq!(none.slot(), Slot::unproduced());
        assert_eq!(
            none.outcome,
            CausalOutcome::NothingActive {
                held: 1,
                decayed: 1,
                last_updated: at(9_400),
            }
        );
    }

    #[test]
    fn a_graph_whose_record_contradicts_itself_or_its_clock_is_refused_rather_than_stamped() {
        // Both unreachable through `CausalGraph::add`, which moves
        // `last_updated` forward on every absorption; both refused rather
        // than repaired, because a digest whose age cannot be accounted for
        // is one nobody can audit.
        let drifted = [edge("AAA", "BBB", at(9_990))];
        let error = CausalIssue::derive(drifted.iter(), Some(at(9_900)), at(10_000))
            .expect_err("an edge newer than the graph's own record is drift");
        assert!(error.to_string().contains("have drifted"), "{error}");

        let future = [edge("AAA", "BBB", at(9_900))];
        let error = CausalIssue::derive(future.iter(), Some(at(10_001)), at(10_000))
            .expect_err("a graph updated after the issue instant is a clock fault");
        assert!(error.to_string().contains("clock fault"), "{error}");
    }

    #[test]
    fn a_fresh_digest_is_what_lifts_a_cells_causal_narrowing_and_silence_restores_it() {
        // What producing this slot actually changes at a cell, asserted
        // through the payload rather than described in a comment — and the
        // premise first, because a test that only asserted the fresh case
        // would pass on a payload that never narrows anything.
        let issued_at = at(10_000);
        let unproduced = PolicyPayload::unproduced(1, "cell-1", issued_at);
        let narrowed = unproduced.narrowing(issued_at);
        assert_eq!(
            narrowed.freshness(Capability::CausalGraph),
            Freshness::Unavailable,
            "premise: an unproduced causal slot must read unavailable, or this test proves nothing"
        );
        assert_eq!(narrowed.allocation_mode(), AllocationMode::Unconditional);
        let floor = narrowed.sizing_multiplier();

        let edges = [edge("AAA", "BBB", at(9_940))];
        let issue = CausalIssue::derive(edges.iter(), Some(at(9_940)), issued_at)
            .expect("one edge a minute old");
        let mut payload = PolicyPayload::unproduced(2, "cell-1", issued_at);
        payload.causal_digest = issue.slot();
        let widened = payload.narrowing(issued_at);
        assert_eq!(widened.freshness(Capability::CausalGraph), Freshness::Fresh);
        assert_eq!(
            widened.allocation_mode(),
            AllocationMode::RegimeConditional,
            "a fresh row 2 must move allocation to regime-conditional, which is one of the two \
             things this slot is worth producing for and the reason it must not be produced on \
             a guess"
        );
        assert!(
            widened.sizing_multiplier() > floor,
            "a fresh row 2 must lift the causal-stale factor: {} is not above {floor}",
            widened.sizing_multiplier()
        );

        // Past the slot's own time to live the same payload narrows again
        // without anything being republished: the widening expires on the
        // graph's silence, not the shipper's.
        let later = issued_at
            .saturating_add(PolicyItem::CausalDigest.time_to_live())
            .saturating_add(Duration::from_secs(60));
        assert_eq!(
            payload.narrowing(later).sizing_multiplier(),
            floor,
            "a digest older than its time to live must stop excusing the widening"
        );
        assert_eq!(
            payload.narrowing(later).allocation_mode(),
            AllocationMode::Unconditional
        );
    }
}

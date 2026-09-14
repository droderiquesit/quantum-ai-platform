//! The adapter that lets blueprint §30.2's router read a cycle the
//! arbitrage search actually found.
//!
//! [`crate::path`] holds the vocabulary and the decision; nothing in it
//! knows about [`qip_arbitrage`]. This module is the one seam between the
//! two, and it exists so that a caller holding a
//! [`qip_arbitrage::search::PathCandidate`] can route it in one line rather
//! than restating a cycle it already has. §31.1 and §33.1 can use
//! [`crate::path`] alone; only a caller sitting on the graph needs this.
//!
//! # The two facts the graph does not hold, and why they are supplied
//!
//! * **Which region a venue is in.** `EdgeKind::Transfer` is "the same
//!   instrument moved between venues" and says nothing about regions, but
//!   §30's edge table splits exactly that case in two: within one region it
//!   is a transport edge and across two it is a mirror edge, and the
//!   difference decides between path 2 and paths 3 to 6. Guessing would mean
//!   routing a New-York-to-London hop as though it completed in one process.
//!   [`VenueRegions`] is therefore supplied, and a venue it does not name is
//!   **refused**, never assumed local — the same fail-closed reading
//!   `ArbitrageGraph::register_venue` gives an unregistered venue.
//! * **What a synthetic edge is.** `EdgeKind::Synthetic` covers both of
//!   §30's remaining classes: a future against spot is a *basis* edge and an
//!   options box against its cash value is an *equivalence* edge. A graph of
//!   rates cannot tell them apart, because the difference is what the two
//!   instruments *are* to each other. [`RepresentationClasses`] is supplied
//!   from the same policy that whitelisted the synthetic, and an unnamed
//!   synthetic is refused rather than defaulted to either — defaulting to
//!   basis would route an options structure with no Greeks gate.
//!
//! # What it cannot do
//!
//! It can never produce a settlement edge, because `EdgeKind` has no
//! settlement arm. [`crate::path::EdgeClass::Settlement`] is reachable only
//! from a hand-built [`crate::path::Composition`]; that is stated here so the
//! next reader does not conclude the refusal is dead code. It is reachable,
//! and `crate::path`'s own tests reach it.
//!
//! Nothing here sends, prices or sizes anything. It reads a graph and
//! returns a classification.

use crate::path::{
    Composition, CompositionEdge, EdgeClass, MirrorFacts, PathAssignment, PathEndpoint, PathPolicy,
    RegionId, assign,
};
use qip_arbitrage::graph::{ArbitrageGraph, EdgeKind};
use qip_contracts::venue::VenueId;
use qip_core::ObjectId;
use qip_core::error::{Error, Result};
use std::collections::BTreeMap;

/// Which region each venue sits in.
///
/// A `BTreeMap` rather than a `HashMap` because the refusal messages below
/// list what *is* known, and a list that reorders between runs is a log two
/// replays disagree about.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct VenueRegions {
    by_venue: BTreeMap<VenueId, RegionId>,
}

impl VenueRegions {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record one venue's region. A second recording for the same venue
    /// replaces the first; the map is configuration, not an accumulation.
    pub fn with(mut self, venue: VenueId, region: RegionId) -> Self {
        self.by_venue.insert(venue, region);
        self
    }

    /// Every named venue in one region: what a cell that has only been told
    /// its own region can honestly say.
    ///
    /// This is the *whole* map, not a default applied to anything else. A
    /// venue absent from `venues` is still refused by [`Self::region_of`],
    /// which is the point: a cell built this way routes its own region's
    /// cycles and refuses one that reaches a venue nobody placed, rather
    /// than assuming the venue is local because the cell is.
    ///
    /// Refused when `venues` is empty. A map naming no venue can route no
    /// cycle, and a caller that built one has a configuration problem the
    /// first refused cycle would report as a venue problem instead.
    pub fn all_in(region: RegionId, venues: &[VenueId]) -> Result<Self> {
        if venues.is_empty() {
            return Err(Error::invalid(format!(
                "no venue was named for region {}, so every cycle would be refused for an \
                 unrecorded venue; name the venues this cell may trade at",
                region.as_str()
            )));
        }
        let mut map = Self::new();
        for venue in venues {
            map = map.with(venue.clone(), region.clone());
        }
        Ok(map)
    }

    pub fn len(&self) -> usize {
        self.by_venue.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_venue.is_empty()
    }

    /// The venue's region, or a refusal naming it.
    ///
    /// Not `Option`, and not a default to the local region. A venue whose
    /// region nobody recorded would be routed as though it were next door,
    /// and the cycle would be assigned path 2 — parallel dispatch inside one
    /// process — for a leg that has to cross an ocean.
    pub fn region_of(&self, venue: &VenueId) -> Result<&RegionId> {
        self.by_venue.get(venue).ok_or_else(|| {
            let known: Vec<&str> = self.by_venue.keys().map(VenueId::as_str).collect();
            Error::not_found(format!(
                "venue {} has no region recorded, and the router tells a transport edge from a \
                 mirror edge by comparing regions; record its region beside the {} already \
                 known [{}], or do not route a cycle through it",
                venue.as_str(),
                self.by_venue.len(),
                known.join(", ")
            ))
        })
    }
}

/// Which of §30's two representation-crossing classes each synthetic is.
///
/// Keyed by the synthetic's own instrument — `EdgeKind::Synthetic`'s
/// `synthetic_object` — because that is the thing whose nature the class is
/// a statement about. The components are what pays for it and can differ per
/// edge.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RepresentationClasses {
    by_object: BTreeMap<ObjectId, EdgeClass>,
}

impl RepresentationClasses {
    pub fn new() -> Self {
        Self::default()
    }

    /// This synthetic is another representation of an underlying the cycle
    /// also holds — spot against perpetual, future, tokenised or ETF form.
    /// §30.2's row 7.
    pub fn basis(mut self, object: ObjectId) -> Self {
        self.by_object.insert(object, EdgeClass::Basis);
        self
    }

    /// This synthetic is a payoff structure replicated by its components —
    /// parity, a box, a conversion. §30.2's row 8, whose extension is a
    /// Greeks gate.
    pub fn equivalence(mut self, object: ObjectId) -> Self {
        self.by_object.insert(object, EdgeClass::Equivalence);
        self
    }

    pub fn len(&self) -> usize {
        self.by_object.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_object.is_empty()
    }

    /// The class, or a refusal. There is no default: basis and equivalence
    /// carry different risks and different §33.1 extensions, and picking one
    /// for a synthetic nobody classified would route an options structure
    /// under a carry check.
    pub fn class_of(&self, object: &ObjectId) -> Result<EdgeClass> {
        self.by_object.get(object).copied().ok_or_else(|| {
            Error::not_found(format!(
                "synthetic {} is classified as neither a basis nor an equivalence edge; the two \
                 are routed to different paths and gated on different things, so classify it \
                 rather than letting the router pick",
                object.as_str()
            ))
        })
    }
}

/// Routes a found cycle to one of §30.2's eight paths.
///
/// Holds the three things that do not change per cycle — the policy, the
/// venue regions and the synthetic classes — so the per-cycle call is one
/// line. Built once where the desk is installed; a caller that rebuilt it
/// per pass would be re-reading configuration on the hot path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CycleRouter {
    policy: PathPolicy,
    regions: VenueRegions,
    representations: RepresentationClasses,
}

impl CycleRouter {
    pub fn new(
        policy: PathPolicy,
        regions: VenueRegions,
        representations: RepresentationClasses,
    ) -> Self {
        Self {
            policy,
            regions,
            representations,
        }
    }

    pub fn policy(&self) -> &PathPolicy {
        &self.policy
    }

    pub fn regions(&self) -> &VenueRegions {
        &self.regions
    }

    /// Turn a cycle's edge indices into the composition §30.2 reads.
    ///
    /// `edges` is `PathCandidate::edges` — "edge indices in traversal order,
    /// the last edge arrives where the first departs". An index naming no
    /// edge is refused rather than skipped: skipping it would silently
    /// shorten the cycle and the closure check would then fail somewhere
    /// else, naming the wrong problem.
    pub fn composition(&self, graph: &ArbitrageGraph, edges: &[usize]) -> Result<Composition> {
        let mut composed = Vec::with_capacity(edges.len());
        for index in edges {
            let edge = graph.edge(*index).ok_or_else(|| {
                Error::not_found(format!(
                    "cycle names edge {index} and the graph holds {}; route the cycle against \
                     the graph it was found in",
                    graph.edge_count()
                ))
            })?;
            let from = PathEndpoint::new(
                edge.from.object.clone(),
                edge.from.venue.clone(),
                self.regions.region_of(&edge.from.venue)?.clone(),
            );
            let to = PathEndpoint::new(
                edge.to.object.clone(),
                edge.to.venue.clone(),
                self.regions.region_of(&edge.to.venue)?.clone(),
            );
            let class = match &edge.kind {
                // A trade is two assets against one book at one venue, which
                // is §30's conversion edge exactly. The graph already refuses
                // a trade that spans two venues.
                EdgeKind::Trade { .. } => EdgeClass::Conversion,
                // The one place the region map earns its keep.
                EdgeKind::Transfer => {
                    if from.region == to.region {
                        EdgeClass::Transport
                    } else {
                        EdgeClass::Mirror
                    }
                }
                EdgeKind::Synthetic {
                    synthetic_object, ..
                } => self.representations.class_of(synthetic_object)?,
            };
            composed.push(CompositionEdge::new(class, from, to)?);
        }
        Composition::cycle(composed)
    }

    /// The one-line call: a cycle in, an assignment or a refusal out.
    pub fn route(
        &self,
        graph: &ArbitrageGraph,
        edges: &[usize],
        mirror_facts: &BTreeMap<usize, MirrorFacts>,
    ) -> Result<PathAssignment> {
        let composition = self.composition(graph, edges)?;
        assign(&composition, mirror_facts, &self.policy)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::path::ExecutionPath;
    use qip_arbitrage::graph::Node;
    use qip_contracts::message::BookSide;
    use qip_core::{Decimal, Timestamp};

    fn object(id: &str) -> ObjectId {
        ObjectId::from_string(id)
    }

    fn venue(id: &str) -> VenueId {
        VenueId::new(id)
    }

    fn region(id: &str) -> RegionId {
        RegionId::new(id).expect("a test region id is valid")
    }

    fn node(o: &str, v: &str) -> Node {
        Node::new(object(o), venue(v))
    }

    fn now() -> Timestamp {
        Timestamp::from_secs(1_700_000_000)
    }

    fn rate(value: &str) -> Decimal {
        Decimal::parse(value).expect("a test rate parses")
    }

    /// A two-venue cycle: buy BTC at one venue, move it, sell it at the
    /// other, move the cash back.
    fn two_venue_graph() -> ArbitrageGraph {
        let mut graph = ArbitrageGraph::new();
        graph
            .add_trade(
                node("USD", "XNAS"),
                node("BTC", "XNAS"),
                rate("0.00002"),
                Decimal::ZERO,
                object("BTCUSD"),
                BookSide::Ask,
                now(),
                4,
            )
            .expect("a trade edge");
        graph
            .add_transfer(
                object("BTC"),
                venue("XNAS"),
                venue("XLON"),
                Decimal::ZERO,
                now(),
                4,
            )
            .expect("a transfer edge");
        graph
            .add_trade(
                node("BTC", "XLON"),
                node("USD", "XLON"),
                rate("50100"),
                Decimal::ZERO,
                object("BTCUSD"),
                BookSide::Bid,
                now(),
                4,
            )
            .expect("a trade edge");
        graph
            .add_transfer(
                object("USD"),
                venue("XLON"),
                venue("XNAS"),
                Decimal::ZERO,
                now(),
                4,
            )
            .expect("a transfer edge");
        graph
    }

    fn all_in_one_region() -> VenueRegions {
        VenueRegions::new()
            .with(venue("XNAS"), region("us-east"))
            .with(venue("XLON"), region("us-east"))
    }

    fn split_across_regions() -> VenueRegions {
        VenueRegions::new()
            .with(venue("XNAS"), region("us-east"))
            .with(venue("XLON"), region("eu-west"))
    }

    #[test]
    fn a_transfer_between_two_venues_of_one_region_becomes_a_transport_edge_and_path_two() {
        let graph = two_venue_graph();
        let router = CycleRouter::new(
            PathPolicy::default(),
            all_in_one_region(),
            RepresentationClasses::new(),
        );
        let composition = router
            .composition(&graph, &[0, 1, 2, 3])
            .expect("the cycle composes");
        // Premise: the graph really does carry two transfer edges, or the
        // class assertion below would be about nothing.
        assert_eq!(
            composition
                .edges()
                .iter()
                .filter(|edge| edge.class() == EdgeClass::Transport)
                .count(),
            2
        );
        assert!(composition.mirror_edges().is_empty());
        let assignment = router
            .route(&graph, &[0, 1, 2, 3], &BTreeMap::new())
            .expect("a single-region cycle routes");
        assert_eq!(assignment.assigned(), ExecutionPath::CrossVenue);
    }

    #[test]
    fn the_same_cycle_with_its_venues_in_two_regions_becomes_mirror_edges_and_never_path_two() {
        // The failure this prevents: a New-York-to-London hop routed as
        // path 2, whose mechanism is "latency-equalised parallel dispatch
        // from pinned I/O slots" and whose whole premise is one process. The
        // only thing that changes between this test and the one above is the
        // region map.
        let graph = two_venue_graph();
        let router = CycleRouter::new(
            PathPolicy::default(),
            split_across_regions(),
            RepresentationClasses::new(),
        );
        let composition = router
            .composition(&graph, &[0, 1, 2, 3])
            .expect("the cycle composes");
        assert_eq!(composition.mirror_edges().len(), 2);
        assert!(!composition.classes().contains(&EdgeClass::Transport));

        let facts: BTreeMap<usize, MirrorFacts> = composition
            .mirror_edges()
            .into_iter()
            .map(|index| {
                (
                    index,
                    MirrorFacts::new(
                        true,
                        false,
                        false,
                        None,
                        qip_core::time::Duration::from_millis(28),
                    )
                    .expect("valid facts"),
                )
            })
            .collect();
        let assignment = router
            .route(&graph, &[0, 1, 2, 3], &facts)
            .expect("a cross-region cycle with inventory routes");
        assert_eq!(assignment.assigned(), ExecutionPath::MirroredInventory);
        assert!(!assignment.eligible().contains(&ExecutionPath::CrossVenue));
    }

    #[test]
    fn a_single_region_map_places_every_named_venue_and_still_refuses_one_it_was_not_given() {
        // The shape a cell is built with: it knows its own region and the
        // venues it may trade at, and nothing else. The second half is the
        // half that matters — a convenience constructor that made the whole
        // map permissive would turn "this cell is local" into "every venue
        // is local", which is exactly the assumption `region_of` exists to
        // refuse.
        let map = VenueRegions::all_in(region("us-east"), &[venue("XNAS"), venue("XLON")])
            .expect("two venues in one region is a map");
        assert_eq!(map.len(), 2);
        assert_eq!(
            map.region_of(&venue("XLON")).expect("named"),
            &region("us-east")
        );
        let refusal = map
            .region_of(&venue("XCBO"))
            .expect_err("a venue the map does not name has no region");
        assert_eq!(refusal.code(), "not_found");

        let empty = VenueRegions::all_in(region("us-east"), &[])
            .expect_err("a map naming no venue can route nothing");
        assert_eq!(empty.code(), "invalid");
        assert!(
            empty.message().contains("no venue was named for region"),
            "the refusal should say why: {}",
            empty.message()
        );
    }

    #[test]
    fn a_venue_with_no_recorded_region_is_refused_rather_than_assumed_local() {
        let graph = two_venue_graph();
        let partial = VenueRegions::new().with(venue("XNAS"), region("us-east"));
        // Premise: one venue is known, so this is not an empty-map artefact.
        assert_eq!(partial.len(), 1);
        let router = CycleRouter::new(PathPolicy::default(), partial, RepresentationClasses::new());
        let refusal = router
            .composition(&graph, &[0, 1, 2, 3])
            .expect_err("an unmapped venue cannot be placed in a region");
        assert_eq!(refusal.code(), "not_found");
        assert!(
            refusal.message().contains("XLON has no region recorded"),
            "the refusal should name the venue: {}",
            refusal.message()
        );
    }

    #[test]
    fn an_unclassified_synthetic_is_refused_rather_than_routed_as_a_basis_edge() {
        // Defaulting to basis would put an options structure through the
        // carry check of path 7 instead of the Greeks gate of path 8.
        let mut graph = ArbitrageGraph::new();
        graph
            .add_synthetic(
                node("SPX-BOX", "XCBO"),
                node("USD", "XCBO"),
                rate("1000"),
                Decimal::ZERO,
                object("SPX-BOX"),
                vec![qip_arbitrage::graph::SyntheticComponent {
                    object: object("SPX-CALL"),
                    venue: venue("XCBO"),
                    units_per_unit: Decimal::ONE,
                    unwind_side: BookSide::Bid,
                }],
                now(),
                4,
            )
            .expect("a synthetic edge");
        graph
            .add_trade(
                node("USD", "XCBO"),
                node("SPX-BOX", "XCBO"),
                rate("0.001"),
                Decimal::ZERO,
                object("SPXBOXUSD"),
                BookSide::Ask,
                now(),
                4,
            )
            .expect("a trade edge");
        let router = CycleRouter::new(
            PathPolicy::default(),
            VenueRegions::new().with(venue("XCBO"), region("us-east")),
            RepresentationClasses::new(),
        );
        let refusal = router
            .composition(&graph, &[0, 1])
            .expect_err("an unclassified synthetic cannot be routed");
        assert_eq!(refusal.code(), "not_found");
        assert!(
            refusal
                .message()
                .contains("SPX-BOX is classified as neither"),
            "the refusal should name the synthetic: {}",
            refusal.message()
        );

        // And the same cycle routes once the class is stated — the half that
        // proves the gate admits a good value rather than refusing
        // everything.
        let classified = CycleRouter::new(
            PathPolicy::default(),
            VenueRegions::new().with(venue("XCBO"), region("us-east")),
            RepresentationClasses::new().equivalence(object("SPX-BOX")),
        );
        let assignment = classified
            .route(&graph, &[0, 1], &BTreeMap::new())
            .expect("a classified synthetic routes");
        assert_eq!(assignment.assigned(), ExecutionPath::PayoffEquivalence);
    }

    #[test]
    fn an_edge_index_the_graph_does_not_hold_is_refused_rather_than_skipped() {
        // Skipping would shorten the cycle silently and the closure check
        // would then fail, naming a break that is not the real problem.
        let graph = two_venue_graph();
        let router = CycleRouter::new(
            PathPolicy::default(),
            all_in_one_region(),
            RepresentationClasses::new(),
        );
        let refusal = router
            .composition(&graph, &[0, 1, 2, 99])
            .expect_err("an index the graph does not hold is a caller bug");
        assert_eq!(refusal.code(), "not_found");
        assert!(
            refusal.message().contains("names edge 99"),
            "the refusal should name the index: {}",
            refusal.message()
        );
    }
}

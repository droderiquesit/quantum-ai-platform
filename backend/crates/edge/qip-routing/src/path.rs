//! Blueprint §30.2's path router: which of the eight execution paths a
//! candidate cycle is assigned, decided from the classes of its edges and
//! from facts about the venues those edges touch.
//!
//! # Why this is a separate vocabulary from the arbitrage graph's own
//!
//! `qip_arbitrage::graph::PathKind` names four *shapes* — cross-venue,
//! triangular, cross-instrument, mixed — derived from a cycle so that a
//! rejection message can describe it. It is a label on what was found. The
//! eight paths of §30.2 are something else: an assignment of *how the cycle
//! will be executed*, and each one carries a different coordination
//! mechanism, a different latency budget and a different primary risk
//! (§31). The two vocabularies answer different questions and neither is
//! derivable from the other, which is why this module does not extend
//! `PathKind`. A cross-venue shape is path 2 when its two venues are in one
//! region and path 3, 4, 5 or 6 when they are not, and the graph holds no
//! region.
//!
//! # The six edge classes, and the one with no row
//!
//! §30's edge table names six classes. §30.2's assignment table names five
//! of them. **Settlement has no row**, and a composition containing a
//! settlement edge is therefore refused rather than assigned — see
//! [`EdgeClass::Settlement`] and [`eligible_paths`]. Guessing a path for it
//! would be the platform inventing a coordination policy for a class the
//! blueprint has not specified one for, and a wrong guess here is a leg that
//! fires against a settlement stage nobody planned for.
//!
//! # What this module refuses to be
//!
//! It selects a *path*, never a venue, never an order and never a size. It
//! constructs nothing that can be sent, names no venue class, and has no
//! reference to a gateway: assignment is a classification over a
//! description of a cycle, and every type in this file is inert. Extending
//! it can widen what the platform will *consider*; it cannot widen what the
//! platform can *send*, because nothing here can produce an order.
//!
//! # Designed to be extended by somebody else, and since extended
//!
//! §31.1 (the cross-region solve) is [`crate::mirror`] and §33.1 (path
//! extensions) is [`crate::extension`]; both attach to
//! [`PathAssignment::assigned`], which is what this module produces.
//!
//! The extension did **not** land where this paragraph predicted, and the
//! prediction is left here corrected rather than deleted, because the reason
//! is the load-bearing part. It said §31.1 would refine
//! [`MirrorFacts::both_sides_at_target`] into the four-row direction-gating
//! table. It could not: `both_sides_at_target` is a claim about two regions
//! and an edge cell can measure only one, so refining it in place would have
//! made every cell assert a fact it cannot hold. §31.1's whole construction
//! is that neither side needs the other's holding, so the table went beside
//! this module rather than inside it, and what arrived here instead is
//! [`MirrorFacts::established_mirror`] — a setup fact a cell can honestly
//! state. The direction gating runs *after* assignment, in §33.1's
//! extension, which is where the blueprint puts it.
//!
//! Three decisions exist for their benefit:
//!
//! * [`ExecutionPath`] is **not** `#[non_exhaustive]`. A ninth path is a
//!   compile error in every `match` that dispatches on one, which is the
//!   only way a per-path check can be guaranteed to have considered it. A
//!   path extension silently defaulting to "no extra check" is precisely the
//!   control that reads as protection and cannot fire.
//! * [`MirrorFacts`] has private fields and one fallible constructor, so a
//!   lane that needs a seventh fact adds it without breaking every caller's
//!   struct literal.
//! * Which path *wins* when several are eligible is [`PathPolicy`], supplied
//!   by the caller. §30.2 is explicit that assignment "is a policy decision
//!   made globally with full cost and risk information, not a local
//!   heuristic", so this module holds no opinion it did not receive — only a
//!   documented default.
//!
//! # No money crosses this module
//!
//! There is no `Decimal` here and no `f64` either. Cost and expected profit
//! are the optimiser's, and restating either here would be a second claim
//! about the same fact. The policy's ranking is the one hook by which cost
//! information reaches the assignment.

use qip_contracts::venue::VenueId;
use qip_core::ObjectId;
use qip_core::error::{Error, Result};
use qip_core::time::Duration;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// The most edges one composition may carry.
///
/// §30.1 indexes "every cycle of length 2–4 the whitelist permits" and the
/// background sweep may surface longer ones, so this is not four. It is a
/// bound on the working set rather than a statement about what is
/// profitable: a composition past this is a graph walk that escaped, and
/// routing it would spend the pass budget deciding how to execute something
/// no planner will size.
pub const MAX_COMPOSITION_EDGES: usize = 8;

/// The fewest edges a cycle can have.
///
/// Two: a synthetic against the components that replicate it is the
/// shortest real cycle in this platform's graph. One edge cannot close.
pub const MIN_COMPOSITION_EDGES: usize = 2;

/// A region a venue sits in.
///
/// Opaque and exact, for the reason [`VenueId`] is: `eu-west` and `eu-west `
/// are two different strings and normalising them here would silently merge
/// a typo into a real region, which for a mirror edge means claiming two
/// venues are on opposite sides of an ocean when they are not.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct RegionId(String);

impl RegionId {
    /// Refuse a region id that is empty or carries surrounding whitespace.
    ///
    /// Trimming would be a caller's bug silently corrected, and the caller
    /// whose configuration reader left a newline on the value would keep
    /// shipping it.
    pub fn new(id: impl Into<String>) -> Result<Self> {
        let id = id.into();
        if id.is_empty() {
            return Err(Error::invalid(
                "a region id is empty; name the region the venue sits in, such as the cell's \
                 own region, rather than leaving it blank",
            ));
        }
        if id.trim() != id {
            return Err(Error::invalid(format!(
                "region id {id:?} has surrounding whitespace; two regions whose ids differ only \
                 by a space are two regions, so supply the id without it rather than relying on \
                 this to trim"
            )));
        }
        Ok(Self(id))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// One of §30's six edge classes.
///
/// The class is what the router reads. It is supplied rather than derived
/// because the two classes that distinguish themselves from a trade — basis
/// and equivalence — are statements about what two instruments *are* to each
/// other, and no graph of rates holds that.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum EdgeClass {
    /// Two assets, same venue. The edge every cycle is mostly made of.
    Conversion,
    /// Same asset, two venues, same region.
    Transport,
    /// Same asset, two regions. What makes a cycle cross-region, and the
    /// only class that can change the region a cycle is standing in.
    Mirror,
    /// Two representations of one underlying — spot against perpetual,
    /// future, tokenised or ETF form.
    Basis,
    /// A payoff structure and its replication — parity, boxes, conversions.
    Equivalence,
    /// Same asset, two settlement stages.
    ///
    /// **§30.2 assigns no path to a composition containing one.** The class
    /// exists in §30's edge table and in this enum so that a caller can say
    /// honestly what it holds; [`eligible_paths`] then refuses it rather
    /// than routing it as though the settlement bridge were free. The
    /// alternative — quietly treating it as a conversion — would price a
    /// lagged leg as an instant one.
    Settlement,
}

impl EdgeClass {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Conversion => "conversion",
            Self::Transport => "transport",
            Self::Mirror => "mirror",
            Self::Basis => "basis",
            Self::Equivalence => "equivalence",
            Self::Settlement => "settlement",
        }
    }
}

/// One end of an edge: what is held, where it is held, and in which region.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct PathEndpoint {
    pub object: ObjectId,
    pub venue: VenueId,
    pub region: RegionId,
}

impl PathEndpoint {
    pub const fn new(object: ObjectId, venue: VenueId, region: RegionId) -> Self {
        Self {
            object,
            venue,
            region,
        }
    }

    /// A stable label for refusal messages.
    pub fn label(&self) -> String {
        format!(
            "{}@{}/{}",
            self.object.as_str(),
            self.venue.as_str(),
            self.region.as_str()
        )
    }
}

/// One edge of a candidate cycle, as the router sees it.
///
/// Constructed only through [`CompositionEdge::new`], which refuses an edge
/// whose endpoints contradict its class. That refusal is the reason the
/// router does not re-derive "one venue" or "one region" later: a closed
/// chain of edges each of which holds its venue fixed has exactly one venue,
/// and the constructor is what holds it. A guarantee the constructor keeps
/// beats one the classifier re-checks.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompositionEdge {
    class: EdgeClass,
    from: PathEndpoint,
    to: PathEndpoint,
}

impl CompositionEdge {
    /// Refuse an edge whose endpoints do not match what its class claims.
    ///
    /// A mirror edge inside one region is a transport edge mislabelled, and
    /// a mislabelled mirror is how a cycle that never leaves London gets
    /// assigned path 3 and sized for capital in two regions.
    pub fn new(class: EdgeClass, from: PathEndpoint, to: PathEndpoint) -> Result<Self> {
        let label = format!("{} -> {}", from.label(), to.label());
        match class {
            EdgeClass::Conversion => {
                if from.venue != to.venue {
                    return Err(Error::invalid(format!(
                        "conversion {label} spans two venues; a conversion is two assets at one \
                         venue, so declare the venue hop as a transport or mirror edge of its own"
                    )));
                }
                if from.object == to.object {
                    return Err(Error::invalid(format!(
                        "conversion {label} does not change the asset held; that is a transport, \
                         a mirror or a settlement edge, not a conversion"
                    )));
                }
            }
            EdgeClass::Transport => {
                if from.object != to.object {
                    return Err(Error::invalid(format!(
                        "transport {label} changes the asset held; that is a conversion"
                    )));
                }
                if from.region != to.region {
                    return Err(Error::invalid(format!(
                        "transport {label} leaves the region; same asset in two regions is a \
                         mirror edge, and calling it a transport hides the cross-region capital \
                         it needs"
                    )));
                }
                if from.venue == to.venue {
                    return Err(Error::invalid(format!(
                        "transport {label} does not leave the venue; an edge that moves nothing \
                         is not an edge"
                    )));
                }
            }
            EdgeClass::Mirror => {
                if from.object != to.object {
                    return Err(Error::invalid(format!(
                        "mirror {label} changes the asset held; a mirror edge is one asset held \
                         in two regions"
                    )));
                }
                if from.region == to.region {
                    return Err(Error::invalid(format!(
                        "mirror {label} does not leave the region; within one region the same \
                         asset at two venues is a transport edge"
                    )));
                }
            }
            EdgeClass::Basis | EdgeClass::Equivalence => {
                if from.object == to.object {
                    return Err(Error::invalid(format!(
                        "{} {label} names the same representation at both ends; a {} edge \
                         connects two different representations of one underlying",
                        class.as_str(),
                        class.as_str()
                    )));
                }
            }
            EdgeClass::Settlement => {
                if from.object != to.object {
                    return Err(Error::invalid(format!(
                        "settlement {label} changes the asset held; a settlement edge is one \
                         asset at two settlement stages"
                    )));
                }
            }
        }
        Ok(Self { class, from, to })
    }

    pub const fn class(&self) -> EdgeClass {
        self.class
    }

    pub const fn from(&self) -> &PathEndpoint {
        &self.from
    }

    pub const fn to(&self) -> &PathEndpoint {
        &self.to
    }
}

/// A candidate cycle, described by its edges in traversal order.
///
/// Closed on purpose. An arbitrage is a cycle; a chain that does not return
/// to where it started is a position, and routing a position as though it
/// were a cycle assigns it a coordination policy that assumes the exposure
/// closes itself. The closure check is the one that catches a truncated
/// edge list, which is the realistic way a caller gets this wrong.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Composition {
    edges: Vec<CompositionEdge>,
}

impl Composition {
    /// Refuse anything that is not a closed chain of between
    /// [`MIN_COMPOSITION_EDGES`] and [`MAX_COMPOSITION_EDGES`] edges.
    pub fn cycle(edges: Vec<CompositionEdge>) -> Result<Self> {
        if edges.len() < MIN_COMPOSITION_EDGES {
            return Err(Error::invalid(format!(
                "a composition of {} edge(s) cannot close; a cycle needs at least \
                 {MIN_COMPOSITION_EDGES}, so supply the edges the cycle actually traverses",
                edges.len()
            )));
        }
        if edges.len() > MAX_COMPOSITION_EDGES {
            return Err(Error::invalid(format!(
                "a composition of {} edges exceeds the {MAX_COMPOSITION_EDGES} this router will \
                 assign a path to; split the cycle or raise the bound deliberately rather than \
                 routing a walk that escaped",
                edges.len()
            )));
        }
        for pair in edges.windows(2) {
            let (before, after) = (&pair[0], &pair[1]);
            if before.to() != after.from() {
                return Err(Error::invalid(format!(
                    "composition breaks between {} and {}; consecutive edges must join, or the \
                     traversal order is not the order given",
                    before.to().label(),
                    after.from().label()
                )));
            }
        }
        // Indexing is safe under the length floor asserted above, but the
        // floor is a runtime fact and `first`/`last` make it a compile-time
        // one. `unwrap` is denied outside tests for exactly this case.
        let (Some(first), Some(last)) = (edges.first(), edges.last()) else {
            return Err(Error::invalid(
                "a composition with no edges cannot close; supply the edges the cycle traverses",
            ));
        };
        if last.to() != first.from() {
            return Err(Error::invalid(format!(
                "composition does not close: it starts at {} and ends at {}. An open chain is a \
                 position, not an arbitrage, so close the cycle or route it as a position",
                first.from().label(),
                last.to().label()
            )));
        }
        Ok(Self { edges })
    }

    pub fn edges(&self) -> &[CompositionEdge] {
        &self.edges
    }

    /// Every class present, in a `BTreeSet` because it reaches the rationale
    /// string and a replay that reorders is not a replay.
    pub fn classes(&self) -> BTreeSet<EdgeClass> {
        self.edges.iter().map(CompositionEdge::class).collect()
    }

    /// Every venue any endpoint names.
    pub fn venues(&self) -> BTreeSet<VenueId> {
        self.edges
            .iter()
            .flat_map(|edge| [edge.from().venue.clone(), edge.to().venue.clone()])
            .collect()
    }

    /// Every region any endpoint names.
    pub fn regions(&self) -> BTreeSet<RegionId> {
        self.edges
            .iter()
            .flat_map(|edge| [edge.from().region.clone(), edge.to().region.clone()])
            .collect()
    }

    /// Positions of the mirror edges, in traversal order. The keys
    /// [`assign`] expects a [`MirrorFacts`] for.
    pub fn mirror_edges(&self) -> BTreeSet<usize> {
        self.edges
            .iter()
            .enumerate()
            .filter(|(_, edge)| edge.class() == EdgeClass::Mirror)
            .map(|(index, _)| index)
            .collect()
    }
}

/// What the platform knows about one mirror edge, for the four paths that
/// turn on it.
///
/// Each field answers exactly one row of §30.2's table. Facts rather than
/// judgements: whether the remote venue *accepts* a resting order is
/// something the connectivity module knows, and whether resting one is a
/// good idea is §33.1's adverse-selection check, not this.
///
/// Private fields and one fallible constructor, so §31.1 can add the
/// inventory-band detail [`Self::both_sides_at_target`] is the coarse form
/// of without breaking a caller.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MirrorFacts {
    both_sides_at_target: bool,
    local_hedge_available: bool,
    remote_accepts_resting: bool,
    firm_quote_window: Option<Duration>,
    round_trip: Duration,
    established_mirror: bool,
}

impl MirrorFacts {
    /// Refuse facts that cannot be true of a measured round trip.
    ///
    /// A round trip of zero is the value a caller supplies when it has not
    /// measured one, and it makes every firm quote look like it outlasts the
    /// wire. §31 puts New York to London at roughly 28 ms each way; the
    /// number is not asserted here, but that it was measured at all is.
    pub fn new(
        both_sides_at_target: bool,
        local_hedge_available: bool,
        remote_accepts_resting: bool,
        firm_quote_window: Option<Duration>,
        round_trip: Duration,
    ) -> Result<Self> {
        if round_trip.as_nanos() <= 0 {
            return Err(Error::invalid(format!(
                "a round trip of {} ns is not a measurement; supply the measured round trip to \
                 the remote region, because path 6 is eligible only when a firm quote outlasts it",
                round_trip.as_nanos()
            )));
        }
        if let Some(window) = firm_quote_window
            && window.as_nanos() <= 0
        {
            return Err(Error::invalid(format!(
                "a firm-quote window of {} ns is not a window; pass None when the remote venue \
                 quotes nothing firm rather than a window of zero",
                window.as_nanos()
            )));
        }
        Ok(Self {
            both_sides_at_target,
            local_hedge_available,
            remote_accepts_resting,
            firm_quote_window,
            round_trip,
            established_mirror: false,
        })
    }

    /// §31.1's SETUP, as the only side that can see it says so: this
    /// instrument is *established* as a mirror — the asset is held in both
    /// regions under one distributed target, as a standing arrangement.
    ///
    /// # Why this exists beside [`Self::both_sides_at_target`] instead of
    /// replacing it
    ///
    /// A cell can never measure the remote region's inventory, and setting
    /// `both_sides_at_target` from its own book alone would be a claim about
    /// a fact it does not hold. §31.1's construction is precisely that it
    /// does not need to: each region gates its own direction from its own
    /// band, and "a region below target may only buy" against "a region
    /// above target may only sell" makes same-side trading impossible
    /// without either side knowing the other's holding.
    ///
    /// So the two facts answer two different questions and both are kept.
    /// `both_sides_at_target` is a caller that has measured both sides —
    /// the centre could, an edge cell cannot. This is a caller saying the
    /// mirror exists, which is a setup fact rather than a measurement, and
    /// which is what an edge cell can honestly assert from a distributed
    /// inventory target it holds and a band its own operator configured.
    ///
    /// Either one makes §30.2's row 3 eligible. Neither says the cycle may
    /// be executed: that is §33.1's extension
    /// ([`crate::extension::check`]), which asks §31.1's direction question
    /// after the path is assigned, because §30.2 assigns and §33.1 gates.
    pub const fn established_mirror(mut self, established: bool) -> Self {
        self.established_mirror = established;
        self
    }

    /// Inventory of the mirrored asset is at target on both sides, so each
    /// region can trade its own side independently. §30.2's path 3 row, in
    /// the coarse form a caller that can see both regions supplies.
    pub const fn both_sides_at_target(&self) -> bool {
        self.both_sides_at_target
    }

    /// Whether the mirror §30.2's rows 3 and 4 turn on is in place at all,
    /// by either of the two facts that can establish it.
    ///
    /// Row 4's "one side lacks inventory" is read as the negation of this,
    /// and deliberately not as a finer statement about where in its band
    /// each side sits: at assignment time the question is whether the mirror
    /// exists to be traded, and where the local side sits inside its band is
    /// §31.1's question, asked by §33.1's extension once a path is assigned.
    pub const fn mirror_is_in_place(&self) -> bool {
        self.both_sides_at_target || self.established_mirror
    }

    /// A local instrument that offsets the exposure is available now. §30.2's
    /// path 4 row.
    pub const fn local_hedge_available(&self) -> bool {
        self.local_hedge_available
    }

    /// The remote venue accepts a resting order. §30.2's path 5 row.
    pub const fn remote_accepts_resting(&self) -> bool {
        self.remote_accepts_resting
    }

    /// How long the remote venue holds its quote firm, when it holds one.
    pub const fn firm_quote_window(&self) -> Option<Duration> {
        self.firm_quote_window
    }

    /// The measured round trip to the remote region.
    pub const fn round_trip(&self) -> Duration {
        self.round_trip
    }

    /// Whether the firm quote outlasts the round trip. §30.2's path 6 row,
    /// "quotes firm beyond round trip".
    ///
    /// Strictly beyond: a quote that expires exactly when the order arrives
    /// has no margin, and §33.1 requires the window to exceed "round trip
    /// plus execution plus margin" on top of this.
    pub fn firm_beyond_round_trip(&self) -> bool {
        self.firm_quote_window
            .is_some_and(|window| window.as_nanos() > self.round_trip.as_nanos())
    }
}

/// What a path does about coordination — §31's second column, which is what
/// the default ranking is ordered by.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Coordination {
    /// Nothing outside this process has to agree to anything.
    Unilateral,
    /// Agreed in advance as policy; nothing is negotiated at execution time.
    PolicyInAdvance,
    /// The platform commits first and completes later at its own speed.
    PreCommitted,
    /// A counterparty's promise is what makes it work.
    FromTheVenue,
    /// Completion happens when it happens, over seconds to minutes.
    Loose,
}

impl Coordination {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Unilateral => "unilateral",
            Self::PolicyInAdvance => "policy_in_advance",
            Self::PreCommitted => "pre_committed",
            Self::FromTheVenue => "from_the_venue",
            Self::Loose => "loose",
        }
    }
}

/// The eight execution paths of §30.2 and §31.
///
/// Deliberately **not** `#[non_exhaustive]`: a ninth path must break every
/// `match` that dispatches on one. §33.1 adds a per-path check to the risk
/// gate, and a check that silently defaults to "nothing extra" for a path
/// nobody considered is a control that reads as protection and cannot fire.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum ExecutionPath {
    /// 1 — four orders back to back on one pre-warmed session.
    IntraVenue,
    /// 2 — latency-equalised parallel dispatch within one region.
    CrossVenue,
    /// 3 — asset held in both regions, each side trading independently
    /// against a distributed reference under direction gating (§31.1).
    MirroredInventory,
    /// 4 — execute locally, hedge locally, complete remotely later.
    HedgedBridging,
    /// 5 — the remote leg rests as a limit order, repriced locally.
    PassiveAnchoring,
    /// 6 — the remote venue quotes firm for longer than the round trip.
    FirmQuoteBridging,
    /// 7 — cycles between spot, perpetual, future, tokenised and ETF forms.
    RepresentationBasis,
    /// 8 — options structures as cycles: parity, boxes, conversions.
    PayoffEquivalence,
}

impl ExecutionPath {
    /// All eight, in §30.2's own order. The set a [`PathPolicy`] must rank
    /// in full.
    pub const ALL: [Self; 8] = [
        Self::IntraVenue,
        Self::CrossVenue,
        Self::MirroredInventory,
        Self::HedgedBridging,
        Self::PassiveAnchoring,
        Self::FirmQuoteBridging,
        Self::RepresentationBasis,
        Self::PayoffEquivalence,
    ];

    /// The path's number in §30.2's table, for a message an operator reads
    /// beside the blueprint.
    pub const fn number(&self) -> u8 {
        match self {
            Self::IntraVenue => 1,
            Self::CrossVenue => 2,
            Self::MirroredInventory => 3,
            Self::HedgedBridging => 4,
            Self::PassiveAnchoring => 5,
            Self::FirmQuoteBridging => 6,
            Self::RepresentationBasis => 7,
            Self::PayoffEquivalence => 8,
        }
    }

    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::IntraVenue => "intra_venue",
            Self::CrossVenue => "cross_venue",
            Self::MirroredInventory => "mirrored_inventory",
            Self::HedgedBridging => "hedged_bridging",
            Self::PassiveAnchoring => "passive_anchoring",
            Self::FirmQuoteBridging => "firm_quote_bridging",
            Self::RepresentationBasis => "representation_basis",
            Self::PayoffEquivalence => "payoff_equivalence",
        }
    }

    /// §31's coordination column.
    pub const fn coordination(&self) -> Coordination {
        match self {
            Self::IntraVenue | Self::CrossVenue => Coordination::Unilateral,
            Self::MirroredInventory => Coordination::PolicyInAdvance,
            Self::HedgedBridging => Coordination::Loose,
            Self::PassiveAnchoring => Coordination::PreCommitted,
            Self::FirmQuoteBridging => Coordination::FromTheVenue,
            Self::RepresentationBasis | Self::PayoffEquivalence => Coordination::Unilateral,
        }
    }

    /// Whether the path needs at least one mirror edge to be eligible.
    pub const fn needs_mirror_edge(&self) -> bool {
        matches!(
            self,
            Self::MirroredInventory
                | Self::HedgedBridging
                | Self::PassiveAnchoring
                | Self::FirmQuoteBridging
        )
    }
}

/// The ranking that decides which eligible path is assigned.
///
/// §30.2: "Where several paths are eligible, the optimiser chooses. Path
/// assignment is a policy decision made globally with full cost and risk
/// information, not a local heuristic." This type is how that decision
/// arrives here. The router holds no preference of its own beyond
/// [`PathPolicy::least_exposed_first`], which is documented as a default
/// rather than presented as the answer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PathPolicy {
    preference: Vec<ExecutionPath>,
}

impl PathPolicy {
    /// Refuse a ranking that is not a permutation of all eight paths.
    ///
    /// A ranking that omits a path leaves that path eligible and never
    /// assignable — the control that reads as protection and cannot fire,
    /// in the shape of a path that can be found and never taken. A ranking
    /// that repeats one says two contradictory things about the same
    /// comparison.
    pub fn new(preference: Vec<ExecutionPath>) -> Result<Self> {
        let distinct: BTreeSet<ExecutionPath> = preference.iter().copied().collect();
        if distinct.len() != preference.len() {
            return Err(Error::invalid(format!(
                "a path preference of {} entries names only {} distinct paths; a path ranked \
                 twice makes the comparison ambiguous, so list each of the eight once",
                preference.len(),
                distinct.len()
            )));
        }
        let missing: Vec<&'static str> = ExecutionPath::ALL
            .iter()
            .filter(|path| !distinct.contains(path))
            .map(ExecutionPath::as_str)
            .collect();
        if !missing.is_empty() {
            return Err(Error::invalid(format!(
                "a path preference must rank all eight paths and omits {}; an unranked path is \
                 one the router can find eligible and never assign, which reads as a path the \
                 platform supports and is not",
                missing.join(", ")
            )));
        }
        Ok(Self { preference })
    }

    /// The default ranking: least time exposed to something the platform
    /// does not control, first.
    ///
    /// 1 and 2 complete inside one process in milliseconds. 3 needs no
    /// coordination at execution time because the policy was agreed in
    /// advance. 7 and 8 need none either but hold a position until
    /// convergence. 6 depends on a counterparty honouring a quote. 5 rests
    /// an order and pays adverse selection for the privilege. 4 holds a
    /// basis open for seconds to minutes, which is the longest interval on
    /// §31's table.
    ///
    /// A default, not a finding: an optimiser with cost and risk in hand is
    /// expected to supply its own, and [`PathPolicy::new`] is how.
    pub fn least_exposed_first() -> Self {
        Self {
            preference: vec![
                ExecutionPath::IntraVenue,
                ExecutionPath::CrossVenue,
                ExecutionPath::MirroredInventory,
                ExecutionPath::RepresentationBasis,
                ExecutionPath::PayoffEquivalence,
                ExecutionPath::FirmQuoteBridging,
                ExecutionPath::PassiveAnchoring,
                ExecutionPath::HedgedBridging,
            ],
        }
    }

    pub fn preference(&self) -> &[ExecutionPath] {
        &self.preference
    }

    /// The highest-ranked member of `eligible`, or `None` when it is empty.
    fn choose(&self, eligible: &BTreeSet<ExecutionPath>) -> Option<ExecutionPath> {
        self.preference
            .iter()
            .find(|path| eligible.contains(path))
            .copied()
    }
}

impl Default for PathPolicy {
    fn default() -> Self {
        Self::least_exposed_first()
    }
}

/// What the router decided, and on what evidence.
///
/// Private fields because the type carries an invariant no caller should be
/// able to break: `assigned` is always a member of `eligible`. Only
/// [`assign`] constructs one. `Serialize` without `Deserialize` for the same
/// reason — this is a record of a decision, and a decoder would be a second
/// way to make one.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PathAssignment {
    assigned: ExecutionPath,
    eligible: BTreeSet<ExecutionPath>,
    rationale: String,
}

impl PathAssignment {
    /// The path this cycle will be executed as. What §33.1's per-path
    /// extension dispatches on.
    pub const fn assigned(&self) -> ExecutionPath {
        self.assigned
    }

    /// Every path the composition and its facts admitted, including the
    /// assigned one. Ordered, so a replay renders it the same way.
    pub const fn eligible(&self) -> &BTreeSet<ExecutionPath> {
        &self.eligible
    }

    /// Why this path and not another, in the words an operator reading
    /// §30.2 beside the log would use.
    pub fn rationale(&self) -> &str {
        &self.rationale
    }
}

/// Every path §30.2's table admits for this composition and these facts.
///
/// Refuses rather than returning an empty set, in two cases, because both
/// are things the caller must act on rather than facts about the market:
///
/// * a composition containing a settlement edge, which §30.2 has no row for;
/// * a mirror edge with no [`MirrorFacts`] supplied, or facts supplied for
///   an edge that is not a mirror.
///
/// An empty result is returned as `Ok` only when the table genuinely admits
/// nothing — a mirror cycle with no inventory, no local hedge, no resting
/// support and no firm quote is a real cycle the platform cannot execute,
/// and the caller distinguishes that from a caller bug.
pub fn eligible_paths(
    composition: &Composition,
    facts: &BTreeMap<usize, MirrorFacts>,
) -> Result<BTreeSet<ExecutionPath>> {
    let classes = composition.classes();
    if classes.contains(&EdgeClass::Settlement) {
        return Err(Error::denied(
            "this composition contains a settlement edge and blueprint §30.2 assigns no path to \
             one; price the settlement bridge into a conversion's cost, or split the cycle at \
             the settlement stage and route each half, rather than executing it as though the \
             stages were the same",
        ));
    }

    let mirrors = composition.mirror_edges();
    for index in &mirrors {
        if !facts.contains_key(index) {
            return Err(Error::invalid(format!(
                "composition edge {index} is a mirror edge and no facts were supplied for it; \
                 paths 3 to 6 are told apart by inventory, hedge availability, resting support \
                 and the firm-quote window, so supply MirrorFacts for it or do not present the \
                 edge as a mirror"
            )));
        }
    }
    for index in facts.keys() {
        if !mirrors.contains(index) {
            let named = composition
                .edges()
                .get(*index)
                .map_or("no edge at all", |edge| edge.class().as_str());
            return Err(Error::invalid(format!(
                "mirror facts were supplied for composition edge {index}, which is {named}; \
                 facts keyed to the wrong edge would decide paths 3 to 6 from a different edge's \
                 inventory, so key them by the mirror edge's own position"
            )));
        }
    }

    let mut eligible = BTreeSet::new();

    // Paths 1 and 2 read off the classes alone. "One venue" and "one
    // region" are not re-checked: `CompositionEdge::new` fixes the venue
    // across a conversion and the region across a transport, and
    // `Composition::cycle` requires the chain to join and close, so a closed
    // cycle of conversions has exactly one venue and a closed cycle of
    // conversions and transports has exactly one region. Re-asserting either
    // here would be a branch no input can reach.
    if classes == BTreeSet::from([EdgeClass::Conversion]) {
        eligible.insert(ExecutionPath::IntraVenue);
    }
    if classes.contains(&EdgeClass::Transport)
        && classes
            .iter()
            .all(|class| matches!(class, EdgeClass::Conversion | EdgeClass::Transport))
    {
        eligible.insert(ExecutionPath::CrossVenue);
    }

    // Paths 3 to 6. A path is eligible only if it is eligible for *every*
    // mirror edge: a cycle whose second mirror lacks inventory cannot be
    // executed as though both were at target, and taking the best of the two
    // would size the cycle against the easier half.
    if !mirrors.is_empty() {
        let all = |predicate: &dyn Fn(&MirrorFacts) -> bool| {
            mirrors
                .iter()
                .all(|index| facts.get(index).is_some_and(predicate))
        };
        if all(&MirrorFacts::mirror_is_in_place) {
            eligible.insert(ExecutionPath::MirroredInventory);
        }
        // §30.2's row 4 is "one side lacks inventory, hedge available
        // locally". The inventory clause is the discriminator against row 3
        // and is kept: without it a cycle with inventory on both sides and a
        // hedge to hand would be eligible for a path that exists to cover
        // the case where inventory is missing.
        if !all(&MirrorFacts::mirror_is_in_place) && all(&MirrorFacts::local_hedge_available) {
            eligible.insert(ExecutionPath::HedgedBridging);
        }
        if all(&MirrorFacts::remote_accepts_resting) {
            eligible.insert(ExecutionPath::PassiveAnchoring);
        }
        if all(&MirrorFacts::firm_beyond_round_trip) {
            eligible.insert(ExecutionPath::FirmQuoteBridging);
        }
    }

    if classes.contains(&EdgeClass::Basis) {
        eligible.insert(ExecutionPath::RepresentationBasis);
    }
    if classes.contains(&EdgeClass::Equivalence) {
        eligible.insert(ExecutionPath::PayoffEquivalence);
    }

    Ok(eligible)
}

/// Assign one path to a composition, under a policy.
///
/// Refuses when nothing is eligible. A cycle the table admits no path for is
/// a cycle that cannot be executed, and returning some nearest path would be
/// exactly the local heuristic §30.2 forbids.
pub fn assign(
    composition: &Composition,
    facts: &BTreeMap<usize, MirrorFacts>,
    policy: &PathPolicy,
) -> Result<PathAssignment> {
    let eligible = eligible_paths(composition, facts)?;
    let classes: Vec<&'static str> = composition
        .classes()
        .iter()
        .map(EdgeClass::as_str)
        .collect();
    let Some(assigned) = policy.choose(&eligible) else {
        return Err(Error::denied(format!(
            "no execution path is eligible for a cycle of {} edges over [{}] across {} region(s); \
             §30.2 assigns a path from the edge classes and, for a mirror edge, from inventory, \
             a local hedge, resting support or a firm quote beyond the round trip — supply \
             whichever of those the cycle actually has, or do not take the cycle",
            composition.edges().len(),
            classes.join(", "),
            composition.regions().len()
        )));
    };
    let names: Vec<String> = eligible
        .iter()
        .map(|path| format!("{} ({})", path.number(), path.as_str()))
        .collect();
    let rationale = format!(
        "edge classes [{}] across {} region(s) admit path(s) {}; assigned path {} ({}), \
         coordination {}, as the highest-ranked eligible path in the supplied preference",
        classes.join(", "),
        composition.regions().len(),
        names.join(", "),
        assigned.number(),
        assigned.as_str(),
        assigned.coordination().as_str()
    );
    Ok(PathAssignment {
        assigned,
        eligible,
        rationale,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn region(id: &str) -> RegionId {
        RegionId::new(id).expect("test region id is valid")
    }

    fn endpoint(object: &str, venue: &str, reg: &str) -> PathEndpoint {
        PathEndpoint::new(
            ObjectId::from_string(object),
            VenueId::new(venue),
            region(reg),
        )
    }

    fn edge(class: EdgeClass, from: PathEndpoint, to: PathEndpoint) -> CompositionEdge {
        CompositionEdge::new(class, from, to).expect("test edge is consistent with its class")
    }

    /// A triangular cycle at one venue: USD -> EUR -> GBP -> USD.
    fn triangle() -> Composition {
        Composition::cycle(vec![
            edge(
                EdgeClass::Conversion,
                endpoint("USD", "XNAS", "us-east"),
                endpoint("EUR", "XNAS", "us-east"),
            ),
            edge(
                EdgeClass::Conversion,
                endpoint("EUR", "XNAS", "us-east"),
                endpoint("GBP", "XNAS", "us-east"),
            ),
            edge(
                EdgeClass::Conversion,
                endpoint("GBP", "XNAS", "us-east"),
                endpoint("USD", "XNAS", "us-east"),
            ),
        ])
        .expect("a closed triangle of conversions at one venue")
    }

    /// A cross-venue cycle inside one region: buy at one venue, transport,
    /// sell at the other, transport the proceeds back.
    fn cross_venue() -> Composition {
        Composition::cycle(vec![
            edge(
                EdgeClass::Conversion,
                endpoint("USD", "XNAS", "us-east"),
                endpoint("BTC", "XNAS", "us-east"),
            ),
            edge(
                EdgeClass::Transport,
                endpoint("BTC", "XNAS", "us-east"),
                endpoint("BTC", "XCME", "us-east"),
            ),
            edge(
                EdgeClass::Conversion,
                endpoint("BTC", "XCME", "us-east"),
                endpoint("USD", "XCME", "us-east"),
            ),
            edge(
                EdgeClass::Transport,
                endpoint("USD", "XCME", "us-east"),
                endpoint("USD", "XNAS", "us-east"),
            ),
        ])
        .expect("a closed cross-venue cycle inside one region")
    }

    /// A cross-region cycle: the asset and the cash are each mirrored.
    fn mirrored() -> Composition {
        Composition::cycle(vec![
            edge(
                EdgeClass::Conversion,
                endpoint("USD", "XNAS", "us-east"),
                endpoint("BTC", "XNAS", "us-east"),
            ),
            edge(
                EdgeClass::Mirror,
                endpoint("BTC", "XNAS", "us-east"),
                endpoint("BTC", "XLON", "eu-west"),
            ),
            edge(
                EdgeClass::Conversion,
                endpoint("BTC", "XLON", "eu-west"),
                endpoint("USD", "XLON", "eu-west"),
            ),
            edge(
                EdgeClass::Mirror,
                endpoint("USD", "XLON", "eu-west"),
                endpoint("USD", "XNAS", "us-east"),
            ),
        ])
        .expect("a closed cross-region cycle with both legs mirrored")
    }

    fn facts_for(composition: &Composition, fact: MirrorFacts) -> BTreeMap<usize, MirrorFacts> {
        composition
            .mirror_edges()
            .into_iter()
            .map(|index| (index, fact))
            .collect()
    }

    fn bare_facts() -> MirrorFacts {
        MirrorFacts::new(false, false, false, None, Duration::from_millis(28))
            .expect("a measured round trip with no other fact is valid")
    }

    #[test]
    fn a_cycle_of_conversions_at_one_venue_is_assigned_the_intra_venue_path() {
        let composition = triangle();
        // Premise: the cycle really is all conversions at one venue.
        assert_eq!(
            composition.classes(),
            BTreeSet::from([EdgeClass::Conversion])
        );
        assert_eq!(composition.venues().len(), 1);
        let assignment = assign(&composition, &BTreeMap::new(), &PathPolicy::default())
            .expect("an all-conversion cycle is routable");
        assert_eq!(assignment.assigned(), ExecutionPath::IntraVenue);
        assert_eq!(assignment.assigned().number(), 1);
    }

    #[test]
    fn a_cycle_with_a_transport_edge_in_one_region_is_assigned_the_cross_venue_path() {
        let composition = cross_venue();
        // Premise: a transport edge is present and the cycle stays in one
        // region, which is what tells path 2 from path 3.
        assert!(composition.classes().contains(&EdgeClass::Transport));
        assert_eq!(composition.regions().len(), 1);
        let assignment = assign(&composition, &BTreeMap::new(), &PathPolicy::default())
            .expect("a conversion-plus-transport cycle is routable");
        assert_eq!(assignment.assigned(), ExecutionPath::CrossVenue);
        assert_eq!(assignment.assigned().number(), 2);
    }

    #[test]
    fn a_mirror_cycle_with_inventory_on_both_sides_is_assigned_mirrored_inventory() {
        let composition = mirrored();
        assert_eq!(composition.mirror_edges().len(), 2);
        let facts = facts_for(
            &composition,
            MirrorFacts::new(true, false, false, None, Duration::from_millis(28))
                .expect("inventory on both sides is a valid fact"),
        );
        let assignment = assign(&composition, &facts, &PathPolicy::default())
            .expect("a mirror cycle with inventory is routable");
        assert_eq!(assignment.assigned(), ExecutionPath::MirroredInventory);
        assert_eq!(assignment.assigned().number(), 3);
    }

    #[test]
    fn a_mirror_cycle_short_of_inventory_with_a_local_hedge_is_assigned_hedged_bridging() {
        let composition = mirrored();
        let facts = facts_for(
            &composition,
            MirrorFacts::new(false, true, false, None, Duration::from_millis(28))
                .expect("a local hedge without inventory is a valid fact"),
        );
        let assignment = assign(&composition, &facts, &PathPolicy::default())
            .expect("a hedgeable mirror cycle is routable");
        assert_eq!(assignment.assigned(), ExecutionPath::HedgedBridging);
        assert_eq!(assignment.assigned().number(), 4);
    }

    #[test]
    fn a_mirror_cycle_with_inventory_is_never_eligible_for_hedged_bridging() {
        // §30.2 row 4 begins "one side lacks inventory". Dropping that clause
        // would make row 4 eligible whenever a hedge existed, and the router
        // would offer a path that exists for a case that has not arisen.
        let composition = mirrored();
        let facts = facts_for(
            &composition,
            MirrorFacts::new(true, true, false, None, Duration::from_millis(28))
                .expect("inventory and a hedge together are a valid fact"),
        );
        let eligible = eligible_paths(&composition, &facts).expect("the cycle routes");
        // Premise: the hedge really is available, so an empty result would
        // not be evidence of anything.
        assert!(eligible.contains(&ExecutionPath::MirroredInventory));
        assert!(!eligible.contains(&ExecutionPath::HedgedBridging));
    }

    #[test]
    fn a_remote_venue_that_accepts_resting_orders_makes_passive_anchoring_eligible() {
        let composition = mirrored();
        let facts = facts_for(
            &composition,
            MirrorFacts::new(false, false, true, None, Duration::from_millis(28))
                .expect("resting support alone is a valid fact"),
        );
        let assignment =
            assign(&composition, &facts, &PathPolicy::default()).expect("the cycle routes");
        assert!(
            assignment
                .eligible()
                .contains(&ExecutionPath::PassiveAnchoring)
        );
        assert_eq!(assignment.assigned(), ExecutionPath::PassiveAnchoring);
        assert_eq!(assignment.assigned().number(), 5);
    }

    #[test]
    fn a_firm_quote_that_does_not_outlast_the_round_trip_does_not_make_path_six_eligible() {
        // The whole content of row 6 is "beyond round trip". A window equal
        // to the round trip arrives exactly as the quote dies.
        let composition = mirrored();
        let equal = facts_for(
            &composition,
            MirrorFacts::new(
                false,
                false,
                false,
                Some(Duration::from_millis(28)),
                Duration::from_millis(28),
            )
            .expect("a window equal to the round trip is a valid fact"),
        );
        let eligible = eligible_paths(&composition, &equal).expect("the cycle routes");
        assert!(!eligible.contains(&ExecutionPath::FirmQuoteBridging));

        let beyond = facts_for(
            &composition,
            MirrorFacts::new(
                false,
                false,
                false,
                Some(Duration::from_millis(29)),
                Duration::from_millis(28),
            )
            .expect("a window beyond the round trip is a valid fact"),
        );
        let eligible = eligible_paths(&composition, &beyond).expect("the cycle routes");
        assert!(eligible.contains(&ExecutionPath::FirmQuoteBridging));
    }

    #[test]
    fn a_path_is_eligible_only_when_every_mirror_edge_admits_it() {
        // Taking the best of two mirror edges would size a cross-region
        // cycle against its easier half.
        let composition = mirrored();
        let mut facts = BTreeMap::new();
        let mirrors: Vec<usize> = composition.mirror_edges().into_iter().collect();
        assert_eq!(mirrors.len(), 2, "the premise is two mirror edges");
        facts.insert(
            mirrors[0],
            MirrorFacts::new(true, false, false, None, Duration::from_millis(28))
                .expect("valid facts"),
        );
        facts.insert(
            mirrors[1],
            MirrorFacts::new(false, false, true, None, Duration::from_millis(28))
                .expect("valid facts"),
        );
        let eligible = eligible_paths(&composition, &facts).expect("the cycle routes");
        assert!(!eligible.contains(&ExecutionPath::MirroredInventory));
        assert!(!eligible.contains(&ExecutionPath::PassiveAnchoring));
        assert!(
            eligible.is_empty(),
            "no path should survive two disagreeing mirror edges: {eligible:?}"
        );
    }

    #[test]
    fn a_mirror_cycle_with_no_usable_fact_is_refused_rather_than_assigned_a_nearest_path() {
        let composition = mirrored();
        let facts = facts_for(&composition, bare_facts());
        // Premise: the facts really are supplied, so this is not the
        // missing-facts refusal.
        assert_eq!(facts.len(), 2);
        let refusal = assign(&composition, &facts, &PathPolicy::default())
            .expect_err("a mirror cycle with nothing behind it cannot be routed");
        assert_eq!(refusal.code(), "denied");
        assert!(
            refusal.message().contains("no execution path is eligible"),
            "the refusal should say nothing was eligible: {}",
            refusal.message()
        );
    }

    #[test]
    fn a_composition_containing_a_settlement_edge_is_refused_because_no_row_assigns_one() {
        let composition = Composition::cycle(vec![
            edge(
                EdgeClass::Conversion,
                endpoint("USD", "XNAS", "us-east"),
                endpoint("TBILL", "XNAS", "us-east"),
            ),
            edge(
                EdgeClass::Settlement,
                endpoint("TBILL", "XNAS", "us-east"),
                endpoint("TBILL", "XNAS", "us-east"),
            ),
            edge(
                EdgeClass::Conversion,
                endpoint("TBILL", "XNAS", "us-east"),
                endpoint("USD", "XNAS", "us-east"),
            ),
        ])
        .expect("a settlement edge inside a closed cycle");
        // Premise: the settlement edge is really present.
        assert!(composition.classes().contains(&EdgeClass::Settlement));
        let refusal = eligible_paths(&composition, &BTreeMap::new())
            .expect_err("§30.2 has no row for a settlement edge");
        assert_eq!(refusal.code(), "denied");
        assert!(
            refusal.message().contains("settlement edge"),
            "the refusal should name the settlement edge: {}",
            refusal.message()
        );
    }

    #[test]
    fn a_mirror_edge_with_no_facts_is_refused_rather_than_treated_as_having_none_of_them() {
        let composition = mirrored();
        assert!(!composition.mirror_edges().is_empty());
        let refusal = eligible_paths(&composition, &BTreeMap::new())
            .expect_err("a mirror edge without facts cannot be routed");
        assert_eq!(refusal.code(), "invalid");
        assert!(
            refusal.message().contains("is a mirror edge and no facts"),
            "the refusal should name the missing facts: {}",
            refusal.message()
        );
    }

    #[test]
    fn facts_keyed_to_an_edge_that_is_not_a_mirror_are_refused() {
        let composition = mirrored();
        let mut facts = facts_for(&composition, bare_facts());
        // Edge 0 is the opening conversion, not a mirror.
        assert_eq!(composition.edges()[0].class(), EdgeClass::Conversion);
        facts.insert(0, bare_facts());
        let refusal =
            eligible_paths(&composition, &facts).expect_err("facts on a conversion are a bug");
        assert_eq!(refusal.code(), "invalid");
        assert!(
            refusal.message().contains("which is conversion"),
            "the refusal should name what the edge actually is: {}",
            refusal.message()
        );
    }

    #[test]
    fn a_basis_edge_assigns_the_representation_basis_path_and_an_equivalence_edge_the_payoff_one() {
        let basis = Composition::cycle(vec![
            edge(
                EdgeClass::Basis,
                endpoint("BTC-SPOT", "XNAS", "us-east"),
                endpoint("BTC-PERP", "XNAS", "us-east"),
            ),
            edge(
                EdgeClass::Basis,
                endpoint("BTC-PERP", "XNAS", "us-east"),
                endpoint("BTC-SPOT", "XNAS", "us-east"),
            ),
        ])
        .expect("spot against perpetual and back");
        let assignment = assign(&basis, &BTreeMap::new(), &PathPolicy::default())
            .expect("a basis cycle is routable");
        assert_eq!(assignment.assigned(), ExecutionPath::RepresentationBasis);
        assert_eq!(assignment.assigned().number(), 7);

        let equivalence = Composition::cycle(vec![
            edge(
                EdgeClass::Equivalence,
                endpoint("SPX-BOX", "XCBO", "us-east"),
                endpoint("SPX-CASH", "XCBO", "us-east"),
            ),
            edge(
                EdgeClass::Equivalence,
                endpoint("SPX-CASH", "XCBO", "us-east"),
                endpoint("SPX-BOX", "XCBO", "us-east"),
            ),
        ])
        .expect("a box against its cash value and back");
        let assignment = assign(&equivalence, &BTreeMap::new(), &PathPolicy::default())
            .expect("an equivalence cycle is routable");
        assert_eq!(assignment.assigned(), ExecutionPath::PayoffEquivalence);
        assert_eq!(assignment.assigned().number(), 8);
    }

    #[test]
    fn the_policy_and_not_the_router_decides_between_several_eligible_paths() {
        // The same composition and the same facts, two policies, two
        // assignments. If the router held the preference itself this could
        // not happen, and §30.2 says the preference is not the router's.
        let composition = mirrored();
        let facts = facts_for(
            &composition,
            MirrorFacts::new(
                true,
                false,
                true,
                Some(Duration::from_millis(40)),
                Duration::from_millis(28),
            )
            .expect("inventory, resting support and a firm quote together"),
        );
        let eligible = eligible_paths(&composition, &facts).expect("the cycle routes");
        // Premise: more than one path really is eligible, or the policy has
        // nothing to arbitrate and this test proves nothing.
        assert!(
            eligible.len() >= 3,
            "expected several eligible paths, got {eligible:?}"
        );

        let default = assign(&composition, &facts, &PathPolicy::default()).expect("routes");
        assert_eq!(default.assigned(), ExecutionPath::MirroredInventory);

        let anchor_first = PathPolicy::new(vec![
            ExecutionPath::PassiveAnchoring,
            ExecutionPath::FirmQuoteBridging,
            ExecutionPath::MirroredInventory,
            ExecutionPath::IntraVenue,
            ExecutionPath::CrossVenue,
            ExecutionPath::RepresentationBasis,
            ExecutionPath::PayoffEquivalence,
            ExecutionPath::HedgedBridging,
        ])
        .expect("a full ranking is a policy");
        let chosen = assign(&composition, &facts, &anchor_first).expect("routes");
        assert_eq!(chosen.assigned(), ExecutionPath::PassiveAnchoring);
    }

    #[test]
    fn a_policy_that_omits_a_path_is_refused_because_that_path_could_never_be_assigned() {
        let partial: Vec<ExecutionPath> = ExecutionPath::ALL
            .iter()
            .copied()
            .filter(|path| *path != ExecutionPath::HedgedBridging)
            .collect();
        assert_eq!(partial.len(), 7, "the premise is a ranking of seven");
        let refusal = PathPolicy::new(partial).expect_err("an unranked path can never be assigned");
        assert_eq!(refusal.code(), "invalid");
        assert!(
            refusal.message().contains("hedged_bridging"),
            "the refusal should name the omitted path: {}",
            refusal.message()
        );
    }

    #[test]
    fn a_policy_that_ranks_a_path_twice_is_refused() {
        let mut doubled: Vec<ExecutionPath> = ExecutionPath::ALL.to_vec();
        doubled.push(ExecutionPath::IntraVenue);
        let refusal = PathPolicy::new(doubled).expect_err("a repeated rank is ambiguous");
        assert_eq!(refusal.code(), "invalid");
        assert!(
            refusal.message().contains("names only 8 distinct paths"),
            "the refusal should say how many distinct paths were named: {}",
            refusal.message()
        );
    }

    #[test]
    fn a_mirror_edge_that_does_not_leave_its_region_is_refused_as_a_mislabelled_transport() {
        let refusal = CompositionEdge::new(
            EdgeClass::Mirror,
            endpoint("BTC", "XNAS", "us-east"),
            endpoint("BTC", "XCME", "us-east"),
        )
        .expect_err("a mirror inside one region is a transport");
        assert_eq!(refusal.code(), "invalid");
        assert!(
            refusal.message().contains("does not leave the region"),
            "the refusal should say why: {}",
            refusal.message()
        );
    }

    #[test]
    fn a_transport_edge_that_leaves_its_region_is_refused_as_a_mislabelled_mirror() {
        // The reverse of the mirror check, and the one that matters more: a
        // cross-region hop labelled transport would be routed as path 2 and
        // sized as though no second region's capital were involved.
        let refusal = CompositionEdge::new(
            EdgeClass::Transport,
            endpoint("BTC", "XNAS", "us-east"),
            endpoint("BTC", "XLON", "eu-west"),
        )
        .expect_err("a transport across regions is a mirror");
        assert_eq!(refusal.code(), "invalid");
        assert!(
            refusal.message().contains("leaves the region"),
            "the refusal should say why: {}",
            refusal.message()
        );
    }

    #[test]
    fn a_composition_whose_edges_do_not_join_is_refused() {
        let refusal = Composition::cycle(vec![
            edge(
                EdgeClass::Conversion,
                endpoint("USD", "XNAS", "us-east"),
                endpoint("EUR", "XNAS", "us-east"),
            ),
            edge(
                EdgeClass::Conversion,
                endpoint("GBP", "XNAS", "us-east"),
                endpoint("USD", "XNAS", "us-east"),
            ),
        ])
        .expect_err("edges that do not join are not a traversal");
        assert_eq!(refusal.code(), "invalid");
        assert!(
            refusal.message().contains("composition breaks between"),
            "the refusal should name the break: {}",
            refusal.message()
        );
    }

    #[test]
    fn a_composition_that_does_not_close_is_refused_because_it_is_a_position() {
        let refusal = Composition::cycle(vec![
            edge(
                EdgeClass::Conversion,
                endpoint("USD", "XNAS", "us-east"),
                endpoint("EUR", "XNAS", "us-east"),
            ),
            edge(
                EdgeClass::Conversion,
                endpoint("EUR", "XNAS", "us-east"),
                endpoint("GBP", "XNAS", "us-east"),
            ),
        ])
        .expect_err("an open chain is a position");
        assert_eq!(refusal.code(), "invalid");
        assert!(
            refusal.message().contains("does not close"),
            "the refusal should say it does not close: {}",
            refusal.message()
        );
    }

    #[test]
    fn a_round_trip_of_zero_is_refused_because_it_makes_every_quote_look_firm_enough() {
        let refusal = MirrorFacts::new(false, false, false, None, Duration::from_nanos(0))
            .expect_err("an unmeasured round trip is not a measurement");
        assert_eq!(refusal.code(), "invalid");
        assert!(
            refusal.message().contains("is not a measurement"),
            "the refusal should say why: {}",
            refusal.message()
        );
    }

    #[test]
    fn a_region_id_with_surrounding_whitespace_is_refused_rather_than_trimmed() {
        let refusal =
            RegionId::new(" us-east").expect_err("whitespace makes it a different region");
        assert_eq!(refusal.code(), "invalid");
        assert!(
            refusal.message().contains("surrounding whitespace"),
            "the refusal should say why: {}",
            refusal.message()
        );
        assert!(RegionId::new("us-east").is_ok());
    }

    #[test]
    fn a_mirror_a_cell_can_only_say_is_established_is_enough_for_row_three_and_shuts_out_row_four()
    {
        // §31.1's SETUP is a standing arrangement, and an edge cell can say
        // it exists without measuring the remote region's book — which it
        // never can. The failure this prevents is the one that made rows 3
        // to 6 unreachable from a cell: the only fact that opened row 3 was
        // a claim about both regions, so a cell either lied or refused every
        // cross-region cycle.
        let composition = mirrored();
        let established = facts_for(
            &composition,
            MirrorFacts::new(false, true, false, None, Duration::from_millis(28))
                .expect("a measured round trip with a local hedge")
                .established_mirror(true),
        );
        // Premise: the coarse fact really is false, so this is not the
        // both-sides-at-target arm passing under a new name.
        assert!(
            established
                .values()
                .all(|fact| !fact.both_sides_at_target()),
            "the premise is that no side was measured"
        );
        let eligible = eligible_paths(&composition, &established).expect("the cycle routes");
        assert!(eligible.contains(&ExecutionPath::MirroredInventory));
        assert!(
            !eligible.contains(&ExecutionPath::HedgedBridging),
            "row 4 covers a mirror that is not in place, and this one is: {eligible:?}"
        );

        // And with the mirror not established, the same facts fall to row 4,
        // so the new fact discriminates rather than merely widening.
        let unestablished = facts_for(
            &composition,
            MirrorFacts::new(false, true, false, None, Duration::from_millis(28))
                .expect("valid facts"),
        );
        let eligible = eligible_paths(&composition, &unestablished).expect("the cycle routes");
        assert!(!eligible.contains(&ExecutionPath::MirroredInventory));
        assert!(eligible.contains(&ExecutionPath::HedgedBridging));
    }

    #[test]
    fn mirror_facts_default_to_an_unestablished_mirror_so_a_caller_must_say_so() {
        // Fail closed: a caller that never mentions the mirror gets no row 3.
        let fact = bare_facts();
        assert!(!fact.mirror_is_in_place());
        assert!(fact.established_mirror(true).mirror_is_in_place());
    }

    #[test]
    fn every_path_is_reachable_from_some_composition_and_facts() {
        // The guard against a router arm nobody can reach. Each of the eight
        // is produced by an input constructed here; a path that could not be
        // reached would be a row of §30.2 the platform claims to support and
        // does not.
        let mut seen: BTreeSet<ExecutionPath> = BTreeSet::new();
        let policy = PathPolicy::default();

        seen.insert(
            assign(&triangle(), &BTreeMap::new(), &policy)
                .expect("routes")
                .assigned(),
        );
        seen.insert(
            assign(&cross_venue(), &BTreeMap::new(), &policy)
                .expect("routes")
                .assigned(),
        );

        let mirror = mirrored();
        for fact in [
            MirrorFacts::new(true, false, false, None, Duration::from_millis(28)),
            MirrorFacts::new(false, true, false, None, Duration::from_millis(28)),
            MirrorFacts::new(false, false, true, None, Duration::from_millis(28)),
            MirrorFacts::new(
                false,
                false,
                false,
                Some(Duration::from_millis(40)),
                Duration::from_millis(28),
            ),
        ] {
            let fact = fact.expect("valid facts");
            let facts = facts_for(&mirror, fact);
            seen.extend(eligible_paths(&mirror, &facts).expect("routes").into_iter());
        }

        let basis = Composition::cycle(vec![
            edge(
                EdgeClass::Basis,
                endpoint("BTC-SPOT", "XNAS", "us-east"),
                endpoint("BTC-PERP", "XNAS", "us-east"),
            ),
            edge(
                EdgeClass::Basis,
                endpoint("BTC-PERP", "XNAS", "us-east"),
                endpoint("BTC-SPOT", "XNAS", "us-east"),
            ),
        ])
        .expect("a basis cycle");
        seen.extend(eligible_paths(&basis, &BTreeMap::new()).expect("routes"));

        let equivalence = Composition::cycle(vec![
            edge(
                EdgeClass::Equivalence,
                endpoint("SPX-BOX", "XCBO", "us-east"),
                endpoint("SPX-CASH", "XCBO", "us-east"),
            ),
            edge(
                EdgeClass::Equivalence,
                endpoint("SPX-CASH", "XCBO", "us-east"),
                endpoint("SPX-BOX", "XCBO", "us-east"),
            ),
        ])
        .expect("an equivalence cycle");
        seen.extend(eligible_paths(&equivalence, &BTreeMap::new()).expect("routes"));

        let missing: Vec<&'static str> = ExecutionPath::ALL
            .iter()
            .filter(|path| !seen.contains(path))
            .map(ExecutionPath::as_str)
            .collect();
        assert!(
            missing.is_empty(),
            "these paths are named by the router and reachable from no input: {missing:?}"
        );
    }
}

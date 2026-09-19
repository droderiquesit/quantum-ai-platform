//! The cell: one region's hot execution path, assembled.
//!
//! Bytes arrive on a feed and leave as orders, without a network hop to the
//! central plane anywhere in between. That is the whole point of the cell and
//! the reason every safety property here has to be local: there is nobody to
//! ask.
//!
//! What makes it safe is that the cell never decides *how much* it may risk.
//! It receives a [`crate::VerifiedEnvelope`] — signed, bounded, venue-scoped,
//! expiring — and the worst it can do while cut off is spend an amount
//! somebody already approved, for as long as the envelope has left to run.

use crate::arbitrage::{ArbitrageDesk, EdgeRefresh};
use crate::decomposition::{Decomposition, DecompositionPolicy, LegSize};
use crate::dispersion::{DispersionPolicy, DispersionVerdict, FillTimes, ReleaseSchedule};
use crate::dropcopy::{CellFill, Discrepancy, DropCopyFill, DropCopyReconciler};
use crate::envelope::VerifiedEnvelope;
use crate::feasibility::{self, VenueModel};
use crate::journal::{Decision, Journal, Mirror};
use crate::mesh::{CellStateDelta, DeltaOrder, DeltaRefusal, StrategyUtilisation};
use crate::mirror::MirrorArrangement;
use crate::passive::{self, PassiveChoice, PassiveOutcome, WholeReason};
use crate::policy::{VerifiedHalt, VerifiedPolicy};
use crate::quoting::{Admission, Depletion, MessageKind, QuoteBudget, RateLimits};
use crate::region::RegionOutlook;
use crate::reservation::RegionTable;
use crate::resume::{ResumeDiscipline, VenueAccount};
use crate::seam::CellLiquidity;
use crate::settlement::{self, GATE_SETTLEMENT, SettlementTerms};
use crate::telemetry::{CellMetrics, RegionShareOutcome};
use qip_arbitrage::liquidity::LiquiditySource;
use qip_arbitrage::scan::{Opportunity, RejectionStage};
use qip_contracts::capital::{CapitalGrant, Utilisation};
use qip_contracts::degradation::{DegradationState, StrategyClass};
use qip_contracts::intent::{Contributor, CycleLeg, Intent, NetIntent, net, netting_ratio};
use qip_contracts::message::{BookSide, MarketMessage};
use qip_contracts::policy::Dispositions;
use qip_contracts::signal::{Signal, SignalKind, StrategyId};
use qip_contracts::venue::{VenueClass, VenueId, VenueStatus};
use qip_core::error::{Error, Result};
use qip_core::{Decimal, Duration, ObjectId, Timestamp};
use qip_feature_dag::engine::FeatureEngine;
use qip_orderbook::venue::VenueState;
use qip_protocols::registry::{FeedKey, ProtocolRegistry};
use qip_risk_engine::autonomy::{AutonomyController, AutonomyLevel};
use qip_routing::extension::{
    ExtensionVerdict, HedgeExtension, MirrorExtension, PathExtensions, check as check_extension,
};
use qip_routing::mirror::Direction;
use qip_routing::path::{
    Composition, CompositionEdge, ExecutionPath, MirrorFacts, PathAssignment, PathEndpoint,
    PathPolicy, RegionId,
};
use qip_routing::pathcycle::{CycleRouter, RepresentationClasses, VenueRegions};
use qip_sequencing::tracker::{ReorderPolicy, Sequencer};
use qip_strategy::compile::CompiledStrategy;
use qip_strategy::program::{Node, Op, Program};
use qip_strategy::runtime::StrategyRuntime;
use std::collections::{BTreeMap, BTreeSet, VecDeque};

/// The gate a cell refuses under when the gateway it was handed is not a
/// simulated venue.
///
/// A constant rather than a bare literal at each of the two sites, because a
/// test that named the gate as its own string would still pass if one site
/// were reworded, and the two sites are the pass gate and the send gate for
/// the same fact. `qip_edge_refusals_total{gate="live_venue"}` is the series.
pub const GATE_LIVE_VENUE: &str = "live_venue";

/// The gate a cell refuses a found cycle under when blueprint §30.2's path
/// router assigns it no execution path (ADR 0068).
///
/// A refusal, not a default. §30.2's eight rows each carry a coordination
/// mechanism and a latency budget that differ by three orders of magnitude,
/// so a cycle routed under the nearest row rather than its own is a cycle
/// sized for one process and executed over minutes. A cycle the table has no
/// row for is one the cell cannot say how it would execute, and a cell that
/// cannot say that sends nothing.
///
/// A literal like every other gate name, refused through [`Cell::refuse`] and
/// therefore counted at the one pass-time recording site rather than a new
/// one, so `qip_edge_refusals_total{gate}` gains a value and no seam.
pub const GATE_PATH_ROUTER: &str = "path_router";

/// The gate a cell refuses a routed cycle under when blueprint §33.1's
/// extension for the assigned path does not hold (§31.1, §33.1).
///
/// Distinct from [`GATE_PATH_ROUTER`] on purpose, and the distinction is the
/// operational one: the router gate says the platform cannot say **how** it
/// would execute this cycle, and this one says it knows how and the
/// conditions that path needs are not met right now. The first is a
/// configuration or a whitelist problem and does not change between passes;
/// the second is a market or an inventory fact and may be true on the next
/// pass. An operator reading one series for both would be unable to tell a
/// mis-configured cell from a cell correctly waiting for its band.
///
/// A literal like every other gate name, refused through [`Cell::refuse`] and
/// therefore counted at the one pass-time recording site rather than a new
/// one, so `qip_edge_refusals_total{gate}` gains a value and no seam.
pub const GATE_PATH_EXTENSION: &str = "path_extension";

/// What the two routing gates made of one found cycle.
///
/// Three arms rather than a `Result`, because a refusal from §30.2's router
/// and a refusal from §33.1's extension are charted under different gates and
/// the second still carries the assignment that was made — see
/// [`GATE_PATH_EXTENSION`] for why an operator needs to tell them apart.
enum RoutedOutcome {
    Assigned(PathAssignment, ExtensionVerdict),
    RouterRefused(Error),
    ExtensionRefused(PathAssignment, Error),
    /// A mirror edge of this cycle reaches a region that has gone dark, so
    /// §36.3 suspends the mirror rather than routing it. An arm of its own
    /// rather than a router refusal: the router would have assigned this
    /// cycle a path, and the reason it did not is in another region.
    RegionDark(Error),
}

/// A local book this cell could hedge one mirror edge's local leg in
/// (blueprint §30.2's row 4, §33.1's path-4 check).
///
/// Built only by [`Cell::local_hedges_for`], and read by the two callers that
/// ask the two halves of the same question: the router, which needs to know a
/// hedge exists before it will assign path 4, and the extension, which needs
/// to know it is deep enough before the cycle may go. One value serves both,
/// so a cycle cannot be assigned on one reading of the books and gated on
/// another.
struct LocalHedge {
    /// Named for the refusal message and for the operator who has to go and
    /// look at the book that was too thin.
    venue: VenueId,
    /// What the hedge book would fill at [`Self::required`], swept this pass.
    depth: Decimal,
    /// The first leg's size, which is what §33.1 measures the depth against.
    required: Decimal,
}

/// The chain entry for an assignment, written from the two places that make
/// one so the record is identical whichever gate ran next.
fn path_assigned(cycle_id: &str, assignment: &PathAssignment) -> Decision {
    Decision::CyclePathAssigned {
        cycle_id: cycle_id.to_string(),
        path: assignment.assigned().number(),
        path_name: assignment.assigned().as_str().to_string(),
        eligible: assignment
            .eligible()
            .iter()
            .map(|path| path.as_str().to_string())
            .collect(),
        rationale: assignment.rationale().to_string(),
    }
}
/// The gate a cell refuses under when a venue's message budget cannot fund
/// what the pass wants to send (§29.2).
///
/// A constant for the same reason as [`GATE_LIVE_VENUE`], and for one more:
/// it too is recorded at two seams. The placement seam goes through
/// [`Cell::refuse`] like every other pass gate; the withdrawal seam is
/// [`Cell::withdraw_expired`], which has no [`WorkReport`] to push a refusal
/// onto and records the series directly. `qip_edge_refusals_total{gate=\"quote_budget\"}`
/// is therefore the sum of a refused quote and a cancel the budget could not
/// fund, which are the same fact about the same bucket seen from the two
/// sides that matter.
pub const GATE_QUOTE_BUDGET: &str = "quote_budget";

/// The gate a cell refuses a cycle under when its legs' venues fill too far
/// apart in time (§32.1). Refused through [`Cell::refuse`] like every other
/// pass gate, so the label needs no second enumeration.
pub const GATE_FILL_DISPERSION: &str = "fill_dispersion";
/// The gate literal an order is refused under when the gateway holding it
/// for its release instant (ADR 0084) found that instant already further in
/// the past than it will send late, and withdrew the order rather than
/// releasing it. A `pub const` for the reason [`GATE_QUOTE_BUDGET`] is one:
/// this refusal is recorded from `Cell::confirm_execution_reports`, which has
/// no `WorkReport` and so records directly, and a formatted string there
/// would unbound the `gate` label.
///
/// Sent late is the guess the schedule exists to prevent: a leg that arrives
/// outside the window its cycle was admitted on is exactly the exposure the
/// offsets were computed against. Withdrawn is the refusal.
pub const GATE_RELEASE_LATE: &str = "release_late";

/// The gate a halted cell journals under when it holds resting orders and
/// the gateway it was handed cannot withdraw them (§29.2).
///
/// Journaled and deliberately **not** counted on
/// `qip_edge_refusals_total`: it is a statement about the gateway a
/// composition root supplied rather than a gate a pass met, it can only be
/// true for as long as that gateway is attached, and counting it would add
/// a per-pass series that rises for a configuration fact. The journal is
/// the record; the chain says it once per halted pass with the order count
/// in it.
pub const GATE_MASS_CANCEL: &str = "mass_cancel";

/// The gate a cell refuses a cross-region cycle under when the region on the
/// other side of one of its mirror edges has gone dark (§36.3).
///
/// A constant rather than a literal because the refusal is raised at the
/// routing seam and asserted by name in two suites, and a gate an operator
/// pages on must not be renameable by an edit that a test would still pass.
/// Refused through [`Cell::refuse`] like every other pass gate, so the label
/// needs no second enumeration.
///
/// Distinct from [`GATE_PATH_ROUTER`] on purpose. "The router had no row for
/// this cycle" and "the cell on the other end of this mirror is not
/// answering" are different findings with different remedies — the first is
/// a whitelist the desk wrote, the second is somebody else's node — and a
/// cycle refused for the second reason under the first gate would send an
/// operator to read a routing table about an outage in another region.
pub const GATE_DARK_REGION: &str = "dark_region";

/// The token a refusal carries when the **centre** has derived the region
/// on the far end of a mirror dark (ADR 0079) — read off policy slot 11's
/// `dark_regions` against this cell's own `venue_regions` in
/// [`Cell::check_extension_for`].
///
/// Distinct from [`GATE_DARK_REGION`], which is this cell's *own* reading of
/// its peers off the region wire the node polls: the two are different
/// sources with different remedies — a mount on this node against a
/// derivation at the centre — and a refusal from the second filed under the
/// first would send an operator to read a local wire about a silence the
/// centre measured. A constant so the token cannot be reworded by an edit a
/// test would still pass.
///
/// **Where it is charted, stated so it is not mistaken for a gate label.**
/// The refusal is raised inside the extension check and so reaches the
/// pass under `GATE_PATH_EXTENSION`, the constant that seam already passes
/// to `Cell::refuse` — `qip_edge_refusals_total{gate}` gains no value and
/// stays bounded. This token opens the refusal's reason, so the journal
/// and the report tell the two dark findings apart even though the series
/// does not. Promoting it to its own `RoutedOutcome` arm and its own label
/// is a change to the routing dispatch and belongs to the lane that owns
/// it.
pub const GATE_CENTRE_DARK_REGION: &str = "centre_dark_region";

/// The gate a second admission of a cycle already resting a leg is refused
/// under (§32.1's passive-first mechanism).
///
/// A constant for the same reason as [`GATE_DARK_REGION`], and distinct from
/// `open_orders` and `arbitrage_cycle_broken` because it is neither: nothing
/// is wrong, the cell is waiting, and a refusal filed under a capacity gate
/// or a break would have an operator looking for a fault during the one
/// window in which the mechanism is doing exactly what it was built to do.
/// Refused through [`Cell::refuse`] like every other pass gate, so the label
/// needs no second enumeration.
pub const GATE_CYCLE_RESTING: &str = "cycle_resting";

/// The gate a restarted cell refuses every pass under until each of its
/// venues has been reconciled against (§36.3, §48's degradation matrix).
///
/// A constant for the same reason as [`GATE_DARK_REGION`]. The cell forms no
/// order at all while this gate is in force; see
/// [`Cell::require_reconciliation_before_resuming`] for what arms it and
/// [`Cell::observe_venue_account`] for the only thing that clears it.
pub const GATE_AWAITING_RECONCILIATION: &str = "awaiting_reconciliation";

/// The gate a cell refuses a disposition under (ADR 0080): the centre named
/// a retired strategy's lot for this cell to unwind, and the cell will not
/// unwind it as named.
///
/// One literal for every reason — the cell holds nothing for that strategy
/// in that instrument, the instruction names no quantity, or its sign would
/// increase the lot or carry it through flat — because the operational
/// reading is the same for all of them: the centre's attribution and this
/// cell's book disagree, and nothing moves until they agree. That is the
/// discipline `disposition_for` applies at the centre when a reported book
/// disagrees with the attribution, applied at the other end of the wire, and
/// the refusal rides the delta so both ends are seen to disagree. The reason
/// string says which. A disposition that fails a *routing* gate — no venue,
/// no book, a stale one, no pricing policy — is refused under that gate's
/// own literal, as a signal would be, and the report line names it.
///
/// The same literal refuses a **signal** from a strategy the applied slot
/// names. The centre has retired it; an envelope it still holds here is one
/// that expires and is never renewed (ADR 0075), and a directional intent
/// raised on it would net against the strategy's own unwind and hide it.
///
/// A literal like every other gate name, refused through [`Cell::refuse`]
/// and therefore counted at the one pass-time recording site rather than a
/// new one, so `qip_edge_refusals_total{gate}` gains a value and no seam.
pub const GATE_DISPOSITION: &str = "disposition";

/// How long a disposition's intent is good for once built. It enters the
/// netting set in the same pass, so this is documentation of the intent's
/// scope rather than a bound anything waits on: the instruction is re-read
/// from the applied slot on every pass, and a lot still open next pass gets
/// a new intent from the book as it then stands.
const DISPOSITION_VALIDITY: Duration = Duration::from_secs(60);

/// How a cell is identified and what it is allowed to reach.
#[derive(Clone, Debug)]
pub struct CellConfig {
    pub cell_id: String,
    pub region: String,
    /// Venues this cell may trade. A venue absent here is unreachable to it
    /// whatever an envelope says — the two are independent bounds, and an
    /// order must clear both.
    pub venues: Vec<VenueId>,
    /// How long a book may go unrefreshed before its prices stop counting.
    pub max_staleness: Duration,
    /// The runtime node budget a strategy may not exceed.
    pub strategy_budget: usize,
    /// What the cell knows about executing at each venue, keyed by venue id
    /// (blueprint §18.1). A venue absent here is judged for depth alone —
    /// see [`crate::feasibility`] for why that is stated rather than
    /// defaulted.
    pub feasibility: BTreeMap<String, VenueModel>,
    /// When each venue's proceeds become usable as funding, keyed by venue
    /// id (§56.2 rule 21, §32.2). A venue absent here is not projected — its
    /// dependent legs are counted on `qip_edge_settlement_unprojected_venues`
    /// rather than judged — see [`crate::settlement`] for why that is stated
    /// rather than defaulted in either direction.
    pub settlement: BTreeMap<String, SettlementTerms>,
    /// Which region each venue sits in, for the venues that are not in this
    /// cell's own (blueprint §31.1).
    ///
    /// Keyed by venue id. **A venue absent here is in the cell's own
    /// region**, which is what every cell did before §31.1 and is still the
    /// default, so an empty map is exactly the behaviour ADR 0068 shipped.
    ///
    /// # This can never widen what the cell may reach
    ///
    /// It says *where* a venue is, never *whether* the cell may trade there.
    /// That is `venues` alone, and [`Cell::install_arbitrage`] refuses an
    /// entry here naming a venue `venues` does not, so the map is a strict
    /// annotation of a list the operator already wrote. A venue that is not
    /// in `venues` is unreachable whatever this says, and a venue that is in
    /// `venues` is reachable whether or not this says anything — all this
    /// decides is whether a hop to it is a transport edge or a mirror edge,
    /// which is the difference between §30.2's row 2 and its rows 3 to 6.
    pub venue_regions: BTreeMap<String, String>,
    /// The interval §27.1's forty percent crossing cap is measured over, if
    /// the owner of the cap has chosen one.
    ///
    /// `None` — the default — measures the cap against each net on its own,
    /// which is what this cell has always done and is the safe reading:
    /// under it a net that cancels completely is always over the cap and is
    /// never crossed (see [`Cell::cross_internally`] for the arithmetic).
    /// The blueprint writes the cap "per instrument per interval" and never
    /// says how long the interval is; the length decides when a safety
    /// control fires, so the default does not guess one, and setting this is
    /// the owner's decision (completion plan D3), not this crate's.
    pub crossing_interval: Option<CrossingInterval>,
    /// The venue message limits this cell quotes within (§29.2).
    ///
    /// Not an `Option`, and that is the decision: a rate limit that
    /// defaults to absent fires in no deployment that forgot to set it,
    /// which is the shape of control this repository has already shipped
    /// once and the rules name by name. Every [`RateLimits`] is built by a
    /// constructor that refuses an incoherent one, so this field being
    /// public cannot smuggle a budget past the checks the way
    /// `crossing_interval` can — the fields inside it are private.
    pub quote_limits: RateLimits,
    /// The spread in fill time a multi-venue cycle may carry (§32.1).
    /// Always in force, for the reason above.
    pub dispersion: DispersionPolicy,
    /// The smallest fraction of its planned size a cycle will be completed at
    /// once a leg has filled short (§32.1). Always in force, for the reason
    /// above: a decomposition policy that defaulted to absent would send
    /// every later leg at its planned size, which is the position this
    /// control exists to stop.
    pub decomposition: DecompositionPolicy,
}

/// The rolling window §27.1's crossing cap is evaluated against.
///
/// Both forms are "trailing, this pass included": the cap compares the
/// crossed size the window has admitted plus the one proposed against the
/// gross intent the window has seen plus this net's. Neither form lets a
/// cross be trimmed to fit — the cap still refuses whole.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CrossingInterval {
    /// The last `n` passes of [`Cell::work`], counting the current one.
    /// `Passes(1)` is the per-net reading with the accounting switched on.
    Passes(u32),
    /// Every net evaluated within the trailing span of wall time.
    Span(Duration),
}

/// How many nets one instrument's crossing window may hold.
///
/// One sample per net per pass, so `Passes(n)` holds at most `n` and is
/// refused above this at configuration. A `Span` window holds as many as
/// arrive; at this bound the history is truncated *and the cap refuses every
/// cross* until it drains, because a window whose oldest gross has been
/// dropped cannot be measured, and a cap measured against part of its window
/// is a cap that fires late.
pub const MAX_CROSSING_WINDOW_SAMPLES: usize = 1_024;

impl CellConfig {
    pub fn new(cell_id: impl Into<String>, region: impl Into<String>) -> Self {
        Self {
            cell_id: cell_id.into(),
            region: region.into(),
            venues: Vec::new(),
            max_staleness: Duration::from_secs(5),
            strategy_budget: 4_096,
            feasibility: BTreeMap::new(),
            settlement: BTreeMap::new(),
            venue_regions: BTreeMap::new(),
            crossing_interval: None,
            quote_limits: RateLimits::default(),
            dispersion: DispersionPolicy::default(),
            decomposition: DecompositionPolicy::default(),
        }
    }

    /// Measure the crossing cap over `interval` rather than per net.
    ///
    /// Refused rather than clamped when the interval is empty or longer
    /// than the history can hold: a zero-pass window would make the cap
    /// compare a cross against nothing and admit everything, and a window
    /// longer than the bound would be silently shortened to it — the safety
    /// parameter the operator wrote replaced by one they did not.
    pub fn with_crossing_interval(mut self, interval: CrossingInterval) -> Result<Self> {
        Self::check_crossing_interval(interval)?;
        self.crossing_interval = Some(interval);
        Ok(self)
    }

    /// The interval check, shared with [`Self::validate`].
    ///
    /// Extracted because every field of this struct is `pub`, so
    /// `config.crossing_interval = Some(CrossingInterval::Passes(0))` reaches
    /// the cell without ever passing through the builder above. Assembly runs
    /// the same predicate again, so the builder is the convenient door rather
    /// than the only one holding the property.
    fn check_crossing_interval(interval: CrossingInterval) -> Result<()> {
        match interval {
            CrossingInterval::Passes(0) => {
                return Err(Error::invalid(
                    "a crossing interval of zero passes measures the cap against nothing; \
                     leave it unset to measure per net, or name at least one pass",
                ));
            }
            CrossingInterval::Passes(passes)
                if usize::try_from(passes).is_ok_and(|n| n > MAX_CROSSING_WINDOW_SAMPLES) =>
            {
                return Err(Error::invalid(format!(
                    "a crossing interval of {passes} passes exceeds the {MAX_CROSSING_WINDOW_SAMPLES} \
                     the history holds per instrument, and would be measured over fewer than \
                     configured"
                )));
            }
            CrossingInterval::Span(span) if span.as_nanos() <= 0 => {
                return Err(Error::invalid(format!(
                    "a crossing interval of {} nanoseconds measures the cap against nothing; \
                     leave it unset to measure per net, or name a positive span",
                    span.as_nanos()
                )));
            }
            CrossingInterval::Passes(_) | CrossingInterval::Span(_) => {}
        }
        Ok(())
    }

    /// What this configuration must satisfy before a cell is assembled from
    /// it, or the refusal naming what to set instead.
    ///
    /// [`Cell::new`] has returned `Result` since it was written and its body
    /// was a single `Ok(Self { .. })`: a signature advertising that assembly
    /// refuses a bad configuration, over a constructor no input could make
    /// fail. That is the shape the risk rules call out by name — a control
    /// that reads as protection and is not — and it sat on the type the
    /// safety rules cite as the third layer of the paper-trading boundary.
    ///
    /// Each clause below refuses a configuration that would leave the cell
    /// looking deployed while it could not do its job, and each refuses
    /// rather than substituting a value the operator did not write:
    ///
    /// * an unnamed cell numbers its orders `-1`, `-2`, … and keys its region
    ///   holds on the empty string, so two such cells sharing one regional
    ///   allocation take each other's holds;
    /// * an unnamed region reaches the centre as a state delta attributed to
    ///   nowhere, and reaches the metric registry as an empty `region` label;
    /// * a cell with no venue can never send: `Cell::venue_for` searches the
    ///   configured list and returns `None` for every object, so every signal
    ///   it raises dies at venue selection and the cell is inert while
    ///   reporting healthy;
    /// * a crossing interval that measures the §27.1 cap against nothing —
    ///   the case [`Self::with_crossing_interval`] already refuses, re-checked
    ///   here because the field is `pub` and the builder is skippable.
    ///
    /// The autonomy ceiling is deliberately *not* among them. A cell builds
    /// its own [`AutonomyController`], whose ceiling is paper trading and
    /// which no argument here can raise, so a clause asserting it would be a
    /// check that cannot fire — the very thing the rest of this list exists
    /// to remove.
    pub fn validate(&self) -> Result<()> {
        if self.cell_id.trim().is_empty() {
            return Err(Error::invalid(
                "a cell was assembled with no cell id; orders are numbered and region holds are \
                 keyed on it, so set QIP_CELL_ID to the identifier the centre knows this cell by",
            ));
        }
        if self.region.trim().is_empty() {
            return Err(Error::invalid(format!(
                "cell {} was assembled with no region; its state deltas and its metrics are \
                 attributed by region, so set QIP_CELL_REGION to the region it runs in",
                self.cell_id
            )));
        }
        if self.venues.is_empty() {
            return Err(Error::invalid(format!(
                "cell {} was assembled with no venue; it would raise signals it could never \
                 send, so name at least one venue in QIP_VENUES",
                self.cell_id
            )));
        }
        // ADR 0078, decision three: no reserved venue identifier, now or
        // later. `qip-routing`'s consolidator reserves this name for "the
        // strategy chose no venue"; a cell configured with it would have
        // `venue_for` choose it and `place_net` send an order to a venue
        // literally named so. In this crate the venue is chosen before the
        // intent exists, so the sentinel has nothing to mean here.
        if let Some(reserved) = self
            .venues
            .iter()
            .find(|venue| venue.as_str() == qip_routing::UNSPECIFIED_VENUE)
        {
            return Err(Error::invalid(format!(
                "cell {} was assembled with the venue {}, which is the reserved name for an \
                 intent that chose no venue; a cell chooses the venue itself before an intent \
                 exists, so name a real venue in QIP_VENUES",
                self.cell_id,
                reserved.as_str()
            )));
        }
        if let Some(interval) = self.crossing_interval {
            Self::check_crossing_interval(interval)?;
        }
        Ok(())
    }

    pub fn with_venue(mut self, venue: VenueId) -> Self {
        self.venues.push(venue);
        self
    }

    /// Name a venue this cell may trade **and** say it sits in another
    /// region (§31.1).
    ///
    /// Both halves in one call on purpose: the venue is pushed onto
    /// `venues` by this method, so there is no way to annotate a region for
    /// a venue the cell was not configured for. That is the structural half
    /// of the guarantee; [`Cell::install_arbitrage`] holds the other half at
    /// runtime, for the caller that set the `pub` field directly.
    ///
    /// Refused when `region` is empty or carries surrounding whitespace, and
    /// **not trimmed**: `RegionId::new` refuses the same thing at the
    /// router, two ids differing by a space are two regions to a mirror
    /// edge, and a value corrected here is a configuration bug that survives
    /// into every later pass.
    pub fn with_venue_in_region(
        mut self,
        venue: VenueId,
        region: impl Into<String>,
    ) -> Result<Self> {
        let region = region.into();
        if region.is_empty() || region.trim() != region {
            return Err(Error::invalid(format!(
                "region id {region:?} for venue {} is empty or carries surrounding whitespace; \
                 two region ids differing by a space are two regions to a mirror edge, so \
                 supply it exactly rather than relying on this to trim",
                venue.as_str()
            )));
        }
        self.venue_regions
            .insert(venue.as_str().to_string(), region);
        self.venues.push(venue);
        Ok(self)
    }

    /// Install the feasibility model for a venue.
    ///
    /// The model is keyed by the venue's id and read on every intent for that
    /// venue; installing one for a venue the cell cannot reach is harmless
    /// and installing none for a venue it can is the depth-only case.
    #[must_use]
    pub fn with_feasibility(mut self, venue: &VenueId, model: VenueModel) -> Self {
        self.feasibility.insert(venue.as_str().to_string(), model);
        self
    }

    /// Install the settlement terms for a venue (§56.2 rule 21).
    ///
    /// Read at every cycle admission for a leg that spends what the leg
    /// before it delivered at this venue. Installing terms for a venue the
    /// cell cannot reach is harmless, as with a feasibility model, and
    /// installing none for a venue it can is the unprojected arm that the
    /// gauge counts.
    #[must_use]
    pub fn with_settlement(mut self, venue: &VenueId, terms: SettlementTerms) -> Self {
        self.settlement.insert(venue.as_str().to_string(), terms);
        self
    }

    /// How many of this cell's venues carry no settlement terms.
    pub fn unprojected_settlement_venues(&self) -> usize {
        self.venues
            .iter()
            .filter(|venue| !self.settlement.contains_key(venue.as_str()))
            .count()
    }
}

/// What one pass of the cell's work produced.
#[derive(Clone, Debug, Default)]
pub struct WorkReport {
    pub signals: Vec<Signal>,
    /// Orders the venue accepted this pass. Accepted, not filled: an entry
    /// here is a resting or working order until a [`Self::fills`] entry names
    /// it, and nothing downstream may read it as a position.
    pub orders: Vec<PlacedOrder>,
    /// Fills the venue reported this pass, on orders from this pass or an
    /// earlier one, each attributed to its contributors. These — and only
    /// these — are what the cell has traded.
    pub fills: Vec<ConfirmedFill>,
    /// Nets that cancelled to zero: strategies that wanted opposite things,
    /// whose disagreement never reached a venue. Recorded because a
    /// cancellation is an outcome the platform should be able to explain, not
    /// an absence.
    pub cancelled: Vec<NetIntent>,
    /// Gross intent over net order volume, per blueprint §27 — the single
    /// best summary of whether the strategy set has genuine diversity. `None`
    /// when everything cancelled, because the ratio is unbounded there and a
    /// sentinel would be a number nobody computed.
    pub netting_ratio: Option<f64>,
    /// Every gate that said no, and why. A cell must answer "why did nothing
    /// trade" as precisely as "why did this trade".
    pub refusals: Vec<(String, String)>,
    /// The venue each *feasibility* refusal in `refusals` was about, keyed
    /// by that entry's index. Only `admit_feasible` pushes here, at the
    /// instant it refuses, so the index is exact and the join in
    /// [`Cell::state_delta`] cannot attach a venue to the wrong refusal. A
    /// side table rather than a third element on `refusals`, because the
    /// pair is read by every test and by `refused_under`, and a posture
    /// refusal has no venue to carry.
    pub feasibility_venues: Vec<(usize, VenueId)>,
    /// Every internal cross booked this pass (§27.1). A cross is a trade
    /// between two of the platform's own strategies; it is reported rather
    /// than merely journaled so a caller can see it without replaying the
    /// chain.
    pub crosses: Vec<InternalCross>,
    /// The execution path blueprint §30.2 assigned each cycle the scan found,
    /// in the scan's own order (ADR 0068).
    ///
    /// An entry here is **not** an order and **not** a trade. It says how the
    /// cell would execute the cycle if every later gate admits it; the
    /// feasibility gate, the capital envelope and the region allocation all
    /// run afterwards and any of them may still veto it, in which case
    /// [`Self::refusals`] names which. The alternative — reporting only the
    /// cycles that survived — would answer "how did this execute" and leave
    /// "what did the router make of what the scan found" unanswerable, and
    /// the second is the question asked about a cell that is quiet.
    ///
    /// A cycle the router **refused** appears in [`Self::refusals`] under
    /// [`GATE_PATH_ROUTER`] and not here. Every cycle the scan surfaced
    /// therefore leaves at least one of the two marks, and never neither.
    ///
    /// "At least", not "exactly", and the difference is §33.1's extension.
    /// A cycle the router assigned and [`GATE_PATH_EXTENSION`] then refused
    /// leaves **both**: the assignment here, because the platform did decide
    /// how it would execute the cycle, and the refusal there, because the
    /// conditions that path needs were not met. Collapsing the two would
    /// lose the distinction between a cell that cannot route a cycle and one
    /// correctly waiting for its inventory band, which are a configuration
    /// fault and a normal market state respectively.
    pub paths: Vec<RoutedCycle>,
    /// ADR 0080: one line per disposition the applied policy named this
    /// pass, acted on or refused, in the slot's own order. A refusal is also
    /// in [`Self::refusals`] under [`GATE_DISPOSITION`] or the routing gate
    /// that refused it; this is the line that pairs the instruction with
    /// what the cell held and what it built, which the refusal pair cannot
    /// carry. Empty on a pass whose applied policy names nothing.
    pub dispositions: Vec<DispositionLine>,
    /// §30.1's edge update as this pass ran it: how many of the desk's trade
    /// edges were re-quoted because their book had moved, and how many were
    /// left holding their rate. `None` on a pass that ran no refresh — no
    /// desk, a halt, or a degradation that paused the scan — which is a
    /// different fact from a refresh that found nothing moved, and the two
    /// must not read alike.
    pub edge_refresh: Option<EdgeRefresh>,
    pub halted: bool,
}

/// What the cell did with one disposition the centre shipped (ADR 0080).
#[derive(Clone, Debug, PartialEq)]
pub struct DispositionLine {
    pub strategy: StrategyId,
    pub object_id: ObjectId,
    /// The signed quantity the centre named: negative flattens a long,
    /// positive a short.
    pub flatten_by: Decimal,
    /// The signed lot this cell held for the strategy in the instrument at
    /// the instant it read the instruction, summed over its venues.
    pub held: Decimal,
    pub verdict: DispositionVerdict,
}

/// The two things a disposition can become at the cell.
#[derive(Clone, Debug, PartialEq)]
pub enum DispositionVerdict {
    /// A reduce-only intent of this signed size, at this venue, entered the
    /// netting set. Not an order: the feasibility gate and the placement
    /// path may still refuse it, under their own literals.
    Intent {
        signed_size: Decimal,
        venue: VenueId,
    },
    /// Refused under this gate for this reason; nothing entered the netting
    /// set for it.
    Refused { gate: String, reason: String },
}

/// One found cycle and the execution path §30.2 assigns it.
///
/// Carries the whole [`PathAssignment`] rather than only
/// [`PathAssignment::assigned`], because the eligible set and the rationale
/// are what separate "this cycle had one possible path" from "a preference
/// chose between four" — and §30.2 is explicit that the preference is the
/// caller's rather than the router's, so which of the two happened is the
/// fact an argument about routing will turn on.
///
/// Inert, like everything in `qip_routing::path`: an `ExecutionPath`, a set
/// of them and a string. It names no venue, carries no size and cannot be
/// turned into an order.
#[derive(Clone, Debug, PartialEq)]
pub struct RoutedCycle {
    pub cycle_id: String,
    pub assignment: PathAssignment,
}

impl RoutedCycle {
    /// The path assigned, for a caller that wants the decision without the
    /// evidence.
    pub const fn path(&self) -> ExecutionPath {
        self.assignment.assigned()
    }
}

/// One offsetting portion crossed inside the cell rather than at a venue.
///
/// §27.1: the price is the prevailing mid at the netting instant, "never a
/// price either side chose", and both sides are named because the blueprint
/// treats a cross as a ledger entry and a regulatory expectation rather than
/// an optimisation detail.
#[derive(Clone, Debug, PartialEq)]
pub struct InternalCross {
    pub object_id: ObjectId,
    pub venue: VenueId,
    /// The matched size — the smaller of the buying and selling sides, which
    /// is exactly how much never needed a venue.
    pub quantity: Decimal,
    /// The prevailing mid at the netting instant, read from the book rather
    /// than taken from any intent's own reference price.
    pub price: Decimal,
    pub bought: Vec<StrategyId>,
    pub sold: Vec<StrategyId>,
}

/// An order the cell actually sent.
#[derive(Clone, Debug, PartialEq)]
pub struct PlacedOrder {
    pub order_id: String,
    /// The largest contributor by absolute intended size, kept so every
    /// existing reader of this field still sees a strategy. It is no longer
    /// the whole truth once an order carries more than one — `contributors`
    /// is — and it is retained rather than removed so the change is additive
    /// at every seam that already reads it.
    pub strategy: StrategyId,
    /// Every strategy whose intent this order carries, and how much each
    /// wanted. This is the mechanism by which a fill remains traceable to the
    /// strategies that caused it after netting has collapsed them into one
    /// order.
    pub contributors: Vec<Contributor>,
    pub object_id: ObjectId,
    pub venue: VenueId,
    /// The side of the book the order takes: `Ask` is a buy, `Bid` is a sell.
    /// Every gateway reads it this way, and so does the sign on each
    /// contributor below — a positive share bought. Stated here because the
    /// enum's own names do not say which reading an *order* carries.
    pub side: BookSide,
    pub quantity: Decimal,
    pub price: Decimal,
    /// Set by the cell from the gateway's own answer, never taken from the
    /// order. A paper fill counted as real is the single most consequential
    /// bit in the execution path.
    pub simulated: bool,
}

/// The order-entry session's report that part of an order traded.
///
/// This is the channel the order went out on answering — the venue's
/// acknowledgement, or a later execution report on an order that rested. It
/// is the only thing that turns a sent order into a fill inside the cell.
/// The drop copy is the *other* channel and is never read for this; it is
/// what the fills confirmed here are checked against.
#[derive(Clone, Debug, PartialEq)]
pub struct ExecutionReport {
    pub order_id: String,
    pub venue: VenueId,
    pub quantity: Decimal,
    pub price: Decimal,
    pub at: Timestamp,
}

/// An order the gateway withdrew without sending, because the instant it was
/// told to release it at was already further in the past than it will send
/// late (ADR 0084 §4).
///
/// Not an [`ExecutionReport`]: nothing traded. Not a cancel either — the
/// venue never saw the order, so there is no remaining quantity for it to
/// answer with. It is the gateway telling the cell that a record the cell
/// wrote as sent describes an order that never arrived, which is why the
/// cell treats it as a break and not as housekeeping.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnreleasedOrder {
    pub order_id: String,
    pub venue: VenueId,
    /// The instant the cell asked for the order to be released no earlier
    /// than.
    pub scheduled: Timestamp,
    /// How far past `scheduled` the gateway found itself on the first pass
    /// that could have released the order.
    pub lag: Duration,
    /// The most lag the gateway will release under; what `lag` exceeded.
    pub tolerance: Duration,
}

/// A fill the venue reported, attributed to the strategies whose intent the
/// order carried (§43.4: the chain starts at the fill).
///
/// `shares` is the pro-rata split of `quantity` across the order's
/// contributors by [`NetIntent::split_fill`], so the shares sum to the fill
/// exactly, per fill. It is computed from what the venue reported traded and
/// never from what the cell sent: an order that filled in three parts is
/// attributed three times, each summing to its own part.
#[derive(Clone, Debug, PartialEq)]
pub struct ConfirmedFill {
    pub order_id: String,
    pub venue: VenueId,
    pub object_id: ObjectId,
    /// The side of the book the order took: `Ask` bought, `Bid` sold.
    pub side: BookSide,
    pub quantity: Decimal,
    pub price: Decimal,
    /// From the gateway's answer at the time the order was sent.
    pub simulated: bool,
    pub at: Timestamp,
    pub shares: Vec<(StrategyId, Decimal)>,
}

/// An order the venue accepted, as the cell holds it until it is settled.
#[derive(Clone, Debug, PartialEq)]
pub struct OpenOrder {
    pub order_id: String,
    pub venue: VenueId,
    pub object_id: ObjectId,
    pub side: BookSide,
    /// What was sent.
    pub quantity: Decimal,
    /// The limit it was sent with.
    pub price: Decimal,
    /// What the venue has reported traded, summed over every report.
    pub filled: Decimal,
    pub simulated: bool,
    /// The pass instant the cell decided the order on.
    pub sent_at: Timestamp,
    /// The instant the gateway was told not to release it before (ADR 0084):
    /// `sent_at` plus the leg's offset on its cycle's release schedule, and
    /// equal to `sent_at` for a net or an unequalised cycle. **The fill time
    /// is measured from here, not from `sent_at`.** Measured from the decision
    /// instant, a held leg's fill time would include its own hold, the median
    /// would grow by the offset, the next schedule would shrink it, and the
    /// equaliser would chase its own tail.
    pub release_at: Timestamp,
    /// When the cell withdraws what has not filled, for an order sent under
    /// [`PricingPolicy::RestAtMid`]. `None` for a marketable order, which
    /// either filled on acceptance or was cancelled by the venue.
    pub expires_at: Option<Timestamp>,
    /// Why the cell has finished with it, once it has: `filled` when the
    /// reports sum to the quantity sent, `expired` when the cell withdrew
    /// the remainder. `None` is an order still working at the venue — which
    /// is not a position, and not a break.
    pub closed: Option<String>,
}

impl OpenOrder {
    pub fn remaining(&self) -> Decimal {
        self.quantity - self.filled
    }
}

/// An open order and the net it was made from, for attributing its fills.
#[derive(Clone, Debug)]
struct Working {
    order: OpenOrder,
    net: NetIntent,
    /// What the region allocation committed for this order when it went out,
    /// kept on the order so an expiry returns exactly that and not a number
    /// recomputed from a different price. Zero for a cell with no allocation
    /// and for a cycle leg, whose cycle holds once for every leg.
    region_committed: Decimal,
}

/// How many orders the cell will hold open at once.
///
/// An order leaves the set when it is settled — closed and agreed with the
/// venue — so this bounds the working memory of the fill path by the number
/// of orders the venue has not finished with. At the bound the cell refuses
/// to send under the `open_orders` gate rather than sending an order it
/// could not attribute a fill on; the refusal is counted and journaled like
/// every other, so a cell that stopped for this reason says so.
pub const MAX_OPEN_ORDERS: usize = 256;

/// How a strategy's intents are priced when they reach a venue.
///
/// Stated at deployment, per strategy, and read when the net order is
/// placed — never defaulted. A strategy deployed with no policy has its
/// intents refused under the `pricing` gate, because the alternative is a
/// cell deciding on its own whether to cross a spread, and a cell that
/// crosses spreads nobody asked it to is paying for liquidity nobody
/// budgeted. Until this existed every order was a limit at the mid: an
/// order that, against a real two-sided book, rests — and, with nothing to
/// withdraw it, rests forever.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PricingPolicy {
    /// Take the touch: a buy is sent at the best ask, a sell at the best bid,
    /// so it fills on acceptance against what rests there. The net's size
    /// is checked against the size at the touch when it is placed, and a net
    /// that would walk past the touch is refused rather than reduced — the
    /// feasibility gate's rule, applied once more at the size that actually
    /// goes out, because two feasible contributors can net to more than the
    /// touch holds.
    Marketable,
    /// Rest at the prevailing mid, and have the cell withdraw whatever has
    /// not filled once `time_to_live` has elapsed.
    ///
    /// The withdrawal is the cell's own, through [`Placer::cancel`], which is
    /// the venue's cancel path and nothing invented here; a gateway that
    /// cannot withdraw refuses to rest at all, because an order nothing can
    /// withdraw is a position the cell has promised to take at a price the
    /// market has since left. The simulated venue offers no venue-side
    /// expiry, so the cell's own clock is the only one there is.
    RestAtMid { time_to_live: Duration },
}

impl PricingPolicy {
    /// A resting policy, refusing a time to live that could not elapse.
    ///
    /// Zero would withdraw the order on the pass after it was sent, which
    /// is a marketable order that pays to rest for nothing; negative would
    /// never withdraw it. Neither is what anybody meant.
    pub fn rest_at_mid(time_to_live: Duration) -> Result<Self> {
        if time_to_live.as_nanos() <= 0 {
            return Err(Error::invalid(format!(
                "a resting order needs a positive time to live and {} nanoseconds is not one; \
                 name how long the order may rest, or price it marketable",
                time_to_live.as_nanos()
            )));
        }
        Ok(Self::RestAtMid { time_to_live })
    }

    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Marketable => "marketable",
            Self::RestAtMid { .. } => "rest_at_mid",
        }
    }
}

/// A deployed strategy, the arena its plan indexes into, and the capital it
/// runs under.
///
/// The runtime is per-strategy rather than per-cell. One shared arena would
/// mean a plan compiled against one program being evaluated against another,
/// and the failure mode is not a crash: `NodeRef` is an index, so the run
/// would read whatever node happened to sit at that position and emit a signal
/// derived from a different strategy's arithmetic. Giving each deployment the
/// program it was compiled against costs a few kilobytes and removes the
/// aliasing entirely.
#[derive(Debug)]
struct Deployed {
    strategy: CompiledStrategy,
    runtime: StrategyRuntime,
    envelope: VerifiedEnvelope,
    utilisation: Utilisation,
    /// Which of the degradation table's pause rules apply to this strategy.
    /// Everything deployed through [`Cell::deploy`] is `PriceOnly`, which is
    /// true of every strategy this platform ships today: nothing at the edge
    /// consumes world events, so an ingestion or episodic loss must not pause
    /// it.
    class: StrategyClass,
    /// How this strategy's intents are priced at the venue, if the
    /// deployment said. `None` refuses every intent under `pricing`.
    pricing: Option<PricingPolicy>,
}

/// One edge cell.
#[derive(Debug)]
pub struct Cell {
    config: CellConfig,
    protocols: ProtocolRegistry,
    sequencer: Sequencer,
    liquidity: CellLiquidity,
    features: FeatureEngine,
    deployed: BTreeMap<String, Deployed>,
    autonomy: AutonomyController,
    /// The last verified policy payload applied, if any ever was.
    ///
    /// `None` is not a neutral state: with no payload every payload-fed
    /// capability reads as unavailable and the cell sizes at its conservative
    /// floor. A cell nobody ships policy to trades small, not blind.
    policy: Option<VerifiedPolicy>,
    /// Whether the centre has halted this cell through policy. Separate from
    /// the local kill switch on purpose: the switch clears only with an
    /// operator credential, while this clears only with a newer verified
    /// payload saying it is over. Two halts, two release disciplines, and
    /// neither can release the other.
    policy_halted: bool,
    /// The instant of the newest halt applied. A payload releases the policy
    /// halt only if it was issued *after* this, so a pre-halt payload still in
    /// flight cannot un-halt the cell it was racing.
    policy_halt_barrier: Option<Timestamp>,
    /// The second halt wire (§46.2): the reason the polled flag gave, while
    /// it is engaged. Independent of the two above in both directions — it
    /// is set only by [`Self::apply_polled_halt`], which reads a flag the
    /// node polls from a file and not a frame off the mesh, and it is
    /// released only by that flag reading absent or released; no policy
    /// payload, however new, and no operator credential on the kill switch
    /// touches it. Two wires that shared a release would share a failure.
    polled_halt: Option<String>,
    dropcopy: DropCopyReconciler,
    /// The arbitrage desk, if the composition root installed one. `None` is
    /// a cell that runs strategy programs and scans no graph, which is every
    /// cell before this field existed and every test that does not ask for
    /// one.
    desk: Option<InstalledDesk>,
    journal: Journal,
    /// Orders the venue accepted and the cell has not settled, by order id.
    /// Bounded by [`MAX_OPEN_ORDERS`]; see the constant for the refusal at
    /// the bound.
    working: BTreeMap<String, Working>,
    /// Every fill the order-entry channel has confirmed on an order still in
    /// `working`. This is the cell's side of reconciliation. Until this
    /// existed the cell wrote a fill here the moment the venue *accepted* an
    /// order, so an order that rested unfilled was a position the cell
    /// believed in and the venue did not — and the reconciler, doing its
    /// job, halted the cell on the first strategy that fired against a real
    /// two-sided book. Retired with its order on settlement; the journal
    /// keeps the record.
    confirmed: Vec<ConfirmedFill>,
    /// Signed quantity held per venue and instrument, from confirmed fills
    /// alone. Keyed like the crossing history — by what the cell holds books
    /// for — so it is bounded by the instrument set, and it survives
    /// settlement because a position does not stop existing when the order
    /// that built it is agreed.
    positions: BTreeMap<String, Decimal>,
    /// What each strategy holds from internal crosses, keyed by strategy,
    /// venue and instrument, and the cash each has paid or received for it,
    /// keyed by strategy. A cross is a trade between two of the cell's own
    /// strategies at the recorded mid: the buyer's lot goes up and its cash
    /// down by the notional, the seller's the other way round, and the two
    /// cash legs sum to zero because nothing left the cell. Until these
    /// existed a cross was booked — journal entry, both sides, price, size —
    /// and no position or cash balance moved, so a reader of
    /// `crossed_internally` in the chain assumed books that did not exist
    /// (traceability F7). Bounded by the deployed strategies and the
    /// instruments the cell holds books for; `positions` above stays the
    /// venue-facing aggregate and a cross moves it by exactly nothing, which
    /// is why the drop-copy reconciler never sees one.
    strategy_positions: BTreeMap<String, Decimal>,
    strategy_cash: BTreeMap<String, Decimal>,
    /// Every disagreement between this cell's fills and the venue's own
    /// account, kept so the centre hears about it in the state delta as well as
    /// in the journal.
    ///
    /// Bounded, like everything else that grows on a signal from outside. It
    /// can only grow while an operator keeps resuming a cell that a break has
    /// already halted, and past the bound the count is what travels: a
    /// truncation nobody can see would understate an incident.
    breaks: Vec<String>,
    breaks_omitted: u32,
    order_sequence: u64,
    /// Passes of [`Self::work`] so far, counting the halted ones. What a
    /// [`CrossingInterval::Passes`] window is measured in.
    pass: u64,
    /// What each instrument's crossing window has seen, oldest first, keyed
    /// by venue, instrument and representation — the same key `net` groups
    /// on. Empty forever when no interval is configured, so the per-net
    /// reading costs nothing.
    ///
    /// Bounded twice: the key set by the instruments the cell holds books
    /// for, since a net exists only for an instrument a strategy could price
    /// here; and each history by [`MAX_CROSSING_WINDOW_SAMPLES`], past which
    /// the oldest sample is dropped and the cap refuses until the window
    /// drains — see the constant for why refusing is the only honest answer.
    crossing_history: BTreeMap<String, VecDeque<CrossingSample>>,
    /// Where the cell's facts go.
    ///
    /// Given, never reached for: a cell assembled without one records into a
    /// registry nobody reads, which is what every test in the tree does. See
    /// [`crate::telemetry`] for why nothing here can block or fail the pass.
    metrics: CellMetrics,
    /// The §31.1 cross-region mirrors this cell takes part in, if an
    /// operator installed any.
    ///
    /// `None` is every cell before §31.1 and is still the default. It is not
    /// a permissive default: without it `mirror_facts_for` supplies no facts
    /// for a mirror edge and `eligible_paths` refuses the cycle whole, which
    /// is what a cell that cannot say how it would execute a cross-region
    /// cycle should do.
    mirror: Option<MirrorArrangement>,
    /// The capital this cell's region may commit, if a composition root gave
    /// it a table, and every hold against it — this cell's and, when the
    /// table is shared, its siblings'.
    ///
    /// `None` is the cell as it was before B12: every strategy bounded by its
    /// own signed envelope and nothing bounding their sum. It is not a
    /// silently permissive default — [`CellMetrics::region_allocation`] writes
    /// on every pass whether one is held, so a cell running without this
    /// bound says so in its series rather than looking like one that has it.
    /// Given, never reached for, like the metric registry: a cell that chose
    /// its own amount would be deciding how much it may risk, which is the
    /// one thing ADR 0008 says it never does.
    region_allocation: Option<RegionTable>,
    /// What this cell may still say to each venue this second (§29.2).
    ///
    /// One bucket per configured venue, fixed at assembly. A venue enforces
    /// a message rate and disconnects a session that exceeds it — at a
    /// moment nobody chose, leaving resting orders the cell can then no
    /// longer withdraw. The cell runs out of budget deliberately instead,
    /// keeping [`RateLimits::withdrawal_reserve`] back so that the mass
    /// cancel below is fundable after a burst of quoting.
    budget: QuoteBudget,
    /// How long each venue has taken from this cell's send to the venue's
    /// own confirmed fill, and the spread between them (§32.1).
    ///
    /// Measured from the cell's own orders and nothing else. Read by
    /// [`Cell::admit_cycle`], which refuses a cycle whose legs would arrive
    /// too far apart to be one position.
    fill_times: FillTimes,
    /// What this cell has been told about the other regions (§36.3).
    ///
    /// [`RegionOutlook::AllLit`] is the state every cell has run in and is
    /// not a permissive default in the dangerous sense: it is what a cell
    /// whose operator has declared nothing believes, and
    /// [`CellMetrics::regions_dark`] writes it on every pass, so a cell
    /// running with no region wire at all says so on a chart rather than
    /// looking like one whose peers are all answering.
    outlook: RegionOutlook,
    /// Cycles whose slow leg is resting at a venue while the rest of the
    /// cycle waits for it (§32.1's passive-first mechanism), by cycle id.
    ///
    /// The one piece of cell state that deliberately survives a pass. Every
    /// other thing `work` decides is committed or released before the pass
    /// ends — that is what the region-hold sweep at the top of `work`
    /// enforces — and this is the exception because the mechanism *is* the
    /// waiting. It is bounded by [`MAX_OPEN_ORDERS`] rather than by anything
    /// new: every entry holds exactly one open order, an entry whose order
    /// the cell no longer holds is removed the next time it is looked at, and
    /// a cycle already in here is refused a second admission.
    suspended: BTreeMap<String, SuspendedCycle>,
    /// The venues this cell must be shown an account of before it forms
    /// another order (§36.3), if a composition root armed the discipline.
    ///
    /// `None` is a cell that is not resuming from anything. Only a
    /// composition root can arm it, because only the composition root knows
    /// whether this process is a restart — see [`crate::resume`].
    resume: Option<ResumeDiscipline>,
}

/// How many reconciliation breaks a cell keeps for reporting.
const MAX_RETAINED_BREAKS: usize = 32;

impl Cell {
    /// Assemble a cell.
    ///
    /// The autonomy ceiling is paper trading and there is no constructor that
    /// takes another. A cell cannot raise its own ceiling; a live-capable cell
    /// is a differently-assembled deployment the central plane signs off, and
    /// the absence of that constructor here is what makes the claim true
    /// rather than merely intended.
    ///
    /// The `Result` is now earned. [`CellConfig::validate`] is the check this
    /// signature always advertised and, until it was added, did not perform:
    /// the body was one `Ok(Self { .. })` and the configuration was moved in
    /// unexamined, so no input could make assembly refuse.
    pub fn new(config: CellConfig, features: FeatureEngine) -> Result<Self> {
        config.validate()?;
        // Both are built from the validated venue list, so the bucket set
        // and the fill-time history are exactly the venues this cell may
        // reach: neither can be grown by an admission naming a venue the
        // cell was never configured for, which would be an unbounded label
        // as well as a venue with a fresh full budget every time its name
        // changed.
        let budget = QuoteBudget::new(config.quote_limits, &config.venues);
        let fill_times = FillTimes::new(config.dispersion, &config.venues);
        Ok(Self {
            protocols: ProtocolRegistry::new(),
            sequencer: Sequencer::new(ReorderPolicy::default()),
            liquidity: CellLiquidity::new(),
            features,
            deployed: BTreeMap::new(),
            autonomy: AutonomyController::new(),
            policy: None,
            policy_halted: false,
            policy_halt_barrier: None,
            polled_halt: None,
            dropcopy: DropCopyReconciler::new(),
            desk: None,
            journal: Journal::new(),
            working: BTreeMap::new(),
            confirmed: Vec::new(),
            positions: BTreeMap::new(),
            strategy_positions: BTreeMap::new(),
            strategy_cash: BTreeMap::new(),
            breaks: Vec::new(),
            breaks_omitted: 0,
            order_sequence: 0,
            pass: 0,
            crossing_history: BTreeMap::new(),
            metrics: CellMetrics::silent(),
            mirror: None,
            region_allocation: None,
            budget,
            fill_times,
            suspended: BTreeMap::new(),
            outlook: RegionOutlook::AllLit,
            resume: None,
            config,
        })
    }

    pub fn config(&self) -> &CellConfig {
        &self.config
    }

    /// Record into the composition root's registry rather than the silent one
    /// this cell was built with.
    ///
    /// Called once, in `qip-edge-node`, with the handle taken from the
    /// telemetry before it is used anywhere else — exactly as `qip-fastbrain`
    /// and `qip-deepbrain` install theirs. Taking a second registry here would
    /// produce a scrape surface that answers empty forever while the cell
    /// records diligently into one nothing can reach, which is the defect this
    /// seam exists to close rebuilt one level up.
    ///
    /// The halt gauge is written immediately so a cell that starts halted, and
    /// is scraped before its first pass, does not read as running.
    #[must_use]
    pub fn with_metrics(mut self, metrics: std::sync::Arc<qip_observability::Metrics>) -> Self {
        self.metrics = CellMetrics::new(metrics, &self.config.cell_id, &self.config.region);
        self.record_halt();
        // Both §36.3 gauges, for the reason the halt gauge is written here: a
        // cell assembled against a dark region, or armed to reconcile before
        // it resumes, and scraped before its first pass must not read as a
        // cell with neither.
        self.record_dark_regions();
        self.record_awaiting_reconciliation();
        self
    }

    /// Bound everything this cell commits by one amount.
    ///
    /// A builder rather than a [`CellConfig`] field, so a cell assembled
    /// without one keeps compiling and behaves exactly as it did. The bound
    /// can only ever narrow a deployment: a hold is taken *in addition to*
    /// the per-strategy envelope check that already ran, never instead of it.
    ///
    /// See [`crate::reservation`] for what this is and is not — in particular
    /// that the amount is an operator's, not a signed grant from the centre.
    pub fn with_region_allocation(self, amount: Decimal) -> Result<Self> {
        Ok(self.with_region_table(RegionTable::new(amount)?))
    }

    /// Bound everything this cell commits by a share the centre has yet to
    /// name, under an operator's ceiling (ADR 0039).
    ///
    /// The table opens funding nothing: every hold is refused under
    /// `region_reservation` until a verified policy payload's grant manifest
    /// names grants this cell holds, at which point [`Self::apply_policy`]
    /// re-bases the table to their gross. `ceiling` is the most the table
    /// will ever be bounded to whatever the centre names — the operator's
    /// backstop, which can only narrow. Contrast [`Self::with_region_allocation`],
    /// which funds the cell at the operator's number from the start: that is
    /// the number nothing at the centre ever checked against a region's
    /// grant, and two such cells under one grant could together spend it
    /// twice.
    pub fn with_unfunded_region(self, ceiling: Decimal) -> Result<Self> {
        Ok(self.with_region_table(RegionTable::unfunded(ceiling)?))
    }

    /// Hold against a table this cell shares with every other cell of its
    /// region — the blueprint's per-region reservation table (§26/§33).
    ///
    /// The root opens one [`RegionTable`] per region and hands a clone to
    /// each cell it assembles there, so a second cell's proposal is refused
    /// against the balance the first cell spent without either asking the
    /// centre. Holds are filed under this cell's id, so two cells running the
    /// same strategy on the same pass number do not collide, and one cell's
    /// pass-scoped sweep cannot return a hold its sibling is mid-pass on.
    #[must_use]
    pub fn with_region_table(mut self, table: RegionTable) -> Self {
        self.region_allocation = Some(table);
        self
    }

    /// What the region allocation has left, or `None` if this cell holds no
    /// allocation. The two are different facts and a zero would conflate them.
    pub fn region_allocation_free(&self) -> Option<Decimal> {
        self.region_allocation.as_ref().map(RegionTable::free)
    }

    /// Install a venue's settlement terms into a cell that is already
    /// running (§56.2 rule 21).
    ///
    /// What a composition root needs where the fact arrives with the seam
    /// that knows it rather than with the configuration: `qip-edge-node`
    /// binds the simulator's feed to the cell after assembly, and it is the
    /// simulator — not the operator — that knows its proceeds are credited
    /// on the fill. Refused for a venue this cell was not configured for:
    /// terms for a venue the cell cannot reach are a wiring error at a
    /// runtime seam, where the same entry in [`CellConfig::with_settlement`]
    /// is merely inert. Terms already held for the venue are replaced, and
    /// the replacement is journaled so a reader can see when the calendar
    /// a cycle was judged against changed.
    pub fn install_settlement(&mut self, venue: &VenueId, terms: SettlementTerms) -> Result<()> {
        if !self.config.venues.iter().any(|known| known == venue) {
            return Err(Error::invalid(format!(
                "cell {} is not configured for venue {}, so it has no leg there to project \
                 settlement for; name the venue in the cell's venue list before giving it terms",
                self.config.cell_id,
                venue.as_str()
            )));
        }
        self.config
            .settlement
            .insert(venue.as_str().to_string(), terms);
        Ok(())
    }

    /// Install the arbitrage desk this cell scans with.
    ///
    /// A builder rather than a constructor argument, like [`Self::with_metrics`],
    /// so [`Self::new`] stays the one way to assemble a cell and stays
    /// paper-only. Refused when the desk's envelope names another cell, or
    /// when its graph reaches a venue this cell may not: a cycle is priced
    /// against the cell's own books, and a venue absent from the cell's list
    /// has no book here to price against and no gateway here to send to.
    pub fn with_arbitrage(mut self, desk: ArbitrageDesk) -> Result<Self> {
        self.install_arbitrage(desk)?;
        Ok(self)
    }

    /// Install the desk into a cell that is already running.
    ///
    /// What a composition root needs, because the desk's two inputs arrive
    /// after the cell is assembled: the whitelist rides a policy payload and
    /// the desk's capital rides a grant, and neither is known at start-up.
    /// The same refusals as [`Self::with_arbitrage`], plus one: a cell that
    /// already holds a desk refuses a second, because replacing one would
    /// discard the utilisation the first has spent and hand the strategy its
    /// gross limit again.
    pub fn install_arbitrage(&mut self, desk: ArbitrageDesk) -> Result<()> {
        if self.desk.is_some() {
            return Err(Error::denied(
                "this cell already holds an arbitrage desk; a second would reset the capital \
                 the first has committed",
            ));
        }
        if desk.envelope().cell() != self.config.cell_id {
            return Err(Error::denied(format!(
                "an envelope for cell {} cannot fund the arbitrage desk at {}",
                desk.envelope().cell(),
                self.config.cell_id
            )));
        }
        for edge in desk.graph().edges() {
            for venue in [&edge.from.venue, &edge.to.venue] {
                if !self.config.venues.contains(venue) {
                    return Err(Error::denied(format!(
                        "conversion {} reaches {}, which this cell may not trade; a cycle \
                         through a venue the cell holds no book for cannot be priced here",
                        edge.label(),
                        venue.as_str()
                    )));
                }
            }
        }
        // §30.2's path router, built from what the cell already knows and
        // nothing else (ADR 0068): its own region, and the venues it may
        // trade. Built *before* the desk is stored, so a router that cannot
        // be built stops the desk installing rather than leaving a cell that
        // scans cycles it can never classify. That is the pairing
        // `path_router`'s own comment names, and it is the only place either
        // field is written.
        //
        // Two refusals travel out of here that did not before, and both are
        // configuration errors rather than market facts. `RegionId::new`
        // refuses a region id with surrounding whitespace — `CellConfig`'s
        // own check is `trim().is_empty()`, which admits `" us-east "`, and
        // two ids differing by a space are two regions to a mirror edge, so
        // the router will not normalise one away. `VenueRegions::all_in`
        // refuses an empty venue list, which `CellConfig::validate` already
        // rejects, so it is the belt to that braces.
        //
        // `RepresentationClasses::new()` is deliberately empty, and **§30.2's
        // rows 7 and 8 are therefore unreachable from a cell — but one layer
        // earlier than this**, which is the thing to get right before anybody
        // reads the empty map as the gate. `ArbitrageDesk::new` already
        // refuses a graph holding any synthetic edge outright: the cell has no
        // book to re-quote a synthetic from, so a cycle through one would be
        // priced on a template rate nobody observed. No desk a cell can hold
        // reaches a synthetic, so no composition this router sees carries a
        // basis or an equivalence edge.
        //
        // The empty map is therefore the second of two refusals, not the
        // first, and it is still worth supplying empty: if the desk ever
        // learns to price a synthetic, the router refuses it until somebody
        // states whether it is a basis or an equivalence, rather than routing
        // an options structure under a carry check. A default here would be
        // the guess ADR 0068 exists to refuse. See `mirror_facts_for` for
        // what rows 3 to 6 now need instead.
        //
        // **The region map is no longer every venue in the cell's own
        // region** (§31.1). It still starts that way — `VenueRegions::all_in`
        // is kept precisely for its refusal of an empty venue list, which is
        // the belt to `CellConfig::validate`'s braces — and then each venue
        // the operator annotated is moved to the region they named. The
        // annotation is checked against `self.config.venues` first, and that
        // check is the one thing standing between "this venue is abroad" and
        // "this venue exists": a `venue_regions` entry naming a venue the
        // cell may not trade is refused here rather than silently placing a
        // venue the cell has no book for. `CellConfig::with_venue_in_region`
        // cannot produce one, but the field is `pub` and the builder is
        // skippable, so the runtime check is not redundant with it.
        let home = RegionId::new(self.config.region.as_str())?;
        let mut regions = VenueRegions::all_in(home, &self.config.venues)?;
        for (venue, region) in &self.config.venue_regions {
            let venue = VenueId::new(venue.as_str());
            if !self.config.venues.contains(&venue) {
                return Err(Error::denied(format!(
                    "venue {} is placed in region {region} and is not one this cell may trade; \
                     a region annotation says where a venue is and never that the cell may \
                     reach it, so add it to QIP_VENUES deliberately or remove the annotation",
                    venue.as_str()
                )));
            }
            regions = regions.with(venue, RegionId::new(region.as_str())?);
        }
        let router = CycleRouter::new(PathPolicy::default(), regions, RepresentationClasses::new());
        self.desk = Some(InstalledDesk { desk, router });
        Ok(())
    }

    /// Install the §31.1 mirror arrangement this cell takes part in.
    ///
    /// Refused twice over rather than replaced: a second arrangement would
    /// move a band under a cycle already priced against the first, and an
    /// arrangement naming a round trip to the cell's own region describes a
    /// mirror edge that cannot exist — `CompositionEdge::new` refuses a
    /// mirror whose ends share a region, so the entry could only ever be
    /// looked up by a lookup that never happens.
    pub fn install_mirror(&mut self, arrangement: MirrorArrangement) -> Result<()> {
        if self.mirror.is_some() {
            return Err(Error::denied(
                "this cell already holds a mirror arrangement; a second would move a band under \
                 a cycle already gated against the first",
            ));
        }
        if arrangement.is_empty() {
            return Err(Error::invalid(
                "a mirror arrangement naming no instrument gates nothing and would leave every \
                 cross-region cycle refused for a missing band; name the instruments this \
                 region mirrors, or install no arrangement at all",
            ));
        }
        if arrangement.round_trip(self.config.region.as_str()).is_ok() {
            return Err(Error::denied(format!(
                "a round trip to this cell's own region {} was recorded; a mirror edge is one \
                 asset in two regions and the router refuses one whose ends share a region, so \
                 that measurement could never be read",
                self.config.region
            )));
        }
        self.mirror = Some(arrangement);
        Ok(())
    }

    /// The mirror arrangement, if one is installed.
    pub const fn mirror(&self) -> Option<&MirrorArrangement> {
        self.mirror.as_ref()
    }

    // --- §36.3: a region that has gone dark ---------------------------------

    /// Apply what the cell has been told about the other regions.
    ///
    /// Handed a reading, exactly as [`Self::apply_polled_halt`] is: the cell
    /// reads no file and opens no socket, so the same seam is driven by a
    /// test with nothing mounted anywhere. A cell cannot work this out for
    /// itself — the venues in another region answer whether or not the cell
    /// that trades them is alive — and a cell that inferred a peer's death
    /// from its own sessions would be publishing a claim about a process it
    /// has never spoken to.
    ///
    /// Idempotent: a reading identical to the one in force is no event, so
    /// polling this every pass writes one chain entry per change rather than
    /// one per pass.
    pub fn apply_region_outlook(&mut self, outlook: RegionOutlook, now: Timestamp) {
        if outlook != self.outlook {
            self.journal.record(
                Decision::RegionOutlookChanged {
                    source: outlook.source().map_or_else(
                        || "all_lit".to_string(),
                        |source| source.as_str().to_string(),
                    ),
                    regions: outlook.named(),
                    detail: outlook.describe(),
                },
                now,
            );
            self.outlook = outlook;
        }
        self.record_dark_regions();
    }

    /// What the cell has been told about the other regions.
    pub const fn region_outlook(&self) -> &RegionOutlook {
        &self.outlook
    }

    /// Whether `region` is dark to this cell.
    ///
    /// A cell is never dark to itself, whatever the reading says: it is the
    /// process asking, and a cell that read itself as dark would suspend the
    /// local half of §36.3's "everything else" column too.
    pub fn is_region_dark(&self, region: &str) -> bool {
        self.outlook.is_dark(region, &self.config.region)
    }

    /// How many of the regions this cell's own venue map places abroad are
    /// dark, which is how many regions its mirrors are suspended into.
    ///
    /// Counted over the venue map rather than over the reading, so the
    /// number says what it costs this cell. A reading naming six regions
    /// this cell has no venue in suspends nothing here, and a gauge carrying
    /// the six would have an operator hunting for mirrors that never
    /// existed.
    fn dark_foreign_regions(&self) -> usize {
        let home = self.config.region.as_str();
        self.config
            .venue_regions
            .values()
            .filter(|region| region.as_str() != home)
            .collect::<std::collections::BTreeSet<&String>>()
            .into_iter()
            .filter(|region| self.is_region_dark(region))
            .count()
    }

    fn record_dark_regions(&self) {
        self.metrics
            .regions_dark(self.outlook.source(), self.dark_foreign_regions());
    }

    // --- §36.3: reconciling against every venue before resuming --------------

    /// Refuse to form an order until every venue has been reconciled against.
    ///
    /// Armed by the composition root when it finds evidence that this
    /// process is a restart of a cell that ran before — the node reads that
    /// from its own journal store. It is not armed by the cell, and
    /// [`crate::resume`] says why: a cell that armed itself on every start
    /// would wait for ever on the first start of a node that has never sent
    /// anything, which is a gate that cannot open rather than a control that
    /// fires.
    ///
    /// Refuses a second arming rather than resetting the first. A cell that
    /// silently re-armed would throw away the venues that had already
    /// answered, and an operator watching the pending list would see it grow
    /// back for no reason they could name.
    pub fn require_reconciliation_before_resuming(
        &mut self,
        reason: impl Into<String>,
        now: Timestamp,
    ) -> Result<()> {
        if let Some(existing) = &self.resume {
            return Err(Error::denied(format!(
                "this cell is already reconciling before it resumes ({}), with {} venue(s) still \
                 to answer; a second arming would discard the accounts already agreed",
                existing.reason(),
                existing.pending().len()
            )));
        }
        let discipline = ResumeDiscipline::new(reason, self.config.venues.iter().cloned())?;
        self.journal.record(
            Decision::ReconciliationRequired {
                reason: discipline.reason().to_string(),
                venues: discipline.pending(),
            },
            now,
        );
        self.resume = Some(discipline);
        self.record_awaiting_reconciliation();
        Ok(())
    }

    /// The discipline in force, if the cell is still reconciling.
    pub const fn awaiting_reconciliation(&self) -> Option<&ResumeDiscipline> {
        self.resume.as_ref()
    }

    /// Compare one venue's own account of what it holds open for this cell
    /// against the cell's record, and clear that venue if the two agree.
    ///
    /// The only thing that clears a venue. Nothing the cell computes about
    /// itself can: a cell that cleared its own gate would be asserting the
    /// very fact it was asked to prove.
    ///
    /// A disagreement is a reconciliation break, which halts the cell and is
    /// never auto-corrected — §36.3's own row for a break, and §48's "human
    /// investigation". That is the failure this whole discipline exists to
    /// find: a restarted process rebuilds its books from the feed and its
    /// journal from genesis, so an order the dead process left resting is
    /// one nothing in this platform can see, withdraw or attribute a fill
    /// on, and the ordinary reconciler cannot find it because the cell's
    /// side of that comparison is empty and agreeing with nothing reads as
    /// agreement.
    ///
    /// Refused outside the resume window rather than compared anyway: the
    /// working set moves within a pass — an order sent, a fill booked — and
    /// a comparison against a venue's account taken at some other instant
    /// would halt a healthy cell on a difference that is just the clock.
    pub fn observe_venue_account(&mut self, account: VenueAccount, now: Timestamp) -> Result<()> {
        let Some(discipline) = self.resume.as_ref() else {
            return Err(Error::denied(format!(
                "this cell is not reconciling before it resumes, so an account from {} would be \
                 compared against a working set this pass is still changing; arm the discipline \
                 first, or reconcile through the drop-copy channel",
                account.venue().as_str()
            )));
        };
        if !self.config.venues.contains(account.venue()) {
            return Err(Error::invalid(format!(
                "{} is not a venue this cell was configured for, so an account from it says \
                 nothing about what this cell left open; supply an account for one of the {} \
                 venues in the cell's own list",
                account.venue().as_str(),
                self.config.venues.len()
            )));
        }
        let awaited = discipline.awaits(account.venue());
        let mine: BTreeMap<String, Decimal> = self
            .working
            .iter()
            .filter(|(_, working)| {
                working.order.closed.is_none() && &working.order.venue == account.venue()
            })
            .map(|(order_id, working)| (order_id.clone(), working.order.remaining()))
            .collect();
        let venue = account.venue().as_str().to_string();
        let mut findings: Vec<String> = Vec::new();
        for (order_id, remaining) in account.open() {
            match mine.get(order_id) {
                None => findings.push(format!(
                    "{venue} is holding order {order_id} open for {remaining} and this cell has \
                     no record of sending it; a restarted cell can neither withdraw it nor \
                     attribute a fill on it"
                )),
                Some(ours) if ours != remaining => findings.push(format!(
                    "{venue} says order {order_id} has {remaining} open and this cell says {ours}"
                )),
                Some(_) => {}
            }
        }
        for (order_id, ours) in &mine {
            if !account.open().contains_key(order_id) {
                findings.push(format!(
                    "this cell holds order {order_id} open for {ours} at {venue} and the venue's \
                     account does not name it"
                ));
            }
        }
        if account.quotes() > 0 {
            // §48 asks for "resting orders and quotes". The cell keeps no
            // quote inventory of its own, so any quote the venue holds live
            // for it is exposure with no owner in this process — the same
            // finding as an unknown resting order, and reported as one
            // rather than left out because there is no field to compare it
            // against.
            findings.push(format!(
                "{venue} is holding {} quote(s) live for this cell and the cell keeps no quote of \
                 its own; something is quoting in this cell's name",
                account.quotes()
            ));
        }
        if !findings.is_empty() {
            for finding in findings {
                self.break_on(finding, now);
            }
            return Ok(());
        }
        if !awaited {
            // A venue that has already answered, answering again. Clean, and
            // deliberately not journaled: the node offers this every pass
            // while the discipline stands, and one chain entry per pass for
            // a venue that cleared three passes ago is noise in the record
            // an incident review reads.
            return Ok(());
        }
        let (pending, resumed) = match self.resume.as_mut() {
            Some(discipline) => {
                discipline.answered(account.venue());
                (discipline.pending(), discipline.is_satisfied())
            }
            None => (Vec::new(), true),
        };
        self.journal.record(
            Decision::VenueReconciled {
                venue,
                open: account.open().len(),
                quotes: account.quotes(),
                pending,
                resumed,
            },
            now,
        );
        if resumed {
            self.resume = None;
        }
        self.record_awaiting_reconciliation();
        Ok(())
    }

    fn record_awaiting_reconciliation(&self) {
        self.metrics.awaiting_reconciliation(
            self.resume
                .as_ref()
                .map_or(0, |discipline| discipline.pending().len()),
        );
    }

    /// The tenth policy slot's inventory targets and the instant they were
    /// produced, **whatever their freshness**.
    ///
    /// Deliberately unlike [`Self::cycle_whitelist`], which reads stale as
    /// none. The window this slot is read under is §33.1's own
    /// "reference inside TTL" check, which
    /// `qip_routing::extension::check` performs on the
    /// `DistributedReference` built from `produced_at`. Filtering here as
    /// well would mean the extension's window arm could never be reached
    /// from a cell — a control that reads as protection and cannot fire —
    /// and would replace a refusal naming a stale reference with one naming
    /// a missing band.
    pub fn inventory_targets(
        &self,
    ) -> Option<(&qip_contracts::policy::InventoryTargets, Timestamp)> {
        let policy = self.policy.as_ref()?;
        let slot = &policy.payload().inventory_targets;
        Some((slot.value()?, slot.produced_at()?))
    }

    /// The cycle whitelist the applied policy carries, while it is fresh.
    ///
    /// Fresh only: the slot's own time-to-live is a minute, and a desk built
    /// from a whitelist the centre has stopped republishing would price a
    /// graph the centre may since have withdrawn. Stale reads as none.
    pub fn cycle_whitelist(
        &self,
        now: Timestamp,
    ) -> Option<&qip_contracts::policy::CycleWhitelist> {
        let policy = self.policy.as_ref()?;
        if policy
            .payload()
            .freshness(qip_contracts::policy::PolicyItem::CycleWhitelist, now)
            != qip_contracts::degradation::Freshness::Fresh
        {
            return None;
        }
        policy.payload().cycle_whitelist.value()
    }

    /// The compiled plan the last applied payload names (§41.5 item 2),
    /// while that slot is fresh.
    ///
    /// Fresh only, exactly as [`Self::cycle_whitelist`]: a stale plan is a
    /// plan the centre has stopped vouching for, and a node that deployed
    /// from it would be running strategies on the strength of a payload
    /// whose every other slot has already narrowed the cell. The slot names
    /// the plan by digest and count and carries no strategy itself; whoever
    /// holds the plan's bytes checks them against this before deploying.
    pub fn compiled_plan(&self, now: Timestamp) -> Option<&qip_contracts::policy::PlanDigest> {
        let policy = self.policy.as_ref()?;
        if policy
            .payload()
            .freshness(qip_contracts::policy::PolicyItem::CompiledPlan, now)
            != qip_contracts::degradation::Freshness::Fresh
        {
            return None;
        }
        policy.payload().compiled_plan.value()
    }

    /// The installed arbitrage desk, if any.
    pub fn arbitrage(&self) -> Option<&ArbitrageDesk> {
        self.desk.as_ref().map(|installed| &installed.desk)
    }

    /// Publish the halt state as it now stands.
    ///
    /// Called wherever either halt can change, rather than once per pass: a
    /// cell halted by a reconciliation break stops running passes, so a gauge
    /// written only inside `work` would never report the halt that stopped it.
    fn record_halt(&self) {
        self.metrics.halt(
            self.autonomy.kill_switch().is_globally_tripped(),
            self.policy_halted,
            self.polled_halt.is_some(),
        );
    }

    pub fn protocols_mut(&mut self) -> &mut ProtocolRegistry {
        &mut self.protocols
    }

    pub fn journal(&self) -> &Journal {
        &self.journal
    }

    /// The registry every series this cell records lands in.
    ///
    /// Exposed so a composition root's test can prove, by pointer identity,
    /// that it is the same registry the scrape surface serves. A cell that
    /// records into one registry while the health thread serves another
    /// answers every scrape empty forever, and nothing at runtime reports it.
    pub fn metrics_registry(&self) -> &std::sync::Arc<qip_observability::Metrics> {
        self.metrics.registry()
    }

    pub fn autonomy(&self) -> &AutonomyController {
        &self.autonomy
    }

    pub fn autonomy_mut(&mut self) -> &mut AutonomyController {
        &mut self.autonomy
    }

    pub fn liquidity(&self) -> &CellLiquidity {
        &self.liquidity
    }

    pub fn dropcopy_mut(&mut self) -> &mut DropCopyReconciler {
        &mut self.dropcopy
    }

    /// Fills the venue has confirmed and the cell has not yet settled.
    ///
    /// Confirmed means reported by the order-entry channel; an order the
    /// venue accepted and has not filled is in [`Self::open_orders`] and not
    /// here. Settled fills leave this list and stay in the journal.
    pub fn fills(&self) -> &[ConfirmedFill] {
        &self.confirmed
    }

    /// Orders the venue accepted and the cell has not settled, in order-id
    /// order — including closed ones awaiting a clean reconciliation.
    pub fn open_orders(&self) -> Vec<OpenOrder> {
        self.working
            .values()
            .map(|working| working.order.clone())
            .collect()
    }

    /// The signed quantity confirmed filled on one instrument at one venue:
    /// bought positive, sold negative. Zero for an instrument nothing has
    /// filled on, however much is resting there.
    pub fn position(&self, venue: &VenueId, object_id: &ObjectId) -> Decimal {
        self.positions
            .get(&Self::position_key(venue, object_id))
            .copied()
            .unwrap_or(Decimal::ZERO)
    }

    fn position_key(venue: &VenueId, object_id: &ObjectId) -> String {
        format!("{}/{}", venue.as_str(), object_id.as_str())
    }

    /// The signed lot one strategy holds on one instrument at one venue at
    /// this cell: bought positive, sold negative. From two sources, both
    /// facts of this cell — internal crosses settled here, and the
    /// strategy's pro-rata share of every fill the venue confirmed here.
    ///
    /// The fill share was not booked here until ADR 0080, and the reason it
    /// is now is the reason that record gives: the cell unwinds a retired
    /// strategy's lot **against the lot it holds for that strategy**, and a
    /// book that held only the crossed part would have refused every
    /// disposition for a lot built at a venue as "holds nothing". The same
    /// fills are attributed at the centre (`CentralPlane::ingest`) from the
    /// same shares, so this is one fact — the fill — read at both ends and
    /// not a second source of truth: the centre's lot for a strategy at this
    /// cell and this book agree by construction, and the sum of this book
    /// over the strategies at one venue is [`Self::position`] there, since
    /// the crosses net to zero.
    pub fn strategy_position(
        &self,
        strategy: &StrategyId,
        venue: &VenueId,
        object_id: &ObjectId,
    ) -> Decimal {
        self.strategy_positions
            .get(&Self::strategy_position_key(strategy, venue, object_id))
            .copied()
            .unwrap_or(Decimal::ZERO)
    }

    /// The cash one strategy has paid (negative) or received (positive) for
    /// its crossed lots at this cell, at the mid each cross was journaled at.
    /// Sums to zero across the strategies of every cross, because a cross
    /// moves money between two of the cell's own books and nowhere else.
    pub fn strategy_cash(&self, strategy: &StrategyId) -> Decimal {
        self.strategy_cash
            .get(strategy.as_str())
            .copied()
            .unwrap_or(Decimal::ZERO)
    }

    /// The signed lot one strategy holds on one instrument at this cell,
    /// summed over every venue the cell reaches — the quantity ADR 0080's
    /// disposition is judged against, because the centre keys its books by
    /// cell, strategy and instrument and names no venue.
    pub fn strategy_lot(&self, strategy: &StrategyId, object_id: &ObjectId) -> Decimal {
        self.config
            .venues
            .iter()
            .map(|venue| self.strategy_position(strategy, venue, object_id))
            .sum()
    }

    fn strategy_position_key(
        strategy: &StrategyId,
        venue: &VenueId,
        object_id: &ObjectId,
    ) -> String {
        format!(
            "{}/{}",
            strategy.as_str(),
            Self::position_key(venue, object_id)
        )
    }

    /// Whether the cell is stopped, by any of its three halts.
    pub fn is_halted(&self) -> bool {
        self.autonomy.kill_switch().is_globally_tripped()
            || self.policy_halted
            || self.polled_halt.is_some()
    }

    /// The reason the polled halt wire is engaged, while it is.
    pub fn polled_halt(&self) -> Option<&str> {
        self.polled_halt.as_deref()
    }

    /// Apply what the polled halt flag read as, this poll.
    ///
    /// The flag is the state: engaged or unreadable halts, absent or
    /// released does not, and every poll re-applies it. That is the
    /// opposite discipline from [`Self::apply_halt`], whose broadcast is
    /// engage-only and released by a newer signed payload, and the
    /// difference is the point. §46.2 asks for two paths that do not share
    /// a failure: the broadcast fails when the mesh does, and a mesh
    /// failure cannot reach this one, because this one is a file on the
    /// node that nothing on the mesh writes or clears. Neither halt can
    /// release the other.
    ///
    /// Unreadable halts. A flag that exists but cannot be read — the mount
    /// is gone, the permission is wrong, the content is not one of the two
    /// words — is a wire whose state is unknown, and a kill switch whose
    /// state is unknown must read as engaged ("stale is treated as engaged",
    /// §46.2). Reading it as absent would let the failure of the mount be
    /// the release of the halt.
    pub fn apply_polled_halt(&mut self, reading: PolledHalt, now: Timestamp) {
        let halting = reading.halts();
        match (self.polled_halt.is_some(), halting) {
            (false, true) => {
                let reason = format!("polled halt: {}", reading.describe());
                self.journal.record(
                    Decision::HaltChanged {
                        halted: true,
                        reason: reason.clone(),
                    },
                    now,
                );
                self.polled_halt = Some(reason);
            }
            (true, false) => {
                self.polled_halt = None;
                // `halted` names the cell, not the wire: the other two halts
                // may still hold it, and a reader of the chain must not take
                // this entry for a cell that resumed.
                let halted = self.is_halted();
                self.journal.record(
                    Decision::HaltChanged {
                        halted,
                        reason: format!(
                            "the polled halt flag {}; the cell is {}",
                            reading.describe(),
                            if halted {
                                "still halted by another wire"
                            } else {
                                "released"
                            }
                        ),
                    },
                    now,
                );
            }
            // Idempotent in both steady states: a flag re-read as engaged is
            // one halt, and a flag re-read as absent is no event.
            (true, true) | (false, false) => {}
        }
        self.record_halt();
    }

    /// The degradation narrowing currently in force, derived from the applied
    /// policy payload.
    ///
    /// With no policy this is [`DegradationState::nothing_known`], which reads
    /// every payload-fed capability as unavailable — the fail-closed floor.
    /// Ingestion is deliberately not observed here: the cell's book-staleness
    /// seam already refuses to route on a stale book, per book, which is
    /// stricter than the capability-level pause would be.
    pub fn narrowing(&self, now: Timestamp) -> DegradationState {
        match &self.policy {
            Some(policy) => policy.payload().narrowing(now),
            None => DegradationState::nothing_known(),
        }
    }

    /// The sequence of the applied policy, if any.
    pub fn policy_sequence(&self) -> Option<u64> {
        self.policy.as_ref().map(VerifiedPolicy::sequence)
    }

    /// Apply a verified halt command.
    ///
    /// Engage-only and idempotent: there is no release command, because
    /// release is a fresh policy decision and rides a newer signed payload
    /// issued after the barrier this records. Applying the same halt twice is
    /// one halt.
    pub fn apply_halt(&mut self, halt: VerifiedHalt, now: Timestamp) {
        // A halt at or behind the barrier of one already resolved is a
        // replay: a captured frame re-delivered after a legitimate release
        // would otherwise re-halt the cell in the gaps between publishes — a
        // bounded denial of service in the safe direction, but free to
        // remove. The asymmetry is preserved with care: a *fresh* halt is
        // accepted unconditionally, and an already-halted cell is never
        // released by this path — refusing the replay below leaves it exactly
        // as halted as it was.
        if !self.policy_halted
            && self
                .policy_halt_barrier
                .is_some_and(|barrier| halt.issued_at() <= barrier)
        {
            self.journal.record(
                Decision::Refused {
                    gate: "halt_replay".to_string(),
                    reason: format!(
                        "a halt issued at {} is at or behind the resolved barrier and does not \
                         re-halt this cell",
                        halt.issued_at()
                    ),
                },
                now,
            );
            return;
        }
        let barrier = match self.policy_halt_barrier {
            Some(existing) if existing >= halt.issued_at() => existing,
            _ => halt.issued_at(),
        };
        self.policy_halt_barrier = Some(barrier);
        if !self.policy_halted {
            self.journal.record(
                Decision::HaltChanged {
                    halted: true,
                    reason: format!("central halt: {}", halt.reason()),
                },
                now,
            );
        }
        self.policy_halted = true;
        self.record_halt();
    }

    /// Apply a verified policy payload by atomic swap.
    ///
    /// "Atomic" in a single-threaded cell means one assignment and never a
    /// partial application: the payload was verified whole before this became
    /// callable — [`VerifiedPolicy`]'s only constructor recomputes the
    /// signature — and nothing below reads a slot before the swap. Trading is
    /// never paused; the previous policy serves until the assignment.
    ///
    /// Sequence discipline lives here because this is where "last applied" is
    /// a fact: a payload at or below the applied sequence is refused, which is
    /// what stops a replayed old payload from un-halting or re-widening the
    /// cell.
    pub fn apply_policy(&mut self, verified: VerifiedPolicy, now: Timestamp) -> Result<()> {
        if verified.payload().cell != self.config.cell_id {
            return Err(Error::denied(format!(
                "a policy payload for cell {} cannot apply to {}",
                verified.payload().cell,
                self.config.cell_id
            )));
        }
        if let Some(applied) = self.policy_sequence()
            && verified.sequence() <= applied
        {
            return Err(Error::denied(format!(
                "policy sequence {} is not newer than the applied {applied}; an old payload \
                 cannot re-widen or un-halt this cell",
                verified.sequence()
            )));
        }

        // A payload that would release the halt must postdate the halt it
        // releases. `halting` is what the cell will actually do, which may be
        // stricter than what the payload says.
        let releasing_too_early = !verified.halted()
            && self.policy_halted
            && self
                .policy_halt_barrier
                .is_some_and(|barrier| verified.payload().issued_at <= barrier);
        let halting = verified.halted() || releasing_too_early;
        let sequence = verified.sequence();
        let was_halted = self.policy_halted;
        let narrowed: Vec<String> = verified
            .payload()
            .narrowing(now)
            .narrowed()
            .iter()
            .map(|(capability, freshness)| {
                format!("{}:{}", capability.as_str(), freshness.as_str())
            })
            .collect();

        self.journal.record(
            Decision::PolicyApplied {
                sequence: verified.sequence(),
                halted: halting,
                narrowed,
            },
            now,
        );
        if releasing_too_early {
            self.journal.record(
                Decision::Refused {
                    gate: "halt_release".to_string(),
                    reason: format!(
                        "policy sequence {} was issued at or before the halt barrier and cannot \
                         release it",
                        verified.sequence()
                    ),
                },
                now,
            );
        }
        if halting != was_halted {
            self.journal.record(
                Decision::HaltChanged {
                    halted: halting,
                    reason: if halting {
                        "the centre halted this cell through policy".to_string()
                    } else {
                        format!(
                            "policy sequence {} released the central halt",
                            verified.sequence()
                        )
                    },
                },
                now,
            );
        }
        if verified.halted() {
            // A halt carried by policy is a halt decision like any other, and
            // it raises the same release barrier: whatever releases it must
            // postdate it, whatever its sequence says.
            self.policy_halt_barrier = Some(match self.policy_halt_barrier {
                Some(existing) if existing >= verified.payload().issued_at => existing,
                _ => verified.payload().issued_at,
            });
        }
        self.policy_halted = halting;
        self.policy = Some(verified);
        self.record_halt();
        // The sequence the cell has *applied*, recorded once the swap has
        // happened. Recording it before would publish a payload the cell might
        // still have refused.
        self.metrics.policy_applied(sequence);
        // After the swap, so the share is applied from a payload the cell has
        // already accepted whole and never from one it went on to refuse.
        self.apply_region_share(sequence, now);
        Ok(())
    }

    /// Re-base the region table to this cell's share of its region's grant,
    /// as the applied payload's grant manifest names it (ADR 0039).
    ///
    /// The share is not a number on the wire. The manifest names the
    /// signatures of the grants the centre believes live for this cell, and
    /// the share is the sum of `gross_limit` over the verified, deployed,
    /// still-live envelopes among them — the one signed fact about each grant
    /// the cell already holds, so the share and the envelopes are one claim
    /// from one source rather than two that can disagree. The centre ships a
    /// manifest only when that sum fits inside the cell's disjoint share of
    /// the region's grant (`CentralPlane::region_shares`), so the sum here can
    /// only be at or below the share the centre computed.
    ///
    /// Three stated cases. A produced manifest naming none of this cell's
    /// grants is a share of nothing, and the table narrows to nothing: a
    /// cell absent from the shares books nothing. An unproduced slot leaves
    /// the table as it was — the centre said nothing about capital, which
    /// cannot widen a table and, for a table opened unfunded, has nothing to
    /// narrow. A cell with no table has nothing to re-base and records
    /// nothing, exactly as it holds nothing on a pass.
    fn apply_region_share(&mut self, sequence: u64, now: Timestamp) {
        let Some((table, share, grants)) = self.derive_region_share(now) else {
            // Charted only for a cell that holds a table. A cell with no table
            // has no share for the centre to withhold, and counting one for it
            // would put every unfunded cell in the tree on a series meant to
            // say that a downlink has gone quiet about capital.
            if self.region_allocation.is_some() {
                self.metrics.region_share(RegionShareOutcome::Withheld);
            }
            return;
        };
        // Read before the ledger is asked, because a refusal changes nothing
        // and this is the only fact that classifies one. The label names the
        // condition the cell verified here — a share offered under a sequence
        // no newer than the one the ledger already holds — and not a parse of
        // the refusal's message. A refusal the cell cannot classify this way
        // is journaled below and left off the series rather than filed under
        // a cause nobody established.
        let replayed = table
            .share_sequence()
            .is_some_and(|applied| sequence <= applied);
        let outcome = table.rebase(&self.config.cell_id, share, sequence);
        self.record_region_share(
            outcome,
            RegionShareOutcome::Applied,
            replayed.then_some(RegionShareOutcome::RefusedLowerSequence),
            sequence,
            grants,
            now,
        );
    }

    /// Sum the applied manifest again under its own sequence, because the
    /// grants this cell holds have changed (ADR 0039).
    ///
    /// Called after a grant is deployed, renewed or withdrawn. The node
    /// applies a payload before it deploys the plan that payload names, so
    /// the first sum over the manifest finds no grant and the table narrows
    /// to nothing; when the grant lands the same signed manifest is summed
    /// once more, under the same sequence, and the table funds. A withdrawal
    /// or a renewal under a new signature narrows by the same arithmetic,
    /// which is the safe direction. Nothing here can widen past the payload:
    /// only grants the manifest names are counted, and the centre ships a
    /// manifest only when their gross fits the share it computed. A table no
    /// payload has funded yet is left alone — a share arrives with a payload
    /// and never from a grant by itself — and the ledger refuses the
    /// re-derivation on its own if this check is ever wrong.
    fn rederive_region_share(&mut self, now: Timestamp) {
        let Some(sequence) = self.policy_sequence() else {
            return;
        };
        if self.region_share_sequence() != Some(sequence) {
            return;
        }
        let Some((table, share, grants)) = self.derive_region_share(now) else {
            return;
        };
        let outcome = table.rederive(&self.config.cell_id, share, sequence);
        // No refusal outcome: a re-derivation is *defined* at the sequence
        // already applied, so "no newer than the applied" is its normal state
        // and cannot classify anything here. The refusals this call can
        // return are about whose share the table holds, which the journal
        // names and the `outcome` label deliberately does not guess at.
        self.record_region_share(
            outcome,
            RegionShareOutcome::Rederived,
            None,
            sequence,
            grants,
            now,
        );
    }

    /// The share the applied manifest names for this cell: the gross of the
    /// verified, deployed, still-live envelopes among the grants it lists,
    /// and how many there were. `None` when there is no table to re-base or
    /// no produced manifest to sum.
    fn derive_region_share(&mut self, now: Timestamp) -> Option<(RegionTable, Decimal, usize)> {
        let table = self.region_allocation.clone()?;
        let manifest = self
            .policy
            .as_ref()
            .and_then(|policy| policy.payload().capital_grants.value())?;
        let mut share = Decimal::ZERO;
        let mut grants = 0usize;
        for deployed in self.deployed.values() {
            let envelope = &deployed.envelope;
            if !envelope.is_live(now)
                || !manifest
                    .live_grants
                    .iter()
                    .any(|named| named == envelope.signature())
            {
                continue;
            }
            match share.checked_add(envelope.gross_limit()) {
                Some(sum) => share = sum,
                None => {
                    // A share the cell cannot compute funds nothing: the
                    // table narrows to zero rather than to a number the cell
                    // guessed at, and the reason is journaled beside it.
                    self.journal.record(
                        Decision::Refused {
                            gate: "region_share".to_string(),
                            reason: format!(
                                "the grants named for this cell cannot be summed past {share}; \
                                 the region share is taken as nothing"
                            ),
                        },
                        now,
                    );
                    share = Decimal::ZERO;
                    grants = 0;
                    break;
                }
            }
            grants += 1;
        }
        Some((table, share, grants))
    }

    /// Journal what a re-base or a re-derivation did, and chart the balance,
    /// the bound and which of the two it was.
    ///
    /// `accepted` is what the caller's own call was — a re-base or a
    /// re-derivation — and `refused` is the outcome to chart if the ledger
    /// turned it down, or `None` where the caller cannot say why a refusal
    /// happened. A refusal nobody can attribute is journaled and left off the
    /// series: an `outcome` label naming a cause the cell did not establish
    /// would read on a chart as a finding.
    fn record_region_share(
        &mut self,
        outcome: Result<crate::reservation::Rebase>,
        accepted: RegionShareOutcome,
        refused: Option<RegionShareOutcome>,
        sequence: u64,
        grants: usize,
        now: Timestamp,
    ) {
        match outcome {
            Ok(rebase) => {
                self.journal.record(
                    Decision::RegionShareApplied {
                        sequence,
                        grants,
                        share: rebase.share.to_string(),
                        bound: rebase.bound.to_string(),
                        free: rebase.free.to_string(),
                        deficit: rebase.deficit.to_string(),
                    },
                    now,
                );
                self.metrics.region_allocation(Some(rebase.free));
                // The bound the ledger now enforces, from the ledger's own
                // answer rather than from the share that was offered: the
                // ceiling may have capped it, and charting the offer would
                // report a number this cell will never be allowed to commit.
                self.metrics.region_share_bound(rebase.bound);
                self.metrics.region_share(accepted);
            }
            Err(error) => {
                // The payload's own sequence check ran above, so this fires
                // only when the ledger knows something the cell does not — a
                // sibling re-based a table that was meant to be private, or a
                // share the centre's plan could not have produced.
                self.journal.record(
                    Decision::Refused {
                        gate: "region_share".to_string(),
                        reason: error.message().to_string(),
                    },
                    now,
                );
                if let Some(refused) = refused {
                    self.metrics.region_share(refused);
                }
            }
        }
    }

    /// The bound the region table enforces now, or `None` if this cell holds
    /// no table. Distinct from [`Self::region_allocation_free`]: the bound is
    /// what the cell may commit in total, and free is what is left of it.
    pub fn region_allocation_bound(&self) -> Option<Decimal> {
        self.region_allocation.as_ref().map(RegionTable::bound)
    }

    /// The sequence of the last share the region table applied, if a table
    /// is held and a share ever was.
    pub fn region_share_sequence(&self) -> Option<u64> {
        self.region_allocation
            .as_ref()
            .and_then(RegionTable::share_sequence)
    }

    /// The operator's ceiling on the region table, which no share can raise,
    /// or `None` if this cell holds no table.
    pub fn region_allocation_ceiling(&self) -> Option<Decimal> {
        self.region_allocation.as_ref().map(RegionTable::ceiling)
    }

    /// Track an instrument at a venue.
    ///
    /// A state inserted here replaces whatever the cell held for the pair
    /// wholesale, so the desk is told to forget which books it has read: a
    /// replacement can carry the same `(observed_at, observations)` pair as
    /// the state it displaced while showing a different touch, and §30.1's
    /// affected-edge filter would otherwise keep a rate no book holds.
    pub fn track(&mut self, state: VenueState) {
        self.liquidity.insert(state);
        if let Some(installed) = self.desk.as_mut() {
            installed.desk.forget_books();
        }
    }

    /// Deploy a strategy, the program its plan indexes into, and the verified
    /// capital envelope it runs under.
    ///
    /// The envelope is the verified type, so a strategy cannot be deployed
    /// against a grant nobody signed. The program is taken here rather than at
    /// assembly for the reason that made this call worth changing: a cell used
    /// to be constructed with an empty arena, so a strategy whose plan pointed
    /// into a real one could be deployed, accepted, and then refuse on every
    /// pass of `work` — a cell that looked healthy, held a strategy, and could
    /// not evaluate it. Every reason that can be established without the market
    /// is established here instead, and a deployment that returns `Ok` is one
    /// the cell can actually run:
    ///
    /// * the envelope names this cell,
    /// * the program is internally consistent,
    /// * every node the plan names exists in that program,
    /// * the strategy fits the cell's evaluation budget.
    ///
    /// What deliberately is *not* checked here is whether the feature engine
    /// will produce the inputs the strategy reads. That depends on the market —
    /// a feature can be registered and still undefined for want of a quote —
    /// so it stays a per-pass judgement the runtime makes against the vector it
    /// was actually handed.
    ///
    /// Deployed this way the strategy names no [`PricingPolicy`], and every
    /// intent it raises is refused under the `pricing` gate until it is
    /// deployed through [`Self::deploy_with_pricing`]. The refusal is the
    /// safe default: a cell that guessed a pricing would either cross
    /// spreads nobody budgeted or rest orders nothing withdraws.
    pub fn deploy(
        &mut self,
        strategy: CompiledStrategy,
        program: Program,
        envelope: VerifiedEnvelope,
    ) -> Result<()> {
        self.install(strategy, program, envelope, None)
    }

    /// [`Self::deploy`], naming how the strategy's intents are priced.
    ///
    /// A `RestAtMid` policy is validated here as it is in
    /// [`PricingPolicy::rest_at_mid`], so a literal built around the
    /// constructor is refused at the same seam.
    pub fn deploy_with_pricing(
        &mut self,
        strategy: CompiledStrategy,
        program: Program,
        envelope: VerifiedEnvelope,
        pricing: PricingPolicy,
    ) -> Result<()> {
        let pricing = match pricing {
            PricingPolicy::Marketable => pricing,
            PricingPolicy::RestAtMid { time_to_live } => PricingPolicy::rest_at_mid(time_to_live)?,
        };
        self.install(strategy, program, envelope, Some(pricing))
    }

    fn install(
        &mut self,
        strategy: CompiledStrategy,
        program: Program,
        envelope: VerifiedEnvelope,
        pricing: Option<PricingPolicy>,
    ) -> Result<()> {
        if envelope.cell() != self.config.cell_id {
            return Err(Error::denied(format!(
                "an envelope for cell {} cannot deploy into {}",
                envelope.cell(),
                self.config.cell_id
            )));
        }
        if envelope.strategy() != strategy.id() {
            return Err(Error::denied(format!(
                "an envelope for strategy {} cannot deploy {}",
                envelope.strategy().as_str(),
                strategy.id().as_str()
            )));
        }

        // `NodeRef` is an index. A plan naming a node the arena does not hold
        // is the case where an out-of-range read would be the *lucky* outcome:
        // in a larger arena the index resolves, to a node belonging to some
        // other strategy, and the cell emits a signal computed from arithmetic
        // nobody wrote for it.
        program.validate()?;
        for node in strategy.plan() {
            if program.node(*node).is_none() {
                return Err(Error::invalid(format!(
                    "strategy {} plans node {} and the program it was deployed \
                     with holds {} node(s); the plan and the program do not \
                     belong together",
                    strategy.id().as_str(),
                    node.index(),
                    program.len()
                )));
            }
        }

        // ADR 0083's deploy stage. A learned function reaches this cell only
        // inline, as `Op::Model`, and every one the plan carries must be a
        // model the centre promoted — named, at its digest, in the manifest
        // this cell holds. Checked after the membership loop above, so every
        // node the walk visits is known to exist.
        self.check_models_promoted(&strategy, &program)?;

        // `with_budget` refuses a program it could not evaluate in bounded
        // time. Doing it here means an over-budget strategy is refused by the
        // deployment that shipped it rather than silently, later, by a market
        // that moved.
        let runtime = StrategyRuntime::with_budget(program, self.config.strategy_budget)?;
        if strategy.cost() > runtime.budget() {
            return Err(Error::guard(format!(
                "strategy {} needs {} nodes and this cell evaluates at most {}",
                strategy.id().as_str(),
                strategy.cost(),
                runtime.budget()
            )));
        }

        // The grant's own clock, for the share: this method takes no
        // timestamp, so `is_live` is judged at the instant the envelope was
        // verified, which is the latest instant it knows and one the grant
        // is live at by construction. A sibling grant that has expired since
        // is counted until the next pass or payload — every order still
        // checks its own envelope's window, and the centre named only grants
        // live at its own clock, so the sum stays inside the centre's share.
        let verified_at = envelope.verified_at();
        self.deployed.insert(
            envelope.strategy().as_str().to_string(),
            Deployed {
                strategy,
                runtime,
                envelope,
                utilisation: Utilisation::default(),
                class: StrategyClass::PriceOnly,
                pricing,
            },
        );
        // A grant the applied manifest names is now held, so the share the
        // manifest carries can be summed against it — see
        // `rederive_region_share`.
        self.rederive_region_share(verified_at);
        Ok(())
    }

    /// Refuse a plan whose inline model the cell's manifest does not name.
    ///
    /// The manifest is the policy's `trained_models` slot, a map from model
    /// name to [`DistilledModel::digest`] — the model's own content identity,
    /// which the cell can compute from the inline value without reaching for
    /// any library above it. A plan's model is admitted only when the
    /// manifest names it under its own name at its own digest: a match on
    /// the digest alone would let promoted weights ship under an unpromoted
    /// name, and a match on the name alone is the whole failure this guards
    /// against — a plan carrying a model nobody promoted, and the deploy
    /// stage then a formality that reads as a control.
    ///
    /// **An absent manifest admits a plan with no inline model and refuses
    /// one with any.** The slot is produced nowhere yet
    /// (`grep -rn 'ModelManifest' backend/crates --include=*.rs | grep -v '/tests/' | grep -v qip-contracts`
    /// returned nothing on 2026-09-19), so today every plan carrying a model
    /// is refused and every plan without one is unaffected. That is the
    /// fail-closed reading: a cell that holds no list of promoted models
    /// has no grounds to say a model was promoted. Freshness is deliberately
    /// not consulted — the slot's staleness narrows the cell's capability
    /// table elsewhere, and a stale manifest is still the last set the
    /// centre signed, so refusing anything outside it is the safe direction.
    ///
    /// [`DistilledModel::digest`]: qip_strategy::model::DistilledModel::digest
    fn check_models_promoted(&self, strategy: &CompiledStrategy, program: &Program) -> Result<()> {
        let manifest = self
            .policy
            .as_ref()
            .and_then(|policy| policy.payload().trained_models.value());
        for node in program.reachable_from(strategy.plan()) {
            let Some(Node {
                op: Op::Model { model, .. },
                ..
            }) = program.node(node)
            else {
                continue;
            };
            let digest = model.digest();
            let Some(manifest) = manifest else {
                return Err(Error::denied(format!(
                    "strategy {} carries model `{}` inline and this cell holds no model \
                     manifest; ship the manifest in the policy's `trained_models` slot before \
                     deploying a plan that carries a model — a model nobody promoted is not one \
                     this cell may evaluate",
                    strategy.id().as_str(),
                    model.name()
                )));
            };
            if manifest.models.get(model.name()) != Some(&digest) {
                return Err(Error::denied(format!(
                    "strategy {} carries model `{}` at digest {digest} inline and the manifest \
                     this cell holds does not name it; promote the model and ship the manifest \
                     naming it before deploying the plan — a model nobody promoted is not one \
                     this cell may evaluate",
                    strategy.id().as_str(),
                    model.name()
                )));
            }
        }
        Ok(())
    }

    /// The pricing policy a deployed strategy was given, if any.
    pub fn pricing_of(&self, strategy: &str) -> Option<PricingPolicy> {
        self.deployed
            .get(strategy)
            .and_then(|deployed| deployed.pricing)
    }

    /// Declare which pause rules govern an already-deployed strategy.
    ///
    /// Separate from [`Self::deploy`] so that classification is an explicit
    /// act rather than a defaulted parameter nobody reads. `PriceOnly` is the
    /// deploy-time default because it is true of everything shipped today; a
    /// strategy that consumes world events must say so, and saying so is what
    /// makes the degradation table able to pause it.
    pub fn classify(&mut self, strategy: &str, class: StrategyClass) -> Result<()> {
        match self.deployed.get_mut(strategy) {
            Some(deployed) => {
                deployed.class = class;
                Ok(())
            }
            None => Err(Error::invalid(format!(
                "no strategy named {strategy} is deployed in this cell, so there is nothing to \
                 classify"
            ))),
        }
    }

    pub fn deployed_strategies(&self) -> Vec<&str> {
        self.deployed.keys().map(String::as_str).collect()
    }

    /// Withdraw a deployed strategy, handing back the envelope it ran under.
    ///
    /// The path a node takes when a fresh plan no longer names a strategy,
    /// or names it differently. Refused — nothing withdrawn — while an
    /// order carrying the strategy's intent is still open at a venue: the
    /// fill that order may yet report is attributed through the strategy's
    /// contributor share, and a strategy withdrawn out from under a resting
    /// order would leave a fill the cell could book but not explain. The
    /// caller tries again once the order has filled or expired; a resting
    /// order has a time to live somebody chose, and this does not shorten it.
    ///
    /// The envelope is returned rather than dropped because it is capital
    /// the centre signed for this strategy at this cell. A plan that renames
    /// nothing but changes a rule redeploys under the same grant; a plan
    /// that drops the strategy leaves the caller holding a grant it must not
    /// spend on anything else, which `renew_capital` already refuses.
    pub fn withdraw(&mut self, strategy: &str, now: Timestamp) -> Result<VerifiedEnvelope> {
        if !self.deployed.contains_key(strategy) {
            return Err(Error::not_found(format!(
                "no strategy named {strategy} is deployed in this cell, so there is nothing to \
                 withdraw"
            )));
        }
        let open: Vec<&str> = self
            .working
            .values()
            .filter(|working| {
                working.order.closed.is_none()
                    && working
                        .net
                        .contributors
                        .iter()
                        .any(|contributor| contributor.strategy.as_str() == strategy)
            })
            .map(|working| working.order.order_id.as_str())
            .collect();
        if !open.is_empty() {
            return Err(Error::denied(format!(
                "strategy {strategy} has {} open order(s) at the venue ({}); it is withdrawn once \
                 they have filled or expired, not while a fill on them could still arrive",
                open.len(),
                open.join(", ")
            )));
        }
        let Some(deployed) = self.deployed.remove(strategy) else {
            return Err(Error::not_found(format!(
                "no strategy named {strategy} is deployed in this cell, so there is nothing to \
                 withdraw"
            )));
        };
        self.journal.record(
            Decision::StrategyWithdrawn {
                strategy: strategy.to_string(),
            },
            now,
        );
        // A grant the cell no longer runs under no longer funds its share:
        // the narrowing direction, applied at once rather than at the next
        // payload.
        self.rederive_region_share(now);
        Ok(deployed.envelope)
    }

    // --- the hot path -------------------------------------------------------

    /// Bytes in: decode, sequence, apply, and mark features dirty.
    ///
    /// Does no file or network I/O. The mirror is drained by [`Cell::flush`]
    /// precisely so that this call's cost is arithmetic and memory, never a
    /// storage system's availability.
    pub fn on_bytes(&mut self, feed: &FeedKey, bytes: &[u8], now: Timestamp) -> Result<usize> {
        let (decoded, skipped) = {
            let decoder = self.protocols.decoder_mut(&feed.venue, &feed.feed)?;
            let decoded = decoder.decode(bytes, now)?;
            // Read the counter after decoding: it is cumulative, and the
            // difference is what this call actually skipped.
            let skipped = usize::try_from(decoder.diagnostics().messages_skipped).unwrap_or(0);
            (decoded, skipped)
        };
        let count = decoded.len();
        self.journal.record(
            Decision::Ingested {
                feed: format!("{}/{}", feed.venue.as_str(), feed.feed),
                decoded: count,
                skipped,
            },
            now,
        );

        let batch = self.sequencer.accept(decoded, now);
        self.apply_batch(batch.released, now)?;
        for event in &batch.events {
            if let Some(detail) = gap_detail(event) {
                self.journal.record(
                    Decision::GapDetected {
                        stream: detail.0,
                        detail: detail.1,
                    },
                    now,
                );
            }
        }
        Ok(count)
    }

    /// Apply released messages to books and the feature graph.
    fn apply_batch(&mut self, messages: Vec<MarketMessage>, _now: Timestamp) -> Result<()> {
        for message in &messages {
            let venue = message.origin.venue.clone();
            if let Some(state) = self.liquidity.get_mut(&venue, &message.object_id) {
                // A message the book refuses is a book that would be wrong if
                // it accepted it; the refusal is recorded by the reset path,
                // not swallowed here.
                state.apply(message)?;
            }
            self.features.ingest(message)?;
        }
        Ok(())
    }

    /// One pass of decide-and-act.
    ///
    /// Every gate that refuses records why, in order, so the reason a cell was
    /// quiet is reconstructable without re-running it.
    pub fn work(&mut self, now: Timestamp, gateway: &mut dyn Placer) -> Result<WorkReport> {
        let mut report = WorkReport {
            halted: self.is_halted(),
            ..WorkReport::default()
        };
        // Recorded before the halt check, so a halted cell still counts its
        // passes. A refusal count with no pass count underneath it cannot tell
        // "nothing was refused" from "the cell never ran".
        self.metrics.work_pass();
        // Counted before the halt check too, for the crossing window: a
        // `Passes` interval that skipped halted passes would stretch over
        // more wall time the longer the cell was stopped.
        self.pass = self.pass.saturating_add(1);

        // Region holds are pass-scoped: each is committed or released before
        // its pass ends. This is the backstop for the path that did neither —
        // most plausibly an error propagating out of phase three — and it runs
        // before the halt check so a cell that halts mid-incident does not pin
        // its region's capital for as long as the halt lasts. Journaled rather
        // than silent: a hold reaching here means a release site was missed,
        // and that is a defect an operator should be able to find afterwards.
        let abandoned = match self.region_allocation.as_ref() {
            Some(allocation) => allocation.sweep_before(&self.config.cell_id, self.pass),
            None => Vec::new(),
        };
        for (id, amount) in abandoned {
            self.journal.record(
                Decision::Refused {
                    gate: "region_reservation_abandoned".to_string(),
                    reason: format!(
                        "the region hold {id} outlived its pass holding {amount}; it has been \
                         returned to the allocation"
                    ),
                },
                now,
            );
        }
        // Published before the halt check, so a halted cell still reports what
        // its region has left rather than going dark on the number.
        self.metrics
            .region_allocation(self.region_allocation.as_ref().map(RegionTable::free));
        // §29.2 and §32.1, published before the halt check for the reason
        // the region allocation above is: a halted cell that goes dark on
        // its own limits reads exactly like a cell that has none. Accrued
        // first, so the headroom on the gauge is what this pass holds
        // rather than what the last pass that sent something left behind —
        // a cell quiet for an hour would otherwise publish the bucket it
        // drained an hour ago and an operator would read a spent budget as
        // the cause of the silence it is not.
        self.budget.refill_all(now);
        self.metrics.quote_budget(&self.budget.summary());
        // The number that says the dispersion gate has nothing to judge
        // with. Every venue unmeasured is the state in which that gate
        // admits every cycle, and it looks identical to a gate that is
        // passing unless this is on a chart.
        self.metrics
            .fill_time_unmeasured(self.fill_times.unmeasured());
        // §56.2 rule 21's silence, made a number for the same reason: a
        // venue with no settlement terms has every dependent leg admitted
        // unjudged, and a chart on which that reads as zero refusals is a
        // chart that says the settlement gate is passing.
        self.metrics
            .settlement_unprojected(self.config.unprojected_settlement_venues());
        // §36.3's two, published before the halt check for the reason every
        // gauge above is: a halted cell that goes dark on how many of its
        // peers have gone dark, or on how many venues it has yet to
        // reconcile against, reads exactly like a cell with neither problem.
        // "Nothing is dark and nothing is outstanding" is the finding an
        // operator is looking for during somebody else's incident.
        self.record_dark_regions();
        self.record_awaiting_reconciliation();

        self.record_halt();

        // What the venue has done with the orders already out, before the
        // halt check: a halted cell sends nothing, and still has to learn
        // what filled, because a fill it does not confirm is a fill the
        // reconciler will read as unknown to it.
        report.fills = self.confirm_execution_reports(gateway, now);
        // And what has rested long enough. Also before the halt check:
        // withdrawing is not sending, and a halted cell with orders resting
        // at a price the market has left is exactly the cell that should
        // withdraw them.
        self.withdraw_expired(gateway, now);

        if report.halted {
            // Books keep absorbing and the journal keeps recording while
            // halted. A cell that stops seeing the market cannot tell whether
            // it is safe to resume. The gate names which halt is in force,
            // because the two release disciplines are different and an
            // operator staring at a quiet cell needs to know which door to
            // knock on.
            let gate = if self.autonomy.kill_switch().is_globally_tripped() {
                "kill_switch"
            } else if self.policy_halted {
                "policy_halt"
            } else {
                "polled_halt"
            };
            self.refuse(&mut report, gate, "the cell is halted", now);
            return Ok(report);
        }

        // The class of the gateway this pass was handed, before a signal is
        // raised or a net is formed. `Cell::send` refuses each order at the
        // seam and is the guarantee; this is what makes a live-class gateway
        // *visible*. Without it a misconfigured cell is silent on every quiet
        // pass and errors out of the middle of a busy one, so the series an
        // operator would look at — `qip_edge_refusals_total{gate}` — never
        // moves until the cell happens to want to trade.
        //
        // Placed after the halt check because a halted cell is already sending
        // nothing and the halt gate is the one an operator must act on first;
        // placed before the confirmation and withdrawal above would have been
        // wrong for the opposite reason, since learning what filled and pulling
        // resting orders back both *reduce* exposure.
        if !gateway.is_simulated() {
            let reason = format!(
                "the gateway this cell was handed reports itself live and the cell's ceiling is \
                 {}; no order is formed this pass. Attach a simulated gateway, or stop the cell",
                self.autonomy.ceiling().as_str()
            );
            self.refuse(&mut report, GATE_LIVE_VENUE, &reason, now);
            return Ok(report);
        }

        // §36.3's node-crash row: a cell that restarted forms no order until
        // every venue's own account has agreed with its record. Placed after
        // the halt and gateway gates, which are the two an operator must act
        // on first, and before anything that could raise a signal — a
        // strategy that ran here would price against a book whose venue may
        // still be holding size this process cannot see.
        //
        // The borrow ends before the refusal, because `refuse` takes the cell
        // mutably to journal and count what it refused.
        let awaiting = self
            .resume
            .as_ref()
            .map(|discipline| (discipline.reason().to_string(), discipline.pending()));
        if let Some((reason, pending)) = awaiting {
            let detail = format!(
                "this cell is reconciling before it resumes ({reason}) and {} venue(s) have not \
                 answered — {}; §36.3 reconciles against every venue, including resting orders \
                 and quotes, before resuming, so no order is formed this pass",
                pending.len(),
                pending.join(", ")
            );
            self.refuse(&mut report, GATE_AWAITING_RECONCILIATION, &detail, now);
            return Ok(report);
        }

        // The degradation table, consulted once per pass. Everything below
        // reads the same narrowing, so a payload applied mid-pass changes the
        // next pass, never half of this one.
        let narrowing = self.narrowing(now);
        let multiplier = narrowing.sizing_multiplier();
        // Freshness is a function of `now`, so this is the instant it becomes
        // known and the only instant at which the recorded value is what the
        // cell actually sized against. Before this the whole table was
        // formatted into a journal string and discarded.
        self.metrics.narrowing(&narrowing);

        // Phase one collects; phase two nets; phase three sends. The split is
        // the blueprint's, and §28 is why the per-strategy gates stay in phase
        // one rather than moving onto the net.
        let mut intents: Vec<Intent> = Vec::new();

        // ADR 0080: the retired strategies' lots the centre has asked this
        // cell to unwind, before the strategy loop and after every halt gate
        // above — so a halted cell unwinds nothing, and an unwind is in the
        // netting set before any live strategy's opposite intent so the two
        // can cross under §27.1's cap. Placed ahead of the feature
        // evaluation, deliberately: reducing a lot needs no feature vector,
        // and a feature-engine fault that stops every signal should not also
        // stop the one intent that only lowers gross.
        let retired = self.disposition_intents(now, &mut report, &mut intents);

        let vector = self.features.evaluate(now)?;
        let strategy_ids: Vec<String> = self.deployed.keys().cloned().collect();

        for id in strategy_ids {
            // A strategy the applied policy names as retired evaluates
            // nothing. See `GATE_DISPOSITION` for why its own signal is
            // refused rather than run: a directional intent on its stale
            // envelope would net against its own unwind.
            if retired.contains(&id) {
                self.refuse(
                    &mut report,
                    GATE_DISPOSITION,
                    &format!(
                        "strategy {id} is retired at the centre and its lot here is being \
                         unwound; it evaluates no signal while the applied policy names it"
                    ),
                    now,
                );
                continue;
            }
            // A paused strategy does not evaluate at all. Refusing before the
            // run rather than after keeps the journal honest about why the
            // cell was quiet: no signal existed, because the capability the
            // strategy depends on is gone.
            if let Some(deployed) = self.deployed.get(&id)
                && narrowing.pauses(deployed.class)
            {
                self.refuse(
                    &mut report,
                    "degradation_pause",
                    &format!("strategy {id} pauses while its capability is degraded"),
                    now,
                );
                continue;
            }
            // Each deployment evaluates against the arena it was compiled
            // with. `runtime` and `strategy` are disjoint fields of the same
            // deployment, so the borrow ends with the call and the refusal
            // path below can take `&mut self` to journal why it refused.
            let outcome = match self.deployed.get_mut(&id) {
                Some(deployed) => deployed.runtime.run(&deployed.strategy, &vector, now),
                None => continue,
            };
            let signal = match outcome {
                Ok(Some(signal)) => signal,
                Ok(None) => continue,
                Err(error) => {
                    self.refuse(&mut report, "strategy_runtime", error.message(), now);
                    continue;
                }
            };

            self.journal.record(
                Decision::SignalRaised {
                    strategy: signal.strategy.as_str().to_string(),
                    object: signal.object_id.as_str().to_string(),
                    kind: signal.kind.as_str().to_string(),
                    conviction_shrunk_f64: signal.conviction.shrunk(),
                },
                now,
            );
            self.metrics.signal(signal.kind);
            report.signals.push(signal.clone());

            if let Some(intent) = self.intent_for(&signal, multiplier, now, &mut report)? {
                intents.push(intent);
            }
        }

        // The arbitrage desk, at the seam §27.2 names: after the strategies
        // have asked and before anything is netted, so that legs and
        // directional intents meet at one place — and part company there,
        // because a leg is never netted. Every leg is judged by the same
        // feasibility gate the directional intents meet below, then held
        // until the nets have gone out.
        let cycles = self.scan_cycles(now, multiplier, &narrowing, &mut report)?;

        // The feasibility gate, between collection and netting (§18.1). An
        // intent that cannot execute at its size never enters the netting
        // set: a net built from an infeasible contributor would carry that
        // contributor's share to the venue inside an order whose other
        // contributors were feasible, and the venue's rejection — or the
        // fee's bite — would land on all of them. `retain` judges in place,
        // so the gate allocates nothing per pass.
        intents.retain(|intent| {
            let feasible = self.admit_feasible(intent, now, &mut report);
            if !feasible {
                // The region hold this intent took in phase one, given back:
                // it will not reach a net, so it can never be committed.
                self.release_region_hold_for(intent.strategy.as_str());
            }
            feasible
        });

        // Phase two. Everything the strategies asked for collapses onto one
        // intent per instrument, venue and representation — so two strategies
        // buying the same thing send one order and pay the spread once, and
        // two wanting opposite things cancel without either reaching the
        // venue. Before this, each strategy placed its own order and the two
        // could cross each other, which is a self-trade: a regulatory problem
        // and a pure loss at the same time.
        let nets = net(intents);
        report.netting_ratio = netting_ratio(&nets);
        // `None` when everything cancelled: the ratio is unbounded there, and
        // observing a sentinel would put a number nobody computed into the
        // distribution. The cancellation is counted in `place_net` instead.
        if let Some(ratio) = report.netting_ratio {
            self.metrics.netting_ratio(ratio);
        }
        for net_intent in &nets {
            if let Some(order) = self.place_net(net_intent, now, gateway, &mut report)? {
                report.orders.push(order);
            }
        }

        // §32.1: a cycle whose slow leg was left resting on an earlier pass
        // is finished — or given up on — before a new one is opened. The
        // order is the point. A cell that opened fresh cycles while an older
        // one sat half-formed would be spending its open-order capacity on
        // opportunities it has not committed to ahead of the one it has.
        self.resume_rested_cycles(now, gateway, &mut report)?;

        // Cycles go out after the nets. Never through `net`: each leg is
        // sent by the same order path a net intent uses, one leg after
        // another in the plan's order, least reversible first.
        for cycle in &cycles {
            self.place_cycle(cycle, now, gateway, &mut report)?;
        }

        Ok(report)
    }

    /// Build a reduce-only intent for each lot the applied policy's
    /// dispositions slot names, or refuse it (ADR 0080, decision four).
    ///
    /// For each `(strategy, instrument, flatten_by)` the cell reads the lot
    /// *it* holds for that strategy in that instrument — its own book, never
    /// the centre's claim — and refuses under [`GATE_DISPOSITION`] when it
    /// holds nothing, when the instruction names no quantity, or when the
    /// instruction's sign would increase the lot or carry it through flat.
    /// Otherwise the intent is sized to the smaller of the instruction and
    /// the lot, so it can never overshoot flat whichever of the two claims
    /// is stale, and taken through the routing gates a signal meets
    /// (`route_for`) and the autonomy gate.
    ///
    /// **Two gates are skipped, and only here.** The capital envelope: a
    /// retired strategy can never again receive one (ADR 0075), and the
    /// intent commits no new notional — the sign check above makes it
    /// structurally reduce-only, so it can only lower gross, and that is the
    /// whole of why an intent with no envelope is admissible at all. The
    /// region hold: a hold reserves capital for exposure being *added*, and
    /// this adds none. The degradation multiplier is not applied either,
    /// because it narrows new risk and an unwind is the removal of risk;
    /// what bounds the size instead is the lot and, after this, the
    /// feasibility gate's reading of the ladder.
    ///
    /// The intent enters the netting set as `Nettable`, so it can cross
    /// internally against a peer strategy's opposite intent — the cheapest
    /// exit there is — and its fill is attributed to the retired strategy
    /// through the same contributor vector as every other fill.
    ///
    /// Returns every strategy the slot named, acted on or refused, so the
    /// strategy loop can decline to evaluate them this pass.
    fn disposition_intents(
        &mut self,
        now: Timestamp,
        report: &mut WorkReport,
        intents: &mut Vec<Intent>,
    ) -> BTreeSet<String> {
        let mut named = BTreeSet::new();
        // Copied out of the applied payload so the loop can take `&mut self`
        // to journal and count each refusal.
        let instructions: Vec<(StrategyId, String, Decimal)> = match self.dispositions() {
            Some(dispositions) => dispositions
                .unwinds
                .iter()
                .flat_map(|(strategy, lots)| {
                    lots.iter().map(move |(instrument, flatten_by)| {
                        (strategy.clone(), instrument.clone(), *flatten_by)
                    })
                })
                .collect(),
            None => return named,
        };
        for (strategy, instrument, flatten_by) in instructions {
            named.insert(strategy.as_str().to_string());
            let object_id = ObjectId::from_string(&instrument);
            let held = self.strategy_lot(&strategy, &object_id);
            let line = |verdict: DispositionVerdict| DispositionLine {
                strategy: strategy.clone(),
                object_id: object_id.clone(),
                flatten_by,
                held,
                verdict,
            };

            // The sign check, and the two readings that make it undecidable.
            // A lot of zero has no sign to reduce against; an instruction of
            // zero asks for nothing. Both are the centre's claim and this
            // book disagreeing, and both are refused rather than read.
            let refusal = if flatten_by.is_zero() {
                Some(format!(
                    "the disposition for strategy {} on {} names no quantity; nothing is unwound \
                     on an instruction to trade nothing",
                    strategy.as_str(),
                    object_id.as_str()
                ))
            } else if held.is_zero() {
                Some(format!(
                    "this cell holds no lot for strategy {} on {}, so there is nothing to unwind \
                     by {flatten_by}; the centre's attribution and this book disagree and nothing \
                     moves until they agree",
                    strategy.as_str(),
                    object_id.as_str()
                ))
            } else if flatten_by.is_positive() == held.is_positive() {
                Some(format!(
                    "the disposition for strategy {} on {} would trade {flatten_by} against a lot \
                     of {held}, which increases the lot rather than reducing it; a disposition \
                     is reduce-only and this one is refused on its sign",
                    strategy.as_str(),
                    object_id.as_str()
                ))
            } else {
                None
            };
            if let Some(reason) = refusal {
                self.refuse(report, GATE_DISPOSITION, &reason, now);
                report.dispositions.push(line(DispositionVerdict::Refused {
                    gate: GATE_DISPOSITION.to_string(),
                    reason,
                }));
                continue;
            }

            // The smaller of what was asked and what is held, in the
            // instruction's direction — which the check above has just made
            // the lot's opposite. Never `flatten_by` alone: an instruction
            // derived from a book one fill behind this one would carry the
            // lot through flat, and that is the one thing this path may not do.
            let size = flatten_by.abs().min(held.abs());
            let signed_size = if flatten_by.is_positive() {
                size
            } else {
                -size
            };

            let Some((venue, price)) = self.route_for(strategy.as_str(), &object_id, now, report)
            else {
                // `route_for` refused, journaled and counted under the
                // routing gate's own literal; the line carries which.
                let (gate, reason) = report
                    .refusals
                    .last()
                    .cloned()
                    .unwrap_or_else(|| (GATE_DISPOSITION.to_string(), "refused".to_string()));
                report
                    .dispositions
                    .push(line(DispositionVerdict::Refused { gate, reason }));
                continue;
            };
            if self.autonomy.level() == AutonomyLevel::Observation {
                let reason = "the cell is at observation and sends nothing".to_string();
                self.refuse(report, "autonomy", &reason, now);
                report.dispositions.push(line(DispositionVerdict::Refused {
                    gate: "autonomy".to_string(),
                    reason,
                }));
                continue;
            }
            let intent = match Intent::new(
                strategy.clone(),
                object_id.clone(),
                venue.clone(),
                signed_size,
                price,
                now.saturating_add(DISPOSITION_VALIDITY),
            ) {
                Ok(intent) => intent,
                Err(error) => {
                    // Unreachable while `size` is the minimum of two non-zero
                    // magnitudes, and refused rather than unwrapped because
                    // the constructor is the one that decides.
                    let reason = error.message().to_string();
                    self.refuse(report, GATE_DISPOSITION, &reason, now);
                    report.dispositions.push(line(DispositionVerdict::Refused {
                        gate: GATE_DISPOSITION.to_string(),
                        reason,
                    }));
                    continue;
                }
            };
            self.journal.record(
                Decision::DispositionIntent {
                    strategy: strategy.as_str().to_string(),
                    object: object_id.as_str().to_string(),
                    venue: venue.as_str().to_string(),
                    flatten_by: flatten_by.to_string(),
                    held: held.to_string(),
                    signed_size: signed_size.to_string(),
                },
                now,
            );
            report
                .dispositions
                .push(line(DispositionVerdict::Intent { signed_size, venue }));
            intents.push(intent);
        }
        named
    }

    /// The venue and reference price an intent for `strategy` on `object_id`
    /// would be reasoned at, through the routing gates every intent passes
    /// before its size is bounded: venue selection, book presence, book
    /// staleness, venue status, a usable mid, and the strategy's stated
    /// pricing policy. `None` when any of them refused, journaled and counted
    /// under that gate's own literal.
    ///
    /// Shared by [`Self::intent_for`] and the disposition path (ADR 0080) so
    /// the two cannot drift: an unwind meets exactly the routing gates a
    /// signal does, in the same order, and a gate added here is added to
    /// both. What the disposition path does **not** share — the degradation
    /// multiplier, the capital envelope and the region hold — is left in
    /// `intent_for` on purpose, and the ADR says why each is skipped.
    fn route_for(
        &mut self,
        strategy: &str,
        object_id: &ObjectId,
        now: Timestamp,
        report: &mut WorkReport,
    ) -> Option<(VenueId, Decimal)> {
        let Some(venue) = self.venue_for(object_id, now) else {
            self.refuse(
                report,
                "venue_selection",
                "no venue this cell may reach quotes the instrument",
                now,
            );
            return None;
        };

        // A stale or unpriceable book routes nothing. The book already refuses
        // to serve a mid; routing against one anyway would use a price from
        // before the gap that made it stale.
        // Read everything needed from the book in one borrow, so the refusal
        // path below can take `&mut self` to journal why it refused.
        let assessment = self.liquidity.get(&venue, object_id).map(|state| {
            (
                state.is_stale(),
                state
                    .reset_reason()
                    .unwrap_or("the book is awaiting resynchronisation")
                    .to_string(),
                state.status(),
                state.mid(),
            )
        });
        let Some((stale, reset_reason, status, mid)) = assessment else {
            self.refuse(
                report,
                "book",
                "the cell holds no book for the instrument",
                now,
            );
            return None;
        };
        if stale {
            self.refuse(report, "stale_book", &reset_reason, now);
            return None;
        }
        if !status.accepts_orders() {
            self.refuse(
                report,
                "venue_status",
                &format!("the venue is {}", status.as_str()),
                now,
            );
            return None;
        }
        let Some(price) = mid else {
            self.refuse(report, "pricing", "the book serves no usable price", now);
            return None;
        };
        // The price the intent is *reasoned* at is the mid; the price it is
        // *sent* at is decided by the strategy's policy when the net is
        // placed, and a strategy that stated none is refused here, before
        // it can contribute to a net that another strategy's policy would
        // then price.
        if self.pricing_of(strategy).is_none() {
            self.refuse(
                report,
                "pricing",
                &format!(
                    "strategy {} was deployed with no pricing policy; deploy it with \
                     deploy_with_pricing naming marketable or rest-at-mid with a time to live, \
                     because an intent with no stated pricing is never sent",
                    strategy
                ),
                now,
            );
            return None;
        }

        Some((venue, price))
    }

    /// Take one signal through every per-strategy gate to an intent, or
    /// refuse it.
    ///
    /// Phase one of the two the blueprint separates. §28 is explicit that
    /// strategy-level limits are checked *before* netting, "because a strategy
    /// that has exhausted its budget must not contribute to a net intent at
    /// all" — so expiry, venue, book staleness, pricing, the degradation
    /// multiplier and the capital envelope all run here, per strategy, exactly
    /// as they did when this function placed an order directly. No gate was
    /// removed and none was reordered; what changed is that the admitted size
    /// becomes an intent instead of an order.
    fn intent_for(
        &mut self,
        signal: &Signal,
        multiplier: Decimal,
        now: Timestamp,
        report: &mut WorkReport,
    ) -> Result<Option<Intent>> {
        if !signal.is_live(now) {
            self.refuse(report, "signal_expiry", "the signal is no longer live", now);
            return Ok(None);
        }

        let Some((venue, price)) =
            self.route_for(signal.strategy.as_str(), &signal.object_id, now, report)
        else {
            return Ok(None);
        };

        let side = match signal.kind {
            SignalKind::Enter => BookSide::Ask,
            SignalKind::Exit | SignalKind::Hedge => BookSide::Bid,
            SignalKind::Stand => {
                self.refuse(report, "signal_kind", "the signal asks for no action", now);
                return Ok(None);
            }
        };

        // Confidence-weighted sizing, §6.2's consumer. The multiplier narrows
        // the *ask* before the envelope bounds it, so utilisation accounting
        // sees the quantity that will actually be requested. It is exact
        // arithmetic — this scales a position — and a multiply that cannot be
        // represented narrows to nothing rather than widening, the same
        // asymmetry the degradation table itself keeps.
        let desired = signal
            .desired_quantity
            .checked_mul(multiplier)
            .unwrap_or(Decimal::ZERO);
        if !desired.is_positive() {
            self.refuse(
                report,
                "degradation_sizing",
                "the degradation multiplier narrowed the size to nothing",
                now,
            );
            return Ok(None);
        }
        let notional = desired * price;
        let key = signal.strategy.as_str().to_string();
        let Some(deployed) = self.deployed.get(&key) else {
            self.refuse(
                report,
                "deployment",
                "the strategy is not deployed here",
                now,
            );
            return Ok(None);
        };

        // Expiry is checked at every use rather than once at verification:
        // it is the backstop bounding a cell that lost contact, and a backstop
        // consulted only on arrival is not one.
        if !deployed.envelope.is_live(now) {
            self.refuse(
                report,
                "envelope_expiry",
                "the capital envelope has expired; the cell stops rather than continues",
                now,
            );
            return Ok(None);
        }

        let quantity = match deployed
            .envelope
            .admit(&venue, notional, &deployed.utilisation, now)
        {
            CapitalGrant::Full => desired,
            CapitalGrant::Reduced(cap) => {
                let reduced = cap.checked_div(price).unwrap_or(Decimal::ZERO);
                self.refuse(
                    report,
                    "capital_reduced",
                    &format!("reduced to {reduced} by the capital envelope"),
                    now,
                );
                reduced
            }
            CapitalGrant::Refused(reason) => {
                self.refuse(report, "capital", &reason, now);
                return Ok(None);
            }
        };
        if !quantity.is_positive() {
            self.refuse(
                report,
                "capital",
                "the permitted size rounded to nothing",
                now,
            );
            return Ok(None);
        }

        if self.autonomy.level() == AutonomyLevel::Observation {
            self.refuse(
                report,
                "autonomy",
                "the cell is at observation and sends nothing",
                now,
            );
            return Ok(None);
        }

        // The region's own bound, taken before the intent exists. The envelope
        // above bounds this *strategy*; until this line nothing bounded the
        // sum of the strategies, so a cell with four deployments could commit
        // four envelopes' worth against a region budget the centre had set
        // aside once — and with seven cells deciding alone while partitioned,
        // that is a double-spend of the same capital. The hold is released
        // below if any gate between here and the venue refuses, and committed
        // when the order goes out.
        let pass = self.pass;
        let Some(notional_held) = quantity.checked_mul(price) else {
            self.refuse(
                report,
                "region_reservation",
                "the admitted notional cannot be represented, so no region hold can be taken \
                 against it; nothing is sent on a number the cell could not compute",
                now,
            );
            return Ok(None);
        };
        if !self.hold_region_capital(
            region_hold_id(pass, signal.strategy.as_str()),
            notional_held,
            now,
            report,
        ) {
            return Ok(None);
        }

        // Signed, because netting is addition: a buy is positive, a sell is
        // negative, and two opposing intents of equal size sum to nothing
        // without anybody writing a conditional that could be got backwards.
        //
        // `side` names the side of the book the order *takes* — the one
        // convention every seam past this point shares (`Placer`, the node's
        // gateways, `sweep_cost`) — so taking the ask is the buy and is the
        // positive one. This line once read the other way round, and
        // `place_net` read `is_buy` the other way round to match, so an
        // `Enter` still reached the venue as a buy while every fact computed
        // from the sign in between — the cross ledger's `bought` and `sold`,
        // the contributor shares shipped to the centre, `NetIntent::is_buy`
        // itself — named the buyer as the seller. Two inversions that cancel
        // at the venue are not a convention; they are a defect the venue
        // happens not to see.
        let signed = if matches!(side, BookSide::Ask) {
            quantity
        } else {
            -quantity
        };
        let intent = Intent::new(
            signal.strategy.clone(),
            signal.object_id.clone(),
            venue,
            signed,
            price,
            signal.valid_until,
        )?
        // The revisions travel with the intent because this is the last point
        // that has them: after netting, several strategies' shares share one
        // order, and a fill can only be traced back to the values that caused
        // it if each contributor kept its own.
        .with_inputs(signal.inputs.clone());
        Ok(Some(intent))
    }

    /// Send one net intent as one order, or record that it cancelled.
    ///
    /// Phase three. A net of zero is not a refusal: it is two strategies that
    /// wanted opposite things, cancelled internally, and the venue never sees
    /// either — which is the self-trade this whole mechanism exists to
    /// prevent. It is recorded so the cell can still explain what happened.
    ///
    /// Every path that returns without sending gives back the region holds
    /// the contributors took in phase one; the send path commits them. The
    /// one exception is `self.send(...)?`, whose error propagates out of
    /// `work` and leaves the holds standing — that pass has ended, and the
    /// sweep at the top of the next one returns them.
    fn place_net(
        &mut self,
        net_intent: &NetIntent,
        now: Timestamp,
        gateway: &mut dyn Placer,
        report: &mut WorkReport,
    ) -> Result<Option<PlacedOrder>> {
        // Evaluated before the zero-net guard below so that a net cancelling
        // to nothing is still assessed rather than skipped. It is *not* true,
        // as this comment previously claimed, that doing so lets the cap catch
        // its flagship case: see `cross_internally`, where the arithmetic that
        // makes a full cancellation permanently out of cap is set out.
        let crossed = self.cross_internally(net_intent, now, report);

        let Some(is_buy) = net_intent.is_buy() else {
            // Nothing reached the venue, so the cross — if the cap allowed one
            // — is final at this point and safe to seal into the chain.
            self.settle_cross(net_intent, crossed, now, report);
            self.journal.record(
                Decision::Refused {
                    gate: "internal_cross".to_string(),
                    reason: format!(
                        "{} intents on {} at {} cancelled to zero; nothing reached the venue",
                        net_intent.contributors.len(),
                        net_intent.object_id.as_str(),
                        net_intent.venue.as_str()
                    ),
                },
                now,
            );
            self.metrics.intent_cancelled();
            report.cancelled.push(net_intent.clone());
            // Nothing reached the venue, so nothing of the region's capital
            // was spent. A hold left standing here would take the whole
            // cancelled pair's notional out of the region for the pass.
            self.release_region_holds(&net_intent.contributors);
            return Ok(None);
        };
        // A buy takes the ask. See `intent_for` for why this must agree with
        // the sign there rather than compensate for it.
        let side = if is_buy { BookSide::Ask } else { BookSide::Bid };
        let quantity = net_intent.order_quantity();
        let venue = net_intent.venue.clone();

        // Every refusal from here to the send drops the cross unsealed, like
        // the send-error path below: nothing of this net happened, and a
        // cross booked beside a refused order would be a trade between two
        // strategies that the chain could not pair with the order it was the
        // residual of.
        if self.would_self_trade(net_intent, side) {
            self.refuse(
                report,
                "self_trade",
                &format!(
                    "an order of the cell's own is resting on the other side of {} at {}; a {} \
                     now would trade with it, so the net is refused until that order fills or \
                     expires",
                    net_intent.object_id.as_str(),
                    venue.as_str(),
                    if is_buy { "buy" } else { "sell" }
                ),
                now,
            );
            self.release_region_holds(&net_intent.contributors);
            return Ok(None);
        }
        if !self.has_open_capacity(1) {
            self.refuse_for_capacity(report, now);
            self.release_region_holds(&net_intent.contributors);
            return Ok(None);
        }
        let Some((price, expires_at)) =
            self.resolve_pricing(net_intent, side, quantity, now, gateway, report)
        else {
            self.release_region_holds(&net_intent.contributors);
            return Ok(None);
        };

        // §29.2's token bucket, immediately before the one call that speaks
        // to the venue and after every gate that could still refuse. Here
        // rather than earlier because a token spent on an order the pricing
        // gate then refused would be a message billed that never ran, and
        // the budget is the cell's account of what it said to the venue.
        // No order object exists yet: this is a price and a quantity.
        //
        // Placements stop at the withdrawal reserve, so a cell that has
        // quoted its rate away can still pull its resting orders back. That
        // is the whole reason the reserve exists.
        if let Admission::Refused { reason } =
            self.budget.admit(&venue, MessageKind::Placement, now)
        {
            self.refuse(report, GATE_QUOTE_BUDGET, &reason, now);
            self.release_region_holds(&net_intent.contributors);
            return Ok(None);
        }
        self.metrics.message_sent(&venue, MessageKind::Placement);

        // ADR 0084: a net is one order at one venue, so its schedule is the
        // trivially equalised one — offset zero — and it is computed rather
        // than assumed so the journal entry carries the same two fields a
        // cycle leg's does and a reader need not know which path sent it.
        let schedule = self
            .fill_times
            .release_schedule(std::slice::from_ref(&venue));
        let release_at = now.saturating_add(schedule.offset(&venue));
        let (order_id, simulated) = self.send(
            &net_intent.object_id,
            &venue,
            side,
            quantity,
            price,
            now,
            release_at,
            gateway,
        )?;

        // Only now, past the call that can fail. `gateway.place` propagates its
        // error out of `work`, and the caller loses the report with it — so a
        // cross sealed into the hash-chained journal before this line would
        // assert that two strategies traded during a pass that produced
        // nothing at all. The chain is the record; it may not carry a trade
        // the pass did not make.
        self.settle_cross(net_intent, crossed, now, report);

        // Utilisation is charged per contributor, pro-rata on what each
        // wanted, so a netted order still spends each strategy's own envelope
        // rather than one strategy's. The split sums exactly to the order, so
        // the envelopes together are charged what was actually sent.
        for (strategy, share) in net_intent.split_fill(quantity) {
            if let Some(deployed) = self.deployed.get_mut(strategy.as_str()) {
                deployed.utilisation.gross_committed += share * price;
                deployed.utilisation.orders_sent += 1;
            }
        }
        // Beside the envelope charge and for the same reason it is here: the
        // region allocation and the envelopes are two claims about one order,
        // and two claims recorded in different places will disagree.
        let region_committed = self.commit_region_holds(&net_intent.contributors);
        self.record_sent(
            Working {
                order: OpenOrder {
                    order_id: order_id.clone(),
                    venue: venue.clone(),
                    object_id: net_intent.object_id.clone(),
                    side,
                    quantity,
                    price,
                    filled: Decimal::ZERO,
                    simulated,
                    sent_at: now,
                    release_at,
                    expires_at,
                    closed: None,
                },
                net: net_intent.clone(),
                region_committed,
            },
            schedule.equalised(),
            now,
        );
        // The venue may have filled some of it on acceptance. Those reports
        // are confirmed now, against the record just written, so a fill on
        // this pass is attributed on this pass.
        let confirmed = self.confirm_execution_reports(gateway, now);
        report.fills.extend(confirmed);

        let largest = net_intent
            .contributors
            .iter()
            .max_by(|left, right| {
                left.signed_size
                    .abs()
                    .cmp(&right.signed_size.abs())
                    .then_with(|| right.strategy.as_str().cmp(left.strategy.as_str()))
            })
            .map_or_else(|| StrategyId::new("unknown"), |c| c.strategy.clone());

        Ok(Some(PlacedOrder {
            order_id,
            strategy: largest,
            contributors: net_intent.contributors.clone(),
            object_id: net_intent.object_id.clone(),
            venue,
            side,
            quantity,
            price,
            simulated,
        }))
    }

    /// Number an order and hand it to the venue, or refuse it because the
    /// venue is not a simulated one.
    ///
    /// The one place a `Placer` is called. Both the net path and the cycle
    /// path go through it, so an order that reaches a venue has been numbered
    /// by the cell's own sequence whichever seam produced it, and a second
    /// route to `gateway.place` — the shape of a control being bypassed —
    /// would have to be written in the open. That is what makes this the seam
    /// the venue class is checked at.
    ///
    /// `qip-execution-engine`'s order manager has refused a live venue below a
    /// live level since it was written. The cell read the same bit and used it
    /// only to stamp `simulated` onto the journal entry, so the one process on
    /// this platform that places orders **without asking the central plane**
    /// (ADR 0008) was the one that never compared its posture against the
    /// class of venue it was sending to. The cell's ceiling is paper by
    /// construction, which made that latent rather than live — but the safety
    /// rules keep three independent layers precisely because "structurally
    /// impossible today" and "checked" are different guarantees.
    ///
    /// Deliberately **stricter than the order manager**, which admits a live
    /// venue once the level is live. A cell has no central check to fall back
    /// on, so no ceiling makes a live venue admissible here and the refusal is
    /// unconditional on the class. The ceiling is read to be *recorded* — an
    /// operator reading the chain needs to know what posture was in force when
    /// the order was stopped — and not to decide, because a ceiling in the
    /// condition would be a live path waiting for a future constructor.
    ///
    /// `release_at` is the instant the gateway is told not to release the
    /// order before (ADR 0084); `now` is the pass instant the refusal, if
    /// any, is journaled at. The two differ by the leg's offset on its
    /// cycle's release schedule and by nothing else.
    #[allow(clippy::too_many_arguments)]
    fn send(
        &mut self,
        object_id: &ObjectId,
        venue: &VenueId,
        side: BookSide,
        quantity: Decimal,
        price: Decimal,
        now: Timestamp,
        release_at: Timestamp,
        gateway: &mut dyn Placer,
    ) -> Result<(String, bool)> {
        let simulated = gateway.is_simulated();
        if !simulated {
            // Before the sequence is advanced: a refused order burns no order
            // number, so the numbering stays a record of what the cell sent.
            let reason = format!(
                "{} is a live venue and this cell's ceiling is {}; live trading is disabled. \
                 A cell decides alone and has no central check behind it, so it sends to a \
                 simulated venue or it sends nothing",
                venue.as_str(),
                self.autonomy.ceiling().as_str()
            );
            self.metrics.refusal(GATE_LIVE_VENUE);
            self.journal.record(
                Decision::Refused {
                    gate: GATE_LIVE_VENUE.to_string(),
                    reason: reason.clone(),
                },
                now,
            );
            return Err(Error::denied(reason));
        }
        self.order_sequence += 1;
        let order_id = format!("{}-{}", self.config.cell_id, self.order_sequence);
        gateway.place(
            &order_id, object_id, venue, side, quantity, price, release_at,
        )?;
        Ok((order_id, simulated))
    }

    /// Record an order the venue accepted: the open order its fills will be
    /// confirmed against, the chain entry, and the series.
    ///
    /// Nothing here is a fill. An accepted order is a resting one until the
    /// order-entry channel says otherwise, and this function once wrote the
    /// sent quantity straight into the list the reconciler compares with the
    /// venue — which is how the platform came to record trades that had not
    /// happened.
    ///
    /// `equalised` is the release schedule's own verdict on the cycle the
    /// order belongs to, journaled beside `release_at` so an operator reading
    /// the chain sees that a leg was held for four milliseconds on purpose
    /// and not that the node was slow.
    fn record_sent(&mut self, working: Working, equalised: bool, now: Timestamp) {
        let order = &working.order;
        self.journal.record(
            Decision::OrderSent {
                order_id: order.order_id.clone(),
                venue: order.venue.as_str().to_string(),
                quantity: order.quantity.to_string(),
                simulated: order.simulated,
                release_at: Some(order.release_at),
                equalised,
            },
            now,
        );
        self.metrics.order_placed(&order.venue);
        self.working.insert(order.order_id.clone(), working);
    }

    /// Whether `orders` more can be held open under [`MAX_OPEN_ORDERS`].
    fn has_open_capacity(&self, orders: usize) -> bool {
        self.working.len().saturating_add(orders) <= MAX_OPEN_ORDERS
    }

    fn refuse_for_capacity(&mut self, report: &mut WorkReport, now: Timestamp) {
        self.refuse(
            report,
            "open_orders",
            &format!(
                "the cell holds {} open order(s), the most it will track; nothing more is sent \
                 until fills or expiries settle some, because an order the cell could not hold \
                 is an order whose fill it could not attribute",
                self.working.len()
            ),
            now,
        );
    }

    // --- pricing -------------------------------------------------------------

    /// The price a net goes out at, and when the cell withdraws it, under
    /// the policy its contributors share — or a refusal and `None`.
    ///
    /// The contributors must agree: a net is one order and one order has
    /// one price, and choosing between two strategies' policies would be
    /// the cell deciding what one of them pays. The touch and the mid are
    /// read from the book now, at the instant the order goes out, because
    /// the reference price the net carries is where the size was reasoned
    /// and not where the venue will match it.
    fn resolve_pricing(
        &mut self,
        net_intent: &NetIntent,
        side: BookSide,
        quantity: Decimal,
        now: Timestamp,
        gateway: &dyn Placer,
        report: &mut WorkReport,
    ) -> Option<(Decimal, Option<Timestamp>)> {
        let mut policy: Option<PricingPolicy> = None;
        for contributor in &net_intent.contributors {
            let Some(theirs) = self.pricing_of(contributor.strategy.as_str()) else {
                // `intent_for` refuses an unpriced strategy before it can
                // contribute, so this is a contributor that was undeployed
                // between phases. Refused, not defaulted.
                self.refuse(
                    report,
                    "pricing",
                    &format!(
                        "contributor {} names no pricing policy at the instant the net is placed",
                        contributor.strategy.as_str()
                    ),
                    now,
                );
                return None;
            };
            match policy {
                None => policy = Some(theirs),
                Some(agreed) if agreed == theirs => {}
                Some(agreed) => {
                    self.refuse(
                        report,
                        "pricing_conflict",
                        &format!(
                            "the net on {} at {} carries {} contributors and they do not agree \
                             how to price it ({} and {}); one order has one price, and the \
                             cell does not choose whose",
                            net_intent.object_id.as_str(),
                            net_intent.venue.as_str(),
                            net_intent.contributors.len(),
                            agreed.as_str(),
                            theirs.as_str()
                        ),
                        now,
                    );
                    return None;
                }
            }
        }
        let policy = policy?;

        let book = self
            .liquidity
            .get(&net_intent.venue, &net_intent.object_id)
            .map(|state| (state.best_bid(), state.best_ask(), state.mid()));
        let Some((bid, ask, mid)) = book else {
            self.refuse(
                report,
                "book",
                "the cell holds no book for the instrument at the instant the net is placed",
                now,
            );
            return None;
        };

        match policy {
            PricingPolicy::Marketable => {
                let touch = match side {
                    BookSide::Ask => ask,
                    BookSide::Bid => bid,
                };
                let Some(touch) = touch else {
                    self.refuse(
                        report,
                        feasibility::GATE_DEPTH,
                        &format!(
                            "nothing rests at the touch on the side the net on {} would take at {}",
                            net_intent.object_id.as_str(),
                            net_intent.venue.as_str()
                        ),
                        now,
                    );
                    return None;
                };
                if quantity > touch.size {
                    self.refuse(
                        report,
                        feasibility::GATE_DEPTH,
                        &format!(
                            "the net of {quantity} on {} exceeds the {} resting at the touch at {}; \
                             the net is refused rather than reduced or walked deeper, because a \
                             reduced order is a size nobody reasoned about and a deeper one is a \
                             price nobody did",
                            net_intent.object_id.as_str(),
                            touch.size,
                            net_intent.venue.as_str()
                        ),
                        now,
                    );
                    return None;
                }
                Some((touch.price, None))
            }
            PricingPolicy::RestAtMid { time_to_live } => {
                if !gateway.can_cancel() {
                    self.refuse(
                        report,
                        "pricing",
                        &format!(
                            "the net on {} at {} would rest and this gateway cannot withdraw an \
                             order; a resting order nothing can withdraw is refused rather than \
                             left to fill at a price the market has since left",
                            net_intent.object_id.as_str(),
                            net_intent.venue.as_str()
                        ),
                        now,
                    );
                    return None;
                }
                let Some(mid) = mid else {
                    self.refuse(report, "pricing", "the book serves no mid to rest at", now);
                    return None;
                };
                // The mid is between two grid prices and need not be on the
                // grid itself; the venue would refuse it, and the cell says
                // so first under the gate the feasibility rule names.
                let tick = feasibility::tick_for(
                    self.config.feasibility.get(net_intent.venue.as_str()),
                    self.feasibility_constraints(),
                    net_intent.venue.as_str(),
                    &net_intent.object_id,
                );
                match tick {
                    Err(infeasible) => {
                        self.refuse(report, infeasible.gate, &infeasible.reason, now);
                        return None;
                    }
                    Ok(Some(tick)) if mid.floor_to_step(tick) != mid => {
                        self.refuse(
                            report,
                            feasibility::GATE_TICK,
                            &format!(
                                "the mid {mid} is not on the {tick} tick grid for {} at {}; an order \
                                 cannot rest there and the price is refused rather than rounded",
                                net_intent.object_id.as_str(),
                                net_intent.venue.as_str()
                            ),
                            now,
                        );
                        return None;
                    }
                    Ok(_) => {}
                }
                Some((mid, Some(now.saturating_add(time_to_live))))
            }
        }
    }

    /// Whether an order of the cell's own rests on the other side of this
    /// net's instrument at its venue.
    ///
    /// Netting prevents two strategies crossing each other within a pass;
    /// a resting order from an earlier pass is the same self-trade one pass
    /// later, and the venue would match it. Refused, not withdrawn: the
    /// resting order has a time to live somebody chose.
    fn would_self_trade(&self, net_intent: &NetIntent, side: BookSide) -> bool {
        self.working.values().any(|working| {
            let order = &working.order;
            order.closed.is_none()
                && order.venue == net_intent.venue
                && order.object_id == net_intent.object_id
                && order.side != side
                && order.remaining().is_positive()
        })
    }

    /// Withdraw every resting order whose time to live has elapsed — or, when
    /// the cell is halted, every resting order it holds.
    ///
    /// Returns the ids withdrawn. The cancel goes through the gateway to
    /// the venue and the venue's answer — what was still open — closes the
    /// order as `expired`; a cancel the venue refuses leaves an order whose
    /// state the cell does not know, which is a break and halts the cell.
    /// A fill that landed between the last report and the cancel is
    /// confirmed straight afterwards, so the order settles with everything
    /// the venue did to it.
    ///
    /// **The halted arm is §29.2's mass cancel, and this is where it is
    /// wired.** A halted cell places nothing, and until this existed it also
    /// withdrew nothing but the orders whose own clock happened to run out —
    /// so the kill switch stopped the cell from adding exposure and left
    /// every order already resting at a venue to be filled by a market the
    /// cell had stopped watching. This is the seam because it is the one the
    /// composition root calls on every pass including the halted ones
    /// (`qip-edge-node`'s `run_pass` returns before `Cell::work` when the cell
    /// is halted, and calls this first): a mass cancel that only ran inside
    /// `work` would never run at all in the state it exists for.
    pub fn withdraw_expired(&mut self, gateway: &mut dyn Placer, now: Timestamp) -> Vec<String> {
        if self.is_halted() {
            return self.mass_cancel(gateway, now);
        }
        let due: Vec<String> = self
            .working
            .values()
            .filter(|working| {
                working.order.closed.is_none()
                    && working
                        .order
                        .expires_at
                        .is_some_and(|expires_at| expires_at <= now)
            })
            .map(|working| working.order.order_id.clone())
            .collect();
        self.withdraw_all(due, gateway, now)
    }

    /// Withdraw every order this cell has resting, whatever its time to live
    /// (§29.2).
    ///
    /// One withdrawal per resting order rather than one venue-wide message,
    /// because the `Placer` seam has no venue-wide cancel and inventing one
    /// would be a venue capability the cell asserts and the gateway does not
    /// have. Each still costs a message, and each is drawn from the part of
    /// the budget placements may not touch — which is what the withdrawal
    /// reserve is for, and why quoting narrows before cancelling does.
    ///
    /// A gateway with no cancel path withdraws nothing and says so in the
    /// chain. That is not a silent no-op: it is the cell stating that it is
    /// halted and cannot reduce its own exposure, which is the single fact an
    /// operator most needs from a halted cell. Calling `cancel` on such a
    /// gateway would instead turn every halted pass into a reconciliation
    /// break, which halts a cell that is already halted and buries the real
    /// finding under repetition.
    pub fn mass_cancel(&mut self, gateway: &mut dyn Placer, now: Timestamp) -> Vec<String> {
        let open: Vec<String> = self
            .working
            .values()
            .filter(|working| working.order.closed.is_none())
            .map(|working| working.order.order_id.clone())
            .collect();
        if open.is_empty() {
            return Vec::new();
        }
        if !gateway.can_cancel() {
            self.journal.record(
                Decision::Refused {
                    gate: GATE_MASS_CANCEL.to_string(),
                    reason: format!(
                        "the cell holds {} resting order(s) and the gateway it was handed has no \
                         cancel path to the venue, so a mass cancel withdraws nothing; the \
                         exposure stands until the venue closes those orders itself",
                        open.len()
                    ),
                },
                now,
            );
            return Vec::new();
        }
        self.withdraw_all(open, gateway, now)
    }

    /// Withdraw `due`, whatever put each of them on the list.
    ///
    /// The one place a cancel is sent, so the budget is spent once per message
    /// and the two entry points cannot disagree about what a withdrawal costs.
    fn withdraw_all(
        &mut self,
        due: Vec<String>,
        gateway: &mut dyn Placer,
        now: Timestamp,
    ) -> Vec<String> {
        let mut withdrawn = Vec::new();
        let mut venue_left_open = Vec::new();
        for order_id in due {
            let Some(working) = self.working.get(&order_id) else {
                continue;
            };
            let venue = working.order.venue.clone();
            let object_id = working.order.object_id.clone();
            // An order past its own time to live is journaled as expired
            // whatever else is going on, because that is what happened to it;
            // everything else on the list during a halt was pulled by the
            // halt. Two causes, two entries, and an incident review can tell
            // routine housekeeping from a kill switch emptying the book.
            let expired = working
                .order
                .expires_at
                .is_some_and(|expires_at| expires_at <= now);
            // §29.2: a cancel is a message the venue's rate limit counts, and
            // it is drawn from the reserve placements may not spend. A cancel
            // the budget cannot fund leaves the order open and is journaled
            // as such — the order is withdrawn on a later pass, which is the
            // honest answer, where quietly sending it anyway would be the
            // cell deciding the venue's limit does not apply to it.
            match self.budget.admit(&venue, MessageKind::Withdrawal, now) {
                Admission::Admitted { .. } => {
                    self.metrics.message_sent(&venue, MessageKind::Withdrawal);
                }
                Admission::Refused { reason } => {
                    // Recorded directly rather than through `Cell::refuse`:
                    // there is no `WorkReport` on this path. See
                    // [`GATE_QUOTE_BUDGET`] for the enumeration this makes a
                    // constant necessary for.
                    self.metrics.refusal(GATE_QUOTE_BUDGET);
                    self.journal.record(
                        Decision::Refused {
                            gate: GATE_QUOTE_BUDGET.to_string(),
                            reason,
                        },
                        now,
                    );
                    continue;
                }
            }
            match gateway.cancel(&order_id, &object_id, &venue, now) {
                Ok(remaining) => {
                    if let Some(working) = self.working.get_mut(&order_id) {
                        working.order.closed =
                            Some(if expired { "expired" } else { "mass_cancel" }.to_string());
                    }
                    if expired {
                        self.journal.record(
                            Decision::OrderExpired {
                                order_id: order_id.clone(),
                                venue: venue.as_str().to_string(),
                                withdrawn: remaining.to_string(),
                            },
                            now,
                        );
                        self.metrics.order_expired(&venue);
                    } else {
                        self.journal.record(
                            Decision::MassCancelled {
                                order_id: order_id.clone(),
                                venue: venue.as_str().to_string(),
                                withdrawn: remaining.to_string(),
                            },
                            now,
                        );
                        self.metrics.order_mass_cancelled(&venue);
                    }
                    venue_left_open.push((order_id.clone(), remaining));
                    withdrawn.push(order_id);
                }
                Err(error) => {
                    self.break_on(
                        format!(
                            "order {order_id} on {} was withdrawn and the venue refused to \
                             withdraw it: {}; whether it is still working is unknown",
                            venue.as_str(),
                            error.message()
                        ),
                        now,
                    );
                }
            }
        }
        if !withdrawn.is_empty() {
            self.confirm_execution_reports(gateway, now);
        }
        // Only after the last reports are in: an order the venue says it
        // withdrew whole, on which no report ever named a fill, did not run,
        // and the region's capital it committed goes back. Anything less —
        // a partial fill on either channel — is a position, and the whole
        // commit stays spent, which is the conservative reading.
        for (order_id, remaining) in venue_left_open {
            self.return_region_capital_for_unfilled(&order_id, remaining, now);
        }
        withdrawn
    }

    /// Give a withdrawn order's committed region capital back, when nothing
    /// of it filled.
    ///
    /// Without this a disconnected cell resting orders the market walks away
    /// from spends its region on orders that never became positions, and its
    /// second proposal is refused against capital nothing is holding — "bill
    /// what ran, not what was planned". The venue's own answer to the cancel
    /// and the cell's fill record must both say nothing filled; either alone
    /// could be ahead of the other by one report.
    fn return_region_capital_for_unfilled(
        &mut self,
        order_id: &str,
        venue_remaining: Decimal,
        now: Timestamp,
    ) {
        let Some(working) = self.working.get(order_id) else {
            return;
        };
        let unfilled_everywhere =
            working.order.filled.is_zero() && venue_remaining == working.order.quantity;
        if !unfilled_everywhere || !working.region_committed.is_positive() {
            return;
        }
        let amount = working.region_committed;
        let outcome = match self.region_allocation.as_ref() {
            None => return,
            Some(allocation) => allocation.return_committed(amount),
        };
        match outcome {
            Ok(()) => {
                if let Some(working) = self.working.get_mut(order_id) {
                    // Returned once. A closed order is never due again, and
                    // this zero holds the line if that ever stopped being
                    // true: the ledger's own bound would only catch a second
                    // return while nothing else was committed.
                    working.region_committed = Decimal::ZERO;
                }
            }
            Err(error) => {
                self.journal.record(
                    Decision::Refused {
                        gate: "region_reservation_return".to_string(),
                        reason: format!(
                            "order {order_id} expired unfilled and its {amount} could not be \
                             returned to the region allocation: {}",
                            error.message()
                        ),
                    },
                    now,
                );
            }
        }
    }

    // --- fills: the venue's facts ------------------------------------------

    /// Absorb what the order-entry channel has reported since the last call.
    ///
    /// Returns the fills confirmed, attributed. Safe to call on a halted
    /// cell and meant to be: a halted cell learns what filled so the
    /// reconciler is comparing a record and not a memory. Every report is
    /// judged against the open-order record and a report that names an
    /// order the cell never sent, or fills one past its size, is a break —
    /// the venue's channel disagreeing with the cell's own record is the
    /// same failure the drop copy exists to catch, arriving on the other
    /// channel.
    pub fn confirm_execution_reports(
        &mut self,
        gateway: &mut dyn Placer,
        now: Timestamp,
    ) -> Vec<ConfirmedFill> {
        // What the gateway withdrew unreleased, before what it reports
        // filled: an order on this list never reached the venue, so a report
        // naming it afterwards is a second disagreement rather than a fill.
        for unreleased in gateway.unreleased() {
            self.withdraw_unreleased(unreleased, now);
        }
        let mut confirmed = Vec::new();
        for execution in gateway.execution_reports() {
            if let Some(fill) = self.confirm(execution, now) {
                confirmed.push(fill);
            }
        }
        confirmed
    }

    /// Book a gateway's withdrawal of an order it never released (ADR 0084
    /// §4), and stop the cell.
    ///
    /// Stopped rather than tidied, because the cell's chain says the order
    /// was sent and the venue never saw it — the same disagreement between
    /// the cell's record and a venue channel that every other break is. For
    /// a cycle leg it is also a position: the legs released on time are out
    /// against one that is not, which is the exposure the schedule was
    /// computed to prevent. The order is closed on the record so the
    /// reconciler does not hold it against the venue, and what the region
    /// committed for it is returned, because nothing was spent.
    fn withdraw_unreleased(&mut self, unreleased: UnreleasedOrder, now: Timestamp) {
        let reason = format!(
            "order {} for {} was to be released no earlier than {} and the gateway found that \
             instant {} ms past on the first pass that could have released it, past the {} ms \
             it will send late; withdrawn rather than sent late, because a leg arriving outside \
             the window its cycle was admitted on is the exposure the release schedule exists to \
             prevent",
            unreleased.order_id,
            unreleased.venue.as_str(),
            unreleased.scheduled.to_rfc3339(),
            unreleased.lag.as_millis(),
            unreleased.tolerance.as_millis()
        );
        // Recorded directly rather than through `Cell::refuse`: there is no
        // `WorkReport` on this path. See [`GATE_RELEASE_LATE`] for the
        // enumeration this makes a constant necessary for.
        self.metrics.refusal(GATE_RELEASE_LATE);
        self.journal.record(
            Decision::Refused {
                gate: GATE_RELEASE_LATE.to_string(),
                reason,
            },
            now,
        );
        let detail = match self.working.get_mut(&unreleased.order_id) {
            Some(working) if working.order.closed.is_none() => {
                working.order.closed = Some("unreleased".to_string());
                let quantity = working.order.quantity;
                self.return_region_capital_for_unfilled(&unreleased.order_id, quantity, now);
                format!(
                    "the cell's record says order {} was sent to {} and the gateway withdrew it \
                     unreleased, so it never reached the venue",
                    unreleased.order_id,
                    unreleased.venue.as_str()
                )
            }
            Some(_) => format!(
                "the gateway reports withdrawing order {} unreleased and the cell's record \
                 already has it closed, so the two disagree about whether it ever reached {}",
                unreleased.order_id,
                unreleased.venue.as_str()
            ),
            None => format!(
                "the gateway reports withdrawing order {} unreleased at {} and the cell has no \
                 open order under that id",
                unreleased.order_id,
                unreleased.venue.as_str()
            ),
        };
        self.break_on(detail, now);
    }

    fn confirm(&mut self, execution: ExecutionReport, now: Timestamp) -> Option<ConfirmedFill> {
        if !execution.quantity.is_positive() || !execution.price.is_positive() {
            self.break_on(
                format!(
                    "the order-entry channel reports {} at {} on order {}; a fill needs both \
                     positive, and one that is not is a record the cell cannot book",
                    execution.quantity, execution.price, execution.order_id
                ),
                now,
            );
            return None;
        }
        let Some(working) = self.working.get_mut(&execution.order_id) else {
            self.break_on(
                format!(
                    "the order-entry channel reports a fill of {} on order {} at {} and the cell \
                     has no open order under that id",
                    execution.quantity,
                    execution.order_id,
                    execution.venue.as_str()
                ),
                now,
            );
            return None;
        };
        if working.order.venue != execution.venue {
            let detail = format!(
                "the order-entry channel reports order {} filled at {} and the cell sent it to {}",
                execution.order_id,
                execution.venue.as_str(),
                working.order.venue.as_str()
            );
            self.break_on(detail, now);
            return None;
        }

        // The fill is booked whatever the size check below says: the venue
        // reports it traded, and a position the cell refuses to believe in
        // is the position nobody is watching.
        //
        // The release instant and not the decision instant, for the fill
        // time below: see `OpenOrder::release_at` for the tail the equaliser
        // would otherwise chase.
        let release_at = working.order.release_at;
        let was_complete = working.order.filled >= working.order.quantity;
        working.order.filled += execution.quantity;
        // §32.1's measurement is of a *completed* order: the leg stops
        // being an exposure when the last of it has traded, and a first
        // partial report would time how long the venue took to start
        // rather than how long the cell was exposed.
        let completed = !was_complete && working.order.filled >= working.order.quantity;
        let overfilled = working.order.filled > working.order.quantity;
        if working.order.filled >= working.order.quantity {
            working.order.closed = Some("filled".to_string());
        }
        let shares = working.net.split_fill(execution.quantity);
        let fill = ConfirmedFill {
            order_id: execution.order_id.clone(),
            venue: execution.venue.clone(),
            object_id: working.order.object_id.clone(),
            side: working.order.side,
            quantity: execution.quantity,
            price: execution.price,
            simulated: working.order.simulated,
            at: execution.at,
            shares,
        };
        let overfill_detail = overfilled.then(|| {
            format!(
                "order {} was sent for {} and the order-entry channel has now reported {} filled",
                fill.order_id, working.order.quantity, working.order.filled
            )
        });

        let signed = if matches!(fill.side, BookSide::Ask) {
            fill.quantity
        } else {
            -fill.quantity
        };
        *self
            .positions
            .entry(Self::position_key(&fill.venue, &fill.object_id))
            .or_insert(Decimal::ZERO) += signed;
        // Each contributor's share of the same fill, to its own lot at this
        // cell. `split_fill` makes the shares sum to the fill, so the
        // per-strategy book and the venue-facing aggregate move by the same
        // total; see `strategy_position` for why the share is booked here.
        for (strategy, share) in &fill.shares {
            let signed_share = if matches!(fill.side, BookSide::Ask) {
                *share
            } else {
                -*share
            };
            *self
                .strategy_positions
                .entry(Self::strategy_position_key(
                    strategy,
                    &fill.venue,
                    &fill.object_id,
                ))
                .or_insert(Decimal::ZERO) += signed_share;
        }
        self.journal.record(
            Decision::Filled {
                order_id: fill.order_id.clone(),
                venue: fill.venue.as_str().to_string(),
                object: fill.object_id.as_str().to_string(),
                quantity: fill.quantity.to_string(),
                price: fill.price.to_string(),
                simulated: fill.simulated,
                shares: fill
                    .shares
                    .iter()
                    .map(|(strategy, share)| (strategy.as_str().to_string(), share.to_string()))
                    .collect(),
            },
            now,
        );
        self.metrics.fill_confirmed(&fill.venue);
        // §29.2's denominator. A trade is what the venue says filled and
        // nothing else — an order the cell sent is a message, and counting
        // it as its own trade would make every message-to-trade ratio read
        // as healthy by construction.
        self.budget.observe_trade(&fill.venue);
        if completed {
            // The venue's own instant for the fill against the instant the
            // cell released the order at. A report claiming to predate its
            // release is counted as an anomaly by `FillTimes` rather than
            // recorded as a fast venue.
            let taken = fill.at.since(release_at);
            self.fill_times.observe(&fill.venue, taken);
            self.metrics.fill_time(&fill.venue, taken);
        }
        self.confirmed.push(fill.clone());
        if let Some(detail) = overfill_detail {
            self.break_on(detail, now);
        }
        Some(fill)
    }

    /// Record a disagreement between the cell's record and a venue channel,
    /// and halt on the first.
    ///
    /// Tripping needs no authority, which is why a break can act immediately:
    /// the cost of a false stop is minutes of missed opportunity, the cost of
    /// trading on a book that disagrees with the venue is unbounded.
    fn break_on(&mut self, detail: String, now: Timestamp) {
        if self.breaks.len() < MAX_RETAINED_BREAKS {
            self.breaks.push(detail.clone());
        } else {
            self.breaks_omitted = self.breaks_omitted.saturating_add(1);
        }
        self.metrics.reconciliation_break();
        self.journal
            .record(Decision::ReconciliationBreak { detail }, now);
        if !self.is_halted() {
            self.autonomy.kill_switch_mut().trip_global(
                now,
                "drop-copy",
                "the cell's fills disagree with the venue's own account",
            );
            self.journal.record(
                Decision::HaltChanged {
                    halted: true,
                    reason: "reconciliation break".to_string(),
                },
                now,
            );
            // A break halts the cell, which stops it running passes. Without
            // this the gauge would keep reporting the state of the last pass
            // that ran, which is the one before the break.
            self.record_halt();
        }
    }

    /// Retire every closed order after a clean comparison.
    ///
    /// Only after a clean one: a closed order whose fills the venue has not
    /// yet matched is exactly the order the next comparison has to see. The
    /// journal already holds every fill retired here.
    fn settle(&mut self) {
        let closed: Vec<String> = self
            .working
            .iter()
            .filter(|(_, working)| working.order.closed.is_some())
            // §32.1: never the order a rested cycle is waiting on. Retiring
            // it would delete the only record of what its venue completed,
            // and the fraction the rest of that cycle must be sized to would
            // have to be guessed from the journal or not at all.
            .filter(|(order_id, _)| !self.awaits_rested_leg(order_id))
            .map(|(order_id, _)| order_id.clone())
            .collect();
        for order_id in closed {
            self.working.remove(&order_id);
            self.confirmed.retain(|fill| fill.order_id != order_id);
            self.dropcopy.retire(&order_id);
        }
    }

    // --- the arbitrage desk --------------------------------------------------

    /// Re-quote the graph from the books, scan it, and admit what survives.
    ///
    /// Returns the cycles to send once the nets have gone, each already past
    /// the feasibility gate and the desk's capital envelope. Everything the
    /// scan refused is journaled under the stage that refused it, every
    /// opportunity found is journaled as priced whether or not it is taken,
    /// and everything past the cap is refused and counted — a scan that
    /// found nothing and said nothing is indistinguishable from one that did
    /// not run.
    ///
    /// Two narrowings stop the scan outright. The degradation table's pause
    /// applies to the desk as to any price-only strategy. And a sizing
    /// multiplier below one opens no cycle at all, rather than a smaller one:
    /// the scanner prices at the size policy's size, edge is not linear in
    /// size, and a cycle re-priced narrower is a different cycle whose legs
    /// no longer close on what was priced. Stopping is the fail-closed
    /// reading of §6.2 for a family whose trades cannot be scaled after the
    /// fact.
    /// Route one found cycle through §30.2's router and then §33.1's
    /// extension for whatever path it was assigned.
    ///
    /// `&self` throughout, deliberately: this reads the graph, the cell's own
    /// book, its confirmed positions, its installed mirror arrangement and
    /// the last policy payload, and writes none of them. Nothing here can
    /// name a venue the desk's graph does not already reach —
    /// [`Self::install_arbitrage`] refused that graph if it did — and nothing
    /// here can produce an order.
    fn route_one(
        &self,
        installed: &InstalledDesk,
        opportunity: &Opportunity,
        now: Timestamp,
    ) -> RoutedOutcome {
        let edges = &opportunity.candidate.edges;
        // The composition is built twice, deliberately, and the second one is
        // the router's own.
        //
        // This one exists because the mirror facts are keyed by a mirror
        // edge's position *in the composition's traversal*, so they cannot be
        // built until the composition is. The second is inside
        // [`CycleRouter::route`], which stays the single entry point by which
        // an assignment leaves the router: reaching past it to
        // `qip_routing::path::assign` would give this file a second way to
        // produce one, and the router's own entry point is where a later
        // change to how a cycle is assigned will land. `composition` is a
        // pure function of the graph, the edge list and the region map — all
        // three unchanged between the two calls — so the two are the same
        // composition by construction rather than by coincidence. The cost is
        // one extra walk of at most `MAX_COMPOSITION_EDGES` edges per cycle.
        let composition = match installed.router.composition(installed.desk.graph(), edges) {
            Ok(composition) => composition,
            Err(refusal) => return RoutedOutcome::RouterRefused(refusal),
        };
        // §36.3, before the facts are assembled and before the router runs: a
        // mirror whose other side is in a region that has gone dark is
        // suspended rather than routed. Checked here rather than inside
        // `mirror_facts_for` so the refusal is charted under its own gate —
        // "the router had no row for this" and "the cell on the other end is
        // not answering" send an operator to two different places.
        if let Some(refusal) = self.dark_mirror(&composition) {
            return RoutedOutcome::RegionDark(refusal);
        }
        // Read once and handed to both the router and the extension, because
        // they ask two questions of the same books and a second read between
        // them would let a cycle be assigned path 4 on a hedge the gate then
        // measures from a different snapshot.
        let hedges = match self.local_hedges_for(&composition, opportunity) {
            Ok(hedges) => hedges,
            Err(refusal) => return RoutedOutcome::RouterRefused(refusal),
        };
        let facts = match self.mirror_facts_for(&composition, &hedges) {
            Ok(facts) => facts,
            Err(refusal) => return RoutedOutcome::RouterRefused(refusal),
        };
        let assignment = match installed
            .router
            .route(installed.desk.graph(), edges, &facts)
        {
            Ok(assignment) => assignment,
            Err(refusal) => return RoutedOutcome::RouterRefused(refusal),
        };
        match self.check_extension_for(&composition, assignment.assigned(), &hedges, now) {
            Ok(verdict) => RoutedOutcome::Assigned(assignment, verdict),
            Err(refusal) => RoutedOutcome::ExtensionRefused(assignment, refusal),
        }
    }

    /// The refusal for the first mirror edge of this composition that reaches
    /// a dark region, if any does (§36.3).
    ///
    /// Both ends are examined rather than only the far one. A composition
    /// whose mirror edge has neither end at home is already refused by
    /// [`Self::mirror_ends`], and reaching for "the remote end" before that
    /// check would be this file deciding which end is remote twice, in two
    /// places, with one of them wrong the day a whitelist names two foreign
    /// venues.
    fn dark_mirror(&self, composition: &Composition) -> Option<Error> {
        let home = self.config.region.as_str();
        for index in composition.mirror_edges() {
            let Some(edge) = composition.edges().get(index) else {
                continue;
            };
            for end in [edge.from(), edge.to()] {
                let region = end.region.as_str();
                if region != home && self.is_region_dark(region) {
                    return Some(Error::denied(format!(
                        "the mirrored leg {} -> {} reaches region {region}, and {}; §36.3 \
                         suspends every mirror involving a dark region rather than taking one \
                         side of a trade whose other side is nobody. This cell's local \
                         strategies and its intra-venue cycles are unaffected, and the mirror \
                         resumes when that region does",
                        edge.from().label(),
                        edge.to().label(),
                        self.outlook.describe()
                    )));
                }
            }
        }
        None
    }

    /// Which end of a mirror edge this cell is standing on, and which way it
    /// therefore trades the mirrored asset locally.
    ///
    /// The direction is read off the traversal and is not a guess. A mirror
    /// edge whose `from` is local is the asset **leaving** this region, and
    /// it got here by being acquired — the conversion before it bought it —
    /// so locally this region buys. A mirror edge whose `to` is local is the
    /// asset **arriving**, and the conversion after it spends it, so locally
    /// this region sells. Inverting that inverts every band gate, which is
    /// why it is derived from the edge rather than configured.
    fn mirror_ends<'a>(&self, edge: &'a CompositionEdge) -> Result<(&'a PathEndpoint, Direction)> {
        let home = self.config.region.as_str();
        match (
            edge.from().region.as_str() == home,
            edge.to().region.as_str() == home,
        ) {
            (true, false) => Ok((edge.from(), Direction::Buy)),
            (false, true) => Ok((edge.to(), Direction::Sell)),
            // Neither end local is the reachable case: a whitelist naming
            // two foreign venues. Both ends local cannot happen —
            // `CompositionEdge::new` refuses a mirror whose ends share a
            // region — and the two share an arm rather than giving the
            // impossible one a branch no input reaches.
            (false, false) | (true, true) => Err(Error::denied(format!(
                "mirror edge {} -> {} has no end in this cell's region {home}; §31.1 gates each \
                 region's own side and this cell is on neither, so it cannot say which \
                 direction its band permits — route the cycle at a cell that is on one",
                edge.from().label(),
                edge.to().label()
            ))),
        }
    }

    /// A local hedge for one mirror edge, as this cell's own books show it.
    ///
    /// §30.2's row 4 and §33.1's path-4 check ask two different questions of
    /// the same book, and the split is the one §31.1 already makes for row 3:
    /// the router asks whether a hedge *exists* to bridge with, and the gate
    /// asks whether it is deep enough for the leg that will need it. Both are
    /// answered here, from this cell's own liquidity and nothing else — no
    /// venue tells the cell, no policy slot carries it, and nothing reads a
    /// clock, so a replay of the same books produces the same answer.
    ///
    /// Three conditions, each of which can be false on its own:
    ///
    /// * The venue is in **this cell's own region**. Path 4 is "execute
    ///   locally, hedge locally, complete remotely later"; a hedge across the
    ///   same boundary the cycle is bridging is the exposure again, not cover
    ///   for it.
    /// * The venue is **not the mirror edge's own local venue**. An
    ///   offsetting order in the same book at the same instant is not a
    ///   hedge, it is the local leg not being done — and admitting it would
    ///   make row 4 eligible for every cycle, which is the shape of control
    ///   this repository has shipped once already.
    /// * The book is **usable and absorbs something on the side the hedge
    ///   takes**, which is the side opposite the one the local leg trades:
    ///   a local buy is covered by a sale, and a sale consumes bids.
    ///   `CellLiquidity` answers nothing from a stale or unpriceable book, so
    ///   a hedge remembered from before a gap is never offered as one.
    ///
    /// The depth reported is what the book would actually **fill** at the
    /// size the first leg needs, from the sweep rather than from the touch:
    /// "available at depth" is a question about the book and not about its
    /// first level, and a touch of one lot above a hollow book would clear a
    /// gate that exists to refuse exactly that. Where several local venues
    /// qualify the deepest is taken, ties going to the first in the cell's
    /// configured venue order, so the choice is fixed at deployment rather
    /// than by whatever order a map happened to iterate in.
    fn local_hedges_for(
        &self,
        composition: &Composition,
        opportunity: &Opportunity,
    ) -> Result<BTreeMap<usize, LocalHedge>> {
        let mirrors = composition.mirror_edges();
        if mirrors.is_empty() {
            return Ok(BTreeMap::new());
        }
        // §33.1 words it "before the first leg", so the size hedged is the
        // first leg's own. A plan with no steps states no size, and a cell
        // that invented one would be gating the hedge against a number
        // nobody computed.
        let Some(required) = opportunity
            .planned
            .plan
            .steps()
            .first()
            .map(|step| step.quantity)
            .filter(|quantity| quantity.is_positive())
        else {
            return Ok(BTreeMap::new());
        };
        let Some(arrangement) = self.mirror.as_ref() else {
            return Ok(BTreeMap::new());
        };
        let home = self.config.region.as_str();
        let mut hedges = BTreeMap::new();
        for index in mirrors {
            let Some(edge) = composition.edges().get(index) else {
                return Err(Error::invalid(format!(
                    "the composition reports a mirror edge at {index} and holds {} edges; route \
                     the cycle against the composition it was built from",
                    composition.edges().len()
                )));
            };
            let (local, intended) = self.mirror_ends(edge)?;
            let Some(discipline) = arrangement.instrument(&local.object) else {
                continue;
            };
            let market = discipline.market();
            // The hedge offsets the local leg, so it takes the other side,
            // and a sale consumes the bids.
            let side = match intended {
                Direction::Buy => BookSide::Bid,
                Direction::Sell => BookSide::Ask,
            };
            let mut deepest: Option<LocalHedge> = None;
            for venue in &self.config.venues {
                if venue == &local.venue {
                    continue;
                }
                let region = self
                    .config
                    .venue_regions
                    .get(venue.as_str())
                    .map_or(home, String::as_str);
                if region != home {
                    continue;
                }
                let Some((_, filled)) = self.liquidity.sweep_cost(venue, market, side, required)
                else {
                    continue;
                };
                if deepest
                    .as_ref()
                    .is_none_or(|current| filled > current.depth)
                {
                    deepest = Some(LocalHedge {
                        venue: venue.clone(),
                        depth: filled,
                        required,
                    });
                }
            }
            if let Some(hedge) = deepest {
                hedges.insert(index, hedge);
            }
        }
        Ok(hedges)
    }

    /// The mirror-edge facts §30.2 tells rows 3 to 6 apart by, as this cell
    /// can honestly state them.
    ///
    /// Two of the five are `false` or `None` and each is a statement rather
    /// than a placeholder: the cell holds no connectivity fact about whether
    /// a remote venue accepts a resting order (row 5), and it receives no
    /// firm quote from one (row 6). **Rows 5 and 6 are therefore unreachable
    /// from a cell**, and that is the fail-closed answer rather than a
    /// delivered row: a cycle whose only possible path was one of the two is
    /// refused whole by the router.
    ///
    /// Row 3 is reached through `MirrorFacts::established_mirror`: the centre
    /// named an inventory target for the mirrored instrument and this cell's
    /// operator configured a band for it, which together are §31.1's SETUP.
    /// Where the local side sits inside that band is not asked here — that
    /// is §33.1's extension, after the path is assigned.
    ///
    /// Row 4 is reached through [`Self::local_hedges_for`]. This sentence
    /// read "a cell measures no hedge book beside the cycle's own
    /// instruments, so it cannot say a local hedge is available" until a
    /// mirrored cell had books for every venue its operator configured, which
    /// is more than the cycle's own; the literal `false` that outlived it
    /// made row 4 unreachable and §33.1's hedge-at-depth check a gate no
    /// input could ever put a fact in front of.
    fn mirror_facts_for(
        &self,
        composition: &Composition,
        hedges: &BTreeMap<usize, LocalHedge>,
    ) -> Result<BTreeMap<usize, MirrorFacts>> {
        let mirrors = composition.mirror_edges();
        if mirrors.is_empty() {
            return Ok(BTreeMap::new());
        }
        let Some(arrangement) = self.mirror.as_ref() else {
            return Err(Error::denied(
                "this cycle crosses a region boundary and no §31.1 mirror arrangement is \
                 installed; a cell with none cannot say what inventory it is meant to hold on \
                 its own side, so install the arrangement or do not place a venue in another \
                 region",
            ));
        };
        let targets = self.inventory_targets();
        let mut facts = BTreeMap::new();
        for index in mirrors {
            let Some(edge) = composition.edges().get(index) else {
                return Err(Error::invalid(format!(
                    "the composition reports a mirror edge at {index} and holds {} edges; route \
                     the cycle against the composition it was built from",
                    composition.edges().len()
                )));
            };
            let (local, _) = self.mirror_ends(edge)?;
            // The remote region is whichever end is not the local one, and
            // the round trip to it must have been measured. Refused rather
            // than defaulted: `MirrorFacts::new` refuses a round trip of
            // zero, and a default here would be a number nobody measured
            // sitting where §30.2's row 6 is decided.
            let remote = if local == edge.from() {
                edge.to()
            } else {
                edge.from()
            };
            let round_trip = arrangement.round_trip(remote.region.as_str())?;
            let established = arrangement.instrument(&local.object).is_some()
                && targets.is_some_and(|(targets, _)| {
                    targets.targets.contains_key(local.object.as_str())
                });
            facts.insert(
                index,
                MirrorFacts::new(false, hedges.contains_key(&index), false, None, round_trip)?
                    .established_mirror(established),
            );
        }
        Ok(facts)
    }

    /// §33.1's extension for the assigned path, over every mirror edge the
    /// cycle carries.
    ///
    /// Every mirror edge, not the first: a cycle with two mirrored legs is
    /// two sides of two bands, and passing on the easier one would gate the
    /// cycle against half of itself — the same argument `eligible_paths`
    /// makes for requiring every mirror edge to admit a path.
    fn check_extension_for(
        &self,
        composition: &Composition,
        path: ExecutionPath,
        hedges: &BTreeMap<usize, LocalHedge>,
        now: Timestamp,
    ) -> Result<ExtensionVerdict> {
        let mut verdict = None;
        for index in composition.mirror_edges() {
            // ADR 0079 decision five, before any band or hedge is read: a
            // mirror whose far venue sits in a region the centre has derived
            // dark is suspended whatever the path says about it, because
            // the cell on the other end may be nobody and the band gates
            // assume somebody is there to take the other side.
            self.refuse_mirror_into_centre_dark_region(composition, index)?;
            // The assigned path decides which facts are assembled, and the
            // match names all eight so a ninth cannot be added without a
            // decision here. Assembling the mirror facts whatever the path
            // was the earlier shape and it was wrong in a way that hid row 4
            // entirely: `mirror_extension_for` refuses when no inventory band
            // or target exists, which is precisely the state row 4 is
            // assigned in, so path 4 could only ever have been refused with a
            // message about a band it does not read.
            let extensions = match path {
                ExecutionPath::MirroredInventory => PathExtensions::new()
                    .with_mirror(self.mirror_extension_for(composition, index)?),
                ExecutionPath::HedgedBridging => match hedges.get(&index) {
                    // `read_this_pass` is true because the depth above was
                    // swept from `self.liquidity` inside this pass of `work`,
                    // and `CellLiquidity` serves nothing from a stale book —
                    // the two together are §33.1's "now, before the first
                    // leg". It is not a constant standing in for a fact
                    // nobody established.
                    Some(hedge) => PathExtensions::new().with_hedge(HedgeExtension::new(
                        hedge.depth,
                        hedge.required,
                        true,
                    )?),
                    // The router made this edge eligible for path 4 from the
                    // same map, so an absence here is the two disagreeing
                    // rather than a state a pass can be in. Empty facts are
                    // supplied and the gate refuses for want of them, which
                    // is the fail-closed reading; supplying an invented depth
                    // would be the other one.
                    None => PathExtensions::new(),
                },
                ExecutionPath::IntraVenue
                | ExecutionPath::CrossVenue
                | ExecutionPath::PassiveAnchoring
                | ExecutionPath::FirmQuoteBridging
                | ExecutionPath::RepresentationBasis
                | ExecutionPath::PayoffEquivalence => PathExtensions::new(),
            };
            // The mirrored instrument is prefixed onto the refusal here and
            // not inside the gate, because the gate is handed one side's
            // facts and does not know which of a cycle's legs they came from.
            // A cycle with two mirrored legs that refuses without naming one
            // leaves an operator re-deriving which band was the problem from
            // the band figures alone.
            let one = check_extension(path, &extensions, now).map_err(|refusal| {
                let named = composition.edges().get(index).map_or_else(
                    || format!("edge {index}"),
                    |edge| edge.from().object.as_str().to_string(),
                );
                // Path 4's refusal names the book that was too thin, for the
                // same reason the mirrored instrument is named at all: the
                // gate is handed sizes and no identity, so an operator told
                // only that a hedge was short would have to re-derive which
                // of the cell's local venues was measured.
                let book = match (path, hedges.get(&index)) {
                    (ExecutionPath::HedgedBridging, Some(hedge)) => {
                        format!(" against the hedge book at {}", hedge.venue.as_str())
                    }
                    _ => String::new(),
                };
                Error::denied(format!(
                    "the mirrored leg in {named} does not clear it{book}: {}",
                    refusal.message()
                ))
            })?;
            if verdict.is_none() {
                verdict = Some(one);
            }
        }
        match verdict {
            Some(verdict) => Ok(verdict),
            // No mirror edge: the path's own facts, none of which a cell
            // holds. Paths 1 and 2 return their no-row verdict here — which
            // is every cycle a single-region cell finds — and paths 4 to 8
            // refuse for facts nobody supplied.
            None => check_extension(path, &PathExtensions::new(), now),
        }
    }

    /// Refuse a mirror edge whose far venue's region the centre has derived
    /// dark, or whose far venue this cell holds no region annotation for.
    ///
    /// Read off policy slot 11's `dark_regions` — the centre's derivation
    /// from silence, subtract-only on the wire — against
    /// `CellConfig::venue_regions`, ADR 0073's operator-stated map, and not
    /// against the composition edge's own `region`: the edge's region was
    /// derived from that map at installation, so reading the map is
    /// reading the fact at its source, and a venue with no annotation is a
    /// missing fact, which ADR 0073 decision five says is a refusal rather
    /// than a home-region default. With no slot 11 applied there is nothing
    /// to read and nothing is refused here; the cell's own region wire
    /// (`dark_mirror`) still gates the same edge from its own reading.
    ///
    /// The refusal opens with [`GATE_CENTRE_DARK_REGION`]; see that
    /// constant for where it is charted.
    fn refuse_mirror_into_centre_dark_region(
        &self,
        composition: &Composition,
        index: usize,
    ) -> Result<()> {
        let Some(constraints) = self.feasibility_constraints() else {
            return Ok(());
        };
        let Some(edge) = composition.edges().get(index) else {
            // The extension assembly refuses a missing edge itself, with the
            // composition's edge count; a second refusal here would name the
            // wrong finding.
            return Ok(());
        };
        let (local, _) = self.mirror_ends(edge)?;
        let remote = if local == edge.from() {
            edge.to()
        } else {
            edge.from()
        };
        let Some(region) = self.config.venue_regions.get(remote.venue.as_str()) else {
            return Err(Error::denied(format!(
                "{GATE_CENTRE_DARK_REGION}: the mirrored leg {} -> {} reaches venue {} and this \
                 cell holds no region annotation for it, so whether the centre has derived \
                 that region dark cannot be read; annotate the venue's region in \
                 QIP_VENUE_REGIONS rather than mirroring into a region nobody named",
                edge.from().label(),
                edge.to().label(),
                remote.venue.as_str()
            )));
        };
        if constraints.dark_regions.contains(region) {
            return Err(Error::denied(format!(
                "{GATE_CENTRE_DARK_REGION}: the mirrored leg {} -> {} reaches region {region}, \
                 which the centre has derived dark — heard from once and from none of its \
                 cells within the operator's window (ADR 0079); every mirror into it is \
                 suspended until a cell there reports to the centre again, and this cell's \
                 local strategies and intra-venue cycles are unaffected",
                edge.from().label(),
                edge.to().label()
            )));
        }
        Ok(())
    }

    /// Everything §33.1's path-3 row needs for one mirror edge, assembled
    /// from the centre's tenth policy slot and this cell's own configuration
    /// and book.
    ///
    /// Every absence is a refusal. The failure this shape prevents is the
    /// one the risk rules name: a gate assembled from optional facts, each of
    /// which defaults to something permissive, reads as a control and admits
    /// everything.
    fn mirror_extension_for(
        &self,
        composition: &Composition,
        index: usize,
    ) -> Result<MirrorExtension> {
        let Some(edge) = composition.edges().get(index) else {
            return Err(Error::invalid(format!(
                "the composition reports a mirror edge at {index} and holds {} edges",
                composition.edges().len()
            )));
        };
        let (local, intended) = self.mirror_ends(edge)?;
        let Some(arrangement) = self.mirror.as_ref() else {
            return Err(Error::denied(
                "this cycle crosses a region boundary and no §31.1 mirror arrangement is \
                 installed",
            ));
        };
        let object = local.object.as_str();
        let discipline = arrangement.instrument(&local.object).ok_or_else(|| {
            Error::denied(format!(
                "{object} crosses a region boundary in this cycle and this cell holds no \
                 inventory band for it; §31.1 gates the direction from the band, and a mirrored \
                 asset with no band is one this region has no discipline for — configure the \
                 band or do not mirror it"
            ))
        })?;
        let (targets, produced_at) = self.inventory_targets().ok_or_else(|| {
            Error::denied(
                "no inventory targets have been applied, so this cell has no distributed target \
                 or reference for the asset it would mirror; §31.1 distributes both, and a \
                 region trading a mirror against neither is trading against its own opinion",
            )
        })?;
        let target = targets.targets.get(object).copied().ok_or_else(|| {
            Error::denied(format!(
                "the applied inventory targets name no target for {object}; the band is centred \
                 on the centre's target, and a region that invented its own would drift away \
                 from the one the other side is holding to"
            ))
        })?;
        let price = targets
            .reference_prices
            .get(object)
            .copied()
            .ok_or_else(|| {
                Error::denied(format!(
                    "the applied inventory targets name no reference price for {object}; §31.1's \
                 direction gating compares this region's own price against the distributed \
                 reference, and without one the two regions could read the same dislocation the \
                 same way and both take it"
                ))
            })?;
        let band = discipline.band_around(target)?;
        // The window is the slot's own — `PolicyItem::InventoryTargets`'
        // time to live — measured from when the centre produced it rather
        // than when it shipped. `Cell::inventory_targets` deliberately does
        // not pre-filter on freshness, so this is the one place the window is
        // checked and §33.1's "reference inside TTL" can actually fire.
        let reference = discipline.reference_at(
            price,
            produced_at,
            qip_contracts::policy::PolicyItem::InventoryTargets.time_to_live(),
        )?;
        let held = self.position(&local.venue, &local.object);
        let local_price = self.local_mid(&local.venue, discipline.market())?;
        Ok(MirrorExtension::new(
            band,
            held,
            reference,
            local_price,
            intended,
        ))
    }

    /// The mid of this cell's own book, for the market a mirrored instrument
    /// is priced on.
    ///
    /// Refused when either side is missing rather than falling back to the
    /// one that is there: a one-sided book has no mid, and taking the side
    /// that exists would compare a bid against a reference struck on a mid
    /// and read every thin book as a dislocation.
    fn local_mid(&self, venue: &VenueId, market: &ObjectId) -> Result<Decimal> {
        let missing = || {
            Error::denied(format!(
                "this cell's book for {} at {} is not two-sided, so it has no mid to compare \
                 against the distributed reference; a one-sided book read as a price would make \
                 every thin moment look like a dislocation",
                market.as_str(),
                venue.as_str()
            ))
        };
        let (bid, _) = self
            .liquidity
            .touch(venue, market, BookSide::Bid)
            .ok_or_else(missing)?;
        let (ask, _) = self
            .liquidity
            .touch(venue, market, BookSide::Ask)
            .ok_or_else(missing)?;
        bid.checked_add(ask)
            .and_then(|sum| sum.checked_div(Decimal::from_int(2)))
            .ok_or_else(|| {
                Error::numeric(format!(
                    "the mid of {bid} and {ask} at {} does not fit a Decimal",
                    venue.as_str()
                ))
            })
    }
    fn scan_cycles(
        &mut self,
        now: Timestamp,
        multiplier: Decimal,
        narrowing: &DegradationState,
        report: &mut WorkReport,
    ) -> Result<Vec<AdmittedCycle>> {
        let Some((cap, validity, strategy)) = self.desk.as_ref().map(|installed| {
            let desk = &installed.desk;
            (
                desk.max_cycles_per_pass(),
                desk.leg_validity(),
                desk.strategy().clone(),
            )
        }) else {
            return Ok(Vec::new());
        };
        if narrowing.pauses(StrategyClass::PriceOnly) {
            self.refuse(
                report,
                "degradation_pause",
                "the arbitrage desk pauses while its capability is degraded",
                now,
            );
            return Ok(Vec::new());
        }
        if multiplier < Decimal::ONE {
            self.refuse(
                report,
                "degradation_sizing",
                "the arbitrage desk opens no cycle while sizing is narrowed: a cycle re-priced \
                 at a narrower size is a different cycle, and the scanner priced this one at \
                 the policy's size",
                now,
            );
            return Ok(Vec::new());
        }

        let scanned = {
            // Two fields of `self`, borrowed disjointly: the desk re-quotes
            // its graph from the liquidity it is handed and never reaches
            // for it.
            let Some(desk) = self.desk.as_mut().map(|installed| &mut installed.desk) else {
                return Ok(Vec::new());
            };
            let refresh = desk.refresh(&self.liquidity)?;
            // Charted and reported the moment it is known. A count of
            // affected edges computed and dropped would be the §30.1 row
            // claiming an incremental update it does not make.
            self.metrics.edge_refresh(refresh);
            report.edge_refresh = Some(refresh);
            desk.scan(&self.liquidity, now)
        };

        // §30.2's path router, over every cycle this scan found and in the
        // scan's own order (ADR 0068). Computed here, in one place, because
        // the loop below takes `&mut self` to journal and to refuse while the
        // router and the graph are two immutable borrows of `self` that have
        // to be live at the same time.
        //
        // One entry per opportunity, always, so the `zip` below cannot pair a
        // cycle with another cycle's assignment: a shorter vector would
        // silently drop the tail of the scan, and a cycle that reached a venue
        // carrying a path nobody assigned it is the failure this whole
        // classification exists to prevent.
        //
        // The router classifies and never routes. It names no venue, it
        // produces no order, and it cannot make a venue reachable that the
        // cell is not configured for — `install_arbitrage` has already refused
        // a graph touching a venue outside `config.venues`, and `Cell::send`
        // remains the one place a `Placer` is called.
        let routed: Vec<RoutedOutcome> = match self.desk.as_ref() {
            Some(installed) => scanned
                .opportunities
                .iter()
                .map(|opportunity| self.route_one(installed, opportunity, now))
                .collect(),
            // The desk was read at the top of this function and nothing since
            // could have removed it, so this arm is the `Option`'s shape and
            // not a state a pass can be in. Empty is safe here and only here:
            // the `zip` below is over the same `scanned` that a cell with no
            // desk cannot have filled, so there is no cycle to send
            // unclassified. There is deliberately no second arm asking whether
            // a router exists — `InstalledDesk` is why one cannot be missing.
            None => Vec::new(),
        };

        for rejection in &scanned.rejections {
            self.refuse(
                report,
                scan_gate(rejection.stage),
                &format!(
                    "{} cycle over edges {:?}: {}",
                    rejection.candidate.kind.as_str(),
                    rejection.candidate.edges,
                    rejection.detail
                ),
                now,
            );
        }

        // Bounded by the cap, and by what the scan found if that is fewer.
        let mut cycles: Vec<AdmittedCycle> =
            Vec::with_capacity(cap.min(scanned.opportunities.len()));
        // Notional admitted against the desk's envelope so far this pass, so
        // the second cycle is judged against what the first will spend.
        let mut pending = Decimal::ZERO;
        // Zipped rather than indexed. `routed` is built with one entry per
        // opportunity immediately above, so pairing them structurally removes
        // the index arithmetic that could pair a cycle with a neighbour's
        // assignment — and removes the out-of-range arm that would otherwise
        // be a branch no input reaches, reading as a control and guarding
        // nothing.
        for (position, (opportunity, routing)) in
            scanned.opportunities.iter().zip(routed).enumerate()
        {
            let cycle_id = opportunity.cycle_id(now);
            self.journal.record(
                Decision::EdgePriced {
                    opportunity: cycle_id.clone(),
                    net: opportunity.net().to_string(),
                    positive: true,
                },
                now,
            );
            if position >= cap {
                self.refuse(
                    report,
                    "arbitrage_cap",
                    &format!(
                        "cycle {cycle_id} is opportunity {} of this pass and the cap is {cap}; \
                         refused and counted rather than dropped, and the next pass will find \
                         it again if it is still there",
                        position + 1
                    ),
                    now,
                );
                continue;
            }
            if self.autonomy.level() == AutonomyLevel::Observation {
                self.refuse(
                    report,
                    "autonomy",
                    "the cell is at observation and sends nothing",
                    now,
                );
                continue;
            }
            // §30.2's assignment, before a leg is planned. Placed after the
            // cap and autonomy gates so those keep the credit for the cycles
            // they bound — a past-cap cycle refused here instead would tell an
            // operator the router was the constraint when the cap was — and
            // before leg planning, because a cycle the platform cannot say how
            // it would execute is one no planning work should be spent on.
            //
            // The assignment is *used*: recorded on the chain and pushed onto
            // the report. A classification computed and dropped would be worse
            // than none, because a reader of this loop would take it for a
            // control.
            let (assignment, extension) = match routing {
                RoutedOutcome::Assigned(assignment, extension) => (assignment, extension),
                // §36.3. Refused whole and charted under its own gate: the
                // cycle was routable, and the reason it is not being taken is
                // an outage in another region.
                RoutedOutcome::RegionDark(refusal) => {
                    self.refuse(
                        report,
                        GATE_DARK_REGION,
                        &format!(
                            "cycle {cycle_id} has a mirrored leg into a region that has gone \
                             dark and is suspended whole: {}",
                            refusal.message()
                        ),
                        now,
                    );
                    continue;
                }
                RoutedOutcome::RouterRefused(refusal) => {
                    self.refuse(
                        report,
                        GATE_PATH_ROUTER,
                        &format!(
                            "cycle {cycle_id} is assigned no execution path and is refused \
                             whole: {}",
                            refusal.message()
                        ),
                        now,
                    );
                    continue;
                }
                // §33.1's extension refused. The assignment still happened
                // and still reaches the chain and the report below, because
                // "the router had no row for this" and "the router assigned
                // path 3 and this region's band forbade the direction" are
                // different findings and an operator needs to see which.
                // This is the one way a cycle leaves *both* marks, and the
                // `paths` field's own documentation says so.
                RoutedOutcome::ExtensionRefused(assignment, refusal) => {
                    let path = assignment.assigned();
                    self.journal
                        .record(path_assigned(&cycle_id, &assignment), now);
                    report.paths.push(RoutedCycle {
                        cycle_id: cycle_id.clone(),
                        assignment,
                    });
                    self.refuse(
                        report,
                        GATE_PATH_EXTENSION,
                        &format!(
                            "cycle {cycle_id} is assigned path {} ({}) and blueprint §33.1's \
                             extension for it does not hold: {}",
                            path.number(),
                            path.as_str(),
                            refusal.message()
                        ),
                        now,
                    );
                    continue;
                }
            };
            self.journal
                .record(path_assigned(&cycle_id, &assignment), now);
            // §33.1: "Every verdict, including silence, is logged." The
            // verdict that *held* is chained here; the verdict that refused
            // is journaled by `Cell::refuse` in the arm above, so both
            // outcomes reach the chain and neither is inferable from the
            // absence of the other.
            self.journal.record(
                Decision::PathExtensionChecked {
                    cycle_id: cycle_id.clone(),
                    path: assignment.assigned().number(),
                    has_row: extension.has_row(),
                    rationale: extension.rationale(),
                },
                now,
            );
            report.paths.push(RoutedCycle {
                cycle_id: cycle_id.clone(),
                assignment,
            });

            let legs = match opportunity.cycle_legs(&strategy, now, now.saturating_add(validity)) {
                Ok(legs) => legs,
                Err(error) => {
                    self.refuse(report, "arbitrage_legs", error.message(), now);
                    continue;
                }
            };
            if let Some(admitted) =
                self.admit_cycle(opportunity, &cycle_id, legs, pending, now, report)
            {
                pending += admitted.notional;
                cycles.push(admitted);
            }
        }
        Ok(cycles)
    }

    /// Take every leg of one cycle through the feasibility gate and the
    /// desk's capital envelope, or veto the cycle whole.
    ///
    /// Whole, because a cycle is an atomic set: a leg that cannot execute at
    /// its size leaves the rest as a position rather than a smaller cycle,
    /// and a leg the envelope would reduce is the same position by another
    /// route. The leg's own refusal is recorded by the gate that found it,
    /// and then the cycle's, so the series counts which rule bound and the
    /// journal says which cycle it bound.
    fn admit_cycle(
        &mut self,
        opportunity: &Opportunity,
        cycle_id: &str,
        legs: Vec<CycleLeg>,
        pending: Decimal,
        now: Timestamp,
        report: &mut WorkReport,
    ) -> Option<AdmittedCycle> {
        // The legs as intents up front, so the venues are known before any
        // of them is admitted: §32.1's gate is about the set, and a set
        // cannot be judged one member at a time.
        let proposed: Vec<Intent> = legs.into_iter().map(Into::into).collect();
        let venues: Vec<VenueId> = proposed.iter().map(|leg| leg.venue.clone()).collect();
        // §32.1, before a leg exists. A cycle is one position until its last
        // leg fills, so the spread between its venues' fill times is the
        // window the cell is exposed for and the unwind cost is whatever the
        // market did inside it. Refused whole rather than sized down: a
        // cycle re-priced at a smaller size is a different cycle, which is
        // the same argument `scan_cycles` makes about the degradation
        // multiplier.
        //
        // An unmeasured set admits, and the crate documentation says why at
        // length: a venue has no fill times until it fills something, so
        // refusing on absence is the one place the cell's fail-closed rule
        // would close a loop on itself and stop a new cell trading forever.
        // `qip_edge_fill_time_unmeasured_venues` is what makes that silence
        // visible instead.
        if let DispersionVerdict::Exceeds {
            spread,
            bound,
            fastest,
            slowest,
        } = self.fill_times.assess(&venues)
        {
            self.refuse(
                report,
                GATE_FILL_DISPERSION,
                &format!(
                    "cycle {cycle_id} is refused whole: {slowest} fills {} ms after \
                     {fastest} typically, past the {} ms this desk will carry, and the \
                     cycle would be an open position for the difference",
                    spread.as_millis(),
                    bound.as_millis()
                ),
                now,
            );
            return None;
        }
        let mut intents: Vec<Intent> = Vec::with_capacity(proposed.len());
        let mut fixed_cost = Decimal::ZERO;
        let mut on_chain = false;
        let mut notional = Decimal::ZERO;
        for intent in proposed {
            if !self.admit_feasible(&intent, now, report) {
                self.veto_cycle(cycle_id, &intent, "is infeasible at its size", now, report);
                return None;
            }
            let cost = {
                let model = self.config.feasibility.get(intent.venue.as_str());
                on_chain |= model.is_some_and(|model| {
                    matches!(model.class(), VenueClass::DecentralisedExchange)
                });
                feasibility::fixed_cost_fraction(model, self.feasibility_constraints(), &intent)
            };
            match cost {
                Ok(fraction) => fixed_cost += fraction,
                Err(infeasible) => {
                    self.refuse(report, infeasible.gate, &infeasible.reason, now);
                    self.veto_cycle(
                        cycle_id,
                        &intent,
                        "has no notional to charge against",
                        now,
                        report,
                    );
                    return None;
                }
            }
            let Some(admitted) = self.admit_leg(&intent, pending + notional, now, report) else {
                self.veto_cycle(
                    cycle_id,
                    &intent,
                    "is not admitted by the capital envelope",
                    now,
                    report,
                );
                return None;
            };
            notional += admitted;
            intents.push(intent);
        }

        // The edge as a fraction of the start size, the unit the summed leg
        // costs are in. A start quantity the scanner priced at is positive by
        // construction; a division that fails anyway is a refusal, not a
        // pass.
        let edge_fraction = opportunity
            .net()
            .checked_div(opportunity.pricing.start_quantity);
        let Some(edge_fraction) = edge_fraction else {
            self.refuse(
                report,
                "arbitrage_cycle",
                &format!("cycle {cycle_id} is refused whole: its edge cannot be stated per unit of start size"),
                now,
            );
            return None;
        };
        if let Err(infeasible) = feasibility::assess_cycle_cost(fixed_cost, edge_fraction, on_chain)
        {
            self.refuse(report, infeasible.gate, &infeasible.reason, now);
            self.refuse(
                report,
                "arbitrage_cycle",
                &format!("cycle {cycle_id} is refused whole: its fixed costs consume its edge"),
                now,
            );
            return None;
        }
        // §56.2 rule 21: the reservation is settlement-aware. The hold below
        // is money out of settled capital, but a cycle is a chain — a leg
        // spends what the leg before it delivered at the same venue — and
        // whether that is usable when the leg fires is the venue's calendar's
        // business, not the ledger's. Projected here, before the hold, so a
        // cycle the calendar refuses never takes region capital it would
        // have to give straight back; refused whole, because a cycle short
        // one leg is a position. The projection dates every fill at `now`,
        // and `settlement::project` says why that is the conservative
        // reading rather than a shortcut.
        if !self.admit_settlement(cycle_id, &intents, &venues, now, report) {
            return None;
        }
        // The region's bound on the cycle, whole and after every leg was
        // admitted. Holding leg by leg would leave a partial hold behind when
        // a later leg is vetoed, and a cycle short one leg is a position
        // rather than a smaller cycle. A cycle whose legs admitted no notional
        // at all is refused here too: a cycle with nothing to hold against is
        // not a smaller cycle either.
        let pass = self.pass;
        if !self.hold_region_capital(
            region_hold_id_for_cycle(pass, cycle_id),
            notional,
            now,
            report,
        ) {
            self.refuse(
                report,
                "arbitrage_cycle",
                &format!(
                    "cycle {cycle_id} is refused whole: the region allocation cannot hold its \
                     {notional} notional"
                ),
                now,
            );
            return None;
        }
        // §29.2's budget, last and all-or-nothing. A cycle short one leg is a
        // position rather than a smaller cycle, so a rate limit that funded
        // three legs of four would convert a message budget into an open
        // position — which is the failure this gate exists to prevent, not a
        // side effect of it. Spent here, past every gate that could still
        // refuse, so the bucket records messages that ran; the region hold
        // taken above is given back when it cannot.
        if let Admission::Refused { reason } = self.budget.admit_all(&venues, now) {
            self.refuse(report, GATE_QUOTE_BUDGET, &reason, now);
            self.release_cycle_hold(cycle_id);
            return None;
        }
        for venue in &venues {
            self.metrics.message_sent(venue, MessageKind::Placement);
        }
        Some(AdmittedCycle {
            cycle_id: cycle_id.to_string(),
            net: opportunity.net(),
            legs: intents,
            notional,
        })
    }

    /// Project a cycle's legs against their venues' settlement terms, or
    /// veto the cycle whole (§56.2 rule 21, §32.2).
    ///
    /// `false` when a leg would spend proceeds still in settlement at the
    /// instant it fires. The refusal names the leg, the leg it depends on,
    /// the venue's terms, the day the proceeds land and how long after the
    /// leg fires that is — a reader of the journal can re-derive the veto
    /// from the calendar alone, which is what makes it a decision rather
    /// than an assertion. A calendar that cannot be walked is refused under
    /// the same gate: a projection the cell could not make is a figure it
    /// could not evaluate.
    fn admit_settlement(
        &mut self,
        cycle_id: &str,
        intents: &[Intent],
        venues: &[VenueId],
        now: Timestamp,
        report: &mut WorkReport,
    ) -> bool {
        let unfunded = match settlement::project(venues, &self.config.settlement, now) {
            Ok(projection) => projection.first_unfunded(),
            Err(error) => {
                self.refuse(
                    report,
                    GATE_SETTLEMENT,
                    &format!(
                        "cycle {cycle_id} is refused whole: its legs' settlement could not be \
                         projected — {}",
                        error.message()
                    ),
                    now,
                );
                return false;
            }
        };
        let Some(unfunded) = unfunded else {
            return true;
        };
        let (Some(leg), Some(source)) = (intents.get(unfunded.leg), intents.get(unfunded.of_leg))
        else {
            // `venues` was built from `intents` one to one, so an index the
            // projection returned is an index into `intents`; refusing here
            // rather than indexing is what keeps that a fact the code holds
            // instead of one a comment asserts.
            self.refuse(
                report,
                GATE_SETTLEMENT,
                &format!(
                    "cycle {cycle_id} is refused whole: the settlement projection named leg {} \
                     of {}, which the cycle does not have",
                    unfunded.leg + 1,
                    intents.len()
                ),
                now,
            );
            return false;
        };
        let terms = self
            .config
            .settlement
            .get(leg.venue.as_str())
            .map_or("unstated", SettlementTerms::describe);
        let wait = unfunded.usable_at.since(now);
        let minutes = wait.as_millis() / 60_000;
        self.refuse(
            report,
            GATE_SETTLEMENT,
            &format!(
                "cycle {cycle_id}: leg {} ({} {} at {}) spends what leg {} ({} {} at {}) delivers \
                 there, and {} settles {terms}: a fill at {} is usable from {}, {} h {} min after \
                 the leg fires. This cell has no bridge that funds a leg from proceeds in \
                 settlement (§32.2), so the cycle waits for settlement or does not run",
                unfunded.leg + 1,
                leg.signed_size,
                leg.object_id.as_str(),
                leg.venue.as_str(),
                unfunded.of_leg + 1,
                source.signed_size,
                source.object_id.as_str(),
                source.venue.as_str(),
                leg.venue.as_str(),
                now.to_rfc3339(),
                unfunded.usable_at.to_rfc3339(),
                minutes / 60,
                minutes % 60,
            ),
            now,
        );
        self.veto_cycle(
            cycle_id,
            leg,
            "is funded by proceeds still in settlement when it fires",
            now,
            report,
        );
        false
    }

    fn veto_cycle(
        &mut self,
        cycle_id: &str,
        leg: &Intent,
        why: &str,
        now: Timestamp,
        report: &mut WorkReport,
    ) {
        self.refuse(
            report,
            "arbitrage_cycle",
            &format!(
                "cycle {cycle_id} is refused whole: its leg {} of {} at {} {why}, and a cycle \
                 short one leg is a position rather than a smaller cycle",
                leg.signed_size,
                leg.object_id.as_str(),
                leg.venue.as_str()
            ),
            now,
        );
    }

    /// One leg through the gates a directional intent meets in `intent_for`
    /// after pricing: the venue's status, the envelope's life, and capital.
    ///
    /// Returns the leg's notional on admission. `pending` is what this pass
    /// has already admitted against the desk's envelope, added to its
    /// utilisation for the check so a cycle cannot be admitted leg by leg
    /// into more than the envelope holds.
    ///
    /// A `Reduced` grant is a refusal here where `intent_for` takes the
    /// reduction: a directional order at a smaller size is a smaller
    /// position, and a cycle leg at a smaller size is a cycle that no longer
    /// closes.
    fn admit_leg(
        &mut self,
        intent: &Intent,
        pending: Decimal,
        now: Timestamp,
        report: &mut WorkReport,
    ) -> Option<Decimal> {
        let status = self
            .liquidity
            .get(&intent.venue, &intent.object_id)
            .map(VenueState::status);
        match status {
            None => {
                self.refuse(
                    report,
                    "book",
                    "the cell holds no book for the instrument",
                    now,
                );
                return None;
            }
            Some(status) if !status.accepts_orders() => {
                self.refuse(
                    report,
                    "venue_status",
                    &format!("the venue is {}", status.as_str()),
                    now,
                );
                return None;
            }
            Some(_) => {}
        }
        let Some(notional) = intent.signed_size.abs().checked_mul(intent.reference_price) else {
            self.refuse(
                report,
                "capital",
                "the leg's notional cannot be represented",
                now,
            );
            return None;
        };

        let grant = self.desk.as_ref().map(|installed| {
            let desk = &installed.desk;
            if !desk.envelope().is_live(now) {
                return None;
            }
            let mut used = desk.utilisation().clone();
            used.gross_committed += pending;
            Some(desk.envelope().admit(&intent.venue, notional, &used, now))
        });
        match grant {
            None => {
                self.refuse(report, "deployment", "no arbitrage desk is installed", now);
                None
            }
            Some(None) => {
                self.refuse(
                    report,
                    "envelope_expiry",
                    "the desk's capital envelope has expired; the cell stops rather than continues",
                    now,
                );
                None
            }
            Some(Some(CapitalGrant::Full)) => Some(notional),
            Some(Some(CapitalGrant::Reduced(cap))) => {
                self.refuse(
                    report,
                    "arbitrage_capital",
                    &format!(
                        "the envelope would reduce the leg to {cap} notional and a cycle leg cannot \
                         be reduced: a reduced leg is a position, not a smaller cycle"
                    ),
                    now,
                );
                None
            }
            Some(Some(CapitalGrant::Refused(reason))) => {
                self.refuse(report, "capital", &reason, now);
                None
            }
        }
    }

    /// Send every leg of an admitted cycle, in plan order.
    ///
    /// # What this cannot promise, and what it does instead
    ///
    /// The blueprint's cycle is atomic-or-cancelled. This cell's
    /// [`Placer`] can place and cannot cancel, and no fill reaches the cell
    /// until the drop-copy is reconciled, so there is nothing here a
    /// `LegGroup` could act on: the coordinator in
    /// `qip-execution-engine::multileg` decides what to unwind from fills it
    /// is told about, and this seam is told nothing. Building one here would
    /// be a control with no input.
    ///
    /// What the cell can do is refuse to carry on. A leg the venue refuses
    /// after an earlier leg went out leaves the cell holding a position it
    /// did not decide to take, which is the state the multi-leg module calls
    /// the one that "looks, to every downstream report, exactly like a
    /// position somebody chose". So the break is journaled naming the cycle
    /// and how many legs were sent, and the kill switch is tripped as a
    /// reconciliation break trips it: the cell stops until an operator has
    /// looked, and the error propagates so the caller knows the pass did not
    /// complete.
    ///
    /// A cycle that breaks that way leaves its region hold standing, because
    /// the error propagates out of `work` before anything could release it —
    /// and legs have already gone out, so the capital is genuinely spent. The
    /// sweep at the top of the next pass returns it; the halt means there is
    /// no next pass until an operator has looked.
    /// Send an admitted cycle — all at once, or its slow leg first (§32.1).
    ///
    /// The passive-first mechanism decides between those two, and it decides
    /// on measurement or it declines: see [`crate::passive`]. A cycle it
    /// declines goes out exactly as this function sent every cycle before the
    /// mechanism existed, which is what makes the decline safe rather than a
    /// silent change of behaviour.
    fn place_cycle(
        &mut self,
        cycle: &AdmittedCycle,
        now: Timestamp,
        gateway: &mut dyn Placer,
        report: &mut WorkReport,
    ) -> Result<()> {
        // A cycle the cell already has resting is never opened a second time.
        // The scanner re-quotes the graph on every pass, and a cycle id names
        // the path and the instant it opened; a second admission under one id
        // would double the position the first is waiting to complete, at a
        // venue that has not answered the first yet.
        if self.suspended.contains_key(&cycle.cycle_id) {
            let reason = format!(
                "cycle {} already has a leg resting at a venue and the rest of it is waiting on \
                 that fill; nothing more of it is sent until the leg is answered or withdrawn",
                cycle.cycle_id
            );
            self.refuse(report, GATE_CYCLE_RESTING, &reason, now);
            // Nothing of this admission reaches a venue, so the hold it took
            // goes back rather than waiting for the sweep.
            self.release_cycle_hold(&cycle.cycle_id);
            return Ok(());
        }
        // Room for every leg before the first is sent: a cycle refused for
        // capacity between legs would be a broken cycle, and a broken cycle
        // is a position nobody chose. Checked against the whole cycle even
        // when only one leg is about to go out, because the cell has to be
        // able to finish what it starts.
        if !self.has_open_capacity(cycle.legs.len()) {
            self.refuse_for_capacity(report, now);
            // Nothing of this cycle reaches a venue, so the hold it took at
            // admission is given back rather than left to the sweep.
            self.release_cycle_hold(&cycle.cycle_id);
            return Ok(());
        }
        // §32.1's size decomposition, carried across the legs of this one
        // cycle and dropped with it. It starts whole, so a cycle whose venues
        // fill everything asked of them is sent exactly as it was admitted.
        // A cycle that rests a leg carries this in `suspended` instead, for
        // the same reason: the fraction belongs to the cycle, not the pass.
        let mut decomposition = Decomposition::new(self.config.decomposition);
        let venues: Vec<VenueId> = cycle.legs.iter().map(|leg| leg.venue.clone()).collect();
        let declined = match passive::choose(
            &venues,
            &self.venue_medians(),
            self.fill_times.policy().bound(),
        ) {
            // A leg may only rest where the cell could withdraw it. The same
            // guard `resolve_pricing` puts on a resting net, for the same
            // reason: an order nothing can take back sits at a price the
            // market has since left — and here it would strand the rest of
            // the cycle behind it for as long as the venue kept it.
            PassiveChoice::Rest {
                position,
                venue,
                median,
            } if gateway.can_cancel() => {
                return self.rest_cycle_leg(
                    cycle,
                    position,
                    &venue,
                    median,
                    decomposition,
                    now,
                    gateway,
                    report,
                );
            }
            PassiveChoice::Rest { .. } => WholeReason::NoWithdrawal,
            PassiveChoice::Whole(reason) => reason,
        };
        self.metrics.passive_cycle(PassiveOutcome::Whole(declined));
        // ADR 0084: one schedule for the whole cycle, computed once before
        // the first leg goes out, so every leg's release instant is a
        // function of the same window and the legs are expected to arrive
        // together. Computed after the passive choice above, because a
        // cycle that rests a leg sends that leg alone and the rest later.
        let schedule = self.fill_times.release_schedule(&venues);
        let mut orders: Vec<String> = Vec::with_capacity(cycle.legs.len());
        for position in 0..cycle.legs.len() {
            let (order_id, quantity) = self.send_cycle_leg(
                cycle,
                position,
                None,
                &schedule,
                &mut decomposition,
                now,
                gateway,
                report,
            )?;
            orders.push(order_id.clone());
            self.observe_cycle_leg(
                cycle,
                position,
                &order_id,
                quantity,
                &mut decomposition,
                now,
                gateway,
                report,
            )?;
        }
        // Every leg is out, so the cycle's hold on the region is spend.
        self.commit_cycle_hold(&cycle.cycle_id);
        self.journal.record(
            Decision::CycleCommitted {
                cycle_id: cycle.cycle_id.clone(),
                orders,
                net: cycle.net.to_string(),
            },
            now,
        );
        Ok(())
    }

    /// What each venue's fill time has been measured at, for the venues with
    /// enough fills to have a median at all.
    ///
    /// A venue missing from the map is unmeasured, which [`crate::passive`]
    /// treats as "no opinion" and never as fast. Built per cycle rather than
    /// kept, because it is derived from `fill_times` and a second copy of a
    /// fact is a second thing that can be stale.
    fn venue_medians(&self) -> BTreeMap<String, Duration> {
        self.fill_times
            .summary()
            .into_iter()
            .filter_map(|state| state.median.map(|median| (state.venue, median)))
            .collect()
    }

    /// Send one leg of a cycle alone and leave it resting (§32.1).
    ///
    /// Nothing else of the cycle goes out. That is the whole mechanism: the
    /// fast legs are crossed on the next pass, against what this venue
    /// actually did, so the slow venue is out of the exposure window instead
    /// of setting its length.
    #[allow(clippy::too_many_arguments)]
    fn rest_cycle_leg(
        &mut self,
        cycle: &AdmittedCycle,
        position: usize,
        venue: &str,
        median: Duration,
        decomposition: Decomposition,
        now: Timestamp,
        gateway: &mut dyn Placer,
        report: &mut WorkReport,
    ) -> Result<()> {
        let Some(leg) = cycle.legs.get(position) else {
            // `passive::choose` returns a position into the same slice, so
            // this cannot happen; stated as a refusal rather than an index,
            // because an out-of-range index on the order path is a panic and
            // this crate does not take that trade.
            let error = Error::invalid(format!(
                "cycle {} has no leg {position} to rest",
                cycle.cycle_id
            ));
            self.break_cycle(&cycle.cycle_id, 0, cycle.legs.len(), &error, now, report);
            return Err(error);
        };
        // The leg rests until the cycle's own legs expire, and not one instant
        // longer. `valid_until` is the desk's `leg_validity` measured from
        // where the cycle was priced, and the arbitrage crate calls it "the
        // deadline after which an unfilled leg is a stranded position rather
        // than a pending one". No second duration is introduced here on
        // purpose: a rest window of its own would be a second opinion about
        // when this cycle stops being one, and the two would disagree.
        let expires_at = leg.valid_until;
        let mut carried = decomposition;
        // A resting leg goes out alone, so its schedule is a single venue's:
        // offset zero, trivially equalised. The legs crossed against it later
        // get their own schedule on the pass that crosses them.
        let schedule = self
            .fill_times
            .release_schedule(std::slice::from_ref(&VenueId::new(venue)));
        let (order_id, quantity) = self.send_cycle_leg(
            cycle,
            position,
            Some(expires_at),
            &schedule,
            &mut carried,
            now,
            gateway,
            report,
        )?;
        // The cycle's region hold becomes spend here rather than when the last
        // leg goes out. A hold is pass-scoped — `work` sweeps any that outlive
        // their pass and journals it as a defect — and this cycle now spans
        // passes, so a hold left standing would be returned underneath an
        // order that is resting against it. Committed in full and returned in
        // full if the leg is withdrawn having filled nothing.
        let committed = self.commit_cycle_hold(&cycle.cycle_id);
        self.journal.record(
            Decision::CycleRested {
                cycle_id: cycle.cycle_id.clone(),
                leg: position,
                venue: venue.to_string(),
                order_id: order_id.clone(),
                // Statistics, not money: a measured duration crosses to an
                // integer of milliseconds here, where the journal needs a
                // number a reader can compare. Nothing prices off it.
                median_millis: median.as_millis(),
            },
            now,
        );
        self.metrics.passive_cycle(PassiveOutcome::Rested);
        self.suspended.insert(
            cycle.cycle_id.clone(),
            SuspendedCycle {
                cycle: cycle.clone(),
                position,
                order_id,
                sent: quantity,
                decomposition: carried,
                committed,
            },
        );
        Ok(())
    }

    /// Whether a rested cycle is still waiting on this order to decide what
    /// to do with its remaining legs.
    fn awaits_rested_leg(&self, order_id: &str) -> bool {
        self.suspended
            .values()
            .any(|rested| rested.order_id == order_id)
    }

    /// Finish, or give up on, every cycle whose slow leg was left resting
    /// (§32.1).
    fn resume_rested_cycles(
        &mut self,
        now: Timestamp,
        gateway: &mut dyn Placer,
        report: &mut WorkReport,
    ) -> Result<()> {
        // Keys first: resuming one takes `&mut self`, and the order is the
        // map's, so a replay resumes them in the same order every time.
        let waiting: Vec<String> = self.suspended.keys().cloned().collect();
        for cycle_id in waiting {
            self.resume_rested_cycle(&cycle_id, now, gateway, report)?;
        }
        Ok(())
    }

    /// One rested cycle: cross the rest of it, abandon it, or keep waiting.
    fn resume_rested_cycle(
        &mut self,
        cycle_id: &str,
        now: Timestamp,
        gateway: &mut dyn Placer,
        report: &mut WorkReport,
    ) -> Result<()> {
        let Some(rested) = self.suspended.get(cycle_id) else {
            return Ok(());
        };
        let order_id = rested.order_id.clone();
        let position = rested.position;
        let sent = rested.sent;
        let committed = rested.committed;
        let cycle = rested.cycle.clone();
        let mut decomposition = rested.decomposition;
        let total = cycle.legs.len();

        let Some(working) = self.working.get(&order_id) else {
            // `settle` passes over exactly this order while a cycle owes legs
            // against it, so its absence is the cell disagreeing with itself
            // about an order at a venue. There is no fraction to size the
            // rest of the cycle from, and inventing one would size the fast
            // legs against a number nobody measured.
            self.suspended.remove(cycle_id);
            let error = Error::invalid(format!(
                "cycle {cycle_id} left leg {position} resting as order {order_id} and the cell \
                 holds no open order under that id, so what that venue completed cannot be read"
            ));
            self.break_cycle(cycle_id, 1, total, &error, now, report);
            return Err(error);
        };
        let filled = working.order.filled;
        let closed = working.order.closed.clone();

        if closed.is_none() && filled < sent {
            // Still working, and not yet whole. The fraction the rest of the
            // cycle would be sized to can still move, and a leg sized against
            // a fraction that then grows leaves the cycle short on the other
            // side — which is the outright position the whole mechanism is
            // about. The wait is bounded by the leg's own time to live, and
            // `withdraw_expired` enforces it at the top of every pass.
            return Ok(());
        }
        self.suspended.remove(cycle_id);

        if filled.is_zero() {
            // Withdrawn having filled nothing. No leg of this cycle ever
            // became a position, which is the outcome the mechanism exists to
            // produce: under the all-at-once discipline the fast legs would
            // already be crossed against a slow leg that never arrived.
            self.return_cycle_capital(cycle_id, committed, now);
            self.journal.record(
                Decision::CycleAbandoned {
                    cycle_id: cycle_id.to_string(),
                    leg: position,
                    venue: cycle.legs[position].venue.as_str().to_string(),
                    reason: closed.unwrap_or_else(|| "unfilled".to_string()),
                },
                now,
            );
            self.metrics.passive_cycle(PassiveOutcome::Abandoned);
            return Ok(());
        }

        // From here the cell holds a position: the slow leg filled something.
        // Every path below either completes the cycle against it or halts.
        let completion = match decomposition.observe(sent, filled) {
            Ok(completion) => completion,
            Err(error) => {
                self.break_cycle(cycle_id, 1, total, &error, now, report);
                return Err(error);
            }
        };
        self.metrics.cycle_leg(completion);
        let remaining = total.saturating_sub(1);
        if !self.has_open_capacity(remaining) {
            let error = Error::denied(format!(
                "cycle {cycle_id} has {remaining} leg(s) left to send and the cell holds {} open \
                 order(s), the most it will track",
                self.working.len()
            ));
            self.break_cycle(cycle_id, 1, total, &error, now, report);
            return Err(error);
        }
        let mut orders = vec![order_id];
        // ADR 0084: the legs crossed against the rested one go out together
        // on this pass, so they are scheduled as a set of their own — the
        // rested leg is already at its venue and is not in it.
        let remaining: Vec<VenueId> = cycle
            .legs
            .iter()
            .enumerate()
            .filter(|(other, _)| *other != position)
            .map(|(_, leg)| leg.venue.clone())
            .collect();
        let schedule = self.fill_times.release_schedule(&remaining);
        for other in 0..total {
            if other == position {
                continue;
            }
            // The remaining legs are priced where the scanner quoted them,
            // and past `valid_until` that price is one the market has left.
            // Crossing there would be the cell completing an arbitrage
            // against numbers that no longer exist, so it stops and says so
            // instead — a stranded position an operator is told about beats a
            // second position the cell took to tidy up the first.
            if cycle.legs[other].valid_until <= now {
                let error = Error::denied(format!(
                    "leg {other} of cycle {cycle_id} was priced to stand until {} and it is now \
                     {}; the leg that rested filled {filled} and the rest of the cycle is not \
                     sent at a price the market has left",
                    cycle.legs[other].valid_until.as_nanos(),
                    now.as_nanos()
                ));
                self.break_cycle(cycle_id, orders.len(), total, &error, now, report);
                return Err(error);
            }
            let (id, quantity) = self.send_cycle_leg(
                &cycle,
                other,
                None,
                &schedule,
                &mut decomposition,
                now,
                gateway,
                report,
            )?;
            orders.push(id.clone());
            self.observe_cycle_leg(
                &cycle,
                other,
                &id,
                quantity,
                &mut decomposition,
                now,
                gateway,
                report,
            )?;
        }
        self.journal.record(
            Decision::CycleCommitted {
                cycle_id: cycle_id.to_string(),
                orders,
                net: cycle.net.to_string(),
            },
            now,
        );
        self.metrics.passive_cycle(PassiveOutcome::Completed);
        Ok(())
    }

    /// Give back the region capital a rested cycle committed, because its one
    /// leg was withdrawn having filled nothing.
    ///
    /// The amount is the one `commit_cycle_hold` returned and nothing
    /// recomputed, for the reason `return_region_capital_for_unfilled` keeps
    /// a net's commit on the order: two independent claims about the same
    /// spend will disagree, and the louder one will be wrong.
    fn return_cycle_capital(&mut self, cycle_id: &str, committed: Decimal, now: Timestamp) {
        if !committed.is_positive() {
            return;
        }
        let outcome = match self.region_allocation.as_ref() {
            None => return,
            Some(allocation) => allocation.return_committed(committed),
        };
        if let Err(error) = outcome {
            self.journal.record(
                Decision::Refused {
                    gate: "region_reservation_return".to_string(),
                    reason: format!(
                        "cycle {cycle_id} was abandoned with nothing filled and its {committed} \
                         could not be returned to the region allocation: {}",
                        error.message()
                    ),
                },
                now,
            );
        }
    }

    /// Size one leg of a cycle, send it, and record it as working.
    ///
    /// `expires_at` is `None` for a leg that takes the touch on acceptance —
    /// every leg of a cycle going out whole — and `Some` only for §32.1's
    /// resting leg. Returns the order id and the quantity that went to the
    /// venue, which is what the caller must observe the fill against: the
    /// planned size would attribute a decomposed leg's fills to a cycle that
    /// was never sent.
    ///
    /// Deliberately does **not** fold the venue's answer into the
    /// decomposition. That is [`Self::observe_cycle_leg`], because a resting
    /// leg is observed on a later pass and observing it here as well would
    /// count one leg's fill twice against the fraction.
    ///
    /// `schedule` is the release schedule of the set of legs this one goes
    /// out with; the leg is released no earlier than `now` plus its venue's
    /// offset on it (ADR 0084).
    #[allow(clippy::too_many_arguments)]
    fn send_cycle_leg(
        &mut self,
        cycle: &AdmittedCycle,
        position: usize,
        expires_at: Option<Timestamp>,
        schedule: &ReleaseSchedule,
        decomposition: &mut Decomposition,
        now: Timestamp,
        gateway: &mut dyn Placer,
        report: &mut WorkReport,
    ) -> Result<(String, Decimal)> {
        let Some(leg) = cycle.legs.get(position) else {
            let error = Error::invalid(format!(
                "cycle {} has no leg {position} to send",
                cycle.cycle_id
            ));
            self.break_cycle(
                &cycle.cycle_id,
                position,
                cycle.legs.len(),
                &error,
                now,
                report,
            );
            return Err(error);
        };
        let leg = leg.clone();
        // A buy takes the ask; the sign was fixed where the leg was made.
        let side = if leg.signed_size.is_positive() {
            BookSide::Ask
        } else {
            BookSide::Bid
        };
        // What the legs already out have completed decides the size of
        // this one. A leg sent at its planned size behind a leg that
        // filled six tenths is four tenths of an outright position at a
        // price chosen for an arbitrage that did not exist at that size.
        let planned = leg.signed_size.abs();
        let grid = self
            .config
            .feasibility
            .get(leg.venue.as_str())
            .map(|model| model.granularity_for(&leg.object_id));
        let quantity = match decomposition.size_for(planned, grid) {
            LegSize::Planned(size) => size,
            LegSize::Decomposed { planned, size } => {
                self.journal.record(
                    Decision::CycleDecomposed {
                        cycle_id: cycle.cycle_id.clone(),
                        leg: position,
                        planned: planned.to_string(),
                        size: size.to_string(),
                        fraction: decomposition.fraction().to_string(),
                    },
                    now,
                );
                size
            }
            LegSize::Unviable { planned, reason } => {
                // The legs already sent are a position nobody chose, and
                // there is no size at which finishing the cycle is worth
                // it. That is exactly the state `break_cycle` exists for.
                let error = Error::denied(format!(
                    "leg {position} of cycle {} cannot be completed at a viable size against \
                     its planned {planned}: {reason}",
                    cycle.cycle_id
                ));
                // Not counted on `qip_edge_cycle_legs_total`: that series
                // counts legs the cell *sent*, by what each completed,
                // and this one never went to a venue. The leg in front of
                // it is already counted `unviable` there, and the cell's
                // declining to send this one is counted where every other
                // refusal is, under the `arbitrage_cycle_broken` gate
                // `break_cycle` refuses through. Counting it in both
                // would make a single stopped cycle read as two.
                self.break_cycle(
                    &cycle.cycle_id,
                    position,
                    cycle.legs.len(),
                    &error,
                    now,
                    report,
                );
                return Err(error);
            }
        };
        let price = leg.reference_price;
        let release_at = now.saturating_add(schedule.offset(&leg.venue));
        let sent = self.send(
            &leg.object_id,
            &leg.venue,
            side,
            quantity,
            price,
            now,
            release_at,
            gateway,
        );
        let (order_id, simulated) = match sent {
            Ok(sent) => sent,
            Err(error) => {
                self.break_cycle(
                    &cycle.cycle_id,
                    position,
                    cycle.legs.len(),
                    &error,
                    now,
                    report,
                );
                return Err(error);
            }
        };
        // A leg is attributed like a net of one: `net` over a single
        // no-net intent yields one contributor, which is the leg's
        // strategy at the size that went to the venue. The size, not the
        // planned one: `NetIntent::split_fill` divides a fill across
        // contributors, and a contributor claiming a size the venue was
        // never asked for would attribute a decomposed leg's fills to a
        // cycle that was never sent.
        //
        // Cloned and resized rather than rebuilt: `Intent::netting` is
        // private precisely so that nothing can mint a leg without the
        // `NoNet` policy §27.2 requires, and a clone carries it through.
        let mut sent_leg = leg.clone();
        sent_leg.signed_size = if leg.signed_size.is_positive() {
            quantity
        } else {
            -quantity
        };
        let leg_net = net(vec![sent_leg.clone()]).into_iter().next();
        let Some(leg_net) = leg_net else {
            let error = Error::invalid(format!(
                "leg {position} of cycle {} nets to nothing and cannot be attributed",
                cycle.cycle_id
            ));
            self.break_cycle(
                &cycle.cycle_id,
                position.saturating_add(1),
                cycle.legs.len(),
                &error,
                now,
                report,
            );
            return Err(error);
        };
        self.record_sent(
            Working {
                order: OpenOrder {
                    order_id: order_id.clone(),
                    venue: leg.venue.clone(),
                    object_id: leg.object_id.clone(),
                    side,
                    quantity,
                    price,
                    filled: Decimal::ZERO,
                    simulated,
                    sent_at: now,
                    release_at,
                    // A leg of a cycle going out whole is priced at the touch
                    // the scanner quoted it from and takes it on acceptance;
                    // nothing rests. §32.1's passive leg is the exception and
                    // carries the cycle's own expiry, so the one thing that
                    // can end its wait is the deadline the cycle was priced
                    // against.
                    expires_at,
                    closed: None,
                },
                net: leg_net,
                // A cycle holds once for all its legs. The commit is the
                // caller's: a cycle going out whole commits when the last leg
                // is away, and one resting a leg commits when that leg is,
                // because a pass-scoped hold cannot outlive its pass. Either
                // way nothing here can expire and return capital, so this
                // stays zero and `return_region_capital_for_unfilled` passes
                // a cycle leg over.
                region_committed: Decimal::ZERO,
            },
            schedule.equalised(),
            now,
        );
        if let Some(desk) = self.desk.as_mut().map(|installed| &mut installed.desk) {
            let utilisation = desk.utilisation_mut();
            utilisation.gross_committed += quantity * price;
            utilisation.orders_sent += 1;
        }
        report.orders.push(PlacedOrder {
            order_id: order_id.clone(),
            strategy: leg.strategy.clone(),
            contributors: vec![Contributor {
                strategy: leg.strategy.clone(),
                signed_size: sent_leg.signed_size,
                inputs: leg.inputs.clone(),
            }],
            object_id: leg.object_id.clone(),
            venue: leg.venue.clone(),
            side,
            quantity,
            price,
            simulated,
        });
        Ok((order_id, quantity))
    }

    /// Fold what one leg completed into the cycle's decomposition.
    #[allow(clippy::too_many_arguments)]
    fn observe_cycle_leg(
        &mut self,
        cycle: &AdmittedCycle,
        position: usize,
        order_id: &str,
        quantity: Decimal,
        decomposition: &mut Decomposition,
        now: Timestamp,
        gateway: &mut dyn Placer,
        report: &mut WorkReport,
    ) -> Result<()> {
        let confirmed = self.confirm_execution_reports(gateway, now);
        report.fills.extend(confirmed);
        // What this leg completed, from the cell's own record of it
        // rather than from the reports just drained: a report for an
        // order sent on an earlier pass is in that list too, and a leg
        // whose venue answered while an earlier leg was still being sent
        // is booked on the record and not in this drain.
        let filled = match self.working.get(order_id) {
            Some(working) => working.order.filled,
            None => {
                // `record_sent` put it there when the leg was sent. Its
                // absence is the cell disagreeing with itself about an
                // order that is already at a venue, which is a break and
                // never a fraction to size the next leg from.
                let error = Error::invalid(format!(
                    "leg {position} of cycle {} was sent as order {order_id} and the cell \
                     holds no open order under that id, so what it completed cannot be read",
                    cycle.cycle_id
                ));
                self.break_cycle(
                    &cycle.cycle_id,
                    position.saturating_add(1),
                    cycle.legs.len(),
                    &error,
                    now,
                    report,
                );
                return Err(error);
            }
        };
        let completion = match decomposition.observe(quantity, filled) {
            Ok(completion) => completion,
            Err(error) => {
                self.break_cycle(
                    &cycle.cycle_id,
                    position.saturating_add(1),
                    cycle.legs.len(),
                    &error,
                    now,
                    report,
                );
                return Err(error);
            }
        };
        self.metrics.cycle_leg(completion);
        Ok(())
    }

    /// A cycle stopped between legs. Record it and stop the cell.
    fn break_cycle(
        &mut self,
        cycle_id: &str,
        sent: usize,
        total: usize,
        error: &Error,
        now: Timestamp,
        report: &mut WorkReport,
    ) {
        self.refuse(
            report,
            "arbitrage_cycle_broken",
            &format!(
                "cycle {cycle_id} stopped after {sent} of {total} legs: {}; the legs already sent \
                 are a position nobody chose, and the cell halts until an operator has looked",
                error.message()
            ),
            now,
        );
        if sent > 0 && !self.is_halted() {
            self.autonomy.kill_switch_mut().trip_global(
                now,
                "arbitrage",
                "a cycle stopped between legs and left a position the cell did not decide to take",
            );
            self.journal.record(
                Decision::HaltChanged {
                    halted: true,
                    reason: format!("cycle {cycle_id} broke after {sent} of {total} legs"),
                },
                now,
            );
            self.record_halt();
        }
    }

    /// Work out the offsetting part of a net that should be crossed between its
    /// own contributors, or refuse it and say why (§27.1).
    ///
    /// Returns the cross rather than recording it: the caller seals it into the
    /// journal once the pass has an outcome, because a venue call can fail
    /// afterwards and take the whole report with it.
    ///
    /// The matched size is the smaller of the buying and selling sides, which
    /// is exactly the quantity that never needed a venue. It is computed as a
    /// minimum rather than as `(gross - |net|) / 2` so that no division enters
    /// a money path: the two are equal, and only one of them can introduce a
    /// remainder.
    ///
    /// **The cap refuses; it does not clamp.** Above forty percent of gross
    /// intent the blueprint's objection is that a persistent internal market
    /// forms whose marks drift from reality — and crossing the permitted forty
    /// percent and abandoning the rest would build exactly that market, just
    /// more slowly. Refusing the whole cross leaves the offsetting intents
    /// netted as before: nothing extra reaches the venue, and nothing is
    /// booked between strategies.
    ///
    /// # What the per-net cap cannot do, stated because the arithmetic is not obvious
    ///
    /// With no [`CrossingInterval`] configured the cap is measured against
    /// this net alone. The matched size is `min(buy, sell)` and the
    /// denominator is `buy + sell`, so the ratio can never exceed one half,
    /// and it reaches one half exactly when the two sides cancel completely.
    /// A forty percent cap therefore fires only in the narrow band above two
    /// fifths — and **a net that cancels to zero is always refused**, which
    /// is §27.1's own flagship case: "strategies that disagree cost nothing
    /// to run together because their disagreement never reaches a venue".
    /// Under the per-net measure that disagreement is never booked as a
    /// cross at all.
    ///
    /// That is the default, and it is deliberate. §27.1 caps crossing at
    /// forty percent of gross intent "per instrument **per interval**", and
    /// never says how long an interval is. The window length decides when a
    /// safety control fires, so this crate does not choose one: unset, the
    /// cap reads per net, which is safe and less than §27.1 asks for, and
    /// `a_fully_offsetting_net_is_out_of_cap_by_arithmetic_and_is_never_crossed`
    /// holds that default in place. Choosing the interval is the owner's
    /// decision (completion plan D3), taken by setting
    /// [`CellConfig::crossing_interval`].
    ///
    /// # With an interval
    ///
    /// The cap is then §27.1's: crossed size over gross intent, per
    /// instrument, accumulated over the trailing window — what the window
    /// has already crossed plus this cross, against what it has already
    /// seen plus this net. A full cancellation can sit inside a larger
    /// instrument-level gross and be admitted, and a run of them is refused
    /// once they are two fifths of the window, which is the persistent
    /// internal market the cap exists to prevent. The window's samples are
    /// written by [`Self::settle_cross`] only once the pass has an outcome,
    /// for the same reason the cross itself is.
    ///
    /// Crossing changes nothing about what is sent. It is a booking decision
    /// on top of netting, which has already decided what a venue sees.
    fn cross_internally(
        &mut self,
        net_intent: &NetIntent,
        now: Timestamp,
        report: &mut WorkReport,
    ) -> Option<InternalCross> {
        let mut bought = Vec::new();
        let mut sold = Vec::new();
        let mut buy_size = Decimal::ZERO;
        let mut sell_size = Decimal::ZERO;
        for contributor in &net_intent.contributors {
            if contributor.signed_size.is_positive() {
                buy_size += contributor.signed_size;
                bought.push(contributor.strategy.clone());
            } else if contributor.signed_size.is_negative() {
                sell_size -= contributor.signed_size;
                sold.push(contributor.strategy.clone());
            }
        }
        // Nothing offset, so there is nothing to cross. The common case, and
        // not a refusal: a net every contributor agreed on has no internal
        // trade in it to record. It is journaled anyway, because a chain that
        // explains a cross but is silent about its absence leaves a reader
        // unable to tell "they agreed" from "the crossing step never ran".
        if buy_size.is_zero() || sell_size.is_zero() {
            self.journal.record(
                Decision::CrossedInternally {
                    object: net_intent.object_id.as_str().to_string(),
                    venue: net_intent.venue.as_str().to_string(),
                    quantity: Decimal::ZERO.to_string(),
                    price: String::new(),
                    bought: bought.iter().map(|id| id.as_str().to_string()).collect(),
                    sold: sold.iter().map(|id| id.as_str().to_string()).collect(),
                },
                now,
            );
            return None;
        }
        let crossed = if buy_size < sell_size {
            buy_size
        } else {
            sell_size
        };

        // What the window has already seen for this instrument. Nothing,
        // exactly, when no interval is configured — so the per-net reading
        // below is byte-for-byte the arithmetic this cell always had.
        let window = self.crossing_window(net_intent, now);
        if window.full {
            self.refuse(
                report,
                "internal_cross_window",
                &format!(
                    "the crossing window for {} on {} holds {MAX_CROSSING_WINDOW_SAMPLES} nets \
                     and its oldest gross has been dropped; the cap cannot be measured against \
                     a partial window and refuses until it drains",
                    net_intent.object_id.as_str(),
                    net_intent.venue.as_str()
                ),
                now,
            );
            return None;
        }

        // Forty percent of gross intent, compared without dividing: the cap is
        // two fifths, so `crossed * 5 > gross * 2` asks the same question in
        // exact arithmetic. The totals are the window's plus this net's. A
        // sum or multiply that cannot be represented refuses, because a cap
        // that silently answered "under" on overflow would be a control that
        // cannot fire.
        let over_cap = match (
            window
                .crossed
                .checked_add(crossed)
                .and_then(|total| total.checked_mul(Decimal::from_int(5))),
            window
                .gross
                .checked_add(net_intent.gross_size)
                .and_then(|total| total.checked_mul(Decimal::from_int(2))),
        ) {
            (Some(five_crossed), Some(two_gross)) => five_crossed > two_gross,
            _ => true,
        };
        if over_cap {
            self.refuse(
                report,
                "internal_cross_cap",
                &format!(
                    "crossing {crossed} of {} gross intent on {} exceeds the forty percent cap \
                     ({} already crossed of {} gross in the window); the cross is refused whole \
                     rather than trimmed to the cap, because a cross repeated at the cap every \
                     interval is the persistent internal market the cap exists to prevent",
                    net_intent.gross_size,
                    net_intent.object_id.as_str(),
                    window.crossed,
                    window.gross
                ),
                now,
            );
            return None;
        }

        // The prevailing mid at the netting instant, read from the book now
        // rather than taken from `reference_price` — that is the largest
        // contributor's own stamped price, and §27.1 requires a price neither
        // side chose. A book that serves no mid refuses the cross instead of
        // falling back to one, because the fallback is precisely the price the
        // rule forbids.
        let mid = self
            .liquidity
            .get(&net_intent.venue, &net_intent.object_id)
            .and_then(|state| state.mid());
        let Some(price) = mid else {
            self.refuse(
                report,
                "internal_cross_price",
                "the book serves no mid at the netting instant, and a cross has no price either \
                 side may choose",
                now,
            );
            return None;
        };

        // The record must settle itself. `book_cross` moves each side's lot
        // and cash from the record alone — one name a side, the size, the
        // mid — so a record that names two buyers or two sellers carries no
        // per-strategy size to move, and splitting it evenly would be a
        // guess dressed as a ledger. The centre refuses exactly this record
        // for exactly this reason (`CentralPlane::ingest`); refusing it here
        // means the two never disagree about a cross that one of them booked
        // and the other could not settle. Refused, not narrowed to a pair:
        // choosing which two of three strategies traded is another guess.
        if bought.len() != 1 || sold.len() != 1 {
            self.refuse(
                report,
                "internal_cross_attribution",
                &format!(
                    "crossing {crossed} on {} names {} buyer(s) and {} seller(s); the record \
                     carries no per-strategy size, so the cross cannot be settled to each book \
                     from the record alone and is refused rather than split by a guess",
                    net_intent.object_id.as_str(),
                    bought.len(),
                    sold.len()
                ),
                now,
            );
            return None;
        }
        // The notional the settlement will move. Judged now, before the
        // record exists, so that a cross the chain says happened can never be
        // one whose cash leg could not be represented.
        if crossed.checked_mul(price).is_none() {
            self.refuse(
                report,
                "internal_cross_price",
                &format!(
                    "crossing {crossed} at {price} on {} has a notional the ledger cannot \
                     represent, so the cross is refused rather than booked without its cash leg",
                    net_intent.object_id.as_str()
                ),
                now,
            );
            return None;
        }

        Some(InternalCross {
            object_id: net_intent.object_id.clone(),
            venue: net_intent.venue.clone(),
            quantity: crossed,
            price,
            bought,
            sold,
        })
    }

    /// Seal a cross into the chain, report it, and let the crossing window
    /// see the net it came from.
    ///
    /// One call for the two records because they must agree: a window that
    /// counted a cross the chain never sealed would refuse later crosses
    /// against a trade that did not happen, and one that missed a sealed
    /// cross would admit the persistent internal market the cap exists to
    /// prevent. Called only once the pass has an outcome, after the venue
    /// call that can fail — see `place_net`.
    fn settle_cross(
        &mut self,
        net_intent: &NetIntent,
        crossed: Option<InternalCross>,
        now: Timestamp,
        report: &mut WorkReport,
    ) {
        let quantity = crossed
            .as_ref()
            .map_or(Decimal::ZERO, |cross| cross.quantity);
        if let Some(cross) = &crossed {
            self.book_cross(cross, now);
        }
        self.record_cross(crossed, now, report);
        self.observe_crossing(net_intent, quantity, now);
    }

    /// Move both sides' lots and cash by what the cross record says, and
    /// nothing else.
    ///
    /// The record is the one source: the buyer named in it gains `quantity`
    /// and pays `quantity × price`, the seller named in it loses `quantity`
    /// and receives the same, at the price the chain is about to seal. No
    /// contributor size, reference price or book read enters here — a
    /// settlement worked out a second time from the intents could disagree
    /// with the journal, and the journal is what a reader replays.
    ///
    /// Every arithmetic step is checked. A lot or cash balance that could
    /// not be represented, or a record naming other than one strategy a side
    /// — impossible from `cross_internally`, which refuses both before the
    /// record exists — is a reconciliation break: the chain would hold a
    /// cross the books do not, which is the disagreement `break_on` exists
    /// to stop the cell on. `positions`, the venue-facing aggregate, is left
    /// alone: the two lots sum to zero and the venue saw nothing.
    fn book_cross(&mut self, cross: &InternalCross, now: Timestamp) {
        let ([buyer], [seller]) = (cross.bought.as_slice(), cross.sold.as_slice()) else {
            self.break_on(
                format!(
                    "a cross of {} {} at {} was booked naming {} buyer(s) and {} seller(s), and \
                     the ledger cannot settle it to one book a side",
                    cross.quantity,
                    cross.object_id.as_str(),
                    cross.price,
                    cross.bought.len(),
                    cross.sold.len()
                ),
                now,
            );
            return;
        };
        let Some(notional) = cross.quantity.checked_mul(cross.price) else {
            self.break_on(
                format!(
                    "a cross of {} {} at {} was booked and its notional cannot be represented, \
                     so the cash legs were not moved",
                    cross.quantity,
                    cross.object_id.as_str(),
                    cross.price
                ),
                now,
            );
            return;
        };
        // Buyer first, then seller; equal and opposite on both legs. Every
        // next balance is worked out before any is written, so a leg that
        // cannot be represented leaves neither book moved. This once wrote
        // leg by leg: a seller-side overflow was found after the buyer's lot
        // and cash were already booked, and "equal and opposite" was true of
        // the record and false of the books it was meant to describe.
        let legs = [
            (buyer.clone(), cross.quantity, -notional),
            (seller.clone(), -cross.quantity, notional),
        ];
        let mut settled = Vec::with_capacity(legs.len());
        for (strategy, lot, cash) in legs {
            let position_key =
                Self::strategy_position_key(&strategy, &cross.venue, &cross.object_id);
            let held = self
                .strategy_positions
                .get(&position_key)
                .copied()
                .unwrap_or(Decimal::ZERO);
            let balance = self
                .strategy_cash
                .get(strategy.as_str())
                .copied()
                .unwrap_or(Decimal::ZERO);
            let (Some(next_held), Some(next_balance)) =
                (held.checked_add(lot), balance.checked_add(cash))
            else {
                self.break_on(
                    format!(
                        "settling a cross of {} {} at {} to {} would overflow its lot or cash \
                         balance, so neither side's book was moved",
                        cross.quantity,
                        cross.object_id.as_str(),
                        cross.price,
                        strategy.as_str()
                    ),
                    now,
                );
                return;
            };
            settled.push((position_key, strategy, next_held, next_balance));
        }
        for (position_key, strategy, next_held, next_balance) in settled {
            self.strategy_positions.insert(position_key, next_held);
            self.strategy_cash
                .insert(strategy.as_str().to_string(), next_balance);
        }
    }

    /// The instrument key the crossing window is kept by: what `net` groups
    /// on, so one window per net key.
    fn crossing_key(net_intent: &NetIntent) -> String {
        format!(
            "{}/{}/{}",
            net_intent.venue.as_str(),
            net_intent.object_id.as_str(),
            net_intent.representation.as_str()
        )
    }

    /// Whether a sample is inside the configured window at `now`, this pass.
    fn in_crossing_window(&self, sample: &CrossingSample, now: Timestamp) -> bool {
        match self.config.crossing_interval {
            None => false,
            // The last `n` passes, this one included: a sample from pass
            // `p` is in while `p + n > current`.
            Some(CrossingInterval::Passes(passes)) => {
                sample.pass.saturating_add(u64::from(passes)) > self.pass
            }
            Some(CrossingInterval::Span(span)) => sample.at >= now.saturating_sub(span),
        }
    }

    /// What the window has seen for this net's instrument, before this net.
    ///
    /// Samples that have left the window are dropped here, so a history is
    /// as long as its window and no longer. With no interval configured the
    /// history is never written and this is zero, zero, not full.
    fn crossing_window(&mut self, net_intent: &NetIntent, now: Timestamp) -> CrossingWindow {
        let mut window = CrossingWindow::default();
        if self.config.crossing_interval.is_none() {
            return window;
        }
        let key = Self::crossing_key(net_intent);
        let Some(history) = self.crossing_history.get(&key) else {
            return window;
        };
        let live: Vec<CrossingSample> = history
            .iter()
            .filter(|sample| self.in_crossing_window(sample, now))
            .copied()
            .collect();
        if let Some(history) = self.crossing_history.get_mut(&key) {
            history.clear();
            history.extend(live.iter().copied());
        }
        // A history at the bound has had its oldest sample dropped by
        // `observe_crossing`, so what it holds is not the whole window.
        window.full = live.len() >= MAX_CROSSING_WINDOW_SAMPLES;
        for sample in &live {
            // Saturating in the safe direction: a total that cannot be
            // summed reads as the largest representable, so the cap's own
            // checked sum over it overflows and refuses rather than
            // trusting a wrapped number.
            window.gross = window
                .gross
                .checked_add(sample.gross)
                .unwrap_or(Decimal::MAX);
            window.crossed = window
                .crossed
                .checked_add(sample.crossed)
                .unwrap_or(Decimal::MAX);
        }
        window
    }

    /// Record one settled net in its instrument's window.
    ///
    /// Written only when an interval is configured, so the per-net default
    /// allocates nothing. At the bound the oldest sample is dropped rather
    /// than the newest: the newest is the one the next evaluation must see,
    /// and `crossing_window` reports the truncation as `full`.
    fn observe_crossing(&mut self, net_intent: &NetIntent, crossed: Decimal, now: Timestamp) {
        if self.config.crossing_interval.is_none() {
            return;
        }
        let history = self
            .crossing_history
            .entry(Self::crossing_key(net_intent))
            .or_default();
        if history.len() >= MAX_CROSSING_WINDOW_SAMPLES {
            history.pop_front();
        }
        history.push_back(CrossingSample {
            pass: self.pass,
            at: now,
            gross: net_intent.gross_size,
            crossed,
        });
    }

    /// Seal a cross into the hash-chained journal and report it.
    ///
    /// Separate from working the cross out, so that the record is written only
    /// once the pass it belongs to has actually produced its outcome.
    fn record_cross(
        &mut self,
        crossed: Option<InternalCross>,
        now: Timestamp,
        report: &mut WorkReport,
    ) {
        let Some(cross) = crossed else {
            return;
        };
        self.journal.record(
            Decision::CrossedInternally {
                object: cross.object_id.as_str().to_string(),
                venue: cross.venue.as_str().to_string(),
                quantity: cross.quantity.to_string(),
                price: cross.price.to_string(),
                bought: cross
                    .bought
                    .iter()
                    .map(|id| id.as_str().to_string())
                    .collect(),
                sold: cross
                    .sold
                    .iter()
                    .map(|id| id.as_str().to_string())
                    .collect(),
            },
            now,
        );
        self.metrics.internal_cross(&cross.venue);
        report.crosses.push(cross);
    }

    /// Judge one intent against the feasibility gate, refusing and counting
    /// it under the rule that bound.
    ///
    /// The book is read here, at the netting instant, and the gate itself is
    /// a pure function of what it is handed — so the fact judged is the one
    /// the journal can replay. The size resting at the touch is read on the
    /// side the intent *takes*: a buy takes the ask, so it is the ask's size
    /// that bounds it.
    fn admit_feasible(&mut self, intent: &Intent, now: Timestamp, report: &mut WorkReport) -> bool {
        let touch = self
            .liquidity
            .get(&intent.venue, &intent.object_id)
            .and_then(|state| {
                if intent.signed_size.is_positive() {
                    state.best_ask()
                } else {
                    state.best_bid()
                }
            })
            .map(|level| level.size);
        let verdict = feasibility::assess(
            self.config.feasibility.get(intent.venue.as_str()),
            self.feasibility_constraints(),
            intent,
            touch,
        );
        match verdict {
            Ok(()) => true,
            Err(infeasible) => {
                self.refuse(report, infeasible.gate, &infeasible.reason, now);
                // The one refusal that is about a venue carries it, keyed
                // on the entry `refuse` just pushed, so the centre's window
                // can say where the lot gate fired and not only that it did.
                report
                    .feasibility_venues
                    .push((report.refusals.len() - 1, intent.venue.clone()));
                false
            }
        }
    }

    /// Item 11 of the applied policy payload, if the centre has produced it.
    ///
    /// Read whatever its freshness: a venue's minimum order and tick change
    /// on the order of months, the slot's own time-to-live is a day, and a
    /// constraint that has gone stale is still the last thing the centre
    /// knew rather than nothing. The degradation table already narrows the
    /// cell's sizing on the slot's staleness; refusing to read the slot as
    /// well would be a second control on the same fact with a different
    /// threshold.
    /// ADR 0080's slot as the applied payload carries it, if the applied
    /// payload carries one. Read from the payload `apply_policy` swapped in,
    /// so a payload refused there — replayed, re-addressed, unverified — is
    /// never read here.
    pub fn dispositions(&self) -> Option<&Dispositions> {
        self.policy
            .as_ref()
            .and_then(|policy| policy.payload().dispositions.value())
    }

    fn feasibility_constraints(&self) -> Option<&qip_contracts::policy::FeasibilityConstraints> {
        self.policy
            .as_ref()
            .and_then(|policy| policy.payload().feasibility_constraints.value())
    }

    /// The venue a signal on `object` is reasoned at, chosen before the
    /// intent exists (ADR 0078).
    ///
    /// This is §27.2's consolidation: every strategy's signal on one
    /// instrument in one pass resolves here to the same venue, lands on the
    /// same netting key and becomes one order. "Best" is the tightest quoted
    /// top-of-book spread among the configured venues whose book is usable
    /// at this instant — present, not stale, accepting orders, serving a mid
    /// — compared in `Decimal`, ties broken by `VenueId` order rather than
    /// configured order so two cells configured in different orders choose
    /// alike. The choice is journaled with every candidate and the spread it
    /// was compared on, so the pick is reproducible from the chain alone.
    /// The spread is the one cost the cell measures itself on every pass;
    /// a fee floor from policy slot 11 may join later and may never override
    /// a staleness refusal. Nothing here calls the centre (ADR 0008).
    ///
    /// **When no book is usable, the first venue holding any book at all is
    /// returned rather than `None`, and no choice is journaled.** That is
    /// not a substitution: the gates in `intent_for` then refuse it under
    /// the specific reason — `stale_book`, `venue_status`, `pricing` — which
    /// an operator can act on, where a `venue_selection` refusal would say
    /// only that nothing was chosen. `None`, and that refusal, is for a cell
    /// holding no book for the instrument at any venue it may reach.
    fn venue_for(&mut self, object: &ObjectId, now: Timestamp) -> Option<VenueId> {
        // A `BTreeMap` because the candidate order reaches the journal and
        // decides the tie-break.
        let mut candidates: BTreeMap<VenueId, Decimal> = BTreeMap::new();
        let mut fallback: Option<VenueId> = None;
        for venue in &self.config.venues {
            let Some(state) = self.liquidity.get(venue, object) else {
                continue;
            };
            if state.status() == VenueStatus::Unreachable {
                continue;
            }
            if fallback.is_none() {
                fallback = Some(venue.clone());
            }
            if state.is_stale() || !state.status().accepts_orders() || state.mid().is_none() {
                continue;
            }
            let Some(spread) = state.spread() else {
                continue;
            };
            candidates.insert(venue.clone(), spread);
        }
        let chosen = candidates
            .iter()
            .min_by(|(venue_a, spread_a), (venue_b, spread_b)| {
                spread_a.cmp(spread_b).then(venue_a.cmp(venue_b))
            })
            .map(|(venue, _)| venue.clone());
        let Some(chosen) = chosen else {
            return fallback;
        };
        self.journal.record(
            Decision::VenueChosen {
                object: object.as_str().to_string(),
                venue: chosen.as_str().to_string(),
                candidates: candidates
                    .iter()
                    .map(|(venue, spread)| (venue.as_str().to_string(), spread.to_string()))
                    .collect(),
            },
            now,
        );
        Some(chosen)
    }

    /// Take the region hold for an admitted size, or refuse it.
    ///
    /// Returns `false` when the caller must abandon what it was building.
    /// The hold is taken *before* the `Intent` exists, which is what makes
    /// this a reservation rather than a check: there is no way to learn the
    /// allocation covers this notional without simultaneously holding it, so
    /// a second strategy in the same pass cannot also pass against it. §28
    /// puts strategy-level limits before netting for the same reason the
    /// envelope check is here — a strategy that has exhausted the region's
    /// budget must not contribute to a net at all.
    ///
    /// Refused whole, never reduced. `CapitalGrant::Reduced` narrows an order
    /// to what one strategy's envelope has left; a region allocation short of
    /// what a strategy asked for is a different fact — the capital is
    /// somewhere else — and trimming would spend a remainder the centre may
    /// have promised elsewhere.
    fn hold_region_capital(
        &mut self,
        id: String,
        notional: Decimal,
        now: Timestamp,
        report: &mut WorkReport,
    ) -> bool {
        let pass = self.pass;
        // The borrow ends with the match, so the refusal path below can take
        // `&mut self` to journal why it refused.
        let outcome = match self.region_allocation.as_ref() {
            None => return true,
            Some(allocation) => allocation.reserve(&self.config.cell_id, id, notional, pass),
        };
        match outcome {
            Ok(()) => true,
            Err(error) => {
                self.refuse(report, "region_reservation", error.message(), now);
                false
            }
        }
    }

    /// Give back one strategy's region hold, because its intent is not
    /// becoming an order.
    fn release_region_hold_for(&mut self, strategy: &str) {
        let pass = self.pass;
        if let Some(allocation) = self.region_allocation.as_ref() {
            // The amount returns to the free balance inside the ledger; this
            // path has nothing to charge it against.
            let _ = allocation.release(&self.config.cell_id, &region_hold_id(pass, strategy));
        }
    }

    /// Give back the holds a net's contributors took.
    ///
    /// Called on every path in [`Self::place_net`] that returns without
    /// sending. A contributor whose hold is already gone is passed over: the
    /// only things that remove a hold mid-pass are this and the commit below,
    /// and both are terminal for the net.
    fn release_region_holds(&mut self, contributors: &[Contributor]) {
        for contributor in contributors {
            self.release_region_hold_for(contributor.strategy.as_str());
        }
    }

    /// Turn a net's holds into spend, and say how much that was.
    ///
    /// The whole hold is committed, not the contributor's share of the sent
    /// order. A strategy whose intent partly cancelled inside the net
    /// therefore spends slightly more of the region's budget than reached the
    /// venue. That is the conservative direction, and it is what putting the
    /// bound before netting buys: the alternative gives a strategy that
    /// offset against another its budget back to bid for again in the same
    /// pass.
    ///
    /// The sum is kept on the order so that an expiry with nothing filled
    /// returns exactly this and nothing recomputed.
    fn commit_region_holds(&mut self, contributors: &[Contributor]) -> Decimal {
        let pass = self.pass;
        let mut committed = Decimal::ZERO;
        if let Some(allocation) = self.region_allocation.as_ref() {
            for contributor in contributors {
                // `None` is unreachable here: the key is derived from the same
                // (pass, strategy) that took the hold, and the only thing that
                // could remove it in between is the sweep, which runs at the
                // top of a pass and cannot run inside one.
                if let Some(amount) = allocation.commit(
                    &self.config.cell_id,
                    &region_hold_id(pass, contributor.strategy.as_str()),
                ) {
                    committed += amount;
                }
            }
        }
        committed
    }

    /// Give back a cycle's region hold, because the cycle is not going out.
    fn release_cycle_hold(&mut self, cycle_id: &str) {
        let pass = self.pass;
        if let Some(allocation) = self.region_allocation.as_ref() {
            let _ = allocation.release(
                &self.config.cell_id,
                &region_hold_id_for_cycle(pass, cycle_id),
            );
        }
    }

    /// Turn a cycle's region hold into spend, and say how much that was.
    ///
    /// The amount is returned rather than discarded because a cycle that
    /// rests a leg commits here and may still be abandoned without ever
    /// becoming a position — and the capital it gets back then must be
    /// exactly what it spent, not a figure recomputed from the legs. The same
    /// reason `Working::region_committed` keeps a net's commit on the order.
    fn commit_cycle_hold(&mut self, cycle_id: &str) -> Decimal {
        let pass = self.pass;
        let mut committed = Decimal::ZERO;
        if let Some(allocation) = self.region_allocation.as_ref()
            && let Some(amount) = allocation.commit(
                &self.config.cell_id,
                &region_hold_id_for_cycle(pass, cycle_id),
            )
        {
            committed = amount;
        }
        committed
    }

    fn refuse(&mut self, report: &mut WorkReport, gate: &str, reason: &str, now: Timestamp) {
        // Every gate a *pass* can refuse at funnels through here, so one
        // recording site covers all of them. `gate` is a string literal at
        // each call, and that is what bounds this series' cardinality. The
        // three refusals that journal directly — a replayed halt, a release
        // that predates its barrier, and a net that cancelled to zero — are
        // not pass-time gates and are deliberately not counted here: the
        // first two are control-plane events with no "why was the cell
        // quiet" reading, and the third is counted as a cancellation.
        self.metrics.refusal(gate);
        report.refusals.push((gate.to_string(), reason.to_string()));
        self.journal.record(
            Decision::Refused {
                gate: gate.to_string(),
                reason: reason.to_string(),
            },
            now,
        );
    }

    // --- the mesh seam ------------------------------------------------------

    /// Describe this cell to the central plane.
    ///
    /// Assembled from what the cell already holds rather than accumulated as it
    /// goes, so a delta is a *view* and building one twice with the same report
    /// produces the same value. That matters because the transport underneath
    /// is at-least-once: a delta that is rebuilt and re-sent after a failed
    /// attempt has to be the same fact, not a second one.
    ///
    /// `cell`, `region` and `sequence` are filled in by
    /// [`crate::mesh::CellUplink::publish`], which owns the stream's numbering.
    /// A cell that numbered its own deltas would eventually skip one, and the
    /// centre cannot tell a skipped sequence from a lost delta.
    pub fn state_delta(&self, report: &WorkReport, at: Timestamp) -> CellStateDelta {
        let mut delta = CellStateDelta {
            cell: self.config.cell_id.clone(),
            region: self.config.region.clone(),
            sequence: 0,
            at,
            halted: self.is_halted(),
            utilisation: self
                .deployed
                .values()
                .map(|deployed| StrategyUtilisation {
                    strategy: deployed.envelope.strategy().clone(),
                    utilisation: deployed.utilisation.clone(),
                    envelope_expires_at: deployed.envelope.expires_at(),
                })
                // The desk spends an envelope too, and the centre that issued
                // it hears how much the same way.
                .chain(
                    self.desk
                        .as_ref()
                        .map(|installed| &installed.desk)
                        .map(|desk| StrategyUtilisation {
                            strategy: desk.strategy().clone(),
                            utilisation: desk.utilisation().clone(),
                            envelope_expires_at: desk.envelope().expires_at(),
                        }),
                )
                .collect(),
            orders: report
                .orders
                .iter()
                .map(|order| DeltaOrder {
                    order_id: order.order_id.clone(),
                    strategy: order.strategy.clone(),
                    object_id: order.object_id.clone(),
                    venue: order.venue.clone(),
                    side: order.side,
                    quantity: order.quantity,
                    price: order.price,
                    simulated: order.simulated,
                    contributors: order.contributors.clone(),
                })
                .collect(),
            // From the fills the venue reported this pass and nothing else.
            // The list above is what was sent; deriving a fill from it is the
            // reading that charged the centre for resting orders.
            fills: report
                .fills
                .iter()
                .map(|fill| qip_contracts::wire::FillRecord {
                    order_id: fill.order_id.clone(),
                    object_id: fill.object_id.clone(),
                    venue: fill.venue.clone(),
                    side: fill.side,
                    quantity: fill.quantity,
                    price: fill.price,
                    simulated: fill.simulated,
                    at: fill.at,
                    shares: fill
                        .shares
                        .iter()
                        .map(|(strategy, quantity)| qip_contracts::wire::FillShare {
                            strategy: strategy.clone(),
                            quantity: *quantity,
                        })
                        .collect(),
                })
                .collect(),
            // Set by `bound_refusals` below, like the other two counters.
            fills_omitted: 0,
            refusals: report
                .refusals
                .iter()
                .enumerate()
                .map(|(index, (gate, reason))| DeltaRefusal {
                    gate: gate.clone(),
                    reason: reason.clone(),
                    // Joined by the index `admit_feasible` recorded, so a
                    // feasibility refusal carries its venue and nothing
                    // else does.
                    venue: report
                        .feasibility_venues
                        .iter()
                        .find(|(at, _)| *at == index)
                        .map(|(_, venue)| venue.as_str().to_string()),
                })
                .collect(),
            // Set by `bound_refusals` below; the caller does not get to claim
            // a truncation that did not happen.
            refusals_omitted: 0,
            reconciliation_breaks: self.breaks.clone(),
            reconciliation_breaks_omitted: self.breaks_omitted,
            crosses: report
                .crosses
                .iter()
                .map(|cross| qip_contracts::wire::CrossRecord {
                    object_id: cross.object_id.clone(),
                    venue: cross.venue.clone(),
                    quantity: cross.quantity,
                    price: cross.price,
                    bought: cross.bought.clone(),
                    sold: cross.sold.clone(),
                })
                .collect(),
            // Set by `bound_refusals` below, like the refusal counter: the
            // caller does not get to claim a truncation that did not happen.
            crosses_omitted: 0,
        };
        delta.bound_refusals();
        delta
    }

    /// Install a capital envelope the centre issued for a strategy already
    /// deployed here.
    ///
    /// Takes the verified type, so there is no path from a frame off the wire
    /// to a live grant that does not go through
    /// [`crate::VerifiedEnvelope::verify`]. Arriving over the mesh buys an
    /// envelope nothing; this signature is what says so.
    ///
    /// Three things this deliberately does not do:
    ///
    /// * **It does not deploy.** A grant names a strategy; it does not carry
    ///   the compiled strategy or the program its plan indexes into, and a cell
    ///   that started running something because capital arrived for it would be
    ///   promoting its own strategy — the thing ADR 0008 says a cell never
    ///   does. An envelope for a strategy that is not deployed is refused.
    /// * **It does not reset utilisation.** What a strategy has committed is
    ///   measured against positions that are still open, and a renewal that
    ///   zeroed it would hand the strategy its whole gross limit again while
    ///   the previous commitment was still live. Carrying it across is the
    ///   conservative direction, and it is the one that is right.
    /// * **It does not widen anything by itself.** The new envelope replaces
    ///   the old one entirely — wider or narrower — because that is what the
    ///   centre signed. A cell that merged the two would be constructing a
    ///   grant nobody approved.
    pub fn renew_capital(&mut self, envelope: VerifiedEnvelope, now: Timestamp) -> Result<()> {
        // `verify` has already checked the cell, and this checks it again
        // against the cell's own identity rather than against the string a
        // caller passed to the verifier. The two are the same today; a
        // downlink misconfigured with another cell's name is the case where
        // they would not be, and that is exactly the case worth catching.
        if envelope.cell() != self.config.cell_id {
            return Err(Error::denied(format!(
                "an envelope for cell {} cannot renew capital at {}",
                envelope.cell(),
                self.config.cell_id
            )));
        }
        let key = envelope.strategy().as_str().to_string();
        let approver = envelope.approver().to_string();
        let expires_at = envelope.expires_at();
        if let Some(desk) = self.desk.as_mut().map(|installed| &mut installed.desk)
            && desk.strategy().as_str() == key
        {
            // The desk is renewed by the same rules as a strategy: the grant
            // replaces the old one whole and utilisation carries across.
            desk.replace_envelope(envelope);
            self.journal.record(
                Decision::CapitalRenewed {
                    strategy: key,
                    approver,
                    expires_at,
                },
                now,
            );
            return Ok(());
        }
        let Some(deployed) = self.deployed.get_mut(&key) else {
            return Err(Error::not_found(format!(
                "no strategy {key} is deployed at this cell, so there is nothing for the grant to \
                 fund; a cell does not deploy a strategy because capital arrived for it"
            )));
        };
        deployed.envelope = envelope;
        self.journal.record(
            Decision::CapitalRenewed {
                strategy: key,
                approver,
                expires_at,
            },
            now,
        );
        // The grant under this name has a new signature. If the applied
        // manifest names it the share widens to what the centre already
        // checked; if it names only the old one the share narrows, until
        // the next payload names the renewal. Either way the sum is of
        // grants the centre signed, never of a number the cell chose.
        self.rederive_region_share(now);
        Ok(())
    }

    /// Reconciliation breaks this cell has recorded, oldest first.
    /// What each venue's message budget holds, in venue order (§29.2).
    ///
    /// The idle reading is the point: a cell that has sent nothing reports
    /// a full bucket, an unnarrowed monitor and zero messages, which is a
    /// different state from a cell that has spent its rate limit and a
    /// different state again from one with no venue at all.
    pub fn quote_budget(&self) -> Vec<crate::quoting::VenueBudgetState> {
        self.budget.summary()
    }

    /// How depleted `venue`'s message budget is (§29.2's threshold
    /// adaptation).
    ///
    /// The requoter asks this before it asks whether an order is stale, and
    /// widens the threshold it judges staleness on by
    /// [`Depletion::widen_ticks`] and [`Depletion::widen_bps`]. The failure
    /// that motivates it is the one the whole budget exists for, arriving by
    /// a different door: quote traffic dominates order traffic, so a session
    /// is far likelier to be cut off by its repricing than by its sending,
    /// and a cell that repriced at the same threshold all the way down to an
    /// empty bucket would spend its last messages on whichever instrument
    /// ticked first and have none left for the orders that had moved
    /// furthest — nor for the mass cancel.
    ///
    /// Unwidened while the budget is [`Depletion::Ample`], which is what
    /// makes this an adaptation rather than a new limit: a cell with a full
    /// bucket reprices exactly as it did before this existed.
    pub fn quote_depletion(&self, venue: &str) -> Depletion {
        self.budget.depletion(venue)
    }

    /// Whether `venue`'s budget could fund a whole requote at `now`.
    ///
    /// Spends nothing. Asked before the repricer is consulted so that the
    /// repricer's own throttle budgets — which count instructions sent —
    /// are never spent on an instruction the venue session could not carry.
    pub fn requote_fundable(&mut self, venue: &VenueId, now: Timestamp) -> bool {
        self.budget.requote_fundable(venue, now)
    }

    /// Spend a requote's two messages at `venue`, both or neither.
    ///
    /// This is the seam that makes the §29.2 budget able to fire at all for
    /// the traffic it was written about. Until it existed the node's
    /// requoter sent its cancel and its replacement straight at the venue
    /// gateway, so every requote was two messages the budget never saw: the
    /// cell believed in headroom it had already spent, and the disconnect
    /// the budget exists to pre-empt would have arrived with the bucket
    /// reading full.
    ///
    /// A refusal is recorded on [`GATE_QUOTE_BUDGET`] here rather than
    /// pushed onto a [`WorkReport`], because a requote happens outside
    /// [`Cell::work`] and there is no report to push onto — the same reason
    /// [`Cell::send`] records directly.
    pub fn spend_requote(&mut self, venue: &VenueId, now: Timestamp) -> Admission {
        let admission = self.budget.admit_requote(venue, now);
        if !admission.is_admitted() {
            self.metrics.refusal(GATE_QUOTE_BUDGET);
        }
        admission
    }

    /// What each venue's fill-time history holds, in venue order (§32.1).
    pub fn fill_times(&self) -> Vec<crate::dispersion::VenueFillTimeState> {
        self.fill_times.summary()
    }

    pub fn reconciliation_breaks(&self) -> &[String] {
        &self.breaks
    }

    // --- reconciliation and the mirror --------------------------------------

    /// Absorb a fill from the independent drop-copy channel.
    pub fn observe_drop_copy(&mut self, fill: DropCopyFill) {
        self.dropcopy.observe(fill);
    }

    /// Compare the two records and halt on any disagreement.
    ///
    /// The cell's side is its *confirmed* fills — what the order-entry
    /// channel reported — never what it sent. An order resting unfilled is
    /// on neither side and is not a break; a fill on either side alone is.
    /// A clean comparison settles every closed order, which is what keeps
    /// both records bounded by the orders still working.
    pub fn reconcile(&mut self, now: Timestamp) -> Vec<Discrepancy> {
        let fills: Vec<CellFill> = self
            .confirmed
            .iter()
            .map(|fill| CellFill {
                order_id: fill.order_id.clone(),
                venue: fill.venue.clone(),
                quantity: fill.quantity,
                price: fill.price,
            })
            .collect();
        let breaks = self.dropcopy.reconcile(&fills);
        for discrepancy in &breaks {
            self.break_on(discrepancy.describe(), now);
        }
        if breaks.is_empty() {
            self.settle();
        }
        breaks
    }

    /// Ship the journal to durable storage.
    ///
    /// The only call in the cell that may block, and deliberately outside both
    /// [`Cell::on_bytes`] and [`Cell::work`].
    pub fn flush(&mut self, mirror: &mut dyn Mirror, now: Timestamp) -> Result<usize> {
        let watermarks = self
            .sequencer
            .watermarks()
            .into_iter()
            .map(|mark| (mark.stream, mark.position))
            .collect();
        crate::journal::ship(
            &mut self.journal,
            mirror,
            &self.config.cell_id,
            watermarks,
            now,
        )
    }
}

/// What the polled halt flag read as, on one poll (§46.2's second wire).
///
/// Built from the flag's bytes by [`Self::from_content`] and from the
/// failure to obtain them by the node, which is the only thing that touches
/// the file. The cell never reads a path: it is handed the reading, so the
/// same seam is driven by a test with no file at all.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PolledHalt {
    /// No flag exists. Not halted: the deployment shape is a file the
    /// operator creates to halt and removes to release, and a node whose
    /// operator has never written one is running. A *missing mount* is not
    /// this — see [`crate::cell::PolledHalt::Unreadable`].
    Absent,
    /// The flag exists and reads `released`. Not halted: the managed-store
    /// shape keeps a key that always exists, and this is its off state.
    Released,
    /// The flag is engaged, with the reason it carried.
    Engaged(String),
    /// The flag could not be read or could not be understood: a permission
    /// error, a missing directory, more bytes than a flag may hold, bytes
    /// that are not text, or text that is neither word. Halted, because a
    /// wire whose state is unknown is a wire that has failed, and a kill
    /// switch fails engaged.
    Unreadable(String),
}

impl PolledHalt {
    /// The most bytes a flag may hold. A flag is a word and a short reason;
    /// a file larger than this is not the flag, whatever put it there.
    pub const MAX_CONTENT_BYTES: usize = 256;

    /// Read the flag's bytes.
    ///
    /// Two words are understood: `released`, and `engaged` optionally
    /// followed by a colon and a reason. An empty file is engaged — its
    /// presence is the signal in the file-per-halt shape. Anything else
    /// halts as unreadable; the content is not echoed into the reason,
    /// because whatever ended up in the file is not a fact the chain should
    /// carry.
    pub fn from_content(bytes: &[u8]) -> Self {
        if bytes.len() > Self::MAX_CONTENT_BYTES {
            return Self::Unreadable(format!(
                "the flag holds {} bytes and a flag may hold at most {}",
                bytes.len(),
                Self::MAX_CONTENT_BYTES
            ));
        }
        let Ok(text) = std::str::from_utf8(bytes) else {
            return Self::Unreadable("the flag is not text".to_string());
        };
        let text = text.trim();
        if text.is_empty() || text == "engaged" {
            return Self::Engaged("the flag is present".to_string());
        }
        if let Some(reason) = text.strip_prefix("engaged:") {
            let reason = reason.trim();
            return Self::Engaged(if reason.is_empty() {
                "the flag is present".to_string()
            } else {
                reason.to_string()
            });
        }
        if text == "released" {
            return Self::Released;
        }
        Self::Unreadable("the flag holds text that is neither `engaged` nor `released`".to_string())
    }

    /// Whether this reading stops the cell.
    pub const fn halts(&self) -> bool {
        matches!(self, Self::Engaged(_) | Self::Unreadable(_))
    }

    /// One bounded word per arm, for a health body or a label.
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Absent => "absent",
            Self::Released => "released",
            Self::Engaged(_) => "engaged",
            Self::Unreadable(_) => "unreadable",
        }
    }

    /// The reading, for the journal.
    pub fn describe(&self) -> String {
        match self {
            Self::Absent => "is absent".to_string(),
            Self::Released => "reads released".to_string(),
            Self::Engaged(reason) => format!("is engaged: {reason}"),
            Self::Unreadable(reason) => format!("is unreadable and reads as engaged: {reason}"),
        }
    }
}

/// One net as the crossing window saw it: when, and how much of its gross
/// was crossed.
#[derive(Clone, Copy, Debug)]
struct CrossingSample {
    pass: u64,
    at: Timestamp,
    gross: Decimal,
    crossed: Decimal,
}

/// The window's totals for one instrument, before the net being judged.
#[derive(Clone, Copy, Debug, Default)]
struct CrossingWindow {
    gross: Decimal,
    crossed: Decimal,
    /// The history was truncated at its bound, so these totals understate
    /// the window and the cap must refuse rather than measure.
    full: bool,
}

/// An arbitrage desk and the path router built for it, as one thing.
///
/// A pair rather than two `Option` fields on the cell, and that is the whole
/// point of the type: a desk without a router would be a cell that scans
/// cycles it cannot classify, and the only honest thing such a cell could do
/// is refuse every cycle it finds — a branch no input could reach, reading in
/// the source as a control and guarding nothing. Holding both in one `Option`
/// makes the pairing something the type system keeps rather than something
/// [`Cell::install_arbitrage`] is trusted to have kept.
///
/// The router is built once, where the desk is installed. Rebuilding it per
/// pass would re-read configuration on the hot path, and configuration read on
/// the hot path is how a pass comes to depend on when it ran.
#[derive(Debug)]
struct InstalledDesk {
    desk: ArbitrageDesk,
    /// Blueprint §30.2's path router (ADR 0068). It classifies and cannot
    /// route: it names no venue, produces no order, and holds no `Decimal`.
    router: CycleRouter,
}

/// A cycle past every gate and waiting for the nets to go out first.
#[derive(Clone, Debug)]
struct AdmittedCycle {
    cycle_id: String,
    /// The scanner's net edge, in units of the start instrument.
    net: Decimal,
    /// In plan order. Every one carries `NettingPolicy::NoNet` by
    /// construction, which is what `CycleLeg` exists to guarantee.
    legs: Vec<Intent>,
    /// The sum of the legs' notionals, admitted against the desk's envelope.
    notional: Decimal,
}

/// A cycle whose slow leg is resting while the rest of it waits (§32.1).
///
/// Everything needed to finish the cycle on a later pass, and nothing that
/// could be re-derived from somewhere else: the admitted cycle as it was
/// gated, which leg rested and as which order, what that leg was actually
/// asked for, the decomposition it carries, and what the cycle committed of
/// the region's capital. The last two are held rather than recomputed for the
/// same reason — a fraction re-derived on the completing pass and a commit
/// recomputed from the legs are both second claims about a fact the cell
/// already has, and the two would disagree.
#[derive(Clone, Debug)]
struct SuspendedCycle {
    cycle: AdmittedCycle,
    /// Index into `cycle.legs` of the leg that is resting.
    position: usize,
    order_id: String,
    /// What the venue was asked for on that leg, which is what its fill is a
    /// fraction of — never the planned size, which a decomposition may
    /// already have reduced.
    sent: Decimal,
    decomposition: Decomposition,
    /// The region capital the cycle spent when the leg went out, returned in
    /// full if the leg is withdrawn having filled nothing.
    committed: Decimal,
}

/// The gate literal a scan rejection is counted under.
///
/// One literal per stage of the scanner, so §30.1's question — which stage
/// refuses most of what the search proposes — is a series rather than a
/// grep of the journal. Bounded by the enum.
/// The key one strategy's region hold is taken under.
///
/// The pass is part of the key so a hold leaked from an earlier pass cannot
/// be mistaken for this pass's and committed by it. Phase one admits at most
/// one intent per strategy, so the strategy is the rest of the key.
fn region_hold_id(pass: u64, strategy: &str) -> String {
    format!("{pass}:strategy:{strategy}")
}

/// The key one cycle's region hold is taken under. A cycle is admitted and
/// refused whole, so it holds once for the sum of its legs.
fn region_hold_id_for_cycle(pass: u64, cycle_id: &str) -> String {
    format!("{pass}:cycle:{cycle_id}")
}

const fn scan_gate(stage: RejectionStage) -> &'static str {
    match stage {
        RejectionStage::Unsized => "arbitrage_scan_unsized",
        RejectionStage::ExactArithmetic => "arbitrage_scan_exact_arithmetic",
        RejectionStage::Unpriceable => "arbitrage_scan_unpriceable",
        RejectionStage::Depth => "arbitrage_scan_depth",
        RejectionStage::Book => "arbitrage_scan_book",
        RejectionStage::NetEdge => "arbitrage_scan_net_edge",
        RejectionStage::Plan => "arbitrage_scan_plan",
    }
}

/// Where a cell sends an order.
///
/// Narrower than the routing crate's `Gateway` on purpose: the cell needs to
/// place and to know whether the venue is simulated, and a wider surface here
/// would be a wider surface to get wrong.
pub trait Placer: std::fmt::Debug {
    /// Whether this is a simulated venue. The cell sets every order's
    /// `simulated` flag from this rather than from anything the caller says.
    fn is_simulated(&self) -> bool;

    /// Accept an order for the venue.
    ///
    /// `at` is the instant before which the gateway must not release the
    /// order — "release no earlier than", not "when it was sent" (ADR 0084).
    /// The cell stamps it from the cycle's release schedule so that the legs
    /// of a multi-venue cycle arrive together; a gateway that releases
    /// immediately whatever `at` says was correct before that record and is
    /// wrong after it. A gateway that finds `at` further in the past than it
    /// will send late withdraws the order and reports it through
    /// [`Self::unreleased`] rather than sending it late.
    #[allow(clippy::too_many_arguments)]
    fn place(
        &mut self,
        order_id: &str,
        object_id: &ObjectId,
        venue: &VenueId,
        side: BookSide,
        quantity: Decimal,
        price: Decimal,
        at: Timestamp,
    ) -> Result<()>;

    /// Orders the gateway withdrew without sending since the last call,
    /// because their release instant had passed by more than it will send
    /// late (ADR 0084 §4).
    ///
    /// Defaults to nothing, which is right for a gateway that releases every
    /// order on the pass it is placed and so never holds one to be late with.
    fn unreleased(&mut self) -> Vec<UnreleasedOrder> {
        Vec::new()
    }

    /// What a production deployment must supply, empty when usable as is.
    fn required_configuration(&self) -> Vec<String> {
        Vec::new()
    }

    /// Everything the order-entry channel has reported filled since the
    /// last call: the fills the venue returned on acceptance, and later
    /// reports on orders that rested.
    ///
    /// Defaults to nothing, which is the honest answer for a gateway that
    /// has no such channel — its orders are then accepted and never filled,
    /// and the cell holds them open rather than assuming. A gateway must not
    /// synthesise a report from the order it was handed; the report is the
    /// venue's answer or it is nothing.
    fn execution_reports(&mut self) -> Vec<ExecutionReport> {
        Vec::new()
    }

    /// Whether [`Self::cancel`] reaches the venue. The cell reads this
    /// before it lets an order rest; a gateway answering `false` gets no
    /// resting orders at all.
    fn can_cancel(&self) -> bool {
        false
    }

    /// Withdraw what remains of an order, returning the quantity the venue
    /// says was still open. The default refuses, and a gateway that has a
    /// venue cancel path overrides both this and [`Self::can_cancel`]
    /// together: one without the other is a promise the cell would act on.
    fn cancel(
        &mut self,
        order_id: &str,
        _object_id: &ObjectId,
        venue: &VenueId,
        _at: Timestamp,
    ) -> Result<Decimal> {
        Err(Error::denied(format!(
            "this gateway cannot withdraw order {order_id} from {}; it has no cancel path to the \
             venue",
            venue.as_str()
        )))
    }
}

/// The gap events worth journalling, and what to say about each.
///
/// An opened gap may still fill, so it is recorded as an observation. An
/// abandoned one has already produced a reset and invalidated a book, which is
/// the event an incident review is looking for.
fn gap_detail(event: &qip_sequencing::tracker::SequenceEvent) -> Option<(String, String)> {
    use qip_sequencing::tracker::SequenceEvent;
    match event {
        SequenceEvent::GapOpened {
            stream,
            missing_from,
            missing_to,
        } => Some((
            stream.clone(),
            format!("sequences {missing_from}..={missing_to} are missing; holding for reorder"),
        )),
        SequenceEvent::GapAbandoned {
            stream,
            missing_from,
            missing_to,
            reason,
        } => Some((
            stream.clone(),
            format!(
                "sequences {missing_from}..={missing_to} will not arrive ({reason:?}); \
                 the affected books are reset"
            ),
        )),
        SequenceEvent::StreamStarted { .. }
        | SequenceEvent::Duplicate { .. }
        | SequenceEvent::GapFilled { .. } => None,
    }
}

#[cfg(test)]
#[allow(clippy::panic_in_result_fn)]
mod crossing_tests {
    //! §27.1's crossing price, tested where the two candidate prices differ.
    //!
    //! The behavioural tests in `qip-edge-node` cannot tell the book's mid from
    //! `NetIntent::reference_price`, because a cell prices every intent off the
    //! same mid in the same pass and the two numbers are equal there. A
    //! mutation that priced crosses from the reference price survived those
    //! tests for exactly that reason. These drive the private seam with a net
    //! intent whose reference price is deliberately nothing like the book, so
    //! "the prevailing mid at the netting instant, never a price either side
    //! chose" becomes an assertion instead of a coincidence.

    use super::*;
    use qip_contracts::message::{MarketMessage, MessageBody};
    use qip_contracts::venue::{Origin, VenueStatus};
    use qip_feature_dag::engine::FeatureEngine;
    use qip_feature_dag::state::MarketState;
    use qip_orderbook::venue::VenueState;

    const CELL: &str = "london-1";

    fn object() -> ObjectId {
        ObjectId::from_string("ACME")
    }

    fn venue() -> VenueId {
        VenueId::new("XLON")
    }

    fn at(seconds: i64) -> Timestamp {
        Timestamp::from_secs(1_700_000_000).saturating_add(qip_core::Duration::from_secs(seconds))
    }

    /// A book quoting 99 / 101, so the mid is 100.
    fn book() -> VenueState {
        let mut state = VenueState::aggregated(object(), venue(), VenueStatus::Open);
        for (index, (side, price, size)) in
            [(BookSide::Bid, "99", "900"), (BookSide::Ask, "101", "300")]
                .iter()
                .enumerate()
        {
            let message = MarketMessage::new(
                object(),
                Origin::new(venue(), "feed-a", 0, index as u64),
                MessageBody::LevelSet {
                    side: *side,
                    price: Decimal::parse(price).expect("a decimal literal"),
                    quantity: Decimal::parse(size).expect("a decimal literal"),
                    order_count: None,
                },
                at(index as i64),
                at(index as i64),
            );
            state.apply(&message).expect("a well-formed level");
        }
        state
    }

    fn cell_with_book() -> Result<Cell> {
        let config = CellConfig::new(CELL, "europe-west2").with_venue(venue());
        let features = FeatureEngine::new(MarketState::default(), qip_core::Duration::from_secs(5));
        let mut cell = Cell::new(config, features)?;
        cell.track(book());
        Ok(cell)
    }

    /// Deploy `id` with a marketable pricing policy, so a net it contributes
    /// to can be priced and reach the venue. The nets these tests build by
    /// hand name strategies; a net whose contributors are not deployed is
    /// refused under `pricing` before any venue is called, which is right,
    /// and not what a test of the venue path is looking at.
    fn deploy_marketable(cell: &mut Cell, id: &str) -> Result<()> {
        use qip_contracts::capital::CapitalEnvelope;
        use qip_strategy::catalogue::FeatureCatalogue;
        use qip_strategy::compile::StrategyCompiler;
        use qip_strategy::ir::{Expr, Rule, StrategySpec};

        let mut compiler = StrategyCompiler::new(FeatureCatalogue::new());
        let spec = StrategySpec::new(
            StrategyId::new(id),
            object(),
            qip_core::Duration::from_secs(30),
        )
        .with_rule(Rule::new(
            "always",
            SignalKind::Enter,
            Expr::Flag(true),
            Expr::Exact(Decimal::ONE),
            Expr::Statistic(0.5),
            10,
        ));
        let compiled = compiler.compile(&spec)?;
        let key = b"a-unit-test-envelope-key";
        let build = |signature: &str| {
            CapitalEnvelope::new(
                StrategyId::new(id),
                CELL,
                Decimal::from_int(1_000_000),
                Decimal::from_int(100_000),
                Decimal::from_int(50_000),
                vec![venue()],
                at(0),
                at(3600),
                "alice@example.com",
                signature,
            )
        };
        let unsigned = build("unsigned")?;
        let signature = crate::envelope::sign_payload(key, &unsigned.signing_payload());
        let envelope = VerifiedEnvelope::verify(build(&signature)?, key, CELL, at(1))?;
        cell.deploy_with_pricing(
            compiled,
            compiler.into_program(),
            envelope,
            PricingPolicy::Marketable,
        )
    }

    /// Net the given `(strategy, signed size)` pairs on the fixture instrument
    /// through [`net`] itself, every intent stamped with `reference_price`.
    ///
    /// Not a literal. `NetIntent` is sealed to `net` so that nobody can
    /// assemble a vector of contributors `net` would have refused, and these
    /// tests were the one caller still forging one by hand — a fixture that
    /// bypasses the seam it is meant to drive is a second construction path
    /// with a friendlier name. Going through `net` also means the net's own
    /// reference price is whatever `net` chose, which is what the crossing
    /// test needs to be sure of before it can claim the mid was chosen over
    /// it.
    fn netted(sizes: &[(&str, &str)], reference_price: Decimal) -> NetIntent {
        let intents = sizes
            .iter()
            .map(|(strategy, size)| {
                Intent::new(
                    StrategyId::new(*strategy),
                    object(),
                    venue(),
                    Decimal::parse(size).expect("a decimal literal"),
                    reference_price,
                    at(60),
                )
                .expect("a fixture size is never zero")
            })
            .collect();
        let mut nets = net(intents);
        assert_eq!(
            nets.len(),
            1,
            "directional intents on one instrument and venue net to one group"
        );
        nets.pop().expect("exactly one net was just asserted")
    }

    /// A net of a 100 buy against a 20 sell: 20 crosses, which is a sixth of
    /// the 120 gross and so comfortably under the forty percent cap.
    fn offsetting_net(reference_price: Decimal) -> NetIntent {
        netted(&[("alpha", "100"), ("beta", "-20")], reference_price)
    }

    #[test]
    fn a_cross_is_priced_at_the_book_mid_and_not_at_a_price_either_side_chose() -> Result<()> {
        let mut cell = cell_with_book()?;
        // A reference price nothing in the book could produce. If the cross
        // were priced from the net intent, this is the number that would
        // appear — and it is one contributor's own stamped price, which §27.1
        // forbids by name.
        let chosen = Decimal::parse("12345").expect("a decimal literal");
        let net_intent = offsetting_net(chosen);
        // The premise, in two halves: the net `net` built really carries the
        // chosen price — otherwise the assertion below that the cross did not
        // take it would be true of any implementation — and the two candidate
        // prices really do differ here, which is the whole reason this test
        // exists rather than the behavioural one.
        assert_eq!(
            net_intent.reference_price, chosen,
            "the fixture net does not carry the price it was built from"
        );
        let mid = cell
            .liquidity()
            .get(&venue(), &object())
            .and_then(|state| state.mid())
            .expect("the fixture book serves a mid");
        assert_ne!(mid, chosen, "the fixture cannot distinguish the two prices");

        let mut report = WorkReport::default();
        let crossed = cell.cross_internally(&net_intent, at(10), &mut report);
        cell.settle_cross(&net_intent, crossed, at(10), &mut report);

        assert_eq!(report.crosses.len(), 1, "nothing was crossed: {report:?}");
        assert_eq!(
            report.crosses[0].price, mid,
            "the cross was priced at {} rather than at the mid",
            report.crosses[0].price
        );
        assert_ne!(
            report.crosses[0].price, chosen,
            "the cross took the reference price, which is a price one side chose"
        );
        assert_eq!(
            report.crosses[0].quantity,
            Decimal::parse("20").expect("a decimal literal"),
            "the matched size is the smaller side, not the net or the gross"
        );
        Ok(())
    }

    /// A venue that refuses everything, so `place_net` returns `Err` and the
    /// caller loses the report.
    #[derive(Debug)]
    struct RefusingGateway;

    impl Placer for RefusingGateway {
        fn is_simulated(&self) -> bool {
            true
        }

        fn place(
            &mut self,
            _order_id: &str,
            _object_id: &ObjectId,
            _venue: &VenueId,
            _side: BookSide,
            _quantity: Decimal,
            _price: Decimal,
            _at: Timestamp,
        ) -> Result<()> {
            Err(qip_core::error::Error::io("the venue refused the order"))
        }
    }

    #[test]
    fn a_venue_that_fails_leaves_no_cross_in_the_chain() -> Result<()> {
        // The journal is hash-chained and is the record. `gateway.place`
        // propagates its error out of `place_net` and out of `work`, and the
        // caller loses the report with it — so a cross written before that call
        // would leave the chain asserting that two strategies traded during a
        // pass that produced nothing at all. Nobody can unwrite it afterwards.
        let mut cell = cell_with_book()?;
        deploy_marketable(&mut cell, "alpha")?;
        deploy_marketable(&mut cell, "beta")?;
        let net_intent = offsetting_net(Decimal::parse("100").expect("a decimal literal"));
        let mut report = WorkReport::default();

        let before = cell.journal().entries().len();
        let outcome = cell.place_net(&net_intent, at(10), &mut RefusingGateway, &mut report);

        // Premise: the venue really did fail, so what follows is about the
        // failure path and not about a quiet success.
        assert!(
            outcome.is_err(),
            "the premise failed: the refusing gateway placed an order"
        );
        let crosses: Vec<_> = cell
            .journal()
            .entries()
            .iter()
            .skip(before)
            .filter(|entry| entry.decision.kind() == "crossed_internally")
            .collect();
        assert!(
            crosses.is_empty(),
            "the chain records a cross for a pass that placed nothing: {crosses:?}"
        );
        Ok(())
    }

    /// A gateway whose class the test chooses, recording what it was asked to
    /// place. `Placer::is_simulated` is a trait method any implementation may
    /// answer `false` to — `qip-edge-node`'s two gateways both read it from
    /// the adapter's own `Broker`, never from configuration — which is what
    /// makes the refusal below a control that can fire rather than one held
    /// shut by construction.
    #[derive(Debug)]
    struct ClassedGateway {
        simulated: bool,
        placed: Vec<String>,
    }

    impl Placer for ClassedGateway {
        fn is_simulated(&self) -> bool {
            self.simulated
        }

        fn place(
            &mut self,
            order_id: &str,
            _object_id: &ObjectId,
            _venue: &VenueId,
            _side: BookSide,
            _quantity: Decimal,
            _price: Decimal,
            _at: Timestamp,
        ) -> Result<()> {
            self.placed.push(order_id.to_string());
            Ok(())
        }
    }

    /// Two strategies wanting the same side, so the net does not cancel and
    /// an order really is due at the venue.
    fn one_sided_net(reference_price: Decimal) -> NetIntent {
        netted(&[("alpha", "100"), ("beta", "40")], reference_price)
    }

    #[test]
    fn the_send_seam_refuses_a_live_class_venue_and_the_gateway_is_never_called() -> Result<()> {
        // `Cell::work` refuses a live-class gateway for the whole pass, so
        // this drives `place_net` directly: the guarantee is that the *seam*
        // holds, for any path that reaches a `Placer`, not that one caller
        // happens to check first. The cell's order path had no refusal keyed
        // on the venue class at all — it read `is_simulated` only to stamp
        // the journal — while `qip-execution-engine`'s order manager has
        // refused on the same bit since it was written.
        let price = Decimal::parse("100").expect("a decimal literal");
        let net_intent = one_sided_net(price);

        // Premise: with a simulated gateway this very net reaches the venue.
        // Without this the assertions below would pass against a cell that
        // sends nothing for some entirely unrelated reason.
        let mut cell = cell_with_book()?;
        deploy_marketable(&mut cell, "alpha")?;
        deploy_marketable(&mut cell, "beta")?;
        let mut simulated = ClassedGateway {
            simulated: true,
            placed: Vec::new(),
        };
        let mut report = WorkReport::default();
        let sent = cell.place_net(&net_intent, at(10), &mut simulated, &mut report)?;
        assert!(
            sent.is_some() && simulated.placed.len() == 1,
            "the premise failed: the simulated gateway saw {:?} for a net that should send one \
             order, refusals {:?}",
            simulated.placed,
            report.refusals
        );

        // The same net, the same cell shape, a gateway that says it is live.
        let mut cell = cell_with_book()?;
        deploy_marketable(&mut cell, "alpha")?;
        deploy_marketable(&mut cell, "beta")?;
        let mut live = ClassedGateway {
            simulated: false,
            placed: Vec::new(),
        };
        let mut report = WorkReport::default();
        let before = cell.journal().entries().len();
        let outcome = cell.place_net(&net_intent, at(10), &mut live, &mut report);

        let error = match outcome {
            Ok(placed) => panic!("a live-class gateway was handed an order: {placed:?}"),
            Err(error) => error.to_string(),
        };
        assert!(
            live.placed.is_empty(),
            "the venue was called anyway: {:?}",
            live.placed
        );
        assert_eq!(
            cell.order_sequence, 0,
            "a refused order burned an order number, so the sequence no longer records what \
             the cell sent"
        );
        assert!(
            error.contains("live trading is disabled") && error.contains("paper_trading"),
            "the refusal does not name the venue class or the ceiling in force: {error}"
        );
        // The gate is matched whole rather than by substring: a chain entry
        // filed under `live_venue_something` must not read as this one.
        let gates: Vec<String> = cell
            .journal()
            .entries()
            .iter()
            .skip(before)
            .filter_map(|entry| match &entry.decision {
                Decision::Refused { gate, .. } => Some(gate.clone()),
                _ => None,
            })
            .collect();
        assert!(
            gates.iter().any(|gate| gate == GATE_LIVE_VENUE),
            "the chain does not name the {GATE_LIVE_VENUE} gate: {gates:?}"
        );
        Ok(())
    }

    #[test]
    fn a_fully_offsetting_net_is_out_of_cap_by_arithmetic_and_is_never_crossed() -> Result<()> {
        // §27.1's flagship case, and the one this cap cannot admit. The matched
        // size is `min(buy, sell)` over a denominator of `buy + sell`, so the
        // ratio tops out at one half and hits it exactly when the two sides
        // cancel — always above forty percent. The test exists so the
        // divergence is asserted rather than merely described in a comment
        // somebody may later delete as stale.
        let mut cell = cell_with_book()?;
        let opposed = netted(
            &[("alpha", "100"), ("beta", "-100")],
            Decimal::parse("100").expect("a decimal literal"),
        );
        // Premise: the sides really do cancel, so this is the full-offset case
        // and not merely a large partial one.
        assert!(
            opposed.net_size.is_zero(),
            "the premise needs a total offset"
        );

        let mut report = WorkReport::default();
        let crossed = cell.cross_internally(&opposed, at(10), &mut report);
        cell.settle_cross(&opposed, crossed, at(10), &mut report);

        assert!(
            report.crosses.is_empty(),
            "a fully offsetting net was crossed, so the cap arithmetic has \
             changed and the comment explaining it is now wrong"
        );
        assert!(
            report
                .refusals
                .iter()
                .any(|(gate, _)| gate == "internal_cross_cap"),
            "the full offset was neither crossed nor refused by the cap: {:?}",
            report.refusals
        );
        Ok(())
    }

    #[test]
    fn a_book_with_no_mid_refuses_the_cross_rather_than_pricing_it_from_the_intent() -> Result<()> {
        // The fallback §27.1 forbids is exactly the one a careless
        // implementation reaches for when the book is silent. There is no
        // price neither side chose available, so there is no cross.
        let config = CellConfig::new(CELL, "europe-west2").with_venue(venue());
        let features = FeatureEngine::new(MarketState::default(), qip_core::Duration::from_secs(5));
        let mut cell = Cell::new(config, features)?;
        cell.track(VenueState::aggregated(object(), venue(), VenueStatus::Open));
        // Premise: this book genuinely serves no mid, so the refusal below is
        // about the price and not about something else.
        assert!(
            cell.liquidity()
                .get(&venue(), &object())
                .and_then(|state| state.mid())
                .is_none(),
            "the fixture book serves a mid, so nothing would be refused"
        );

        let mut report = WorkReport::default();
        let net_intent = offsetting_net(Decimal::parse("12345").expect("a decimal literal"));
        let crossed = cell.cross_internally(&net_intent, at(10), &mut report);
        cell.settle_cross(&net_intent, crossed, at(10), &mut report);

        assert!(report.crosses.is_empty(), "a cross was priced with no mid");
        assert!(
            report
                .refusals
                .iter()
                .any(|(gate, _)| gate == "internal_cross_price"),
            "the refusal did not name the pricing gate: {:?}",
            report.refusals
        );
        Ok(())
    }

    #[test]
    fn a_cross_whose_seller_leg_would_overflow_books_nothing_on_either_side() -> Result<()> {
        // The seller's cash sits at the top of the representable range, so
        // receiving the notional overflows. The buyer's leg is fine on its
        // own — and that is the trap: booked leg by leg, the buyer would be
        // long a lot and short the cash before the seller's overflow was
        // found, and the two books would no longer sum to zero. Neither side
        // may move, and the cell must halt on the disagreement.
        let mut cell = cell_with_book()?;
        let alpha = StrategyId::new("alpha");
        let beta = StrategyId::new("beta");
        cell.strategy_cash
            .insert(beta.as_str().to_string(), Decimal::MAX);
        let cross = InternalCross {
            object_id: object(),
            venue: venue(),
            quantity: Decimal::parse("10").expect("a decimal literal"),
            price: price(),
            bought: vec![alpha.clone()],
            sold: vec![beta.clone()],
        };
        // Premise: the seller's leg really cannot be represented, and the
        // buyer's really can — so what follows is about atomicity and not
        // about a cross that would have failed on the first leg.
        let notional = cross
            .quantity
            .checked_mul(cross.price)
            .expect("the fixture notional is representable");
        assert!(
            Decimal::MAX.checked_add(notional).is_none(),
            "the premise failed: the seller's cash does not overflow"
        );
        assert!(
            Decimal::ZERO.checked_add(cross.quantity).is_some()
                && Decimal::ZERO.checked_add(-notional).is_some(),
            "the premise failed: the buyer's leg overflows on its own"
        );

        cell.book_cross(&cross, at(10));

        assert!(
            cell.strategy_position(&alpha, &venue(), &object())
                .is_zero(),
            "the buyer's lot was booked although the seller's leg could not be"
        );
        assert!(
            cell.strategy_cash(&alpha).is_zero(),
            "the buyer's cash was moved although the seller's leg could not be"
        );
        assert!(
            cell.strategy_position(&beta, &venue(), &object()).is_zero(),
            "the seller's lot was moved on a leg that overflowed"
        );
        assert_eq!(
            cell.strategy_cash(&beta),
            Decimal::MAX,
            "the seller's cash moved on a leg that overflowed"
        );
        assert!(
            cell.is_halted(),
            "a cross the books could not settle did not halt the cell"
        );
        Ok(())
    }

    // --- the interval (§27.1 "per instrument per interval") -----------------

    fn cell_with_interval(interval: CrossingInterval) -> Result<Cell> {
        let config = CellConfig::new(CELL, "europe-west2")
            .with_venue(venue())
            .with_crossing_interval(interval)?;
        let features = FeatureEngine::new(MarketState::default(), qip_core::Duration::from_secs(5));
        let mut cell = Cell::new(config, features)?;
        cell.track(book());
        Ok(cell)
    }

    fn price() -> Decimal {
        Decimal::parse("100").expect("a decimal literal")
    }

    /// Run one net through the crossing seam as `place_net` would, on a
    /// fresh pass, and return what was crossed.
    fn judge(cell: &mut Cell, net_intent: &NetIntent, now: Timestamp) -> WorkReport {
        cell.pass = cell.pass.saturating_add(1);
        let mut report = WorkReport::default();
        let crossed = cell.cross_internally(net_intent, now, &mut report);
        cell.settle_cross(net_intent, crossed, now, &mut report);
        report
    }

    fn refused_under(report: &WorkReport, gate: &str) -> bool {
        report.refusals.iter().any(|(g, _)| g == gate)
    }

    #[test]
    fn with_an_interval_two_strategies_cancelling_completely_inside_a_larger_window_both_fill_at_the_mid()
    -> Result<()> {
        // §27.1's flagship case, reachable once the cap is measured per
        // interval. Pass one is a one-sided 400 on the instrument; pass two
        // is a 100 against a 100. Over the two-pass window that is 100
        // crossed of 600 gross — a sixth, well under two fifths — so the
        // full cancellation crosses, at the mid, and both sides are named.
        let mut cell = cell_with_interval(CrossingInterval::Passes(3))?;
        let mid = cell
            .liquidity()
            .get(&venue(), &object())
            .and_then(|state| state.mid())
            .expect("the fixture book serves a mid");

        let one_sided = netted(&[("alpha", "400")], price());
        let first = judge(&mut cell, &one_sided, at(10));
        assert!(
            first.crosses.is_empty(),
            "a one-sided net has nothing to cross"
        );

        let opposed = netted(&[("alpha", "100"), ("beta", "-100")], price());
        // Premise: the sides really cancel, and without the interval this
        // very net is refused — the default the cited test holds — so what
        // admits it below is the window and nothing else.
        assert!(
            opposed.net_size.is_zero(),
            "the premise needs a total offset"
        );
        let mut per_net = cell_with_book()?;
        let refused = judge(&mut per_net, &opposed, at(11));
        assert!(
            refused.crosses.is_empty() && refused_under(&refused, "internal_cross_cap"),
            "the premise failed: the per-net default admitted a full cancellation"
        );

        let second = judge(&mut cell, &opposed, at(11));
        assert_eq!(
            second.crosses.len(),
            1,
            "the full cancellation was not crossed inside the window: {:?}",
            second.refusals
        );
        let cross = &second.crosses[0];
        assert_eq!(
            cross.quantity,
            Decimal::parse("100").expect("a decimal literal")
        );
        assert_eq!(cross.price, mid, "the cross was not priced at the mid");
        assert_eq!(cross.bought, vec![StrategyId::new("alpha")]);
        assert_eq!(cross.sold, vec![StrategyId::new("beta")]);
        Ok(())
    }

    #[test]
    fn a_run_of_full_cancellations_is_refused_once_it_is_two_fifths_of_the_window() -> Result<()> {
        // The persistent internal market the cap exists to prevent, built
        // one pass at a time. Passes one to three: 100 against 100 each. The
        // first is refused (100 of 200), the second admitted (100 of 400),
        // the third admitted (200 of 600). The fourth sees passes two to
        // four only — 300 crossed of 600 gross, half — and is refused. If
        // pass one were still counted the fourth would read 300 of 800 and
        // pass, so this also holds that a `Passes` window forgets.
        let mut cell = cell_with_interval(CrossingInterval::Passes(3))?;
        let opposed = netted(&[("alpha", "100"), ("beta", "-100")], price());
        let outcomes: Vec<bool> = (1..=4)
            .map(|pass| {
                let report = judge(&mut cell, &opposed, at(pass));
                !report.crosses.is_empty()
            })
            .collect();
        assert_eq!(
            outcomes,
            vec![false, true, true, false],
            "crossed-per-pass did not follow the rolling cap"
        );
        Ok(())
    }

    #[test]
    fn a_span_interval_forgets_nets_older_than_the_span() -> Result<()> {
        let mut cell =
            cell_with_interval(CrossingInterval::Span(qip_core::Duration::from_secs(10)))?;
        let one_sided = netted(&[("alpha", "400")], price());
        let opposed = netted(&[("alpha", "100"), ("beta", "-100")], price());
        judge(&mut cell, &one_sided, at(0));
        // Premise: inside the span the one-sided gross admits the cross.
        let inside = judge(&mut cell, &opposed, at(5));
        assert_eq!(
            inside.crosses.len(),
            1,
            "the premise failed: {:?}",
            inside.refusals
        );
        // Thirty seconds on, both earlier nets are outside the ten-second
        // span and the same cancellation is judged on its own again.
        let outside = judge(&mut cell, &opposed, at(35));
        assert!(
            outside.crosses.is_empty() && refused_under(&outside, "internal_cross_cap"),
            "a net outside the span still counted towards the window: {outside:?}"
        );
        Ok(())
    }

    #[test]
    fn with_no_interval_the_window_is_never_written() -> Result<()> {
        // The default must be today's behaviour exactly, and "exactly"
        // includes allocating nothing: a history that accumulated while
        // unread would be a bound with nothing behind it.
        let mut cell = cell_with_book()?;
        assert!(
            cell.config().crossing_interval.is_none(),
            "the premise is the default"
        );
        judge(&mut cell, &offsetting_net(price()), at(10));
        judge(&mut cell, &offsetting_net(price()), at(11));
        assert!(
            cell.crossing_history.is_empty(),
            "the per-net default kept a crossing history: {:?}",
            cell.crossing_history
        );
        Ok(())
    }

    #[test]
    fn a_window_that_hit_its_bound_refuses_rather_than_measuring_part_of_itself() -> Result<()> {
        // A `Span` long enough to hold everything, fed one net per pass past
        // the bound. The 1,025th net finds a truncated history and is
        // refused under the window gate — not admitted against a gross that
        // is missing its oldest sample, and not silently trimmed.
        let mut cell =
            cell_with_interval(CrossingInterval::Span(qip_core::Duration::from_hours(1)))?;
        let one_sided = netted(&[("alpha", "400")], price());
        for pass in 0..MAX_CROSSING_WINDOW_SAMPLES {
            judge(&mut cell, &one_sided, at(pass as i64));
        }
        let opposed = netted(&[("alpha", "100"), ("beta", "-100")], price());
        let report = judge(&mut cell, &opposed, at(MAX_CROSSING_WINDOW_SAMPLES as i64));
        assert!(
            report.crosses.is_empty(),
            "a cross was admitted against a partial window"
        );
        assert!(
            refused_under(&report, "internal_cross_window"),
            "the refusal did not name the window gate: {:?}",
            report.refusals
        );
        Ok(())
    }

    #[test]
    fn an_empty_or_oversized_interval_is_refused_at_configuration() {
        for interval in [
            CrossingInterval::Passes(0),
            CrossingInterval::Span(qip_core::Duration::ZERO),
            CrossingInterval::Span(qip_core::Duration::from_secs(-1)),
            CrossingInterval::Passes(u32::try_from(MAX_CROSSING_WINDOW_SAMPLES + 1).expect("fits")),
        ] {
            assert!(
                CellConfig::new(CELL, "europe-west2")
                    .with_crossing_interval(interval)
                    .is_err(),
                "{interval:?} was accepted, and would measure the cap against nothing or \
                 against less than it names"
            );
        }
        assert!(
            CellConfig::new(CELL, "europe-west2")
                .with_crossing_interval(CrossingInterval::Passes(1))
                .is_ok(),
            "a one-pass interval is the per-net reading with accounting on, and is valid"
        );
    }
}

#[cfg(test)]
#[allow(clippy::panic_in_result_fn)]
mod polled_halt_tests {
    //! §46.2's second wire, at the seam the node drives: what each reading
    //! of the flag does to the cell, and that no other wire releases it.

    use super::*;
    use qip_feature_dag::engine::FeatureEngine;
    use qip_feature_dag::state::MarketState;

    fn cell() -> Result<Cell> {
        // A venue, because `CellConfig::validate` refuses a cell that has
        // none: these tests are about the halt wire, not about assembly.
        let config = CellConfig::new("london-1", "europe-west2").with_venue(VenueId::new("XLON"));
        let features = FeatureEngine::new(MarketState::default(), qip_core::Duration::from_secs(5));
        Cell::new(config, features)
    }

    fn at(seconds: i64) -> Timestamp {
        Timestamp::from_secs(1_700_000_000).saturating_add(qip_core::Duration::from_secs(seconds))
    }

    #[test]
    fn every_content_the_flag_can_hold_reads_the_way_the_wire_needs() {
        // The two words, the empty file, and everything else. Everything
        // else halts: a flag whose content cannot be understood is a wire
        // whose state is unknown, and a kill switch fails engaged.
        assert_eq!(
            PolledHalt::from_content(b""),
            PolledHalt::Engaged("the flag is present".to_string())
        );
        assert_eq!(
            PolledHalt::from_content(b"engaged\n"),
            PolledHalt::Engaged("the flag is present".to_string())
        );
        assert_eq!(
            PolledHalt::from_content(b"engaged: drill 7\n"),
            PolledHalt::Engaged("drill 7".to_string())
        );
        assert_eq!(
            PolledHalt::from_content(b" released \n"),
            PolledHalt::Released
        );
        assert!(
            matches!(
                PolledHalt::from_content(b"release"),
                PolledHalt::Unreadable(_)
            ),
            "a near-miss of the release word must not release"
        );
        assert!(matches!(
            PolledHalt::from_content(b"\xff\xfe"),
            PolledHalt::Unreadable(_)
        ));
        let oversized = vec![b'e'; PolledHalt::MAX_CONTENT_BYTES + 1];
        assert!(matches!(
            PolledHalt::from_content(&oversized),
            PolledHalt::Unreadable(_)
        ));
        for (reading, halts) in [
            (PolledHalt::Absent, false),
            (PolledHalt::Released, false),
            (PolledHalt::Engaged("x".to_string()), true),
            (PolledHalt::Unreadable("x".to_string()), true),
        ] {
            assert_eq!(reading.halts(), halts, "{reading:?}");
        }
    }

    #[test]
    fn an_unreadable_flag_halts_and_an_absent_one_releases_and_the_chain_says_which() -> Result<()>
    {
        let mut cell = cell()?;
        assert!(!cell.is_halted(), "the premise is a running cell");

        cell.apply_polled_halt(
            PolledHalt::Unreadable("permission denied".to_string()),
            at(1),
        );
        assert!(cell.is_halted(), "an unreadable flag did not halt the cell");
        assert!(
            cell.polled_halt()
                .is_some_and(|reason| reason.contains("permission denied")),
            "the halt does not carry the read failure: {:?}",
            cell.polled_halt()
        );
        // Re-reading the same state is one halt, not a second chain entry.
        let entries = cell.journal().entries().len();
        cell.apply_polled_halt(PolledHalt::Engaged("still".to_string()), at(2));
        assert_eq!(
            cell.journal().entries().len(),
            entries,
            "a re-read was journaled as a new halt"
        );

        cell.apply_polled_halt(PolledHalt::Absent, at(3));
        assert!(
            !cell.is_halted(),
            "an absent flag did not release the polled halt"
        );
        let last = cell
            .journal()
            .entries()
            .last()
            .expect("the release was journaled");
        assert_eq!(last.decision.kind(), "halt_changed");
        assert!(
            format!("{:?}", last.decision).contains("polled halt flag is absent"),
            "the release entry does not name the wire: {:?}",
            last.decision
        );
        Ok(())
    }

    #[test]
    fn the_polled_wire_and_the_kill_switch_release_each_other_never() -> Result<()> {
        // Two wires that shared a release would share a failure. With both
        // engaged, clearing one leaves the cell exactly as halted, and the
        // chain entry for the polled release says so rather than reading as
        // a resumed cell.
        let mut cell = cell()?;
        cell.apply_polled_halt(PolledHalt::Engaged("drill".to_string()), at(1));
        cell.autonomy_mut()
            .kill_switch_mut()
            .trip_global(at(2), "operator", "drill");
        assert!(cell.is_halted(), "the premise needs both wires engaged");

        cell.apply_polled_halt(PolledHalt::Released, at(3));
        assert!(
            cell.is_halted(),
            "releasing the polled wire released a kill switch it does not own"
        );
        let last = cell.journal().entries().last().expect("journaled");
        assert!(
            matches!(last.decision, Decision::HaltChanged { halted: true, .. }),
            "the polled release entry claims the cell resumed: {:?}",
            last.decision
        );
        Ok(())
    }
}

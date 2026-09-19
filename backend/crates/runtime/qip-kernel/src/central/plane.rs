//! The central plane itself: what the cells are told, and what they report
//! back.
//!
//! Everything here is composition. The plane owns a [`StrategyFactory`], a
//! [`CapitalAllocator`], an [`EnvelopeIssuer`], a [`CompliancePlane`] and an
//! [`AggregateExposure`], and its job is to make the five behave as one thing:
//!
//! * capital is sized by the allocator, bounded by the issuer and recorded by
//!   the approval chain, and only ever for a strategy the factory says stands
//!   at a capital-holding rung;
//! * cell reports feed the exposure aggregate, which answers the question no
//!   cell can — [`AggregateExposure::crowded`] — and turns a breach into
//!   [`RecallOrder`]s;
//! * a reconciliation break stops that cell and nothing else, because a cell
//!   whose book does not agree with its venue is a cell whose risk numbers are
//!   fiction, and the rest of the platform's are not.
//!
//! Nothing here reads a clock or draws a random number. Every entry point takes
//! the [`Timestamp`] it is reasoning about, and the incident ids are a counter
//! rather than a generated id, so a replay of the same reports produces the
//! same halts and the same recalls.

use super::darkness::{LastHeard, RegionDarkness, RegionTransition};
use super::dna::StrategyDna;
use super::factory::StrategyFactory;
use super::horizon::{HorizonArming, HorizonPolicy, PoolReconciler, UnarmedHorizons};
use super::learning::CellOutcome;
use super::realised::{RealisedCalendar, RealisedSeries};
use super::regions::{GrantManifests, RegionMembership, RegionShares, partition};
use super::whitelist::{ArbitragePolicy, WhitelistIssue, WhitelistOutcome};
use crate::venue_review::{FeasibilityRefusal, FeasibilitySeam, RefusalStanding};
use qip_capital::allocation::{
    Allocation, AllocationLimits, AllocationPlan, CapitalAllocator, DrawdownSchedule,
    StrategyProposal,
};
use qip_capital::envelope::{EnvelopeIssuer, EnvelopeTerms, MAXIMUM_ENVELOPE_VALIDITY};
use qip_capital::exposure::{
    AggregateExposure, CellPosition, ConcentrationFinding, ConcentrationLimits, CrowdedPosition,
};
use qip_capital::recall::{RecallOrder, RecallReason, RecallRegister};
use qip_compliance::approval::{ApprovedCapital, CapitalRequest, OperatorCredential};
use qip_compliance::incident::{HaltScope, Incident, ResponsePolicy};
use qip_compliance::plane::{CompliancePlane, ComplianceReport};
use qip_compliance::signing::SigningKey;
use qip_contracts::feasibility::{EDGE_GATES, GATE_WITHDRAWN_VENUE, is_withdrawal_echo};
use qip_contracts::governance::{Approval, Severity};
use qip_contracts::message::BookSide;
use qip_contracts::policy::{CycleWhitelist, FeasibilityConstraints};
use qip_contracts::signal::StrategyId;
use qip_contracts::wire::{CrossRecord, FillRecord};
use qip_contracts::{CapitalEnvelope, Utilisation};
use qip_core::error::{Error, Result};
use qip_core::{Decimal, Duration, Timestamp};
use qip_learning_engine::attribution::{Attribution, Attributor, PositionPeriod};
use qip_lifecycle::horizon::HorizonAssurance;
use qip_mesh::delta::{DeltaOrder, DeltaRefusal};
use qip_observability::metrics::{Metrics, labels, names};
use qip_risk_engine::autonomy::KillSwitch;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::Arc;

/// The key id every signature the central plane makes is recorded under.
///
/// Carried into the envelope issuer, the approval chain and the artifact store
/// so that when asymmetric signing arrives, existing records say which key they
/// were made under. See `qip_compliance::signing` for what this scheme is not.
const CENTRAL_KEY_ID: &str = "central-plane-key";

/// The `venue` label a carried refusal is counted under when it names a
/// venue neither the arbitrage policy nor any live grant permits. One
/// literal, so a cell cannot mint a series by naming a venue.
pub const UNKNOWN_VENUE: &str = "unknown";

/// The `constraint` label a carried refusal is counted under when its gate
/// is outside `qip_contracts::feasibility::EDGE_GATES`. One literal, for the
/// same reason.
pub const OTHER_CONSTRAINT: &str = "other";

/// The subject an [`Approval`] must name to authorise capital for a strategy
/// at a cell.
///
/// Built by asking [`CapitalRequest::subject`] rather than by formatting the
/// same string a second time: an approval whose subject does not match the
/// request is refused, and two independent formatters would eventually
/// disagree about a separator and make every grant fail for a reason nobody
/// could see.
pub fn capital_subject(strategy: &StrategyId, cell: &str) -> String {
    CapitalRequest {
        strategy: strategy.clone(),
        cell: cell.to_string(),
        gross_limit: Decimal::ZERO,
        order_limit: Decimal::ZERO,
        loss_limit: Decimal::ZERO,
        venues: Vec::new(),
        expires_at: Timestamp::from_secs(0),
        requested_by: String::new(),
    }
    .subject()
}

/// How the central plane is sized and bounded.
///
/// Deliberately holds no key material. A configuration that carried a secret
/// would print it the first time anything derived `Debug` on the struct that
/// holds it, and [`crate::PlatformConfig`] derives `Debug`. The signing secret
/// is passed to [`CentralPlane::new`] instead.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CentralConfig {
    /// The whole risk budget across every cell.
    pub total_budget: Decimal,
    pub per_strategy: Decimal,
    pub per_cell: Decimal,
    pub per_venue: Decimal,
    /// How allocation shrinks as a drawdown deepens.
    pub drawdown: DrawdownSchedule,
    /// How long an issued envelope lives. Capped by
    /// [`MAXIMUM_ENVELOPE_VALIDITY`], which is the only revocation mechanism
    /// there is for a cell nobody can reach.
    pub envelope_validity: Duration,
    /// How long a cell has to acknowledge a recall before it is treated as
    /// unreachable. Must be positive: the recall register refuses a recall
    /// with no window, and [`CentralPlane::new`] refuses the configuration
    /// first, because a refusal that surfaced only when a recall was issued
    /// would surface mid-ingestion, after a cell had already been halted.
    pub recall_acknowledgement: Duration,
    /// Above this gross limit the approval chain demands two different humans.
    pub dual_approval_threshold: Decimal,
    /// Cells that must independently hold one name for it to count as crowded.
    pub minimum_cells_for_crowding: usize,
    /// Treat every incident as at least this severe.
    pub response_floor: Severity,
    /// What the arbitrage desk may price, or `None` for nothing.
    ///
    /// The source of the shipping payload's cycle whitelist (slot 8), stated
    /// by an operator because the centre holds no pair list and no fee
    /// schedule of its own — see [`super::whitelist`]. `#[serde(default)]`
    /// for the same reason [`crate::PlatformConfig::central`] is: a stored
    /// configuration written before the desk had a producer still reads,
    /// and reads as the fail-closed empty whitelist.
    #[serde(default)]
    pub arbitrage: Option<ArbitragePolicy>,
    /// How [`Self::total_budget`] divides across the four blueprint §23.4
    /// horizons, and which horizon each strategy sits at — or `None` for no
    /// pool reconciliation at all.
    ///
    /// Stated by an operator for the same reason [`Self::arbitrage`] is: the
    /// centre measures neither a capital split nor a strategy's holding
    /// horizon, and inferring either would be asserting an attribute nobody
    /// measured. `#[serde(default)]` so a configuration written before this
    /// field existed still reads, and reads as no reconciliation — which is
    /// what every deployment does today, and is why
    /// [`CentralPlane::arm_horizons`] says so on the cycle rather than
    /// silently arming nothing.
    #[serde(default)]
    pub horizons: Option<HorizonPolicy>,
    /// How long every cell of a region may be silent before the centre
    /// derives the region dark (ADR 0079), or `None` for no derivation.
    ///
    /// **Stated by an operator, and deliberately given no default.** There is
    /// no measurement in this tree to pick the number from: too short refuses
    /// healthy regions their grants, too long delays the refusal past the
    /// point it protects anything, and a default would be a number nobody
    /// chose sitting where a region's capital is decided. `None` means the
    /// derivation is off, and [`CentralPlane::region_dark_after`] says so to
    /// every reader rather than reporting "no region is dark" as if it had
    /// looked. `#[serde(default)]` so a configuration written before the
    /// field reads — as off, which is what it was.
    ///
    /// [`CentralPlane::new`] refuses zero, because a region silent for no
    /// time at all is every region between two reports, and refuses a window
    /// above [`MAXIMUM_ENVELOPE_VALIDITY`], because a darkness the centre
    /// would notice only after every envelope in the region had already
    /// expired is a control that fires after the fact it exists to catch.
    #[serde(default)]
    pub region_dark_after: Option<Duration>,
}

impl Default for CentralConfig {
    fn default() -> Self {
        Self {
            total_budget: Decimal::from_int(10_000_000),
            per_strategy: Decimal::from_int(2_000_000),
            per_cell: Decimal::from_int(4_000_000),
            per_venue: Decimal::from_int(6_000_000),
            drawdown: DrawdownSchedule::default(),
            // Two thirds of the twelve-hour ceiling: long enough that a cell
            // survives an afternoon of central-plane maintenance, short enough
            // that a grant issued this morning is not still live tonight.
            envelope_validity: Duration::from_hours(8),
            recall_acknowledgement: Duration::from_mins(5),
            // Zero means every grant needs two names. Everything issued here
            // can lose money, and `GateStage::requires_human_approval` already
            // says every rung that can lose money needs two; a threshold above
            // zero would be this plane disagreeing with the ladder.
            dual_approval_threshold: Decimal::ZERO,
            minimum_cells_for_crowding: 3,
            response_floor: Severity::Observation,
            arbitrage: None,
            horizons: None,
            region_dark_after: None,
        }
    }
}

/// A position a cell reports that its venue or custodian does not confirm.
///
/// The one finding that stops a cell on its own. Everything else the central
/// plane sees is a number it can reason about; a break means the cell's own
/// account of what it holds is wrong, and every limit it checks locally is
/// therefore being checked against fiction.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ReconciliationBreak {
    pub instrument: String,
    /// What the cell believes it holds.
    pub cell_quantity: Decimal,
    /// What the venue, custodian or clearer says it holds.
    pub external_quantity: Decimal,
    pub detail: String,
    /// Which record the break was found in. Defaulted so a break sealed
    /// before the centre kept its own record of sent orders replays as what
    /// it was: a disagreement between the cell's book and the venue's.
    #[serde(default)]
    pub origin: BreakOrigin,
}

/// Where a reconciliation break was found.
///
/// A break's direction is read from the sign of its quantity gap, which is
/// the right reading for a book that disagrees with a venue and the wrong
/// one for a fill the centre cannot match to any order it saw sent: there
/// the "cell quantity" is nothing, so the sign would file it under
/// `venue_over_cell` and an operator reading the series would go looking
/// for a custody gap that does not exist. The origin says which record to
/// open, and the direction is taken from it first.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum BreakOrigin {
    /// The cell's position book against the venue's account of it.
    #[default]
    Book,
    /// A fill the cell reported on an order the centre never saw sent, or
    /// beyond the quantity it saw sent. Found by [`CentralPlane::ingest`]
    /// itself while settling, not shipped by the cell.
    UnsentFill,
}

/// Which way a reconciliation break points.
///
/// The bounded shape of a break, for a series that must not grow with the
/// instrument list: a break is the cell holding more than the venue confirms,
/// the venue confirming more than the cell holds, or — when the quantities
/// agree — a discrepancy that lives only in the detail. Three arms and no
/// free text, so the label set is closed by construction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BreakDirection {
    CellOverVenue,
    VenueOverCell,
    DetailOnly,
    /// A fill on an order the centre never saw sent, or beyond what was
    /// sent. The fourth arm: a venue claim with no order of the platform's
    /// behind it, which is neither book over venue nor venue over book.
    UnsentFill,
}

impl BreakDirection {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::CellOverVenue => "cell_over_venue",
            Self::VenueOverCell => "venue_over_cell",
            Self::DetailOnly => "detail_only",
            Self::UnsentFill => "unsent_fill",
        }
    }
}

impl ReconciliationBreak {
    /// Signed gap between the two books.
    pub fn difference(&self) -> Decimal {
        self.cell_quantity - self.external_quantity
    }

    /// The bounded shape of this break: its origin where the origin is
    /// specific, and otherwise the sign of [`Self::difference`].
    pub fn direction(&self) -> BreakDirection {
        if self.origin == BreakOrigin::UnsentFill {
            return BreakDirection::UnsentFill;
        }
        let difference = self.difference();
        if difference.is_positive() {
            BreakDirection::CellOverVenue
        } else if difference.is_negative() {
            BreakDirection::VenueOverCell
        } else {
            BreakDirection::DetailOnly
        }
    }

    pub fn describe(&self) -> String {
        format!(
            "{}: the cell holds {} and the venue confirms {} (difference {}) — {}",
            self.instrument,
            self.cell_quantity,
            self.external_quantity,
            self.difference(),
            self.detail
        )
    }
}

/// What one cell tells the centre.
///
/// The positions are the whole of that cell's book rather than a delta: a
/// central plane that accumulated deltas would drift from the cell it is
/// describing at exactly the moment a message was lost, and the aggregate
/// exposure is the one number that has to be right during an incident.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CellReport {
    pub cell: String,
    /// The region the cell reports itself in — the `region` its delta has
    /// carried since the delta existed, filled by `qip_api::mesh::report_from`
    /// (ADR 0079 decision two). The only fact the centre derives a region's
    /// darkness from, and a self-assertion on a wire that authenticates
    /// nobody: it can move a cell's silence between regions and nothing
    /// else, because every consequence of darkness is a refusal. Defaulted,
    /// so a report journaled before the field replays — as a cell in no
    /// region, which contributes to no derivation.
    #[serde(default)]
    pub region: String,
    pub at: Timestamp,
    pub positions: Vec<CellPosition>,
    /// What each strategy has committed against its envelope.
    pub utilisation: Vec<(StrategyId, Utilisation)>,
    pub reconciliation_breaks: Vec<ReconciliationBreak>,
    /// Orders the cell *sent* since its previous report — accepted by the
    /// venue, not filled — each carrying the contributor vector the cell
    /// netted it from. Incremental, unlike the positions above. The centre
    /// registers each as sent, against which later fills are matched, and
    /// books nothing from it: for one slice it attributed, charged and
    /// settled every one of these as a fill, for orders still resting or
    /// already expired. Defaulted so a report written before the field
    /// replays.
    #[serde(default)]
    pub orders: Vec<DeltaOrder>,
    /// Fills the venue confirmed since the previous report, each with the
    /// cell's own attribution. The only thing the centre bills, attributes,
    /// charges into the risk aggregate and moves positions from. Defaulted
    /// so a report written before the field replays — as having confirmed
    /// nothing, which is what it said.
    #[serde(default)]
    pub fills: Vec<FillRecord>,
    /// Internal crosses the cell booked since its previous report (§27.1).
    /// Incremental for the same reason.
    #[serde(default)]
    pub crosses: Vec<CrossRecord>,
    /// Every gate that refused at the cell since its previous report, as the
    /// delta carried them. The centre reads only the feasibility ones that
    /// name a venue it knows, into the window blueprint §12.3's fourth row
    /// is judged over; the rest were counted at the cell and are not
    /// re-counted here. Defaulted so a report written before the field
    /// replays as having carried no refusal, which is what it did.
    #[serde(default)]
    pub refusals: Vec<DeltaRefusal>,
}

impl CellReport {
    pub fn new(cell: impl Into<String>, at: Timestamp) -> Self {
        Self {
            cell: cell.into(),
            region: String::new(),
            at,
            positions: Vec::new(),
            utilisation: Vec::new(),
            reconciliation_breaks: Vec::new(),
            orders: Vec::new(),
            fills: Vec::new(),
            crosses: Vec::new(),
            refusals: Vec::new(),
        }
    }

    pub fn with_refusals(mut self, refusals: Vec<DeltaRefusal>) -> Self {
        self.refusals = refusals;
        self
    }

    /// The region the cell reports itself in. See the field.
    pub fn with_region(mut self, region: impl Into<String>) -> Self {
        self.region = region.into();
        self
    }

    pub fn with_positions(mut self, positions: Vec<CellPosition>) -> Self {
        self.positions = positions;
        self
    }

    pub fn with_utilisation(mut self, utilisation: Vec<(StrategyId, Utilisation)>) -> Self {
        self.utilisation = utilisation;
        self
    }

    pub fn with_break(mut self, reconciliation_break: ReconciliationBreak) -> Self {
        self.reconciliation_breaks.push(reconciliation_break);
        self
    }

    pub fn with_orders(mut self, orders: Vec<DeltaOrder>) -> Self {
        self.orders = orders;
        self
    }

    pub fn with_fills(mut self, fills: Vec<FillRecord>) -> Self {
        self.fills = fills;
        self
    }

    pub fn with_crosses(mut self, crosses: Vec<CrossRecord>) -> Self {
        self.crosses = crosses;
        self
    }

    pub fn reconciles(&self) -> bool {
        self.reconciliation_breaks.is_empty()
    }
}

/// What ingesting one cell report produced.
#[derive(Clone, Debug, PartialEq)]
pub struct CellIngestion {
    pub cell: String,
    pub positions_absorbed: usize,
    /// What the incident response halted, `None` where the report reconciled.
    pub halted: Option<HaltScope>,
    /// Buckets over their share of gross, on any axis.
    pub concentrations: Vec<ConcentrationFinding>,
    /// Names several cells hold at once — the question no cell can answer.
    pub crowded: Vec<CrowdedPosition>,
    /// Recalls issued because of a concentration finding.
    pub recalls: Vec<RecallOrder>,
    /// What the report's orders and crosses did to the strategy books.
    pub settlement: Settlement,
    /// The report's feasibility refusals the centre admitted: each names a
    /// feasibility gate and a venue the configuration or a live grant
    /// permits, stamped with the report's instant. The platform puts these
    /// in the window a venue is withdrawn on.
    pub feasibility_refusals: Vec<FeasibilityRefusal>,
    /// The report's venue-bearing refusals the centre could *not* attribute
    /// — a gate outside the feasibility vocabulary, or a venue no
    /// configuration and no grant names — as the `(venue, constraint)`
    /// label pair each is counted under, with `unknown` and `other` in
    /// place of whichever half could not be established. Counted, never
    /// admitted: a window entry under a cause nobody established would be
    /// a withdrawal nobody could explain.
    pub feasibility_refusals_unattributed: Vec<(String, String)>,
    /// The report's *repeat* refusals: attributed in full — real venue,
    /// declared gate — but naming a venue and gate this report has already
    /// seated. Carried as the `(venue, constraint)` label pair each is
    /// counted under, so the series still counts every refusal the cell made,
    /// and given no window seat.
    ///
    /// **Why the first of a pair takes a seat and the rest do not.** One
    /// report is one cell's observation at one instant. The number of times
    /// a gate appears in it is a fact about how many cycles that cell's desk
    /// happened to enumerate — not about the venue. Admitted whole to a
    /// 256-entry rate window, one report is the whole window: a probe
    /// withdrew a venue for the entire platform using two messages, thirty
    /// refusals in one and a single refusal from a second cell name to clear
    /// `venue_review::VENUE_WITHDRAWAL_MIN_CELLS`, which counts distinct
    /// cells and so means nothing unless one report is bounded. Admitted at
    /// nothing, a withdrawn venue would leave the denominator every other
    /// venue's share is measured against and the runner-up would become a
    /// cluster of the remainder. So one seat per venue per gate per report,
    /// which bounds a report's seats by the gate vocabulary times the
    /// configured venue list and by nothing a sender chooses.
    ///
    /// This field was `feasibility_refusals_repeated` and held only repeats
    /// under `feasibility::GATE_WITHDRAWN_VENUE`; the rule that justified it
    /// was never particular to that gate, and the eight gates it did not
    /// cover were the ones an attacker would have used.
    pub feasibility_refusals_repeated: Vec<(String, String)>,
}

/// What settling one report's interval to the strategy books produced.
///
/// The centre's half of blueprint §43.4's chain: fill → contributor vector →
/// strategy, pro rata. Every share booked here is a line in the attribution,
/// and the attribution is exact — [`Attribution::residual`] is zero or the
/// settlement is refused and counted, never absorbed.
///
/// Only the report's `fills` are settled. Its `orders` are registered as
/// sent and counted under [`Self::orders_sent`], and a report carrying
/// orders and no fills settles nothing — that is a cell with resting
/// orders, not a break.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Settlement {
    /// Contributor shares booked, across every fill settled.
    pub fills_attributed: usize,
    /// Per strategy, the execution cost of this settlement's fills, as a
    /// running quantity-weighted sum. Read through
    /// [`Settlement::cost_bps_by_strategy`], which is where the meaning is
    /// stated; the accumulator is public only because the struct is.
    pub cost: BTreeMap<String, CostAccrual>,
    /// Venue fills booked to the strategy books and charged to the aggregate.
    pub fills_settled: usize,
    /// Orders registered as sent — accepted by the venue, not filled — and
    /// billed nothing. A resting order the venue later fills is billed then,
    /// from the fill, and never from this count.
    pub orders_sent: usize,
    pub crosses_settled: usize,
    /// Orders and crosses the centre would not settle, each with why. A
    /// refusal here is a report that carried something the books cannot
    /// take without guessing — a cross naming two buyers and no sizes, an
    /// order whose contributors are all on the other side.
    pub refused: Vec<String>,
    /// The exact decomposition of everything settled, or `None` where the
    /// report carried nothing to settle.
    pub attribution: Option<Attribution>,
    /// Every venue fill the settlement booked, one entry per fill settled,
    /// in report order — what the platform charges into its risk aggregate.
    ///
    /// Recorded at the line that counts the fill settled, so what the
    /// aggregate is charged and what the strategy books absorbed are one
    /// list rather than two readings of the report that could disagree.
    /// Crosses are deliberately absent: a cross moves one strategy's lot up
    /// and another's down inside the same cell, so the book's exposure is
    /// unchanged and charging it would be a gross that nobody holds.
    pub absorbed: Vec<AbsorbedFill>,
    /// Breaks the settlement itself found: fills on orders the centre never
    /// saw sent, or beyond what it saw sent. Each halts the cell exactly as
    /// a break the cell shipped does; they are listed here so the caller
    /// can see which fill was refused and why, and nothing in `absorbed`
    /// or the books carries them.
    pub breaks: Vec<ReconciliationBreak>,
}

/// One venue fill the centre absorbed from a cell's report.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AbsorbedFill {
    pub object_id: String,
    /// Positive for a buy, negative for a sell — the sign
    /// [`RiskAggregates::apply_fill`] takes.
    ///
    /// [`RiskAggregates::apply_fill`]: qip_risk::aggregate::RiskAggregates::apply_fill
    pub signed_notional: Decimal,
    /// Who executed it, as the cell's own [`FillRecord`] named the venue.
    ///
    /// Carried so `Platform::charge_cell_fills` can charge the fill to the
    /// same [`qip_risk::limits::COUNTERPARTY_AXIS`] bucket a desk fill is
    /// charged to. It was absent, and the consequence was not that the cap
    /// was approximate: a book that traded through cells as well as the desk
    /// held counterparty exposure the running balance did not carry, so
    /// `LimitKind::MaxCounterpartyExposure` read low on it and admitted
    /// orders it existed to refuse. A limit that reads low is a defect, not
    /// a conservative reading.
    ///
    /// Required rather than `#[serde(default)]`: a `Settlement` is an
    /// in-process return value and nothing replays one, and an empty default
    /// would file a real fill under a counterparty named by nobody — which
    /// is the same failure wearing a name.
    pub venue: String,
}

/// One strategy's execution cost over a settlement's fills, accumulated so a
/// mean can be taken over the quantity that produced it.
///
/// Two fields rather than a running mean because the fills of one settlement
/// differ in size by orders of magnitude, and a mean of means would weight a
/// one-lot fill the same as the block beside it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CostAccrual {
    /// Sum of `cost_bps * quantity` over the fills booked to the strategy.
    pub weighted: f64,
    /// Sum of `quantity` over those same fills, the divisor of the mean.
    pub quantity: f64,
}

impl Settlement {
    /// Per strategy, the quantity-weighted mean execution cost of this
    /// settlement's fills, in basis points, signed so that paying away is
    /// positive.
    ///
    /// **What this measures, precisely.** Each fill is costed against the
    /// price the platform *sent the order at* — under `PricingPolicy::RestAtMid`
    /// the mid the cell rested at — and not against a decision-time arrival
    /// mid, which the wire does not carry. So it is slippage against the
    /// platform's own asking price, not full implementation shortfall, and a
    /// marketable order sent through the spread shows the part of its cost
    /// that landed beyond its own limit rather than all of it. That is the
    /// honest bound of what two prices on the wire can support.
    ///
    /// **Why it exists.** `KillCondition::CostOverrun` compares this against
    /// a modelled figure plus a tolerance. Until this was measured the centre
    /// supplied the literal `0.0`, so the comparison was `0.0 > modelled +
    /// tolerance` — false for every non-negative modelled cost a person would
    /// write. The condition shipped in kill-condition sets, read as
    /// protection, and could not fire. This is the same defect
    /// `MaxExpectedShortfall` had, and the rule it broke is the one that says
    /// a limit that cannot fire is a defect rather than a spare part.
    ///
    /// A strategy with no filled quantity is absent rather than zero: zero is
    /// a cost that was measured and found to be nil, and a strategy that
    /// filled nothing has no cost to report.
    pub fn cost_bps_by_strategy(&self) -> BTreeMap<String, f64> {
        self.cost
            .iter()
            .filter(|(_, accrual)| accrual.quantity > 0.0)
            .map(|(strategy, accrual)| (strategy.clone(), accrual.weighted / accrual.quantity))
            .collect()
    }

    /// The strategy-level P&L the settlement realised, by strategy id.
    pub fn by_strategy(&self) -> BTreeMap<String, Decimal> {
        self.attribution
            .as_ref()
            .map(Attribution::by_hypothesis)
            .unwrap_or_default()
    }
}

/// One strategy's holding in one instrument at one cell, as the centre's
/// attribution has moved it.
///
/// Average-cost, signed. The book the contributor vector lands on: a fill
/// attributed pro rata moves each contributor's lot by its share, and an
/// internal cross moves the buyer's up and the seller's down at the mid, so
/// the two strategies that disagreed each hold what they intended.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct StrategyLot {
    /// Negative is short.
    pub quantity: Decimal,
    /// Zero when flat.
    pub average_price: Decimal,
}

impl StrategyLot {
    /// Apply a signed trade at a price, average-cost.
    ///
    /// Adding in the held direction re-averages; reducing keeps the average
    /// and realises against it; crossing through flat starts the new lot at
    /// the trade price. Returns the lot as it stood before, so the caller can
    /// write the period the attribution grades.
    fn apply(&mut self, signed: Decimal, price: Decimal) -> Self {
        let before = *self;
        let after = before.quantity + signed;
        let average = if after.is_zero() {
            Decimal::ZERO
        } else if before.quantity.is_zero() || before.quantity.signum() == signed.signum() {
            // The one division on this path. A rounding at the ninth place
            // moves the average, never the quantity, and the attribution
            // measures P&L against the average it recorded rather than
            // against an average it re-derives.
            (before.quantity * before.average_price + signed * price) / after
        } else if after.signum() == before.quantity.signum() {
            before.average_price
        } else {
            price
        };
        self.quantity = after;
        self.average_price = average;
        before
    }
}

impl CellIngestion {
    /// Whether this report changed anything an operator needs to look at.
    pub fn is_quiet(&self) -> bool {
        self.halted.is_none()
            && self.concentrations.is_empty()
            && self.recalls.is_empty()
            && self.settlement.refused.is_empty()
    }
}

/// One grant, in both the forms it has to exist in.
///
/// See the module documentation of [`super`] for why there are two signatures
/// over one set of terms, and what a production deployment should do about it.
#[derive(Clone, Debug, PartialEq)]
pub struct IssuedCapital {
    approved: ApprovedCapital,
    envelope: CapitalEnvelope,
    allocation: Allocation,
}

impl IssuedCapital {
    /// The governance record: a value with no public constructor, so holding
    /// one is evidence that a human granted this capital.
    pub fn approved(&self) -> &ApprovedCapital {
        &self.approved
    }

    /// The grant the cell enforces, signed per `qip_capital::envelope`.
    pub fn envelope(&self) -> &CapitalEnvelope {
        &self.envelope
    }

    /// What the allocator gave it, and what stopped it being given more.
    pub fn allocation(&self) -> &Allocation {
        &self.allocation
    }
}

/// The centre: research, approval, allocation and aggregate risk.
#[derive(Debug)]
pub struct CentralPlane {
    factory: StrategyFactory,
    allocator: CapitalAllocator,
    issuer: EnvelopeIssuer,
    compliance: CompliancePlane,
    /// Kept alongside the compliance plane's copy so a DNA can be sealed
    /// without reaching through the governance object for key material.
    key: SigningKey,
    /// Whether the signing secret was reproducible from configuration.
    ///
    /// Recorded at construction rather than inferred later: once a signature
    /// exists the key behind it is indistinguishable, and a deployment that
    /// forgot to supply real material would look exactly like one that did.
    key_is_reproducible: bool,
    config: CentralConfig,
    concentration: ConcentrationLimits,
    /// Where a reconciliation break and the halt it causes are counted, if
    /// whoever composed the plane gave it a registry. Held by the plane rather
    /// than by the platform around it because the count has to happen at the
    /// seam — the instant after the kill switch is tripped — where no later
    /// refusal in the same ingestion can reach it. Optional for the same
    /// reason the ledger's is: a missing registry must not stop a halt.
    metrics: Option<Arc<Metrics>>,

    /// The allocator's input per strategy, updated by the learn edge.
    proposals: BTreeMap<StrategyId, StrategyProposal>,
    /// The last full book each cell reported.
    positions: BTreeMap<String, Vec<CellPosition>>,
    exposure: AggregateExposure,
    utilisation: BTreeMap<(String, StrategyId), Utilisation>,
    envelopes: BTreeMap<(String, StrategyId), CapitalEnvelope>,
    recalls: RecallRegister,
    /// The strategy books: what each strategy holds in each instrument at
    /// each cell, as the contributor vectors have moved them. Keyed by cell,
    /// strategy and instrument, in that order, because a replay that
    /// reorders is not a replay.
    books: BTreeMap<(String, StrategyId, String), StrategyLot>,
    /// Closes every settlement's decomposition, or refuses it.
    attributor: Attributor,
    /// Per cell, the orders it has reported sent and how much of each has
    /// since filled — what a fill is matched against before it is billed.
    /// Bounded per cell; see [`SentOrders`].
    sent: BTreeMap<String, SentOrders>,
    /// Incidents raised here, so their ids are a deterministic counter rather
    /// than a generated id: a replay of the same reports produces the same
    /// incident record.
    incidents_raised: u64,
    /// What each strategy has realised at each cell, session by session, as
    /// the attribution booked it — the series the demotion monitor reads
    /// (see `central::realised`). Kept only for strategies the factory holds
    /// a baseline for, which is what bounds the map: cells times strategies
    /// that have reached pilot, both fixed by decisions rather than by
    /// traffic. Keyed cell then strategy, in that order, because a replay
    /// that reviews strategies in a different order is not a replay.
    realised: BTreeMap<(String, StrategyId), RealisedSeries>,
    /// Venues withdrawn on feasibility evidence, omitted from every cycle
    /// whitelist this plane issues. Written only by the kernel after it has
    /// journaled the decision; read only by [`Self::cycle_whitelist_for`],
    /// to `retain`. A cache of the event log, and a subtraction.
    withdrawn_venues: BTreeSet<String>,
    /// Venues this plane withdrew and two operators have since put back.
    ///
    /// **Why the centre must remember, rather than only forget.** A
    /// reinstatement makes every cell's policy slot 11 stale about this
    /// venue *by construction* — a cell learns on its next policy frame, not
    /// on the signature — so until that frame arrives every intent there
    /// comes back refused under `GATE_WITHDRAWN_VENUE`. Those refusals are
    /// not echoes, because the centre no longer holds the withdrawal, and
    /// admitting them as evidence about the venue withdrew it again on the
    /// next LEARN pass. A probe on this plane re-withdrew a venue seconds
    /// after two signatures put it back, on 256 window entries of which not
    /// one was a genuine refusal: the signatures made the cells stale, the
    /// staleness withdrew the venue, and the venue's return could never have
    /// survived a cycle.
    ///
    /// This set is how [`Self::attribute_refusals`] tells "a cell behind on
    /// a decision the centre made" from "a cell asserting a withdrawal the
    /// centre never made" — the second is the case the security review
    /// admitted as evidence, and it stays evidence.
    ///
    /// Bounded by the venues this plane has withdrawn, so by the configured
    /// venue list; cleared when a venue is withdrawn again, because such a
    /// refusal is an echo once more and an echo is classified on its own
    /// terms.
    reinstated_venues: BTreeSet<String>,
    /// When the centre last heard from each cell, and the region the cell
    /// said it was in — the whole of the input a region's darkness is
    /// derived from (ADR 0079). Written in [`Self::ingest`] before anything
    /// can refuse the report, because a halted cell still spoke; read by
    /// [`Self::dark_regions`] on every call and cached nowhere. Bounded by
    /// the cells that have ever reported.
    last_heard: BTreeMap<String, LastHeard>,
    /// The regions whose darkness the platform has journaled and not yet
    /// journaled the end of. **Not the derivation and read by nothing that
    /// decides**: `dark_regions` recomputes from `last_heard` every time,
    /// and this set exists only so [`Self::region_transitions`] can offer
    /// each change once, in each direction, and the platform can write the
    /// record before the set moves. Empty after a restart, when every region
    /// is unknown rather than dark; a region that was dark across a restart
    /// gets no `region.lit` record when it speaks, and the log carries a
    /// `service.started` between the two instead.
    announced_dark: BTreeSet<String>,
    /// The share bound each cell was last computed while its region was
    /// lit. Read only for a cell whose region is dark, by
    /// [`Self::region_shares`], which hands the cell this number rather
    /// than the plan's current one: a dark region's share is frozen, neither
    /// counted as free nor recomputed, so nothing the allocator does while a
    /// region is silent moves that region's bound (ADR 0079 decision four).
    /// A dark cell with no entry here is withheld a manifest, with the
    /// reason, rather than given a share computed on a dark reading.
    lit_share_bounds: BTreeMap<String, Decimal>,
}

impl CentralPlane {
    /// Wire the plane.
    ///
    /// The secret is passed in rather than configured so it does not travel in
    /// a serialisable, `Debug`-printable configuration. It is shared by the
    /// envelope issuer and the compliance signing key on purpose: they are the
    /// same trust root, and a deployment that rotated one and not the other
    /// would have grants that verify at a cell and not in the audit trail.
    pub fn new(signing_secret: &[u8], config: CentralConfig) -> Result<Self> {
        Self::assemble(signing_secret, config, false)
    }

    /// Assemble with a secret that is reproducible from configuration.
    ///
    /// Used where the platform has no source of real key material — it has no
    /// ambient entropy and must not grow one, because a replay of the same
    /// configuration has to produce the same signatures. Anyone who knows the
    /// seed can mint an envelope, so this is not a production key, and the
    /// plane records that it was assembled this way.
    ///
    /// Separate from [`CentralPlane::new`] rather than a flag on it: the
    /// distinction then lives in the call site a reviewer reads, and choosing
    /// it is a visible act rather than an argument that defaults.
    pub fn with_reproducible_key(signing_secret: &[u8], config: CentralConfig) -> Result<Self> {
        Self::assemble(signing_secret, config, true)
    }

    fn assemble(
        signing_secret: &[u8],
        config: CentralConfig,
        key_is_reproducible: bool,
    ) -> Result<Self> {
        if config.envelope_validity > MAXIMUM_ENVELOPE_VALIDITY {
            return Err(Error::denied(format!(
                "an envelope validity of {:.1} hour(s) is above the {:.1} hour ceiling; expiry \
                 is the only backstop against a cell the central plane cannot reach",
                config.envelope_validity.as_secs_f64() / 3600.0,
                MAXIMUM_ENVELOPE_VALIDITY.as_secs_f64() / 3600.0
            )));
        }
        if config.envelope_validity <= Duration::ZERO {
            return Err(Error::invalid(
                "an envelope with no validity period grants nothing",
            ));
        }
        // Refused here rather than clamped, and here rather than where the
        // recall is issued: the register refuses a non-positive window, and a
        // plane that carried one would discover it inside `ingest`, after a
        // reconciliation break had halted a cell and raised an incident. The
        // error would then propagate out of the one call that was supposed to
        // record the halt. A configuration that cannot issue a recall is a
        // configuration this plane will not start with.
        if config.recall_acknowledgement <= Duration::ZERO {
            return Err(Error::invalid(format!(
                "CentralConfig::recall_acknowledgement is {:.0} second(s); a recall needs a \
                 positive window to be acknowledged in, so set it above zero rather than \
                 leaving every concentration recall to fail at the moment it is issued",
                config.recall_acknowledgement.as_secs_f64()
            )));
        }
        // ADR 0079's window, refused at both ends and never clamped. Zero
        // would derive every region dark between any two reports; a window
        // past the envelope ceiling would notice a region's silence only
        // after every grant in it had already expired on its own, which is a
        // control that fires after the fact it exists to catch. `None` is
        // not refused: it is the derivation switched off, said so by
        // `region_dark_after`, and is what every configuration written
        // before the field means.
        if let Some(window) = config.region_dark_after {
            if window <= Duration::ZERO {
                return Err(Error::invalid(format!(
                    "CentralConfig::region_dark_after is {:.0} second(s); a region cannot be \
                     dark after no silence at all, so state a positive window or omit the \
                     field to leave the derivation off",
                    window.as_secs_f64()
                )));
            }
            if window > MAXIMUM_ENVELOPE_VALIDITY {
                return Err(Error::denied(format!(
                    "CentralConfig::region_dark_after is {:.1} hour(s), above the {:.1} hour \
                     envelope ceiling; a region whose silence is noticed only after every \
                     grant in it has expired is noticed too late to refuse anything, so \
                     state a window at or under the ceiling",
                    window.as_secs_f64() / 3600.0,
                    MAXIMUM_ENVELOPE_VALIDITY.as_secs_f64() / 3600.0
                )));
            }
        }
        // Refused here for the same reason the recall window is: every
        // refusal in `ArbitragePolicy::validate` is one the cell would make
        // when the whitelist arrived, and a plane that carried one would ship
        // a whitelist every few minutes that every cell refused whole, with
        // the reason in a delta stream rather than at start-up.
        if let Some(policy) = &config.arbitrage {
            policy.validate()?;
        }
        // And the §23.4 posture, for the same reason and one more: a split
        // that does not sum to the budget reaches the lifecycle gate as a
        // refusal of every promotion to a capital-holding rung, which is
        // indistinguishable months later from a book that is genuinely
        // over-committed. A configuration that cannot be reconciled against is
        // one this plane will not start with.
        if let Some(policy) = &config.horizons {
            policy.validate(config.total_budget)?;
        }
        let key = SigningKey::from_secret(CENTRAL_KEY_ID, signing_secret)?;
        let limits = AllocationLimits::new(
            config.total_budget,
            config.per_strategy,
            config.per_cell,
            config.per_venue,
        )?;
        Ok(Self {
            factory: StrategyFactory::new(),
            allocator: CapitalAllocator::new(limits, config.drawdown.clone()),
            issuer: EnvelopeIssuer::new(signing_secret.to_vec(), CENTRAL_KEY_ID)?,
            compliance: CompliancePlane::new(
                key.clone(),
                config.dual_approval_threshold,
                ResponsePolicy::with_floor(config.response_floor),
            )?,
            key,
            key_is_reproducible,
            config,
            concentration: ConcentrationLimits::default(),
            metrics: None,
            proposals: BTreeMap::new(),
            positions: BTreeMap::new(),
            exposure: AggregateExposure::default(),
            utilisation: BTreeMap::new(),
            envelopes: BTreeMap::new(),
            recalls: RecallRegister::new(),
            books: BTreeMap::new(),
            attributor: Attributor::new(),
            sent: BTreeMap::new(),
            incidents_raised: 0,
            realised: BTreeMap::new(),
            withdrawn_venues: BTreeSet::new(),
            reinstated_venues: BTreeSet::new(),
            last_heard: BTreeMap::new(),
            announced_dark: BTreeSet::new(),
            lit_share_bounds: BTreeMap::new(),
        })
    }

    pub fn config(&self) -> &CentralConfig {
        &self.config
    }

    /// The cycle whitelist this cell's payload carries at `now` — slot 8 of
    /// blueprint §41.5 — and why.
    ///
    /// Empty, and said so, when no [`CentralConfig::arbitrage`] policy is set
    /// or the desk's strategy holds no live grant at the cell: the grant's
    /// order limit is the funding instrument's start size, and a whitelist
    /// without one is refused by the cell's installer as unsized. An error is
    /// a policy venue the grant does not permit, or a grant that permits no
    /// order — refused at the producer, naming the entry, because the cell
    /// would refuse the whole whitelist for the same reason and say so only
    /// in its delta stream.
    ///
    /// Policy, not an order: this names what the desk may price and how much
    /// it may commit. Whether any cycle is taken is decided at the cell,
    /// against its own books.
    pub fn cycle_whitelist_for(&self, cell: &str, now: Timestamp) -> Result<WhitelistIssue> {
        let empty = |outcome| WhitelistIssue {
            cell: cell.to_string(),
            issued_at: now,
            whitelist: CycleWhitelist {
                cycles: BTreeMap::new(),
                conversions: Vec::new(),
                start_sizes: BTreeMap::new(),
            },
            outcome,
        };
        let Some(policy) = &self.config.arbitrage else {
            return Ok(empty(WhitelistOutcome::NoPolicy));
        };
        let Some(envelope) = self
            .envelopes
            .get(&(cell.to_string(), policy.strategy.clone()))
            .filter(|envelope| envelope.is_live(now))
        else {
            return Ok(empty(WhitelistOutcome::NoLiveGrant {
                strategy: policy.strategy.clone(),
            }));
        };
        let mut whitelist = policy.whitelist_for(envelope)?;
        // Omission, and only omission. A venue withdrawn on feasibility
        // evidence has its conversions dropped from what the policy already
        // produced; nothing here constructs a conversion, so the most the
        // withdrawn set can do to a whitelist is shrink it. Parsed venue,
        // never substring: `WhitelistedConversion.venue` is the id the
        // policy's map is keyed on. `CycleWhitelist.cycles` is not filtered
        // because nothing produces it — `whitelist_for` leaves it empty and
        // no cell reads it.
        let withdrawn: Vec<String> = policy
            .venues
            .keys()
            .filter(|venue| self.withdrawn_venues.contains(*venue))
            .cloned()
            .collect();
        whitelist
            .conversions
            .retain(|conversion| !self.withdrawn_venues.contains(conversion.venue.as_str()));
        let outcome = if whitelist.conversions.is_empty() && !withdrawn.is_empty() {
            WhitelistOutcome::AllWithdrawn { venues: withdrawn }
        } else {
            WhitelistOutcome::Emitted {
                edges: whitelist.conversions.len(),
                sized_against: envelope.signature().to_string(),
                withdrawn,
            }
        };
        Ok(WhitelistIssue {
            cell: cell.to_string(),
            issued_at: now,
            outcome,
            whitelist,
        })
    }

    /// Omit `venue` from every whitelist this plane issues until it is
    /// reinstated. Idempotent; a subtraction, and the plane has no path that
    /// reads the set to add a conversion.
    pub fn withdraw_venue(&mut self, venue: impl Into<String>) {
        let venue = venue.into();
        // A venue withdrawn again is no longer one the cells are merely
        // behind on: their slot 11 is right about it, and a refusal there is
        // an echo, classified as one. Leaving it here would keep the
        // *correct* refusals of a withdrawn venue out of a numerator for
        // ever, which is a distinction the echo rule already draws better.
        self.reinstated_venues.remove(&venue);
        self.withdrawn_venues.insert(venue);
    }

    /// Stop omitting `venue`, and remember that this plane is the reason the
    /// cells are about to be wrong about it. `true` if it was withdrawn.
    /// What comes back is only what the policy and the grant already
    /// permitted.
    ///
    /// The second effect is not bookkeeping: see the `reinstated_venues` field
    /// for the loop it breaks. Recorded only where a withdrawal was actually
    /// lifted, so a call naming a venue this plane never withdrew excuses
    /// nothing.
    pub fn reinstate_venue(&mut self, venue: &str) -> bool {
        let was_withdrawn = self.withdrawn_venues.remove(venue);
        if was_withdrawn {
            self.reinstated_venues.insert(venue.to_string());
        }
        was_withdrawn
    }

    /// The venues this plane omits from every whitelist, in name order.
    pub fn withdrawn_venues(&self) -> &BTreeSet<String> {
        &self.withdrawn_venues
    }

    /// Policy slot 11 as this plane can honestly fill it: the withdrawn set
    /// it is *applying*, and three empty grids.
    ///
    /// **Why the grids stay empty.** `central::whitelist`'s register argues
    /// the refusal at length and it still holds: the slot's grids are keyed
    /// by venue, the only grids this platform states are keyed by instrument
    /// (`Platform::assemble` installs them through
    /// `with_instrument_feasibility`), and `qip_edge::feasibility::effective`
    /// takes a slot grid in *preference* to the cell's own — so an
    /// instrument's tick filed under a venue key would not sit beside the
    /// right number, it would replace it for every instrument at that venue.
    /// An empty map resolves to `None` at that lookup and leaves the cell's
    /// own grid in force, which is the same thing an unproduced slot did.
    ///
    /// **Why the set is what is applied, not what is intended.** This reads
    /// `withdrawn_venues`, the single field `Platform::withdraw_venue` writes
    /// *after* the `venue.withdrawn` record is in the log and the same field
    /// [`Self::cycle_whitelist_for`] `retain`s against. The cells therefore
    /// refuse on exactly the set the centre's own whitelist omits on, and a
    /// withdrawal the log does not hold reaches neither.
    ///
    /// **And the dark regions, derived at `now`** (ADR 0079 decision five).
    /// The set a cell suspends its mirrors on is the same derivation `issue`
    /// refuses on and the share freeze reads, taken at the instant the slot
    /// is produced, so the cells and the centre cannot disagree about which
    /// regions are dark at one payload. Empty — and so absent from the wire,
    /// leaving every pre-field digest intact — whenever the derivation is
    /// off or nothing is dark.
    pub fn feasibility_constraints(&self, now: Timestamp) -> FeasibilityConstraints {
        FeasibilityConstraints {
            minimum_order: BTreeMap::new(),
            fee_floor: BTreeMap::new(),
            tick: BTreeMap::new(),
            withdrawn_venues: self.withdrawn_venues.clone(),
            dark_regions: self.dark_regions(now),
        }
    }

    // --- ADR 0079: a dark region is the centre's word for silence ----------

    /// The operator's window, or `None` where the derivation is off.
    ///
    /// Every surface that renders darkness reads this first, so an operator
    /// looking at "no region is dark" can tell it from "nobody is looking".
    pub fn region_dark_after(&self) -> Option<Duration> {
        self.config.region_dark_after
    }

    /// When the centre last heard from each cell, and where the cell said it
    /// was. The input to the derivation, in cell order.
    pub fn last_heard(&self) -> &BTreeMap<String, LastHeard> {
        &self.last_heard
    }

    /// Every region dark at `now`, with the reading each was derived from,
    /// in region order.
    ///
    /// Derived on every call and stored nowhere: a region is dark when the
    /// centre has heard from at least one of its cells and the most recent
    /// such report is more than the window old. Silence is measured on the
    /// centre's clock (`LastHeard::at` is the ingestion instant), and a
    /// region whose last report is *exactly* the window old is not yet dark
    /// — "silent past the interval", not "silent for the interval". With no
    /// window configured nothing is dark, and [`Self::region_dark_after`]
    /// is how a reader tells that apart from a healthy fleet.
    pub fn region_darkness(&self, now: Timestamp) -> BTreeMap<String, RegionDarkness> {
        let Some(window) = self.config.region_dark_after else {
            return BTreeMap::new();
        };
        // The most recent report per region, and which cell made it. A cell
        // that reported no region is in none and derives nothing.
        let mut latest: BTreeMap<&str, (&str, Timestamp)> = BTreeMap::new();
        for (cell, heard) in &self.last_heard {
            if heard.region.is_empty() {
                continue;
            }
            match latest.get(heard.region.as_str()) {
                Some((_, at)) if *at >= heard.at => {}
                _ => {
                    latest.insert(heard.region.as_str(), (cell.as_str(), heard.at));
                }
            }
        }
        latest
            .into_iter()
            .filter(|(_, (_, at))| now > at.saturating_add(window))
            .map(|(region, (cell, at))| {
                (
                    region.to_string(),
                    RegionDarkness {
                        region: region.to_string(),
                        last_heard_from: cell.to_string(),
                        last_heard_at: at,
                        window,
                    },
                )
            })
            .collect()
    }

    /// The names of every region dark at `now` — what the feasibility slot
    /// ships and the share freeze reads.
    pub fn dark_regions(&self, now: Timestamp) -> BTreeSet<String> {
        self.region_darkness(now).into_keys().collect()
    }

    /// Whether `cell` is in a region dark at `now`, and the reading if so.
    ///
    /// Keyed on the region the cell itself last reported, because that is
    /// the only region the centre has for it; a cell the centre has never
    /// heard from is in no region and is not refused here — it is unknown,
    /// and unknown already receives no share.
    pub fn darkness_of(&self, cell: &str, now: Timestamp) -> Option<RegionDarkness> {
        let region = self.last_heard.get(cell)?.region.as_str();
        self.region_darkness(now).remove(region)
    }

    /// Every change in derived darkness since the last announced read, for
    /// the platform to journal — a `WentDark` for each region dark now and
    /// not yet announced, a `SpokeAgain` for each region announced and no
    /// longer dark.
    ///
    /// Pure: nothing moves until [`Self::announce`] is called with the
    /// transition the platform has put in the log. A journal failure
    /// therefore leaves the change pending and it is offered again on the
    /// next read, under the same idempotency key, rather than being lost
    /// between a set that moved and a record that never existed.
    pub fn region_transitions(&self, now: Timestamp) -> Vec<RegionTransition> {
        let dark = self.region_darkness(now);
        let mut transitions = Vec::new();
        for (region, reading) in &dark {
            if !self.announced_dark.contains(region) {
                transitions.push(RegionTransition::WentDark(reading.clone()));
            }
        }
        for region in &self.announced_dark {
            if dark.contains_key(region) {
                continue;
            }
            // The report that cleared it: the most recent from any cell of
            // the region. Absent only if the region was announced from a
            // reading this plane no longer holds, which no path produces —
            // `last_heard` is never pruned — so the arm is a refusal to
            // guess rather than a case.
            let Some((cell, heard_at)) = self
                .last_heard
                .iter()
                .filter(|(_, heard)| heard.region == *region)
                .map(|(cell, heard)| (cell.clone(), heard.at))
                .max_by_key(|(_, at)| *at)
            else {
                continue;
            };
            transitions.push(RegionTransition::SpokeAgain {
                region: region.clone(),
                cell,
                heard_at,
            });
        }
        transitions
    }

    /// Record that `transition` is in the log, so it is not offered again.
    pub fn announce(&mut self, transition: &RegionTransition) {
        match transition {
            RegionTransition::WentDark(reading) => {
                self.announced_dark.insert(reading.region.clone());
            }
            RegionTransition::SpokeAgain { region, .. } => {
                self.announced_dark.remove(region);
            }
        }
    }

    /// The regions whose darkness has been journaled and not yet cleared —
    /// the announced set, for a reader that wants to compare it with the
    /// derivation. Decides nothing.
    pub fn announced_dark(&self) -> &BTreeSet<String> {
        &self.announced_dark
    }

    /// Count every strategy move the plane's ledger records, and every
    /// reconciliation break and cell halt this plane causes, into `metrics`.
    ///
    /// Attached after assembly rather than taken by the constructor, because
    /// the plane a deployment builds is swapped into a platform that already
    /// owns the registry, and the swap is where the two meet.
    pub fn attach_metrics(&mut self, metrics: Arc<Metrics>) {
        self.factory.attach_metrics(Arc::clone(&metrics));
        self.metrics = Some(metrics);
    }

    pub fn factory(&self) -> &StrategyFactory {
        &self.factory
    }

    pub fn factory_mut(&mut self) -> &mut StrategyFactory {
        &mut self.factory
    }

    pub fn allocator(&self) -> &CapitalAllocator {
        &self.allocator
    }

    pub fn compliance(&self) -> &CompliancePlane {
        &self.compliance
    }

    pub fn compliance_mut(&mut self) -> &mut CompliancePlane {
        &mut self.compliance
    }

    /// The whole book as the centre sees it, stale by the round trip from the
    /// cells — see the cost section of ADR 0008.
    /// Gross notional per reporting cell — the absolute values of every
    /// position a cell last reported, summed.
    ///
    /// This is a per-cell read and it is deliberately *not* an insight: it
    /// exists for `central::insights`, which aggregates it behind the
    /// confidential gate, and for nothing else. A caller with an operational
    /// need for one named cell's book reads the plane's other accessors and is
    /// audited as such. See the insights module on why the two must not blur.
    pub fn gross_notional_by_cell(&self) -> Vec<(String, Decimal)> {
        self.positions
            .iter()
            .map(|(cell, positions)| {
                let gross = positions
                    .iter()
                    .map(|position| position.signed_notional().abs())
                    .fold(Decimal::ZERO, |sum, notional| {
                        sum.checked_add(notional).unwrap_or(Decimal::MAX)
                    });
                (cell.clone(), gross)
            })
            .collect()
    }

    /// Realised loss per reporting cell, summed over its strategies.
    pub fn realised_loss_by_cell(&self) -> Vec<(String, Decimal)> {
        let mut by_cell: BTreeMap<String, Decimal> = BTreeMap::new();
        for ((cell, _strategy), utilisation) in &self.utilisation {
            let entry = by_cell.entry(cell.clone()).or_insert(Decimal::ZERO);
            *entry = entry
                .checked_add(utilisation.realised_loss)
                .unwrap_or(Decimal::MAX);
        }
        by_cell.into_iter().collect()
    }

    pub fn exposure(&self) -> &AggregateExposure {
        &self.exposure
    }

    pub fn recalls(&self) -> &RecallRegister {
        &self.recalls
    }

    pub fn concentration_limits(&self) -> ConcentrationLimits {
        self.concentration
    }

    /// Tighten or loosen the shares of gross any one bucket may take.
    pub fn set_concentration_limits(&mut self, limits: ConcentrationLimits) {
        self.concentration = limits;
    }

    /// Register or replace the evidence the allocator sizes a strategy on.
    pub fn set_proposal(&mut self, proposal: StrategyProposal) {
        self.proposals.insert(proposal.strategy.clone(), proposal);
    }

    pub fn proposal(&self, strategy: &StrategyId) -> Option<&StrategyProposal> {
        self.proposals.get(strategy)
    }

    /// The grant a cell currently holds for a strategy, if one was issued here.
    pub fn envelope(&self, cell: &str, strategy: &StrategyId) -> Option<&CapitalEnvelope> {
        self.envelopes.get(&(cell.to_string(), strategy.clone()))
    }

    /// What a strategy has committed against its grant, as the cell last said.
    /// What one strategy holds in one instrument at one cell, as attributed.
    pub fn strategy_lot(
        &self,
        cell: &str,
        strategy: &StrategyId,
        instrument: &str,
    ) -> Option<&StrategyLot> {
        self.books
            .get(&(cell.to_string(), strategy.clone(), instrument.to_string()))
    }

    /// Every strategy book, in key order.
    pub fn strategy_books(&self) -> &BTreeMap<(String, StrategyId, String), StrategyLot> {
        &self.books
    }

    /// The last book each cell reported, in cell order — the cell's own claim
    /// about its positions, as distinct from the attribution's books above.
    /// Empty for a cell whose report carried no positions, which is what the
    /// delta wire ships; see `disposition_for` in the learn edge for how the
    /// two claims are read together.
    pub fn reported_positions(&self) -> impl Iterator<Item = &CellPosition> {
        self.positions.values().flatten()
    }

    /// Book one settlement's attributed P&L into each contributing strategy's
    /// realised sessions at the reporting cell.
    ///
    /// Read off the settlement's attribution and not off the report a second
    /// time, for the same reason the risk aggregate is: what the review
    /// judges and what the centre billed are then one figure. A strategy the
    /// factory holds no baseline for is not recorded — it has no pilot to
    /// have decayed from, so `learn` would skip it, and a series nothing
    /// reads is exactly the unbounded working set the retention rule
    /// forbids. The capital beside the day's P&L is the envelope the centre
    /// holds for the pair at this instant, which is the grant the fills were
    /// made under.
    fn record_realised(&mut self, cell: &str, settlement: &Settlement, at: Timestamp) {
        let cost = settlement.cost_bps_by_strategy();
        for (strategy, pnl) in settlement.by_strategy() {
            // Taken before `strategy` is consumed into a `StrategyId`, and by
            // the same key the P&L was: what the monitor reads as this
            // strategy's cost and what it reads as its P&L come off one
            // settlement under one name.
            let accrual = settlement.cost.get(&strategy).copied().unwrap_or_default();
            let mean = cost.get(&strategy).copied();
            let strategy = StrategyId::new(strategy);
            if self.factory.baseline(&strategy).is_none() {
                continue;
            }
            let capital = self
                .envelope(cell, &strategy)
                .map(CapitalEnvelope::gross_limit);
            let series = self
                .realised
                .entry((cell.to_string(), strategy))
                .or_default();
            series.absorb(at, pnl, capital);
            // Only where quantity actually filled. A settlement that booked a
            // strategy's P&L without filling anything for it — an internal
            // cross moves a lot at the mid and sends no order — has no cost
            // to add, and adding a zero would pull the mean toward nil on
            // exactly the days the strategy did not pay a spread.
            if mean.is_some() {
                series.absorb_cost(at, accrual.weighted, accrual.quantity);
            }
        }
    }

    /// Retain, on the day of `at`, every live grant this cell holds — whether
    /// or not anything settled under it.
    ///
    /// The fact a settlement cannot carry. A day under a grant that traded
    /// nothing left no session at all before this, so afterwards the centre
    /// could not tell it from a day under no grant: the first is a return of
    /// zero and the second is not a return, and nothing retained distinguished
    /// them. This writes the first as what it is.
    ///
    /// Gated on [`CapitalEnvelope::is_live`] at the report's own instant, not
    /// on the presence of an envelope. The map keeps a grant after it expires,
    /// and a lapsed grant is an authority the cell may no longer commit
    /// against; retaining one would put a denominator behind a day on which
    /// the strategy held nothing.
    ///
    /// Filtered by the factory's baseline for the same reason
    /// [`CentralPlane::record_realised`] is, and it is what bounds the work:
    /// grants held at this cell for strategies that have reached pilot, both
    /// fixed by decisions rather than by traffic.
    fn retain_grants(&mut self, cell: &str, at: Timestamp) {
        // Collected before the write because the grants and the series are
        // two fields of the same plane; the list is bounded by the strategies
        // holding a grant at one cell.
        let live: Vec<(StrategyId, Decimal)> = self
            .envelopes
            .iter()
            .filter(|((held_cell, strategy), envelope)| {
                held_cell == cell
                    && envelope.is_live(at)
                    && self.factory.baseline(strategy).is_some()
            })
            .map(|((_, strategy), envelope)| (strategy.clone(), envelope.gross_limit()))
            .collect();
        for (strategy, grant) in live {
            self.realised
                .entry((cell.to_string(), strategy))
                .or_default()
                .retain_grant(at, grant);
        }
    }

    /// The retained corpus as one day-keyed series per strategy: what each
    /// attributed on each closed day it held a live grant, summed across the
    /// cells that held one.
    ///
    /// The exposure [`CentralPlane::live_outcomes`] is not. That one answers
    /// "how has this strategy done since its baseline" and hands back returns
    /// with the day dropped, which is the right shape for a verdict on one
    /// strategy and the wrong shape for anything comparing two: a correlation
    /// needs both series on one calendar, and a `Vec<f64>` cannot be aligned
    /// to anything.
    ///
    /// Derived on every call rather than kept beside the sessions, for the
    /// same reason `live_outcomes` is: the calendar a caller reads is the one
    /// the retained sessions support at `now`. A late report revises a day
    /// that has already closed, and the revision is visible from the next call
    /// onward — a fact recorded forward, never one legible before the centre
    /// knew it.
    pub fn realised_calendar(&self, now: Timestamp) -> RealisedCalendar {
        let mut calendar = RealisedCalendar::default();
        for ((_, strategy), series) in &self.realised {
            for (day, granted) in series.granted_days(now) {
                calendar.absorb(strategy, day, granted);
            }
        }
        calendar
    }

    /// The live observation the demotion monitor should review this tick,
    /// one per strategy per cell that has closed a session since its
    /// baseline was established, in cell then strategy order.
    ///
    /// Derived on every call from the retained sessions rather than kept
    /// beside them, so the observation a review reads is the one the
    /// sessions support at `now` and not one computed under an earlier
    /// clock. A strategy whose baseline the factory has since dropped
    /// contributes nothing: the sessions were recorded against a baseline,
    /// and without one there is nothing for them to have decayed from.
    pub fn live_outcomes(&self, now: Timestamp) -> Vec<CellOutcome> {
        self.realised
            .iter()
            .filter_map(|((cell, strategy), series)| {
                let baseline = self.factory.baseline(strategy)?;
                series.outcome(strategy, cell, baseline.established_at, now)
            })
            .collect()
    }

    /// Drop the retained sessions of a strategy the ledger has retired.
    ///
    /// Retirement is terminal — the ledger refuses to move a retired strategy
    /// again — so a series kept for it would be reviewed every cycle for a
    /// verdict that cannot change, and would hold the working set open for a
    /// strategy the platform has finished with. The fills behind it stay in
    /// the event log.
    pub(super) fn forget_realised(&mut self, strategy: &StrategyId) {
        self.realised
            .retain(|(_, retained), _| retained != strategy);
    }

    pub fn utilisation(&self, cell: &str, strategy: &StrategyId) -> Option<&Utilisation> {
        self.utilisation.get(&(cell.to_string(), strategy.clone()))
    }

    /// Whether a strategy on a cell may act, per the incident log.
    pub fn may_act(&self, scope: &str, cell: &str) -> bool {
        self.compliance.may_act(scope, cell)
    }

    /// Enumerate the six controls and what enforces each.
    /// Whether this plane's signatures were made with a reproducible secret.
    ///
    /// A deployment that never supplied real key material is otherwise
    /// indistinguishable from one that did, which is the failure this answers.
    pub const fn signing_key_is_reproducible(&self) -> bool {
        self.key_is_reproducible
    }

    /// The compliance report, carrying this plane's own signing posture.
    ///
    /// The caveat is added here rather than inside `qip-compliance`, which
    /// cannot know how the key it was handed was obtained. A report that
    /// enumerated six enforced controls while the signing secret was
    /// derivable from a config file would be accurate and misleading.
    pub fn compliance_report(&self, now: Timestamp) -> Result<ComplianceReport> {
        let report = self.compliance.report(now);
        if !self.key_is_reproducible {
            return Ok(report);
        }
        report.with_additional_caveat(
            qip_contracts::Control::SignedArtifactsAndProvenance,
            "this plane's signing secret is reproducible from its configuration, so anyone who \
             knows the seed can mint an envelope; a deployment supplies real key material \
             through Platform::set_central",
        )
    }

    /// Size every capital-holding strategy against the budget at once.
    ///
    /// Built over the whole set rather than one strategy at a time, because
    /// the per-cell, per-venue and total limits bind jointly: a plan computed
    /// for one strategy in isolation is a plan that has not seen the headroom
    /// the others have already taken.
    pub fn allocate(&self, drawdown: f64, now: Timestamp) -> Result<AllocationPlan> {
        let proposals: Vec<StrategyProposal> = self
            .factory
            .holding_capital()
            .iter()
            .filter_map(|strategy| self.proposals.get(strategy).cloned())
            .collect();
        self.allocator.allocate(&proposals, drawdown, now)
    }

    /// Arm the blueprint §23.4 pool gate over this plane's lifecycle ledger,
    /// on the figures as they stand.
    ///
    /// From here on every promotion to a rung that holds capital is reconciled
    /// against the four pools, and one whose bucket would push a pool past its
    /// total is refused by [`qip_lifecycle::horizon::HorizonAssurance`] rather
    /// than trimmed. `Ok(None)` where the desk has stated no
    /// [`HorizonPolicy`]: with no split and no claims there is nothing to
    /// reconcile against, and arming a gate that would refuse every promotion
    /// for want of a claim is the `MaxExpectedShortfall` shape — a control that
    /// reads as protection and is not.
    ///
    /// **Three of the four inputs are computed rather than stated.** The pools'
    /// total is the configured risk budget; the split and the claims are the
    /// desk's statement; the liability is the commitment book's at `now`; and
    /// the budgets are this allocator's own sizing of the whole proposal book
    /// at `drawdown`. That last is the one that matters: the figure the gate
    /// reconciles is the figure the platform would actually deploy, not a
    /// number typed beside the split. A proposal the allocator sized at nothing
    /// carries **no** budget rather than a budget of zero, so promoting it is
    /// refused for want of a stated claim — a claim of zero and an unknown
    /// claim are not the same thing, and the second lets a strategy hold
    /// capital no pool was charged for.
    ///
    /// The assurance is attached **before** the standings are computed, so a
    /// register the platform's own sources are still arguing over arms the gate
    /// (which refuses on the dispute) instead of leaving it unarmed. Failing to
    /// describe a disagreement must not be the reason a promotion goes
    /// unchecked.
    ///
    /// An error before the attachment — an unreconcilable split, an allocator
    /// that cannot size the book — leaves whatever the previous cycle armed in
    /// place, and leaves nothing armed if no cycle has succeeded yet. The
    /// caller records it as a problem on the cycle rather than swallowing it.
    ///
    /// # The drawdown moves the charges and not the pools
    ///
    /// The two sides of the §23.4 comparison scale differently and that is
    /// deliberate. The pools are [`CentralConfig::total_budget`] split four
    /// ways, unscaled; every budget charged against them has already been
    /// multiplied by `DrawdownSchedule::multiplier_at(drawdown)` inside
    /// [`qip_capital::CapitalAllocator::allocate`]. An independent review
    /// queried the asymmetry, so the argument is written down here rather than
    /// left to be re-derived by whoever asks next:
    ///
    /// * The four pools state capital the desk **holds** and how liquid it is.
    ///   The drawdown schedule is an appetite response, not a balance sheet —
    ///   the shipped one takes the multiplier to 0.5 at a ten per cent
    ///   drawdown, and a book that has lost a tenth does not hold half its
    ///   capital. Scaling the pools by it would put a figure nobody measured on
    ///   the side of the comparison that is supposed to be the measurement.
    /// * The years pool is charged the unfunded commitment liability, which is
    ///   not scaled and must not be: a capital call does not shrink because the
    ///   platform chose to deploy less this cycle. A scaled reserved pool
    ///   against an unscaled liability would breach the years bucket at every
    ///   drawdown, refusing promotions against reserved capital the desk still
    ///   has.
    /// * At the schedule's deepest step the multiplier is zero, so a scaled
    ///   split would be four zero pools, which `CapitalPools::new` refuses by
    ///   name. The gate would fail to arm and [`UnarmedHorizons`] would refuse
    ///   every promotion — a control firing on the claim that the desk holds
    ///   nothing, which is false, and at the one drawdown where the allocator
    ///   has already sized everything at zero and nothing can breach.
    ///
    /// So fewer promotions breach while the book is falling, because the
    /// platform is committing less capital against unchanged pools. That is the
    /// appetite control working at the numerator, where it fires once; §23.4 is
    /// a liquidity-mismatch control and asks a different question. The honest
    /// limit is that a promotion admitted during a drawdown is not revisited
    /// when the multiplier returns to one: the next arming reports the bucket
    /// breached and refuses the *next* promotion, which is what a
    /// reconciliation does and a position limit does not.
    /// `a_drawdown_shrinks_the_charges_and_leaves_the_pools_at_the_capital_the_desk_holds`
    /// and
    /// `a_drawdown_deep_enough_to_stop_all_deployment_still_charges_the_commitment_liability`
    /// in `qip-kernel/tests/central.rs` pin both halves.
    pub fn arm_horizons(
        &mut self,
        unfunded_commitments: Decimal,
        drawdown: f64,
        now: Timestamp,
    ) -> Result<Option<HorizonArming>> {
        let Some(policy) = self.config.horizons.clone() else {
            return Ok(None);
        };

        // Every `?` from here on would otherwise leave the PREVIOUS cycle's
        // assurance attached, so the gate would keep measuring promotions
        // against pool bounds computed from a liability it can no longer read.
        // `arm_or_refuse` attaches `UnarmedHorizons` on the way out of any
        // failure, so the gate closes rather than going stale. See
        // `super::horizon::UnarmedHorizons` for why this is not a detach.
        let armed = self.arm_or_refuse(&policy, unfunded_commitments, drawdown, now);
        if let Err(error) = &armed {
            self.factory
                .attach_horizons(HorizonAssurance::new(Arc::new(UnarmedHorizons::new(
                    error.message(),
                )) as Arc<_>));
        }
        armed
    }

    /// The arming itself. Every failure here is turned into a closed gate by
    /// [`Self::arm_horizons`], which is the only caller.
    fn arm_or_refuse(
        &mut self,
        policy: &HorizonPolicy,
        unfunded_commitments: Decimal,
        drawdown: f64,
        now: Timestamp,
    ) -> Result<Option<HorizonArming>> {
        // `total_budget` unscaled, on purpose: the pools are capital the desk
        // holds, and `drawdown` does not reach them. See the "drawdown moves
        // the charges and not the pools" section on `arm_horizons` — this line
        // is the denominator that section is about.
        let mut reconciler =
            PoolReconciler::from_policy(policy, self.config.total_budget, unfunded_commitments)?;

        // Every proposal, not only the strategies already holding capital: the
        // candidate at a promotion is by definition not yet at a capital rung,
        // and a reconciler that knew nothing about it would refuse it for want
        // of a budget every time.
        //
        // `drawdown` is a statistic and stays one across this call: it selects
        // a step of the `DrawdownSchedule`, and the multiplier that step holds
        // is already `Decimal`, so the only multiplication is money by money.
        // The statistic never crosses into a currency figure here or inside
        // `allocate` — which is why the numerator shrinks exactly rather than
        // to whatever an `f64` product rounded to.
        let proposals: Vec<StrategyProposal> = self.proposals.values().cloned().collect();
        let plan = self.allocator.allocate(&proposals, drawdown, now)?;
        let mut budgeted = Decimal::ZERO;
        for allocation in &plan.allocations {
            reconciler.budget(&allocation.strategy, allocation.notional)?;
            budgeted = budgeted.checked_add(allocation.notional).ok_or_else(|| {
                Error::numeric(
                    "the allocator's own budgets overflow when summed; check the units of the \
                     central plane's risk budget",
                )
            })?;
        }

        let armed = Arc::new(reconciler);
        self.factory
            .attach_horizons(HorizonAssurance::new(Arc::clone(&armed) as Arc<_>));

        let (standings, unsettled) = match armed.standing() {
            Ok(standings) => (standings, None),
            Err(refusal) => (Vec::new(), Some(refusal.message().to_string())),
        };
        Ok(Some(HorizonArming {
            strategies_budgeted: plan.allocations.len(),
            budgeted,
            liability: unfunded_commitments,
            standings,
            unsettled,
            unbudgeted: plan
                .refusals
                .iter()
                .map(|(strategy, reason)| format!("{strategy}: {reason}"))
                .collect(),
        }))
    }

    /// Partition a plan into disjoint per-cell shares of each region's grant
    /// (ADR 0039), against the grants this plane holds issued.
    ///
    /// Refuses a plan whose cells' shares would together exceed a region's
    /// grant, and withholds a manifest from any cell whose live grants
    /// already sum past its share — see [`super::regions`] for why each is a
    /// refusal rather than a correction. Membership is an argument rather
    /// than configuration because where it comes from is the ADR's third
    /// owner decision, still open.
    ///
    /// **A dark region's share is frozen** (ADR 0079 decision four). Each
    /// cell whose region the membership files under a name dark at `now` is
    /// partitioned at the bound it last had while lit, not at the plan's
    /// current figure, and a dark cell with no such bound is withheld with
    /// the reason. The frozen bound still counts against the region's grant
    /// in the partitioner's invariant, so it is neither freed to the other
    /// cells of that region nor recomputed; and since `issue` refuses a
    /// grant into a dark region, the manifest a frozen share names can lose
    /// grants to expiry and never gain one. `&mut self` because the bound
    /// each lit cell is partitioned at is what the next dark reading will
    /// freeze — the memory is written here and nowhere else.
    pub fn region_shares(
        &mut self,
        plan: &AllocationPlan,
        membership: &RegionMembership,
        now: Timestamp,
    ) -> Result<RegionShares> {
        let dark = self.dark_regions(now);
        let mut frozen: BTreeMap<String, Decimal> = BTreeMap::new();
        let mut unfrozen: Vec<String> = Vec::new();
        for (cell, region) in membership.cells() {
            if !dark.contains(region) {
                continue;
            }
            match self.lit_share_bounds.get(cell) {
                Some(bound) => {
                    frozen.insert(cell.clone(), *bound);
                }
                None => unfrozen.push(cell.clone()),
            }
        }
        let mut shares = partition(plan, membership, &self.envelopes, &frozen, now)?;
        for cell in unfrozen {
            let region = membership.region_of(&cell).unwrap_or_default().to_string();
            shares.withhold(
                &cell,
                format!(
                    "cell {cell} is in region {region}, which is dark, and no share bound was \
                     computed for it while the region was lit; nothing new enters a dark \
                     region, so no manifest ships until one of its cells reports again"
                ),
            );
        }
        for (cell, share) in shares.shares() {
            if !dark.contains(share.region()) {
                self.lit_share_bounds.insert(cell.clone(), share.amount());
            }
        }
        Ok(shares)
    }

    /// The `capital_grants` slot for every configured cell's payload: each
    /// cell's share of its region's grant, as a manifest of the grants this
    /// plane holds issued to it, or the reason the slot ships unproduced
    /// (ADR 0039).
    ///
    /// The plan is sized here, by the same [`Self::allocate`] the envelopes
    /// were issued against, so the share and the envelopes are one number
    /// from one source. `drawdown` is the statistic the allocator shrinks
    /// under, as [`Self::issue`] takes it. A plan the partitioner refuses
    /// withholds every cell with the refusal, and a cell absent from the
    /// membership is withheld with that: no cell is ever shipped a manifest
    /// the plan did not produce, and none is given a region by default.
    pub fn grant_manifests<'a>(
        &mut self,
        cells: impl IntoIterator<Item = &'a str>,
        membership: &RegionMembership,
        drawdown: f64,
        now: Timestamp,
    ) -> GrantManifests {
        let partitioned = self
            .allocate(drawdown, now)
            .and_then(|plan| self.region_shares(&plan, membership, now));
        GrantManifests::decide(cells, partitioned)
    }

    /// Issue a grant.
    ///
    /// Refuses before it computes anything if the factory does not say the
    /// strategy holds capital. That check is the whole of the connection
    /// between the ladder and the money: a strategy at shadow with flawless
    /// evidence and two willing approvers still gets nothing, because the
    /// stage it stands at is what decides, and the stage is the ledger's to
    /// say.
    ///
    /// # This has no production caller, and the cycle must not become one
    ///
    /// Every call site is a test — `qip-kernel/tests/central.rs`,
    /// `qip-acceptance/tests/region_share.rs`, `qip-api/tests/mesh.rs`. No
    /// `src/` file in any binary reaches it, which is the opposite of
    /// [`Self::ingest`] beside it: that one is called from
    /// `Platform::ingest_cell_report`, which `qip_api::mesh`'s delta sink
    /// drives on every frame a cell ships.
    ///
    /// **The missing caller is an operator-authenticated route on `qip-api`**
    /// — a fifth mutating row of its route table, beside
    /// `/ledger/users/:user/eligibility` and `/registrations/:source/approve`,
    /// raising a typed kernel intent the way those raise
    /// `Platform::decide_eligibility` and `Platform::approve_registration`.
    /// Three things have to exist first, and only that surface has any of
    /// them: an [`Approval`] naming two humans, neither the requester; one
    /// [`OperatorCredential`] per name, minted by the API's authentication
    /// middleware, which `qip_compliance::approval` documents as the one place
    /// `OperatorCredential::verified` may be called; and the platform's own
    /// `Platform::drawdown` for the `drawdown` argument, because the manifests
    /// in `qip_api::mesh::pending_policy` are partitioned under that figure
    /// and an envelope sized under a different one would make the grant and
    /// the share two numbers instead of one. Every test here passes `0.0`,
    /// which is a fixture's answer and not a deployment's.
    ///
    /// **A cycle stage must not be that caller.** It would have to manufacture
    /// the approval and the credentials, which is forging control 4, human
    /// capital approval — `qip-compliance`'s own first line is that capital is
    /// not granted by code. It would also defeat the bound this platform
    /// relies on most where it can see least: `MAXIMUM_ENVELOPE_VALIDITY` is
    /// the only revocation there is for a cell the centre cannot reach, and an
    /// expiry a process renews for itself every cycle is not an expiry.
    ///
    /// **What stays dead until the route exists**, because this is the only
    /// writer of `self.envelopes`: `retain_grants` retains no day, so the
    /// realised calendar is empty and the LEARN stage's family measurement
    /// (blueprint §23.1) records nothing on any cycle, however much the cells
    /// settle — pinned by
    /// `the_learn_stage_measures_no_family_structure_on_a_corpus_the_centre_never_granted`
    /// in `tests/central.rs`; [`Self::cycle_whitelist_for`] answers
    /// `NoLiveGrant` for every cell; `recall_for` has no live grant to recall
    /// when the exposure aggregate finds a concentration; and
    /// [`Self::grant_manifests`], which *does* have a production caller,
    /// partitions an empty book. That list is what arrives with the route. It
    /// is not an argument for reaching this from the cycle instead.
    pub fn issue(
        &mut self,
        strategy: &StrategyId,
        requested_by: &str,
        approval: &Approval,
        credentials: &[OperatorCredential],
        drawdown: f64,
        now: Timestamp,
    ) -> Result<IssuedCapital> {
        let stage = self.factory.stage_of(strategy);
        if !stage.holds_capital() {
            return Err(Error::denied(format!(
                "{strategy} stands at {}, which holds no capital; no envelope is issued until it \
                 has passed the pilot gate",
                stage.as_str()
            )));
        }
        if !self.proposals.contains_key(strategy) {
            return Err(Error::not_found(format!(
                "{strategy} holds capital but has no proposal for the allocator to size it on"
            )));
        }

        let plan = self.allocate(drawdown, now)?;
        let allocation = plan
            .for_strategy(strategy)
            .ok_or_else(|| {
                let refusal = plan
                    .refusals
                    .iter()
                    .find(|(id, _)| id == strategy)
                    .map(|(_, reason)| reason.clone())
                    .unwrap_or_else(|| {
                        "the allocator produced neither an allocation nor a refusal".to_string()
                    });
                Error::guard(format!("{strategy} was allocated nothing: {refusal}"))
            })?
            .clone();

        // ADR 0079 decision four: nothing new enters a dark region. Refused
        // here, after the allocator has named the cell and before either
        // record is signed, so a refusal leaves no half-issued grant behind.
        // The refusal carries the reading it was made from — the region, the
        // cell last heard from, the instant and the window — because an
        // operator told only "region dark" has to re-derive all four.
        if let Some(darkness) = self.darkness_of(&allocation.cell, now) {
            return Err(Error::denied(format!(
                "no envelope is issued to {strategy} at {}: {}",
                allocation.cell,
                darkness.describe()
            )));
        }

        let terms = EnvelopeTerms::from_allocation(&allocation, self.config.envelope_validity);
        let envelope = self.issuer.issue(&terms, approval, now)?;
        // Verified immediately rather than trusted: the issuer is the boundary
        // that decides whether a cell may commit capital, and a grant that does
        // not verify here would fail at the cell with nobody watching.
        self.issuer.verify(&envelope, now)?;

        let request = CapitalRequest {
            strategy: strategy.clone(),
            cell: allocation.cell.clone(),
            gross_limit: envelope.gross_limit(),
            order_limit: envelope.order_limit(),
            loss_limit: envelope.loss_limit(),
            venues: terms.venues.clone(),
            expires_at: envelope.expires_at(),
            requested_by: requested_by.to_string(),
        };
        let approved =
            self.compliance
                .approvals_mut()
                .grant(&request, approval, credentials, now)?;

        // The two records must bound the same thing. The signing payload covers
        // every field that limits what the cell may do, so equality here is
        // equality of the whole authority, not of a summary of it.
        if approved.envelope().signing_payload() != envelope.signing_payload() {
            return Err(Error::guard(format!(
                "the approved grant and the issued grant for {strategy} describe different \
                 terms: approved `{}`, issued `{}`",
                approved.envelope().signing_payload(),
                envelope.signing_payload()
            )));
        }

        self.envelopes.insert(
            (allocation.cell.clone(), strategy.clone()),
            envelope.clone(),
        );
        Ok(IssuedCapital {
            approved,
            envelope,
            allocation,
        })
    }

    /// Seal the bundle a cell runs from.
    ///
    /// The stage comes from the ledger rather than from the caller, so a DNA
    /// cannot be sealed for a rung a strategy is not standing on.
    pub fn ship(
        &self,
        issued: &IssuedCapital,
        signer: impl Into<String>,
        now: Timestamp,
    ) -> Result<StrategyDna> {
        let strategy = issued.envelope().strategy();
        let candidate = self.factory.candidate(strategy).ok_or_else(|| {
            Error::not_found(format!(
                "{strategy} holds a grant but is not registered, so there is no compiled program \
                 to ship"
            ))
        })?;
        StrategyDna::seal(
            candidate,
            self.factory.stage_of(strategy),
            issued.approved(),
            issued.envelope(),
            &self.key,
            signer,
            now,
        )
    }

    /// Check a bundle under this plane's key. What a cell does on arrival.
    pub fn verify_dna(&self, dna: &StrategyDna, now: Timestamp) -> Result<()> {
        dna.verify(&self.key, now)
    }

    /// Absorb one cell's report.
    ///
    /// The kill switch is passed in rather than owned because the platform
    /// already has one and two kill switches is one too many: an operator
    /// looking at `qip_risk_engine::autonomy` must see every halt, including
    /// the ones the central plane caused.
    pub fn ingest(
        &mut self,
        report: CellReport,
        kill_switch: &mut KillSwitch,
        now: Timestamp,
    ) -> Result<CellIngestion> {
        if report.cell.trim().is_empty() {
            return Err(Error::invalid(
                "a cell report must name the cell it is from",
            ));
        }
        for position in &report.positions {
            if position.cell != report.cell {
                return Err(Error::invalid(format!(
                    "the report from {} carries a position booked at {}; a cell reports its own \
                     book and nobody else's",
                    report.cell, position.cell
                )));
            }
        }

        // The centre heard from this cell, now, in the region it says it is
        // in — written before anything below can halt or refuse the report,
        // because a halted cell still spoke and a region whose only cell is
        // halted is not dark, it is halted, which the kill switch already
        // says. `now` and not `report.at`: silence is measured on the
        // centre's clock, since a silent cell's clock is exactly what the
        // centre cannot read.
        self.last_heard.insert(
            report.cell.clone(),
            LastHeard {
                region: report.region.clone(),
                at: now,
            },
        );

        let absorbed = report.positions.len();
        // Replace rather than merge: the report is the whole of this cell's
        // book, and a stale position that survived a replace would show up in
        // the aggregate as risk nobody holds. Never removed and never zeroed
        // by silence: a dark region's last book stays here, in the aggregate
        // below, in every concentration, in `crowded`'s cell count and in
        // `cells_behind`, because a position does not vanish when its
        // reporter does (ADR 0079 decision three).
        self.positions
            .insert(report.cell.clone(), report.positions.clone());
        let all: Vec<CellPosition> = self.positions.values().flatten().cloned().collect();
        self.exposure = AggregateExposure::of(&all);

        for (strategy, utilisation) in &report.utilisation {
            self.utilisation
                .insert((report.cell.clone(), strategy.clone()), utilisation.clone());
        }

        // The interval's orders and crosses reach the strategy books whether
        // or not the report reconciles: they are what the cell did, and a
        // halt is about what it may do next. Settled before the halt so a
        // refusal in the recall step cannot leave a fill half-attributed.
        let settlement = self.settle(&report, now);
        self.record_realised(&report.cell, &settlement, report.at);
        // The grant the cell held while it made this report, on the same day
        // the settlement was booked into and at the report's own instant. A
        // day that settled nothing is a day of the record too, and until this
        // line it was thrown away.
        self.retain_grants(&report.cell, report.at);

        // The cell's breaks and the settlement's, halted together: a fill on
        // an order the centre never saw sent is the venue's channel and the
        // platform's record disagreeing, which is the same failure the cell
        // halts itself on when its own drop copy finds it.
        let breaks: Vec<ReconciliationBreak> = report
            .reconciliation_breaks
            .iter()
            .chain(settlement.breaks.iter())
            .cloned()
            .collect();
        let halted = if breaks.is_empty() {
            None
        } else {
            Some(self.halt_cell(&report.cell, &breaks, now)?)
        };
        if halted.is_some() {
            let reason = breaks
                .iter()
                .map(ReconciliationBreak::describe)
                .collect::<Vec<_>>()
                .join("; ");
            // Scoped, not global. The other cells' books still reconcile, and
            // stopping them would turn one cell's bookkeeping failure into the
            // platform's outage.
            kill_switch.trip_scope(
                report.cell.clone(),
                now,
                "central-plane:reconciliation",
                format!(
                    "{} record(s) at {} do not reconcile with the venue: {reason}",
                    breaks.len(),
                    report.cell
                ),
            );
            // Counted here, the instant after the switch is tripped, and not
            // by the caller on the returned ingestion: the recall step below
            // can still refuse, and a count that waited for `Ok` would be
            // un-counted by any error between the trip and the return. The
            // halt has happened by this line whatever happens after it, so
            // this is the only place the count is true. The break is keyed on
            // its direction and the halt on its cause; neither the cell nor
            // the instrument is a label, because both are dimensions that
            // grow.
            self.record_halt(&breaks);
        }

        let concentrations = self.exposure.concentrations(&self.concentration);
        let crowded = self
            .exposure
            .crowded(self.config.minimum_cells_for_crowding);
        let recalls = self.recall_for(&concentrations, now)?;
        let (
            feasibility_refusals,
            feasibility_refusals_unattributed,
            feasibility_refusals_repeated,
        ) = self.attribute_refusals(&report, now);

        Ok(CellIngestion {
            cell: report.cell,
            positions_absorbed: absorbed,
            halted,
            concentrations,
            crowded,
            recalls,
            settlement,
            feasibility_refusals,
            feasibility_refusals_unattributed,
            feasibility_refusals_repeated,
        })
    }

    /// Admit the report's venue-bearing refusals to the feasibility window,
    /// or say why each could not be.
    ///
    /// A refusal is admitted when its gate is one of the nine
    /// `qip_contracts::feasibility::EDGE_GATES` **and** its venue is one
    /// the centre knows — a key of the arbitrage policy's venue map, or a
    /// venue on a grant live at `now`. Both are checked against the source
    /// the cell would itself have been configured from, so the `venue`
    /// label on the series stays bounded by configuration however a delta
    /// is worded. A refusal that names no venue at all is not looked at:
    /// only `admit_feasible` at the cell names one, and a posture refusal
    /// was counted at the cell under its own gate.
    ///
    /// What cannot be attributed is returned as the label pair it is
    /// counted under and kept out of the window. `unknown` for a venue
    /// nothing permits and `other` for a gate outside the vocabulary: two
    /// literals, so a cell that ships a venue nobody configured cannot mint
    /// a series, and a cluster that would have withdrawn it is charted
    /// under a name an operator can search for rather than acted on.
    ///
    /// An admitted refusal also carries `report.cell` — the reporting cell's
    /// self-asserted identity on a wire that authenticates nobody, so not a
    /// verified fact, but the only key `venue_review::assess` has to require
    /// more than one cell before edge-only evidence withdraws a venue. See
    /// `VENUE_WITHDRAWAL_MIN_CELLS`.
    ///
    /// **Every refusal is admitted at a bounded rate, whatever its gate.**
    /// The first refusal per venue per gate in a report takes a window seat;
    /// every repeat comes back on `feasibility_refusals_repeated`, counted
    /// on the series and seated nowhere. A report is one cell's observation
    /// at one instant, and one message must not be able to be the whole
    /// window: `venue_review::VENUE_WITHDRAWAL_MIN_CELLS` counts *distinct
    /// cells*, not evidence per cell, so without this bound thirty refusals
    /// in one report plus one token refusal from a second name withdrew a
    /// venue for the entire platform — a probe did it in two messages on a
    /// wire that authenticates nobody. This cap existed before only for
    /// `feasibility::GATE_WITHDRAWN_VENUE`, and the argument for it was
    /// never particular to that gate.
    ///
    /// **Three classifications, all taken here and none re-derived later.**
    /// `venue_review::assess` used to re-ask them against whatever the
    /// withdrawn set held when it ran, and a reinstatement therefore
    /// rewrote the meaning of records already made.
    ///
    /// - An **echo** is a refusal under `GATE_WITHDRAWN_VENUE` at a venue
    ///   this plane itself holds withdrawn: the cell enforcing a decision
    ///   the centre already made. Seated, because a venue the platform is
    ///   still attempting must stay in the denominator a share is measured
    ///   against — dropping it is how the runner-up becomes a cluster of the
    ///   remainder — and weight-capped by `venue_review::VenueTally::weight`,
    ///   so it can sustain that venue's weight and never amplify it.
    /// - A **stale withdrawal** is the same gate at a venue this plane
    ///   withdrew and two operators have since put back. The centre is the
    ///   reason the cell is wrong, and a reinstatement makes every cell
    ///   stale about that venue by construction, so admitting these as
    ///   evidence meant the signatures withdrew the venue themselves: a
    ///   probe re-withdrew a venue on 256 entries not one of which was a
    ///   genuine refusal. Seated as `pardoned` — denominator only, never a
    ///   numerator.
    /// - Everything else is **evidence**, including that gate at a venue
    ///   this plane has *never* withdrawn. That is the security review's
    ///   case and it is unchanged: a cell inventing a withdrawal, or holding
    ///   one the centre never shipped, makes an ordinary refusal at a venue
    ///   in use, and keeping those out of the window is what once made a
    ///   venue unwithdrawable for as long as a slot stayed stale.
    ///
    /// Whose decision any of this is, is the centre's: the sets consulted
    /// are `self.withdrawn_venues` and `self.reinstated_venues`, never the
    /// cell's gate string alone.
    #[allow(clippy::type_complexity)]
    fn attribute_refusals(
        &self,
        report: &CellReport,
        now: Timestamp,
    ) -> (
        Vec<FeasibilityRefusal>,
        Vec<(String, String)>,
        Vec<(String, String)>,
    ) {
        let known = self.known_venues(now);
        let mut admitted = Vec::new();
        let mut unattributed = Vec::new();
        let mut repeated = Vec::new();
        let mut seated: BTreeSet<(&str, &str)> = BTreeSet::new();
        for refusal in &report.refusals {
            let Some(venue) = &refusal.venue else {
                continue;
            };
            let gate_known = EDGE_GATES.contains(&refusal.gate.as_str());
            let venue_known = known.contains(venue);
            let echo = is_withdrawal_echo(&refusal.gate, venue, &self.withdrawn_venues);
            // A cell that is behind on this plane's own reinstatement: the
            // gate is the withdrawn-venue gate, the centre no longer holds
            // the withdrawal, and the centre is the reason the cell still
            // thinks otherwise. Seated as a denominator entry and never as
            // evidence about the venue — see `reinstated_venues`.
            let stale_withdrawal = refusal.gate == GATE_WITHDRAWN_VENUE
                && self.reinstated_venues.contains(venue.as_str());
            if gate_known && venue_known && !seated.insert((venue.as_str(), refusal.gate.as_str()))
            {
                // The second and every later refusal at the same venue under
                // the same gate in one report. One report is one cell's
                // observation at one instant, and repeating a gate once per
                // intent in a pass's fan-out says nothing further about the
                // venue than the first did. Counted under its real venue and
                // its real gate, so an operator sees the rate, and given no
                // window seat.
                //
                // **This cap used to apply to the withdrawn-venue gate
                // alone, and the hole that left was not a corner case.** One
                // report carrying thirty refusals at one venue, plus a
                // single refusal from a second name to satisfy
                // `venue_review::VENUE_WITHDRAWAL_MIN_CELLS`, withdrew that
                // venue for the whole platform on a wire
                // `qip-edge/src/mesh.rs` says authenticates nobody — a probe
                // did exactly that in two messages. The corroboration bar
                // counts distinct cells and not evidence per cell, so it can
                // only mean something if one report cannot be a window.
                // Seats per report are now bounded by the gate vocabulary
                // (nine literals) times the configured venue list, and by
                // nothing a sender chooses.
                repeated.push((venue.clone(), refusal.gate.clone()));
            } else if gate_known && venue_known {
                admitted.push(FeasibilityRefusal {
                    venue: venue.clone(),
                    constraint: refusal.gate.clone(),
                    seam: FeasibilitySeam::Edge,
                    cell: Some(report.cell.clone()),
                    at: report.at,
                    // The classification, taken here and carried on the
                    // entry. `venue_review::assess` used to ask the same
                    // question of whatever the withdrawn set held when it
                    // ran, so a reinstatement turned every seat this venue
                    // had earned as an echo into a full refusal against it.
                    // Whether this was the platform quoting itself, or the
                    // wake of the centre's own reinstatement arriving from a
                    // cell that has not heard it yet, is a fact about the
                    // instant it arrived and is settled here.
                    standing: if echo {
                        RefusalStanding::Echo
                    } else if stale_withdrawal {
                        RefusalStanding::Pardoned
                    } else {
                        RefusalStanding::Evidence
                    },
                });
            } else {
                unattributed.push((
                    if venue_known {
                        venue.clone()
                    } else {
                        UNKNOWN_VENUE.to_string()
                    },
                    if gate_known {
                        refusal.gate.clone()
                    } else {
                        OTHER_CONSTRAINT.to_string()
                    },
                ));
            }
        }
        (admitted, unattributed, repeated)
    }

    /// Every venue the centre has told a cell it may trade at: the arbitrage
    /// policy's venues and every venue on a grant live at `now`. The bound
    /// on the `venue` label of a carried refusal.
    fn known_venues(&self, now: Timestamp) -> BTreeSet<String> {
        let mut known: BTreeSet<String> = self
            .config
            .arbitrage
            .as_ref()
            .map(|policy| policy.venues.keys().cloned().collect())
            .unwrap_or_default();
        for envelope in self.envelopes.values() {
            if envelope.is_live(now) {
                known.extend(
                    envelope
                        .venues()
                        .iter()
                        .map(|venue| venue.as_str().to_string()),
                );
            }
        }
        known
    }

    /// Register the interval's orders as sent, bill its fills to their
    /// contributors and settle the crosses, exactly, or say which entries
    /// could not be.
    ///
    /// **Orders bill nothing.** A `DeltaOrder` is what the cell sent and the
    /// venue accepted; it is registered under its cell and order id, counted
    /// under `qip_central_orders_sent_total`, and otherwise left alone. For
    /// one slice this function read the order list as a fill list and
    /// attributed, charged and settled orders that were still resting or
    /// had expired unfilled. A report carrying orders and no fills settles
    /// nothing, and that is a cell with open orders, not a break.
    ///
    /// **A fill is billed as the cell attributed it.** The shares on a
    /// `FillRecord` are the cell's pro-rata split of what the venue reported
    /// traded, and they sum to the fill or the fill is refused — the centre
    /// does not re-split on a vector the fill no longer carries, and it does
    /// not book a fill whose parts do not add up to the whole. Each share
    /// moves one strategy's lot at the fill price.
    ///
    /// **A fill must name an order the centre saw sent**, in this report or
    /// an earlier one, with enough quantity still unfilled to cover it. One
    /// that does not is a [`BreakOrigin::UnsentFill`] break: a venue claim
    /// with no order of the platform's behind it, which the caller halts
    /// the cell on exactly as it halts on a break the cell shipped. It is
    /// not booked and not charged, because a position the centre cannot
    /// trace to an order is a position nobody authorised.
    ///
    /// A cross is settled only where its size per strategy is determinable:
    /// one buyer and one seller, each moved by the crossed quantity at the
    /// mid the cell recorded, the buyer up and the seller down. A cross
    /// naming two on a side carries no per-strategy size on the wire, and an
    /// even split would be a guess in the one record §27.1 calls a
    /// regulatory expectation; it is refused, counted and reported.
    ///
    /// The decomposition is checked, not assumed. The independent total is
    /// what the books say each touched lot gained at the trade's mark; the
    /// attributor rebuilds it from the periods and refuses if the two do not
    /// close to the last unit. A refusal here is counted under
    /// `qip_central_attribution_failures_total`, which must stay at zero.
    fn settle(&mut self, report: &CellReport, now: Timestamp) -> Settlement {
        let mut settlement = Settlement::default();
        let mut periods: Vec<PositionPeriod> = Vec::new();
        let mut total = Decimal::ZERO;

        // Orders first, so a fill in the same report as its order matches.
        for order in &report.orders {
            if !order.quantity.is_positive() {
                self.refuse_settlement(
                    &mut settlement,
                    "order",
                    format!(
                        "order {} was reported sent for {}; a sent order needs a positive \
                         quantity to be matched against",
                        order.order_id, order.quantity
                    ),
                );
                continue;
            }
            let sent = self.sent.entry(report.cell.clone()).or_default();
            if let Err(reason) = sent.register(&order.order_id, order.quantity, order.price) {
                self.refuse_settlement(&mut settlement, "order", reason);
                continue;
            }
            settlement.orders_sent += 1;
            if let Some(metrics) = &self.metrics {
                metrics.count(names::CENTRAL_ORDERS_SENT, labels([]));
            }
        }

        for fill in &report.fills {
            if !fill.quantity.is_positive() || !fill.price.is_positive() {
                self.refuse_settlement(
                    &mut settlement,
                    "fill",
                    format!(
                        "fill on order {} has quantity {} at price {}; a fill needs both positive",
                        fill.order_id, fill.quantity, fill.price
                    ),
                );
                continue;
            }
            let sent = self.sent.entry(report.cell.clone()).or_default();
            let reference = match sent.fill(&fill.order_id, fill.quantity) {
                Ok(reference) => reference,
                Err(detail) => {
                    // Not refused: refused is for a record the books cannot take
                    // without guessing. This is a record the platform has no order
                    // behind, and the response to that is the halt, not a line in
                    // a list of refusals nobody pages on.
                    settlement.breaks.push(ReconciliationBreak {
                        instrument: fill.object_id.as_str().to_string(),
                        cell_quantity: Decimal::ZERO,
                        external_quantity: fill.quantity,
                        detail,
                        origin: BreakOrigin::UnsentFill,
                    });
                    continue;
                }
            };
            let shared: Decimal = fill.shares.iter().map(|share| share.quantity).sum();
            if fill.shares.is_empty()
                || fill
                    .shares
                    .iter()
                    .any(|share| !share.quantity.is_positive())
                || shared != fill.quantity
            {
                self.refuse_settlement(
                    &mut settlement,
                    "fill",
                    format!(
                        "fill of {} on order {} carries {} share(s) summing to {}; the shares \
                         must be positive and sum to the fill exactly, or the difference is a \
                         quantity nobody is attributed",
                        fill.quantity,
                        fill.order_id,
                        fill.shares.len(),
                        shared
                    ),
                );
                continue;
            }
            let direction = order_direction(fill.side);
            // Money is `Decimal` up to this line. Cost in basis points is a
            // statistic the kill condition compares against a modelled figure
            // in `f64`, and this is where the two prices cross into that
            // arithmetic. Signed so that paying away is positive: a buy
            // filled above the price the platform sent, or a sell filled
            // below it, costs; the other direction is price improvement and
            // is recorded as the negative it is rather than floored at zero,
            // because a mean that cannot go below zero would read every
            // improvement as break-even and bias the series toward the
            // overrun this measurement exists to catch.
            //
            // A non-positive reference is skipped rather than divided by. The
            // register only ever holds an order the settle path admitted, and
            // that path already refuses a non-positive quantity, but the
            // division is guarded where it happens rather than at a distance.
            if reference.is_positive() {
                let slippage = direction * (fill.price - reference) / reference;
                let cost_bps = slippage.to_f64() * 10_000.0;
                for share in &fill.shares {
                    let entry = settlement
                        .cost
                        .entry(share.strategy.as_str().to_string())
                        .or_default();
                    entry.weighted += cost_bps * share.quantity.to_f64();
                    entry.quantity += share.quantity.to_f64();
                }
            }
            for share in &fill.shares {
                let (period, gained) = self.book(
                    &report.cell,
                    &share.strategy,
                    fill.object_id.as_str(),
                    direction * share.quantity,
                    fill.price,
                );
                periods.push(period);
                total += gained;
                settlement.fills_attributed += 1;
                if let Some(metrics) = &self.metrics {
                    metrics.count(
                        names::CENTRAL_FILLS_ATTRIBUTED,
                        labels([("basis", "contributor_vector")]),
                    );
                }
            }
            settlement.fills_settled += 1;
            settlement.absorbed.push(AbsorbedFill {
                object_id: fill.object_id.as_str().to_string(),
                signed_notional: direction * fill.quantity * fill.price,
                // The venue the cell reported the fill from, taken here at the
                // line that counts the fill settled rather than re-read from
                // the report later: what the aggregate is charged and what the
                // settlement says it absorbed stay one list.
                venue: fill.venue.as_str().to_string(),
            });
        }

        for cross in &report.crosses {
            if cross.bought.len() != 1 || cross.sold.len() != 1 {
                self.refuse_settlement(
                    &mut settlement,
                    "cross",
                    format!(
                        "the cross of {} {} at {} names {} buyer(s) and {} seller(s); the wire \
                         carries no per-strategy size, and splitting it evenly would be a guess",
                        cross.quantity,
                        cross.object_id,
                        cross.price,
                        cross.bought.len(),
                        cross.sold.len()
                    ),
                );
                continue;
            }
            if !cross.quantity.is_positive() || !cross.price.is_positive() {
                self.refuse_settlement(
                    &mut settlement,
                    "cross",
                    format!(
                        "the cross of {} {} at {} needs a positive quantity and a positive mid",
                        cross.quantity, cross.object_id, cross.price
                    ),
                );
                continue;
            }
            let instrument = cross.object_id.as_str();
            let (bought, gained) = self.book(
                &report.cell,
                &cross.bought[0],
                instrument,
                cross.quantity,
                cross.price,
            );
            periods.push(bought);
            total += gained;
            let (sold, gained) = self.book(
                &report.cell,
                &cross.sold[0],
                instrument,
                -cross.quantity,
                cross.price,
            );
            periods.push(sold);
            total += gained;
            settlement.fills_attributed += 2;
            settlement.crosses_settled += 1;
            if let Some(metrics) = &self.metrics {
                metrics.count(names::CENTRAL_CROSSES_SETTLED, labels([]));
            }
        }

        if periods.is_empty() {
            return settlement;
        }
        match self
            .attributor
            .attribute(&periods, total, Decimal::ZERO, now, now)
        {
            Ok(attribution) => settlement.attribution = Some(attribution),
            Err(error) => {
                if let Some(metrics) = &self.metrics {
                    metrics.count(names::CENTRAL_ATTRIBUTION_FAILURES, labels([]));
                }
                settlement.refused.push(format!(
                    "the settlement's decomposition did not close: {}",
                    error.message()
                ));
            }
        }
        settlement
    }

    /// Move one strategy's lot and write the period the attribution grades.
    ///
    /// Returns the period and what the lot gained at the trade's mark — the
    /// independent figure the attributor's decomposition must close to.
    fn book(
        &mut self,
        cell: &str,
        strategy: &StrategyId,
        instrument: &str,
        signed: Decimal,
        price: Decimal,
    ) -> (PositionPeriod, Decimal) {
        let lot = self
            .books
            .entry((cell.to_string(), strategy.clone(), instrument.to_string()))
            .or_default();
        let before = lot.apply(signed, price);
        let after = *lot;
        // Marked at the trade price: what the lot is worth now, less what it
        // was carried at, less what was paid or received for the trade.
        let gained =
            after.quantity * price - before.quantity * before.average_price - signed * price;
        let period = PositionPeriod {
            object_id: format!("{cell}/{strategy}/{instrument}"),
            hypotheses: vec![strategy.as_str().to_string()],
            opening_quantity: before.quantity,
            opening_price: before.average_price,
            closing_quantity: after.quantity,
            closing_price: price,
            decision_price: price,
            traded_quantity: signed,
            traded_price: price,
            // The wire carries no costs for a cell's order, and none are
            // invented: a commission the centre guessed would be exactly the
            // unexplained line the exact decomposition exists to refuse.
            //
            // This is not in tension with the execution cost
            // `Settlement::cost_bps_by_strategy` measures, and the two must
            // not be confused. That one is a *statistic* — how far a fill
            // landed from the price the platform sent, which two prices on
            // the wire do support — read by a kill condition. These are
            // *money*, and they have to sum to the P&L exactly. A spread cost
            // derived from the same difference would look plausible here and
            // would be an unattributed figure in a decomposition that admits
            // none, so it stays zero until a venue reports the money.
            commission: Decimal::ZERO,
            spread_cost: Decimal::ZERO,
            impact_cost: Decimal::ZERO,
            income: Decimal::ZERO,
            financing: Decimal::ZERO,
            realised_pnl: Decimal::ZERO,
            factor_returns: BTreeMap::new(),
            factor_betas: BTreeMap::new(),
            contract_multiplier: Decimal::ONE,
        };
        (period, gained)
    }

    fn refuse_settlement(&self, settlement: &mut Settlement, kind: &str, reason: String) {
        if let Some(metrics) = &self.metrics {
            metrics.count(names::CENTRAL_SETTLEMENTS_REFUSED, labels([("kind", kind)]));
        }
        settlement.refused.push(reason);
    }

    /// Count each break by direction and the halt by its one cause.
    fn record_halt(&self, breaks: &[ReconciliationBreak]) {
        let Some(metrics) = &self.metrics else {
            return;
        };
        for reconciliation_break in breaks {
            metrics.count(
                names::CENTRAL_RECONCILIATION_BREAKS,
                labels([("direction", reconciliation_break.direction().as_str())]),
            );
        }
        metrics.count(
            names::CENTRAL_CELL_HALTS,
            labels([("cause", "reconciliation")]),
        );
    }

    /// Record the incident a reconciliation break is, and apply the policy.
    fn halt_cell(
        &mut self,
        cell: &str,
        breaks: &[ReconciliationBreak],
        now: Timestamp,
    ) -> Result<HaltScope> {
        self.incidents_raised += 1;
        let summary = format!(
            "{} record(s) reported by {} do not reconcile with the venue; every limit that \
             cell checks locally is being checked against a book that is wrong",
            breaks.len(),
            cell
        );
        let incident = Incident::new(
            format!("inc-reconciliation-{}-{}", cell, self.incidents_raised),
            now,
            Severity::Cell,
            "central-plane",
            summary,
            None,
            Some(cell.to_string()),
        )?;
        Ok(self.compliance.incidents_mut().record(incident))
    }

    /// Turn concentration findings into recalls.
    ///
    /// One recall per (cell, strategy) grant that contributes to a breach, at
    /// most once per ingestion however many axes name the same cell. A recall
    /// is a request — the reliable bound is the envelope's expiry, which the
    /// cell enforces against its own clock — so the order carries that expiry
    /// as its backstop, taken from the grant rather than restated here.
    fn recall_for(
        &mut self,
        findings: &[ConcentrationFinding],
        now: Timestamp,
    ) -> Result<Vec<RecallOrder>> {
        let mut targeted: Vec<(String, StrategyId, CapitalEnvelope, String)> = Vec::new();
        let mut seen: BTreeSet<(String, StrategyId)> = BTreeSet::new();
        for finding in findings {
            for cell in self.cells_behind(finding) {
                for ((held_cell, strategy), envelope) in &self.envelopes {
                    if held_cell != &cell || !envelope.is_live(now) {
                        continue;
                    }
                    let key = (held_cell.clone(), strategy.clone());
                    if !seen.insert(key) {
                        continue;
                    }
                    targeted.push((
                        held_cell.clone(),
                        strategy.clone(),
                        envelope.clone(),
                        finding.describe(),
                    ));
                }
            }
        }

        let mut orders = Vec::new();
        for (_, _, envelope, detail) in targeted {
            orders.push(self.recalls.issue(
                &envelope,
                RecallReason::RiskReduction,
                detail,
                self.config.recall_acknowledgement,
                now,
            )?);
        }
        Ok(orders)
    }

    /// The cells contributing to one finding.
    ///
    /// Derived from the reported positions rather than from the exposure
    /// aggregate, because the aggregate has already netted the axis away and
    /// a recall has to name a cell.
    fn cells_behind(&self, finding: &ConcentrationFinding) -> Vec<String> {
        if finding.axis == "cell" {
            return vec![finding.bucket.clone()];
        }
        let mut cells: BTreeSet<String> = BTreeSet::new();
        for position in self.positions.values().flatten() {
            let matches = match finding.axis {
                "instrument" => position.instrument == finding.bucket,
                "sector" => position.sector.as_str() == finding.bucket,
                "venue" => position.venue.as_str() == finding.bucket,
                "currency" => position.currency.as_str() == finding.bucket,
                // A new axis in `qip_capital::exposure` reaches here naming no
                // cells rather than silently recalling everything. The finding
                // is still reported; only the automatic recall waits for
                // somebody to say which cells it implicates.
                _ => false,
            };
            if matches {
                cells.insert(position.cell.clone());
            }
        }
        cells.into_iter().collect()
    }
}

/// How many sent orders the centre remembers per cell.
///
/// An order leaves the register when its fills sum to what was sent; a
/// partially filled or expired one stays until it is the oldest of this
/// many. The bound is generous because the cost of eviction is stated and
/// severe: a fill arriving for an evicted order is an unsent-fill break and
/// halts the cell. That is the fail-closed direction — a fill the centre
/// cannot trace is refused rather than believed — and it is much better
/// than a register that grows with every order a cell ever sent.
const MAX_SENT_ORDERS_PER_CELL: usize = 4_096;

/// One order the centre saw a cell send, and how much of it has filled.
#[derive(Clone, Copy, Debug, PartialEq)]
struct SentOrder {
    quantity: Decimal,
    filled: Decimal,
    /// The price the cell sent the order at, kept as the reference every
    /// fill on it is costed against.
    ///
    /// Kept here rather than re-read from the report because a fill arrives
    /// in a later interval than its order as often as not, and by then the
    /// report that carried the send is gone. It is the price the platform
    /// *asked for* — under `PricingPolicy::RestAtMid` the mid the cell rested
    /// at — and not a decision-time arrival mid the wire does not carry. What
    /// that makes measurable is stated on `Settlement::cost_bps_by_strategy`.
    price: Decimal,
}

/// The orders one cell has reported sent, keyed by order id, bounded.
///
/// The map is what a fill is matched against; the deque is the eviction
/// order, oldest first. Both are kept rather than one because a `BTreeMap`
/// orders by id and the id carries no age.
#[derive(Clone, Debug, Default, PartialEq)]
struct SentOrders {
    by_id: BTreeMap<String, SentOrder>,
    arrival: VecDeque<String>,
}

impl SentOrders {
    /// Record an order as sent, or say why it cannot be.
    ///
    /// The same id reported sent twice is refused rather than summed: an
    /// order id is the key a fill is matched under, and two sends behind one
    /// key would make the register's quantity a number neither send said.
    fn register(
        &mut self,
        order_id: &str,
        quantity: Decimal,
        price: Decimal,
    ) -> std::result::Result<(), String> {
        if self.by_id.contains_key(order_id) {
            return Err(format!(
                "order {order_id} was reported sent twice; the first send is kept and this one \
                 is not added to it, because two sends under one id cannot be matched to fills"
            ));
        }
        self.by_id.insert(
            order_id.to_string(),
            SentOrder {
                quantity,
                filled: Decimal::ZERO,
                price,
            },
        );
        self.arrival.push_back(order_id.to_string());
        while self.by_id.len() > MAX_SENT_ORDERS_PER_CELL {
            let Some(oldest) = self.arrival.pop_front() else {
                break;
            };
            self.by_id.remove(&oldest);
        }
        Ok(())
    }

    /// Match a fill against the order it names, or describe why it cannot be.
    ///
    /// An order whose fills now sum to its quantity leaves the register, so
    /// a fill after that is an unsent fill like any other — the venue
    /// reporting more than the platform asked for.
    fn fill(&mut self, order_id: &str, quantity: Decimal) -> std::result::Result<Decimal, String> {
        let Some(order) = self.by_id.get_mut(order_id) else {
            return Err(format!(
                "the cell reports a fill of {quantity} on order {order_id} and the centre never \
                 saw that order sent, in this report or any it retains"
            ));
        };
        let remaining = order.quantity - order.filled;
        if quantity > remaining {
            return Err(format!(
                "the cell reports a fill of {quantity} on order {order_id}, which was sent for {} \
                 and has {remaining} unfilled; the excess was never sent",
                order.quantity
            ));
        }
        order.filled += quantity;
        let reference = order.price;
        if order.filled >= order.quantity {
            self.by_id.remove(order_id);
        }
        // Returned rather than left for the caller to look up, because the
        // order is gone from the register on the fill that completes it and a
        // second lookup would find nothing exactly on the fills that matter
        // most — the ones that closed an order out.
        Ok(reference)
    }
}

/// The signed direction of an order from the side of the book it takes.
///
/// A buy lifts the offer, so the cell records it against the ask — the same
/// convention `qip_kernel::platform` writes its own placements with, and the
/// one the contributor vector's sign follows: positive is a buy. Matched
/// exhaustively so a third side would fail to compile here rather than fall
/// through to a direction.
const fn order_direction(side: BookSide) -> Decimal {
    match side {
        BookSide::Ask => Decimal::ONE,
        BookSide::Bid => Decimal::NEG_ONE,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use qip_capital::exposure::CellPosition;
    use qip_contracts::venue::VenueId;
    use qip_core::{Currency, dec};
    use qip_financial::asset_class::Sector;
    use qip_risk_engine::autonomy::AutonomyController;

    const CELL: &str = "cell-lon-1";

    fn now() -> Timestamp {
        Timestamp::from_secs(1_760_000_000)
    }

    fn plane_with_metrics() -> Result<(CentralPlane, Arc<Metrics>)> {
        let mut plane = CentralPlane::new(&[7u8; 32], CentralConfig::default())?;
        let metrics = Arc::new(Metrics::new("test"));
        plane.attach_metrics(Arc::clone(&metrics));
        Ok((plane, metrics))
    }

    /// A live grant at the cell, inserted directly: the point of this module's
    /// tests is what `ingest` does after the trip, and the ladder that would
    /// normally issue the grant is somebody else's test.
    fn live_grant(plane: &mut CentralPlane, strategy: &StrategyId) -> Result<()> {
        let envelope = CapitalEnvelope::new(
            strategy.clone(),
            CELL,
            dec!("500000"),
            dec!("50000"),
            dec!("50000"),
            vec![VenueId::new("XNYS")],
            now(),
            now().saturating_add(Duration::from_hours(8)),
            "alice.chen",
            "not-verified-here",
        )?;
        plane
            .envelopes
            .insert((CELL.to_string(), strategy.clone()), envelope);
        Ok(())
    }

    /// The refusals `qip-edge-node`'s `graph_from_whitelist` and
    /// `sizes_from_whitelist` make, mirrored here because this crate cannot
    /// depend on the node. If the node grows a refusal this does not mirror,
    /// the producer can emit what the cell refuses, and this test stops
    /// proving what its name says — so the list is the node's, in its order.
    fn cell_would_accept(
        whitelist: &CycleWhitelist,
        venues: &[VenueId],
    ) -> std::result::Result<(), String> {
        const MAX_CONVERSIONS: usize = 256;
        if whitelist.conversions.is_empty() {
            return Err("no conversion".to_string());
        }
        if whitelist.conversions.len() > MAX_CONVERSIONS {
            return Err("too many conversions".to_string());
        }
        let mut classes = BTreeMap::new();
        for (position, conversion) in whitelist.conversions.iter().enumerate() {
            let venue = VenueId::new(conversion.venue.as_str());
            if !venues.contains(&venue) {
                return Err(format!(
                    "conversion {position} names a venue the cell may not trade"
                ));
            }
            if conversion.from == conversion.to {
                return Err(format!("conversion {position} converts into itself"));
            }
            if conversion.cost_fraction.is_negative() || conversion.cost_fraction >= Decimal::ONE {
                return Err(format!("conversion {position} has a cost outside [0, 1)"));
            }
            if let Some(previous) = classes.insert(venue, conversion.venue_class)
                && previous != conversion.venue_class
            {
                return Err(format!("conversion {position} reclassifies its venue"));
            }
            if !whitelist.start_sizes.contains_key(&conversion.from) {
                return Err(format!(
                    "conversion {position} leaves {} unsized",
                    conversion.from
                ));
            }
        }
        for (object, size) in &whitelist.start_sizes {
            if !size.is_positive() {
                return Err(format!("{object} has a non-positive size"));
            }
        }
        Ok(())
    }

    /// A plane with a two-venue policy and a grant permitting both, so a
    /// withdrawal of one venue has a survivor to keep.
    fn two_venue_plane() -> (CentralPlane, StrategyId, Vec<VenueId>) {
        use super::super::whitelist::{ArbitragePolicy, WhitelistedMarket, WhitelistedVenue};
        use qip_contracts::venue::VenueClass;

        let desk = StrategyId::new("arb-desk");
        let venue = |cost| WhitelistedVenue {
            class: VenueClass::Exchange,
            taker_cost: cost,
        };
        let market = |venue: &str| WhitelistedMarket {
            venue: venue.to_string(),
            market: format!("AAA-USD@{venue}"),
            base: "AAA".to_string(),
            quote: "USD".to_string(),
        };
        let config = CentralConfig {
            arbitrage: Some(ArbitragePolicy {
                strategy: desk.clone(),
                funding_instrument: "USD".to_string(),
                venues: BTreeMap::from([
                    ("XNYS".to_string(), venue(dec!("0.0005"))),
                    ("XLON".to_string(), venue(dec!("0.001"))),
                ]),
                markets: vec![market("XNYS"), market("XLON")],
                start_sizes: BTreeMap::from([("AAA".to_string(), dec!("100"))]),
            }),
            ..CentralConfig::default()
        };
        let mut plane = CentralPlane::new(&[7u8; 32], config).expect("the policy is valid");
        let cell_venues = vec![VenueId::new("XNYS"), VenueId::new("XLON")];
        let envelope = CapitalEnvelope::new(
            desk.clone(),
            CELL,
            dec!("500000"),
            dec!("25000"),
            dec!("50000"),
            cell_venues.clone(),
            now(),
            now().saturating_add(Duration::from_hours(8)),
            "alice.chen",
            "sig-arb-desk",
        )
        .expect("a well-formed grant");
        plane
            .envelopes
            .insert((CELL.to_string(), desk.clone()), envelope);
        (plane, desk, cell_venues)
    }

    #[test]
    fn a_withdrawn_venue_is_omitted_from_the_whitelist_and_the_omission_is_on_the_record() {
        // Blueprint §12.3's fourth row at the edge seam. A withdrawal is an
        // omission from what the policy already produced — the conversions
        // naming the venue are dropped, the survivor's are kept, and the
        // journaled outcome names what was omitted so an operator reading
        // "two edges" does not have to infer a withdrawal from an edge count.
        // When nothing survives the outcome says so in words: the cell's
        // installer refuses "no conversion" and installs nothing, which is
        // the fail-closed answer.
        let (mut plane, _desk, cell_venues) = two_venue_plane();
        // Premise: both venues emit, nothing withdrawn.
        let before = plane
            .cycle_whitelist_for(CELL, now())
            .expect("two permitted venues emit");
        assert!(matches!(
            &before.outcome,
            WhitelistOutcome::Emitted { edges: 4, withdrawn, .. } if withdrawn.is_empty()
        ));
        assert!(
            before
                .whitelist
                .conversions
                .iter()
                .any(|conversion| conversion.venue == "XLON")
        );

        plane.withdraw_venue("XLON");
        let one = plane
            .cycle_whitelist_for(CELL, now())
            .expect("a whitelist with a survivor emits");
        assert_eq!(
            one.outcome,
            WhitelistOutcome::Emitted {
                edges: 2,
                sized_against: "sig-arb-desk".to_string(),
                withdrawn: vec!["XLON".to_string()],
            },
            "{}",
            one.describe()
        );
        assert!(
            one.whitelist
                .conversions
                .iter()
                .all(|conversion| conversion.venue == "XNYS"),
            "a conversion still names the withdrawn venue: {:?}",
            one.whitelist.conversions
        );
        assert_eq!(one.whitelist.conversions.len(), 2);
        if let Err(reason) = cell_would_accept(&one.whitelist, &cell_venues) {
            panic!("the cell would refuse the narrowed whitelist: {reason}");
        }
        assert!(
            one.describe().contains("omitted as withdrawn (XLON)"),
            "{}",
            one.describe()
        );

        plane.withdraw_venue("XNYS");
        let none = plane
            .cycle_whitelist_for(CELL, now())
            .expect("an all-withdrawn policy is an empty whitelist, not an error");
        assert_eq!(
            none.outcome,
            WhitelistOutcome::AllWithdrawn {
                venues: vec!["XLON".to_string(), "XNYS".to_string()],
            },
            "{}",
            none.describe()
        );
        assert!(none.is_empty());
        assert_eq!(
            cell_would_accept(&none.whitelist, &cell_venues),
            Err("no conversion".to_string()),
            "the cell would install something from an all-withdrawn whitelist"
        );

        // Reinstating restores only what the policy and the grant already
        // permitted: XLON comes back with its own cost and nothing else.
        assert!(plane.reinstate_venue("XLON"));
        let back = plane
            .cycle_whitelist_for(CELL, now())
            .expect("a reinstated venue emits");
        assert!(matches!(
            &back.outcome,
            WhitelistOutcome::Emitted { edges: 2, withdrawn, .. } if withdrawn == &["XNYS".to_string()]
        ));
        assert!(
            back.whitelist
                .conversions
                .iter()
                .all(|conversion| conversion.venue == "XLON")
        );
    }

    /// Slot 8 shipped unproduced from every payload because nothing in the
    /// centre produced it, so the desk the edge node could install from it
    /// installed never. The plane now derives it from the operator's policy
    /// and the desk's live grant, and what it derives is what the cell's
    /// `graph_from_whitelist` accepts.
    #[test]
    fn a_plane_with_two_venues_and_a_pair_set_emits_a_whitelist_the_cell_would_accept() {
        use super::super::whitelist::{ArbitragePolicy, WhitelistedMarket, WhitelistedVenue};
        use qip_contracts::venue::VenueClass;

        let desk = StrategyId::new("arb-desk");
        let venue = |class, cost| WhitelistedVenue {
            class,
            taker_cost: cost,
        };
        let market = |venue: &str| WhitelistedMarket {
            venue: venue.to_string(),
            market: format!("AAA-USD@{venue}"),
            base: "AAA".to_string(),
            quote: "USD".to_string(),
        };
        let config = CentralConfig {
            arbitrage: Some(ArbitragePolicy {
                strategy: desk.clone(),
                funding_instrument: "USD".to_string(),
                venues: BTreeMap::from([
                    (
                        "XNYS".to_string(),
                        venue(VenueClass::Exchange, dec!("0.0005")),
                    ),
                    (
                        "XLON".to_string(),
                        venue(VenueClass::Exchange, dec!("0.001")),
                    ),
                ]),
                markets: vec![market("XNYS"), market("XLON")],
                start_sizes: BTreeMap::from([("AAA".to_string(), dec!("100"))]),
            }),
            ..CentralConfig::default()
        };
        let mut plane = CentralPlane::new(&[7u8; 32], config).expect("the policy is valid");
        let cell_venues = vec![VenueId::new("XNYS"), VenueId::new("XLON")];
        let envelope = CapitalEnvelope::new(
            desk.clone(),
            CELL,
            dec!("500000"),
            dec!("25000"),
            dec!("50000"),
            cell_venues.clone(),
            now(),
            now().saturating_add(Duration::from_hours(8)),
            "alice.chen",
            "sig-arb-desk",
        )
        .expect("a well-formed grant");
        plane
            .envelopes
            .insert((CELL.to_string(), desk.clone()), envelope);

        let issue = plane
            .cycle_whitelist_for(CELL, now())
            .expect("two permitted venues emit");
        // Premise: something was emitted, and it says what it was sized by.
        assert_eq!(
            issue.outcome,
            WhitelistOutcome::Emitted {
                edges: 4,
                sized_against: "sig-arb-desk".to_string(),
                withdrawn: Vec::new(),
            },
            "{}",
            issue.describe()
        );
        assert!(!issue.is_empty());
        if let Err(reason) = cell_would_accept(&issue.whitelist, &cell_venues) {
            panic!("the cell would refuse: {reason}");
        }
        // The funding size is the grant's order limit, and the grant alone
        // decides it: the policy carried no size for USD.
        assert_eq!(
            issue.whitelist.start_sizes.get("USD"),
            Some(&dec!("25000")),
            "the funding instrument is sized by the grant"
        );
        // Both venues reach the whitelist with their own cost.
        let costs: BTreeMap<&str, Decimal> = issue
            .whitelist
            .conversions
            .iter()
            .map(|conversion| (conversion.venue.as_str(), conversion.cost_fraction))
            .collect();
        assert_eq!(costs.get("XNYS"), Some(&dec!("0.0005")));
        assert_eq!(costs.get("XLON"), Some(&dec!("0.001")));

        // The same grant expired is no grant: the desk cannot be sized, and
        // the whitelist says so instead of shipping stale sizes.
        let later = now().saturating_add(Duration::from_hours(9));
        let expired = plane
            .cycle_whitelist_for(CELL, later)
            .expect("an expired grant is an empty whitelist, not an error");
        assert_eq!(
            expired.outcome,
            WhitelistOutcome::NoLiveGrant {
                strategy: desk.clone()
            }
        );
        assert!(expired.is_empty());
    }

    fn position(strategy: &StrategyId) -> CellPosition {
        CellPosition {
            cell: CELL.to_string(),
            strategy: strategy.clone(),
            instrument: "AAA".to_string(),
            sector: Sector::InformationTechnology,
            venue: VenueId::new("XNYS"),
            currency: Currency::USD,
            quantity: dec!("10"),
            price: dec!("100"),
        }
    }

    /// A reconciliation break tripped the cell's kill switch and raised an
    /// incident, and then the same ingestion refused — the recall window was
    /// zero — so the error propagated out of `ingest` and the caller, which
    /// counted on the returned ingestion, counted nothing. A halt that had
    /// fired and an incident that had been raised left no series behind
    /// them: the exact class the counters exist to close, reopened by a
    /// configuration value. The constructor now refuses that value, so this
    /// test reaches past it — the property is that nothing after the trip,
    /// whatever its cause, can un-count a halt that happened.
    #[test]
    fn a_halt_is_counted_even_when_the_same_ingestion_then_refuses() {
        let (mut plane, metrics) = plane_with_metrics().expect("a default plane assembles");
        let strategy = StrategyId::new("momentum-lon");
        live_grant(&mut plane, &strategy).expect("a live grant is well formed");
        plane.config.recall_acknowledgement = Duration::ZERO;
        let mut autonomy = AutonomyController::new();

        // One position is the whole book on every axis, so the report breaches
        // the per-cell share and targets the live grant for a recall.
        let report = CellReport::new(CELL, now())
            .with_positions(vec![position(&strategy)])
            .with_break(ReconciliationBreak {
                instrument: "AAA".to_string(),
                cell_quantity: dec!("10"),
                external_quantity: dec!("4"),
                detail: "six lots the venue has no record of".to_string(),
                origin: BreakOrigin::Book,
            });
        let outcome = plane.ingest(report, autonomy.kill_switch_mut(), now());

        // Premise: the ingestion really did refuse after the trip, and the
        // cell really was halted, so a count keyed on `Ok` would have missed
        // this halt.
        assert!(
            outcome.is_err(),
            "the zero window should have refused: {outcome:?}"
        );
        let error = outcome
            .err()
            .map(|error| error.to_string())
            .unwrap_or_default();
        assert!(
            error.contains("positive window"),
            "the refusal should be the recall register's: {error}"
        );
        assert!(
            autonomy.kill_switch().is_halted(CELL),
            "the trip happened before the refusal"
        );
        assert!(!plane.may_act(strategy.as_str(), CELL));

        let snapshot = metrics.snapshot();
        assert_eq!(
            snapshot.counter(
                names::CENTRAL_RECONCILIATION_BREAKS,
                &labels([("direction", "cell_over_venue")])
            ),
            1,
            "the break was counted although the ingestion refused"
        );
        assert_eq!(
            snapshot.counter(
                names::CENTRAL_CELL_HALTS,
                &labels([("cause", "reconciliation")])
            ),
            1,
            "the halt was counted although the ingestion refused"
        );
    }
}

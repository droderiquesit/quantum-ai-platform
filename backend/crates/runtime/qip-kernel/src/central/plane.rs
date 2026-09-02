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

use super::dna::StrategyDna;
use super::factory::StrategyFactory;
use super::whitelist::{ArbitragePolicy, WhitelistIssue, WhitelistOutcome};
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
use qip_contracts::governance::{Approval, Severity};
use qip_contracts::message::BookSide;
use qip_contracts::policy::CycleWhitelist;
use qip_contracts::signal::StrategyId;
use qip_contracts::wire::CrossRecord;
use qip_contracts::{CapitalEnvelope, Utilisation};
use qip_core::error::{Error, Result};
use qip_core::{Decimal, Duration, Timestamp};
use qip_learning_engine::attribution::{Attribution, Attributor, PositionPeriod, split_pro_rata};
use qip_mesh::delta::DeltaOrder;
use qip_observability::metrics::{Metrics, labels, names};
use qip_risk_engine::autonomy::KillSwitch;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

/// The key id every signature the central plane makes is recorded under.
///
/// Carried into the envelope issuer, the approval chain and the artifact store
/// so that when asymmetric signing arrives, existing records say which key they
/// were made under. See `qip_compliance::signing` for what this scheme is not.
const CENTRAL_KEY_ID: &str = "central-plane-key";

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
}

impl BreakDirection {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::CellOverVenue => "cell_over_venue",
            Self::VenueOverCell => "venue_over_cell",
            Self::DetailOnly => "detail_only",
        }
    }
}

impl ReconciliationBreak {
    /// Signed gap between the two books.
    pub fn difference(&self) -> Decimal {
        self.cell_quantity - self.external_quantity
    }

    /// The bounded shape of this break, from the sign of [`Self::difference`].
    pub fn direction(&self) -> BreakDirection {
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
    pub at: Timestamp,
    pub positions: Vec<CellPosition>,
    /// What each strategy has committed against its envelope.
    pub utilisation: Vec<(StrategyId, Utilisation)>,
    pub reconciliation_breaks: Vec<ReconciliationBreak>,
    /// Orders the cell sent since its previous report, each carrying the
    /// contributor vector the cell netted it from. Incremental, unlike the
    /// positions above: the centre attributes each one to its contributors'
    /// books and never sees it again. Defaulted so a report written before
    /// the field replays.
    #[serde(default)]
    pub orders: Vec<DeltaOrder>,
    /// Internal crosses the cell booked since its previous report (§27.1).
    /// Incremental for the same reason.
    #[serde(default)]
    pub crosses: Vec<CrossRecord>,
}

impl CellReport {
    pub fn new(cell: impl Into<String>, at: Timestamp) -> Self {
        Self {
            cell: cell.into(),
            at,
            positions: Vec::new(),
            utilisation: Vec::new(),
            reconciliation_breaks: Vec::new(),
            orders: Vec::new(),
            crosses: Vec::new(),
        }
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
}

/// What settling one report's interval to the strategy books produced.
///
/// The centre's half of blueprint §43.4's chain: fill → contributor vector →
/// strategy, pro rata. Every share booked here is a line in the attribution,
/// and the attribution is exact — [`Attribution::residual`] is zero or the
/// settlement is refused and counted, never absorbed.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Settlement {
    /// Contributor shares booked, across every order settled.
    pub fills_attributed: usize,
    pub orders_settled: usize,
    pub crosses_settled: usize,
    /// Orders and crosses the centre would not settle, each with why. A
    /// refusal here is a report that carried something the books cannot
    /// take without guessing — a cross naming two buyers and no sizes, an
    /// order whose contributors are all on the other side.
    pub refused: Vec<String>,
    /// The exact decomposition of everything settled, or `None` where the
    /// report carried nothing to settle.
    pub attribution: Option<Attribution>,
    /// Every venue fill the settlement booked, one entry per order settled,
    /// in report order — what the platform charges into its risk aggregate.
    ///
    /// Recorded at the line that counts the order settled, so what the
    /// aggregate is charged and what the strategy books absorbed are one
    /// list rather than two readings of the report that could disagree.
    /// Crosses are deliberately absent: a cross moves one strategy's lot up
    /// and another's down inside the same cell, so the book's exposure is
    /// unchanged and charging it would be a gross that nobody holds.
    pub absorbed: Vec<AbsorbedFill>,
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
}

impl Settlement {
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
    /// Incidents raised here, so their ids are a deterministic counter rather
    /// than a generated id: a replay of the same reports produces the same
    /// incident record.
    incidents_raised: u64,
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
        // Refused here for the same reason the recall window is: every
        // refusal in `ArbitragePolicy::validate` is one the cell would make
        // when the whitelist arrived, and a plane that carried one would ship
        // a whitelist every few minutes that every cell refused whole, with
        // the reason in a delta stream rather than at start-up.
        if let Some(policy) = &config.arbitrage {
            policy.validate()?;
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
            incidents_raised: 0,
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
        let whitelist = policy.whitelist_for(envelope)?;
        Ok(WhitelistIssue {
            cell: cell.to_string(),
            issued_at: now,
            outcome: WhitelistOutcome::Emitted {
                edges: whitelist.conversions.len(),
                sized_against: envelope.signature().to_string(),
            },
            whitelist,
        })
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

    /// Issue a grant.
    ///
    /// Refuses before it computes anything if the factory does not say the
    /// strategy holds capital. That check is the whole of the connection
    /// between the ladder and the money: a strategy at shadow with flawless
    /// evidence and two willing approvers still gets nothing, because the
    /// stage it stands at is what decides, and the stage is the ledger's to
    /// say.
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

        let absorbed = report.positions.len();
        // Replace rather than merge: the report is the whole of this cell's
        // book, and a stale position that survived a replace would show up in
        // the aggregate as risk nobody holds.
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

        let halted = if report.reconciles() {
            None
        } else {
            Some(self.halt_cell(&report, now)?)
        };
        if halted.is_some() {
            let reason = report
                .reconciliation_breaks
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
                    "{} position(s) at {} do not reconcile with the venue: {reason}",
                    report.reconciliation_breaks.len(),
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
            self.record_halt(&report.reconciliation_breaks);
        }

        let concentrations = self.exposure.concentrations(&self.concentration);
        let crowded = self
            .exposure
            .crowded(self.config.minimum_cells_for_crowding);
        let recalls = self.recall_for(&concentrations, now)?;

        Ok(CellIngestion {
            cell: report.cell,
            positions_absorbed: absorbed,
            halted,
            concentrations,
            crowded,
            recalls,
            settlement,
        })
    }

    /// Attribute the interval's fills to their contributors and settle the
    /// crosses, exactly, or say which entries could not be.
    ///
    /// A fill is attributed to the contributors *on its own side*, pro rata
    /// by the magnitude of their signed size. The contributors on the other
    /// side received their fill in the cross the cell booked before the
    /// order went out — that is what netting is — so crediting them a share
    /// of the venue fill too would fill them twice. A netted order that
    /// crossed nothing has contributors on one side only, where this is the
    /// plain pro-rata split. An order shipped by a cell older than the
    /// contributor vector names none, and is attributed whole to the strategy
    /// the older wire named, counted under its own basis so the two cannot be
    /// mistaken for each other.
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

        for order in &report.orders {
            if !order.quantity.is_positive() || !order.price.is_positive() {
                self.refuse_settlement(
                    &mut settlement,
                    "order",
                    format!(
                        "order {} has quantity {} at price {}; a fill needs both positive",
                        order.order_id, order.quantity, order.price
                    ),
                );
                continue;
            }
            let direction = order_direction(order.side);
            let same_side: Vec<(&StrategyId, Decimal)> = order
                .contributors
                .iter()
                .filter(|contributor| contributor.signed_size.signum() == direction.signum())
                .map(|contributor| (&contributor.strategy, contributor.signed_size.abs()))
                .collect();
            let (basis, shares): (&str, Vec<(StrategyId, Decimal)>) = if same_side.is_empty() {
                if !order.contributors.is_empty() {
                    self.refuse_settlement(
                        &mut settlement,
                        "order",
                        format!(
                            "order {} names {} contributor(s) and none on its own side; a {} \
                             fill cannot be attributed to strategies that intended the opposite",
                            order.order_id,
                            order.contributors.len(),
                            order.side.as_str()
                        ),
                    );
                    continue;
                }
                (
                    "largest_contributor",
                    vec![(order.strategy.clone(), order.quantity)],
                )
            } else {
                let weights: Vec<Decimal> = same_side.iter().map(|(_, size)| *size).collect();
                match split_pro_rata(order.quantity, &weights) {
                    Ok(split) => (
                        "contributor_vector",
                        same_side
                            .iter()
                            .zip(split)
                            .map(|((strategy, _), share)| ((*strategy).clone(), share))
                            .collect(),
                    ),
                    Err(error) => {
                        self.refuse_settlement(
                            &mut settlement,
                            "order",
                            format!(
                                "order {} could not be split across its contributors: {}",
                                order.order_id,
                                error.message()
                            ),
                        );
                        continue;
                    }
                }
            };
            for (strategy, share) in shares {
                let (period, gained) = self.book(
                    &report.cell,
                    &strategy,
                    order.object_id.as_str(),
                    direction * share,
                    order.price,
                );
                periods.push(period);
                total += gained;
                settlement.fills_attributed += 1;
                if let Some(metrics) = &self.metrics {
                    metrics.count(names::CENTRAL_FILLS_ATTRIBUTED, labels([("basis", basis)]));
                }
            }
            settlement.orders_settled += 1;
            settlement.absorbed.push(AbsorbedFill {
                object_id: order.object_id.as_str().to_string(),
                signed_notional: direction * order.quantity * order.price,
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
    fn halt_cell(&mut self, report: &CellReport, now: Timestamp) -> Result<HaltScope> {
        self.incidents_raised += 1;
        let summary = format!(
            "{} position(s) reported by {} do not reconcile with the venue; every limit that \
             cell checks locally is being checked against a book that is wrong",
            report.reconciliation_breaks.len(),
            report.cell
        );
        let incident = Incident::new(
            format!(
                "inc-reconciliation-{}-{}",
                report.cell, self.incidents_raised
            ),
            now,
            Severity::Cell,
            "central-plane",
            summary,
            None,
            Some(report.cell.clone()),
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
                sized_against: "sig-arb-desk".to_string()
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

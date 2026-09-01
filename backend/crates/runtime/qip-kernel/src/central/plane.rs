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
use qip_contracts::signal::StrategyId;
use qip_contracts::{CapitalEnvelope, Utilisation};
use qip_core::error::{Error, Result};
use qip_core::{Decimal, Duration, Timestamp};
use qip_observability::metrics::Metrics;
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
    /// unreachable.
    pub recall_acknowledgement: Duration,
    /// Above this gross limit the approval chain demands two different humans.
    pub dual_approval_threshold: Decimal,
    /// Cells that must independently hold one name for it to count as crowded.
    pub minimum_cells_for_crowding: usize,
    /// Treat every incident as at least this severe.
    pub response_floor: Severity,
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
}

impl CellReport {
    pub fn new(cell: impl Into<String>, at: Timestamp) -> Self {
        Self {
            cell: cell.into(),
            at,
            positions: Vec::new(),
            utilisation: Vec::new(),
            reconciliation_breaks: Vec::new(),
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
}

impl CellIngestion {
    /// Whether this report changed anything an operator needs to look at.
    pub fn is_quiet(&self) -> bool {
        self.halted.is_none() && self.concentrations.is_empty() && self.recalls.is_empty()
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

    /// The allocator's input per strategy, updated by the learn edge.
    proposals: BTreeMap<StrategyId, StrategyProposal>,
    /// The last full book each cell reported.
    positions: BTreeMap<String, Vec<CellPosition>>,
    exposure: AggregateExposure,
    utilisation: BTreeMap<(String, StrategyId), Utilisation>,
    envelopes: BTreeMap<(String, StrategyId), CapitalEnvelope>,
    recalls: RecallRegister,
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
            proposals: BTreeMap::new(),
            positions: BTreeMap::new(),
            exposure: AggregateExposure::default(),
            utilisation: BTreeMap::new(),
            envelopes: BTreeMap::new(),
            recalls: RecallRegister::new(),
            incidents_raised: 0,
        })
    }

    pub fn config(&self) -> &CentralConfig {
        &self.config
    }

    /// Count every strategy move the plane's ledger records into `metrics`.
    ///
    /// Attached after assembly rather than taken by the constructor, because
    /// the plane a deployment builds is swapped into a platform that already
    /// owns the registry, and the swap is where the two meet.
    pub fn attach_metrics(&mut self, metrics: Arc<Metrics>) {
        self.factory.attach_metrics(metrics);
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
        })
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

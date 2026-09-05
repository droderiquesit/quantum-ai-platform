//! The fabric journal: every wallet, corridor, destination and gate decision
//! as a record in the hash-chained event log, so that the state of the
//! control can be rebuilt from the log alone.
//!
//! `rules/10-product-direction.md` requires every decision to be reproducible
//! from the event log, and `architecture/00-boundaries.md` forbids a second
//! source of truth for a fact the log already holds. Before this module the
//! fabric's controls were deterministic — every entry point takes the clock
//! as an argument — but their decisions lived only in the process that made
//! them: a corridor's `history` was a `Vec` in memory, a gate's veto was a
//! return value, and a wallet's halt was an alert. An operator asked "why
//! was that transfer refused on Tuesday" had nothing to replay.
//!
//! # A record is the command and its outcome, not the state
//!
//! Each [`FabricRecord`] carries the inputs of one decision — the
//! [`FabricCommand`] exactly as the caller supplied it, clock included — and
//! the [`FabricOutcome`] the control produced. It does not carry the
//! resulting state. [`crate::replay`] rebuilds the state by *re-executing*
//! the command against the state the previous records built, and refuses a
//! record whose recorded outcome disagrees with what the control computes.
//! That is stronger than replaying a snapshot: a record that has a valid
//! hash and says "admitted" where the seven checks say "vetoed" is refused,
//! rather than trusted because its chain verified.
//!
//! # Refusals are recorded, not swallowed
//!
//! A corridor edge the lifecycle table refuses, a wallet assembled from a
//! stale observation, a gate veto — each is an [`Outcome::Refused`] in the
//! log with the control's own message. A journal that recorded only what
//! was admitted could say what the platform did and not what it declined,
//! and the second is the number that says whether the controls are
//! calibrated or merely shut.
//!
//! # The journal writes first and commits second
//!
//! [`FabricJournal::decide`] executes the command on a scratch copy of the
//! state, appends the record, and only then adopts the new state. A full or
//! duplicate-id log therefore leaves the live state exactly where it was:
//! the platform never holds a decision the log does not, which is the seam
//! at which "bill what ran" would otherwise break.
//!
//! # What this does not do
//!
//! Nothing here moves capital, and the journal is a `FabricJournal` and not
//! a transfer engine: an admitted gate assessment is recorded as an
//! [`crate::gate::Approved`], which carries no way to execute (ADR 0021).
//! The journal keeps no clock — every timestamp is the one inside the
//! command — and mints event ids from a seeded [`IdGenerator`], so two
//! journals given the same seed and the same commands write byte-identical
//! records.

use crate::corridor::{Corridor, CorridorCaps, CorridorId, CorridorStage};
use crate::custody::{CorridorKind, CustodyClass, CustodyPolicy};
use crate::destination::{
    Approver, DestinationKey, DestinationRegistry, DestinationStatus, SignatureRecord,
};
use crate::gate::{
    Approved, KillSwitchState, SourceBalances, TransferGate, TransferHistory, TransferIntent,
    VelocityState, Vetoed,
};
use crate::location::CapitalLocation;
use crate::wallet::{
    HoldingObservation, LedgerView, ReconciliationOutcome, TolerancePolicy, VenueAsset, Wallet,
};
use qip_core::error::Result;
use qip_core::ids::IdGenerator;
use qip_core::{CorrelationId, Duration, EventId, Lineage, Timestamp};
use qip_events::log::LogRecord;
use qip_events::{Envelope, EventBody, EventLog, Topic};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The producer name every fabric record carries in its lineage.
///
/// The topic is shared with nothing else today, but a topic is not a claim
/// of authorship; a replay reads only records whose producer is this one,
/// so another crate that later writes `compliance.evaluated` cannot be
/// decoded as a fabric decision by mistake.
pub const PRODUCER: &str = "capital-fabric";

/// The outcome of one command: applied, with what the control produced, or
/// refused, with the control's own words.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome<T> {
    /// The control accepted the command and produced this.
    Applied(T),
    /// The control refused, and this is why.
    Refused(String),
}

impl<T> Outcome<T> {
    /// Whether the command was refused.
    pub fn is_refused(&self) -> bool {
        matches!(self, Self::Refused(_))
    }
}

/// One change to the destination allowlist (§38.4), as its inputs.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum DestinationAction {
    /// Add a candidate.
    Propose {
        key: DestinationKey,
        by: Approver,
        at: Timestamp,
    },
    /// Record the out-of-band verification.
    Verify {
        key: DestinationKey,
        by: Approver,
        at: Timestamp,
    },
    /// Record that a hardware-key signature covers it.
    RecordSignature {
        key: DestinationKey,
        signature: SignatureRecord,
    },
    /// Withdraw it permanently.
    Revoke {
        key: DestinationKey,
        by: Approver,
        at: Timestamp,
    },
}

impl DestinationAction {
    /// The destination the action is about.
    pub fn key(&self) -> &DestinationKey {
        match self {
            Self::Propose { key, .. }
            | Self::Verify { key, .. }
            | Self::RecordSignature { key, .. }
            | Self::Revoke { key, .. } => key,
        }
    }

    /// When the action happened, on the caller's clock.
    pub fn at(&self) -> Timestamp {
        match self {
            Self::Propose { at, .. } | Self::Verify { at, .. } | Self::Revoke { at, .. } => *at,
            Self::RecordSignature { signature, .. } => signature.signed_at,
        }
    }
}

/// One step of a corridor's life (§37.1), as its inputs.
///
/// Every variant maps to exactly one method on [`Corridor`], with the same
/// arguments, so a replay calls the same code the live decision did and the
/// lifecycle table is consulted once, in one place.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum CorridorAction {
    /// [`Corridor::propose`].
    Propose {
        id: CorridorId,
        source: CapitalLocation,
        source_class: CustodyClass,
        kind: CorridorKind,
        destination: DestinationKey,
        caps: CorridorCaps,
        purpose: String,
        by: Approver,
        at: Timestamp,
    },
    /// One step on a corridor that already exists.
    ///
    /// A separate arm from `Propose` rather than one flat list, so that the
    /// dispatch over an existing corridor has no proposal to account for: a
    /// step names a corridor the log has already proposed, and a proposal
    /// names one it has not.
    Step { id: CorridorId, step: CorridorStep },
}

impl CorridorAction {
    /// The corridor the action is about.
    pub fn id(&self) -> &CorridorId {
        match self {
            Self::Propose { id, .. } | Self::Step { id, .. } => id,
        }
    }

    /// When the action happened, on the caller's clock.
    pub fn at(&self) -> Timestamp {
        match self {
            Self::Propose { at, .. } => *at,
            Self::Step { step, .. } => step.at(),
        }
    }
}

/// One step of an existing corridor's life, as the arguments of the
/// [`Corridor`] method it maps to.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "step", rename_all = "snake_case")]
pub enum CorridorStep {
    /// [`Corridor::review`].
    Review { by: Approver, at: Timestamp },
    /// [`Corridor::record_signature`].
    RecordSignature { signature: SignatureRecord },
    /// [`Corridor::begin_delay`].
    BeginDelay { now: Timestamp },
    /// [`Corridor::activate`].
    Activate { now: Timestamp },
    /// [`Corridor::suspend`].
    Suspend {
        by: Option<Approver>,
        reason: String,
        at: Timestamp,
    },
    /// [`Corridor::reactivate`].
    Reactivate { by: Approver, now: Timestamp },
    /// [`Corridor::revoke`].
    Revoke {
        by: Approver,
        reason: String,
        at: Timestamp,
    },
    /// [`Corridor::tighten_caps`].
    TightenCaps {
        caps: CorridorCaps,
        by: Approver,
        at: Timestamp,
    },
    /// [`Corridor::loosen_caps`].
    LoosenCaps {
        caps: CorridorCaps,
        signature: SignatureRecord,
        now: Timestamp,
    },
}

impl CorridorStep {
    /// When the step happened, on the caller's clock.
    pub fn at(&self) -> Timestamp {
        match self {
            Self::Review { at, .. }
            | Self::Suspend { at, .. }
            | Self::Revoke { at, .. }
            | Self::TightenCaps { at, .. } => *at,
            Self::BeginDelay { now }
            | Self::Activate { now }
            | Self::Reactivate { now, .. }
            | Self::LoosenCaps { now, .. } => *now,
            Self::RecordSignature { signature } => signature.signed_at,
        }
    }

    /// Take the step on `corridor`, through the one method it maps to.
    fn apply(&self, corridor: &mut Corridor) -> Result<()> {
        match self {
            Self::Review { by, at } => corridor.review(by.clone(), *at),
            Self::RecordSignature { signature } => corridor.record_signature(signature.clone()),
            Self::BeginDelay { now } => corridor.begin_delay(*now).map(|_| ()),
            Self::Activate { now } => corridor.activate(*now),
            Self::Suspend { by, reason, at } => corridor.suspend(by.clone(), reason.clone(), *at),
            Self::Reactivate { by, now } => corridor.reactivate(by.clone(), *now),
            Self::Revoke { by, reason, at } => corridor.revoke(by.clone(), reason.clone(), *at),
            Self::TightenCaps { caps, by, at } => {
                corridor.tighten_caps(caps.clone(), by.clone(), *at)
            }
            Self::LoosenCaps {
                caps,
                signature,
                now,
            } => corridor
                .loosen_caps(caps.clone(), signature.clone(), *now)
                .map(|_| ()),
        }
    }
}

/// Where a corridor stands after an applied action.
///
/// The stage and the delay's end are the two facts the gate reads; recording
/// them beside the action lets a replay refuse a record whose action the
/// lifecycle table would have taken somewhere else.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CorridorStanding {
    /// The stage the corridor is in after the action.
    pub stage: CorridorStage,
    /// When its current delay ends, if one is or was running.
    pub activation_at: Option<Timestamp>,
}

/// One wallet decision (§38.3), as its inputs.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case")]
pub enum WalletCommand {
    /// [`Wallet::assemble`]: the balances the venues reported and the
    /// balances the ledger booked, exactly as handed over. This is where a
    /// ledger balance enters the fabric's record; the wallet itself books
    /// nothing, and a journal that replayed credits into it would be the
    /// write path §38.1 keeps out of this platform.
    Assemble {
        observations: Vec<HoldingObservation>,
        ledger_views: Vec<LedgerView>,
        freshness: Duration,
        now: Timestamp,
    },
    /// [`Wallet::reconcile`] against these tolerances, at this instant.
    Reconcile {
        tolerances: TolerancePolicy,
        at: Timestamp,
    },
}

impl WalletCommand {
    /// When the command was issued, on the caller's clock.
    pub fn at(&self) -> Timestamp {
        match self {
            Self::Assemble { now, .. } => *now,
            Self::Reconcile { at, .. } => *at,
        }
    }
}

/// What a wallet command produced.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WalletOutcome {
    /// Assembled: the venue-assets it now holds evidence about, in order.
    Assembled { venue_assets: Vec<VenueAsset> },
    /// Reconciled: one outcome per venue-asset, in order. A halt is in here
    /// as a halt; the alert is part of the record.
    Reconciled {
        outcomes: Vec<ReconciliationOutcome>,
    },
}

/// One gate assessment (§37.3), as its inputs.
///
/// The corridor and the destination allowlist are named rather than carried,
/// because the replay has rebuilt both from the records before this one and
/// a copy inside the record would be a second claim about them. Everything
/// else the gate reads — the custody table, the carried history, the source
/// balances, the breaker and the kill switch — is carried, because the
/// fabric holds none of it and the log is the only place it is written down.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GateCommand {
    pub intent: TransferIntent,
    pub corridor: CorridorId,
    pub custody: CustodyPolicy,
    pub history: TransferHistory,
    pub balances: SourceBalances,
    pub velocity: VelocityState,
    pub kill_switch: KillSwitchState,
    pub now: Timestamp,
}

/// What the gate said.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GateVerdict {
    /// All seven checks passed. The record carries no way to execute.
    Admitted(Approved),
    /// A check refused, and the record names it.
    Vetoed(Vetoed),
}

/// One gate assessment as the rebuilt state keeps it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GateAssessment {
    /// The corridor the intent was assessed against.
    pub corridor: CorridorId,
    /// What the gate said.
    pub verdict: GateVerdict,
}

/// Any command the fabric journals.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "subject", rename_all = "snake_case")]
pub enum FabricCommand {
    Destination(DestinationAction),
    Corridor(CorridorAction),
    Wallet(WalletCommand),
    Gate(GateCommand),
}

impl FabricCommand {
    /// When the command was issued, on the caller's clock.
    pub fn at(&self) -> Timestamp {
        match self {
            Self::Destination(action) => action.at(),
            Self::Corridor(action) => action.at(),
            Self::Wallet(command) => command.at(),
            Self::Gate(command) => command.now,
        }
    }
}

/// What a command produced, by subject.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "subject", content = "outcome", rename_all = "snake_case")]
pub enum FabricOutcome {
    /// The destination's status after the action.
    Destination(Outcome<DestinationStatus>),
    /// The corridor's standing after the action.
    Corridor(Outcome<CorridorStanding>),
    /// What the wallet produced.
    Wallet(Outcome<WalletOutcome>),
    /// What the gate said, or why the assessment could not be made at all.
    Gate(Outcome<GateVerdict>),
}

impl FabricOutcome {
    /// Whether the command was refused before or by the control.
    ///
    /// A gate veto is *not* a refused command: the assessment ran and its
    /// verdict is a veto. The refused gate outcome is the one where the
    /// assessment could not run, because the corridor named does not exist.
    pub fn is_refused(&self) -> bool {
        match self {
            Self::Destination(outcome) => outcome.is_refused(),
            Self::Corridor(outcome) => outcome.is_refused(),
            Self::Wallet(outcome) => outcome.is_refused(),
            Self::Gate(outcome) => outcome.is_refused(),
        }
    }
}

/// One fabric decision as written to the event log: the command and what
/// came of it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FabricRecord {
    /// The inputs, exactly as supplied.
    pub command: FabricCommand,
    /// What the control produced from them.
    pub outcome: FabricOutcome,
}

impl EventBody for FabricRecord {
    /// Every fabric record is a deterministic control's evaluation — an
    /// allowlist, a lifecycle table, reconciliation arithmetic, the seven
    /// checks — which is what `compliance.evaluated` names. The topic sits
    /// in the Decide group, so the log never evicts one to make room and
    /// refuses the append instead; a fabric decision that could be dropped
    /// from the working set would be a decision nobody could replay.
    const TOPIC: Topic = Topic::ComplianceEvaluated;
    const SCHEMA_VERSION: u32 = 1;
}

/// Everything the fabric's controls have decided, rebuilt from records.
///
/// Every field is either a `BTreeMap` or a `Vec` in record order, so two
/// replays of the same records are equal and render identically.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FabricState {
    destinations: DestinationRegistry,
    corridors: BTreeMap<CorridorId, Corridor>,
    wallet: Option<Wallet>,
    reconciliations: BTreeMap<VenueAsset, ReconciliationOutcome>,
    assessments: Vec<GateAssessment>,
}

impl FabricState {
    /// Nothing decided yet.
    pub fn new() -> Self {
        Self::default()
    }

    /// The allowlist as the records built it.
    pub fn destinations(&self) -> &DestinationRegistry {
        &self.destinations
    }

    /// Every corridor, by name.
    pub fn corridors(&self) -> &BTreeMap<CorridorId, Corridor> {
        &self.corridors
    }

    /// One corridor, if a record proposed it.
    pub fn corridor(&self, id: &CorridorId) -> Option<&Corridor> {
        self.corridors.get(id)
    }

    /// The wallet as last assembled, if one has been.
    pub fn wallet(&self) -> Option<&Wallet> {
        self.wallet.as_ref()
    }

    /// The latest reconciliation outcome per venue-asset.
    pub fn reconciliations(&self) -> &BTreeMap<VenueAsset, ReconciliationOutcome> {
        &self.reconciliations
    }

    /// Every gate assessment, in record order.
    pub fn assessments(&self) -> &[GateAssessment] {
        &self.assessments
    }

    /// Run a command against this state and produce the record of it.
    ///
    /// Infallible by design: a command the control refuses is an
    /// [`Outcome::Refused`] in the returned record, not an error, because
    /// the refusal is a decision and belongs in the log. The state changes
    /// only when the outcome is applied.
    pub fn execute(&mut self, command: FabricCommand) -> FabricRecord {
        let outcome = match &command {
            FabricCommand::Destination(action) => {
                FabricOutcome::Destination(self.execute_destination(action))
            }
            FabricCommand::Corridor(action) => {
                FabricOutcome::Corridor(self.execute_corridor(action))
            }
            FabricCommand::Wallet(wallet) => FabricOutcome::Wallet(self.execute_wallet(wallet)),
            FabricCommand::Gate(gate) => FabricOutcome::Gate(self.execute_gate(gate)),
        };
        FabricRecord { command, outcome }
    }

    fn execute_destination(&mut self, action: &DestinationAction) -> Outcome<DestinationStatus> {
        // On a scratch copy, so a refused action leaves the registry as it was
        // even if a registry method ever mutated before refusing.
        let mut registry = self.destinations.clone();
        let applied = match action {
            DestinationAction::Propose { key, by, at } => {
                registry.propose(key.clone(), by.clone(), *at)
            }
            DestinationAction::Verify { key, by, at } => registry.verify(key, by.clone(), *at),
            DestinationAction::RecordSignature { key, signature } => {
                registry.record_signature(key, signature.clone())
            }
            DestinationAction::Revoke { key, by, at } => registry.revoke(key, by.clone(), *at),
        };
        if let Err(err) = applied {
            return Outcome::Refused(err.message().to_string());
        }
        let Some(record) = registry.get(action.key()) else {
            // Unreachable through the registry's own API, which never removes
            // an entry; stated as a refusal rather than a panic so a future
            // registry that did remove one is caught here.
            return Outcome::Refused(format!(
                "destination {} is not in the registry after the action was applied",
                action.key()
            ));
        };
        let status = record.status.clone();
        self.destinations = registry;
        Outcome::Applied(status)
    }

    fn execute_corridor(&mut self, action: &CorridorAction) -> Outcome<CorridorStanding> {
        let id = action.id();
        let applied: Result<Corridor> = match action {
            CorridorAction::Propose {
                id,
                source,
                source_class,
                kind,
                destination,
                caps,
                purpose,
                by,
                at,
            } => {
                if self.corridors.contains_key(id) {
                    return Outcome::Refused(format!(
                        "corridor {id} is already proposed; a corridor is proposed once, and a \
                         second proposal under the same name would let a replay confuse the two"
                    ));
                }
                Corridor::propose(
                    id.clone(),
                    source.clone(),
                    *source_class,
                    *kind,
                    destination.clone(),
                    caps.clone(),
                    purpose.clone(),
                    by.clone(),
                    *at,
                )
            }
            CorridorAction::Step { id, step } => {
                let Some(existing) = self.corridors.get(id) else {
                    return Outcome::Refused(format!(
                        "corridor {id} has never been proposed; propose it before acting on it"
                    ));
                };
                // On a scratch copy, so a refused edge leaves the live corridor
                // untouched whatever the method did before refusing.
                let mut corridor = existing.clone();
                step.apply(&mut corridor).map(|()| corridor)
            }
        };
        match applied {
            Ok(corridor) => {
                let standing = CorridorStanding {
                    stage: corridor.stage(),
                    activation_at: corridor.activation_at(),
                };
                self.corridors.insert(id.clone(), corridor);
                Outcome::Applied(standing)
            }
            Err(err) => Outcome::Refused(err.message().to_string()),
        }
    }

    fn execute_wallet(&mut self, command: &WalletCommand) -> Outcome<WalletOutcome> {
        match command {
            WalletCommand::Assemble {
                observations,
                ledger_views,
                freshness,
                now,
            } => {
                match Wallet::assemble(observations.clone(), ledger_views.clone(), *freshness, *now)
                {
                    Ok(wallet) => {
                        let venue_assets = wallet.venue_assets().cloned().collect();
                        self.wallet = Some(wallet);
                        Outcome::Applied(WalletOutcome::Assembled { venue_assets })
                    }
                    Err(err) => Outcome::Refused(err.message().to_string()),
                }
            }
            WalletCommand::Reconcile { tolerances, at } => {
                let Some(wallet) = &self.wallet else {
                    return Outcome::Refused(format!(
                        "no wallet has been assembled as of {at}; assemble one from \
                         observations and ledger views before reconciling"
                    ));
                };
                if wallet.as_of() > *at {
                    return Outcome::Refused(format!(
                        "the wallet was assembled as of {} and the reconciliation is dated {at}; \
                         a reconciliation cannot predate the evidence it judges",
                        wallet.as_of()
                    ));
                }
                match wallet.reconcile(tolerances) {
                    Ok(outcomes) => {
                        for outcome in &outcomes {
                            self.reconciliations
                                .insert(outcome.venue_asset(), outcome.clone());
                        }
                        Outcome::Applied(WalletOutcome::Reconciled { outcomes })
                    }
                    Err(err) => Outcome::Refused(err.message().to_string()),
                }
            }
        }
    }

    fn execute_gate(&mut self, command: &GateCommand) -> Outcome<GateVerdict> {
        let Some(corridor) = self.corridors.get(&command.corridor) else {
            return Outcome::Refused(format!(
                "corridor {} has never been proposed, so there is nothing to assess the intent \
                 against; an intent is assessed only against a corridor the log knows",
                command.corridor
            ));
        };
        let verdict = match TransferGate::assess(
            &command.intent,
            corridor,
            &self.destinations,
            &command.custody,
            &command.history,
            &command.balances,
            command.velocity,
            command.kill_switch,
            command.now,
        ) {
            Ok(approved) => GateVerdict::Admitted(approved),
            Err(vetoed) => GateVerdict::Vetoed(vetoed),
        };
        self.assessments.push(GateAssessment {
            corridor: command.corridor.clone(),
            verdict: verdict.clone(),
        });
        Outcome::Applied(verdict)
    }
}

/// The journal: a [`FabricState`] and the [`EventLog`] that is its only
/// record.
#[derive(Debug)]
pub struct FabricJournal {
    log: EventLog,
    ids: IdGenerator,
    lineage: Lineage,
    state: FabricState,
}

impl FabricJournal {
    /// A journal writing to a fresh in-memory log, minting ids from `seed`.
    ///
    /// Two journals built with the same seed and given the same commands
    /// write byte-identical records; that is what lets a replayed run be
    /// diffed against the original.
    pub fn new(seed: u64, correlation: CorrelationId) -> Self {
        Self {
            log: EventLog::in_memory(),
            ids: IdGenerator::new(seed),
            lineage: Lineage::root(correlation, PRODUCER),
            state: FabricState::new(),
        }
    }

    /// A journal resuming an existing log — the composition root's
    /// file-backed one, or one another topic already writes to.
    ///
    /// The state is rebuilt by [`crate::replay::replay`] over everything the
    /// log holds, so a log with a broken chain or a lying record refuses to
    /// be resumed rather than being written after. The id generator is
    /// advanced past the records already present, for the reason
    /// [`IdGenerator::advance`] gives.
    pub fn resume(log: EventLog, seed: u64, correlation: CorrelationId) -> Result<Self> {
        let replayed = crate::replay::replay(log.records())?;
        let ids = IdGenerator::new(seed);
        ids.advance(log.records().len() as u64);
        Ok(Self {
            log,
            ids,
            lineage: Lineage::root(correlation, PRODUCER),
            state: replayed.state,
        })
    }

    /// Execute a command, write its record, and only then adopt the result.
    ///
    /// The order is the guarantee: the log is appended before the live state
    /// moves, so a log that refuses the append (full of audit records, or
    /// handed a duplicate id) leaves the state exactly where it was. The
    /// alternative — state first, record second — is a platform that can
    /// hold a decision the log does not, which is the seam at which
    /// "bill what ran" breaks.
    pub fn decide(&mut self, command: FabricCommand) -> Result<FabricRecord> {
        let at = command.at();
        let mut next = self.state.clone();
        let record = next.execute(command);
        let event_id: EventId = self.ids.generate(at);
        // The journal keeps no clock, so `recorded_at` is the command's own
        // instant: a wall-clock recorded_at would make two runs of the same
        // commands hash differently, and the point of the journal is that
        // they do not.
        let envelope = Envelope::new(event_id, at, at, self.lineage.clone(), record.clone());
        self.log.append(&envelope.erase()?)?;
        self.state = next;
        Ok(record)
    }

    /// The live state.
    pub fn state(&self) -> &FabricState {
        &self.state
    }

    /// The log itself, for its own chain verification and statistics.
    pub fn log(&self) -> &EventLog {
        &self.log
    }

    /// Every record written, oldest first.
    pub fn records(&self) -> &[LogRecord] {
        self.log.records()
    }

    /// Give the log back, for a composition root that owns it.
    pub fn into_log(self) -> EventLog {
        self.log
    }
}

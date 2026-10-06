//! The peer half of the Reflex Mesh, as a protocol with no sockets.
//!
//! [`crate::spine`] is the centre-to-cell wire of ADR 0011. This module is the
//! other thing the word "mesh" names: what regional cells say to *each other*
//! while they coordinate a multi-leg opportunity. It is deliberately pure:
//! frames in, state and refusals out. Putting the rules here means the future
//! transport (MESH-038, blocked on a dependency record) can only carry bytes
//! and cannot decide anything, so it cannot weaken them.
//!
//! What each rule prevents:
//!
//! * **An epoch missing a field is refused by name** ([`EpochDraft::build`]).
//!   A token with no unwind policy is a cycle with no loss bound, discovered
//!   when a leg strands.
//! * **Nothing fires after the epoch's expiry, and nothing stale is applied**
//!   ([`PeerEndpoint`]). A node that restarts holding an old epoch would
//!   otherwise reserve or fire against a cycle the coordinator has abandoned.
//! * **FIRE only when every leg is reserved and the declared loss bound covers
//!   the worst recovery** ([`PeerEndpoint::declare_fire`]).
//! * **Frames are bounded and the receiver refuses before allocating**
//!   ([`encode`], [`decode`]).
//! * **A peer cannot carry a pack, policy or grant** ([`decode`]). Those arrive
//!   only down the signed path; a peer frame naming one is refused, never
//!   interpreted.
//! * **The local gate has the last word** ([`PeerEndpoint::send_leg`]): FIRE
//!   authorises a leg to be *attempted*, never to bypass the sending node's own
//!   deterministic risk gate.
//! * **A node's reservations are bounded in amount and time**
//!   ([`ReservationBook`]).
//! * **The local gate runs before anything is reserved, and a refusal is told
//!   to the coordinator** ([`PeerEndpoint::reserve_own_legs`], MESH-028). A
//!   node that reserved first and asked its gate second would hold capital
//!   for a leg it was never going to send, and a coordinator that never heard
//!   the refusal would wait out the whole TTL for a reservation that is not
//!   coming.
//! * **A cycle that cannot have its guarantees is rejected out loud**
//!   (MESH-036). A FIRE refused for an unmet reservation condition or an
//!   unestablished loss bound, and an epoch that reached its expiry unfired,
//!   are each journaled naming the guarantee that was missing. Before this
//!   they were an `Err` the caller could drop and a silent `retain`.
//! * **Every message kind is placed in a time lane** ([`MeshFunction`],
//!   MESH-013). The five functions §4 names are Lane 1; the match that says
//!   so is exhaustive, so a sixth kind does not compile until somebody
//!   decides its lane.
//!
//! Nothing here persists. An endpoint that restarts is empty; the durable
//! record of what a cycle did is the journal, not the mesh (MESH-017).

use qip_core::error::{Error, Result};
use qip_core::{Decimal, Duration, Timestamp, sha256_hex};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Fewest legs a distributed cycle has (MESH-010).
pub const MIN_LEGS: usize = 2;
/// Most legs a distributed cycle has (MESH-010).
pub const MAX_LEGS: usize = 20;

/// Message kinds that belong to the downward signed path. A peer frame naming
/// one is refused outright rather than parsed as an unknown message, so the
/// refusal says why (MESH-003).
const DOWNWARD_ONLY: [&str; 7] = [
    "policy",
    "policy_pack",
    "capital_grant",
    "risk_pack",
    "model_pack",
    "strategy_pack",
    "belief_pack",
];

/// Longest reason a [`PeerMessage::Declined`] carries, in characters.
///
/// The reason is diagnostic text for the coordinator's journal, not an input
/// to any decision, so it is shortened to fit rather than allowed to push a
/// refusal past the frame bound and be lost. Everything that decides — which
/// leg, which epoch — travels in its own field at full length.
pub const MAX_DECLINE_REASON: usize = 256;

/// The blueprint's time lanes this module has to tell apart (§4).
///
/// Only the two that matter to the mesh are named. Lane 0 is the reflex hot
/// path, whose decisions take no remote dependency; Lane 1 is where a
/// cross-region round trip is an accepted and explicit cost. The slower lanes
/// never carry a peer message and naming them here would be a registry of
/// things this crate does not hold.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Lane {
    /// Lane 0: local reflex. Never waits on a peer.
    Reflex = 0,
    /// Lane 1: coordinated fast, about a millisecond to seconds.
    CoordinatedFast = 1,
}

impl Lane {
    /// The lane's number as the placement register writes it.
    pub const fn number(self) -> u8 {
        self as u8
    }
}

/// The five functions §4 places in Lane 1 (MESH-013).
///
/// A function is what a message is *for*; a kind is how it is spelled on the
/// wire. The registry is kept on functions because the blueprint names
/// functions, and [`PeerMessage::function`] maps every kind onto one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum MeshFunction {
    /// Latency-sensitive state a peer tells the others: a leg's fill.
    PeerReflexMessage,
    /// The token that carries a distributed opportunity's epoch.
    OpportunityToken,
    /// A node's reservation for its own legs, or its refusal to make one.
    DistributedReservation,
    /// The step that turns reservations into a cycle: FIRE.
    MultiLegCoordination,
    /// A command that recovers a stranded cycle. Only unwind has a kind
    /// today; a hedge command (MESH-008) must be classified here when it is
    /// added, and the exhaustive match in [`PeerMessage::function`] is what
    /// makes forgetting to a compile error.
    HedgeUnwindCommand,
}

impl MeshFunction {
    /// Every function, in the order §4 lists them.
    pub const ALL: [Self; 5] = [
        Self::PeerReflexMessage,
        Self::OpportunityToken,
        Self::DistributedReservation,
        Self::MultiLegCoordination,
        Self::HedgeUnwindCommand,
    ];

    /// The lane this function runs in.
    ///
    /// Exhaustive on purpose, with no wildcard arm: placing a function in a
    /// lane is a decision, and a default would make it for whoever adds the
    /// next one. None of the five is [`Lane::Reflex`], because every one of
    /// them is a statement to or from another region, and a Lane 0 decision
    /// that waited for one would have taken the remote dependency Lane 0
    /// exists to refuse (MESH-002).
    pub const fn lane(self) -> Lane {
        match self {
            Self::PeerReflexMessage
            | Self::OpportunityToken
            | Self::DistributedReservation
            | Self::MultiLegCoordination
            | Self::HedgeUnwindCommand => Lane::CoordinatedFast,
        }
    }

    /// A stable name for logs and the placement check.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::PeerReflexMessage => "peer_reflex_message",
            Self::OpportunityToken => "opportunity_token",
            Self::DistributedReservation => "distributed_reservation",
            Self::MultiLegCoordination => "multi_leg_coordination",
            Self::HedgeUnwindCommand => "hedge_unwind_command",
        }
    }
}

/// One leg of an epoch: which node executes it, how large it is and the most
/// that unwinding it can cost if the cycle strands after it fills.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EpochLeg {
    pub leg: String,
    pub node: String,
    pub size: Decimal,
    pub unwind_loss: Decimal,
}

/// The seven things MESH-005 names, plus the identity and fencing counter every
/// message is checked against. Fields are public for reading; build through
/// [`EpochDraft::build`] so none can be missing, and re-validate with
/// [`OpportunityEpoch::validate`] after deserialising, because serde bypasses
/// the builder.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpportunityEpoch {
    pub opportunity: String,
    /// Fencing counter: a message is current only if it carries this value.
    pub sequence: u64,
    pub cycle: String,
    pub legs: Vec<EpochLeg>,
    pub size: Decimal,
    pub min_edge_bps: Decimal,
    pub model_versions: Vec<String>,
    pub opened_at_nanos: i64,
    pub ttl_nanos: i64,
    /// The unwind policy: the most recovery may lose, fixed before FIRE.
    pub max_recovery_loss: Decimal,
}

impl OpportunityEpoch {
    /// Instant from which nothing of this epoch may fire or be applied.
    pub fn expires_at(&self) -> Timestamp {
        Timestamp::from_nanos(self.opened_at_nanos.saturating_add(self.ttl_nanos))
    }

    /// Content identity; two epochs differing in any field differ here.
    pub fn identity(&self) -> String {
        // Struct field order is fixed, so the JSON is canonical for this type.
        sha256_hex(serde_json::to_string(self).unwrap_or_default().as_bytes())
    }

    /// Refuse an epoch that violates the contract, whatever built it.
    pub fn validate(&self, ttl_ceiling: Duration) -> Result<()> {
        if self.opportunity.is_empty() {
            return Err(missing("opportunity"));
        }
        if self.cycle.is_empty() {
            return Err(missing("cycle"));
        }
        if self.model_versions.is_empty() || self.model_versions.iter().any(String::is_empty) {
            return Err(missing("model_versions"));
        }
        if !(MIN_LEGS..=MAX_LEGS).contains(&self.legs.len()) {
            return Err(Error::invalid(format!(
                "epoch field `legs` holds {} legs; a distributed cycle has {MIN_LEGS} to {MAX_LEGS}",
                self.legs.len()
            )));
        }
        let mut seen = BTreeSet::new();
        for l in &self.legs {
            if l.leg.is_empty() || l.node.is_empty() || !seen.insert(l.leg.as_str()) {
                return Err(Error::invalid(
                    "epoch field `legs` needs a distinct non-empty leg id and node on every leg",
                ));
            }
            if !l.size.is_positive() || l.unwind_loss.is_negative() {
                return Err(Error::invalid(format!(
                    "epoch leg `{}` needs a positive size and a non-negative unwind loss",
                    l.leg
                )));
            }
        }
        if !self.size.is_positive() {
            return Err(Error::invalid("epoch field `size` must be positive"));
        }
        if self.min_edge_bps.is_negative() {
            return Err(Error::invalid(
                "epoch field `min_edge_bps` must not be negative",
            ));
        }
        if self.max_recovery_loss.is_negative() {
            return Err(Error::invalid(
                "epoch field `max_recovery_loss` (the unwind policy) must not be negative",
            ));
        }
        if self.ttl_nanos <= 0 {
            return Err(Error::invalid(
                "epoch field `ttl` must be finite and positive; an epoch without a TTL is refused",
            ));
        }
        if self.ttl_nanos > ttl_ceiling.as_nanos() {
            return Err(Error::invalid(format!(
                "epoch field `ttl` of {} ns exceeds the configured ceiling of {} ns; shorten it",
                self.ttl_nanos,
                ttl_ceiling.as_nanos()
            )));
        }
        Ok(())
    }
}

fn missing(field: &str) -> Error {
    Error::invalid(format!("epoch is missing required field `{field}`"))
}

/// An epoch under construction. Every field is an `Option` so that absence is
/// representable and refused by name instead of defaulted.
#[derive(Clone, Debug, Default)]
pub struct EpochDraft {
    pub opportunity: Option<String>,
    pub sequence: Option<u64>,
    pub cycle: Option<String>,
    pub legs: Option<Vec<EpochLeg>>,
    pub size: Option<Decimal>,
    pub min_edge_bps: Option<Decimal>,
    pub model_versions: Option<Vec<String>>,
    pub opened_at_nanos: Option<i64>,
    pub ttl: Option<Duration>,
    pub max_recovery_loss: Option<Decimal>,
}

impl EpochDraft {
    pub fn build(self, ttl_ceiling: Duration) -> Result<OpportunityEpoch> {
        let epoch = OpportunityEpoch {
            opportunity: self.opportunity.ok_or_else(|| missing("opportunity"))?,
            sequence: self.sequence.ok_or_else(|| missing("sequence"))?,
            cycle: self.cycle.ok_or_else(|| missing("cycle"))?,
            legs: self.legs.ok_or_else(|| missing("legs"))?,
            size: self.size.ok_or_else(|| missing("size"))?,
            min_edge_bps: self.min_edge_bps.ok_or_else(|| missing("min_edge_bps"))?,
            model_versions: self
                .model_versions
                .ok_or_else(|| missing("model_versions"))?,
            opened_at_nanos: self.opened_at_nanos.ok_or_else(|| missing("opened_at"))?,
            ttl_nanos: self.ttl.ok_or_else(|| missing("ttl"))?.as_nanos(),
            max_recovery_loss: self
                .max_recovery_loss
                .ok_or_else(|| missing("unwind_policy"))?,
        };
        epoch.validate(ttl_ceiling)?;
        Ok(epoch)
    }
}

/// What peers say. A closed set: anything else, including every downward-only
/// pack kind, fails to decode.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum PeerMessage {
    Token {
        epoch: OpportunityEpoch,
    },
    Reserved {
        opportunity: String,
        sequence: u64,
        leg: String,
        amount: Decimal,
    },
    Fire {
        opportunity: String,
        sequence: u64,
    },
    Fill {
        opportunity: String,
        sequence: u64,
        leg: String,
        quantity: Decimal,
    },
    Unwind {
        opportunity: String,
        sequence: u64,
        leg: String,
        realized_loss: Decimal,
    },
    /// A participant's local gate, or its reservation book, refused one of
    /// its legs: it holds nothing for this epoch and says so (MESH-028).
    Declined {
        opportunity: String,
        sequence: u64,
        leg: String,
        reason: String,
    },
}

impl PeerMessage {
    /// Which of the five Lane 1 functions this kind performs.
    ///
    /// No wildcard arm: a new kind does not compile until it is placed.
    pub const fn function(&self) -> MeshFunction {
        match self {
            Self::Token { .. } => MeshFunction::OpportunityToken,
            Self::Reserved { .. } | Self::Declined { .. } => MeshFunction::DistributedReservation,
            Self::Fire { .. } => MeshFunction::MultiLegCoordination,
            Self::Fill { .. } => MeshFunction::PeerReflexMessage,
            Self::Unwind { .. } => MeshFunction::HedgeUnwindCommand,
        }
    }

    /// The lane this message runs in: its function's.
    pub const fn lane(&self) -> Lane {
        self.function().lane()
    }
}

/// Encode one message as a 4-byte big-endian length then JSON. Refuses a body
/// over `max_frame`, so the sender never relies on the receiver to notice.
pub fn encode(msg: &PeerMessage, max_frame: usize) -> Result<Vec<u8>> {
    let body = serde_json::to_vec(msg).map_err(|e| Error::invalid(format!("encode: {e}")))?;
    let len = u32::try_from(body.len())
        .ok()
        .filter(|_| body.len() <= max_frame)
        .ok_or_else(|| {
            Error::invalid(format!(
                "peer message of {} bytes exceeds the {max_frame}-byte bound; send less per message",
                body.len()
            ))
        })?;
    let mut out = Vec::with_capacity(4 + body.len());
    out.extend_from_slice(&len.to_be_bytes());
    out.extend_from_slice(&body);
    Ok(out)
}

/// Decode one frame. The declared length is checked against the bound before
/// any body byte is read or buffer sized from it.
pub fn decode(frame: &[u8], max_frame: usize) -> Result<PeerMessage> {
    let head: [u8; 4] = frame
        .get(..4)
        .and_then(|h| h.try_into().ok())
        .ok_or_else(|| Error::denied("peer frame shorter than its length prefix"))?;
    let declared = u32::from_be_bytes(head) as usize;
    if declared > max_frame {
        return Err(Error::denied(format!(
            "peer frame declares {declared} bytes, over the {max_frame}-byte bound; refused unread"
        )));
    }
    if frame.len() - 4 != declared {
        return Err(Error::denied("peer frame length disagrees with its prefix"));
    }
    let value: serde_json::Value = serde_json::from_slice(&frame[4..])
        .map_err(|e| Error::denied(format!("peer frame is not valid JSON: {e}")))?;
    if let Some(kind) = value.get("kind").and_then(|k| k.as_str())
        && DOWNWARD_ONLY.contains(&kind)
    {
        return Err(Error::denied(format!(
            "peer frame of kind `{kind}` refused: packs, policy and capital grants arrive only \
             down the signed centre path, never from a peer"
        )));
    }
    serde_json::from_value(value)
        .map_err(|e| Error::denied(format!("peer frame is not a known message: {e}")))
}

/// Limits an endpoint enforces on everything it receives.
#[derive(Clone, Copy, Debug)]
pub struct PeerLimits {
    pub max_frame: usize,
    pub ttl_ceiling: Duration,
    /// Largest clock offset, in either direction, at which this node may join
    /// or act on a distributed epoch (MESH-007). An epoch TTL is only the same
    /// instant in every region while offsets stay inside this.
    pub max_clock_offset: Duration,
    /// Most peers one update may be sent to (MESH-041). Exceeding it is
    /// refused, never truncated: a silently shortened recipient list leaves a
    /// participant acting on an epoch nobody told it about.
    pub max_fan_out: usize,
}

/// A frame or an action the endpoint refused, or a peer's refusal it was
/// told of, kept so a refusal is journaled and never silent. `kind` is a
/// short stable token (`stale_epoch`, `expired`, `guarantee_unmet`, ...).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Refusal {
    pub kind: &'static str,
    pub reason: String,
}

#[derive(Debug)]
struct OpportunityState {
    epoch: OpportunityEpoch,
    held: BTreeMap<String, Decimal>,
    fired: bool,
    sent: BTreeSet<String>,
    filled: BTreeMap<String, Decimal>,
    unwound: BTreeMap<String, Decimal>,
    /// Legs whose node declined to reserve, with why. Final for this epoch:
    /// only a newer token clears it.
    declined: BTreeMap<String, String>,
}

impl OpportunityState {
    fn leg(&self, leg: &str) -> Option<&EpochLeg> {
        self.epoch.legs.iter().find(|l| l.leg == leg)
    }
    fn every_leg_reserved(&self) -> bool {
        self.epoch
            .legs
            .iter()
            .all(|l| self.held.get(&l.leg).is_some_and(|h| *h == l.size))
    }
    fn worst_recovery_loss(&self) -> Option<Decimal> {
        self.epoch
            .legs
            .iter()
            .try_fold(Decimal::from_int(0), |a, l| a.checked_add(l.unwind_loss))
    }
    /// The legs not held at their full size, as `` `a`, `b` ``.
    fn unreserved_legs(&self) -> String {
        self.epoch
            .legs
            .iter()
            .filter(|l| self.held.get(&l.leg).is_none_or(|h| *h != l.size))
            .map(|l| format!("`{}`", l.leg))
            .collect::<Vec<_>>()
            .join(", ")
    }
    /// Why the reservation condition is not met, or `None` when it is.
    ///
    /// A decline is named ahead of a merely missing reservation: a leg that
    /// was declined is never coming, and a leg that is only late might.
    fn reservation_shortfall(&self) -> Option<String> {
        if let Some((leg, reason)) = self.declined.iter().next() {
            return Some(format!(
                "the reservation condition cannot be met: leg `{leg}` was declined by its node \
                 ({reason})"
            ));
        }
        if self.every_leg_reserved() {
            return None;
        }
        Some(format!(
            "not every leg is reserved at its full size (the reservation condition is unmet \
             for {})",
            self.unreserved_legs()
        ))
    }
}

type Rejected = (&'static str, String);

/// One node's view of the opportunities it takes part in. Holds nothing
/// durable: a restarted node starts empty and hears nothing from before.
#[derive(Debug)]
pub struct PeerEndpoint {
    node: String,
    limits: PeerLimits,
    opportunities: BTreeMap<String, OpportunityState>,
    refusals: Vec<Refusal>,
    /// Measured offset of this node's clock from the reference, in nanoseconds.
    /// `None` until something measures it, and `None` is refused, not zero.
    clock_offset_nanos: Option<i64>,
}

impl PeerEndpoint {
    pub fn new(node: impl Into<String>, limits: PeerLimits) -> Self {
        Self {
            node: node.into(),
            limits,
            opportunities: BTreeMap::new(),
            refusals: Vec::new(),
            clock_offset_nanos: None,
        }
    }

    /// Record the latest measured offset of this node's clock (MESH-007).
    pub fn observe_clock_offset(&mut self, offset_nanos: i64) {
        self.clock_offset_nanos = Some(offset_nanos);
    }

    /// Forget that the offset is known, as when the measurement goes stale.
    pub fn clear_clock_offset(&mut self) {
        self.clock_offset_nanos = None;
    }

    fn clock_in_bound(&self) -> std::result::Result<(), Rejected> {
        let bound = self.limits.max_clock_offset.as_nanos();
        match self.clock_offset_nanos {
            None => Err((
                "clock_unknown",
                "this node's clock offset is unknown; measure it before joining an epoch".into(),
            )),
            Some(o) if o.unsigned_abs() > bound.unsigned_abs() => Err((
                "clock_out_of_bound",
                format!("clock offset {o} ns is beyond the {bound} ns bound; resynchronise first"),
            )),
            Some(_) => Ok(()),
        }
    }

    /// Drop every opportunity whose epoch has expired. The mesh keeps nothing
    /// past its usefulness (MESH-017): afterwards a late frame finds no epoch
    /// and is refused, not replayed from a retained copy.
    ///
    /// An epoch that reaches its expiry without FIRE was a cycle whose
    /// guarantees could not be had in time, and it is rejected here by name
    /// (MESH-036) before it is dropped. Dropping it silently, as this did,
    /// left "the cycle was rejected" and "the cycle was never offered"
    /// indistinguishable to whoever asks afterwards why nothing traded.
    pub fn forget_expired(&mut self, now: Timestamp) {
        let mut rejected = Vec::new();
        self.opportunities.retain(|opportunity, s| {
            if now < s.epoch.expires_at() {
                return true;
            }
            if !s.fired {
                let missing = s.reservation_shortfall().unwrap_or_else(|| {
                    "every leg was reserved but FIRE was never declared".to_string()
                });
                rejected.push(Refusal {
                    kind: "guarantee_unmet",
                    reason: format!(
                        "cycle `{opportunity}` rejected at the end of its TTL: {missing}; no leg \
                         was sent"
                    ),
                });
            }
            false
        });
        self.refusals.extend(rejected);
    }

    /// Opportunities currently held.
    pub fn held_opportunities(&self) -> usize {
        self.opportunities.len()
    }

    /// The peers an update for `opportunity` goes to: the other nodes that
    /// hold a leg of it, never every node (MESH-041). Refused when that set
    /// exceeds `max_fan_out`.
    pub fn recipients(&self, opportunity: &str) -> Result<Vec<String>> {
        let st = self
            .opportunities
            .get(opportunity)
            .ok_or_else(|| Error::denied(format!("no epoch held for `{opportunity}`")))?;
        let peers: BTreeSet<&str> = st
            .epoch
            .legs
            .iter()
            .map(|l| l.node.as_str())
            .filter(|n| *n != self.node)
            .collect();
        if peers.len() > self.limits.max_fan_out {
            return Err(Error::denied(format!(
                "update for `{opportunity}` would reach {} peers, over the fan-out bound of {};                  split the opportunity rather than broadcasting it",
                peers.len(),
                self.limits.max_fan_out
            )));
        }
        Ok(peers.into_iter().map(str::to_string).collect())
    }

    /// Every refusal so far, oldest first.
    pub fn refusals(&self) -> &[Refusal] {
        &self.refusals
    }

    pub fn current_sequence(&self, opportunity: &str) -> Option<u64> {
        self.opportunities
            .get(opportunity)
            .map(|s| s.epoch.sequence)
    }

    pub fn reserved(&self, opportunity: &str, leg: &str) -> Option<Decimal> {
        self.opportunities.get(opportunity)?.held.get(leg).copied()
    }

    pub fn is_fired(&self, opportunity: &str) -> bool {
        self.opportunities.get(opportunity).is_some_and(|s| s.fired)
    }

    pub fn filled(&self, opportunity: &str, leg: &str) -> Option<Decimal> {
        self.opportunities
            .get(opportunity)?
            .filled
            .get(leg)
            .copied()
    }

    pub fn leg_sent(&self, opportunity: &str, leg: &str) -> bool {
        self.opportunities
            .get(opportunity)
            .is_some_and(|s| s.sent.contains(leg))
    }

    /// Realized recovery loss so far for one opportunity.
    pub fn realized_recovery_loss(&self, opportunity: &str) -> Decimal {
        self.opportunities
            .get(opportunity)
            .map(|s| s.unwound.values().fold(Decimal::from_int(0), |a, v| a + *v))
            .unwrap_or_default()
    }

    fn refuse(&mut self, (kind, reason): Rejected) -> Error {
        self.refusals.push(Refusal {
            kind,
            reason: reason.clone(),
        });
        Error::denied(reason)
    }

    /// Accept one frame from a peer, or refuse and journal it. A refused
    /// frame changes no state.
    pub fn receive(&mut self, frame: &[u8], now: Timestamp) -> Result<()> {
        let msg = match decode(frame, self.limits.max_frame) {
            Ok(m) => m,
            Err(e) => return Err(self.refuse(("malformed", e.to_string()))),
        };
        match self.admit(msg, now) {
            Ok(()) => Ok(()),
            Err(r) => Err(self.refuse(r)),
        }
    }

    fn admit(&mut self, msg: PeerMessage, now: Timestamp) -> std::result::Result<(), Rejected> {
        let (opp, seq) = match &msg {
            PeerMessage::Token { epoch } => return self.install(epoch.clone(), now),
            PeerMessage::Reserved {
                opportunity,
                sequence,
                ..
            }
            | PeerMessage::Fire {
                opportunity,
                sequence,
            }
            | PeerMessage::Fill {
                opportunity,
                sequence,
                ..
            }
            | PeerMessage::Unwind {
                opportunity,
                sequence,
                ..
            }
            | PeerMessage::Declined {
                opportunity,
                sequence,
                ..
            } => (opportunity.clone(), *sequence),
        };
        let st = self
            .opportunities
            .get_mut(&opp)
            .ok_or(("unknown_opportunity", format!("no epoch held for `{opp}`")))?;
        if st.epoch.sequence != seq {
            return Err((
                "stale_epoch",
                format!(
                    "message for `{opp}` carries epoch {seq}, current is {}; not applied",
                    st.epoch.sequence
                ),
            ));
        }
        if now >= st.epoch.expires_at() {
            return Err((
                "expired",
                format!("epoch of `{opp}` has expired; discarded"),
            ));
        }
        match msg {
            PeerMessage::Reserved { leg, amount, .. } => {
                let l = st
                    .leg(&leg)
                    .ok_or(("unknown_leg", format!("`{leg}` is not a leg of `{opp}`")))?;
                if !amount.is_positive() || amount > l.size {
                    return Err((
                        "reservation_bound",
                        format!(
                            "reservation of {amount} for leg `{leg}` is not within its requirement {}",
                            l.size
                        ),
                    ));
                }
                // A decline is final for the epoch. A reservation arriving
                // after one is a replay, a forgery or a node that changed
                // its mind without a new token, and applying it would let
                // FIRE through on a leg its own node has refused.
                if st.declined.contains_key(&leg) {
                    return Err((
                        "declined",
                        format!(
                            "leg `{leg}` of `{opp}` was declined by its node in this epoch; a \
                             reservation for it needs a new token"
                        ),
                    ));
                }
                st.held.insert(leg, amount);
            }
            PeerMessage::Declined { leg, reason, .. } => {
                st.leg(&leg)
                    .ok_or(("unknown_leg", format!("`{leg}` is not a leg of `{opp}`")))?;
                if st.fired {
                    return Err((
                        "late_decline",
                        format!(
                            "decline of leg `{leg}` arrived after FIRE for `{opp}`; a leg vetoed \
                             after FIRE is a failed leg for recovery, not a decline"
                        ),
                    ));
                }
                // The node holds nothing for this leg, so neither does this
                // view of it.
                st.held.remove(&leg);
                st.declined.insert(leg.clone(), reason.clone());
                // Admitted, and still journaled: the frame is valid, and what
                // it reports is a refusal somebody has to be able to read.
                self.refusals.push(Refusal {
                    kind: "peer_declined",
                    reason: format!("leg `{leg}` of `{opp}` declined by its node: {reason}"),
                });
            }
            PeerMessage::Fire { .. } => {
                if !st.every_leg_reserved() {
                    return Err((
                        "fire_condition",
                        format!("FIRE for `{opp}` before every leg is reserved"),
                    ));
                }
                st.fired = true;
            }
            PeerMessage::Fill { leg, quantity, .. } => {
                let l = st
                    .leg(&leg)
                    .ok_or(("unknown_leg", format!("`{leg}` is not a leg of `{opp}`")))?;
                if !st.fired || !quantity.is_positive() || quantity > l.size {
                    return Err((
                        "fill",
                        format!("fill of {quantity} on `{leg}` precedes FIRE or exceeds the leg"),
                    ));
                }
                st.filled.insert(leg, quantity);
            }
            PeerMessage::Unwind {
                leg, realized_loss, ..
            } => {
                let l = st
                    .leg(&leg)
                    .ok_or(("unknown_leg", format!("`{leg}` is not a leg of `{opp}`")))?;
                if realized_loss.is_negative() || realized_loss > l.unwind_loss {
                    return Err((
                        "loss_bound",
                        format!(
                            "realized unwind loss {realized_loss} on `{leg}` exceeds the {} declared \
                             before FIRE; the cost model is wrong, halt the cell",
                            l.unwind_loss
                        ),
                    ));
                }
                st.unwound.insert(leg, realized_loss);
            }
            PeerMessage::Token { .. } => {}
        }
        Ok(())
    }

    fn install(
        &mut self,
        epoch: OpportunityEpoch,
        now: Timestamp,
    ) -> std::result::Result<(), Rejected> {
        self.clock_in_bound()?;
        epoch
            .validate(self.limits.ttl_ceiling)
            .map_err(|e| ("invalid_epoch", e.to_string()))?;
        if now >= epoch.expires_at() {
            return Err((
                "expired",
                format!("token for `{}` arrived expired", epoch.opportunity),
            ));
        }
        if let Some(cur) = self.opportunities.get(&epoch.opportunity)
            && cur.epoch.sequence >= epoch.sequence
        {
            return Err((
                "stale_epoch",
                format!(
                    "token for `{}` carries epoch {}, current is {}; not applied",
                    epoch.opportunity, epoch.sequence, cur.epoch.sequence
                ),
            ));
        }
        self.opportunities.insert(
            epoch.opportunity.clone(),
            OpportunityState {
                epoch,
                held: BTreeMap::new(),
                fired: false,
                sent: BTreeSet::new(),
                filled: BTreeMap::new(),
                unwound: BTreeMap::new(),
                declined: BTreeMap::new(),
            },
        );
        Ok(())
    }

    /// Reserve this node's own legs of an epoch it holds: the local gate
    /// first, the book second, and the coordinator told either way
    /// (MESH-028).
    ///
    /// The order is the point. Every one of this node's legs is put to
    /// `gate` before any of them touches `book`, so a node whose second leg
    /// breaches a limit holds nothing for its first. Reservation is then
    /// whole or not at all: if the book refuses any leg, the legs it took in
    /// this call are released again before the decline is returned.
    ///
    /// `Ok` carries the frames to send — one `Reserved` per leg, or the one
    /// `Declined`. `Err` is this node being unable to take part at all: an
    /// unknown or expired epoch, a clock out of bound, an epoch already
    /// fired or one in which it holds no leg. Each is journaled.
    pub fn reserve_own_legs(
        &mut self,
        opportunity: &str,
        now: Timestamp,
        book: &mut ReservationBook,
        gate: &dyn Fn(&EpochLeg) -> Result<()>,
    ) -> Result<ReservationOutcome> {
        match self.try_reserve_own_legs(opportunity, now, book, gate) {
            Ok(outcome) => Ok(outcome),
            Err(r) => Err(self.refuse(r)),
        }
    }

    fn try_reserve_own_legs(
        &mut self,
        opportunity: &str,
        now: Timestamp,
        book: &mut ReservationBook,
        gate: &dyn Fn(&EpochLeg) -> Result<()>,
    ) -> std::result::Result<ReservationOutcome, Rejected> {
        self.clock_in_bound()?;
        let max = self.limits.max_frame;
        let st = self.opportunities.get_mut(opportunity).ok_or((
            "unknown_opportunity",
            format!("no epoch held for `{opportunity}`"),
        ))?;
        if now >= st.epoch.expires_at() {
            return Err((
                "expired",
                format!("epoch of `{opportunity}` has expired; nothing is reserved for it"),
            ));
        }
        if st.fired {
            return Err((
                "reservation_after_fire",
                format!("`{opportunity}` has already fired; reservations precede FIRE"),
            ));
        }
        let own: Vec<EpochLeg> = st
            .epoch
            .legs
            .iter()
            .filter(|l| l.node == self.node)
            .cloned()
            .collect();
        if own.is_empty() {
            return Err((
                "no_own_leg",
                format!("this node holds no leg of `{opportunity}`, so it has nothing to reserve"),
            ));
        }

        // The gate, for every leg, before the book is touched for any.
        let mut refusal: Option<(String, &'static str, String)> = None;
        for l in &own {
            if let Err(e) = gate(l) {
                refusal = Some((
                    l.leg.clone(),
                    "gate_refused",
                    format!("local gate refused leg `{}`: {e}", l.leg),
                ));
                break;
            }
        }
        // Then the book, all of this node's legs or none of them.
        if refusal.is_none() {
            let mut taken: Vec<&str> = Vec::new();
            for l in &own {
                match book.reserve(&st.epoch, &l.leg, l.size) {
                    Ok(()) => taken.push(&l.leg),
                    Err(e) => {
                        for leg in &taken {
                            book.release_leg(opportunity, leg);
                        }
                        refusal = Some((
                            l.leg.clone(),
                            "reservation_refused",
                            format!("reservation refused for leg `{}`: {e}", l.leg),
                        ));
                        break;
                    }
                }
            }
        }

        let sequence = st.epoch.sequence;
        if let Some((leg, kind, reason)) = refusal {
            st.declined.insert(leg.clone(), reason.clone());
            self.refusals.push(Refusal {
                kind,
                reason: reason.clone(),
            });
            let frame = encode(
                &PeerMessage::Declined {
                    opportunity: opportunity.into(),
                    sequence,
                    leg,
                    reason: reason.chars().take(MAX_DECLINE_REASON).collect(),
                },
                max,
            )
            .map_err(|e| ("malformed", e.to_string()))?;
            return Ok(ReservationOutcome::Declined(frame));
        }

        let mut frames = Vec::with_capacity(own.len());
        for l in &own {
            frames.push(
                encode(
                    &PeerMessage::Reserved {
                        opportunity: opportunity.into(),
                        sequence,
                        leg: l.leg.clone(),
                        amount: l.size,
                    },
                    max,
                )
                .map_err(|e| ("malformed", e.to_string()))?,
            );
        }
        for l in own {
            st.held.insert(l.leg, l.size);
        }
        Ok(ReservationOutcome::Reserved(frames))
    }

    /// Declare FIRE for an opportunity this node coordinates. Refused unless
    /// every leg is reserved, the epoch is live, and the worst recovery loss
    /// fits the bound the epoch declared. Returns the frame to send peers.
    ///
    /// A refusal is journaled naming the guarantee that was missing
    /// (MESH-036). It used to be only the returned `Err`, which a caller can
    /// drop; a cycle rejected for want of a guarantee then left no record of
    /// which one.
    pub fn declare_fire(&mut self, opportunity: &str, now: Timestamp) -> Result<Vec<u8>> {
        match self.try_declare_fire(opportunity, now) {
            Ok(frame) => Ok(frame),
            Err(r) => Err(self.refuse(r)),
        }
    }

    fn try_declare_fire(
        &mut self,
        opportunity: &str,
        now: Timestamp,
    ) -> std::result::Result<Vec<u8>, Rejected> {
        let max = self.limits.max_frame;
        self.clock_in_bound()?;
        let st = self.opportunities.get_mut(opportunity).ok_or((
            "unknown_opportunity",
            format!("no epoch held for `{opportunity}`"),
        ))?;
        if now >= st.epoch.expires_at() {
            return Err((
                "expired",
                format!(
                    "FIRE refused for `{opportunity}`: epoch expired; nothing fires at or after \
                     expiry"
                ),
            ));
        }
        if let Some(shortfall) = st.reservation_shortfall() {
            return Err((
                "fire_condition",
                format!("FIRE refused for `{opportunity}`: {shortfall}"),
            ));
        }
        let worst = st.worst_recovery_loss().ok_or((
            "fire_loss_bound",
            format!(
                "FIRE refused for `{opportunity}`: the loss bound cannot be established, the \
                 worst recovery loss overflowed"
            ),
        ))?;
        if worst > st.epoch.max_recovery_loss {
            return Err((
                "fire_loss_bound",
                format!(
                    "FIRE refused for `{opportunity}`: the loss bound cannot be established, \
                     worst recovery loss {worst} exceeds the declared bound {}; raise the unwind \
                     policy or shrink the cycle",
                    st.epoch.max_recovery_loss
                ),
            ));
        }
        let frame = encode(
            &PeerMessage::Fire {
                opportunity: opportunity.into(),
                sequence: st.epoch.sequence,
            },
            max,
        )
        .map_err(|e| ("malformed", e.to_string()))?;
        st.fired = true;
        Ok(frame)
    }

    /// Send one of this node's legs. Needs FIRE and a live epoch, and the
    /// local gate decides last: a FIRE never bypasses it. A refusal is
    /// journaled, so "no leg was sent" has a reason beside it.
    pub fn send_leg(
        &mut self,
        opportunity: &str,
        leg: &str,
        now: Timestamp,
        gate: &dyn Fn(&EpochLeg) -> Result<()>,
    ) -> Result<()> {
        match self.try_send_leg(opportunity, leg, now, gate) {
            Ok(()) => Ok(()),
            Err(r) => Err(self.refuse(r)),
        }
    }

    fn try_send_leg(
        &mut self,
        opportunity: &str,
        leg: &str,
        now: Timestamp,
        gate: &dyn Fn(&EpochLeg) -> Result<()>,
    ) -> std::result::Result<(), Rejected> {
        self.clock_in_bound()?;
        let st = self.opportunities.get_mut(opportunity).ok_or((
            "unknown_opportunity",
            format!("no epoch held for `{opportunity}`"),
        ))?;
        if !st.fired {
            return Err((
                "before_fire",
                format!("leg `{leg}` of `{opportunity}` not sent: no leg is sent before FIRE"),
            ));
        }
        if now >= st.epoch.expires_at() {
            return Err((
                "expired",
                format!(
                    "leg `{leg}` of `{opportunity}` not sent: epoch expired; nothing fires at or \
                     after expiry"
                ),
            ));
        }
        let l = st
            .leg(leg)
            .filter(|l| l.node == self.node)
            .cloned()
            .ok_or(("unknown_leg", format!("`{leg}` is not a leg of this node")))?;
        gate(&l).map_err(|e| {
            (
                "gate_refused",
                format!("local gate refused leg `{leg}` after FIRE: {e}"),
            )
        })?;
        st.sent.insert(l.leg);
        Ok(())
    }
}

/// What a participant's attempt to reserve its own legs produced, as the
/// frames it now has to send.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReservationOutcome {
    /// Every one of this node's legs passed its gate and is held: one
    /// `Reserved` frame per leg.
    Reserved(Vec<Vec<u8>>),
    /// The gate or the book refused. Nothing is held, and this is the
    /// `Declined` frame that tells the coordinator.
    Declined(Vec<u8>),
}

/// A node's reservations for epochs, bounded in amount by the leg's
/// requirement and the node's remaining grant, and in time by the epoch.
#[derive(Debug)]
pub struct ReservationBook {
    grant: Decimal,
    held: BTreeMap<(String, String), (Decimal, Timestamp)>,
}

impl ReservationBook {
    pub fn new(grant: Decimal) -> Self {
        Self {
            grant,
            held: BTreeMap::new(),
        }
    }

    /// Grant not yet reserved.
    pub fn available(&self) -> Decimal {
        self.held.values().fold(self.grant, |a, (amt, _)| a - *amt)
    }

    pub fn reserve(&mut self, epoch: &OpportunityEpoch, leg: &str, amount: Decimal) -> Result<()> {
        let l = epoch.legs.iter().find(|l| l.leg == leg).ok_or_else(|| {
            Error::denied(format!("`{leg}` is not a leg of `{}`", epoch.opportunity))
        })?;
        if !amount.is_positive() || amount > l.size {
            return Err(Error::denied(format!(
                "reservation of {amount} is not within leg `{leg}`'s requirement of {}",
                l.size
            )));
        }
        let key = (epoch.opportunity.clone(), leg.to_string());
        if self.held.contains_key(&key) {
            return Err(Error::denied(format!(
                "leg `{leg}` of `{}` is already reserved; release it first",
                epoch.opportunity
            )));
        }
        if amount > self.available() {
            return Err(Error::denied(format!(
                "reservation of {amount} exceeds the remaining grant of {}",
                self.available()
            )));
        }
        self.held.insert(key, (amount, epoch.expires_at()));
        Ok(())
    }

    /// Release one leg's hold, as when a sibling leg of the same node was
    /// refused and the node's reservation has to be whole or absent.
    pub fn release_leg(&mut self, opportunity: &str, leg: &str) {
        self.held
            .remove(&(opportunity.to_string(), leg.to_string()));
    }

    /// Release everything held for an epoch that completed or was rejected.
    pub fn release_epoch(&mut self, opportunity: &str) {
        self.held.retain(|(o, _), _| o != opportunity);
    }

    /// Release every reservation whose epoch has reached its expiry.
    pub fn expire(&mut self, now: Timestamp) {
        self.held.retain(|_, (_, expiry)| now < *expiry);
    }
}

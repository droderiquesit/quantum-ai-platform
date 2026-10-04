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
}

/// A frame the endpoint refused, kept so a refusal is journaled and never
/// silent. `kind` is a short stable token (`stale_epoch`, `expired`, ...).
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
}

impl PeerEndpoint {
    pub fn new(node: impl Into<String>, limits: PeerLimits) -> Self {
        Self {
            node: node.into(),
            limits,
            opportunities: BTreeMap::new(),
            refusals: Vec::new(),
        }
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
                st.held.insert(leg, amount);
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
            },
        );
        Ok(())
    }

    /// Declare FIRE for an opportunity this node coordinates. Refused unless
    /// every leg is reserved, the epoch is live, and the worst recovery loss
    /// fits the bound the epoch declared. Returns the frame to send peers.
    pub fn declare_fire(&mut self, opportunity: &str, now: Timestamp) -> Result<Vec<u8>> {
        let max = self.limits.max_frame;
        let st = self
            .opportunities
            .get_mut(opportunity)
            .ok_or_else(|| Error::denied(format!("no epoch held for `{opportunity}`")))?;
        if now >= st.epoch.expires_at() {
            return Err(Error::denied(
                "epoch expired; nothing fires at or after expiry",
            ));
        }
        if !st.every_leg_reserved() {
            return Err(Error::denied(
                "FIRE refused: not every leg is reserved at its full size",
            ));
        }
        let worst = st
            .worst_recovery_loss()
            .ok_or_else(|| Error::numeric("worst recovery loss overflowed"))?;
        if worst > st.epoch.max_recovery_loss {
            return Err(Error::denied(format!(
                "FIRE refused: worst recovery loss {worst} exceeds the declared bound {}; \
                 raise the unwind policy or shrink the cycle",
                st.epoch.max_recovery_loss
            )));
        }
        st.fired = true;
        encode(
            &PeerMessage::Fire {
                opportunity: opportunity.into(),
                sequence: st.epoch.sequence,
            },
            max,
        )
    }

    /// Send one of this node's legs. Needs FIRE and a live epoch, and the
    /// local gate decides last: a FIRE never bypasses it.
    pub fn send_leg(
        &mut self,
        opportunity: &str,
        leg: &str,
        now: Timestamp,
        gate: &dyn Fn(&EpochLeg) -> Result<()>,
    ) -> Result<()> {
        let st = self
            .opportunities
            .get_mut(opportunity)
            .ok_or_else(|| Error::denied(format!("no epoch held for `{opportunity}`")))?;
        if !st.fired {
            return Err(Error::denied("no leg is sent before FIRE"));
        }
        if now >= st.epoch.expires_at() {
            return Err(Error::denied(
                "epoch expired; nothing fires at or after expiry",
            ));
        }
        let l = st
            .leg(leg)
            .filter(|l| l.node == self.node)
            .cloned()
            .ok_or_else(|| Error::denied(format!("`{leg}` is not a leg of this node")))?;
        gate(&l)?;
        st.sent.insert(l.leg);
        Ok(())
    }
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

    /// Release everything held for an epoch that completed or was rejected.
    pub fn release_epoch(&mut self, opportunity: &str) {
        self.held.retain(|(o, _), _| o != opportunity);
    }

    /// Release every reservation whose epoch has reached its expiry.
    pub fn expire(&mut self, now: Timestamp) {
        self.held.retain(|_, (_, expiry)| now < *expiry);
    }
}

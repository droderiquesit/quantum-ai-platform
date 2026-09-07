//! The identity of one gate assessment, minted before the gate runs.
//!
//! §37.4 requires three enforcement points to agree before capital leaves a
//! venue. [`crate::custody::EnforcementPoints::all_agree`] proves that three
//! spoke; what an attestation *agreed to* is its
//! [`crate::custody::Attestation::reference`], and a reference validated only
//! for being non-empty is a reference any assessment satisfies. That is the
//! same defect the venue-allowlist mirror already closed once, where an
//! attestation filed against one address satisfied a corridor running to any
//! other.
//!
//! # Why a content digest and not the record's event id
//!
//! The obvious identity for a gate attestation is the id of the record the
//! decision is written under. It cannot be used, and the reason is an
//! ordering one rather than a preference: [`crate::journal::FabricJournal::decide`]
//! mints the [`qip_core::EventId`] *after*
//! [`crate::gate::TransferGate::assess`] has already run and returned its
//! verdict. The value an attestor would have to name does not exist at the
//! seam that would check it, and the attestor files its attestation before
//! the assessment, not after. A control cannot compare against a number that
//! is minted downstream of it.
//!
//! The prior chain head was the other candidate and is rejected for a
//! different reason: it identifies *when* an assessment was made and not
//! *what* was assessed, so one attestation would bind to whatever the gate
//! happened to be asked next at that chain position — which is the property
//! being replaced, moved one step along.
//!
//! So the identity is derived from the assessment's own content: the corridor
//! it runs on, the source it leaves, the destination it reaches, the amount,
//! and the instant it is assessed at. Those five are the movement an attestor
//! is agreeing to. Every one of them exists before `assess` is called, all
//! five are carried on the [`crate::journal::GateCommand`], and a replay
//! re-derives the digest from the record rather than trusting it — so an
//! attestation lifted from another assessment names a digest that does not
//! match, whichever of the five differs.
//!
//! Two assessments agreeing in all five are the same question asked twice at
//! the same instant on the same corridor, and the gate is deterministic, so
//! they receive the same answer. Sharing an identity is therefore honest
//! rather than a collision: there is nothing to distinguish and no second
//! movement to attribute.
//!
//! # Why the world's state is deliberately not in the digest
//!
//! The balances, the carried history, the breaker and the kill switch are all
//! inputs to the gate and none of them is in the digest. They are what the
//! *platform* knows, not what the attestor agreed to, and folding them in
//! would invalidate an attestation every time a balance moved — which turns
//! the control into one nobody can satisfy, and a control nobody can satisfy
//! is removed rather than fixed. What the attestor agreed to is the movement.
//! Whether the platform can afford it is checks 2 through 7's question.
//!
//! # Encoding
//!
//! The digest material is length-prefixed field by field
//! ([`push_field`]), so no field's content can be read as a
//! separator and no two distinct tuples produce one string. A digest joined
//! on a delimiter would be exactly the ambiguity
//! [`crate::custody::CustodyPolicy::mirrors_the_venue_allowlist`] documents in
//! `asset@address`, rebuilt one level up. The numeric fields are digested from
//! their underlying integers — [`qip_core::Decimal`] is a scaled `i128` and
//! [`qip_core::Timestamp`] a nanosecond `i64` — so equality of the value and
//! equality of the digested bytes are the same relation, rather than a
//! property of a `Display` impl in another crate.
//!
//! SHA-256 comes from [`qip_core`], which ADR 0002 authorises by name. No
//! dependency is added.

use crate::corridor::CorridorId;
use crate::destination::DestinationKey;
use crate::location::CapitalLocation;
use qip_core::{Decimal, Timestamp, sha256_hex};
use serde::{Deserialize, Serialize};
use std::fmt;

/// Domain separation for the assessment digest.
///
/// Prefixed so that an assessment identity can never equal a custody-policy
/// fingerprint even if every other byte of material coincided. Two digests
/// over different things that could collide would let a `custody_policy`
/// attestation satisfy the `transfer_gate` binding.
const ASSESSMENT_DOMAIN: &str = "qip.capital-fabric.assessment.v1";

/// Append one field to digest material as `len:content|`.
///
/// Length-prefixed rather than delimiter-joined, because a delimiter is only a
/// separator until a field contains one. `USDC` + `@a@b` and `USDC@a` + `@b`
/// join to the same string and hash to the same digest; prefixed by length
/// they cannot. The trailing `|` is for a human reading the material in a
/// failing test and is not what makes the encoding injective.
pub(crate) fn push_field(material: &mut String, field: &str) {
    use fmt::Write as _;
    // `write!` to a `String` is infallible; the result is discarded rather
    // than unwrapped, which the workspace forbids outside tests.
    let _ = write!(material, "{}:{}|", field.len(), field);
}

/// The identity of one gate assessment: a digest of the movement being
/// assessed.
///
/// Minted by [`AssessmentId::of`] before
/// [`crate::gate::TransferGate::assess`] runs, so the value a
/// [`crate::custody::EnforcementPoint::TransferGate`] attestation must name
/// exists at the seam that checks it. See the module documentation for why it
/// is not the record's event id.
///
/// It is not a capability and unlocks nothing. Holding one, or naming one in
/// an attestation, moves no capital: ADR 0021 leaves this platform with no
/// path an agreement could unlock, and this type only decides whether the gate
/// refuses.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct AssessmentId(String);

impl AssessmentId {
    /// Derive the identity of the assessment of `amount` from `source` to
    /// `destination` on `corridor` at `at`.
    ///
    /// The destination's asset and address are digested as two fields rather
    /// than as their `asset@address` rendering, for the reason
    /// [`crate::custody::CustodyPolicy::mirrors_the_venue_allowlist`] gives at
    /// length: that rendering reaches this crate off the event log, where
    /// serde builds an `Asset` from its field and calls no constructor, so a
    /// key whose asset holds an `@` renders exactly as a different key does.
    /// Two destinations sharing one digest would be one attestation binding to
    /// two movements.
    pub fn of(
        corridor: &CorridorId,
        source: &CapitalLocation,
        destination: &DestinationKey,
        amount: Decimal,
        at: Timestamp,
    ) -> Self {
        let mut material = String::new();
        push_field(&mut material, ASSESSMENT_DOMAIN);
        push_field(&mut material, corridor.as_str());
        push_field(&mut material, source.region.as_str());
        push_field(&mut material, source.currency.as_str());
        push_field(&mut material, source.venue.as_str());
        push_field(&mut material, destination.asset.as_str());
        push_field(&mut material, &destination.address);
        push_field(&mut material, &amount.raw().to_string());
        push_field(&mut material, &at.as_nanos().to_string());
        Self(sha256_hex(material.as_bytes()))
    }

    /// The digest, lower-case hex, as an attestation would reference it.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for AssessmentId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

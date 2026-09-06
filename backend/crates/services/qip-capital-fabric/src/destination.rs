//! The destination registry: where capital is *permitted* to go, as a record.
//!
//! Blueprint §38.4 puts the control on the corridor rather than the
//! transaction: a human verifies a destination out of band, signs it with a
//! hardware key, and the system waits twenty-four hours before anything may be
//! addressed to it. The blast radius of a compromised transfer engine is then
//! the allowlist — it can only move money to places a human already verified,
//! and it cannot invent a destination.
//!
//! This module is the registry half of that control and only the registry
//! half. It records who proposed a destination, who verified it, whose
//! signature covers it and when, and it answers one question:
//! [`DestinationRegistry::usable`] at a platform time the caller supplies. It
//! holds no key, verifies no signature cryptographically, and has no method
//! that sends anything anywhere. Under ADR 0021 that is the whole of what may
//! exist here: a signed corridor that authorises nothing is a data structure,
//! and a registry that refuses is the half worth having.
//!
//! # Why the clock is an argument
//!
//! Every check takes a [`Timestamp`] rather than reading one. A registry that
//! consults the wall clock cannot be replayed, and the twenty-four hour delay
//! is exactly the kind of control whose test would otherwise have to sleep for
//! a day or be quietly shortened for the test — after which nothing proves the
//! deployed value is still a day.

use qip_core::error::{Error, Result};
use qip_core::{Duration, Timestamp};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;

/// How long a newly signed destination waits before it can be used.
///
/// Twenty-four hours, from §37.1 and §38.4. A constant rather than a parameter
/// so there is no constructor through which a test or a deployment can pass a
/// shorter one: the delay exists to give a human who signed the wrong address
/// a day to notice, and a configurable delay is a delay somebody will set to
/// zero on the day it mattered.
pub const ACTIVATION_DELAY: Duration = Duration::from_days(1);

/// The thing being moved — a currency code, a token symbol, a collateral line.
///
/// A newtype so an asset cannot be compared against a venue name or an
/// address by accident; a destination is keyed on the pair, and a key built
/// from the wrong two strings would allowlist a place nobody verified.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Asset(String);

impl Asset {
    /// Name an asset. Refuses an empty name: an empty asset would key a
    /// destination that matches nothing a transfer could name and still read
    /// as an allowlist entry.
    pub fn new(name: impl Into<String>) -> Result<Self> {
        let name = name.into();
        if name.trim().is_empty() {
            return Err(Error::invalid("an asset needs a name"));
        }
        Ok(Self(name))
    }

    /// The asset's name.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Asset {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// What a destination is keyed on: the asset and the address or institution
/// it would be sent to.
///
/// Ordered so the registry iterates deterministically and a dump of it is
/// diffable against yesterday's during an incident.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct DestinationKey {
    /// The asset this destination accepts.
    pub asset: Asset,
    /// The address, account or institution identifier, exactly as verified.
    pub address: String,
}

impl DestinationKey {
    /// Build a key. Refuses an empty address for the same reason [`Asset::new`]
    /// refuses an empty name.
    pub fn new(asset: Asset, address: impl Into<String>) -> Result<Self> {
        let address = address.into();
        if address.trim().is_empty() {
            return Err(Error::invalid(format!(
                "a {asset} destination needs an address or institution"
            )));
        }
        Ok(Self { asset, address })
    }
}

impl fmt::Display for DestinationKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}@{}", self.asset, self.address)
    }
}

/// A human identity attached to a registry action.
///
/// A record of *who*, kept so that every stage change in the registry can be
/// attributed after the fact. It carries no credential and proves nothing
/// about the person; authentication happened wherever the action was taken,
/// and this is the name that was recorded there.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Approver(String);

impl Approver {
    /// Name an approver. Refuses an empty name: an action attributed to nobody
    /// is an action nobody can be asked about.
    pub fn new(name: impl Into<String>) -> Result<Self> {
        let name = name.into();
        if name.trim().is_empty() {
            return Err(Error::invalid(
                "an approver needs a name; an unattributed approval cannot be audited",
            ));
        }
        Ok(Self(name))
    }

    /// The name recorded.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Approver {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// The record that a human signed something with a hardware key, out of band.
///
/// This is a *record of* a signature and not a signature: it holds the
/// signer's name, when they signed, and the reference under which the signed
/// artefact was filed. No key material, no digest, nothing this crate could
/// verify or produce. ADR 0021 refuses the signing half of §37 outright and
/// ADR 0009 forbids in-tree cryptography, so the platform's knowledge of a
/// signature is limited to the fact that a person says one exists and where
/// to find it — which is enough for the gate to refuse a corridor that has
/// none.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignatureRecord {
    /// Who signed.
    pub signer: Approver,
    /// When, on the platform clock the caller supplied.
    pub signed_at: Timestamp,
    /// Where the signed artefact is filed, so an auditor can find it.
    pub reference: String,
}

impl SignatureRecord {
    /// Record a signature. Refuses an empty reference: a signature nobody can
    /// locate is a claim rather than a record.
    pub fn new(
        signer: Approver,
        signed_at: Timestamp,
        reference: impl Into<String>,
    ) -> Result<Self> {
        let reference = reference.into();
        if reference.trim().is_empty() {
            return Err(Error::invalid(
                "a signature record needs a filing reference an auditor can follow",
            ));
        }
        Ok(Self {
            signer,
            signed_at,
            reference,
        })
    }
}

/// Where a destination sits in §38.4's table.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DestinationStatus {
    /// Added as a candidate. Can be watched; nothing can be sent to it.
    Proposed,
    /// Confirmed directly with the institution by a human, never from inside
    /// the platform.
    Verified {
        /// Who confirmed it.
        by: Approver,
        /// When.
        at: Timestamp,
    },
    /// A signature record covers it; usable once [`ACTIVATION_DELAY`] has
    /// elapsed after the signature's own timestamp.
    Signed {
        /// The record of the hardware-key signature.
        signature: SignatureRecord,
        /// The first instant at which the destination may be used.
        usable_from: Timestamp,
    },
    /// Withdrawn by a human. Permanent: a revoked destination is never
    /// re-proposed under the same key, because "we removed it and then it
    /// came back" is indistinguishable from an attacker re-adding it.
    Revoked {
        /// Who revoked it.
        by: Approver,
        /// When.
        at: Timestamp,
    },
}

impl DestinationStatus {
    /// The stage name, for refusals and logs.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Proposed => "proposed",
            Self::Verified { .. } => "verified",
            Self::Signed { .. } => "signed",
            Self::Revoked { .. } => "revoked",
        }
    }
}

/// One destination and everything the registry knows about it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DestinationRecord {
    /// Who proposed it.
    pub proposed_by: Approver,
    /// When.
    pub proposed_at: Timestamp,
    /// Where it is in its life.
    pub status: DestinationStatus,
}

/// The allowlist, keyed on [`DestinationKey`].
///
/// Every mutation moves a record one stage forward and refuses anything else,
/// so a destination cannot be signed without having been verified, and a
/// revoked one cannot be quietly proposed again. The only read that matters
/// is [`usable`], and it refuses by naming the stage the destination is in
/// and, where the answer is "not yet", the instant at which it becomes yes.
///
/// [`usable`]: DestinationRegistry::usable
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DestinationRegistry {
    entries: BTreeMap<DestinationKey, DestinationRecord>,
}

impl DestinationRegistry {
    /// An empty allowlist, which permits nothing. The safe default.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a destination as a candidate. It can be watched; nothing can be
    /// sent to it.
    ///
    /// Refuses a key already present in any stage. Re-proposing a revoked
    /// destination in particular must fail, since that is the move an
    /// attacker with a proposal credential would make.
    pub fn propose(&mut self, key: DestinationKey, by: Approver, at: Timestamp) -> Result<()> {
        if let Some(existing) = self.entries.get(&key) {
            return Err(Error::invalid(format!(
                "destination {key} is already registered and is {}; a destination is proposed \
                 once and never re-proposed under the same key",
                existing.status.as_str()
            )));
        }
        self.entries.insert(
            key,
            DestinationRecord {
                proposed_by: by,
                proposed_at: at,
                status: DestinationStatus::Proposed,
            },
        );
        Ok(())
    }

    /// Record that a human confirmed the destination directly with the
    /// institution.
    ///
    /// Only a proposed destination can be verified: verifying a signed one
    /// would be a second, later claim about the same fact, and the two would
    /// disagree about the date.
    pub fn verify(&mut self, key: &DestinationKey, by: Approver, at: Timestamp) -> Result<()> {
        let record = self.entry_mut(key)?;
        match &record.status {
            DestinationStatus::Proposed => {
                record.status = DestinationStatus::Verified { by, at };
                Ok(())
            }
            other => Err(Error::invalid(format!(
                "destination {key} is {} and only a proposed destination can be verified",
                other.as_str()
            ))),
        }
    }

    /// Record that a hardware-key signature covers the destination.
    ///
    /// The delay starts from the signature's own timestamp rather than from
    /// the moment the record was entered, so a signature dated earlier than
    /// the entry does not gain a head start it was never given.
    ///
    /// Refuses a signature dated before the verification it depends on: a
    /// signature that predates the out-of-band check covered an unverified
    /// address, which is the thing the whole table exists to prevent.
    pub fn record_signature(
        &mut self,
        key: &DestinationKey,
        signature: SignatureRecord,
    ) -> Result<()> {
        let record = self.entry_mut(key)?;
        match &record.status {
            DestinationStatus::Verified { at, .. } => {
                if signature.signed_at < *at {
                    return Err(Error::invalid(format!(
                        "destination {key} was verified at {at} but the signature is dated \
                         {}; a signature that predates verification covered an unverified \
                         address",
                        signature.signed_at
                    )));
                }
                let usable_from = signature.signed_at.saturating_add(ACTIVATION_DELAY);
                record.status = DestinationStatus::Signed {
                    signature,
                    usable_from,
                };
                Ok(())
            }
            other => Err(Error::invalid(format!(
                "destination {key} is {} and only a verified destination can be signed",
                other.as_str()
            ))),
        }
    }

    /// Withdraw a destination permanently. Any human, from any stage but
    /// revoked, with no delay — §37.2 removes delays only from operations
    /// that cannot widen where money goes, and this one narrows it.
    pub fn revoke(&mut self, key: &DestinationKey, by: Approver, at: Timestamp) -> Result<()> {
        let record = self.entry_mut(key)?;
        if let DestinationStatus::Revoked { by: earlier, at } = &record.status {
            return Err(Error::invalid(format!(
                "destination {key} was already revoked by {earlier} at {at}"
            )));
        }
        record.status = DestinationStatus::Revoked { by, at };
        Ok(())
    }

    /// Whether the destination may be used at `now`, and if not, why not.
    ///
    /// The one read the transfer gate makes. Refuses a destination the
    /// registry has never seen, one in any stage before signed, one whose
    /// delay has not elapsed (naming the instant it will have), and one that
    /// was revoked. Returns the record so the caller can cite the signature it
    /// relied on.
    pub fn usable(&self, key: &DestinationKey, now: Timestamp) -> Result<&DestinationRecord> {
        let record = self.entries.get(key).ok_or_else(|| {
            Error::denied(format!(
                "destination {key} is not on the allowlist; propose it, verify it out of band \
                 with the institution, record the signature and wait {} before using it",
                describe(ACTIVATION_DELAY)
            ))
        })?;
        match &record.status {
            DestinationStatus::Signed { usable_from, .. } => {
                if now < *usable_from {
                    return Err(Error::denied(format!(
                        "destination {key} was signed but its {} delay has not elapsed; it \
                         becomes usable at {usable_from}, and it is {now}",
                        describe(ACTIVATION_DELAY)
                    )));
                }
                Ok(record)
            }
            DestinationStatus::Proposed => Err(Error::denied(format!(
                "destination {key} is proposed and unverified; a human must confirm it \
                 directly with the institution before it can be signed"
            ))),
            DestinationStatus::Verified { .. } => Err(Error::denied(format!(
                "destination {key} is verified but unsigned; a hardware-key signature record \
                 must cover it before the delay can start"
            ))),
            DestinationStatus::Revoked { by, at } => Err(Error::denied(format!(
                "destination {key} was revoked by {by} at {at} and revocation is permanent"
            ))),
        }
    }

    /// The record for a key, in any stage.
    pub fn get(&self, key: &DestinationKey) -> Option<&DestinationRecord> {
        self.entries.get(key)
    }

    /// Every destination in key order.
    pub fn iter(&self) -> impl Iterator<Item = (&DestinationKey, &DestinationRecord)> {
        self.entries.iter()
    }

    /// How many destinations are registered, in any stage.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether nothing is registered.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    fn entry_mut(&mut self, key: &DestinationKey) -> Result<&mut DestinationRecord> {
        self.entries.get_mut(key).ok_or_else(|| {
            Error::not_found(format!(
                "destination {key} is not registered; propose it first"
            ))
        })
    }
}

/// Render a delay for a refusal, in hours.
pub(crate) fn describe(delay: Duration) -> String {
    let hours = delay.as_nanos() / qip_core::time::NANOS_PER_HOUR;
    format!("{hours}h")
}

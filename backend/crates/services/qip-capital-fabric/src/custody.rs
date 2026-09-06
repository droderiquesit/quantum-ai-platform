//! The custody policy as data (blueprint §37.4, ADR 0021).
//!
//! §37.4 is a table: for each class of asset, who holds it and through which
//! kind of corridor it may ever leave. ADR 0021 permits that table and refuses
//! the machinery the blueprint puts behind it — the policy *engine* that holds
//! a share of a signing key and releases it on approval. So this module holds
//! the table and nothing else: [`CustodyPolicy::permits`] answers whether a
//! class may leave through a corridor kind, and no type here can sign, hold a
//! share of anything, or release anything. The self-custody row's rule — *no
//! single component can sign* — is recorded as the policy fact
//! [`ClassConstraints::requires_multi_party_release`], which the policy refuses
//! to construct as `false`, and there is deliberately no type that could act on
//! it.
//!
//! §37.4 closes with a rule about *who* may agree to a movement: three
//! independent enforcement points — the venue's own allowlist configured out of
//! band, the platform's corridor gate, and the custody policy — must all agree
//! before capital leaves a venue, and trading authority and transfer authority
//! never share an identity. [`EnforcementPoints::all_agree`] is that rule as a
//! check over attestation records. A record is a claim that a point agreed,
//! not a mechanism by which it did; three records that agree authorise nothing
//! here, because there is nothing here for them to authorise.

use qip_core::Timestamp;
use qip_core::error::{Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

/// The five asset classes of §37.4's custody table.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CustodyClass {
    /// Fiat at a broker or bank: the institution of record holds it.
    FiatAtInstitutionOfRecord,
    /// Crypto the venue custodies; the venue's allowlist governs where it may go.
    CryptoInVenueCustody,
    /// Crypto in self-custody; no single component can sign for it.
    CryptoSelfCustody,
    /// Collateral and margin posted at a venue: inventory, never a transfer.
    CollateralAndMargin,
    /// Commitments to private funds, held by the administrator.
    PrivateCommitment,
}

impl CustodyClass {
    /// Every class, in table order.
    pub const ALL: [Self; 5] = [
        Self::FiatAtInstitutionOfRecord,
        Self::CryptoInVenueCustody,
        Self::CryptoSelfCustody,
        Self::CollateralAndMargin,
        Self::PrivateCommitment,
    ];

    /// A stable label for logs and refusals.
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::FiatAtInstitutionOfRecord => "fiat_at_institution_of_record",
            Self::CryptoInVenueCustody => "crypto_in_venue_custody",
            Self::CryptoSelfCustody => "crypto_self_custody",
            Self::CollateralAndMargin => "collateral_and_margin",
            Self::PrivateCommitment => "private_commitment",
        }
    }
}

impl fmt::Display for CustodyClass {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Who holds an asset class, per §37.4's "Custody" column.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Custodian {
    /// The broker or bank of record.
    InstitutionOfRecord,
    /// The trading venue itself.
    Venue,
    /// The platform's own custody, under multi-party control.
    SelfCustody,
    /// The fund administrator.
    FundAdministrator,
}

/// The kinds of corridor §37.4 and §38.2 name as the only ways capital moves.
///
/// Each is a *name for a route*, not a route. A corridor kind in this enum
/// says which external approval flow a movement of that class would have to
/// pass through; nothing in this crate can enter one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CorridorKind {
    /// A payment or transfer request placed into the institution's own
    /// approval flow (a bank's or custodian's), which decides independently.
    InstitutionApprovalFlow,
    /// A transfer between two accounts at the same institution; nothing
    /// leaves it.
    InternalAtSameInstitution,
    /// A venue-side withdrawal to an address on the venue's own allowlist,
    /// configured out of band and mirrored by the corridor registry.
    VenueAllowlistedWithdrawal,
    /// An on-chain movement from self-custody, permissible only after the
    /// gate has approved and only under multi-party release.
    OnChainAfterGateApproval,
    /// A capital call paid from reserve to a fund administrator.
    CapitalCallFromReserve,
}

impl CorridorKind {
    /// A stable label for logs and refusals.
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::InstitutionApprovalFlow => "institution_approval_flow",
            Self::InternalAtSameInstitution => "internal_at_same_institution",
            Self::VenueAllowlistedWithdrawal => "venue_allowlisted_withdrawal",
            Self::OnChainAfterGateApproval => "on_chain_after_gate_approval",
            Self::CapitalCallFromReserve => "capital_call_from_reserve",
        }
    }
}

impl fmt::Display for CorridorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One row of §37.4, as constraints rather than prose.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClassConstraints {
    /// Who holds the asset.
    pub custodian: Custodian,
    /// The corridor kinds through which the class may ever leave. Empty means
    /// it never does.
    pub permitted_corridors: BTreeSet<CorridorKind>,
    /// Whether the class may ever be the source of a transfer. Collateral and
    /// margin are inventory: they are drawn down or released at the venue and
    /// never moved from it.
    pub may_be_transfer_source: bool,
    /// Whether the venue's own allowlist, configured out of band, must be
    /// mirrored by the corridor registry before a corridor is permissible.
    pub venue_allowlist_mirrored: bool,
    /// §37.4's rule for self-custody, as a fact: *no single component can
    /// sign*. `true` means any release requires more than one independent
    /// party, and the policy refuses to be built with it `false` for
    /// [`CustodyClass::CryptoSelfCustody`]. This is a constraint on any future
    /// mechanism and is not itself one: nothing in this crate can release, and
    /// nothing holds a share of anything that could.
    pub requires_multi_party_release: bool,
}

/// Why the custody policy refused.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum RefusalReason {
    /// The class is not in the policy at all.
    ClassNotInPolicy,
    /// The class never leaves its custodian — inventory, not a transfer.
    ClassNeverTransfers,
    /// The class may transfer, but not through this kind of corridor.
    CorridorNotPermittedForClass,
    /// An enforcement point has not attested.
    EnforcementPointMissing {
        /// Which one.
        point: EnforcementPoint,
    },
    /// Two enforcement points attested under one identity, so they are not
    /// independent.
    SharedIdentity {
        /// One of them.
        first: EnforcementPoint,
        /// The other.
        second: EnforcementPoint,
    },
    /// An enforcement point attested under the identity that trades.
    TradingIdentityHoldsTransferAuthority {
        /// Which point.
        point: EnforcementPoint,
    },
}

impl RefusalReason {
    /// A stable label for logs and metrics.
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::ClassNotInPolicy => "class_not_in_policy",
            Self::ClassNeverTransfers => "class_never_transfers",
            Self::CorridorNotPermittedForClass => "corridor_not_permitted_for_class",
            Self::EnforcementPointMissing { .. } => "enforcement_point_missing",
            Self::SharedIdentity { .. } => "shared_identity",
            Self::TradingIdentityHoldsTransferAuthority { .. } => {
                "trading_identity_holds_transfer_authority"
            }
        }
    }
}

/// A custody refusal, with what was asked and why it was declined.
///
/// Distinct from [`crate::plan::Refusal`], which is a pre-positioning lane
/// declined on price; this one is a class-and-corridor pairing the policy
/// forbids, and no figure would change it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Refusal {
    /// The class asked about, when the question was about one.
    pub class: Option<CustodyClass>,
    /// The corridor asked about, when the question was about one.
    pub corridor: Option<CorridorKind>,
    /// Why.
    pub reason: RefusalReason,
    /// What to do instead, in a sentence.
    pub detail: String,
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "custody refused ({}): {}",
            self.reason.as_str(),
            self.detail
        )
    }
}

/// §37.4 as a lookup: class → constraints.
///
/// Constructed either as [`CustodyPolicy::blueprint`], the table as written,
/// or from caller-supplied rows through [`CustodyPolicy::from_constraints`],
/// which refuses any table that contradicts the two rules §37.4 states
/// unconditionally: self-custody always requires multi-party release, and
/// collateral is never a transfer source and has no corridor. A policy that
/// could be configured to relax either would be a control that reads as one.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CustodyPolicy {
    classes: BTreeMap<CustodyClass, ClassConstraints>,
}

impl CustodyPolicy {
    /// The §37.4 table, verbatim as data.
    pub fn blueprint() -> Self {
        let mut classes = BTreeMap::new();
        classes.insert(
            CustodyClass::FiatAtInstitutionOfRecord,
            ClassConstraints {
                custodian: Custodian::InstitutionOfRecord,
                permitted_corridors: BTreeSet::from([
                    CorridorKind::InstitutionApprovalFlow,
                    CorridorKind::InternalAtSameInstitution,
                ]),
                may_be_transfer_source: true,
                venue_allowlist_mirrored: false,
                requires_multi_party_release: false,
            },
        );
        classes.insert(
            CustodyClass::CryptoInVenueCustody,
            ClassConstraints {
                custodian: Custodian::Venue,
                permitted_corridors: BTreeSet::from([CorridorKind::VenueAllowlistedWithdrawal]),
                may_be_transfer_source: true,
                venue_allowlist_mirrored: true,
                requires_multi_party_release: false,
            },
        );
        classes.insert(
            CustodyClass::CryptoSelfCustody,
            ClassConstraints {
                custodian: Custodian::SelfCustody,
                permitted_corridors: BTreeSet::from([CorridorKind::OnChainAfterGateApproval]),
                may_be_transfer_source: true,
                venue_allowlist_mirrored: false,
                requires_multi_party_release: true,
            },
        );
        classes.insert(
            CustodyClass::CollateralAndMargin,
            ClassConstraints {
                custodian: Custodian::Venue,
                permitted_corridors: BTreeSet::new(),
                may_be_transfer_source: false,
                venue_allowlist_mirrored: false,
                requires_multi_party_release: false,
            },
        );
        classes.insert(
            CustodyClass::PrivateCommitment,
            ClassConstraints {
                custodian: Custodian::FundAdministrator,
                permitted_corridors: BTreeSet::from([CorridorKind::CapitalCallFromReserve]),
                may_be_transfer_source: true,
                venue_allowlist_mirrored: false,
                requires_multi_party_release: false,
            },
        );
        Self { classes }
    }

    /// Build a policy from rows, refusing one that contradicts §37.4's
    /// unconditional rules.
    ///
    /// The failure this prevents is a "temporary" table in which self-custody
    /// is marked single-party, or collateral is given a corridor, reaching
    /// [`CustodyPolicy::permits`] and answering yes.
    pub fn from_constraints(classes: BTreeMap<CustodyClass, ClassConstraints>) -> Result<Self> {
        if let Some(row) = classes.get(&CustodyClass::CryptoSelfCustody)
            && !row.requires_multi_party_release
        {
            return Err(Error::denied(
                "self-custody must require multi-party release; §37.4 says no single \
                 component can sign, and a policy that says otherwise is refused rather \
                 than recorded",
            ));
        }
        if let Some(row) = classes.get(&CustodyClass::CollateralAndMargin)
            && (row.may_be_transfer_source || !row.permitted_corridors.is_empty())
        {
            return Err(Error::denied(
                "collateral and margin are inventory and never a transfer source; remove \
                 the corridor rather than the rule",
            ));
        }
        for (class, row) in &classes {
            if !row.may_be_transfer_source && !row.permitted_corridors.is_empty() {
                return Err(Error::invalid(format!(
                    "{class} is marked as never a transfer source yet lists {} corridor(s); \
                     one of the two is wrong and the policy will not guess which",
                    row.permitted_corridors.len()
                )));
            }
        }
        Ok(Self { classes })
    }

    /// The constraints for a class, if the policy has a row for it.
    pub fn constraints(&self, class: CustodyClass) -> Option<&ClassConstraints> {
        self.classes.get(&class)
    }

    /// Whether `class` may leave its custodian through a corridor of `kind`.
    ///
    /// A class the policy has no row for is refused, not assumed
    /// unrestricted: an unlisted class is the one nobody thought about.
    pub fn permits(
        &self,
        class: CustodyClass,
        kind: CorridorKind,
    ) -> std::result::Result<(), Refusal> {
        let Some(row) = self.classes.get(&class) else {
            return Err(Refusal {
                class: Some(class),
                corridor: Some(kind),
                reason: RefusalReason::ClassNotInPolicy,
                detail: format!("{class} has no row in the custody policy; add one before asking"),
            });
        };
        if !row.may_be_transfer_source {
            return Err(Refusal {
                class: Some(class),
                corridor: Some(kind),
                reason: RefusalReason::ClassNeverTransfers,
                detail: format!(
                    "{class} is inventory at its custodian and never a transfer source; \
                     draw it down or release it at the venue instead"
                ),
            });
        }
        if !row.permitted_corridors.contains(&kind) {
            let permitted: Vec<&str> = row.permitted_corridors.iter().map(|k| k.as_str()).collect();
            return Err(Refusal {
                class: Some(class),
                corridor: Some(kind),
                reason: RefusalReason::CorridorNotPermittedForClass,
                detail: format!(
                    "{class} may not leave through {kind}; its permitted corridors are [{}]",
                    permitted.join(", ")
                ),
            });
        }
        Ok(())
    }
}

/// The three enforcement points §37.4 requires to agree.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EnforcementPoint {
    /// The platform's deterministic transfer gate.
    TransferGate,
    /// The custody policy — this module's table.
    CustodyPolicy,
    /// The venue's own allowlist, configured out of band.
    VenueAllowlist,
}

impl EnforcementPoint {
    /// All three, in the order agreement is checked.
    pub const ALL: [Self; 3] = [
        Self::TransferGate,
        Self::CustodyPolicy,
        Self::VenueAllowlist,
    ];

    /// A stable label for logs and refusals.
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::TransferGate => "transfer_gate",
            Self::CustodyPolicy => "custody_policy",
            Self::VenueAllowlist => "venue_allowlist",
        }
    }
}

impl fmt::Display for EnforcementPoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The identity an enforcement point attests under.
///
/// A name, compared for equality and nothing more — it carries no credential,
/// and the platform cannot act as it. Non-empty by construction, because two
/// empty identities are equal and would fail the distinctness check for the
/// wrong reason, or — worse — an empty-string check elsewhere would pass them.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Identity(String);

impl Identity {
    /// Name an identity, refusing an empty or whitespace-only one.
    pub fn new(name: impl Into<String>) -> Result<Self> {
        let name = name.into();
        if name.trim().is_empty() {
            return Err(Error::invalid(
                "an attesting identity needs a name; an anonymous attestation is not one",
            ));
        }
        Ok(Self(name))
    }

    /// The identity's name.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Identity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A record that one enforcement point agreed, under one identity, with a
/// reference to what it agreed to.
///
/// A claim about the past, not a capability: holding three of these moves
/// nothing, because there is no code path in this platform that a set of
/// attestations could unlock.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attestation {
    /// Which point.
    pub point: EnforcementPoint,
    /// Under which identity.
    pub identity: Identity,
    /// What it agreed to — a gate decision id, a policy version, an allowlist
    /// entry reference. Non-empty, so an attestation always says what it is
    /// about.
    pub reference: String,
    /// When.
    pub attested_at: Timestamp,
}

impl Attestation {
    /// Record an attestation, refusing an empty reference.
    pub fn new(
        point: EnforcementPoint,
        identity: Identity,
        reference: impl Into<String>,
        attested_at: Timestamp,
    ) -> Result<Self> {
        let reference = reference.into();
        if reference.trim().is_empty() {
            return Err(Error::invalid(format!(
                "the {point} attestation names nothing it agreed to; an attestation without \
                 a reference cannot be checked against anything"
            )));
        }
        Ok(Self {
            point,
            identity,
            reference,
            attested_at,
        })
    }
}

/// The attestations gathered so far from the three enforcement points.
///
/// Each point may attest once. A second attestation from the same point is
/// refused rather than replacing the first, because "the gate agreed twice"
/// is not "two points agreed", and a structure that let a point re-attest is
/// one that lets it change its answer after the others have given theirs.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnforcementPoints {
    attestations: BTreeMap<EnforcementPoint, Attestation>,
}

impl EnforcementPoints {
    /// No attestations yet.
    pub fn new() -> Self {
        Self::default()
    }

    /// Record one point's attestation, refusing a point that already has one.
    pub fn attest(&mut self, attestation: Attestation) -> Result<()> {
        if self.attestations.contains_key(&attestation.point) {
            return Err(Error::denied(format!(
                "{} has already attested; a point does not attest twice, and this is not the \
                 way to change its answer",
                attestation.point
            )));
        }
        self.attestations.insert(attestation.point, attestation);
        Ok(())
    }

    /// The attestation a point has given, if any.
    pub fn attestation(&self, point: EnforcementPoint) -> Option<&Attestation> {
        self.attestations.get(&point)
    }

    /// §37.4's closing rule: all three points have attested, under three
    /// distinct identities.
    ///
    /// The failure this prevents: a gate and a custody policy both running
    /// under one service identity count as two approvals while being one
    /// decision made twice. Checked pairwise so the refusal names the two
    /// points that collapsed into one.
    pub fn all_agree(&self) -> std::result::Result<Agreement, Refusal> {
        let mut present = Vec::with_capacity(EnforcementPoint::ALL.len());
        for point in EnforcementPoint::ALL {
            match self.attestations.get(&point) {
                Some(attestation) => present.push(attestation.clone()),
                None => {
                    return Err(Refusal {
                        class: None,
                        corridor: None,
                        reason: RefusalReason::EnforcementPointMissing { point },
                        detail: format!(
                            "{point} has not attested; two of three points agreeing is not \
                             agreement"
                        ),
                    });
                }
            }
        }
        for (i, first) in present.iter().enumerate() {
            for second in &present[i + 1..] {
                if first.identity == second.identity {
                    return Err(Refusal {
                        class: None,
                        corridor: None,
                        reason: RefusalReason::SharedIdentity {
                            first: first.point,
                            second: second.point,
                        },
                        detail: format!(
                            "{} and {} both attested as {}; two points under one identity \
                             are one point, so they are not independent",
                            first.point, second.point, first.identity
                        ),
                    });
                }
            }
        }
        Ok(Agreement {
            attestations: present,
        })
    }
}

/// Three independent attestations, one per enforcement point, under three
/// distinct identities.
///
/// Evidence that the rule held, and only that. It is not a token and unlocks
/// nothing: ADR 0021 leaves this platform with no path for it to unlock.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Agreement {
    attestations: Vec<Attestation>,
}

impl Agreement {
    /// The attestations, in [`EnforcementPoint::ALL`] order.
    pub fn attestations(&self) -> &[Attestation] {
        &self.attestations
    }

    /// The other half of §37.4's closing rule: none of the three transfer
    /// identities is the identity that trades.
    ///
    /// The failure this prevents is the one the blueprint names outright —
    /// trading authority and transfer authority sharing an identity, so a
    /// compromised or runaway trading process could also attest to its own
    /// capital movement.
    pub fn disjoint_from_trading_authority(
        &self,
        trading: &Identity,
    ) -> std::result::Result<(), Refusal> {
        for attestation in &self.attestations {
            if &attestation.identity == trading {
                return Err(Refusal {
                    class: None,
                    corridor: None,
                    reason: RefusalReason::TradingIdentityHoldsTransferAuthority {
                        point: attestation.point,
                    },
                    detail: format!(
                        "{} attested as {trading}, which is the trading identity; transfer \
                         authority must attest under an identity that never trades",
                        attestation.point
                    ),
                });
            }
        }
        Ok(())
    }
}

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
//! never share an identity. [`EnforcementPoints::all_agree`] and
//! [`Agreement::disjoint_from_trading_authority`] are the two halves of that
//! rule, paired as [`TransferAuthority`], and
//! [`crate::gate::TransferGate::assess`] vetoes an intent whose authority
//! fails either half. A record is still a claim that a point agreed and not a
//! mechanism by which it did: three records that agree authorise no movement,
//! because there is no movement here to authorise. What they decide is whether
//! the gate refuses.
//!
//! Agreement is also asked *what* the venue agreed to, not only that it did.
//! [`EnforcementPoints::all_agree`] reads no attestation's `reference`, so a
//! venue-allowlist attestation filed against one address once satisfied a
//! corridor running to any other — the venue's allowlist counted as an
//! enforcement point while enforcing nothing about the destination.
//! [`CustodyPolicy::mirrors_the_venue_allowlist`] closes that for every class
//! whose row sets [`ClassConstraints::venue_allowlist_mirrored`], and
//! [`CustodyPolicy::conforms`] refuses a table that offers
//! [`CorridorKind::VenueAllowlistedWithdrawal`] with the flag clear, so the
//! check cannot be disabled by a row rather than by a review.
//!
//! Both halves were unreachable from any non-test caller until the gate was
//! wired to them. That is the defect `risk-and-execution.md` names by its
//! other instance — a limit that cannot fire reads as protection and is not —
//! and it is why the check lives in the control that a replay re-runs rather
//! than in a constructor a replay never calls.

use crate::destination::DestinationKey;
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
    /// mirrored before a corridor of this class is permissible.
    ///
    /// `true` obliges the [`EnforcementPoint::VenueAllowlist`] attestation to
    /// name the destination the corridor runs to, checked by
    /// [`CustodyPolicy::mirrors_the_venue_allowlist`] from inside the gate. It
    /// is what turns "the venue agreed to something" into "the venue agreed to
    /// *this address*", which is the only form of the claim worth having for a
    /// class whose sole corridor is a venue-side withdrawal.
    ///
    /// **This field read nothing until that check existed.** It was set by
    /// [`CustodyPolicy::blueprint`], published through the API, and consulted
    /// by no code — a documented precondition enforced nowhere, which is the
    /// shape `risk-and-execution.md` names by its other instance: a limit that
    /// cannot fire reads as protection and is not. [`CustodyPolicy::conforms`]
    /// now also refuses a table that lists
    /// [`CorridorKind::VenueAllowlistedWithdrawal`] for a class with this
    /// `false`, so the check cannot be switched off by a table arriving off
    /// the event log.
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
    /// The class requires the venue's own allowlist to be mirrored, and the
    /// venue-allowlist attestation does not name the destination in question.
    VenueAllowlistNotMirrored {
        /// The class whose row demanded the mirror.
        class: CustodyClass,
    },
    /// The table itself contradicts a rule §37.4 states unconditionally, so
    /// no answer it gives about that class can be relied on.
    PolicyContradictsBlueprint {
        /// The row that contradicts it.
        class: CustodyClass,
        /// Which rule.
        rule: PolicyRule,
    },
}

/// A rule §37.4 states unconditionally, which no custody table may contradict
/// whatever else it says.
///
/// Named rather than described so a refusal, a log line and a metric can all
/// say which rule was broken with the same token.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicyRule {
    /// Self-custody requires multi-party release: no single component can sign.
    SelfCustodyIsMultiParty,
    /// Collateral and margin are inventory and never a transfer source.
    CollateralNeverTransfers,
    /// A class that is not a transfer source lists no corridors.
    CorridorsImplyATransferSource,
    /// A class that may leave through a venue-side withdrawal mirrors the
    /// venue's own allowlist. §37.4 makes the venue's allowlist one of the
    /// three enforcement points, so a row offering that corridor while
    /// declaring the mirror unnecessary removes an enforcement point by
    /// setting a flag.
    VenueWithdrawalMirrorsTheAllowlist,
}

impl PolicyRule {
    /// A stable label for logs and refusals.
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::SelfCustodyIsMultiParty => "self_custody_is_multi_party",
            Self::CollateralNeverTransfers => "collateral_never_transfers",
            Self::CorridorsImplyATransferSource => "corridors_imply_a_transfer_source",
            Self::VenueWithdrawalMirrorsTheAllowlist => "venue_withdrawal_mirrors_the_allowlist",
        }
    }
}

impl fmt::Display for PolicyRule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
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
            Self::VenueAllowlistNotMirrored { .. } => "venue_allowlist_not_mirrored",
            Self::PolicyContradictsBlueprint { .. } => "policy_contradicts_blueprint",
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
///
/// Neither constructor is where those rules are *enforced*, because neither is
/// on the path a replayed record takes: this type is `Deserialize`, it travels
/// inside [`crate::journal::GateCommand`], and serde builds it from its fields.
/// [`CustodyPolicy::conforms`] states the rules, `from_constraints` calls it,
/// and [`crate::gate::TransferGate::assess`] calls it again on every
/// assessment, live and replayed alike.
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
        let policy = Self { classes };
        if let Err(refusal) = policy.conforms() {
            // The rule is stated once, in `conforms`; this maps its refusal to
            // the error class the caller sees. A contradiction of one of
            // §37.4's two named rules is `denied` — the table asked for
            // something the blueprint forbids outright — while a row that
            // lists corridors it also says can never be used is `invalid`:
            // internally inconsistent, and the policy will not guess which
            // half the caller meant.
            return Err(match refusal.reason {
                RefusalReason::PolicyContradictsBlueprint {
                    rule: PolicyRule::CorridorsImplyATransferSource,
                    ..
                } => Error::invalid(refusal.detail),
                _ => Error::denied(refusal.detail),
            });
        }
        Ok(policy)
    }

    /// Whether the table contradicts any rule §37.4 states unconditionally.
    ///
    /// **This is a check on the policy, not on a question asked of it, and it
    /// is re-run by [`crate::gate::TransferGate::assess`] on every assessment
    /// rather than only by [`CustodyPolicy::from_constraints`].** The reason
    /// is the one [`TransferAuthority`] already documents: a `CustodyPolicy`
    /// is carried inside [`crate::journal::GateCommand`] and so arrives
    /// deserialised straight off the event log, where a validating constructor
    /// is a check every replayed record walks past. `serde` builds this struct
    /// from its fields directly, so before the gate re-ran this rule a table
    /// that `from_constraints` refuses — collateral marked transferable with a
    /// corridor listed — could be written into a gate record, would make
    /// [`CustodyPolicy::permits`] answer *yes* for a class §37.4 says never
    /// moves, and would then be *confirmed* by the replay, because the replay
    /// re-executes the control and the control did not look. A hash chain
    /// proves a record has not changed; only the control re-asking the
    /// question proves it was true.
    ///
    /// Checked in §37.4's own order, so a table breaking more than one rule
    /// is named by the rule the blueprint states most narrowly.
    pub fn conforms(&self) -> std::result::Result<(), Refusal> {
        if let Some(row) = self.classes.get(&CustodyClass::CryptoSelfCustody)
            && !row.requires_multi_party_release
        {
            return Err(Refusal {
                class: Some(CustodyClass::CryptoSelfCustody),
                corridor: None,
                reason: RefusalReason::PolicyContradictsBlueprint {
                    class: CustodyClass::CryptoSelfCustody,
                    rule: PolicyRule::SelfCustodyIsMultiParty,
                },
                detail: "self-custody must require multi-party release; §37.4 says no single \
                         component can sign, and a policy that says otherwise is refused rather \
                         than recorded"
                    .to_string(),
            });
        }
        if let Some(row) = self.classes.get(&CustodyClass::CollateralAndMargin)
            && (row.may_be_transfer_source || !row.permitted_corridors.is_empty())
        {
            return Err(Refusal {
                class: Some(CustodyClass::CollateralAndMargin),
                corridor: None,
                reason: RefusalReason::PolicyContradictsBlueprint {
                    class: CustodyClass::CollateralAndMargin,
                    rule: PolicyRule::CollateralNeverTransfers,
                },
                detail: "collateral and margin are inventory and never a transfer source; remove \
                         the corridor rather than the rule"
                    .to_string(),
            });
        }
        for (class, row) in &self.classes {
            if !row.may_be_transfer_source && !row.permitted_corridors.is_empty() {
                return Err(Refusal {
                    class: Some(*class),
                    corridor: None,
                    reason: RefusalReason::PolicyContradictsBlueprint {
                        class: *class,
                        rule: PolicyRule::CorridorsImplyATransferSource,
                    },
                    detail: format!(
                        "{class} is marked as never a transfer source yet lists {} corridor(s); \
                         one of the two is wrong and the policy will not guess which",
                        row.permitted_corridors.len()
                    ),
                });
            }
        }
        for (class, row) in &self.classes {
            if row
                .permitted_corridors
                .contains(&CorridorKind::VenueAllowlistedWithdrawal)
                && !row.venue_allowlist_mirrored
            {
                return Err(Refusal {
                    class: Some(*class),
                    corridor: Some(CorridorKind::VenueAllowlistedWithdrawal),
                    reason: RefusalReason::PolicyContradictsBlueprint {
                        class: *class,
                        rule: PolicyRule::VenueWithdrawalMirrorsTheAllowlist,
                    },
                    detail: format!(
                        "{class} may leave through {kind} but its row says the venue's own \
                         allowlist need not be mirrored; §37.4 counts that allowlist as one of \
                         the three enforcement points, and a table that waives it removes a \
                         point by flipping a flag — set venue_allowlist_mirrored, or remove \
                         the corridor",
                        kind = CorridorKind::VenueAllowlistedWithdrawal
                    ),
                });
            }
        }
        Ok(())
    }

    /// Whether the venue's own allowlist has been mirrored *for this
    /// destination*, where the class's row demands it.
    ///
    /// §37.4 names the venue's out-of-band allowlist as one of the three
    /// enforcement points, and [`EnforcementPoints::all_agree`] proves it
    /// spoke. It does not prove *what it spoke about*: an
    /// [`Attestation::reference`] is checked only for being non-empty, so a
    /// venue-allowlist attestation filed against one address satisfied the
    /// agreement for a corridor running to any other. For a class whose
    /// [`ClassConstraints::venue_allowlist_mirrored`] is set — crypto in venue
    /// custody, whose sole corridor *is* a venue-side withdrawal — that is the
    /// difference between mirroring an allowlist and asserting one exists.
    ///
    /// So the reference must be the destination key exactly, compared whole
    /// rather than by containment: `USDC@addr-1` is a prefix of
    /// `USDC@addr-10`, and a substring check would admit the wrong address in
    /// the one place the address is the entire control.
    ///
    /// Asked by [`crate::gate::TransferGate::assess`] rather than at
    /// construction, for the reason [`CustodyPolicy::conforms`] gives at
    /// length: every input here arrives deserialised off the event log on a
    /// replay, and a rule only a constructor holds is a rule the replay never
    /// re-derives.
    pub fn mirrors_the_venue_allowlist(
        &self,
        class: CustodyClass,
        destination: &DestinationKey,
        agreement: &Agreement,
    ) -> std::result::Result<(), Refusal> {
        let Some(row) = self.classes.get(&class) else {
            return Err(Refusal {
                class: Some(class),
                corridor: None,
                reason: RefusalReason::ClassNotInPolicy,
                detail: format!("{class} has no row in the custody policy; add one before asking"),
            });
        };
        if !row.venue_allowlist_mirrored {
            return Ok(());
        }
        let expected = destination.to_string();
        // An absent attestation renders as the empty string, which a
        // `DestinationKey` can never equal — it refuses an empty asset and an
        // empty address — so one comparison covers both "said nothing" and
        // "said something else", and neither arm is unreachable.
        let attested = agreement
            .attestation(EnforcementPoint::VenueAllowlist)
            .map(|attestation| attestation.reference.as_str())
            .unwrap_or_default();
        if attested != expected {
            return Err(Refusal {
                class: Some(class),
                corridor: None,
                reason: RefusalReason::VenueAllowlistNotMirrored { class },
                detail: format!(
                    "{class} requires the venue's own allowlist to be mirrored, and the \
                     {point} attestation references [{attested}] rather than the destination \
                     [{expected}]; have the venue allowlist this destination out of band and \
                     file the attestation against it, rather than against another entry",
                    point = EnforcementPoint::VenueAllowlist
                ),
            });
        }
        Ok(())
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
/// nothing: ADR 0021 leaves this platform with no path for it to unlock, and
/// the [`crate::gate::Approved`] that carries one still carries no way to
/// execute. What carrying it buys is attribution — an admitted assessment
/// names the three identities on whose agreement it was admitted, rather than
/// asserting that three agreed and keeping no record of which.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Agreement {
    attestations: Vec<Attestation>,
}

impl Agreement {
    /// The attestations, in [`EnforcementPoint::ALL`] order.
    pub fn attestations(&self) -> &[Attestation] {
        &self.attestations
    }

    /// The attestation one point gave, so a caller can ask what it agreed to
    /// rather than only that it agreed.
    pub fn attestation(&self, point: EnforcementPoint) -> Option<&Attestation> {
        self.attestations
            .iter()
            .find(|attestation| attestation.point == point)
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

/// §37.4's closing rule as one input to one assessment: who attested, and
/// which identity trades.
///
/// The two halves travel together because either one alone is satisfied by
/// the arrangement the rule forbids. Three attestations under three distinct
/// identities are not a separation of duties if one of the three is the
/// identity that trades; and an attestor that never trades authorises nothing
/// if only two of the three points spoke.
///
/// **Nothing is validated at construction, deliberately.** A
/// [`TransferAuthority`] is deserialised straight off the event log as part of
/// [`crate::journal::GateCommand`], so a constructor that refused would be a
/// check every replayed record walks past — and the replay is the thing that
/// has to catch a record written by something other than the control.
/// [`Self::agreement`] is therefore evaluated by
/// [`crate::gate::TransferGate::assess`] on every assessment, live and
/// replayed alike.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransferAuthority {
    points: EnforcementPoints,
    trading: Identity,
}

impl TransferAuthority {
    /// Pair the attestations gathered so far with the identity that trades.
    pub fn new(points: EnforcementPoints, trading: Identity) -> Self {
        Self { points, trading }
    }

    /// The attestations, exactly as gathered.
    pub fn points(&self) -> &EnforcementPoints {
        &self.points
    }

    /// The identity that trades, which none of the attestors may be.
    pub fn trading(&self) -> &Identity {
        &self.trading
    }

    /// Both halves of §37.4's closing rule, in order.
    ///
    /// Missing-point and shared-identity first, so a refusal names the point
    /// that did not speak rather than the trading identity it happens not to
    /// be; then the disjointness from trading authority, which is a question
    /// only a complete agreement can be asked.
    pub fn agreement(&self) -> std::result::Result<Agreement, Refusal> {
        let agreement = self.points.all_agree()?;
        agreement.disjoint_from_trading_authority(&self.trading)?;
        Ok(agreement)
    }
}

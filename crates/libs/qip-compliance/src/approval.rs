//! Control 4 — human capital approvals.
//!
//! Capital is not granted by code. [`ApprovalChain::grant`] is the only
//! function in this crate that produces an [`ApprovedCapital`], and it needs a
//! [`qip_contracts::Approval`] naming a human who is not the requester, plus a
//! fresh credential for every name on it. Above a configured threshold it
//! needs two such humans, and they must be different people.
//!
//! [`ApprovedCapital`] has no public constructor and deliberately implements
//! `Serialize` but not `Deserialize`. A downstream component written against
//! `ApprovedCapital` therefore cannot be handed one that skipped the chain,
//! and an envelope that arrives over the wire has to be readmitted through
//! [`ApprovalChain::admit`], which re-verifies the signature. The honest limit
//! of this control is stated on [`ApprovedCapital`] itself:
//! `CapitalEnvelope::new` is public in `qip-contracts` and always will be, so
//! the enforcement is that nothing *approved* exists without an approval, not
//! that the envelope type is unconstructible.
//!
//! The freshness window is the fifteen minutes
//! `qip_risk_engine::autonomy::AutonomyController` uses for a level change.
//! Granting capital is at least as consequential as raising autonomy, so it is
//! not held to a looser standard.

use crate::signing::SigningKey;
use qip_contracts::capital::CapitalEnvelope;
use qip_contracts::governance::Approval;
use qip_contracts::signal::StrategyId;
use qip_contracts::venue::VenueId;
use qip_core::error::{Error, Result};
use qip_core::{Decimal, Duration, Timestamp};
use serde::Serialize;

/// How stale a credential may be when it authorises a capital grant.
///
/// The same window `qip_risk_engine::autonomy` uses. A session token from this
/// morning is not evidence that anyone is at the keyboard now.
pub const MAXIMUM_CREDENTIAL_AGE: Duration = Duration::from_mins(15);

/// Evidence that a named human authenticated recently.
///
/// The same shape as `qip_risk_engine::autonomy::OperatorIdentity`, and
/// deliberately a separate type rather than a shared one: this is a library
/// and that is a service, so depending on it would invert the crate layering.
/// The semantics are identical on purpose — two freshness rules that disagree
/// would be a gap somebody eventually walks through.
///
/// Constructing one is the authentication boundary. There is no `Default`, no
/// parse-from-string, and nothing an automated caller can do to produce one.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct OperatorCredential {
    subject: String,
    method: String,
    authenticated_at: Timestamp,
}

impl OperatorCredential {
    /// Build from a verified authentication.
    ///
    /// The caller asserts it has already checked the credential; everything
    /// downstream trusts that, so this should be called in exactly one place —
    /// the API's authentication middleware.
    pub fn verified(
        subject: impl Into<String>,
        method: impl Into<String>,
        authenticated_at: Timestamp,
    ) -> Result<Self> {
        let subject = subject.into();
        let method = method.into();
        if subject.trim().is_empty() {
            return Err(Error::denied("a credential must name a subject"));
        }
        if method.trim().is_empty() {
            return Err(Error::denied(
                "a credential must record how the subject authenticated; the method is what an \
                 incident review needs to know whether to trust it",
            ));
        }
        Ok(Self {
            subject,
            method,
            authenticated_at,
        })
    }

    pub fn subject(&self) -> &str {
        &self.subject
    }

    pub fn method(&self) -> &str {
        &self.method
    }

    pub fn authenticated_at(&self) -> Timestamp {
        self.authenticated_at
    }

    /// Whether the authentication is recent enough to authorise something.
    ///
    /// A credential dated in the future is not fresh either — that is a clock
    /// problem or a forgery, and both should stop the grant.
    pub fn is_fresh(&self, now: Timestamp, maximum_age: Duration) -> bool {
        now >= self.authenticated_at && now.since(self.authenticated_at) <= maximum_age
    }
}

/// What somebody is asking for.
#[derive(Clone, Debug, PartialEq)]
pub struct CapitalRequest {
    pub strategy: StrategyId,
    pub cell: String,
    pub gross_limit: Decimal,
    pub order_limit: Decimal,
    pub loss_limit: Decimal,
    pub venues: Vec<VenueId>,
    pub expires_at: Timestamp,
    /// Who is asking. Never allowed to be the approver.
    pub requested_by: String,
}

impl CapitalRequest {
    /// The string an [`Approval`] must name as its subject.
    ///
    /// An approval that does not name what it approves can be replayed against
    /// a different request, which is how a small grant becomes a large one.
    pub fn subject(&self) -> String {
        format!("capital:{}@{}", self.strategy.as_str(), self.cell)
    }
}

/// Capital that a human granted.
///
/// No public constructor, and no `Deserialize`: the only ways to hold one are
/// [`ApprovalChain::grant`] and [`ApprovalChain::admit`], both of which check
/// the approval and the signature.
///
/// The limit worth stating plainly: `CapitalEnvelope::new` is public in
/// `qip-contracts`, so a caller elsewhere can build an *unapproved* envelope.
/// What it cannot do is turn one into an `ApprovedCapital` — the signature
/// will not verify — so any component that takes `&ApprovedCapital` rather
/// than `&CapitalEnvelope` is structurally protected.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ApprovedCapital {
    envelope: CapitalEnvelope,
    approval: Approval,
    granted_at: Timestamp,
    /// Which key signed the envelope, so a rotation can be reasoned about.
    key_id: String,
}

impl ApprovedCapital {
    /// The envelope, now safe to enforce against.
    pub fn envelope(&self) -> &CapitalEnvelope {
        &self.envelope
    }

    /// Who approved it, and why.
    pub fn approval(&self) -> &Approval {
        &self.approval
    }

    pub fn granted_at(&self) -> Timestamp {
        self.granted_at
    }

    pub fn key_id(&self) -> &str {
        &self.key_id
    }

    /// The humans accountable for this grant, first approver first.
    pub fn approvers(&self) -> Vec<&str> {
        let mut names = vec![self.approval.approver.as_str()];
        if let Some(second) = self.approval.second_approver.as_deref() {
            names.push(second);
        }
        names
    }
}

/// One recorded grant.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct GrantRecord {
    pub at: Timestamp,
    pub subject: String,
    pub gross_limit: Decimal,
    pub approvers: Vec<String>,
    pub requested_by: String,
    pub signature: String,
}

/// The approvals capital must pass through.
#[derive(Debug)]
pub struct ApprovalChain {
    /// Above this gross limit, two different humans must approve.
    dual_threshold: Decimal,
    maximum_credential_age: Duration,
    key: SigningKey,
    grants: Vec<GrantRecord>,
    /// Refused attempts, kept for the same reason `qip-agents` keeps denied
    /// capability accesses: somebody repeatedly trying to grant themselves
    /// capital is a finding even though every attempt failed.
    refusals: Vec<(Timestamp, String, String)>,
}

impl ApprovalChain {
    /// A chain that requires two approvers above `dual_threshold`.
    pub fn new(dual_threshold: Decimal, key: SigningKey) -> Result<Self> {
        if dual_threshold < Decimal::ZERO {
            return Err(Error::invalid(
                "a dual-approval threshold below zero would mean everything needs two approvers \
                 including a grant of nothing; use ZERO to mean 'always two'",
            ));
        }
        Ok(Self {
            dual_threshold,
            maximum_credential_age: MAXIMUM_CREDENTIAL_AGE,
            key,
            grants: Vec::new(),
            refusals: Vec::new(),
        })
    }

    pub fn dual_threshold(&self) -> Decimal {
        self.dual_threshold
    }

    pub fn maximum_credential_age(&self) -> Duration {
        self.maximum_credential_age
    }

    pub fn key_id(&self) -> &str {
        self.key.key_id()
    }

    pub fn grants(&self) -> &[GrantRecord] {
        &self.grants
    }

    /// Attempts that were refused, with the reason.
    pub fn refusals(&self) -> &[(Timestamp, String, String)] {
        &self.refusals
    }

    /// Whether a request would need two approvers.
    pub fn requires_dual_approval(&self, gross_limit: Decimal) -> bool {
        gross_limit > self.dual_threshold
    }

    /// Grant capital. The only path to an [`ApprovedCapital`].
    ///
    /// Every check below removes a way capital could be granted without a
    /// human deciding to grant it, so each refusal names exactly which one
    /// failed rather than reporting a generic denial.
    pub fn grant(
        &mut self,
        request: &CapitalRequest,
        approval: &Approval,
        credentials: &[OperatorCredential],
        now: Timestamp,
    ) -> Result<ApprovedCapital> {
        match self.check(request, approval, credentials, now) {
            Ok(()) => {}
            Err(error) => {
                self.refusals
                    .push((now, request.subject(), error.message().to_string()));
                return Err(error);
            }
        }

        // Built twice on purpose: the signature covers the envelope's own
        // signing payload, which does not exist until the envelope does. The
        // payload excludes the signature field, so the two constructions agree.
        let unsigned = self.envelope(request, approval, now, String::new())?;
        let signature = self.key.sign(&unsigned.signing_payload());
        let envelope = self.envelope(request, approval, now, signature.clone())?;

        let approvers: Vec<String> = std::iter::once(approval.approver.clone())
            .chain(approval.second_approver.clone())
            .collect();
        self.grants.push(GrantRecord {
            at: now,
            subject: request.subject(),
            gross_limit: request.gross_limit,
            approvers,
            requested_by: request.requested_by.clone(),
            signature,
        });

        Ok(ApprovedCapital {
            envelope,
            approval: approval.clone(),
            granted_at: now,
            key_id: self.key.key_id().to_string(),
        })
    }

    /// Readmit an envelope that arrived from elsewhere — a cell restarting, a
    /// grant read back out of storage.
    ///
    /// The signature is the whole check: an envelope nobody signed, or one
    /// whose limits were edited after signing, does not become
    /// [`ApprovedCapital`] here.
    pub fn admit(
        &self,
        envelope: CapitalEnvelope,
        approval: Approval,
        granted_at: Timestamp,
    ) -> Result<ApprovedCapital> {
        self.key.require(
            &format!(
                "the capital envelope for {} at {}",
                envelope.strategy().as_str(),
                envelope.cell()
            ),
            &envelope.signing_payload(),
            envelope.signature(),
        )?;
        if envelope.approver() != approval.approver {
            return Err(Error::denied(format!(
                "the envelope names {} as approver but the approval is from {}",
                envelope.approver(),
                approval.approver
            )));
        }
        Ok(ApprovedCapital {
            envelope,
            approval,
            granted_at,
            key_id: self.key.key_id().to_string(),
        })
    }

    /// Whether an envelope was signed by this chain, without admitting it.
    pub fn verifies(&self, envelope: &CapitalEnvelope) -> bool {
        self.key
            .verifies(&envelope.signing_payload(), envelope.signature())
    }

    /// The checks. Factored out so a refusal is recorded whichever one fails.
    fn check(
        &self,
        request: &CapitalRequest,
        approval: &Approval,
        credentials: &[OperatorCredential],
        now: Timestamp,
    ) -> Result<()> {
        // An approval for a different subject is an approval for a different
        // thing; accepting it is how a pilot grant is replayed at scale size.
        if approval.subject != request.subject() {
            return Err(Error::denied(format!(
                "the approval is for `{}` but the request is for `{}`",
                approval.subject,
                request.subject()
            )));
        }
        // Re-checked rather than trusted: `Approval::new` enforces this, but
        // `Approval`'s fields are public and it deserialises, so a value can
        // reach here without ever having passed the constructor.
        if approval.rationale.trim().len() < 10 {
            return Err(Error::denied(
                "the approval states no reviewable rationale; the audit trail is the point",
            ));
        }
        if approval.approver.trim().is_empty() {
            return Err(Error::denied("the approval names no approver"));
        }
        if approval.approver == request.requested_by {
            return Err(Error::denied(format!(
                "{} cannot approve their own capital request; an approval by the requester is a \
                 decision by one person wearing two hats",
                approval.approver
            )));
        }
        if let Some(second) = approval.second_approver.as_deref() {
            if second == approval.approver {
                return Err(Error::denied(format!(
                    "{second} is named as both approvers; a second approver who is the first \
                     approver is not a second approver"
                )));
            }
            if second == request.requested_by {
                return Err(Error::denied(format!(
                    "{second} requested this capital and cannot be its second approver"
                )));
            }
        } else if self.requires_dual_approval(request.gross_limit) {
            return Err(Error::denied(format!(
                "a gross limit of {} is above the {} dual-approval threshold and needs a second \
                 named approver",
                request.gross_limit, self.dual_threshold
            )));
        }

        for approver in
            std::iter::once(approval.approver.as_str()).chain(approval.second_approver.as_deref())
        {
            let Some(credential) = credentials.iter().find(|c| c.subject() == approver) else {
                return Err(Error::denied(format!(
                    "no authenticated credential was presented for approver {approver}; a name \
                     in a record is not evidence that the person was there"
                )));
            };
            if !credential.is_fresh(now, self.maximum_credential_age) {
                return Err(Error::denied(format!(
                    "the credential for {approver} was issued at {} and is stale at {now}; \
                     re-authenticate to grant capital",
                    credential.authenticated_at()
                )));
            }
        }
        Ok(())
    }

    fn envelope(
        &self,
        request: &CapitalRequest,
        approval: &Approval,
        now: Timestamp,
        signature: String,
    ) -> Result<CapitalEnvelope> {
        CapitalEnvelope::new(
            request.strategy.clone(),
            request.cell.clone(),
            request.gross_limit,
            request.order_limit,
            request.loss_limit,
            request.venues.clone(),
            now,
            request.expires_at,
            approval.approver.clone(),
            signature,
        )
    }
}

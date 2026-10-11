//! The §27 `ActionIntent` (blueprint v12.0, R12-CONTRACT-7082): an action a
//! brain intends to take *outside* the platform through a registered tool or
//! channel — a message, a filing, a request to a counterparty system — with
//! who is acting, what is disclosed, which policies admitted it, and the key
//! that makes a retry harmless.
//!
//! **Not `crate::action::ActionIntent`.** That type is the executable-scope
//! intent (opportunity, instrument, action class) and is a different concept
//! that happens to share the name; it is owned by another lane and left as it
//! is. This one lives in its own module and is not re-exported at the crate
//! root, so a caller has to name `action_intent::ActionIntent` and cannot get
//! the wrong one by import. Unifying the two names is a follow-up decision,
//! not something to do inside a contract file.
//!
//! **Nothing here submits an order.** An intent is a record of a decision to
//! act, admitted only with every policy decision allowing it; it moves no
//! money and names no venue. Execution stays behind the paper-trading
//! boundary (ADR 0003) and is not reachable from this type.

use crate::validated::{Contract, Validated, after, digest, listed, named, names, refuse};
use qip_core::error::Result;
use qip_core::{Timestamp, sha256_hex};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// Who is acting, and on whose authority.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActingIdentity {
    /// The brain or agent acting.
    pub actor: String,
    /// The principal (operator, mandate) the actor acts for.
    pub on_behalf_of: String,
}

/// What must be disclosed with the action. `NoneRequired` is a statement
/// the producer makes, checked by policy, not a default.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum Disclosure {
    NoneRequired,
    Required { statements: Vec<String> },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    Allowed,
    Denied,
}

/// One policy's decision on this intent.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PolicyDecision {
    pub policy: String,
    pub verdict: Verdict,
    pub decided_at: Timestamp,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActionIntentFields {
    pub schema_version: u32,
    pub intent_id: String,
    pub identity: ActingIdentity,
    /// The registered tool or channel the action goes through.
    pub channel: String,
    /// The payload, as the canonical JSON text the channel will receive.
    pub payload: String,
    /// `sha256_hex(payload)`. Recomputed, not trusted.
    pub payload_digest: String,
    /// Evidence, goal and plan ids the intent rests on; at least one.
    pub provenance: Vec<String>,
    pub disclosure: Disclosure,
    /// Every policy consulted; at least one, all `Allowed`.
    pub policy_decisions: Vec<PolicyDecision>,
    pub created_at: Timestamp,
    pub expires_at: Timestamp,
    /// The same key on a retry makes the retry the same action.
    pub idempotency_key: String,
    /// The audit chain: the ids of the records that led here, oldest first;
    /// at least one.
    pub audit_lineage: Vec<String>,
}

/// The §27 action intent. Built only through [`Validated::new`].
pub type ActionIntent = Validated<ActionIntentFields>;

impl ActionIntentFields {
    /// The digest [`ActionIntentFields::payload_digest`] must hold.
    pub fn digest_payload(payload: &str) -> String {
        sha256_hex(payload.as_bytes())
    }
}

impl Contract for ActionIntentFields {
    const NAME: &'static str = "ActionIntent";
    const SCHEMA_VERSION: u32 = 1;

    fn schema_version(&self) -> u32 {
        self.schema_version
    }

    /// Refuses blank names; a payload digest that is malformed or does not
    /// match the payload (so the action sent is the action reviewed); no
    /// provenance or audit lineage; a required disclosure with no statement;
    /// no policy decision, the same policy decided twice, or any `Denied`
    /// verdict — a denied intent is recorded as a denial, never carried as
    /// an intent; and an expiry not after creation.
    fn check(&self) -> Result<()> {
        let c = Self::NAME;
        named(c, "intent_id", &self.intent_id)?;
        named(c, "identity.actor", &self.identity.actor)?;
        named(c, "identity.on_behalf_of", &self.identity.on_behalf_of)?;
        named(c, "channel", &self.channel)?;
        named(c, "payload", &self.payload)?;
        digest(c, "payload_digest", &self.payload_digest)?;
        if Self::digest_payload(&self.payload) != self.payload_digest {
            return refuse(
                c,
                "carries a payload whose digest does not match `payload_digest`; the action sent \
                 must be the action that was reviewed",
            );
        }
        names(c, "provenance", &self.provenance)?;
        if let Disclosure::Required { statements } = &self.disclosure {
            names(c, "disclosure.statements", statements)?;
        }
        listed(c, "policy_decisions", &self.policy_decisions)?;
        let mut policies = BTreeSet::new();
        for d in &self.policy_decisions {
            named(c, "policy_decisions.policy", &d.policy)?;
            if !policies.insert(d.policy.as_str()) {
                return refuse(c, format!("records policy `{}` twice", d.policy));
            }
            if d.verdict == Verdict::Denied {
                return refuse(
                    c,
                    format!(
                        "carries a denial from policy `{}`; record the denial, not an intent to act",
                        d.policy
                    ),
                );
            }
        }
        after(
            c,
            "expires_at",
            self.expires_at,
            "created_at",
            self.created_at,
        )?;
        named(c, "idempotency_key", &self.idempotency_key)?;
        names(c, "audit_lineage", &self.audit_lineage)
    }
}

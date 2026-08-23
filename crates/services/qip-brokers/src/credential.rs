//! What a venue needs before it will talk to us, and how it is handed over.
//!
//! Three types, kept apart on purpose:
//!
//! * [`VenueRequirement`] is a *statement of need* — one thing a production
//!   deployment must supply, what it is for, and which environment variable it
//!   is read from. Adapters publish the list whether or not they are usable, so
//!   "what would it take to run this for real" always has an answer.
//! * [`VenueCredential`] is what an operator *hands the adapter*. Ordinarily it
//!   carries references to secrets rather than their values: a secret in a
//!   struct is a secret in a log, a panic message and a serialised event.
//! * [`Secret`] is the value itself, for the one caller that genuinely needs
//!   it — a transport writing an authentication frame. It redacts in `Debug`
//!   and `Display`, both written by hand, and implements neither
//!   [`Serialize`] nor [`Deserialize`], so a struct that holds one cannot
//!   derive them either. The compiler refuses the leak rather than a reviewer
//!   catching it.
//!
//! Nothing here has a default. A credential that is missing an item produces
//! [`Error::unavailable`] naming that item, because the alternative — falling
//! back to an empty string, a demo account or the last environment that
//! happened to work — is how a platform ends up authenticated as something
//! nobody chose.

use qip_contracts::venue::VenueId;
use qip_core::error::{Error, Result};
use qip_routing::gateway::GatewayCredential;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;

/// What kind of thing a venue is asking for.
///
/// The categories are the ones an operator has to satisfy separately: an
/// endpoint is a config change, a session credential is a secret, protocol
/// parameters come from the venue's onboarding pack, and an entitlement is a
/// permission somebody at the venue has to grant. Collapsing them into one
/// "credentials" bucket hides that three of the four cannot be self-served.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RequirementKind {
    /// Where the session is established, and the trust store that verifies it.
    Endpoint,
    /// The secret that authenticates the session.
    SessionCredential,
    /// A FIX or wire-protocol parameter: comp ids, session schedule, heartbeat
    /// interval, message-level encryption.
    ProtocolParameter,
    /// A permission the venue must have granted the account before it will
    /// accept the traffic.
    Entitlement,
    /// The account or sub-account orders are sent for.
    Account,
}

impl RequirementKind {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Endpoint => "endpoint",
            Self::SessionCredential => "session credential",
            Self::ProtocolParameter => "protocol parameter",
            Self::Entitlement => "entitlement",
            Self::Account => "account",
        }
    }

    /// Whether the deployment can satisfy this on its own.
    ///
    /// An endpoint is a configuration change; an entitlement needs somebody at
    /// the venue to act. The difference is days of lead time, so it is recorded
    /// rather than discovered.
    pub const fn is_self_service(&self) -> bool {
        matches!(self, Self::Endpoint | Self::ProtocolParameter)
    }
}

/// One thing a production deployment has to supply.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VenueRequirement {
    pub kind: RequirementKind,
    /// What it is, in words an operator would recognise.
    pub name: String,
    /// Environment variable it is read from. Never the value.
    pub env_var: String,
    /// What breaks without it.
    pub purpose: String,
}

impl VenueRequirement {
    pub fn new(
        kind: RequirementKind,
        name: impl Into<String>,
        env_var: impl Into<String>,
        purpose: impl Into<String>,
    ) -> Self {
        Self {
            kind,
            name: name.into(),
            env_var: env_var.into(),
            purpose: purpose.into(),
        }
    }

    /// A sentence naming the item, where it comes from, and what it unblocks.
    pub fn describe(&self) -> String {
        format!(
            "{} — {} (from {}), without which {}",
            self.kind.as_str(),
            self.name,
            self.env_var,
            self.purpose
        )
    }

    /// The same requirement in the vocabulary `qip-routing` gateways already
    /// use.
    ///
    /// The two frameworks describe the same deployment. Converting rather than
    /// restating means a venue cannot be missing an endpoint according to one
    /// and complete according to the other.
    pub fn as_gateway_credential(&self) -> GatewayCredential {
        GatewayCredential::new(
            format!("{} ({})", self.name, self.kind.as_str()),
            self.env_var.clone(),
            self.purpose.clone(),
        )
    }
}

/// Everything any real venue adapter needs, named once.
///
/// Returned by every adapter in this crate — the simulators included. A
/// simulator is complete for what it is and incomplete as a venue, and
/// reporting the second is the honest answer to "what would it take to run this
/// for real". Listing it in one place is what keeps the simulated and the
/// unfinished adapters from drifting into disagreeing about what is missing.
pub fn standard_requirements(venue: &VenueId) -> Vec<VenueRequirement> {
    let prefix = venue.as_str().to_ascii_uppercase().replace('-', "_");
    vec![
        VenueRequirement::new(
            RequirementKind::Endpoint,
            "the session endpoint and the TLS trust store that verifies it",
            format!("QIP_{prefix}_ENDPOINT"),
            "there is no transport, and this build ships none",
        ),
        VenueRequirement::new(
            RequirementKind::SessionCredential,
            "a FIX session password or venue API secret",
            format!("QIP_{prefix}_CREDENTIAL"),
            "no session can be authenticated and every instruction is refused",
        ),
        VenueRequirement::new(
            RequirementKind::ProtocolParameter,
            "SenderCompID, TargetCompID, session schedule and heartbeat interval",
            format!("QIP_{prefix}_SESSION"),
            "a logon cannot be composed, so the venue never answers",
        ),
        VenueRequirement::new(
            RequirementKind::Account,
            "the account or sub-account orders are sent for",
            format!("QIP_{prefix}_ACCOUNT"),
            "fills cannot be attributed to a book",
        ),
        VenueRequirement::new(
            RequirementKind::Entitlement,
            "market-data and order-entry entitlements on that account",
            format!("QIP_{prefix}_ENTITLEMENTS"),
            "the venue accepts the logon and refuses the traffic, which is the \
             confusing failure rather than the obvious one",
        ),
        VenueRequirement::new(
            RequirementKind::Entitlement,
            "an explicit operator enablement for this venue",
            format!("QIP_{prefix}_ENABLED"),
            "holding a credential is not the same as having decided to trade",
        ),
    ]
}

/// The subset of `requirements` in any of `kinds`.
///
/// An adapter uses this to say which requirements it actually enforces, which
/// is never the whole list: a simulator cannot check an endpoint it does not
/// dial, and claiming otherwise would be theatre rather than a control.
pub fn requirements_of_kind(
    requirements: &[VenueRequirement],
    kinds: &[RequirementKind],
) -> Vec<VenueRequirement> {
    requirements
        .iter()
        .filter(|requirement| kinds.contains(&requirement.kind))
        .cloned()
        .collect()
}

/// A secret value, which can be handed on but not looked at by accident.
///
/// The three ways a secret escapes are a log line, a serialised event and a
/// panic message, and all three go through the same two traits. So `Debug` and
/// `Display` are written by hand and redact, and neither is derived: a derive
/// here would print the value, and it would do it in the one place nobody
/// re-reads — an error path.
///
/// `Serialize` and `Deserialize` are not implemented *at all*, which is the
/// stronger statement. A struct holding a `Secret` cannot derive them either,
/// so the compiler refuses the snapshot rather than emitting one with a token
/// in it.
///
/// The bytes are zeroed on drop. That is best-effort — a `Clone`, a reallocated
/// `Vec` or a core dump can still hold a copy — but it costs one safe line and
/// shortens the window, and this workspace forbids the `unsafe` a stronger
/// guarantee would need.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret {
    bytes: Vec<u8>,
}

impl Secret {
    pub fn new(value: impl Into<String>) -> Self {
        Self {
            bytes: value.into().into_bytes(),
        }
    }

    /// Hand the value out.
    ///
    /// Named to be conspicuous: a reviewer scanning a diff should see the word
    /// `expose` at every point a secret leaves this type, and every such point
    /// should be a transport writing an authentication frame.
    pub fn expose_str(&self) -> Result<&str> {
        std::str::from_utf8(&self.bytes).map_err(|_| {
            Error::invalid("this secret is not valid UTF-8 and cannot be read as text")
        })
    }

    /// Whether anything was supplied. One bit, and the bit an operator needs:
    /// an environment variable set to the empty string is the failure that
    /// looks exactly like success.
    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Deliberately not `debug_struct`: no field, no length, no prefix. A
        // length is a hint and a prefix is a head start.
        f.write_str("Secret(<redacted>)")
    }
}

impl fmt::Display for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted>")
    }
}

impl Drop for Secret {
    fn drop(&mut self) {
        self.bytes.fill(0);
    }
}

/// What an operator hands an adapter at authentication.
///
/// Normally it holds *references* to secrets — the environment variable each
/// one is read from — and no values at all, because a reference in a log is a
/// configuration note and a value in a log is an incident. Where a transport
/// genuinely needs the value, [`Self::with_secret`] carries it in a [`Secret`],
/// which cannot be printed or serialised.
///
/// `Debug`, `Display` and `Serialize` are all written by hand for that reason.
/// A derived `Debug` would print whatever the struct grows next, and the point
/// of this type is that it is safe to hold in a struct that gets logged.
#[derive(Clone, PartialEq, Eq)]
pub struct VenueCredential {
    venue: String,
    account: String,
    /// Requirement name to the environment variable that satisfies it.
    references: BTreeMap<String, String>,
    /// Requirement name to the value an operator resolved for it. Never
    /// printed, never serialised, and empty in the ordinary case.
    resolved: BTreeMap<String, Secret>,
}

impl fmt::Debug for VenueCredential {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("VenueCredential")
            .field("venue", &self.venue)
            .field("account", &self.account)
            // The names of what is satisfied, never the environment variables'
            // contents and never the resolved values.
            .field("satisfies", &self.references.keys().collect::<Vec<_>>())
            .field(
                "resolved",
                &format_args!("{} secret(s) <redacted>", self.resolved.len()),
            )
            .finish()
    }
}

impl fmt::Display for VenueCredential {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "credential for account {} at {} satisfying {} requirement(s), {} resolved <redacted>",
            self.account,
            self.venue,
            self.references.len(),
            self.resolved.len()
        )
    }
}

/// The serialised shape of a [`VenueCredential`]: references only.
///
/// A separate type rather than a `skip_serializing` attribute, so that adding a
/// field to the credential cannot silently add it to the wire format.
#[derive(Serialize)]
struct CredentialOut<'a> {
    venue: &'a str,
    account: &'a str,
    references: &'a BTreeMap<String, String>,
}

#[derive(Deserialize)]
struct CredentialIn {
    venue: String,
    account: String,
    #[serde(default)]
    references: BTreeMap<String, String>,
}

impl Serialize for VenueCredential {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        CredentialOut {
            venue: &self.venue,
            account: &self.account,
            references: &self.references,
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for VenueCredential {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        let wire = CredentialIn::deserialize(deserializer)?;
        Ok(Self {
            venue: wire.venue,
            account: wire.account,
            references: wire.references,
            // A secret cannot arrive over the wire, because it never left over
            // it. Deserialising a credential yields one that still has to be
            // resolved from the environment.
            resolved: BTreeMap::new(),
        })
    }
}

impl VenueCredential {
    /// Name the venue and account this credential is for.
    ///
    /// Both are refused when blank rather than defaulted. An adapter that
    /// accepted a blank account would send orders somewhere nobody chose.
    pub fn new(venue: impl Into<String>, account: impl Into<String>) -> Result<Self> {
        let venue = venue.into();
        let account = account.into();
        if venue.trim().is_empty() {
            return Err(Error::invalid(
                "a venue credential must name the venue it is for; there is no default venue",
            ));
        }
        if account.trim().is_empty() {
            return Err(Error::invalid(format!(
                "the credential for {venue} names no account, and there is no default account to \
                 fall back to"
            )));
        }
        Ok(Self {
            venue,
            account,
            references: BTreeMap::new(),
            resolved: BTreeMap::new(),
        })
    }

    /// Record that a requirement is satisfied by the named environment
    /// variable. The value is never read here.
    pub fn with_reference(
        mut self,
        requirement: impl Into<String>,
        env_var: impl Into<String>,
    ) -> Self {
        self.references.insert(requirement.into(), env_var.into());
        self
    }

    /// Carry a resolved secret for a requirement, for a transport that has to
    /// put the value on the wire.
    ///
    /// The requirement is still recorded as referenced, so a credential that
    /// carries a value is complete by the same rule as one that carries a
    /// reference and [`Self::require`] does not need to know the difference.
    pub fn with_secret(
        mut self,
        requirement: impl Into<String>,
        env_var: impl Into<String>,
        secret: Secret,
    ) -> Self {
        let requirement = requirement.into();
        self.references.insert(requirement.clone(), env_var.into());
        self.resolved.insert(requirement, secret);
        self
    }

    /// The resolved value for a requirement, if one was supplied.
    pub fn secret(&self, requirement: &str) -> Option<&Secret> {
        self.resolved.get(requirement)
    }

    /// How many requirements have a value rather than only a reference.
    pub fn resolved_count(&self) -> usize {
        self.resolved.len()
    }

    /// Satisfy every requirement in `requirements` from its own `env_var`.
    ///
    /// The shape of a fully provisioned deployment, and the only place in this
    /// crate where a complete credential can come from.
    pub fn satisfying(
        venue: impl Into<String>,
        account: impl Into<String>,
        requirements: &[VenueRequirement],
    ) -> Result<Self> {
        let mut credential = Self::new(venue, account)?;
        for requirement in requirements {
            credential = credential.with_reference(&requirement.name, &requirement.env_var);
        }
        Ok(credential)
    }

    pub fn venue(&self) -> &str {
        &self.venue
    }

    pub fn account(&self) -> &str {
        &self.account
    }

    /// Where a requirement's secret is read from, if this credential carries it.
    pub fn reference(&self, requirement: &str) -> Option<&str> {
        self.references.get(requirement).map(String::as_str)
    }

    pub fn is_empty(&self) -> bool {
        self.references.is_empty()
    }

    /// The requirements this credential does not satisfy.
    pub fn missing(&self, requirements: &[VenueRequirement]) -> Vec<VenueRequirement> {
        requirements
            .iter()
            .filter(|requirement| !self.references.contains_key(&requirement.name))
            .cloned()
            .collect()
    }

    /// Check the credential covers every requirement, naming what it does not.
    pub fn require(&self, venue: &VenueId, requirements: &[VenueRequirement]) -> Result<()> {
        if self.venue != venue.as_str() {
            return Err(Error::denied(format!(
                "this credential is for {}, not {}; a credential is not transferable between \
                 venues",
                self.venue,
                venue.as_str()
            )));
        }
        let missing = self.missing(requirements);
        if missing.is_empty() {
            return Ok(());
        }
        Err(Error::unavailable(describe_missing(venue, &missing)))
    }
}

/// One sentence an operator can act on, naming every missing item.
pub fn describe_missing(venue: &VenueId, missing: &[VenueRequirement]) -> String {
    if missing.is_empty() {
        return format!("{} needs nothing further", venue.as_str());
    }
    let parts: Vec<String> = missing.iter().map(VenueRequirement::describe).collect();
    format!(
        "{} cannot be used: {} item(s) are missing — {}. Nothing is defaulted; supply them or \
         route to a simulated venue.",
        venue.as_str(),
        missing.len(),
        parts.join("; and ")
    )
}

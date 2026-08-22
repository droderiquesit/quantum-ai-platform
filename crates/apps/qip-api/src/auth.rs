//! Authentication and authorisation for the API.
//!
//! Two things are separated here that are often conflated: *who is calling*
//! and *what they may do*. A read-only monitoring token and an operator token
//! are both authenticated; only one of them may change the autonomy level.
//!
//! Token comparison is constant-time. A comparison that returns early on the
//! first differing byte leaks the token one byte at a time to anyone who can
//! measure response latency, and the whole authentication boundary rests on
//! this one function.

use qip_core::error::{Error, Result};
use qip_core::hash::{constant_time_eq, sha256};
use qip_core::time::Timestamp;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// What a caller is permitted to do.
///
/// Ordered from least to most authority, and every level implies the ones
/// below it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// Health checks and metrics. No portfolio data.
    Monitor,
    /// Read the platform's state: positions, theses, decisions.
    Viewer,
    /// Read, and run research operations that change nothing.
    Analyst,
    /// Approve or veto a proposal.
    Approver,
    /// Change the autonomy level, clear the kill switch, halt the platform.
    Operator,
}

impl Role {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Monitor => "monitor",
            Self::Viewer => "viewer",
            Self::Analyst => "analyst",
            Self::Approver => "approver",
            Self::Operator => "operator",
        }
    }

    pub fn parse(s: &str) -> Result<Self> {
        match s {
            "monitor" => Ok(Self::Monitor),
            "viewer" => Ok(Self::Viewer),
            "analyst" => Ok(Self::Analyst),
            "approver" => Ok(Self::Approver),
            "operator" => Ok(Self::Operator),
            other => Err(Error::invalid(format!("unknown role: {other}"))),
        }
    }

    /// Whether this role includes the authority of another.
    pub fn includes(&self, other: Role) -> bool {
        *self >= other
    }
}

/// An authenticated caller.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Principal {
    /// Who they are, from the credential.
    pub subject: String,
    pub role: Role,
    /// When the credential was issued.
    pub issued_at: Timestamp,
}

impl Principal {
    /// Refuse unless the caller holds at least `required`.
    ///
    /// The error deliberately does not say which role the caller holds: an
    /// unauthorised caller learning the shape of the role hierarchy is a small
    /// leak, and there is no reason to give it away.
    pub fn require(&self, required: Role) -> Result<()> {
        if self.role.includes(required) {
            return Ok(());
        }
        Err(Error::denied(format!(
            "this operation requires the {} role",
            required.as_str()
        )))
    }
}

/// A credential the API will accept.
///
/// Only the hash is stored. A configuration file or a memory dump containing
/// this struct does not contain a usable token.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Credential {
    pub subject: String,
    pub role: Role,
    /// SHA-256 of the token, hex-encoded.
    pub token_hash: String,
    pub issued_at: Timestamp,
    /// When the credential stops working. Every credential expires: one that
    /// does not is one nobody will ever get around to rotating.
    pub expires_at: Timestamp,
}

impl Credential {
    /// Build from a token, hashing it immediately.
    ///
    /// The token is taken by value and dropped here, so it does not linger in
    /// the caller's frame by accident.
    pub fn from_token(
        subject: impl Into<String>,
        role: Role,
        token: String,
        issued_at: Timestamp,
        expires_at: Timestamp,
    ) -> Self {
        Self {
            subject: subject.into(),
            role,
            token_hash: qip_core::hash::to_hex(&sha256(token.as_bytes())),
            issued_at,
            expires_at,
        }
    }

    pub fn is_expired(&self, now: Timestamp) -> bool {
        now >= self.expires_at
    }
}

/// Checks credentials.
#[derive(Debug, Default)]
pub struct Authenticator {
    credentials: Vec<Credential>,
    /// Failed attempts by subject, for lockout.
    failures: std::sync::Mutex<BTreeMap<String, u32>>,
    /// Failures before a subject is locked out.
    lockout_threshold: u32,
}

impl Authenticator {
    pub fn new(credentials: Vec<Credential>) -> Self {
        Self {
            credentials,
            failures: std::sync::Mutex::new(BTreeMap::new()),
            lockout_threshold: 10,
        }
    }

    pub fn credential_count(&self) -> usize {
        self.credentials.len()
    }

    /// Authenticate a bearer token.
    ///
    /// Every credential is compared even after a match is found. Returning
    /// early would make the response time depend on how far down the list the
    /// matching credential sits, which is a slower but perfectly usable oracle.
    pub fn authenticate(&self, header: Option<&str>, now: Timestamp) -> Result<Principal> {
        let Some(header) = header else {
            return Err(Error::denied("no credential was presented"));
        };
        let Some(token) = header.strip_prefix("Bearer ") else {
            return Err(Error::denied("the credential must be a bearer token"));
        };
        let presented = qip_core::hash::to_hex(&sha256(token.trim().as_bytes()));

        let mut matched: Option<&Credential> = None;
        for credential in &self.credentials {
            let same = constant_time_eq(credential.token_hash.as_bytes(), presented.as_bytes());
            if same && matched.is_none() {
                matched = Some(credential);
            }
        }

        let Some(credential) = matched else {
            return Err(Error::denied("the credential was not recognised"));
        };
        if credential.is_expired(now) {
            self.record_failure(&credential.subject);
            return Err(Error::denied(format!(
                "the credential for {} expired at {}",
                credential.subject, credential.expires_at
            )));
        }
        if self.is_locked_out(&credential.subject) {
            return Err(Error::denied(format!(
                "{} is locked out after repeated failures",
                credential.subject
            )));
        }

        self.clear_failures(&credential.subject);
        Ok(Principal {
            subject: credential.subject.clone(),
            role: credential.role,
            issued_at: credential.issued_at,
        })
    }

    pub fn record_failure(&self, subject: &str) {
        if let Ok(mut failures) = self.failures.lock() {
            *failures.entry(subject.to_string()).or_insert(0) += 1;
        }
    }

    pub fn is_locked_out(&self, subject: &str) -> bool {
        self.failures
            .lock()
            .map(|failures| failures.get(subject).copied().unwrap_or(0) >= self.lockout_threshold)
            .unwrap_or(false)
    }

    fn clear_failures(&self, subject: &str) {
        if let Ok(mut failures) = self.failures.lock() {
            failures.remove(subject);
        }
    }
}

/// A simple fixed-window rate limiter.
///
/// Per-subject rather than per-address: an authenticated caller behind a proxy
/// shares an address with everyone else behind it, and limiting on the address
/// would let one caller lock out the rest.
#[derive(Debug)]
pub struct RateLimiter {
    limits: std::sync::Mutex<BTreeMap<String, (Timestamp, u32)>>,
    window: qip_core::Duration,
    maximum: u32,
}

impl RateLimiter {
    pub fn new(window: qip_core::Duration, maximum: u32) -> Self {
        Self {
            limits: std::sync::Mutex::new(BTreeMap::new()),
            window,
            maximum,
        }
    }

    /// Record a request and say whether it is permitted.
    pub fn permit(&self, subject: &str, now: Timestamp) -> bool {
        let Ok(mut limits) = self.limits.lock() else {
            // A poisoned lock means another thread panicked while holding it.
            // Refusing is the safe direction: a rate limiter that fails open
            // is not a rate limiter.
            return false;
        };
        let entry = limits.entry(subject.to_string()).or_insert((now, 0));
        if now.since(entry.0) >= self.window {
            *entry = (now, 0);
        }
        if entry.1 >= self.maximum {
            return false;
        }
        entry.1 += 1;
        true
    }
}

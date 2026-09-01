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

/// How long an unrecognised-token budget lasts before it starts again.
///
/// Fixed windows, like [`RateLimiter`]'s, rather than a rolling average: a
/// rolling window needs per-attempt timestamps, and per-attempt state keyed by
/// anything an anonymous caller controls is the unbounded-memory defect this
/// counter exists to avoid.
const UNRECOGNISED_WINDOW: qip_core::Duration = qip_core::Duration::from_mins(1);

/// Unattributable authentication failures inside one fixed window.
///
/// Two scalars, and deliberately not a map. See [`Authenticator`] for why.
#[derive(Clone, Copy, Debug)]
struct UnrecognisedAttempts {
    window_start: Timestamp,
    count: u32,
}

impl UnrecognisedAttempts {
    /// The count as of `now`, without recording anything.
    fn count_at(&self, now: Timestamp, window: qip_core::Duration) -> u32 {
        if now.since(self.window_start) >= window {
            0
        } else {
            self.count
        }
    }

    /// Record one attempt and return the resulting count.
    ///
    /// The count saturates at `ceiling` rather than at [`u32::MAX`]: the only
    /// question asked of it is whether the budget is spent, so counting past
    /// that point stores an attacker-controlled number for no reason. The
    /// overflow behaviour is therefore stated rather than incidental — the
    /// millionth guess in a window leaves exactly the same two scalars behind
    /// as the tenth.
    fn record(&mut self, now: Timestamp, window: qip_core::Duration, ceiling: u32) -> u32 {
        if now.since(self.window_start) >= window {
            self.window_start = now;
            self.count = 0;
        }
        self.count = self.count.saturating_add(1).min(ceiling);
        self.count
    }
}

/// Checks credentials.
///
/// # Why the brute-force budget is not keyed on a subject
///
/// This type used to hold a per-subject failure map, and the lockout it fed
/// could not fire. A failure is only attributable once a token has matched a
/// credential, so the sole path that recorded one was the expired-credential
/// branch — and expiry is monotone in time, so a subject whose credential has
/// expired never again reaches the lockout check on a later call. The counter
/// was incremented on a path that could never read it. Random-token guessing,
/// the attack a lockout is for, touched nothing at all; the threat model says
/// so in as many words. A control that reads as protection and cannot fire is
/// worse than no control, because the reader stops looking.
///
/// The identifier the guessing attempt does not have is a subject, and it must
/// not be given a synthetic one derived from the presented token: that keys a
/// map on attacker-chosen bytes and lets anyone allocate an entry per guess,
/// trading a missing control for an unbounded-memory one. Nor is a caller
/// address available — [`Authenticator::authenticate`] is handed a header and a
/// clock and nothing else, and the transport in front of it is a proxy this
/// repository does not configure, so an address here would be the proxy's.
///
/// What remains is an attempt counter that is not per-anything: a single
/// fixed-window budget for failures nobody can attribute. It is bounded by
/// construction — two scalars, whatever the traffic — and it fires.
///
/// # What the budget does and does not buy
///
/// Every presented token is compared against every credential, spent budget
/// or not; the budget is consulted only after the comparison finds no match.
/// What a spent budget changes is the refusal — its message names the flood
/// and the wait, and the state it leaves behind is one an operator can read —
/// not whether the comparison ran. That ordering is deliberate rather than an
/// oversight. Refusing before comparing would be cheaper under a flood, but
/// it would also refuse a valid token once the budget was spent, which is a
/// lockout any anonymous caller could trip with ten wrong guesses — and the
/// callers it would lock out are the operators holding the halt and
/// kill-switch routes. Trading a guessing risk for a control-plane outage is
/// not a trade this platform makes, so the cost of the comparison is paid on
/// every attempt and the budget never stands between an operator and a halt.
///
/// The consequence, stated plainly rather than left for a reader to discover:
/// because matching happens before the budget is consulted, a *correct* guess
/// made inside a window that is not yet spent still authenticates. The budget
/// bounds the work an unattributable caller can extract and turns an invisible
/// stream of refusals into a state the process holds and can be asked about
/// ([`Authenticator::unrecognised_attempts`]). Token entropy and rotation are
/// what make the guess itself hopeless; this counter does not pretend to.
#[derive(Debug)]
pub struct Authenticator {
    credentials: Vec<Credential>,
    /// Failures nobody can attribute, for the brute-force budget.
    unrecognised: std::sync::Mutex<UnrecognisedAttempts>,
    /// Unattributable failures admitted per window before the budget is spent.
    lockout_threshold: u32,
    window: qip_core::Duration,
}

impl Default for Authenticator {
    fn default() -> Self {
        Self::new(Vec::new())
    }
}

impl Authenticator {
    pub fn new(credentials: Vec<Credential>) -> Self {
        Self {
            credentials,
            unrecognised: std::sync::Mutex::new(UnrecognisedAttempts {
                window_start: Timestamp::EPOCH,
                count: 0,
            }),
            lockout_threshold: 10,
            window: UNRECOGNISED_WINDOW,
        }
    }

    pub fn credential_count(&self) -> usize {
        self.credentials.len()
    }

    /// Unattributable failures admitted per window before refusal.
    pub fn lockout_threshold(&self) -> u32 {
        self.lockout_threshold
    }

    /// Unattributable failures recorded in the window containing `now`.
    ///
    /// A poisoned lock reports a spent budget rather than an empty one. The
    /// reader is an operator asking whether the platform is under a guessing
    /// flood, and the safe direction for that answer is the alarming one.
    pub fn unrecognised_attempts(&self, now: Timestamp) -> u32 {
        self.unrecognised
            .lock()
            .map(|attempts| attempts.count_at(now, self.window))
            .unwrap_or(self.lockout_threshold)
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
            // The two refusal paths are counted differently on purpose. This
            // one is unattributable and is the guessing channel, so it spends
            // the budget. The expired one below does not: it is attributable,
            // its holder has already lost access, and a stale poller repeating
            // it every second would otherwise keep the budget permanently
            // spent — a control an honest client can exhaust is a control an
            // attacker can disarm.
            if self.spend_unrecognised_budget(now) {
                return Err(Error::denied(
                    "too many unrecognised credentials have been presented; wait for the current \
                     minute to elapse and present a credential this deployment was configured with",
                ));
            }
            return Err(Error::denied("the credential was not recognised"));
        };
        if credential.is_expired(now) {
            // This message names the subject and the expiry where the one
            // above says nothing, and the asymmetry is deliberate rather than
            // an oversight. Reaching it requires presenting a token that
            // matched a stored hash, which is possession of the secret itself;
            // the holder learns only which of their own credentials it is and
            // that rotation is what fixes it. A caller who cannot produce a
            // matching token can never tell the two refusals apart, so no
            // oracle for "this token exists" is offered to anyone who does not
            // already hold one.
            return Err(Error::denied(format!(
                "the credential for {} expired at {}; rotate it and restart with the new one",
                credential.subject, credential.expires_at
            )));
        }

        Ok(Principal {
            subject: credential.subject.clone(),
            role: credential.role,
            issued_at: credential.issued_at,
        })
    }

    /// Charge one unattributable failure, returning whether the budget is now
    /// spent.
    ///
    /// A poisoned lock spends the budget: the counter cannot be trusted, and
    /// refusing unrecognised tokens is the direction that costs a legitimate
    /// caller nothing, because a legitimate caller's token matched.
    fn spend_unrecognised_budget(&self, now: Timestamp) -> bool {
        let Ok(mut attempts) = self.unrecognised.lock() else {
            return true;
        };
        attempts.record(now, self.window, self.lockout_threshold) >= self.lockout_threshold
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

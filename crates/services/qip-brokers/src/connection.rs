//! The connection lifecycle, and the proof of readiness an order needs.
//!
//! ```text
//! disconnected -> connecting -> authenticated -> ready
//!                                  |               |
//!                                  +-> degraded <--+
//!                                        |
//!                     everything -> disconnected
//! ```
//!
//! The refusals are the point. There is no move from `disconnected` to `ready`,
//! so a session cannot skip authentication; there is no move from `connecting`
//! to `ready`, so a socket that opened is not a session that logged on.
//!
//! Readiness is *structural* rather than advisory.
//! [`crate::adapter::VenueAdapter::submit_order`] takes a [`ReadyTicket`], and
//! the only way to obtain one is [`ConnectionState::ready_ticket`], which
//! refuses unless the venue is ready *at the timestamp given*. A caller
//! therefore cannot write the code that submits from a degraded venue — not
//! because a check would catch it, but because there is no ticket to pass.
//!
//! Every ticket names a session number that increments on each transition, so a
//! ticket minted before a heartbeat gap is refused after it: proof of readiness
//! is proof about *a moment*, and a venue that fell over in between is not the
//! venue the ticket was issued for.
//!
//! Cancels deliberately do not need a ticket. Cancelling reduces risk, and the
//! same reasoning that lets the kill switch trip without authority applies
//! here: the urgent direction must always be the fast one.

use qip_contracts::venue::VenueId;
use qip_core::error::{Error, Result};
use qip_core::time::{Duration, Timestamp};
use serde::{Deserialize, Serialize};

/// Where a venue connection is in its life.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionPhase {
    /// No socket, no session.
    Disconnected,
    /// Transport established, nothing authenticated.
    Connecting,
    /// Logged on. The venue knows who we are and has not yet confirmed the
    /// session is healthy.
    Authenticated,
    /// Logged on, heartbeating, and accepting orders.
    Ready,
    /// The session exists but is not trustworthy — a heartbeat gap, a venue
    /// halt, a sequence break. Orders stop; cancels do not.
    Degraded,
}

impl ConnectionPhase {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Disconnected => "disconnected",
            Self::Connecting => "connecting",
            Self::Authenticated => "authenticated",
            Self::Ready => "ready",
            Self::Degraded => "degraded",
        }
    }

    /// Whether a session exists at all, healthy or not.
    pub const fn has_session(&self) -> bool {
        matches!(self, Self::Authenticated | Self::Ready | Self::Degraded)
    }

    /// Whether new risk may be sent. Only one phase qualifies.
    pub const fn accepts_orders(&self) -> bool {
        matches!(self, Self::Ready)
    }
}

/// Proof that a venue was ready at a particular moment.
///
/// Constructible only through [`ConnectionState::ready_ticket`]. The fields are
/// private and there is no other constructor, so holding one of these is
/// evidence rather than assertion.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReadyTicket {
    venue: String,
    session: u64,
    issued_at: Timestamp,
}

impl ReadyTicket {
    pub fn venue(&self) -> &str {
        &self.venue
    }

    pub fn session(&self) -> u64 {
        self.session
    }

    pub fn issued_at(&self) -> Timestamp {
        self.issued_at
    }
}

/// A venue session and everything known about its health.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ConnectionState {
    venue: VenueId,
    phase: ConnectionPhase,
    /// Increments on every transition, so a ticket cannot outlive the moment it
    /// was issued for.
    session: u64,
    since: Timestamp,
    last_heartbeat: Option<Timestamp>,
    /// How long a gap the venue tolerates before the session is untrustworthy.
    heartbeat_interval: Duration,
    detail: String,
    /// Every phase this session has been in, for reconstructing an incident.
    transitions: Vec<(Timestamp, String)>,
}

impl ConnectionState {
    /// A fresh, disconnected session.
    pub fn new(venue: VenueId, heartbeat_interval: Duration, at: Timestamp) -> Self {
        Self {
            venue,
            phase: ConnectionPhase::Disconnected,
            session: 0,
            since: at,
            last_heartbeat: None,
            heartbeat_interval,
            detail: "never connected".to_string(),
            transitions: vec![(at, "disconnected".to_string())],
        }
    }

    pub fn venue(&self) -> &VenueId {
        &self.venue
    }

    /// The recorded phase, which may be stale if nothing has observed the clock.
    /// Use [`Self::effective_phase`] when a timestamp is available.
    pub const fn phase(&self) -> ConnectionPhase {
        self.phase
    }

    pub const fn session(&self) -> u64 {
        self.session
    }

    pub const fn since(&self) -> Timestamp {
        self.since
    }

    pub const fn last_heartbeat(&self) -> Option<Timestamp> {
        self.last_heartbeat
    }

    pub const fn heartbeat_interval(&self) -> Duration {
        self.heartbeat_interval
    }

    pub fn detail(&self) -> &str {
        &self.detail
    }

    pub fn transitions(&self) -> &[(Timestamp, String)] {
        &self.transitions
    }

    /// How long since the venue last answered a heartbeat.
    pub fn heartbeat_gap(&self, at: Timestamp) -> Option<Duration> {
        self.last_heartbeat.map(|last| at.since(last))
    }

    /// Whether the venue has gone quiet for longer than it is allowed to.
    ///
    /// A session that has never heartbeated is not overdue — it has not been
    /// asked yet. It is also not ready, which is the phase's job to say.
    pub fn heartbeat_overdue(&self, at: Timestamp) -> bool {
        match self.last_heartbeat {
            Some(last) => at.since(last) > self.heartbeat_interval,
            None => false,
        }
    }

    /// The phase as of `at`, accounting for a heartbeat that stopped arriving.
    ///
    /// Pure: reporting a stale `ready` because nobody happened to call a
    /// mutating method would be the exact failure this crate exists to avoid.
    pub fn effective_phase(&self, at: Timestamp) -> ConnectionPhase {
        if self.phase == ConnectionPhase::Ready && self.heartbeat_overdue(at) {
            return ConnectionPhase::Degraded;
        }
        self.phase
    }

    /// Commit what [`Self::effective_phase`] already sees.
    ///
    /// Called at the start of every mutating adapter operation, so a venue that
    /// went quiet is recorded as degraded even if nobody polled its health.
    pub fn observe(&mut self, at: Timestamp) -> ConnectionPhase {
        if self.effective_phase(at) == ConnectionPhase::Degraded
            && self.phase == ConnectionPhase::Ready
        {
            let gap = self.heartbeat_gap(at).unwrap_or(Duration::ZERO);
            // The transition cannot fail — ready to degraded is legal — but the
            // result is not discarded, because a refusal here would mean the
            // table above changed and the venue is now in a phase nobody
            // intended.
            if let Err(error) = self.degrade(
                at,
                format!(
                    "no heartbeat for {gap:?}, which is longer than the {:?} the session allows",
                    self.heartbeat_interval
                ),
            ) {
                self.detail = error.message().to_string();
            }
        }
        self.phase
    }

    /// Open the transport.
    pub fn connect(&mut self, at: Timestamp) -> Result<()> {
        self.transition(ConnectionPhase::Connecting, at, "transport established")
    }

    /// Record a successful logon.
    pub fn authenticated(&mut self, at: Timestamp, account: &str) -> Result<()> {
        self.transition(
            ConnectionPhase::Authenticated,
            at,
            format!("logged on for account {account}"),
        )
    }

    /// Promote an authenticated session to ready.
    ///
    /// Only reachable from `authenticated` or `degraded`, and only with a
    /// heartbeat on record: a session nobody has heard from is not ready, it is
    /// merely new.
    pub fn make_ready(&mut self, at: Timestamp) -> Result<()> {
        if self.last_heartbeat.is_none() {
            return Err(Error::invalid(format!(
                "{} cannot be ready before its first heartbeat; a session nobody has heard from is \
                 not a session that works",
                self.venue.as_str()
            )));
        }
        self.transition(ConnectionPhase::Ready, at, "heartbeating")
    }

    /// Mark the session untrustworthy, with the reason.
    pub fn degrade(&mut self, at: Timestamp, reason: impl Into<String>) -> Result<()> {
        self.transition(ConnectionPhase::Degraded, at, reason)
    }

    /// Tear the session down. Always legal: the safe direction never needs
    /// permission.
    pub fn disconnect(&mut self, at: Timestamp, reason: impl Into<String>) {
        self.phase = ConnectionPhase::Disconnected;
        self.session = self.session.saturating_add(1);
        self.since = at;
        self.last_heartbeat = None;
        self.detail = reason.into();
        self.transitions.push((at, "disconnected".to_string()));
    }

    /// Record that the venue answered a heartbeat.
    ///
    /// Promotes an authenticated session to ready and recovers a degraded one,
    /// which is what a heartbeat means: the session is answering again.
    pub fn observe_heartbeat(&mut self, at: Timestamp) -> Result<ConnectionPhase> {
        if !self.phase.has_session() {
            return Err(Error::denied(format!(
                "{} has no session to heartbeat; it is {}",
                self.venue.as_str(),
                self.phase.as_str()
            )));
        }
        self.last_heartbeat = Some(at);
        match self.phase {
            ConnectionPhase::Authenticated | ConnectionPhase::Degraded => {
                self.transition(ConnectionPhase::Ready, at, "heartbeating")?;
            }
            _ => {}
        }
        Ok(self.phase)
    }

    /// Mint proof that the venue is ready at `at`.
    ///
    /// The error says which phase refused, because "the venue is not ready" is
    /// not something anyone can act on and "the venue is degraded after a
    /// 12-second heartbeat gap" is.
    pub fn ready_ticket(&self, at: Timestamp) -> Result<ReadyTicket> {
        let phase = self.effective_phase(at);
        if phase != ConnectionPhase::Ready {
            return Err(Error::denied(format!(
                "{} is {} and cannot accept an order: {}",
                self.venue.as_str(),
                phase.as_str(),
                self.refusal_detail(at)
            )));
        }
        Ok(ReadyTicket {
            venue: self.venue.as_str().to_string(),
            session: self.session,
            issued_at: at,
        })
    }

    /// Check a ticket still authorises this session at `at`.
    pub fn authorise(&self, ticket: &ReadyTicket, at: Timestamp) -> Result<()> {
        if ticket.venue != self.venue.as_str() {
            return Err(Error::denied(format!(
                "a readiness ticket for {} does not authorise anything at {}",
                ticket.venue,
                self.venue.as_str()
            )));
        }
        if ticket.session != self.session {
            return Err(Error::denied(format!(
                "the readiness ticket for {} was issued for session {} and this is session {}; the \
                 connection changed phase in between",
                self.venue.as_str(),
                ticket.session,
                self.session
            )));
        }
        let phase = self.effective_phase(at);
        if phase != ConnectionPhase::Ready {
            return Err(Error::denied(format!(
                "{} is {} and cannot accept an order: {}",
                self.venue.as_str(),
                phase.as_str(),
                self.refusal_detail(at)
            )));
        }
        Ok(())
    }

    /// Check a session exists, for instructions that reduce risk.
    pub fn require_session(&self, at: Timestamp) -> Result<()> {
        if self.effective_phase(at).has_session() {
            return Ok(());
        }
        Err(Error::denied(format!(
            "{} is {}: there is no session to send an instruction on",
            self.venue.as_str(),
            self.phase.as_str()
        )))
    }

    /// Why the venue would refuse right now, in a sentence.
    fn refusal_detail(&self, at: Timestamp) -> String {
        if self.phase == ConnectionPhase::Ready && self.heartbeat_overdue(at) {
            let gap = self.heartbeat_gap(at).unwrap_or(Duration::ZERO);
            return format!(
                "the venue has not answered a heartbeat for {gap:?}, longer than the {:?} the \
                 session allows",
                self.heartbeat_interval
            );
        }
        self.detail.clone()
    }

    /// The only way a phase changes.
    ///
    /// Written as one `matches!` so the legal moves read as a table. The
    /// refusals matter more than the permissions: a session that can reach
    /// `ready` without authenticating is one that sends orders into a socket
    /// nobody logged on to.
    fn transition(
        &mut self,
        next: ConnectionPhase,
        at: Timestamp,
        detail: impl Into<String>,
    ) -> Result<()> {
        let legal = matches!(
            (self.phase, next),
            (ConnectionPhase::Disconnected, ConnectionPhase::Connecting)
                | (ConnectionPhase::Connecting, ConnectionPhase::Authenticated)
                | (ConnectionPhase::Authenticated, ConnectionPhase::Ready)
                | (ConnectionPhase::Authenticated, ConnectionPhase::Degraded)
                | (ConnectionPhase::Ready, ConnectionPhase::Degraded)
                | (ConnectionPhase::Degraded, ConnectionPhase::Ready)
        );
        if !legal {
            return Err(Error::invalid(format!(
                "{} cannot move from {} to {}",
                self.venue.as_str(),
                self.phase.as_str(),
                next.as_str()
            )));
        }
        self.phase = next;
        self.session = self.session.saturating_add(1);
        self.since = at;
        self.detail = detail.into();
        self.transitions.push((at, next.as_str().to_string()));
        Ok(())
    }
}

/// What the session looks like from outside.
///
/// Distinct from `qip_routing::VenueHealth`, which measures execution quality
/// over time — reject rates and latency. This measures whether there is a
/// working session at all. A venue can be perfectly connected and filling
/// badly, or quoting beautifully on a session that has stopped answering, and
/// conflating the two loses whichever one is wrong.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SessionHealth {
    pub venue: String,
    pub at: Timestamp,
    pub phase: ConnectionPhase,
    pub session: u64,
    pub since: Timestamp,
    pub last_heartbeat: Option<Timestamp>,
    /// `None` when the venue has never been asked for a heartbeat.
    pub heartbeat_gap: Option<Duration>,
    pub heartbeat_interval: Duration,
    /// Whether an order would be accepted right now.
    pub accepts_orders: bool,
    /// Whether the venue is usable at all, or is a port awaiting credentials.
    pub is_available: bool,
    /// Whether fills from this venue are simulated.
    pub simulated: bool,
    /// What a deployment still has to supply. Empty only for an adapter that is
    /// genuinely complete, which is none of the ones in this crate.
    pub missing: Vec<String>,
    pub detail: String,
}

impl SessionHealth {
    /// Whether the session is up but untrustworthy.
    pub fn is_degraded(&self) -> bool {
        self.phase == ConnectionPhase::Degraded
    }

    /// A sentence for an operator.
    pub fn describe(&self) -> String {
        let gap = match self.heartbeat_gap {
            Some(gap) => format!(", last heartbeat {gap:?} ago"),
            None => ", never heartbeated".to_string(),
        };
        format!(
            "{} is {}{} and {} orders: {}",
            self.venue,
            self.phase.as_str(),
            gap,
            if self.accepts_orders {
                "accepts"
            } else {
                "refuses"
            },
            self.detail
        )
    }
}

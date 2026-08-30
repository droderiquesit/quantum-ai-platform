//! Per-peer circuit breaking: stop spending the retry ladder on a peer that is
//! down.
//!
//! # The failure this closes
//!
//! [`crate::retry`] bounds what *one* message spends on a failing peer: five
//! attempts over roughly six seconds. It says nothing about the next message.
//! When a peer is genuinely down — a rolled pod that will not come back for two
//! minutes, a NetworkPolicy someone tightened, a node that has gone — every
//! message queued behind the first one pays the same six seconds, in full,
//! discovering the same fact. A thousand queued deltas become an hour and a
//! half of connect timeouts against a socket nobody is listening on, and the
//! outbound queue fills while the process is busy waiting.
//!
//! A circuit breaker turns the *second* discovery into a refusal that costs
//! nothing. It is not an optimisation: it is what keeps the failure of one peer
//! from consuming the thread that also serves the other eight.
//!
//! # The state machine, exactly
//!
//! ```text
//!                 failure_threshold consecutive failures
//!      ┌────────┐ ─────────────────────────────────────► ┌──────┐
//!      │ Closed │                                        │ Open │
//!      └────────┘ ◄───────────────────────────────────── └──────┘
//!           ▲      success_threshold probe successes         │
//!           │                                                │ cooldown
//!           │                                                │ elapsed
//!           │            ┌───────────┐                       │
//!           └────────────│ HalfOpen  │◄──────────────────────┘
//!                        └───────────┘
//!                              │  a probe fails
//!                              └────────► Open, with a longer cooldown
//! ```
//!
//! * **Closed** — every call is admitted. A success resets the consecutive
//!   failure count; intermittent failures spread over a healthy stream
//!   therefore never accumulate into an open circuit, which is the difference
//!   between a breaker and a rate limiter.
//! * **Open** — every call is refused *without touching the network*, with the
//!   time left on the cooldown attached to the refusal so the caller can decide
//!   whether to queue, shed or dead-letter rather than guess.
//! * **HalfOpen** — reached when the cooldown elapses. At most
//!   [`BreakerPolicy::concurrent_probes`] calls are admitted, and the rest are
//!   refused. A probe that fails reopens the circuit with a *longer* cooldown;
//!   a probe that succeeds counts towards
//!   [`BreakerPolicy::success_threshold`], and reaching it closes the circuit.
//!
//! The transition that is easiest to get wrong, and is therefore stated on its
//! own: **a half-open probe that fails reopens the circuit; it does not close
//! it.** A breaker that closed on any half-open outcome would open, wait, admit
//! one call, close, admit a thousand, and open again — a slow oscillation that
//! is worse than no breaker at all because it hides the outage behind a
//! constant trickle of successes.
//!
//! # Per peer, never global
//!
//! Nine regional cells publish to one central plane and it publishes back to
//! all nine. A global breaker would let the loss of one cell refuse traffic to
//! the other eight, which is the failure the cellular architecture in ADR 0008
//! exists to prevent. Circuits are keyed by peer and share nothing but the
//! policy.
//!
//! The registry is bounded, because a map keyed by a string a peer supplies is
//! an unbounded allocation. See [`CircuitBreaker::new`] for what happens at the
//! bound, and for why that case is counted rather than hidden.
//!
//! # No ambient anything, here either
//!
//! Every instant comes from the injected [`Clock`]: the cooldown expires
//! because the clock says so, not because a thread slept. Every jitter draw
//! comes from a seeded [`Xoshiro256`] held by the breaker. A test therefore
//! asserts "the circuit was still open one nanosecond before the cooldown
//! ended" as a fact rather than as a race.
//!
//! Jitter subtracts, for [`crate::retry`]'s reason: nine cells whose circuits
//! opened on the same rollout must not all probe on the same millisecond, and a
//! [`BreakerPolicy::max_cooldown`] that jitter could exceed would be a number
//! quoted in a runbook and then wrong.
//!
//! # What this does not do
//!
//! * **It does not retry.** A refusal is returned to the caller, which decides.
//!   Combining the two here would put a retry ladder behind a breaker whose
//!   whole purpose is to stop the ladder from running.
//! * **It does not measure a failure rate.** The trigger is *consecutive*
//!   failures, not a percentage over a window. A percentage needs a window, a
//!   window needs a minimum sample size to avoid tripping on the first two
//!   calls after a quiet hour, and both are knobs that get tuned once and then
//!   misunderstood. Consecutive failures answer the question this breaker is
//!   for — "is the peer down right now" — and answer it the same way every time.
//! * **It is not shared between processes.** Each publisher has its own view of
//!   each peer. Ten pods discovering the same outage will each spend one ladder
//!   discovering it. Sharing breaker state across pods would need a store and a
//!   consensus about staleness, and would make one pod's bad network everyone's
//!   refusal.
//! * **It does not distinguish "slow" from "failing".** A peer answering
//!   successfully at the edge of the read timeout keeps the circuit closed. A
//!   latency-based breaker is a different instrument and needs a latency budget
//!   that this transport does not have.

use std::collections::BTreeMap;

use qip_core::rng::{Rng, Xoshiro256};
use qip_core::{Clock, Duration, Error, Result, Timestamp};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Where one peer's circuit is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BreakerState {
    /// Calls pass through.
    Closed,
    /// Calls are refused without touching the network.
    Open,
    /// The cooldown has elapsed and a bounded number of probes are admitted.
    HalfOpen,
}

impl BreakerState {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Closed => "closed",
            Self::Open => "open",
            Self::HalfOpen => "half-open",
        }
    }

    /// Whether the peer is believed usable. `HalfOpen` is deliberately *not*
    /// healthy: it means one call is being spent to find out.
    pub const fn is_healthy(&self) -> bool {
        matches!(self, Self::Closed)
    }
}

impl std::fmt::Display for BreakerState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// When to open, how long to wait, and how many probes to admit.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BreakerPolicy {
    /// Consecutive failures that open a closed circuit. `1` opens on the first.
    pub failure_threshold: u32,
    /// Probe successes needed to close a half-open circuit.
    pub success_threshold: u32,
    /// How long the circuit stays open the first time it opens.
    pub cooldown: Duration,
    /// The ceiling the cooldown never exceeds, jitter included.
    pub max_cooldown: Duration,
    /// What each successive reopen multiplies the cooldown by, so a peer that
    /// stays down is probed less and less often rather than every cooldown
    /// forever. Reset to the base cooldown when the circuit closes.
    pub cooldown_multiplier: u32,
    /// How far below the computed cooldown the jitter may pull, in basis
    /// points. Integer arithmetic, for [`crate::retry`]'s reason: a float here
    /// makes the schedule depend on rounding.
    pub jitter_basis_points: u32,
    /// How many calls a half-open circuit admits at once. One is the
    /// conservative answer and the default; more than one lets a peer that is
    /// up be confirmed faster at the cost of more calls into a peer that is not.
    pub concurrent_probes: u32,
}

impl Default for BreakerPolicy {
    /// Open after three consecutive failures, wait two seconds, close after one
    /// good probe.
    ///
    /// Tuned against [`crate::retry`]'s default ladder, and the relationship
    /// between the two is the point: one message already spends five attempts
    /// over roughly six seconds before it dead-letters, so three *messages*
    /// failing in a row is about twenty seconds of evidence — enough that the
    /// peer is not merely restarting. The two-second cooldown is shorter than a
    /// pod's start-up so a peer that comes back is found quickly, and the
    /// multiplier is what stops that from becoming a probe every two seconds
    /// for an outage measured in hours.
    fn default() -> Self {
        Self {
            failure_threshold: 3,
            success_threshold: 1,
            cooldown: Duration::from_secs(2),
            max_cooldown: Duration::from_secs(60),
            cooldown_multiplier: 4,
            jitter_basis_points: 2_500,
            concurrent_probes: 1,
        }
    }
}

impl BreakerPolicy {
    /// Refuse a policy that cannot do what it says.
    ///
    /// Checked at construction, because every one of these mistakes presents at
    /// run time as "the transport stopped sending" during the incident the
    /// breaker was added for.
    pub fn validate(&self) -> Result<()> {
        if self.failure_threshold == 0 {
            return Err(Error::invalid(
                "a failure threshold of zero opens the circuit before any call has failed, so the \
                 peer is never tried at all",
            ));
        }
        if self.success_threshold == 0 {
            return Err(Error::invalid(
                "a success threshold of zero closes the circuit without a probe succeeding",
            ));
        }
        if self.concurrent_probes == 0 {
            return Err(Error::invalid(
                "a half-open circuit that admits no probe never closes, so the breaker is a \
                 permanent refusal rather than a breaker",
            ));
        }
        if self.cooldown_multiplier == 0 {
            return Err(Error::invalid(
                "a cooldown multiplier of zero collapses every reopen to no wait at all",
            ));
        }
        if self.cooldown.as_nanos() <= 0 {
            return Err(Error::invalid(
                "a cooldown of zero admits a probe immediately, which is the hammering this exists \
                 to stop",
            ));
        }
        if self.cooldown > self.max_cooldown {
            return Err(Error::invalid(
                "the initial cooldown is longer than the maximum, so the maximum is not a maximum",
            ));
        }
        if self.jitter_basis_points > 10_000 {
            return Err(Error::invalid(
                "jitter above 10000 basis points would pull a cooldown below zero",
            ));
        }
        Ok(())
    }
}

/// Permission to make one call.
///
/// Returned by [`CircuitBreaker::admit`] and handed back to
/// [`CircuitBreaker::record`] with what happened. It exists so an outcome
/// cannot be reported for a call that was never admitted: a half-open circuit
/// counts probes in flight, and a stray success from some other code path would
/// close a circuit that no probe ever tested.
///
/// `#[must_use]` because dropping one leaks a probe slot — a half-open circuit
/// whose only probe was admitted and never reported stays half-open until the
/// call that reports it arrives.
#[derive(Clone, Debug, PartialEq, Eq)]
#[must_use = "a permit must be handed back to CircuitBreaker::record with the outcome, or a \
              half-open circuit keeps a probe slot that nothing will ever release"]
pub struct Permit {
    peer: String,
    probe: bool,
    tracked: bool,
}

impl Permit {
    /// Which peer this permits a call to.
    pub fn peer(&self) -> &str {
        &self.peer
    }

    /// Whether this is the half-open probe rather than an ordinary call. A
    /// caller may want to send its cheapest message as the probe.
    pub const fn is_probe(&self) -> bool {
        self.probe
    }

    /// Whether the breaker is keeping a circuit for this peer at all. `false`
    /// means the registry was at its bound — see [`CircuitBreaker::new`].
    pub const fn is_tracked(&self) -> bool {
        self.tracked
    }
}

/// A call the breaker declined to make.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Refusal {
    pub peer: String,
    pub state: BreakerState,
    /// Consecutive failures at the moment the circuit opened.
    pub consecutive_failures: u32,
    /// How long until the circuit next admits a probe. Zero when the refusal is
    /// a half-open circuit whose probe slots are all in flight — that clears
    /// when the outstanding probe reports, not when a timer expires.
    pub retry_after: Duration,
    /// The last failure this circuit saw, so the refusal explains itself
    /// without a second lookup.
    pub last_error: String,
}

impl Refusal {
    /// A stable machine-readable code, for metrics and for tests that assert
    /// the failure mode rather than matching on prose.
    pub const fn code(&self) -> &'static str {
        "circuit_open"
    }
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "the circuit to {} is {} after {} consecutive failures and admits nothing for another \
             {:?}: {}",
            self.peer, self.state, self.consecutive_failures, self.retry_after, self.last_error
        )
    }
}

impl std::error::Error for Refusal {}

/// A refusal is a [`qip_core::Error::Guard`]: nothing failed, something was
/// declined. The same mapping [`crate::TransportError::QueueFull`] takes, and
/// for the same reason — the socket was never touched.
impl From<Refusal> for Error {
    fn from(refusal: Refusal) -> Self {
        Self::guard(refusal.to_string())
    }
}

/// Whether the breaker will let a call through.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Decision {
    /// Make the call, then report the outcome with this permit.
    Admitted(Permit),
    /// Do not make the call.
    Refused(Refusal),
}

impl Decision {
    pub const fn is_admitted(&self) -> bool {
        matches!(self, Self::Admitted(_))
    }

    /// The permit, or `None` when the call was refused.
    pub fn permit(self) -> Option<Permit> {
        match self {
            Self::Admitted(permit) => Some(permit),
            Self::Refused(_) => None,
        }
    }

    /// The refusal, or `None` when the call was admitted.
    pub fn refusal(&self) -> Option<&Refusal> {
        match self {
            Self::Admitted(_) => None,
            Self::Refused(refusal) => Some(refusal),
        }
    }
}

/// What a call did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The peer answered and the answer was usable.
    Success,
    /// The peer did not answer, or answered in a way that says it is unwell.
    ///
    /// The judgement of which failures count is the caller's and it matters: a
    /// connect timeout is evidence about the *peer*, and a 422 saying the frame
    /// was malformed is evidence about the *message*. Feeding the second into a
    /// breaker opens a circuit to a healthy peer because one publisher is
    /// sending rubbish. [`crate::http::HttpError::is_transient`] is the
    /// distinction already drawn for retries, and it is the right one here.
    Failure(String),
}

impl Outcome {
    /// A failure whose detail is taken from any error that can render itself.
    pub fn failed(error: &impl std::fmt::Display) -> Self {
        Self::Failure(error.to_string())
    }

    pub const fn is_success(&self) -> bool {
        matches!(self, Self::Success)
    }
}

/// Counters across every circuit this breaker holds.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BreakerStats {
    /// Calls let through, probes included.
    pub admitted: u64,
    /// Calls let through as a half-open probe.
    pub probes: u64,
    /// Calls refused without touching the network. This is the number the
    /// breaker exists to make non-zero.
    pub refused: u64,
    /// Transitions into [`BreakerState::Open`], counting reopens from
    /// half-open.
    pub opened: u64,
    /// Transitions into [`BreakerState::HalfOpen`].
    pub half_opened: u64,
    /// Transitions back into [`BreakerState::Closed`].
    pub closed: u64,
    pub successes: u64,
    pub failures: u64,
    /// Calls admitted for a peer the registry had no room to track. Non-zero
    /// means the breaker is not protecting that peer at all.
    pub untracked_admissions: u64,
    /// Circuits dropped to make room for a new peer. Only closed circuits are
    /// ever evicted, so this never loses knowledge of an outage in progress.
    pub evicted_circuits: u64,
}

/// One peer's circuit, as a value a health endpoint can render.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CircuitSnapshot {
    pub peer: String,
    pub state: BreakerState,
    pub consecutive_failures: u32,
    /// How many times this circuit has opened without an intervening close.
    /// What the cooldown is currently multiplied by.
    pub consecutive_opens: u32,
    /// When the circuit next admits a probe. Meaningless while closed, and
    /// carried anyway so a snapshot needs no branch to be serialised.
    pub retry_at: Timestamp,
    pub last_error: String,
    pub last_change: Timestamp,
}

#[derive(Clone, Debug)]
struct Circuit {
    state: BreakerState,
    consecutive_failures: u32,
    consecutive_opens: u32,
    half_open_successes: u32,
    probes_in_flight: u32,
    retry_at: Timestamp,
    last_error: String,
    last_change: Timestamp,
    last_admission: Timestamp,
}

impl Circuit {
    fn new(now: Timestamp) -> Self {
        Self {
            state: BreakerState::Closed,
            consecutive_failures: 0,
            consecutive_opens: 0,
            half_open_successes: 0,
            probes_in_flight: 0,
            retry_at: Timestamp::EPOCH,
            last_error: String::new(),
            last_change: now,
            last_admission: now,
        }
    }
}

/// A bounded registry of per-peer circuits.
///
/// `&mut self` on every decision, exactly as [`crate::MeshPublisher`] takes it:
/// a breaker is state, and sharing it between threads without saying so is how
/// two publishers each see half the failures and neither opens. A deployment
/// that needs one breaker across several publishers wraps this in a `Mutex` and
/// pays for it deliberately.
#[derive(Debug)]
pub struct CircuitBreaker {
    policy: BreakerPolicy,
    clock: Arc<dyn Clock>,
    rng: Xoshiro256,
    circuits: BTreeMap<String, Circuit>,
    max_peers: usize,
    stats: BreakerStats,
}

impl CircuitBreaker {
    /// Build a breaker holding at most `max_peers` circuits.
    ///
    /// The bound exists because the key is a peer address and this transport
    /// authenticates nobody: a map keyed by whatever a caller passes is an
    /// unbounded allocation, which is the failure mode every other bound in
    /// this crate refuses.
    ///
    /// At the bound, a new peer evicts the least recently used circuit that is
    /// **closed** — a closed circuit with no failures holds no information, so
    /// dropping it costs nothing but the next failure count. If every tracked
    /// circuit is open or half-open, nothing is evicted: knowledge of an outage
    /// in progress is worth more than a circuit for a peer that has just
    /// appeared. The new peer's call is then admitted *untracked* and counted
    /// in [`BreakerStats::untracked_admissions`], because refusing a call on
    /// the grounds that a bookkeeping table is full would be a breaker
    /// inventing an outage. That counter being non-zero means the bound is too
    /// small for the deployment, and it says so in a number rather than in a
    /// silence.
    pub fn new(
        policy: BreakerPolicy,
        clock: Arc<dyn Clock>,
        seed: u64,
        max_peers: usize,
    ) -> Result<Self> {
        policy.validate()?;
        if max_peers == 0 {
            return Err(Error::invalid(
                "a breaker that can track no peers refuses nothing and protects nothing",
            ));
        }
        Ok(Self {
            policy,
            clock,
            rng: Xoshiro256::seeded(seed),
            circuits: BTreeMap::new(),
            max_peers,
            stats: BreakerStats::default(),
        })
    }

    /// A breaker with the default policy, tracking up to 64 peers — comfortably
    /// more than the nine cells plus the central plane.
    pub fn with_defaults(clock: Arc<dyn Clock>, seed: u64) -> Result<Self> {
        Self::new(BreakerPolicy::default(), clock, seed, 64)
    }

    pub const fn policy(&self) -> BreakerPolicy {
        self.policy
    }

    pub const fn stats(&self) -> BreakerStats {
        self.stats
    }

    /// How many circuits are held. Never more than `max_peers`.
    pub fn tracked_peers(&self) -> usize {
        self.circuits.len()
    }

    /// This peer's state, without changing anything.
    ///
    /// Reports [`BreakerState::Open`] for a circuit whose cooldown has expired
    /// but which nothing has asked about yet: the transition to half-open
    /// happens on [`Self::admit`], because a state machine that advanced while
    /// being observed would make two consecutive reads of an idle breaker
    /// disagree. [`Self::would_admit`] is the question a caller actually means.
    pub fn state(&self, peer: &str) -> BreakerState {
        self.circuits
            .get(peer)
            .map_or(BreakerState::Closed, |circuit| circuit.state)
    }

    /// Whether a call to this peer would be admitted right now, without taking
    /// a probe slot.
    pub fn would_admit(&self, peer: &str) -> bool {
        let Some(circuit) = self.circuits.get(peer) else {
            return true;
        };
        match circuit.state {
            BreakerState::Closed => true,
            BreakerState::Open => self.clock.now() >= circuit.retry_at,
            BreakerState::HalfOpen => circuit.probes_in_flight < self.policy.concurrent_probes,
        }
    }

    /// Every circuit held, for a health endpoint or an operator.
    pub fn snapshot(&self) -> Vec<CircuitSnapshot> {
        self.circuits
            .iter()
            .map(|(peer, circuit)| CircuitSnapshot {
                peer: peer.clone(),
                state: circuit.state,
                consecutive_failures: circuit.consecutive_failures,
                consecutive_opens: circuit.consecutive_opens,
                retry_at: circuit.retry_at,
                last_error: circuit.last_error.clone(),
                last_change: circuit.last_change,
            })
            .collect()
    }

    /// Ask permission to call `peer`.
    ///
    /// This is where the open → half-open transition happens, on the clock, and
    /// where a probe slot is taken.
    pub fn admit(&mut self, peer: &str) -> Decision {
        let now = self.clock.now();
        self.ensure_circuit(peer, now);

        let Some(circuit) = self.circuits.get_mut(peer) else {
            // The registry was full of circuits that are all open or
            // half-open. Admitting untracked is the deliberate choice
            // documented on `new`.
            self.stats.admitted += 1;
            self.stats.untracked_admissions += 1;
            return Decision::Admitted(Permit {
                peer: peer.to_string(),
                probe: false,
                tracked: false,
            });
        };

        circuit.last_admission = now;

        if circuit.state == BreakerState::Open && now >= circuit.retry_at {
            circuit.state = BreakerState::HalfOpen;
            circuit.half_open_successes = 0;
            circuit.probes_in_flight = 0;
            circuit.last_change = now;
            self.stats.half_opened += 1;
        }

        match circuit.state {
            BreakerState::Closed => {
                self.stats.admitted += 1;
                Decision::Admitted(Permit {
                    peer: peer.to_string(),
                    probe: false,
                    tracked: true,
                })
            }
            BreakerState::HalfOpen if circuit.probes_in_flight < self.policy.concurrent_probes => {
                circuit.probes_in_flight += 1;
                self.stats.admitted += 1;
                self.stats.probes += 1;
                Decision::Admitted(Permit {
                    peer: peer.to_string(),
                    probe: true,
                    tracked: true,
                })
            }
            BreakerState::HalfOpen => {
                self.stats.refused += 1;
                Decision::Refused(Refusal {
                    peer: peer.to_string(),
                    state: BreakerState::HalfOpen,
                    consecutive_failures: circuit.consecutive_failures,
                    // Not a timer: this clears when the outstanding probe
                    // reports, and saying "try again in 0s" would be a lie
                    // dressed as a number.
                    retry_after: Duration::ZERO,
                    last_error: circuit.last_error.clone(),
                })
            }
            BreakerState::Open => {
                self.stats.refused += 1;
                Decision::Refused(Refusal {
                    peer: peer.to_string(),
                    state: BreakerState::Open,
                    consecutive_failures: circuit.consecutive_failures,
                    retry_after: circuit.retry_at.since(now).max(Duration::ZERO),
                    last_error: circuit.last_error.clone(),
                })
            }
        }
    }

    /// Report what the admitted call did.
    ///
    /// Takes the permit by value, so one call reports exactly once.
    pub fn record(&mut self, permit: Permit, outcome: Outcome) {
        let now = self.clock.now();
        match &outcome {
            Outcome::Success => self.stats.successes += 1,
            Outcome::Failure(_) => self.stats.failures += 1,
        }
        if !permit.tracked {
            return;
        }
        // Recomputed rather than captured, so the jitter draw and the stat
        // updates below stay outside the borrow of the circuit.
        let cooldown = match &outcome {
            Outcome::Success => Duration::ZERO,
            Outcome::Failure(_) => {
                let opens = self
                    .circuits
                    .get(&permit.peer)
                    .map_or(1, |circuit| circuit.consecutive_opens + 1);
                self.cooldown_for(opens)
            }
        };

        let mut opened = false;
        let mut closed = false;
        if let Some(circuit) = self.circuits.get_mut(&permit.peer) {
            if permit.probe {
                circuit.probes_in_flight = circuit.probes_in_flight.saturating_sub(1);
            }
            match outcome {
                Outcome::Success => {
                    circuit.consecutive_failures = 0;
                    if circuit.state == BreakerState::HalfOpen {
                        circuit.half_open_successes += 1;
                        if circuit.half_open_successes >= self.policy.success_threshold {
                            circuit.state = BreakerState::Closed;
                            circuit.consecutive_opens = 0;
                            circuit.half_open_successes = 0;
                            circuit.probes_in_flight = 0;
                            circuit.last_change = now;
                            closed = true;
                        }
                    }
                }
                Outcome::Failure(detail) => {
                    circuit.last_error = detail;
                    circuit.consecutive_failures = circuit.consecutive_failures.saturating_add(1);
                    // A half-open probe that fails reopens. Unconditionally,
                    // and regardless of the failure threshold: the probe *was*
                    // the test, and it failed.
                    let reopen = circuit.state == BreakerState::HalfOpen;
                    let trip = circuit.state == BreakerState::Closed
                        && circuit.consecutive_failures >= self.policy.failure_threshold;
                    if reopen || trip {
                        circuit.state = BreakerState::Open;
                        circuit.consecutive_opens = circuit.consecutive_opens.saturating_add(1);
                        circuit.half_open_successes = 0;
                        circuit.probes_in_flight = 0;
                        circuit.retry_at = now.saturating_add(cooldown);
                        circuit.last_change = now;
                        opened = true;
                    }
                }
            }
        }
        if opened {
            self.stats.opened += 1;
        }
        if closed {
            self.stats.closed += 1;
        }
    }

    /// Force a peer's circuit closed, discarding its failure history.
    ///
    /// For an operator who knows the peer is back — a rollout that finished, a
    /// NetworkPolicy that was fixed — and does not want to wait out a cooldown
    /// that has been multiplied up to a minute. Counted as a close so the
    /// transition history stays complete.
    pub fn reset(&mut self, peer: &str) {
        let now = self.clock.now();
        if let Some(circuit) = self.circuits.get_mut(peer) {
            let was_closed = circuit.state == BreakerState::Closed;
            *circuit = Circuit::new(now);
            if !was_closed {
                self.stats.closed += 1;
            }
        }
    }

    /// Make room for `peer` if it is not already tracked.
    ///
    /// Leaves the registry unchanged when it is full of circuits that all hold
    /// live information; `admit` then treats the peer as untracked.
    fn ensure_circuit(&mut self, peer: &str, now: Timestamp) {
        if self.circuits.contains_key(peer) {
            return;
        }
        if self.circuits.len() >= self.max_peers {
            let victim = self
                .circuits
                .iter()
                .filter(|(_, circuit)| circuit.state == BreakerState::Closed)
                .min_by_key(|(_, circuit)| circuit.last_admission.as_nanos())
                .map(|(name, _)| name.clone());
            match victim {
                Some(name) => {
                    self.circuits.remove(&name);
                    self.stats.evicted_circuits += 1;
                }
                None => return,
            }
        }
        self.circuits.insert(peer.to_string(), Circuit::new(now));
    }

    /// How long a circuit opening for the `opens`-th consecutive time waits.
    ///
    /// Identical in shape to [`crate::RetryPolicy::backoff`]: exponential,
    /// capped, jittered downward from the cap so the cap is real.
    fn cooldown_for(&mut self, opens: u32) -> Duration {
        let ceiling = self.policy.max_cooldown.as_nanos().max(0) as u128;
        let mut nanos = self.policy.cooldown.as_nanos().max(0) as u128;
        for _ in 1..opens.max(1) {
            nanos = nanos.saturating_mul(u128::from(self.policy.cooldown_multiplier));
            if nanos >= ceiling {
                break;
            }
        }
        let capped = nanos.min(ceiling);
        let span = capped * u128::from(self.policy.jitter_basis_points) / 10_000;
        // `below(n)` draws from `[0, n)`, so `span + 1` makes the whole span
        // reachable and a zero span still draws zero.
        let drawn = u128::from(self.rng.below((span + 1).min(u128::from(u64::MAX)) as u64));
        Duration::from_nanos((capped - drawn.min(capped)) as i64)
    }
}

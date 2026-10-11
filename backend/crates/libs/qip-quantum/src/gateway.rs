//! The Quantum Gateway: the only seam through which anything reaches a
//! [`QuantumProvider`], and it never waits for one.
//!
//! # The failure this closes
//!
//! Until this module existed the compute router in `qip-optimization-engine`
//! called [`QuantumProvider::solve_qubo`] inline, on the kernel's cycle
//! thread, and waited for the answer. With the in-tree simulator that cost
//! milliseconds and nobody noticed. With [`crate::HostedProvider`] attached —
//! it implements the same trait — the DECIDE stage would have blocked on IBM's
//! queue, whose typical delay the adapter itself declares as thirty minutes,
//! and every stage after it (ACT, LEARN, the kill-switch checks they carry)
//! would have waited with it. A cycle whose progress depends on a queue it does
//! not control is the exact shape R12-QUANT-4029 and R12-QUANT-7016 forbid.
//! [`crate::foundry::admit_job`] already refused a synchronous job, and had no
//! production caller, so the refusal guarded nothing.
//!
//! # What the gateway does instead
//!
//! * [`QuantumGateway::submit`] goes through [`crate::foundry::admit_job`] —
//!   its production caller — so an [`Delivery::Inline`] request is refused by
//!   name and a job outside the admitted classes is refused too. An admitted
//!   job runs on its own worker thread and `submit` returns a [`JobTicket`]
//!   at once. No async runtime: a thread and a channel are the blocking-I/O
//!   house style (ADR 0001, ADR 0011) applied to a caller that must not block.
//! * [`QuantumGateway::poll`] never blocks. It answers `Pending`, `Completed`
//!   or `Abandoned`, and the caller — the router — keeps the classical answer
//!   it already holds for every cycle in which the answer is not `Completed`.
//! * Patience is counted in polls, not read from a clock. A job still pending
//!   after [`GatewayLimits::patience_polls`] polls is **abandoned**: whatever
//!   it returns later is discarded unread, because a candidate that arrives
//!   after its caller gave up on it would be judged against a baseline nobody
//!   is holding any more.
//! * An abandoned job keeps its slot until its thread actually returns. A
//!   provider that ignores its own timeout is not handed another job on top
//!   of the one it is still sitting on; with the default single slot the
//!   quantum arm simply stops being tried, which is the fail-closed direction
//!   — the classical baseline is what every cycle runs on regardless.
//!
//! Nothing here judges a result. A completed [`QaoaResult`] is a candidate, and
//! verifying it classically against the problem is the router's job, exactly
//! as it was before the call moved off the cycle thread.

use crate::foundry::{JobClass, admit_job};
use crate::provider::QuantumProvider;
use crate::qaoa::{QaoaResult, QaoaSettings};
use qip_core::error::{Error, Result};
use qip_numerics::anneal::Qubo;
use std::collections::BTreeMap;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::{Arc, Mutex, MutexGuard};

/// How a caller wants the answer delivered.
///
/// Two variants rather than a `synchronous: bool` so that a call site reads as
/// what it asks for. [`Delivery::Inline`] exists to be refused: it is the
/// request R12-QUANT-7016 says must never be admitted, and naming it lets a
/// test prove the refusal rather than assert that nobody would ask.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Delivery {
    /// Submit and return a ticket; the caller polls on its own schedule.
    Polled,
    /// The caller waits for the answer. Always refused.
    Inline,
}

/// A handle on a submitted job. Opaque, ordered, and never reused within one
/// gateway, so a record naming a ticket names one job.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct JobTicket(u64);

impl JobTicket {
    pub fn id(&self) -> u64 {
        self.0
    }
}

/// The gateway's bounds. Both are refused at construction when zero, because a
/// zero slot count is a gateway that admits nothing while reading as one that
/// is configured, and zero patience abandons every job on its first poll.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GatewayLimits {
    /// Jobs that may hold a worker thread at once, abandoned ones included.
    pub max_in_flight: usize,
    /// Polls a job may stay pending before it is abandoned.
    pub patience_polls: u32,
}

impl Default for GatewayLimits {
    /// One slot: the router has one problem per cycle and a second job on the
    /// same problem is a second bill for the same question. Ten polls: the
    /// router polls once per construction, so ten cycles — long enough for a
    /// simulator or a short device queue, and short enough that a result is
    /// still about a universe the platform recognises.
    fn default() -> Self {
        Self {
            max_in_flight: 1,
            patience_polls: 10,
        }
    }
}

/// What a poll found.
#[derive(Debug)]
pub enum JobPoll {
    /// Still running; `polls` is how many times it has now been asked.
    Pending { polls: u32 },
    /// Returned, after `polls` polls (zero when it was ready on the first).
    /// The result is the provider's own, error included.
    Completed {
        polls: u32,
        result: Result<QaoaResult>,
    },
    /// Ran out of patience. Its eventual answer will be discarded unread.
    Abandoned { polls: u32 },
}

/// Counters an operator reads to see what the arm has been doing. Not money:
/// cost is the router's to account, because only it knows which jobs it
/// actually submitted against a budget.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GatewayStats {
    pub submitted: u64,
    pub completed: u64,
    pub abandoned: u64,
    /// Answers that arrived after their job was abandoned and were dropped.
    pub discarded_late: u64,
    /// Submissions refused: inline delivery, an unadmitted class, or no slot.
    pub refused: u64,
}

#[derive(Debug)]
struct Slot {
    class: JobClass,
    receiver: Receiver<Result<QaoaResult>>,
    polls: u32,
    abandoned: bool,
    /// An answer [`QuantumGateway::is_ready`] took off the channel before the
    /// owning poll asked for it.
    ready: Option<Result<QaoaResult>>,
}

#[derive(Debug, Default)]
struct State {
    next: u64,
    slots: BTreeMap<JobTicket, Slot>,
    stats: GatewayStats,
}

/// Submit-and-poll access to one provider.
#[derive(Debug)]
pub struct QuantumGateway {
    provider: Arc<dyn QuantumProvider>,
    limits: GatewayLimits,
    state: Mutex<State>,
}

impl QuantumGateway {
    /// A gateway in front of `provider`, refusing limits that would make it
    /// admit nothing or abandon everything.
    pub fn new(provider: Arc<dyn QuantumProvider>, limits: GatewayLimits) -> Result<Self> {
        if limits.max_in_flight == 0 {
            return Err(Error::invalid(
                "a quantum gateway with no job slots admits nothing while reading as configured; \
                 set max_in_flight to at least one, or attach no quantum provider at all",
            ));
        }
        if limits.patience_polls == 0 {
            return Err(Error::invalid(
                "a quantum gateway with zero patience abandons every job on its first poll; set \
                 patience_polls to at least one",
            ));
        }
        Ok(Self {
            provider,
            limits,
            state: Mutex::new(State::default()),
        })
    }

    pub fn provider(&self) -> &Arc<dyn QuantumProvider> {
        &self.provider
    }

    pub fn limits(&self) -> GatewayLimits {
        self.limits
    }

    fn lock(&self) -> Result<MutexGuard<'_, State>> {
        self.state.lock().map_err(|_| {
            Error::guard(
                "the quantum gateway's state was poisoned by a panicking holder; its job table \
                 cannot be trusted, so nothing is submitted or harvested from it",
            )
        })
    }

    /// Submit one QUBO and return without waiting.
    ///
    /// `class` is matched against [`JobClass`] by its whole delimited name
    /// through [`admit_job`]; [`Delivery::Inline`] is refused there, before
    /// any thread exists.
    pub fn submit(
        &self,
        class: &str,
        delivery: Delivery,
        qubo: Qubo,
        settings: QaoaSettings,
    ) -> Result<JobTicket> {
        let mut state = self.lock()?;
        let admitted = match admit_job(class, delivery == Delivery::Inline) {
            Ok(admitted) => admitted,
            Err(refusal) => {
                state.stats.refused += 1;
                return Err(refusal);
            }
        };
        Self::reap(&mut state);
        if state.slots.len() >= self.limits.max_in_flight {
            state.stats.refused += 1;
            let abandoned = state.slots.values().filter(|slot| slot.abandoned).count();
            return Err(Error::denied(format!(
                "the quantum gateway's {} slot(s) are occupied ({abandoned} by abandoned job(s) \
                 whose provider has not yet returned); harvest or wait rather than queue a second \
                 job on top — the classical baseline stands meanwhile",
                self.limits.max_in_flight
            )));
        }
        let ticket = JobTicket(state.next);
        let (sender, receiver) = mpsc::channel();
        let provider = Arc::clone(&self.provider);
        std::thread::Builder::new()
            .name(format!("qip-quantum-job-{}", ticket.0))
            .spawn(move || {
                // The receiver may be gone (gateway dropped); an answer
                // nobody is waiting for is correctly dropped with it.
                let _ = sender.send(provider.solve_qubo(&qubo, &settings));
            })
            .map_err(|error| {
                Error::unavailable(format!(
                    "the quantum gateway could not start a worker thread ({error}); the job was \
                     not submitted and the classical baseline stands"
                ))
            })?;
        state.next += 1;
        state.stats.submitted += 1;
        state.slots.insert(
            ticket,
            Slot {
                class: admitted,
                receiver,
                polls: 0,
                abandoned: false,
                ready: None,
            },
        );
        Ok(ticket)
    }

    /// Ask after a job without waiting for it.
    ///
    /// Each call counts as one poll against the job's patience. An unknown
    /// ticket — never issued, already harvested, or already abandoned — is
    /// refused rather than reported as pending, because "pending forever" is
    /// the one answer a caller would wait on.
    pub fn poll(&self, ticket: JobTicket) -> Result<JobPoll> {
        let mut state = self.lock()?;
        Self::reap(&mut state);
        let patience = self.limits.patience_polls;
        let Some(slot) = state.slots.get_mut(&ticket) else {
            return Err(Error::not_found(format!(
                "quantum job {} is not held by this gateway: it was never issued, has already been \
                 harvested, or was abandoned",
                ticket.0
            )));
        };
        if slot.abandoned {
            return Err(Error::not_found(format!(
                "quantum job {} was abandoned after {} poll(s); its answer will be discarded",
                ticket.0, slot.polls
            )));
        }
        let received = match slot.ready.take() {
            Some(result) => Ok(result),
            None => slot.receiver.try_recv(),
        };
        match received {
            Ok(result) => {
                let polls = slot.polls;
                state.slots.remove(&ticket);
                state.stats.completed += 1;
                Ok(JobPoll::Completed { polls, result })
            }
            Err(TryRecvError::Disconnected) => {
                let polls = slot.polls;
                let class = slot.class.name();
                state.slots.remove(&ticket);
                state.stats.completed += 1;
                Ok(JobPoll::Completed {
                    polls,
                    result: Err(Error::unavailable(format!(
                        "the {class} job {}'s worker ended without an answer (the provider \
                         panicked); the classical baseline stands",
                        ticket.0
                    ))),
                })
            }
            Err(TryRecvError::Empty) => {
                slot.polls = slot.polls.saturating_add(1);
                let polls = slot.polls;
                if polls >= patience {
                    slot.abandoned = true;
                    state.stats.abandoned += 1;
                    Ok(JobPoll::Abandoned { polls })
                } else {
                    Ok(JobPoll::Pending { polls })
                }
            }
        }
    }

    /// Free the slots of abandoned jobs whose threads have since returned,
    /// discarding their answers unread.
    fn reap(state: &mut State) {
        let finished: Vec<JobTicket> = state
            .slots
            .iter()
            .filter(|(_, slot)| {
                slot.abandoned && !matches!(slot.receiver.try_recv(), Err(TryRecvError::Empty))
            })
            .map(|(ticket, _)| *ticket)
            .collect();
        for ticket in finished {
            state.slots.remove(&ticket);
            state.stats.discarded_late += 1;
        }
    }

    /// Whether a job's answer has arrived, without counting a poll against its
    /// patience and without handing the answer over.
    ///
    /// For a reader that must not change what the owning caller will see — an
    /// operator's view, or a test waiting for a worker to finish before it
    /// drives the next cycle. It never blocks.
    pub fn is_ready(&self, ticket: JobTicket) -> Result<bool> {
        let mut state = self.lock()?;
        let Some(slot) = state.slots.get_mut(&ticket) else {
            return Err(Error::not_found(format!(
                "quantum job {} is not held by this gateway",
                ticket.0
            )));
        };
        if slot.ready.is_some() {
            return Ok(true);
        }
        match slot.receiver.try_recv() {
            Ok(result) => {
                slot.ready = Some(result);
                Ok(true)
            }
            Err(TryRecvError::Disconnected) => Ok(true),
            Err(TryRecvError::Empty) => Ok(false),
        }
    }

    /// Jobs currently holding a slot, abandoned ones included.
    pub fn in_flight(&self) -> Result<usize> {
        let mut state = self.lock()?;
        Self::reap(&mut state);
        Ok(state.slots.len())
    }

    pub fn stats(&self) -> Result<GatewayStats> {
        Ok(self.lock()?.stats)
    }
}

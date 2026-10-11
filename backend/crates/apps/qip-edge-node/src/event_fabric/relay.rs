//! The journal relay: the stage between the decision thread's handoff and
//! the spool writer, and the thread the configured journal store is written
//! on.
//!
//! # Why a relay rather than a second mirror on the decision thread
//!
//! Until this stage existed the node called `Cell::flush` into a
//! `StoreMirror` on the thread that runs `Cell::work`, so every pass waited
//! for a key-value write — on the engine target an fsync, on a managed one a
//! network round trip — and a store that hung stopped the cell deciding for
//! exactly as long as it hung (ADR 0100 §6, red-team M5). The decision thread
//! now hands every journal batch to a bounded channel through
//! [`FabricMirror`](super::mirror::FabricMirror), which never waits; this
//! relay takes it from there on its own thread.
//!
//! Each handoff is forwarded to the spool writer first — the local, fsynced
//! record ADR 0100 §1 makes the source of truth — and a journal batch is then
//! shipped to the store, retried with a bounded backoff until the store takes
//! it. Order matters: a batch the store holds and the spool does not would be
//! a record the fabric never sees.
//!
//! # What a stalled store costs, and who pays
//!
//! While the store refuses, the relay keeps retrying the batch it holds and
//! takes nothing else, so the handoff channel fills and the mirror refuses
//! the next flush. The refused entries stay unshipped in the cell's journal,
//! in memory and chained — nothing is lost — and the node's journal bound
//! (`scheduler::JournalBound`) narrows and then halts new exposure as that
//! backlog grows. The decision thread never waits: a stalled store is a
//! narrowing the chain records, not a frozen cell.

use crate::event_fabric::writer::{Handoff, HandoffReceiver, HandoffSender, Offer, Received};
use qip_core::error::{Error, Result};
use qip_edge::journal::Mirror;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

/// The relay thread's name, as `ps` and a panic message show it.
pub const RELAY_THREAD: &str = "qip-journal-relay";

/// How the relay waits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RelayConfig {
    poll: Duration,
    retry_floor: Duration,
    retry_ceiling: Duration,
}

impl RelayConfig {
    /// Wait at most `poll` on an empty channel or a full writer, and retry a
    /// refused store write after `retry_floor`, doubling to `retry_ceiling`.
    ///
    /// Refuses a zero poll, which spins a core on an idle channel; a zero
    /// floor, which retries a failing store in a hot loop; and a ceiling
    /// below the floor, which is two numbers that cannot both be meant.
    pub fn new(poll: Duration, retry_floor: Duration, retry_ceiling: Duration) -> Result<Self> {
        if poll.is_zero() {
            return Err(Error::invalid(
                "a relay poll of zero spins on an idle channel; configure a bounded wait",
            ));
        }
        if retry_floor.is_zero() {
            return Err(Error::invalid(
                "a store retry floor of zero retries a failing store in a hot loop; configure a \
                 positive backoff",
            ));
        }
        if retry_ceiling < retry_floor {
            return Err(Error::invalid(format!(
                "a store retry ceiling of {retry_ceiling:?} is below its {retry_floor:?} floor"
            )));
        }
        Ok(Self {
            poll,
            retry_floor,
            retry_ceiling,
        })
    }
}

/// Counters the relay publishes, read by the health surface.
#[derive(Debug, Default)]
struct Counters {
    shipped_entries: AtomicU64,
    shipped_batches: AtomicU64,
    store_failures: AtomicU64,
    forwarded: AtomicU64,
}

/// The running relay. Dropping it asks the thread to stop after its current
/// wait; it is not joined, because a store write that hangs would make the
/// drop hang with it, and the drop is on the decision thread.
#[derive(Debug)]
pub struct JournalRelay {
    stop: Arc<AtomicBool>,
    counters: Arc<Counters>,
    handle: Option<JoinHandle<()>>,
}

impl JournalRelay {
    /// Start relaying from `receiver` to `forward`, shipping every journal
    /// batch to `store` after the spool writer has taken it.
    pub fn spawn<M>(
        receiver: HandoffReceiver,
        forward: HandoffSender,
        store: M,
        config: RelayConfig,
    ) -> Result<Self>
    where
        M: Mirror + Send + 'static,
    {
        let stop = Arc::new(AtomicBool::new(false));
        let counters = Arc::new(Counters::default());
        let thread_stop = Arc::clone(&stop);
        let thread_counters = Arc::clone(&counters);
        let handle = std::thread::Builder::new()
            .name(RELAY_THREAD.to_string())
            .spawn(move || {
                run(
                    &receiver,
                    &forward,
                    store,
                    config,
                    &thread_stop,
                    &thread_counters,
                );
            })
            .map_err(|error| {
                Error::io(format!("cannot start the {RELAY_THREAD} thread: {error}"))
            })?;
        Ok(Self {
            stop,
            counters,
            handle: Some(handle),
        })
    }

    /// Journal entries the store has taken.
    pub fn shipped_entries(&self) -> u64 {
        self.counters.shipped_entries.load(Ordering::Acquire)
    }

    /// Journal batches the store has taken.
    pub fn shipped_batches(&self) -> u64 {
        self.counters.shipped_batches.load(Ordering::Acquire)
    }

    /// Store writes refused and retried.
    pub fn store_failures(&self) -> u64 {
        self.counters.store_failures.load(Ordering::Acquire)
    }

    /// Handoffs the spool writer's channel has taken from the relay.
    pub fn forwarded(&self) -> u64 {
        self.counters.forwarded.load(Ordering::Acquire)
    }

    /// Whether the thread is still running. A relay that has ended — its
    /// writer gone, or asked to stop — ships nothing more, and the mirror
    /// then refuses every flush as closed.
    pub fn is_running(&self) -> bool {
        self.handle
            .as_ref()
            .is_some_and(|handle| !handle.is_finished())
    }
}

impl Drop for JournalRelay {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
    }
}

fn run<M: Mirror>(
    receiver: &HandoffReceiver,
    forward: &HandoffSender,
    mut store: M,
    config: RelayConfig,
    stop: &AtomicBool,
    counters: &Counters,
) {
    while !stop.load(Ordering::Acquire) {
        let handoff = match receiver.receive(config.poll) {
            Received::Handoff(handoff) => handoff,
            Received::Idle => continue,
            Received::Closed => return,
        };
        let journal = match &handoff {
            Handoff::Journal(batch) => Some(batch.clone()),
            Handoff::Pass(_) | Handoff::InputGap(_) => None,
        };
        if !forward_until_taken(forward, handoff, config.poll, stop) {
            return;
        }
        counters.forwarded.fetch_add(1, Ordering::AcqRel);
        if let Some(batch) = journal {
            let entries = batch.entries.len() as u64;
            let mut backoff = config.retry_floor;
            loop {
                match store.ship(batch.clone()) {
                    Ok(()) => {
                        counters
                            .shipped_entries
                            .fetch_add(entries, Ordering::AcqRel);
                        counters.shipped_batches.fetch_add(1, Ordering::AcqRel);
                        break;
                    }
                    Err(error) => {
                        let failures = counters.store_failures.fetch_add(1, Ordering::AcqRel);
                        // The first refusal and then one in a hundred: a
                        // store that is down for an hour must not turn
                        // stderr into the incident.
                        if failures.is_multiple_of(100) {
                            eprintln!(
                                "qip-edge-node: the journal store refused a batch ({}); retrying \
                                 in {backoff:?} — the entries are in the spool and stay \
                                 unshipped in the cell until the store takes them",
                                error.message()
                            );
                        }
                        if stop.load(Ordering::Acquire) {
                            return;
                        }
                        std::thread::sleep(backoff);
                        backoff = backoff.saturating_mul(2).min(config.retry_ceiling);
                    }
                }
            }
        }
    }
}

/// Offer `handoff` to the spool writer until it is taken. `false` when the
/// writer has gone or the relay was asked to stop.
fn forward_until_taken(
    forward: &HandoffSender,
    mut handoff: Handoff,
    wait: Duration,
    stop: &AtomicBool,
) -> bool {
    loop {
        match forward.offer(handoff) {
            Offer::Accepted => return true,
            Offer::Full(back) => {
                if stop.load(Ordering::Acquire) {
                    return false;
                }
                handoff = back;
                std::thread::sleep(wait);
            }
            Offer::Closed(_) => return false,
        }
    }
}

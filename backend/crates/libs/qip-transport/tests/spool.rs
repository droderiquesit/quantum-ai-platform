//! The durable spool, across simulated restarts.
//!
//! Every test here that matters does the same thing: builds a spool, does some
//! work, *drops it*, and opens a new one over the same store. That drop is the
//! pod restart. A spool tested without one is a spool whose single claim —
//! that it survives the process — is the one thing untested.

#![allow(clippy::panic_in_result_fn)]

use qip_core::error::{Error, Result};
use qip_core::{Id, Lineage, Timestamp};
use qip_events::{AnyEvent, Envelope, EventBody, Topic};
use qip_storage::kv::{KeyValueStore, MemoryKeyValueStore};
use qip_transport::deadletter::{DeadLetter, DeadLetterReason, DeadLetterSink};
use qip_transport::spool::{DurableDeadLetters, DurableSpool};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

const BASE_NANOS: i64 = 1_704_205_445_000_000_000;

fn at(millis: i64) -> Timestamp {
    Timestamp::from_nanos(BASE_NANOS + millis * 1_000_000)
}

/// A signed capital envelope: the message class the spool exists for, and the
/// one for which losing it is not an acceptable trade.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct CapitalEnvelope {
    cell: String,
    sequence: u64,
    notional_minor: i64,
}

impl EventBody for CapitalEnvelope {
    const TOPIC: Topic = Topic::OrderApproved;
    const SCHEMA_VERSION: u32 = 1;

    fn idempotency_key(&self) -> Option<String> {
        Some(format!("{}:{}", self.cell, self.sequence))
    }
}

fn frame(cell: &str, sequence: u64) -> Result<AnyEvent> {
    Envelope::new(
        Id::from_string(format!("EVT{cell}{sequence:0>20}")),
        at(sequence as i64),
        at(sequence as i64),
        Lineage::root(
            qip_core::CorrelationId::from_string(format!("COR{cell}{sequence:0>20}")),
            "qip-transport-spool-tests",
        ),
        CapitalEnvelope {
            cell: cell.to_string(),
            sequence,
            notional_minor: 25_000_000 + sequence as i64,
        },
    )
    .erase()
}

/// The disk that outlives the process. Shared between a spool and the one that
/// replaces it after the restart.
fn disk() -> Arc<dyn KeyValueStore> {
    Arc::new(MemoryKeyValueStore::new())
}

// --- the ordering that is the whole design ----------------------------------

#[test]
fn a_message_persisted_but_not_committed_is_still_there_after_a_restart() -> Result<()> {
    let disk = disk();

    // Before the crash: three messages persisted, the first one sent and
    // acknowledged. The other two were in flight.
    let first_sequence = {
        let mut spool = DurableSpool::open(Arc::clone(&disk), "cell-london", 64)?;
        let first = spool.push(frame("london", 1)?).map_err(Error::from)?;
        spool.push(frame("london", 2)?).map_err(Error::from)?;
        spool.push(frame("london", 3)?).map_err(Error::from)?;
        spool.commit(first)?;
        assert_eq!(spool.depth()?, 2);
        first
    }; // <- the pod dies here.

    let recovered = DurableSpool::open(Arc::clone(&disk), "cell-london", 64)?;
    assert_eq!(
        recovered.depth()?,
        2,
        "the restart lost the messages that had not been acknowledged"
    );
    assert_eq!(
        recovered.stats().recovered,
        2,
        "the spool did not report that it inherited unfinished work"
    );

    // The committed one is gone and stays gone: a spool that resurrected
    // acknowledged messages would re-send capital instructions on every
    // restart, which is a worse failure than the one it prevents.
    let backlog = recovered.backlog()?;
    assert!(
        !backlog.iter().any(|entry| entry.sequence == first_sequence),
        "an acknowledged message came back after the restart"
    );
    assert_eq!(
        backlog
            .iter()
            .map(|entry| entry.sequence)
            .collect::<Vec<_>>(),
        vec![1, 2],
        "the backlog is not in order"
    );
    Ok(())
}

#[test]
fn a_recovered_spool_continues_the_sequence_rather_than_restarting_it() -> Result<()> {
    // If the sequence reset, new messages would sort *before* the unsent
    // backlog, and the backlog would never be reached — a queue that silently
    // stops draining its oldest entries while looking perfectly healthy.
    let disk = disk();
    {
        let mut spool = DurableSpool::open(Arc::clone(&disk), "cell-tokyo", 64)?;
        spool.push(frame("tokyo", 1)?).map_err(Error::from)?;
        spool.push(frame("tokyo", 2)?).map_err(Error::from)?;
    }

    let mut recovered = DurableSpool::open(Arc::clone(&disk), "cell-tokyo", 64)?;
    let next = recovered.push(frame("tokyo", 3)?).map_err(Error::from)?;
    assert_eq!(
        next, 2,
        "the sequence restarted and will overwrite or reorder"
    );

    let order = recovered
        .backlog()?
        .iter()
        .map(|entry| entry.sequence)
        .collect::<Vec<_>>();
    assert_eq!(order, vec![0, 1, 2], "the new message did not sort last");

    // And the oldest is still what comes out first.
    let front = recovered
        .front()?
        .ok_or_else(|| Error::not_found("a front entry"))?;
    assert_eq!(front.sequence, 0);
    Ok(())
}

#[test]
fn the_attempt_count_survives_the_restart_that_the_retry_budget_depends_on() -> Result<()> {
    // Without this, a message that fails permanently is retried from zero
    // after every restart and never reaches the dead-letter sink: an infinite
    // retry loop that looks like a transport doing its job.
    let disk = disk();
    let sequence = {
        let mut spool = DurableSpool::open(Arc::clone(&disk), "cell-frankfurt", 64)?;
        let sequence = spool.push(frame("frankfurt", 1)?).map_err(Error::from)?;
        spool.record_attempt(sequence, &"connection refused")?;
        spool.record_attempt(sequence, &"connection refused")?;
        sequence
    };

    let recovered = DurableSpool::open(Arc::clone(&disk), "cell-frankfurt", 64)?;
    let entry = recovered
        .front()?
        .ok_or_else(|| Error::not_found("the recovered entry"))?;
    assert_eq!(
        entry.attempts, 2,
        "the attempt count reset across the restart; this message would retry forever"
    );
    assert_eq!(entry.sequence, sequence);
    assert_eq!(
        entry.last_error.as_deref(),
        Some("connection refused"),
        "the recovered entry does not say why it is still here"
    );
    Ok(())
}

#[test]
fn recording_an_attempt_on_a_committed_entry_is_an_error_rather_than_a_resurrection() -> Result<()>
{
    // The race a real publisher can hit: the acknowledgement arrives and the
    // entry is committed, then a slow duplicate send reports its failure. That
    // must not write the entry back, or a delivered message returns to the
    // backlog and is sent again.
    let disk = disk();
    let mut spool = DurableSpool::open(Arc::clone(&disk), "cell-sydney", 64)?;
    let sequence = spool.push(frame("sydney", 1)?).map_err(Error::from)?;
    assert!(spool.commit(sequence)?);

    assert!(
        spool.record_attempt(sequence, &"a late failure").is_err(),
        "a committed entry was written back into the backlog"
    );
    assert!(
        spool.is_empty()?,
        "the spool is not empty after a committed entry"
    );
    Ok(())
}

// --- bounded, refusing, never dropping --------------------------------------

#[test]
fn a_full_spool_refuses_and_does_not_drop_what_it_already_holds() -> Result<()> {
    // The same contract as the in-memory queue: refuse, never drop, never
    // grow. A spool that grew without bound trades a lost message for an
    // exhausted disk, which fails everything rather than one thing.
    let disk = disk();
    let mut spool = DurableSpool::open(Arc::clone(&disk), "cell-small", 2)?;
    spool.push(frame("small", 1)?).map_err(Error::from)?;
    spool.push(frame("small", 2)?).map_err(Error::from)?;

    let refusal = spool
        .push(frame("small", 3)?)
        .expect_err("a full spool admitted a message");
    assert_eq!(refusal.code(), "queue_full");
    assert_eq!(spool.stats().refused, 1);
    assert_eq!(
        spool.depth()?,
        2,
        "the refusal disturbed what was already held"
    );

    // Committing one makes room, and the refusal was not permanent.
    spool.commit(0)?;
    spool.push(frame("small", 3)?).map_err(Error::from)?;
    assert_eq!(spool.depth()?, 2);

    // A spool that can hold nothing is a disabled transport, not a small one.
    assert!(DurableSpool::open(disk, "cell-zero", 0).is_err());
    Ok(())
}

#[test]
fn two_spools_over_one_store_do_not_see_each_other() -> Result<()> {
    // Namespaced by name, so one cell's backlog is not another's. A shared
    // namespace would have each publisher draining the other's messages.
    let disk = disk();
    let mut london = DurableSpool::open(Arc::clone(&disk), "cell-london", 64)?;
    let mut tokyo = DurableSpool::open(Arc::clone(&disk), "cell-tokyo", 64)?;

    london.push(frame("london", 1)?).map_err(Error::from)?;
    london.push(frame("london", 2)?).map_err(Error::from)?;
    tokyo.push(frame("tokyo", 1)?).map_err(Error::from)?;

    assert_eq!(london.depth()?, 2);
    assert_eq!(tokyo.depth()?, 1);
    Ok(())
}

#[test]
fn the_frame_comes_back_byte_for_byte_so_it_can_be_re_sent_not_reconstructed() -> Result<()> {
    let disk = disk();
    let original = frame("london", 42)?;
    {
        let mut spool = DurableSpool::open(Arc::clone(&disk), "cell-london", 64)?;
        spool.push(original.clone()).map_err(Error::from)?;
    }

    let recovered = DurableSpool::open(disk, "cell-london", 64)?;
    let entry = recovered
        .front()?
        .ok_or_else(|| Error::not_found("the recovered entry"))?;
    assert_eq!(
        entry.frame, original,
        "the frame did not survive the round trip intact"
    );
    assert_eq!(
        entry.key,
        original.dedup_key(),
        "the idempotency key was not carried, so the peer cannot detect the duplicate"
    );
    Ok(())
}

// --- the durable dead-letter sink -------------------------------------------

fn letter(key: &str) -> Result<DeadLetter> {
    Ok(DeadLetter {
        key: key.to_string(),
        transport: "mesh".to_string(),
        peer: "http://cell-london:8080".to_string(),
        reason: DeadLetterReason::RetriesExhausted,
        attempts: 5,
        last_error: "connection refused".to_string(),
        recorded_at: at(0),
        frame: frame("london", 1)?,
    })
}

#[test]
fn dead_letters_outlive_the_process_that_recorded_them() -> Result<()> {
    // The record of what never arrived is most needed after the restart that
    // usually follows the incident that produced it. An in-memory sink loses
    // it exactly then.
    let disk = disk();
    {
        let mut sink = DurableDeadLetters::open(Arc::clone(&disk), "cell-london")?;
        sink.record(letter("london:1")?);
        sink.record(letter("london:2")?);
        assert_eq!(sink.len(), 2);
    }

    let recovered = DurableDeadLetters::open(Arc::clone(&disk), "cell-london")?;
    assert_eq!(recovered.len(), 2, "the restart lost the dead letters");
    assert_eq!(
        recovered.recorded(),
        2,
        "the recovered sink does not count what it inherited"
    );

    let letters = recovered.letters()?;
    assert_eq!(letters.len(), 2);
    assert!(
        letters[0].frame == frame("london", 1)?,
        "the letter did not keep the message, so it cannot be re-sent"
    );

    // An operator who has dealt with one removes it, and the other stays.
    assert!(recovered.release("london:1")?);
    assert_eq!(recovered.len(), 1);
    assert!(
        !recovered.release("london:1")?,
        "releasing twice reported success"
    );
    Ok(())
}

#[test]
fn the_same_message_failing_twice_is_two_letters_not_one() -> Result<()> {
    // Keyed by arrival, not by message. Two failures of one message are two
    // facts an operator needs; keying by the message would keep only the last
    // and quietly understate how bad the outage was.
    let disk = disk();
    let mut sink = DurableDeadLetters::open(Arc::clone(&disk), "cell-london")?;
    sink.record(letter("london:1")?);
    sink.record(letter("london:1")?);
    assert_eq!(
        sink.len(),
        2,
        "a repeated failure overwrote the first record"
    );
    Ok(())
}

/// A store whose writes fail on demand — a full disk, or a revoked mount.
#[derive(Debug)]
struct FailingStore {
    inner: MemoryKeyValueStore,
    failing: Arc<AtomicBool>,
}

impl KeyValueStore for FailingStore {
    fn get(&self, key: &str) -> Result<Option<serde_json::Value>> {
        self.inner.get(key)
    }

    fn put(&self, key: &str, value: serde_json::Value) -> Result<()> {
        if self.failing.load(Ordering::SeqCst) {
            return Err(Error::io("no space left on device"));
        }
        self.inner.put(key, value)
    }

    fn delete(&self, key: &str) -> Result<bool> {
        self.inner.delete(key)
    }

    fn keys_with_prefix(&self, prefix: &str) -> Result<Vec<String>> {
        self.inner.keys_with_prefix(prefix)
    }

    fn len(&self) -> Result<usize> {
        self.inner.len()
    }
}

#[test]
fn a_sink_that_cannot_write_counts_that_fact_rather_than_failing() -> Result<()> {
    // `DeadLetterSink::record` is infallible by contract: a sink that could
    // fail would need a dead-letter path of its own and the recursion has to
    // stop somewhere. So the failure becomes a number — and a non-zero one is
    // worse news than a dead letter, because it means the record of what never
    // arrived is itself incomplete.
    let failing = Arc::new(AtomicBool::new(false));
    let store = Arc::new(FailingStore {
        inner: MemoryKeyValueStore::new(),
        failing: Arc::clone(&failing),
    });
    let mut sink = DurableDeadLetters::open(store, "cell-london")?;

    sink.record(letter("london:1")?);
    assert_eq!(sink.len(), 1);
    assert_eq!(sink.unrecordable(), 0);

    failing.store(true, Ordering::SeqCst);
    sink.record(letter("london:2")?);

    assert_eq!(
        sink.len(),
        1,
        "a letter was recorded despite the store failing"
    );
    assert_eq!(
        sink.unrecordable(),
        1,
        "the sink swallowed a letter it could not write without counting it"
    );
    assert_eq!(
        sink.recorded(),
        1,
        "the sink counted a letter it did not store as recorded"
    );
    Ok(())
}

#[test]
fn a_spool_that_cannot_write_refuses_the_message_rather_than_reporting_success() -> Result<()> {
    // The spool's whole promise is that a message it accepted is on disk. If
    // the write fails, accepting it would be a lie the caller acts on by
    // sending and then forgetting.
    let failing = Arc::new(AtomicBool::new(true));
    let store = Arc::new(FailingStore {
        inner: MemoryKeyValueStore::new(),
        failing: Arc::clone(&failing),
    });
    let mut spool = DurableSpool::open(store, "cell-london", 64)?;

    let error = spool
        .push(frame("london", 1)?)
        .expect_err("the spool accepted a message it could not persist");
    assert!(
        error.to_string().contains("could not be written"),
        "the refusal does not say the write failed: {error}"
    );
    assert!(spool.is_empty()?);

    // And it recovers: the failure was the disk's, not the spool's state.
    failing.store(false, Ordering::SeqCst);
    spool.push(frame("london", 1)?).map_err(Error::from)?;
    assert_eq!(spool.depth()?, 1);
    Ok(())
}

#[test]
fn the_backlog_stays_in_order_past_the_first_digit_boundary() -> Result<()> {
    // Keys are ordered lexicographically by the store, so the zero-padding is
    // what makes that ordering numeric. Without it "10" sorts before "9" and
    // the spool starts draining out of order at the eleventh message — a bug
    // that every small test passes straight over.
    let disk = disk();
    let mut spool = DurableSpool::open(Arc::clone(&disk), "cell-london", 4096)?;
    for sequence in 0..25 {
        spool
            .push(frame("london", sequence)?)
            .map_err(Error::from)?;
    }

    let order = spool
        .backlog()?
        .iter()
        .map(|entry| entry.sequence)
        .collect::<Vec<_>>();
    assert_eq!(
        order,
        (0..25).collect::<Vec<_>>(),
        "the backlog left numeric order once the sequence grew a digit"
    );

    // And the same across a restart, since recovery reads the same ordering.
    let recovered = DurableSpool::open(disk, "cell-london", 4096)?;
    let front = recovered
        .front()?
        .ok_or_else(|| Error::not_found("a front entry"))?;
    assert_eq!(
        front.sequence, 0,
        "the oldest entry is not what comes out first"
    );
    Ok(())
}

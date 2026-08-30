//! The transports: the durable log that works, and the Pub/Sub port that does
//! not pretend to.

#![allow(clippy::panic_in_result_fn)]

mod common;

use common::{at, hot_tick, warm_note};
use qip_contracts::time::Watermark;
use qip_core::Timestamp;
use qip_core::error::{Error, Result};
use qip_events::EventFilter;
use qip_streaming::{
    DurableLogTransport, LocalQueue, PubSubBinding, PubSubTransport, Publisher, StreamEnvelope,
    Subscriber, TransportPath,
};

/// A directory this test run owns, under the system temporary directory.
fn scratch(label: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("qip-streaming-{}-{label}", std::process::id()))
}

#[test]
fn the_durable_transport_round_trips_a_batch_and_its_hash_chain_verifies() -> Result<()> {
    let mut transport = DurableLogTransport::in_memory("orders");
    let batch = vec![
        warm_note("A", at(0), at(10))?,
        warm_note("B", at(1), at(20))?,
        warm_note("C", at(2), at(30))?,
    ];

    let receipts = transport.publish_batch(batch.clone(), at(30))?;
    assert_eq!(
        receipts.iter().map(|r| r.position).collect::<Vec<_>>(),
        vec![1, 2, 3],
        "the log assigns its own contiguous positions"
    );
    assert!(
        receipts.iter().all(|r| r.record_hash.is_some()),
        "every durable receipt carries the record hash it was chained under"
    );
    assert!(
        receipts.iter().all(|r| r.path == TransportPath::Durable),
        "the receipt must say which wire it went down"
    );

    let drained = transport.poll(at(30))?;
    assert_eq!(
        drained, batch,
        "what comes back must be exactly what went in, envelope for envelope"
    );
    for envelope in &drained {
        envelope.verify_payload_hash()?;
    }

    transport
        .verify_chain()
        .map_err(|sequence| Error::invalid(format!("the chain broke at {sequence}")))?;
    Ok(())
}

#[test]
fn tampering_with_a_stored_record_breaks_the_chain_at_that_record() -> Result<()> {
    let path = scratch("tamper");
    let _ = std::fs::remove_dir_all(&path);
    let file = path.join("log.jsonl");

    {
        let mut transport = DurableLogTransport::open("orders", &file)?;
        for (label, step) in [("A", 10), ("B", 20), ("C", 30)] {
            transport.publish(warm_note(label, at(0), at(step))?, at(step))?;
        }
        transport
            .verify_chain()
            .map_err(|sequence| Error::invalid(format!("the chain broke at {sequence}")))?;
    }

    // Edit the middle record on disk, the way an operator covering something up
    // would. The chain is what makes that detectable at all.
    let contents = std::fs::read_to_string(&file)?;
    let mut lines: Vec<String> = contents.lines().map(str::to_string).collect();
    assert_eq!(lines.len(), 3);
    lines[1] = lines[1].replace("note B", "note B (edited)");
    std::fs::write(&file, lines.join("\n") + "\n")?;

    let reopened = DurableLogTransport::open("orders", &file)?;
    let broken_at = reopened
        .verify_chain()
        .expect_err("an edited record must break the chain");
    assert_eq!(
        broken_at, 2,
        "the break must be located, not merely reported"
    );

    let _ = std::fs::remove_dir_all(&path);
    Ok(())
}

#[test]
fn a_file_backed_log_is_still_there_after_the_process_that_wrote_it() -> Result<()> {
    let path = scratch("restart");
    let _ = std::fs::remove_dir_all(&path);
    let file = path.join("log.jsonl");

    let written = vec![
        warm_note("A", at(0), at(10))?,
        warm_note("B", at(1), at(20))?,
    ];
    {
        let mut transport = DurableLogTransport::open("orders", &file)?;
        transport.publish_batch(written.clone(), at(20))?;
    }

    let mut reopened = DurableLogTransport::open("orders", &file)?;
    assert_eq!(reopened.len(), 2);
    assert_eq!(reopened.poll(at(20))?, written);
    assert_eq!(reopened.replay(&EventFilter::new())?, written);

    let _ = std::fs::remove_dir_all(&path);
    Ok(())
}

#[test]
fn the_durable_transport_refuses_a_hot_event() -> Result<()> {
    let mut transport = DurableLogTransport::in_memory("ticks");
    let error = transport
        .publish(hot_tick(1, at(0), at(1))?, at(1))
        .expect_err("a hot event must not be appended to the durable log");
    assert!(matches!(error, Error::Invalid(_)));
    assert!(
        error.message().contains("routed hot"),
        "the refusal must name the class that was wrong: {error}"
    );
    assert_eq!(transport.len(), 0, "nothing may be written on the way out");
    Ok(())
}

#[test]
fn until_is_known_time_so_a_consumer_never_drains_past_its_own_clock() -> Result<()> {
    let mut transport = DurableLogTransport::in_memory("orders");
    // Valid-time is the same for all three; only known-time differs. A
    // consumer at `until` must see exactly what the platform knew by then.
    transport.publish(warm_note("A", at(0), at(10))?, at(10))?;
    transport.publish(warm_note("B", at(0), at(20))?, at(20))?;
    transport.publish(warm_note("C", at(0), at(30))?, at(30))?;

    assert!(transport.poll(at(9))?.is_empty(), "nothing was known yet");
    assert_eq!(transport.poll(at(20))?.len(), 2);
    assert_eq!(
        transport.watermark(),
        Some(Watermark::new("orders", 2, at(20)))
    );
    assert_eq!(transport.poll(at(20))?.len(), 0, "a drain is not repeated");
    assert_eq!(transport.poll(at(60))?.len(), 1);
    assert_eq!(
        transport.watermark(),
        Some(Watermark::new("orders", 3, at(30)))
    );
    Ok(())
}

#[test]
fn a_record_appended_late_but_stamped_early_is_held_rather_than_skipped() -> Result<()> {
    let mut transport = DurableLogTransport::in_memory("orders");
    transport.publish(warm_note("A", at(0), at(10))?, at(10))?;
    transport.publish(warm_note("B", at(0), at(50))?, at(50))?;
    // Stamped before B but appended after it — a late arrival on a slow path.
    transport.publish(warm_note("C", at(0), at(20))?, at(50))?;

    // The cursor stops at B rather than jumping over it, so C is not lost
    // behind a cursor that has already moved past its position.
    let first = transport.poll(at(30))?;
    assert_eq!(first.len(), 1);
    let second = transport.poll(at(60))?;
    assert_eq!(second.len(), 2, "nothing may be stranded behind the cursor");
    Ok(())
}

#[test]
fn seeking_backwards_lets_a_crashed_consumer_re_read() -> Result<()> {
    let mut transport = DurableLogTransport::in_memory("orders");
    transport.publish(warm_note("A", at(0), at(10))?, at(10))?;
    transport.publish(warm_note("B", at(0), at(20))?, at(20))?;
    assert_eq!(transport.poll(at(60))?.len(), 2);

    transport.seek(0, Timestamp::EPOCH);
    assert_eq!(
        transport.poll(at(60))?.len(),
        2,
        "a durable transport whose cursor could only move forward would make recovery impossible"
    );
    Ok(())
}

#[test]
fn the_pubsub_transport_reports_unavailable_and_names_everything_it_needs() -> Result<()> {
    let mut transport = PubSubTransport::declared(PubSubBinding::new(
        "projects/PROJECT/topics/qip-sense",
        "projects/PROJECT/subscriptions/qip-sense-worldmodel",
    ));

    let attempts: Vec<Error> = vec![
        transport
            .publish(warm_note("A", at(0), at(10))?, at(10))
            .expect_err("publish must not appear to succeed"),
        transport
            .publish_batch(vec![warm_note("B", at(0), at(10))?], at(10))
            .expect_err("a batch publish must not appear to succeed"),
        transport
            .poll(at(10))
            .expect_err("a pull must not appear to succeed"),
    ];

    for error in &attempts {
        // `Unavailable` specifically: a caller distinguishing "not configured
        // here" from "broken" needs the code, not the prose.
        assert!(
            matches!(error, Error::Unavailable(_)),
            "expected unavailable, got {error:?}"
        );
        let message = error.message();
        for required in [
            "GCP project",
            "qip-sense",
            "qip-sense-worldmodel",
            "gRPC",
            "TLS",
            "workload-identity",
            "roles/pubsub.publisher",
            "roles/pubsub.subscriber",
        ] {
            assert!(
                message.contains(required),
                "the error does not name {required}: {message}"
            );
        }
        assert!(
            message.contains("will not fall back"),
            "the error must say it is not silently serving something else: {message}"
        );
    }

    let descriptor = Publisher::descriptor(&transport);
    assert!(!descriptor.available);
    assert!(descriptor.production_requirement.is_some());
    assert_eq!(
        Subscriber::watermark(&transport),
        None,
        "a zero watermark would claim the subscription had been read to position zero"
    );
    assert!(!Subscriber::has_pending(&transport, at(10)));
    Ok(())
}

#[test]
fn the_local_queue_refuses_anything_that_cannot_afford_to_be_dropped() -> Result<()> {
    let mut queue = LocalQueue::new("hot", 8)?;
    let error = queue
        .publish(warm_note("A", at(0), at(10))?, at(10))
        .expect_err("a durable-class event must not enter a lossy queue");
    assert!(matches!(error, Error::Invalid(_)));
    assert!(error.message().contains("local path"), "{error}");
    assert_eq!(queue.depth(), 0);
    Ok(())
}

#[test]
fn the_local_queue_is_bounded_and_drops_the_stalest_entry_first() -> Result<()> {
    let mut queue = LocalQueue::new("hot", 2)?;
    for sequence in 1..=5 {
        queue.publish(hot_tick(sequence, at(0), at(sequence as i64))?, at(10))?;
    }
    assert_eq!(queue.depth(), 2, "the queue must not grow without limit");
    assert_eq!(queue.stats().dropped, 3, "every drop must be counted");

    let drained = queue.poll(at(60))?;
    assert_eq!(
        drained
            .iter()
            .filter_map(StreamEnvelope::sequence_number)
            .collect::<Vec<_>>(),
        vec![4, 5],
        "under overload the queue keeps the entries that are still true"
    );
    Ok(())
}

#[test]
fn a_zero_capacity_local_queue_is_refused_rather_than_silently_useless() {
    assert!(LocalQueue::new("hot", 0).is_err());
}

#[test]
fn the_local_queue_drains_only_up_to_the_callers_clock() -> Result<()> {
    let mut queue = LocalQueue::new("hot", 8)?;
    queue.publish(hot_tick(1, at(0), at(10))?, at(10))?;
    queue.publish(hot_tick(2, at(0), at(20))?, at(20))?;

    assert!(queue.poll(at(5))?.is_empty());
    assert!(
        queue.has_pending(at(10)),
        "the scheduler must be able to ask without consuming"
    );
    assert_eq!(queue.poll(at(10))?.len(), 1);
    assert_eq!(queue.poll(at(30))?.len(), 1);
    assert!(queue.poll(at(30))?.is_empty());
    Ok(())
}

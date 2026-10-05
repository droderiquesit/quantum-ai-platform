//! Archiver for durable storage of event fabric messages.
//! See ADR 0100 § 1 for this module's role.
//!
//! FABRIC-012: every sealed segment of every archive-required stream is
//! copied to the archive, **off the commit path**. The copying itself is
//! `qip_storage::segment::archive::Archiver` and the walk over partitions is
//! `Broker::archive_sealed`; what this module adds is one pass of it with
//! the numbers an operator needs recorded, for the housekeeping thread in
//! this crate's root to run on a thread of its own.
//!
//! That thread is the whole of "off the commit path". A produce is answered
//! on the connection's thread and never waits for this one: an object store
//! that has stalled holds [`pass`] inside its `put` and holds nothing else.
//! What a stalled archive does cost is stated rather than hidden —
//! `archived_through` stops advancing, so a producer on a quorum profile
//! stops being told its records are safe to release, and its spool fills.
//! That is ADR 0100 §3's producer-retained durability doing its job, and
//! [`crate::telemetry::FabricdTelemetry::archive_lag`] is the number that
//! shows it.

use std::collections::BTreeMap;
use std::sync::Mutex;

use qip_core::error::Result;
use qip_storage::segment::archive::Archiver;
use qip_streaming::event_fabric::broker::Broker;

use crate::telemetry::FabricdTelemetry;

/// How many sealed segments each partition held at the previous pass, so the
/// sealed-segments counter moves by what was sealed since and not by what
/// was already on disk when the process started.
#[derive(Debug, Default)]
pub struct SealCounts(Mutex<BTreeMap<(String, u32), u64>>);

/// One archive pass over every declared partition, recording what it found.
///
/// Returns how many segments this pass archived, or the first failure. The
/// gauges are written either way: an archive that is failing is exactly when
/// the lag it is building needs to be visible.
pub fn pass(
    broker: &Broker,
    archiver: &Archiver,
    telemetry: &FabricdTelemetry,
    seals: &SealCounts,
) -> Result<u64> {
    // Seal what has aged past its stream's cadence first, so this same pass
    // archives it; a failure to seal is reported but does not stop what is
    // already sealed from being archived.
    let sealed = broker.seal_aged();
    let outcome = broker
        .archive_sealed(archiver)
        .and_then(|archived| sealed.map(|_| archived));
    let mut unarchived = 0u64;
    let mut seen = seals.0.lock().unwrap_or_else(|e| e.into_inner());
    for (stream, partition_count) in broker.declared_streams() {
        for partition in 0..partition_count {
            let Ok(metadata) = broker.metadata(&stream, partition) else {
                continue;
            };
            telemetry.high_watermark(&stream, partition, metadata.high_watermark());
            telemetry.archived_through(&stream, partition, metadata.archived_through());
            let Ok(sealed) = broker.sealed_segment_starts(&stream, partition) else {
                continue;
            };
            unarchived += sealed
                .iter()
                .filter(|start| **start >= metadata.archived_through())
                .count() as u64;
            let now = sealed.len() as u64;
            if let Some(before) = seen.insert((stream.clone(), partition), now) {
                for _ in before..now {
                    telemetry.segments_sealed();
                }
            }
        }
    }
    drop(seen);
    telemetry.archive_lag(unarchived);
    outcome
}

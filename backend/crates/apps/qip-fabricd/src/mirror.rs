//! The cross-region mirror role (ADR 0119 §1-§3, R12-OBS-9148): one pass
//! copies committed records from a source region's broker into the target
//! region's `<source-region>.<stream>` stream, advances a checkpoint only on
//! the target's acknowledgement, and records how far behind it is.
//!
//! # What exists and what does not
//!
//! ADR 0119 §3 builds and proves the mirror on one host, against two brokers
//! standing in for two regions, and deploys it only at a second region,
//! which ADR 0035 does not authorise. So:
//!
//! * **Nothing starts this role.** `main` does not construct a [`Mirror`],
//!   and no environment variable selects one; it is a library role the
//!   tests drive. Wiring it in waits on the record that adds a region.
//! * **No stream policy names it.** `StreamPolicy` still refuses every
//!   mirroring arm but `None` ("fabric-mirror is not built", C8), and
//!   nothing here relaxes that: which streams are mirrored is the
//!   [`MirrorConfig`] the caller builds, not a policy a catalogue declares.
//! * **The checkpoint is held in memory.** ADR 0119 puts it in the target
//!   region's regional Spanner instance so a lost mirror host loses no
//!   position; here a restart begins again at offset zero, and the target
//!   broker's producer table answers each re-sent batch as a duplicate at
//!   its original offset, which [`Mirror::pass`] checks against the source
//!   offset like any other acknowledgement.
//! * **A gap is refused, not journaled.** [`plan_copy`] reports a source
//!   offset the fetch skipped; ADR 0119 journals it as a `Gap` record in the
//!   target stream, and that record is not built. A lane that meets one
//!   stops copying and keeps reporting its lag, which then grows, rather
//!   than copying past a hole nobody wrote down.
//!
//! # The lag series, and what each one measures
//!
//! `qip_fabric_mirror_lag_records{source_region,stream}` is the source's
//! committed bound minus the checkpoint's next offset, summed over the
//! stream's mirrored partitions: records the source region has acknowledged
//! and the target region does not yet hold. For a local (P2-style) source
//! the committed bound is the high watermark, the bound a fetch is already
//! limited to; the managed log's committed sequence is the bound for a P0
//! or P1 source once a managed source is wired, and is not here.
//!
//! `qip_fabric_mirror_lag_seconds{source_region,stream}` is the age, by
//! this mirror's injected clock, of the oldest committed source record not
//! yet copied — read from the logical timestamp the source broker stamped on
//! it at admission — and the largest such age over the stream's partitions.
//! Zero when every committed record is copied. It is the age of what is
//! missing, not the time since the last copy: a quiet stream with nothing
//! to copy is not behind, and a stream whose link is cut ages from the first
//! record the target lacks, whatever was copied before it. A source clock
//! ahead of the mirror's would make that age negative; that is refused for
//! the pass rather than recorded as zero, because a skew the series hid is a
//! lag nobody could see.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use qip_core::Clock;
use qip_core::error::{Error, Result};
use qip_events::event_fabric::codec::{Batch, ContentHash, DecodeOutcome, PrefixDecodeOutcome};
use qip_observability::metrics::{Labels, labels};
use qip_streaming::event_fabric::broker::Broker;
use qip_streaming::event_fabric::managed::{CommittedBatch, PartitionRef};
use qip_streaming::event_fabric::mirror::{
    MirrorCheckpoint, SourcePartition, plan_copy, region_qualified,
};

use crate::telemetry::FabricdTelemetry;

/// The largest fetch one lane makes per pass, so one lane far behind cannot
/// hold the pass on a single unbounded read.
pub const DEFAULT_FETCH_BYTES: u32 = 1 << 20;

/// The source side of the link: what a mirror reads. Implemented for an
/// in-process [`Broker`]; the mTLS client to a remote `qip-fabricd` is the
/// deployment's implementation and is not built.
pub trait MirrorSource {
    /// One past the last committed offset of `stream`'s `partition`.
    fn committed_through(&self, stream: &str, partition: u32) -> Result<u64>;

    /// Committed batches from `from` upwards, ascending, at most about
    /// `max_bytes` of them, never past [`Self::committed_through`].
    fn fetch_committed(
        &self,
        stream: &str,
        partition: u32,
        from: u64,
        max_bytes: u32,
    ) -> Result<Vec<CommittedBatch>>;
}

/// The target side of the link: where copies are produced. `Ok` is the
/// target's acknowledgement and carries the offset the copy was given.
pub trait MirrorTarget {
    fn produce_copy(&self, stream: &str, partition: u32, batch: Batch) -> Result<u64>;
}

impl MirrorSource for Broker {
    fn committed_through(&self, stream: &str, partition: u32) -> Result<u64> {
        Ok(self.metadata(stream, partition)?.high_watermark())
    }

    fn fetch_committed(
        &self,
        stream: &str,
        partition: u32,
        from: u64,
        max_bytes: u32,
    ) -> Result<Vec<CommittedBatch>> {
        let response = self.fetch(stream, partition, from, max_bytes)?;
        let buffer = qip_core::hash::from_hex(response.batches()).ok_or_else(|| {
            Error::invalid(format!(
                "{stream}:{partition}: the fetch answered batches that are not hex"
            ))
        })?;
        let mut rows = Vec::new();
        let mut rest: &[u8] = &buffer;
        while !rest.is_empty() {
            let (batch, consumed) = match Batch::decode_prefix(rest)? {
                PrefixDecodeOutcome::Complete { batch, consumed } => (batch, consumed),
                PrefixDecodeOutcome::Torn => {
                    return Err(Error::io(format!(
                        "{stream}:{partition}: the fetch from offset {from} ended inside a batch"
                    )));
                }
            };
            let (frame, tail) = rest.split_at(consumed);
            rows.push(CommittedBatch {
                partition: PartitionRef::new(stream, partition),
                offset: batch.base_offset,
                lease_epoch: batch.leader_epoch,
                batch_hash: qip_core::hash::sha256_hex(frame),
                previous_batch_hash: batch.previous_batch_hash.as_ref().and_then(hex_of),
                bytes: frame.to_vec(),
            });
            rest = tail;
        }
        Ok(rows)
    }
}

impl MirrorTarget for Broker {
    fn produce_copy(&self, stream: &str, partition: u32, mut batch: Batch) -> Result<u64> {
        // A committed source batch carries the source broker's stamps:
        // the partition it was appended to and, under a session, its region
        // and principal. The target broker refuses either from a producer
        // (ADR 0129 §1, R12-FABRIC-8084), because they are the accepting
        // broker's to write. The mirror is a producer here, so it hands the
        // records over unstamped. The source region is not lost: it is the
        // first label of the region-qualified stream the copy lands in.
        batch.partition = None;
        batch.origin = None;
        match self.produce_to(stream, partition, batch)? {
            Ok(ack) => Ok(ack.base_offset()),
            Err(refused) => Err(refused.error),
        }
    }
}

fn hex_of(hash: &ContentHash) -> Option<String> {
    match hash {
        ContentHash::Sha256(digest) => Some(qip_core::hash::to_hex(digest)),
        ContentHash::Blake3(_) => None,
    }
}

/// The regions a mirror may name and the streams it may copy: the bound on
/// both lag series' labels. The composition root that one day starts a
/// mirror builds this from the committed region list and the committed
/// catalogue's stream names, never from a value a peer sent.
#[derive(Clone, Debug)]
pub struct MirrorConfig {
    regions: BTreeSet<String>,
    streams: BTreeSet<String>,
}

impl MirrorConfig {
    /// Refuses an empty list of either, and any region that would not
    /// qualify a stream name unambiguously (see [`region_qualified`]).
    pub fn new(
        regions: impl IntoIterator<Item = String>,
        streams: impl IntoIterator<Item = String>,
    ) -> Result<Self> {
        let regions: BTreeSet<String> = regions.into_iter().collect();
        let streams: BTreeSet<String> = streams.into_iter().collect();
        if regions.is_empty() || streams.is_empty() {
            return Err(Error::invalid(
                "a mirror configuration needs at least one region and one committed stream; \
                 with neither there is nothing its lag could be labelled by",
            ));
        }
        for region in &regions {
            for stream in &streams {
                region_qualified(region, stream)?;
            }
        }
        Ok(Self { regions, streams })
    }
}

/// A `(source_region, stream)` pair the configuration admitted: the only
/// value [`FabricdTelemetry::mirror_lag`] accepts. Constructed nowhere but
/// in [`Mirror::new`], after the pair was checked against [`MirrorConfig`].
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct MirrorLabel {
    source_region: String,
    stream: String,
}

impl MirrorLabel {
    pub fn source_region(&self) -> &str {
        &self.source_region
    }

    pub fn stream(&self) -> &str {
        &self.stream
    }

    pub(crate) fn labels(&self) -> Labels {
        labels([
            ("source_region", self.source_region.as_str()),
            ("stream", self.stream.as_str()),
        ])
    }
}

#[derive(Debug)]
struct Lane {
    source: SourcePartition,
    target_stream: String,
    checkpoint: MirrorCheckpoint,
}

/// One lane's failure in a pass, kept rather than ending the pass: one
/// partition whose link is down must not stop the others' copies or hide
/// their lag.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LaneFailure {
    pub stream: String,
    pub partition: u32,
    pub message: String,
}

/// What one [`Mirror::pass`] did.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MirrorReport {
    /// Copies the target acknowledged this pass.
    pub copied: u64,
    /// Fetched rows at or below a checkpoint, dropped as already copied.
    pub duplicates: u64,
    /// Lag as recorded, per label: `(records, seconds)`. A label whose lag
    /// could not be measured on this pass is absent, and its failure is in
    /// [`Self::failures`].
    pub lag: BTreeMap<MirrorLabel, (u64, f64)>,
    pub failures: Vec<LaneFailure>,
}

/// The mirror role for one source region. See the module documentation.
#[derive(Debug)]
pub struct Mirror {
    lanes: Vec<Lane>,
    labels: BTreeMap<String, MirrorLabel>,
    clock: Arc<dyn Clock>,
    fetch_bytes: u32,
}

impl Mirror {
    /// A mirror copying `partitions` of `streams` from `source_region`.
    ///
    /// Refuses a region outside the configured list and a stream outside the
    /// committed set: either would become a label value no configuration
    /// bounds. Refuses an empty stream list and a stream with no partitions,
    /// since a mirror with no lanes would publish no lag and read as one
    /// that is never behind.
    pub fn new(
        config: &MirrorConfig,
        source_region: &str,
        streams: &BTreeMap<String, u32>,
        clock: Arc<dyn Clock>,
    ) -> Result<Self> {
        if !config.regions.contains(source_region) {
            return Err(Error::invalid(format!(
                "mirror source region `{source_region}` is not in the configured region list; \
                 add it to the committed list rather than labelling a series with it"
            )));
        }
        if streams.is_empty() {
            return Err(Error::invalid("a mirror needs at least one stream to copy"));
        }
        let mut lanes = Vec::new();
        let mut labels = BTreeMap::new();
        for (stream, partitions) in streams {
            if !config.streams.contains(stream) {
                return Err(Error::invalid(format!(
                    "stream `{stream}` is not in the mirror's committed stream set; commit it to \
                     the catalogue the set is built from rather than mirroring it by name"
                )));
            }
            if *partitions == 0 {
                return Err(Error::invalid(format!(
                    "stream `{stream}` is mirrored with zero partitions"
                )));
            }
            for partition in 0..*partitions {
                let source = SourcePartition {
                    region: source_region.to_string(),
                    stream: stream.clone(),
                    partition,
                };
                let target_stream = source.target_stream()?;
                lanes.push(Lane {
                    source,
                    target_stream,
                    checkpoint: MirrorCheckpoint::default(),
                });
            }
            labels.insert(
                stream.clone(),
                MirrorLabel {
                    source_region: source_region.to_string(),
                    stream: stream.clone(),
                },
            );
        }
        Ok(Self {
            lanes,
            labels,
            clock,
            fetch_bytes: DEFAULT_FETCH_BYTES,
        })
    }

    /// The checkpoint the target has acknowledged for `stream`'s
    /// `partition`, if this mirror copies it.
    pub fn checkpoint(&self, stream: &str, partition: u32) -> Option<&MirrorCheckpoint> {
        self.lanes
            .iter()
            .find(|lane| lane.source.stream == stream && lane.source.partition == partition)
            .map(|lane| &lane.checkpoint)
    }

    /// One pass over every lane: copy what is committed and not yet
    /// acknowledged, then measure and record the lag. Never fails as a
    /// whole; each lane's failure is in the report, and the lag of a stream
    /// whose every lane was measured is recorded whether or not its copies
    /// succeeded — a cut link is exactly when the lag needs to be seen.
    pub fn pass(
        &mut self,
        source: &dyn MirrorSource,
        target: &dyn MirrorTarget,
        telemetry: &FabricdTelemetry,
    ) -> MirrorReport {
        let mut report = MirrorReport::default();
        // Per stream: summed records and the oldest age, or `None` once any
        // of its lanes could not be measured, so a partial sum is never
        // published as the stream's lag.
        let mut lag: BTreeMap<String, Option<(u64, f64)>> = BTreeMap::new();
        let now_ns = self.clock.now().as_nanos();
        for lane in &mut self.lanes {
            if let Err(error) = copy(lane, source, target, self.fetch_bytes, &mut report) {
                report.failures.push(LaneFailure {
                    stream: lane.source.stream.clone(),
                    partition: lane.source.partition,
                    message: error.message().to_string(),
                });
            }
            let measured = measure(lane, source, now_ns);
            let entry = lag
                .entry(lane.source.stream.clone())
                .or_insert(Some((0, 0.0)));
            match measured {
                Ok((records, seconds)) => {
                    if let Some((sum, oldest)) = entry {
                        *sum += records;
                        *oldest = oldest.max(seconds);
                    }
                }
                Err(error) => {
                    *entry = None;
                    report.failures.push(LaneFailure {
                        stream: lane.source.stream.clone(),
                        partition: lane.source.partition,
                        message: error.message().to_string(),
                    });
                }
            }
        }
        for (stream, measured) in lag {
            let (Some((records, seconds)), Some(label)) = (measured, self.labels.get(&stream))
            else {
                continue;
            };
            telemetry.mirror_lag(label, records, seconds);
            report.lag.insert(label.clone(), (records, seconds));
        }
        report
    }
}

/// Copy one lane's committed, unacknowledged batches, advancing its
/// checkpoint one acknowledgement at a time so a link cut halfway keeps
/// every copy the target did take.
fn copy(
    lane: &mut Lane,
    source: &dyn MirrorSource,
    target: &dyn MirrorTarget,
    fetch_bytes: u32,
    report: &mut MirrorReport,
) -> Result<()> {
    let stream = lane.source.stream.as_str();
    let partition = lane.source.partition;
    let committed = source.committed_through(stream, partition)?;
    if committed <= lane.checkpoint.next_offset {
        return Ok(());
    }
    let rows =
        source.fetch_committed(stream, partition, lane.checkpoint.next_offset, fetch_bytes)?;
    let plan = plan_copy(&lane.source, &lane.checkpoint, committed, rows)?;
    report.duplicates += plan.duplicates;
    if let Some(gap) = plan.gaps.first() {
        return Err(Error::invalid(format!(
            "{stream}:{partition}: source offsets {}..{} were never served; the mirror journals \
             a gap as a Gap record in the target stream (ADR 0119 §1), which is not built, so it \
             copies nothing past it rather than skip it",
            gap.from_offset, gap.to_offset
        )));
    }
    for copied in plan.copies {
        let batch = match Batch::decode(&copied.bytes)? {
            DecodeOutcome::Complete(batch) => batch,
            DecodeOutcome::Torn => {
                return Err(Error::io(format!(
                    "{stream}:{partition}: offset {} is torn after planning",
                    copied.offset
                )));
            }
        };
        let acknowledged = target.produce_copy(&lane.target_stream, partition, batch)?;
        // The target offset is the source position: the region-qualified
        // stream is written by the mirror alone, one partition for one, so
        // any other offset means something else wrote there, and advancing
        // past it would checkpoint a copy the target holds somewhere else.
        if acknowledged != copied.offset {
            return Err(Error::invalid(format!(
                "{}:{partition} acknowledged source offset {} at offset {acknowledged}; only the \
                 mirror may write a region-qualified stream, so stop and find the other writer",
                lane.target_stream, copied.offset
            )));
        }
        lane.checkpoint = MirrorCheckpoint {
            next_offset: copied.offset + 1,
            last_batch_hash: Some(copied.batch_hash),
        };
        report.copied += 1;
    }
    Ok(())
}

/// `(records, seconds)` behind for one lane. See the module documentation.
fn measure(lane: &Lane, source: &dyn MirrorSource, now_ns: i64) -> Result<(u64, f64)> {
    let stream = lane.source.stream.as_str();
    let partition = lane.source.partition;
    let committed = source.committed_through(stream, partition)?;
    let next = lane.checkpoint.next_offset;
    let records = committed.checked_sub(next).ok_or_else(|| {
        Error::invalid(format!(
            "{stream}:{partition}: the source is committed through {committed} and the target \
             has acknowledged through {next}; a source that lost committed records is not a lag \
             of zero"
        ))
    })?;
    if records == 0 {
        return Ok((0, 0.0));
    }
    let oldest = source
        .fetch_committed(stream, partition, next, 1)?
        .into_iter()
        .next()
        .ok_or_else(|| {
            Error::io(format!(
                "{stream}:{partition}: offset {next} is committed and the source served nothing \
                 for it, so its age cannot be read"
            ))
        })?;
    let stamped_ns = match Batch::decode(&oldest.bytes)? {
        DecodeOutcome::Complete(batch) => batch.logical_timestamp.physical_ns,
        DecodeOutcome::Torn => {
            return Err(Error::io(format!(
                "{stream}:{partition}: offset {next} is torn"
            )));
        }
    };
    let age_ns = now_ns
        .checked_sub(stamped_ns)
        .filter(|age| *age >= 0)
        .ok_or_else(|| {
            Error::invalid(format!(
                "{stream}:{partition}: offset {next} was stamped at {stamped_ns}ns, after this \
             mirror's clock reads {now_ns}ns; a source clock ahead of the mirror's is a skew to \
             fix, not a lag of zero"
            ))
        })?;
    // Statistics, not money: nanoseconds as an f64 count of seconds, exact
    // to the microsecond for any age under about a hundred days.
    Ok((records, age_ns as f64 / 1e9))
}

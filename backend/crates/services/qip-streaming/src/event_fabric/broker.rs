//! Broker core: partitions, producer table, groups, QoS admission.
//! See ADR 0100 §1. Filled by SLICE-27 and SLICE-30.
//!
//! [`Broker`] is the library ADR 0100 §1's table names once: "a library, so
//! every guarantee gets a property test without a process" (FABRIC-061).
//! This packet gives it four guarantees:
//!
//! * **Every batch is admitted through [`super::producer::ProducerTable`]**
//!   before it ever reaches a partition's segment log, so a lost-ack retry
//!   is acknowledged without appending twice and a fenced producer is
//!   refused before it can write anything (ADR 0100 §4).
//! * **The leader epoch is bumped and persisted in [`Broker::open`]**, before
//!   the constructed value is ever handed back to a caller — there is no
//!   later moment a produce call could race ahead of it (ADR 0100 §3).
//! * **`archived_through` on every produce acknowledgement and metadata
//!   answer comes from [`super::partition::PartitionLog::archived_through`]**,
//!   which is itself read straight off the segment log's own archive marks.
//!   One source; nothing here counts archived bytes a second way.
//! * **A schema a stream has never registered is refused before the batch
//!   that names it is appended** — refused, not merely rejected after the
//!   fact, so it can never become fetchable (FABRIC-024).
//!
//! # Only the broker's own fields are stamped
//!
//! [`Broker::produce`] calls
//! [`qip_events::event_fabric::codec::stamp_broker`] and nothing else on the
//! caller's batch: the base offset, this broker's leader epoch, the logical
//! timestamp and the previous-batch hash. Every other field — the writer's
//! and the drain's — is appended exactly as the caller supplied it (this
//! packet's own constraint: "the broker sets only the header fields SLICE-06
//! assigns to the broker").

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use qip_core::Clock;
use qip_core::error::{Error, Result};
use qip_storage::segment::log::SegmentLogConfig;
use qip_storage::{DurableStore, EngineConfig, KeyValueStore};

use qip_events::event_fabric::codec::{Batch, ContentHash, LogicalTimestamp, stamp_broker};
use qip_events::event_fabric::hlc::{HlcTimestamp, PartitionClock};
use qip_events::event_fabric::schema_id::{Shape, check_compatible};

use qip_transport::event_fabric::protocol::{FetchResponse, Metadata, ProduceAck};

use super::partition::{self, PartitionLog};
use super::producer::{Admission, ProducerTable, WINDOW_SIZE};

/// The one key this broker's metadata store holds. A `DurableStore` rooted
/// at its own directory, separate from every partition's data (FABRIC-065):
/// the leader epoch is a fact about the broker process, not about any one
/// partition, and mixing the two directories would make a partition's
/// segment listing (this module's own [`partition::open_read_only`]) have to
/// know to skip a file that is not a segment.
const LEADER_EPOCH_KEY: &str = "leader_epoch";

/// Read the persisted leader epoch (or 0, for a metadata store that has
/// never held one) and persist one past it, returning the new value.
///
/// Runs once, inside [`Broker::open`], entirely before a [`Broker`] exists
/// for a caller to call [`Broker::produce`] on — which is what makes "bumped
/// and persisted before it accepts a produce" structural rather than a race
/// this function has to win.
fn next_leader_epoch(store: &DurableStore) -> Result<u64> {
    let current = match store.get(LEADER_EPOCH_KEY)? {
        Some(value) => serde_json::from_value::<u64>(value).map_err(|e| {
            Error::schema(format!(
                "stored leader epoch does not decode as an integer: {e}"
            ))
        })?,
        None => 0,
    };
    let next = current.checked_add(1).ok_or_else(|| {
        Error::numeric(
            "this broker's leader epoch has reached u64::MAX and refuses to wrap back to a \
             value an old incarnation might still recognise",
        )
    })?;
    store.put(LEADER_EPOCH_KEY, serde_json::to_value(next)?)?;
    Ok(next)
}

/// One registered schema id's current version and shape, for the
/// compatibility check [`Broker::register_schema`] runs on every re-
/// registration of the same id.
#[derive(Clone, Debug)]
struct CurrentSchema {
    version: u32,
    shape: Shape,
}

/// A stream's registered schemas: every `(schema_id, version)` a produce may
/// declare, plus each schema id's current version for evolution checking.
#[derive(Debug, Default)]
struct StreamSchemas {
    registered: BTreeSet<(u32, u32)>,
    current: BTreeMap<u32, CurrentSchema>,
}

/// A stream's fixed shape: how many partitions it has, and the segment log
/// configuration every one of its partitions opens with.
#[derive(Clone, Debug)]
struct DeclaredStream {
    partition_count: u32,
    config: SegmentLogConfig,
}

/// The produce-time state one partition's dense stream needs beyond what
/// [`PartitionLog`] itself holds: the logical clock this broker has ticked
/// for it, and the hash of the last batch this broker appended to it, so the
/// next batch's `previous_batch_hash` chains to the truth rather than to
/// `None` every time.
#[derive(Debug)]
struct ProduceCursor {
    clock: PartitionClock,
    last_batch_hash: Option<ContentHash>,
}

#[derive(Debug)]
struct PartitionState {
    log: PartitionLog,
    cursor: Mutex<ProduceCursor>,
}

/// Recover the hash a fresh [`ProduceCursor`] should chain its first new
/// batch to: the hash of whatever batch this partition's segment log last
/// durably holds, re-derived by re-encoding it (`Batch::encode` is a pure
/// function of the fields `PartitionLog::read` just returned, so this
/// reproduces the exact bytes that batch was originally appended as)
/// rather than trusted from anywhere this broker itself remembered — a
/// restarted broker has forgotten everything it once held in memory, and the
/// segment log's own last batch is the only fact left to recover it from.
fn recover_last_batch_hash(log: &PartitionLog) -> Result<Option<ContentHash>> {
    let high_water = log.high_water();
    if high_water == 0 {
        return Ok(None);
    }
    let last_offset = high_water - 1;
    let last = log.read(last_offset)?.ok_or_else(|| {
        Error::io(format!(
            "the partition reports a high watermark of {high_water} but offset {last_offset} \
             does not read back; recovery and the watermark have disagreed"
        ))
    })?;
    let encoded = last.encode()?;
    Ok(Some(ContentHash::sha256_of(&encoded)))
}

/// A deterministic fingerprint of a batch's writer- and drain-owned content:
/// the schema declaration and every record's identity and payload, in order.
///
/// Deliberately excludes the broker-owned fields (`base_offset`,
/// `leader_epoch`, `logical_timestamp`, `previous_batch_hash`): at the point
/// this is computed the caller's batch has not been stamped with any of
/// them yet, and a genuine retry of the *same* logical batch must fingerprint
/// identically to its first attempt regardless of which offset either
/// attempt is eventually assigned.
fn payload_fingerprint(batch: &Batch) -> ContentHash {
    let mut material = Vec::new();
    material.extend_from_slice(&batch.schema_id.to_le_bytes());
    material.extend_from_slice(&batch.schema_version.to_le_bytes());
    for record in &batch.records {
        material.extend_from_slice(&(record.event_id.len() as u64).to_le_bytes());
        material.extend_from_slice(record.event_id.as_bytes());
        material.extend_from_slice(&(record.payload.len() as u64).to_le_bytes());
        material.extend_from_slice(&record.payload);
    }
    ContentHash::sha256_of(&material)
}

/// Convert a partition clock's reading to the wire's plain-integer form,
/// refusing rather than truncating a logical counter that has grown past
/// what a `u32` can carry — see [`HlcTimestamp`]'s own counter, which is a
/// `u64` for exactly this reason.
fn to_logical_timestamp(reading: HlcTimestamp) -> Result<LogicalTimestamp> {
    let logical = u32::try_from(reading.logical).map_err(|_| {
        Error::numeric(format!(
            "this partition's logical clock counter ({}) no longer fits the wire's u32 field",
            reading.logical
        ))
    })?;
    Ok(LogicalTimestamp {
        physical_ns: reading.physical.as_nanos(),
        logical,
    })
}

fn partition_label(stream: &str, partition: u32) -> String {
    format!("{stream}:{partition}")
}

/// A bounded, per-`(producer_id, partition label)` window of recently
/// assigned offsets, keyed the same way as
/// [`super::producer::ProducerTable`]'s own internal table. Named as a type
/// alias purely to keep [`Broker`]'s own field declaration readable — the
/// shape itself is documented on [`Broker::recent_offsets`].
type RecentOffsets = BTreeMap<(String, String), VecDeque<(u64, u64)>>;

/// The event-fabric broker library: partitions over segment logs, a static
/// leader epoch, and schema admission. See the module documentation.
#[derive(Debug)]
pub struct Broker {
    data_dir: PathBuf,
    clock: Arc<dyn Clock>,
    leader_epoch: u64,
    declared: Mutex<BTreeMap<String, DeclaredStream>>,
    partitions: Mutex<BTreeMap<(String, u32), Arc<PartitionState>>>,
    producers: Mutex<ProducerTable>,
    schemas: Mutex<BTreeMap<String, StreamSchemas>>,
    /// A bounded window (capped at [`WINDOW_SIZE`], mirroring
    /// [`super::producer::ProducerTable`]'s own bound) of the offset each of
    /// a producer's recent sequences was assigned, so a verified
    /// [`Admission::Duplicate`] can be acknowledged with the same offset its
    /// first attempt received rather than a fabricated one.
    recent_offsets: Mutex<RecentOffsets>,
}

impl Broker {
    /// Open (or create) a broker rooted at `data_dir`, bumping and
    /// persisting its leader epoch before returning — see the module
    /// documentation's second guarantee.
    pub fn open(data_dir: impl Into<PathBuf>, clock: Arc<dyn Clock>) -> Result<Self> {
        let data_dir = data_dir.into();
        std::fs::create_dir_all(&data_dir)?;
        let metadata_dir = data_dir.join("metadata");
        let epoch_store = DurableStore::open(&metadata_dir, EngineConfig::new(clock.clone()))?;
        let leader_epoch = next_leader_epoch(&epoch_store)?;
        Ok(Self {
            data_dir,
            clock,
            leader_epoch,
            declared: Mutex::new(BTreeMap::new()),
            partitions: Mutex::new(BTreeMap::new()),
            producers: Mutex::new(ProducerTable::new()),
            schemas: Mutex::new(BTreeMap::new()),
            recent_offsets: Mutex::new(BTreeMap::new()),
        })
    }

    /// This broker's leader epoch: bumped and persisted once, in
    /// [`Broker::open`], for the lifetime of this instance.
    pub fn leader_epoch(&self) -> u64 {
        self.leader_epoch
    }

    /// Fix a stream's partition count and segment log configuration.
    ///
    /// Refuses a partition count of zero (there is nowhere for a key to
    /// route to) and refuses redeclaring an already-declared stream with a
    /// *different* count: silently changing it would silently change which
    /// partition every existing key resolves to, which is exactly the
    /// "no API merges partitions" constraint this packet is built to hold.
    /// Redeclaring with the same count (as a restarted process's own
    /// caller does, since this fact is not persisted — see the module
    /// documentation) is idempotent.
    pub fn declare_stream(
        &self,
        stream: &str,
        partition_count: u32,
        config: SegmentLogConfig,
    ) -> Result<()> {
        if partition_count == 0 {
            return Err(Error::invalid(
                "a declared stream must have at least one partition",
            ));
        }
        let mut declared = self.declared.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(existing) = declared.get(stream)
            && existing.partition_count != partition_count
        {
            return Err(Error::denied(format!(
                "stream '{stream}' is already declared with {} partitions; {partition_count} \
                 would silently change which partition every existing key resolves to",
                existing.partition_count
            )));
        }
        declared.insert(
            stream.to_string(),
            DeclaredStream {
                partition_count,
                config,
            },
        );
        Ok(())
    }

    /// Register a schema id's version and shape for `stream`.
    ///
    /// The first registration of a schema id is always accepted. A later
    /// registration of the same id is checked with
    /// [`check_compatible`] against the id's current version and shape,
    /// refusing a breaking change that did not bump the version
    /// (FABRIC-025) — this is the one place this broker uses that check,
    /// because it is the one place a shape is actually known: a produce
    /// only ever declares a numeric `(schema_id, schema_version)`, never a
    /// shape, so there is nothing to compare a payload against at produce
    /// time.
    pub fn register_schema(
        &self,
        stream: &str,
        schema_id: u32,
        schema_version: u32,
        shape: Shape,
    ) -> Result<()> {
        let mut schemas = self.schemas.lock().unwrap_or_else(|e| e.into_inner());
        let entry = schemas.entry(stream.to_string()).or_default();
        if let Some(current) = entry.current.get(&schema_id) {
            check_compatible(current.version, &current.shape, schema_version, &shape)?;
            if schema_version > current.version {
                entry.current.insert(
                    schema_id,
                    CurrentSchema {
                        version: schema_version,
                        shape,
                    },
                );
            }
        } else {
            entry.current.insert(
                schema_id,
                CurrentSchema {
                    version: schema_version,
                    shape,
                },
            );
        }
        entry.registered.insert((schema_id, schema_version));
        Ok(())
    }

    /// Produce `batch` (already stamped by the drain — see
    /// [`qip_events::event_fabric::codec::stamp_drain`]) under `key`, which
    /// [`partition::partition_for`] routes to one of `stream`'s declared
    /// partitions.
    ///
    /// Refuses, before anything is appended:
    /// * a stream that was never declared;
    /// * a `(schema_id, schema_version)` `stream` has never registered
    ///   (FABRIC-024) — refused here means it can never become fetchable;
    /// * whatever [`super::producer::ProducerTable::admit`] refuses:
    ///   a fenced epoch, a sequence conflict, a hole in the dense stream, or
    ///   a retry too old for this table's bounded window to verify.
    ///
    /// A verified [`Admission::Duplicate`] is acknowledged with the offset
    /// its original attempt received, without appending again.
    pub fn produce(&self, stream: &str, key: &str, mut batch: Batch) -> Result<ProduceAck> {
        let partition_count = self.partition_count(stream)?;
        let partition = partition::partition_for(key, partition_count)?;
        self.require_registered_schema(stream, batch.schema_id, batch.schema_version)?;

        let record_count = u64::try_from(batch.records.len()).map_err(|_| {
            Error::invalid("a batch carries more records than a u64 count can represent")
        })?;
        let payload_hash = payload_fingerprint(&batch);
        let label = partition_label(stream, partition);

        let admission = {
            let mut producers = self.producers.lock().unwrap_or_else(|e| e.into_inner());
            producers.admit(
                &batch.producer_id,
                &label,
                batch.producer_epoch,
                batch.base_sequence,
                record_count,
                payload_hash,
            )?
        };

        let state = self.partition_state(stream, partition)?;

        match admission {
            Admission::Appended { .. } => {
                let base_offset = {
                    let mut cursor = state.cursor.lock().unwrap_or_else(|e| e.into_inner());
                    // Predicted while holding this partition's own produce
                    // lock, which is the only thing serialising calls to
                    // `PartitionLog::append` for this partition: as long as
                    // nothing else ever calls it, the offset `high_water`
                    // names here is exactly the offset `append` assigns
                    // below.
                    let predicted_offset = state.log.high_water();
                    let now = self.clock.now();
                    let logical = cursor.clock.tick(now)?;
                    let logical_timestamp = to_logical_timestamp(logical)?;
                    let previous_hash = cursor.last_batch_hash.clone();
                    stamp_broker(
                        &mut batch,
                        predicted_offset,
                        self.leader_epoch,
                        logical_timestamp,
                        previous_hash,
                    );
                    let encoded = batch.encode()?;
                    let appended_offset = state.log.append(&batch)?;
                    if appended_offset != predicted_offset {
                        return Err(Error::io(format!(
                            "partition {label} assigned offset {appended_offset} but this broker \
                             predicted {predicted_offset} while holding the partition's own \
                             produce lock; something else is appending to this partition"
                        )));
                    }
                    cursor.last_batch_hash = Some(ContentHash::sha256_of(&encoded));
                    appended_offset
                };
                self.remember_offset(&batch.producer_id, &label, batch.base_sequence, base_offset);
                let high_watermark = state.log.high_water();
                let archived_through = state.log.archived_through()?;
                ProduceAck::new(
                    stream,
                    partition,
                    base_offset,
                    high_watermark,
                    archived_through,
                )
            }
            Admission::Duplicate => {
                let base_offset = self
                    .recall_offset(&batch.producer_id, &label, batch.base_sequence)
                    .ok_or_else(|| {
                        Error::io(format!(
                            "producer '{}' sequence {} at {label} was verified as a duplicate, \
                             but this broker holds no record of the offset its first attempt \
                             received",
                            batch.producer_id, batch.base_sequence
                        ))
                    })?;
                let high_watermark = state.log.high_water();
                let archived_through = state.log.archived_through()?;
                ProduceAck::new(
                    stream,
                    partition,
                    base_offset,
                    high_watermark,
                    archived_through,
                )
            }
            Admission::Conflict => Err(Error::denied(format!(
                "producer '{}' sequence {} at {label} conflicts with a batch this broker already \
                 accepted under the same epoch and sequence but a different payload",
                batch.producer_id, batch.base_sequence
            ))),
            Admission::FencedEpoch { current_epoch } => Err(Error::denied(format!(
                "producer '{}' epoch {} at {label} is fenced by epoch {current_epoch}",
                batch.producer_id, batch.producer_epoch
            ))),
            Admission::OutsideWindow => Err(Error::denied(format!(
                "producer '{}' sequence {} at {label} is behind this broker's bounded dedup \
                 window and cannot be verified as a duplicate",
                batch.producer_id, batch.base_sequence
            ))),
            Admission::OutOfOrder { expected } => Err(Error::invalid(format!(
                "producer '{}' sequence {} at {label} is ahead of the dense stream; the next \
                 sequence this partition accepts is {expected}",
                batch.producer_id, batch.base_sequence
            ))),
        }
    }

    /// Fetch batches from `(stream, partition)` starting at `offset`,
    /// accumulating up to `max_bytes` of encoded batch bytes (always at
    /// least one batch, so a consumer facing one oversized batch can still
    /// make progress).
    ///
    /// Never serves an offset at or past [`PartitionLog::high_water`] — the
    /// loop below is bounded by it, and every offset it does read has
    /// already passed through [`PartitionLog::read`], which is itself
    /// bounded by it a second time (SLICE-16).
    pub fn fetch(
        &self,
        stream: &str,
        partition: u32,
        offset: u64,
        max_bytes: u32,
    ) -> Result<FetchResponse> {
        let state = self.partition_state(stream, partition)?;
        let high_watermark = state.log.high_water();
        let archived_through = state.log.archived_through()?;
        let max_bytes = max_bytes as usize;

        let mut encoded = Vec::new();
        let mut cursor = offset;
        while cursor < high_watermark {
            let Some(batch) = state.log.read(cursor)? else {
                break;
            };
            let bytes = batch.encode()?;
            if !encoded.is_empty() && encoded.len() + bytes.len() > max_bytes {
                break;
            }
            encoded.extend_from_slice(&bytes);
            cursor += 1;
            if encoded.len() >= max_bytes {
                break;
            }
        }

        let hex = qip_core::hash::to_hex(&encoded);
        FetchResponse::new(stream, partition, high_watermark, archived_through, hex)
    }

    /// This partition's metadata: leader epoch, high watermark and
    /// `archived_through`, the same release signal [`Self::produce`]'s
    /// acknowledgement carries (ADR 0100 §3).
    pub fn metadata(&self, stream: &str, partition: u32) -> Result<Metadata> {
        let state = self.partition_state(stream, partition)?;
        let high_watermark = state.log.high_water();
        let archived_through = state.log.archived_through()?;
        Metadata::new(
            stream,
            partition,
            self.leader_epoch,
            high_watermark,
            archived_through,
        )
    }

    /// Record that the sealed segment starting at `start_offset` of
    /// `(stream, partition)` has been archived. In production the archiver
    /// (SLICE-21 through SLICE-38) calls this once its upload is durable;
    /// tests call it directly (this packet's own constraint).
    pub fn mark_archived(&self, stream: &str, partition: u32, start_offset: u64) -> Result<()> {
        let state = self.partition_state(stream, partition)?;
        state.log.mark_archived(start_offset)
    }

    /// Every sealed segment's start offset for `(stream, partition)`,
    /// ascending. See [`PartitionLog::sealed_segment_starts`].
    pub fn sealed_segment_starts(&self, stream: &str, partition: u32) -> Result<Vec<u64>> {
        let state = self.partition_state(stream, partition)?;
        Ok(state.log.sealed_segment_starts())
    }

    fn partition_count(&self, stream: &str) -> Result<u32> {
        let declared = self.declared.lock().unwrap_or_else(|e| e.into_inner());
        declared
            .get(stream)
            .map(|d| d.partition_count)
            .ok_or_else(|| {
                Error::invalid(format!(
                    "stream '{stream}' was never declared; call Broker::declare_stream before \
                     producing to or fetching from it"
                ))
            })
    }

    fn require_registered_schema(
        &self,
        stream: &str,
        schema_id: u32,
        schema_version: u32,
    ) -> Result<()> {
        let schemas = self.schemas.lock().unwrap_or_else(|e| e.into_inner());
        let ok = schemas
            .get(stream)
            .is_some_and(|s| s.registered.contains(&(schema_id, schema_version)));
        if ok {
            return Ok(());
        }
        Err(Error::denied(format!(
            "stream '{stream}' has no registered schema id {schema_id} version {schema_version}; \
             refusing to produce it (FABRIC-024) rather than let it become fetchable"
        )))
    }

    fn partition_state(&self, stream: &str, partition: u32) -> Result<Arc<PartitionState>> {
        let key = (stream.to_string(), partition);
        {
            let partitions = self.partitions.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(state) = partitions.get(&key) {
                return Ok(state.clone());
            }
        }
        let config = {
            let declared = self.declared.lock().unwrap_or_else(|e| e.into_inner());
            declared
                .get(stream)
                .map(|d| d.config.clone())
                .ok_or_else(|| {
                    Error::invalid(format!(
                        "stream '{stream}' was never declared; call Broker::declare_stream first"
                    ))
                })?
        };
        let log = PartitionLog::open(&self.data_dir, stream, partition, config)?;
        let last_batch_hash = recover_last_batch_hash(&log)?;
        let state = Arc::new(PartitionState {
            log,
            cursor: Mutex::new(ProduceCursor {
                clock: PartitionClock::new(self.clock.now()),
                last_batch_hash,
            }),
        });
        let mut partitions = self.partitions.lock().unwrap_or_else(|e| e.into_inner());
        // Another call may have opened the same partition first while this
        // one built `state` outside the lock; keep whichever won rather than
        // hold two `SegmentLog`s open on the same directory, which
        // `SegmentLog::open`'s own guard would refuse for the second one
        // anyway.
        let state = partitions.entry(key).or_insert(state).clone();
        Ok(state)
    }

    fn remember_offset(
        &self,
        producer_id: &str,
        label: &str,
        base_sequence: u64,
        base_offset: u64,
    ) {
        let mut recent = self
            .recent_offsets
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let window = recent
            .entry((producer_id.to_string(), label.to_string()))
            .or_default();
        window.push_back((base_sequence, base_offset));
        while window.len() > WINDOW_SIZE {
            window.pop_front();
        }
    }

    fn recall_offset(&self, producer_id: &str, label: &str, base_sequence: u64) -> Option<u64> {
        let recent = self
            .recent_offsets
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        recent
            .get(&(producer_id.to_string(), label.to_string()))?
            .iter()
            .find(|(sequence, _)| *sequence == base_sequence)
            .map(|(_, offset)| *offset)
    }
}

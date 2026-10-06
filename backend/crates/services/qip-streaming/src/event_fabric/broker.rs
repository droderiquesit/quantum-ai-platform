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
use qip_storage::segment::archive::Archiver;
use qip_storage::segment::log::SegmentLogConfig;
use qip_storage::{DurableStore, EngineConfig, KeyValueStore};
use serde::{Deserialize, Serialize};

use qip_events::event_fabric::codec::{Batch, ContentHash, LogicalTimestamp, stamp_broker};
use qip_events::event_fabric::hlc::{HlcTimestamp, PartitionClock};
use qip_events::event_fabric::policy::{OverloadPolicy, StreamPolicy};
use qip_events::event_fabric::schema_id::{Shape, check_compatible};

use qip_transport::event_fabric::protocol::{FetchResponse, Metadata, ProduceAck, Refusal};

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
    policy: StreamPolicy,
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
    /// When the active segment took its first batch, or `None` while it is
    /// empty: what [`Broker::seal_aged`] measures a stream's seal age from.
    active_since: Option<qip_core::Timestamp>,
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

fn checkpoint_key(group: &str, stream: &str, partition: u32) -> Result<String> {
    if group.trim().is_empty() || group.contains('|') || stream.contains('|') {
        return Err(Error::invalid(
            "a consumer group and stream name must be non-empty and must not contain '|', which \
             separates the parts of a checkpoint key",
        ));
    }
    Ok(format!("{group}|{stream}|{partition}"))
}

/// The metadata key holding one partition's isolation, while it is isolated.
/// `|` cannot appear in a declared stream name's checkpoint key either, so
/// the three parts cannot be confused with one another.
fn isolation_key(stream: &str, partition: u32) -> String {
    format!("isolated|{stream}|{partition}")
}

/// The metadata prefix every operator action is recorded under.
const ADMIN_LOG_PREFIX: &str = "admin|";

/// A partition an operator has parked (FABRIC-028): who, why, and the high
/// watermark it was parked at. Held in the broker's metadata store, so a
/// restart does not quietly lift an isolation somebody imposed on purpose.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Isolation {
    pub operator: String,
    pub reason: String,
    pub at_offset: u64,
}

/// One operator action on a partition, as [`Broker::admin_log`] reads it
/// back: an isolation or a release, attributed and in the order taken.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminAction {
    pub action: String,
    pub stream: String,
    pub partition: u32,
    pub operator: String,
    pub reason: String,
    pub at_offset: u64,
    pub at_ns: i64,
}

/// A produce this broker turned down for a reason the wire protocol names:
/// the [`Refusal`] a producer acts on, beside the message [`Broker::produce`]
/// has always answered with. The protocol handler sends the first; a caller
/// inside the process reads the second.
#[derive(Debug)]
pub struct ProduceRefusal {
    pub refusal: Refusal,
    pub error: Error,
}

impl ProduceRefusal {
    fn new(refusal: Refusal, error: Error) -> Self {
        Self { refusal, error }
    }
}

/// What [`Broker::produce_to`] answers when nothing failed: the
/// acknowledgement, or the named refusal.
pub type Produced = std::result::Result<ProduceAck, ProduceRefusal>;

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
    /// Every consumer group's committed offset per `(stream, partition)`,
    /// in a store of its own so a restart serves the checkpoint back
    /// (FABRIC-016). Separate from `metadata/` for the reason that
    /// directory's own constant gives: the leader epoch is a fact about the
    /// process, a checkpoint is a fact about a consumer.
    checkpoints: DurableStore,
    /// The broker's own facts: the leader epoch, every partition isolation
    /// in force and the log of operator actions (FABRIC-028). Kept open for
    /// the broker's lifetime so an isolation is durable before the call that
    /// imposed it returns.
    metadata: DurableStore,
    /// What each `(stream, producer)` has spent in the current
    /// [`QUOTA_WINDOW_NS`] window: `(window_start_ns, bytes, messages)`.
    /// Entries from an older window are dropped whenever a new window
    /// opens, so the map is bounded by the producers active in one window.
    quota_spent: Mutex<QuotaSpend>,
}

/// `(stream, producer)` to `(window_start_ns, bytes, messages)`.
type QuotaSpend = BTreeMap<(String, String), (i64, u64, u64)>;

/// The period a stream's per-producer byte and message quotas are measured
/// over: one second, because `peak_bytes_per_second` is the unit the same
/// policy sizes its stream in (FABRIC-049).
const QUOTA_WINDOW_NS: i64 = 1_000_000_000;

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
        let checkpoints =
            DurableStore::open(data_dir.join("consumers"), EngineConfig::new(clock.clone()))?;
        Ok(Self {
            data_dir,
            clock,
            leader_epoch,
            declared: Mutex::new(BTreeMap::new()),
            partitions: Mutex::new(BTreeMap::new()),
            producers: Mutex::new(ProducerTable::new()),
            schemas: Mutex::new(BTreeMap::new()),
            recent_offsets: Mutex::new(BTreeMap::new()),
            checkpoints,
            metadata: epoch_store,
            quota_spent: Mutex::new(BTreeMap::new()),
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
    ///
    /// Takes the stream's whole [`StreamPolicy`] (FABRIC-057): class,
    /// partitioning key, ordering, retention, replication, overload,
    /// mirroring, acknowledgement floor, quotas and lag limit. `StreamPolicy`
    /// has no `Default` and one validating constructor, so a stream cannot be
    /// declared here with any of them left to an unspecified default — the
    /// call does not type-check. Redeclaring a stream under a *different*
    /// policy is refused for the same reason a different partition count is:
    /// it would change what the stream's existing records were promised.
    pub fn declare_stream(
        &self,
        stream: &str,
        partition_count: u32,
        policy: StreamPolicy,
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
        if let Some(existing) = declared.get(stream)
            && existing.policy != policy
        {
            return Err(Error::denied(format!(
                "stream '{stream}' is already declared under a different stream policy; \
                 redeclaring it would change what its existing records were promised"
            )));
        }
        declared.insert(
            stream.to_string(),
            DeclaredStream {
                partition_count,
                policy,
                config,
            },
        );
        Ok(())
    }

    /// The policy `stream` was declared under, exactly as declared.
    pub fn stream_policy(&self, stream: &str) -> Result<StreamPolicy> {
        let declared = self.declared.lock().unwrap_or_else(|e| e.into_inner());
        declared
            .get(stream)
            .map(|d| d.policy.clone())
            .ok_or_else(|| {
                Error::invalid(format!(
                    "stream '{stream}' was never declared; call Broker::declare_stream first"
                ))
            })
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
    pub fn produce(&self, stream: &str, key: &str, batch: Batch) -> Result<ProduceAck> {
        let partition_count = self.partition_count(stream)?;
        let partition = partition::partition_for(key, partition_count)?;
        self.produce_to(stream, partition, batch)?
            .map_err(|refused| refused.error)
    }

    /// Produce `batch` to a partition the caller names, answering a refusal
    /// as the [`Refusal`] the wire protocol carries.
    ///
    /// This is [`Self::produce`]'s whole body; that method only routes a key
    /// here and flattens the answer. It exists because the protocol's
    /// `produce` route names a partition, not a key, and a producer acts on
    /// which refusal it got: `OutOfOrderSequence` carries the sequence to
    /// resend from, `Quota` how long to wait, `Fenced` that it must stop. A
    /// handler that only had [`Self::produce`]'s prose could tell a producer
    /// nothing but "no".
    ///
    /// Refused before anything else is read: a partition the stream does not
    /// have, and a partition an operator has isolated (FABRIC-028). A
    /// consumer group past the stream's lag limit stays an `Err` rather than
    /// a [`Refusal`]: the protocol's closed set has no code for it, and a
    /// transport-level failure is what makes the producer back off and try
    /// again once the group has caught up.
    pub fn produce_to(&self, stream: &str, partition: u32, mut batch: Batch) -> Result<Produced> {
        let partition_count = self.partition_count(stream)?;
        if partition >= partition_count {
            return Err(Error::invalid(format!(
                "stream '{stream}' has partitions 0 to {}; partition {partition} does not \
                 exist, so route the key with partition_for before producing",
                partition_count - 1
            )));
        }
        if let Some(isolation) = self.isolation(stream, partition)? {
            let error = Error::denied(format!(
                "{stream}:{partition} is isolated by '{}' ({}); produce resumes when an \
                 operator releases it",
                isolation.operator, isolation.reason
            ));
            return Ok(Err(ProduceRefusal::new(
                Refusal::Isolated {
                    operator: isolation.operator,
                    reason: isolation.reason,
                },
                error,
            )));
        }
        if let Err(error) =
            self.require_registered_schema(stream, batch.schema_id, batch.schema_version)
        {
            return Ok(Err(ProduceRefusal::new(Refusal::SchemaRefused, error)));
        }
        self.refuse_when_a_group_lags(stream, partition)?;
        if let Err(refused) = self.charge_quota(stream, &batch)? {
            return Ok(Err(refused));
        }

        let record_count = u64::try_from(batch.records.len()).map_err(|_| {
            Error::invalid("a batch carries more records than a u64 count can represent")
        })?;
        let payload_hash = payload_fingerprint(&batch);
        let label = partition_label(stream, partition);

        // Opened before admission: opening replays the log into the producer
        // table, and a retry admitted against an empty table is appended twice.
        let state = self.partition_state(stream, partition)?;
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
                    // The append may itself have sealed the segment by size,
                    // in which case the active one is empty again and has no
                    // age to measure.
                    cursor.active_since = if state.log.active_batches() == 0 {
                        None
                    } else {
                        cursor.active_since.or(Some(now))
                    };
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
                .map(Ok)
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
                .map(Ok)
            }
            Admission::Conflict => Ok(Err(ProduceRefusal::new(
                Refusal::SequenceConflict,
                Error::denied(format!(
                    "producer '{}' sequence {} at {label} conflicts with a batch this broker \
                     already accepted under the same epoch and sequence but a different payload",
                    batch.producer_id, batch.base_sequence
                )),
            ))),
            Admission::FencedEpoch { current_epoch } => Ok(Err(ProduceRefusal::new(
                Refusal::Fenced,
                Error::denied(format!(
                    "producer '{}' epoch {} at {label} is fenced by epoch {current_epoch}",
                    batch.producer_id, batch.producer_epoch
                )),
            ))),
            Admission::OutsideWindow => Ok(Err(ProduceRefusal::new(
                Refusal::SequenceBelowWindow,
                Error::denied(format!(
                    "producer '{}' sequence {} at {label} is behind this broker's bounded dedup \
                     window and cannot be verified as a duplicate",
                    batch.producer_id, batch.base_sequence
                )),
            ))),
            Admission::OutOfOrder { expected } => Ok(Err(ProduceRefusal::new(
                Refusal::OutOfOrderSequence { expected },
                Error::invalid(format!(
                    "producer '{}' sequence {} at {label} is ahead of the dense stream; the next \
                     sequence this partition accepts is {expected}",
                    batch.producer_id, batch.base_sequence
                )),
            ))),
        }
    }

    /// The sequence `producer_id`'s dense stream at `(stream, partition)`
    /// continues at: one past the last record this broker appended for it,
    /// or zero if it has appended none. The answer a restarted producer
    /// needs to carry its sequence across epochs (ADR 0100 §4), read from
    /// the same table that will judge its next batch.
    pub fn next_sequence(&self, stream: &str, partition: u32, producer_id: &str) -> Result<u64> {
        // Opening the partition replays its log into the producer table, so
        // the answer after a restart is what the log holds, not zero.
        self.partition_state(stream, partition)?;
        let label = partition_label(stream, partition);
        let producers = self.producers.lock().unwrap_or_else(|e| e.into_inner());
        match producers.last_sequence(producer_id, &label) {
            Some(last) => last.checked_add(1).ok_or_else(|| {
                Error::numeric(format!(
                    "producer '{producer_id}' at {label} has used every sequence a u64 can \
                     carry; retire it and issue a new producer id"
                ))
            }),
            None => Ok(0),
        }
    }

    /// FABRIC-028: park `(stream, partition)`. Every produce to it is
    /// refused with [`Refusal::Isolated`] naming `operator` and `reason`
    /// until [`Self::release`]; nothing is deleted and fetches keep working,
    /// so the retained records stay readable while the partition is parked.
    /// Durable before it returns and recorded in [`Self::admin_log`].
    ///
    /// Refuses an empty operator or reason (an isolation nobody can
    /// attribute or explain is the incident, not the response to one) and a
    /// partition that is already isolated, naming who holds it: a second
    /// isolation would overwrite the first one's reason.
    pub fn isolate(
        &self,
        stream: &str,
        partition: u32,
        operator: &str,
        reason: &str,
    ) -> Result<u64> {
        if operator.trim().is_empty() || reason.trim().is_empty() {
            return Err(Error::invalid(
                "an isolation must name the operator imposing it and the reason; state both",
            ));
        }
        let state = self.partition_state(stream, partition)?;
        if let Some(held) = self.isolation(stream, partition)? {
            return Err(Error::denied(format!(
                "{stream}:{partition} is already isolated by '{}' ({}); release it before \
                 isolating it again",
                held.operator, held.reason
            )));
        }
        let at_offset = state.log.high_water();
        let isolation = Isolation {
            operator: operator.to_string(),
            reason: reason.to_string(),
            at_offset,
        };
        self.record_admin("isolate", stream, partition, operator, reason, at_offset)?;
        self.metadata.put(
            &isolation_key(stream, partition),
            serde_json::to_value(&isolation)?,
        )?;
        Ok(at_offset)
    }

    /// Lift an isolation, recording who lifted it. Refuses a partition that
    /// is not isolated rather than reporting a release that changed nothing.
    pub fn release(&self, stream: &str, partition: u32, operator: &str) -> Result<u64> {
        if operator.trim().is_empty() {
            return Err(Error::invalid(
                "a release must name the operator lifting the isolation",
            ));
        }
        let state = self.partition_state(stream, partition)?;
        let Some(held) = self.isolation(stream, partition)? else {
            return Err(Error::invalid(format!(
                "{stream}:{partition} is not isolated; there is nothing to release"
            )));
        };
        let at_offset = state.log.high_water();
        self.record_admin(
            "release",
            stream,
            partition,
            operator,
            &held.reason,
            at_offset,
        )?;
        self.metadata.delete(&isolation_key(stream, partition))?;
        Ok(at_offset)
    }

    /// The isolation in force on `(stream, partition)`, if any.
    pub fn isolation(&self, stream: &str, partition: u32) -> Result<Option<Isolation>> {
        match self.metadata.get(&isolation_key(stream, partition))? {
            Some(value) => serde_json::from_value(value).map(Some).map_err(|e| {
                Error::schema(format!(
                    "the stored isolation of {stream}:{partition} does not decode: {e}"
                ))
            }),
            None => Ok(None),
        }
    }

    /// Every operator action this broker has recorded, oldest first.
    pub fn admin_log(&self) -> Result<Vec<AdminAction>> {
        let mut actions = Vec::new();
        for key in self.metadata.keys_with_prefix(ADMIN_LOG_PREFIX)? {
            let Some(value) = self.metadata.get(&key)? else {
                continue;
            };
            actions.push(serde_json::from_value(value).map_err(|e| {
                Error::schema(format!(
                    "the stored operator action {key} does not decode: {e}"
                ))
            })?);
        }
        Ok(actions)
    }

    /// Append one operator action. Written before the state change it
    /// describes, so after a crash between the two writes an isolation can
    /// be in the log without being in force, and never the reverse: an
    /// isolation in force that no record attributes.
    fn record_admin(
        &self,
        action: &str,
        stream: &str,
        partition: u32,
        operator: &str,
        reason: &str,
        at_offset: u64,
    ) -> Result<()> {
        let index = self.metadata.keys_with_prefix(ADMIN_LOG_PREFIX)?.len();
        let entry = AdminAction {
            action: action.to_string(),
            stream: stream.to_string(),
            partition,
            operator: operator.to_string(),
            reason: reason.to_string(),
            at_offset,
            at_ns: self.clock.now().as_nanos(),
        };
        // Zero-padded so the store's key order is the order taken.
        self.metadata.put(
            &format!("{ADMIN_LOG_PREFIX}{index:020}"),
            serde_json::to_value(&entry)?,
        )
    }

    /// Seal every open partition's active segment that has held a batch for
    /// at least its stream's declared `seal_age_ms`, returning how many were
    /// sealed.
    ///
    /// The segment log seals by size alone. Without this a batch on a quiet
    /// partition is never sealed, so never archived, so `archived_through`
    /// never passes it, and a P0 or P1 producer — which the SDK holds to a
    /// quorum acknowledgement — waits on a grant or a fill that is durable
    /// on the broker's disk and will never be reported safe. The seal
    /// cadence was declared per stream in the catalogue and read by nothing.
    ///
    /// Takes each partition's produce lock for the seal, the same lock a
    /// size-triggered seal already holds inside `produce_to`, so the two
    /// cannot interleave.
    pub fn seal_aged(&self) -> Result<u64> {
        let open: Vec<((String, u32), Arc<PartitionState>)> = {
            let partitions = self.partitions.lock().unwrap_or_else(|e| e.into_inner());
            partitions
                .iter()
                .map(|(key, state)| (key.clone(), state.clone()))
                .collect()
        };
        let now = self.clock.now();
        let mut sealed = 0u64;
        for ((stream, _), state) in open {
            let seal_age_ns = i64::try_from(self.stream_policy(&stream)?.seal_age_ms())
                .unwrap_or(i64::MAX)
                .saturating_mul(1_000_000);
            let mut cursor = state.cursor.lock().unwrap_or_else(|e| e.into_inner());
            let Some(since) = cursor.active_since else {
                continue;
            };
            if now.as_nanos().saturating_sub(since.as_nanos()) < seal_age_ns {
                continue;
            }
            if state.log.seal_active()? {
                sealed += 1;
            }
            cursor.active_since = None;
        }
        Ok(sealed)
    }

    /// FABRIC-012: archive every sealed, not yet archived segment of every
    /// partition of every archive-required stream, returning how many were
    /// archived by this call.
    ///
    /// Meant to be called from a thread of its own. Nothing here holds the
    /// produce path's locks while a segment's bytes are being uploaded:
    /// `Archiver::archive` reads a sealed, immutable file and touches the
    /// segment log only to read its seal and to set its archive mark. A
    /// stalled object store therefore stalls this call and nothing else.
    ///
    /// The first failure is returned after every other partition has been
    /// tried, so one partition's unreadable segment does not stop the rest
    /// of the broker's history from being archived.
    pub fn archive_sealed(&self, archiver: &Archiver) -> Result<u64> {
        let declared: Vec<(String, u32, StreamPolicy)> = {
            let declared = self.declared.lock().unwrap_or_else(|e| e.into_inner());
            declared
                .iter()
                .map(|(name, d)| (name.clone(), d.partition_count, d.policy.clone()))
                .collect()
        };
        let mut archived = 0u64;
        let mut first_failure = None;
        for (stream, partition_count, policy) in declared {
            if !policy.archive_required() {
                continue;
            }
            let entitlements = BTreeSet::from([format!(
                "{}:{}",
                policy.entitlement().dataset(),
                policy.entitlement().usage()
            )]);
            for partition in 0..partition_count {
                let outcome = self.partition_state(&stream, partition).and_then(|state| {
                    state
                        .log
                        .archive_sealed(archiver, &stream, partition, &entitlements)
                });
                match outcome {
                    Ok(count) => archived += count,
                    Err(error) => {
                        first_failure.get_or_insert(error);
                    }
                }
            }
        }
        match first_failure {
            Some(error) => Err(error),
            None => Ok(archived),
        }
    }

    /// Every declared stream and its partition count, by name.
    pub fn declared_streams(&self) -> BTreeMap<String, u32> {
        let declared = self.declared.lock().unwrap_or_else(|e| e.into_inner());
        declared
            .iter()
            .map(|(name, d)| (name.clone(), d.partition_count))
            .collect()
    }

    /// Assign `producer_id` its next epoch at `(stream, partition)`, fencing
    /// every earlier incarnation of the same id immediately (CONTRACT-049).
    pub fn init_producer(&self, stream: &str, partition: u32, producer_id: &str) -> Result<u64> {
        self.partition_state(stream, partition)?;
        let label = partition_label(stream, partition);
        let mut producers = self.producers.lock().unwrap_or_else(|e| e.into_inner());
        producers.init(producer_id, &label)
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

    /// FABRIC-016: commit `group`'s checkpoint for `(stream, partition)` at
    /// `offset`, the last batch offset the group has finished processing.
    /// Durable before it returns, so a consumer killed after this call
    /// resumes from [`Self::committed_offset`] plus one on this broker or on
    /// a restart of it.
    ///
    /// Refuses an offset the partition has not reached (a checkpoint over
    /// data that does not exist would make the group skip whatever is later
    /// written there), and refuses moving a checkpoint backwards: an old
    /// instance of the group resuming after a pause would otherwise rewind
    /// its successor and re-deliver what was processed. Committing the
    /// offset already held is idempotent.
    pub fn commit_offset(
        &self,
        group: &str,
        stream: &str,
        partition: u32,
        offset: u64,
    ) -> Result<u64> {
        let key = checkpoint_key(group, stream, partition)?;
        let state = self.partition_state(stream, partition)?;
        let high_watermark = state.log.high_water();
        if offset >= high_watermark {
            return Err(Error::invalid(format!(
                "group '{group}' tried to commit offset {offset} of {stream}:{partition}, which \
                 holds offsets below {high_watermark}; commit only an offset already read"
            )));
        }
        if let Some(held) = self.committed_offset(group, stream, partition)?
            && offset < held
        {
            return Err(Error::denied(format!(
                "group '{group}' tried to move its checkpoint on {stream}:{partition} back from \
                 {held} to {offset}; seek a fresh consumer instead of rewinding the group"
            )));
        }
        self.checkpoints.put(&key, serde_json::to_value(offset)?)?;
        Ok(offset)
    }

    /// `group`'s last committed offset for `(stream, partition)`, or `None`
    /// if it has never committed — in which case it resumes from wherever it
    /// chooses to seek, not from a guess made here.
    pub fn committed_offset(
        &self,
        group: &str,
        stream: &str,
        partition: u32,
    ) -> Result<Option<u64>> {
        let key = checkpoint_key(group, stream, partition)?;
        match self.checkpoints.get(&key)? {
            Some(value) => serde_json::from_value::<u64>(value).map(Some).map_err(|e| {
                Error::schema(format!("stored checkpoint {key} is not an integer: {e}"))
            }),
            None => Ok(None),
        }
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

    /// FABRIC-049's lag limit: on a stream declared `RefuseProducer` (P0/P1,
    /// never dropped), refuse a produce while any consumer group that has
    /// checkpointed this partition trails the high watermark by more than
    /// the policy's `lag_limit` batches. Other overload policies tolerate a
    /// backlog by declaration and are not refused here. Runs before the
    /// producer table so a refused batch consumes no sequence number.
    ///
    /// A group that has never committed has no checkpoint and is invisible
    /// here; the limit bounds the lag of groups the broker knows about.
    /// ponytail: scans checkpoint keys per produce, O(groups on the
    /// broker); index groups by partition if that count ever grows large.
    fn refuse_when_a_group_lags(&self, stream: &str, partition: u32) -> Result<()> {
        let policy = self.stream_policy(stream)?;
        if policy.overload_policy() != OverloadPolicy::RefuseProducer {
            return Ok(());
        }
        let high_watermark = self.partition_state(stream, partition)?.log.high_water();
        let suffix = format!("|{stream}|{partition}");
        for key in self.checkpoints.keys_with_prefix("")? {
            let Some(group) = key.strip_suffix(&suffix) else {
                continue;
            };
            let Some(committed) = self.committed_offset(group, stream, partition)? else {
                continue;
            };
            let lag = high_watermark.saturating_sub(committed.saturating_add(1));
            if lag > policy.lag_limit() {
                return Err(Error::denied(format!(
                    "group '{group}' trails {stream}:{partition} by {lag} batches, past the                      stream's lag limit of {}; the producer is refused until the group catches up",
                    policy.lag_limit()
                )));
            }
        }
        Ok(())
    }

    /// Charge `batch` against its producer's byte and message quota for the
    /// current window, refusing with the time to wait when either would be
    /// exceeded (FABRIC-049). A refused batch is not charged, so a producer
    /// that backs off as told is not penalised twice. Runs before the
    /// producer table sees the batch: a producer over quota must not be
    /// able to consume sequence numbers it was never allowed to use.
    fn charge_quota(
        &self,
        stream: &str,
        batch: &Batch,
    ) -> Result<std::result::Result<(), ProduceRefusal>> {
        let policy = self.stream_policy(stream)?;
        let messages = u64::try_from(batch.records.len())
            .map_err(|_| Error::invalid("a batch carries more records than a u64 can count"))?;
        let bytes = batch
            .records
            .iter()
            .try_fold(0u64, |sum, r| sum.checked_add(r.payload.len() as u64))
            .ok_or_else(|| Error::numeric("a batch's payload bytes overflow a u64"))?;
        let now = self.clock.now().as_nanos();
        let mut spent = self.quota_spent.lock().unwrap_or_else(|e| e.into_inner());
        let window_start = now - now.rem_euclid(QUOTA_WINDOW_NS);
        spent.retain(|_, (start, _, _)| *start == window_start);
        let entry = spent
            .entry((stream.to_string(), batch.producer_id.clone()))
            .or_insert((window_start, 0, 0));
        let over_bytes = entry.1.saturating_add(bytes) > policy.byte_quota_per_producer();
        let over_messages = entry.2.saturating_add(messages) > policy.message_quota_per_producer();
        if over_bytes || over_messages {
            let retry_after_ms = (window_start + QUOTA_WINDOW_NS - now) / 1_000_000;
            let error = Error::denied(format!(
                "producer '{}' is over its quota on stream '{stream}' ({} of {} bytes, {} of {}                  messages this second, this batch adds {bytes} and {messages}); retry after                  {retry_after_ms} ms",
                batch.producer_id,
                entry.1,
                policy.byte_quota_per_producer(),
                entry.2,
                policy.message_quota_per_producer(),
            ));
            return Ok(Err(ProduceRefusal::new(
                Refusal::Quota {
                    retry_after_ms: u64::try_from(retry_after_ms).unwrap_or(0),
                },
                error,
            )));
        }
        entry.1 += bytes;
        entry.2 += messages;
        Ok(Ok(()))
    }

    /// How many partitions `stream` was declared with.
    pub fn partition_count(&self, stream: &str) -> Result<u32> {
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
        // A restart finds whatever the previous process left unsealed. Its
        // age is unknown, so it is measured from now: late by at most one
        // seal age, and never left unsealed for want of a next append.
        let active_since = (log.active_batches() > 0).then(|| self.clock.now());
        let state = Arc::new(PartitionState {
            log,
            cursor: Mutex::new(ProduceCursor {
                clock: PartitionClock::new(self.clock.now()),
                last_batch_hash,
                active_since,
            }),
        });
        let mut partitions = self.partitions.lock().unwrap_or_else(|e| e.into_inner());
        // Another call may have opened the same partition first while this
        // one built `state` outside the lock; keep whichever won rather than
        // hold two `SegmentLog`s open on the same directory, which
        // `SegmentLog::open`'s own guard would refuse for the second one
        // anyway.
        if let Some(winner) = partitions.get(&key) {
            return Ok(winner.clone());
        }
        // Replayed while still holding the partitions lock, so no produce
        // can be admitted against this partition before the table knows what
        // the log already holds.
        self.recover_producers(stream, partition, &state.log)?;
        partitions.insert(key, state.clone());
        Ok(state)
    }

    /// Rebuild the producer table and the retry-offset memory for one
    /// partition from the batches already durable in it (RES-058).
    ///
    /// ponytail: reads the whole partition once at open, so the cost grows
    /// with its length; persist a snapshot beside the segments if opening a
    /// long partition ever becomes slow.
    fn recover_producers(&self, stream: &str, partition: u32, log: &PartitionLog) -> Result<()> {
        let label = partition_label(stream, partition);
        for offset in 0..log.high_water() {
            let Some(batch) = log.read(offset)? else {
                continue;
            };
            let Ok(record_count) = u64::try_from(batch.records.len()) else {
                continue;
            };
            if record_count == 0 || batch.producer_id.is_empty() {
                continue;
            }
            {
                let mut producers = self.producers.lock().unwrap_or_else(|e| e.into_inner());
                producers.restore(
                    &batch.producer_id,
                    &label,
                    batch.producer_epoch,
                    batch.base_sequence,
                    record_count,
                    payload_fingerprint(&batch),
                )?;
            }
            self.remember_offset(&batch.producer_id, &label, batch.base_sequence, offset);
        }
        Ok(())
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

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
use qip_storage::BlobStore;
use qip_storage::segment::archive::{ArchiveReader, Archiver};
use qip_storage::segment::log::SegmentLogConfig;
use qip_storage::{DurableStore, EngineConfig, KeyValueStore};
use serde::{Deserialize, Serialize};

use qip_events::event_fabric::codec::{
    Batch, BrokerOrigin, ContentHash, DecodeOutcome, LogicalTimestamp, stamp_broker,
    stamp_broker_origin, stamp_broker_partition,
};
use qip_events::event_fabric::hlc::{HlcTimestamp, PartitionClock};
use qip_events::event_fabric::policy::{QosClass, StreamPolicy};
use qip_events::event_fabric::schema_id::{Shape, check_compatible};

use qip_transport::event_fabric::protocol::{
    FetchResponse, Metadata, ProduceAck, ProducerCredit, Refusal,
};

use super::managed::{
    CommitOutcome, CommittedBatch, Lease, LeaseOutcome, ManagedLog, PartitionRef,
};
use super::partition::{self, PartitionLog};
use super::producer::{Admission, ProducerTable, WINDOW_SIZE};

/// How many committed rows one catch-up read asks the managed log for.
const CATCH_UP_PAGE: u32 = 256;

/// Hex of a content hash this build produces. `None` for the Blake3 tag the
/// codec refuses everywhere, which never reaches a managed row.
fn content_hash_hex(hash: &ContentHash) -> Option<String> {
    match hash {
        ContentHash::Sha256(digest) => Some(qip_core::hash::to_hex(digest)),
        ContentHash::Blake3(_) => None,
    }
}

/// This instance's seat at the managed consensus (ADR 0111 §1): the port,
/// the identity it holds leases under, and the leases it holds now.
///
/// `holder` is the instance's name joined to its local leader epoch, so a
/// restarted process never believes it still holds the lease its previous
/// incarnation held: the database sees a different holder and either hands
/// it a fresh epoch after expiry or tells it somebody else leads.
#[derive(Debug)]
struct ManagedSeat {
    log: Arc<dyn ManagedLog>,
    holder: String,
    /// This instance's name without the leader epoch, which is what the
    /// rendezvous placement is computed over.
    instance: String,
    placement: Option<Placement>,
    leases: Mutex<BTreeMap<(String, u32), Lease>>,
}

/// ADR 0111 Amendment 1 §2: the instances of the region and how long a
/// partition's non-preferred instances wait after its lease has expired.
#[derive(Debug)]
struct Placement {
    instances: Vec<String>,
    grace_ms: u64,
    /// Until this broker-clock instant a non-preferred instance only follows
    /// a partition it holds no lease on, even when no lease row exists yet:
    /// a vacant row has no expiry to count a grace from, and without this the
    /// first instance up would lead everything.
    settled_at_ns: i64,
}

impl ManagedSeat {
    /// What this instance defers to for `(stream, partition)`: the grace it
    /// waits after the lease has expired and the instant before which it only
    /// follows. `None` for the rendezvous-preferred leader, and without a
    /// placement.
    fn deferral(&self, stream: &str, partition: u32) -> Option<(u64, i64)> {
        let placement = self.placement.as_ref()?;
        let names: Vec<&str> = placement.instances.iter().map(String::as_str).collect();
        match partition::leader_for(stream, partition, &names) {
            Ok(preferred) if preferred != self.instance => {
                Some((placement.grace_ms, placement.settled_at_ns))
            }
            // The preferred leader contends at once. An error is unreachable
            // once `with_placement` has validated the set; contending at once
            // is then the pre-placement behaviour.
            _ => None,
        }
    }

    fn lease(&self, stream: &str, partition: u32) -> Option<Lease> {
        self.leases
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&(stream.to_string(), partition))
            .cloned()
    }

    fn set_lease(&self, stream: &str, partition: u32, lease: Option<Lease>) {
        let mut leases = self.leases.lock().unwrap_or_else(|e| e.into_inner());
        let key = (stream.to_string(), partition);
        match lease {
            Some(lease) => {
                leases.insert(key, lease);
            }
            None => {
                leases.remove(&key);
            }
        }
    }
}

/// What one [`Broker::tend_leases`] pass did, per managed partition.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LeaseReport {
    /// Partitions this instance leads, with the epoch it leads them at.
    pub leading: BTreeMap<(String, u32), u64>,
    /// Partitions another instance leads; this one followed their log.
    pub following: BTreeMap<(String, u32), String>,
    /// Partitions whose lease or log could not be read this pass, and why.
    pub failed: BTreeMap<(String, u32), String>,
}

/// What [`Broker::relinquish_leases`] did, per partition this instance led.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReleaseReport {
    /// Partitions whose lease was lapsed in the database, so a follower can
    /// take them now rather than at the TTL.
    pub released: Vec<(String, u32)>,
    /// Partitions led here whose lease had already been superseded.
    pub superseded: Vec<(String, u32)>,
    /// Partitions whose release the database refused or never answered, and
    /// why. This instance has stopped acknowledging them all the same; their
    /// leases lapse at the TTL.
    pub failed: BTreeMap<(String, u32), String>,
}

/// What [`Broker::rebuild_from_archive`] replayed, per local partition it
/// found empty.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RebuildReport {
    /// Batches replayed into each partition, which is also the offset the
    /// partition now continues at. A partition the archive holds nothing for
    /// is absent rather than zero.
    pub replayed: BTreeMap<(String, u32), u64>,
}

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
    /// The event topics the stream admits in a record's body, each with the
    /// schema versions the committed lock holds for it (R12-FABRIC-8090,
    /// ADR 0156 §1.3). `None` is a stream declared without a topic list,
    /// which admits any; `Some` refuses a record of any other topic, or of a
    /// version the lock does not hold for its topic, before it is appended.
    body_schemas: Option<BTreeMap<String, BTreeSet<u32>>>,
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
    /// For a managed partition, the lease epoch at which this log was last
    /// proven to hold every committed row. `None` forces a catch-up from the
    /// managed log before the next commit: after a new lease, a lost commit
    /// answer, or a conflict.
    caught_up_epoch: Option<u64>,
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

/// A batch re-appended from the managed log or the archive carries the
/// partition the broker that first accepted it stamped (R12-FABRIC-8084);
/// one stamped for another partition is a row or an object misfiled, and is
/// refused rather than hydrated into a log whose key order it would change
/// in silence. A batch with no stamp — written before format version 2 —
/// has nothing to disagree with and is taken as the offset check took it.
fn require_stamped_for(batch: &Batch, partition: u32, label: &str) -> Result<()> {
    if let Some(stamped) = batch.partition
        && stamped != partition
    {
        return Err(Error::invalid(format!(
            "{label}: the batch at offset {} is stamped for partition {stamped}; refusing to \
             hydrate it into another partition's log",
            batch.base_offset
        )));
    }
    Ok(())
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

/// The protocol refusal for an admission that did not append, shared by the
/// local and the managed produce paths so the two cannot answer the same
/// sequence fault differently.
fn refused_admission(admission: Admission, batch: &Batch, label: &str) -> Result<Produced> {
    let refusal = match admission {
        Admission::Conflict => ProduceRefusal::new(
            Refusal::SequenceConflict,
            Error::denied(format!(
                "producer '{}' sequence {} at {label} conflicts with a batch this broker \
                 already accepted under the same epoch and sequence but a different payload",
                batch.producer_id, batch.base_sequence
            )),
        ),
        Admission::FencedEpoch { current_epoch } => ProduceRefusal::new(
            Refusal::Fenced,
            Error::denied(format!(
                "producer '{}' epoch {} at {label} is fenced by epoch {current_epoch}",
                batch.producer_id, batch.producer_epoch
            )),
        ),
        Admission::OutsideWindow => ProduceRefusal::new(
            Refusal::SequenceBelowWindow,
            Error::denied(format!(
                "producer '{}' sequence {} at {label} is behind this broker's bounded dedup \
                 window and cannot be verified as a duplicate",
                batch.producer_id, batch.base_sequence
            )),
        ),
        Admission::OutOfOrder { expected } => ProduceRefusal::new(
            Refusal::OutOfOrderSequence { expected },
            Error::invalid(format!(
                "producer '{}' sequence {} at {label} is ahead of the dense stream; the next \
                 sequence this partition accepts is {expected}",
                batch.producer_id, batch.base_sequence
            )),
        ),
        Admission::Appended { .. } | Admission::Duplicate => {
            return Err(Error::io(format!(
                "{label}: an admission that appended was routed to the refusal path"
            )));
        }
    };
    Ok(Err(refusal))
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
    /// ADR 0111's managed consensus, when the composition root configured
    /// one. `None` is the local single-broker fabric — dev, test and every
    /// environment that has not configured Spanner — in which a stream
    /// declared `managed_majority` is refused at declaration.
    managed: Option<ManagedSeat>,
    /// The archive, read back (FABRIC-8079): what [`Self::fetch`] hydrates
    /// an offset from once its local segment has been retained away, and
    /// what [`Self::retain_hot`] requires before it deletes anything.
    archive: Option<ArchiveReader>,
    /// The region this broker serves, stamped into every batch it accepts
    /// from an authenticated session (ADR 0129 §1). `None` stamps no origin:
    /// a broker that does not know its region does not guess one, and its
    /// consumers refuse what it serves rather than read a region into it.
    region: Option<String>,
}

/// `(stream, producer)` to `(window_start_ns, bytes, messages)`.
type QuotaSpend = BTreeMap<(String, String), (i64, u64, u64)>;

/// The period a stream's per-producer byte and message quotas are measured
/// over: one second, because `peak_bytes_per_second` is the unit the same
/// policy sizes its stream in (FABRIC-049).
const QUOTA_WINDOW_NS: i64 = 1_000_000_000;

/// The hot window, in batches, a stream of `class` keeps on local disk when
/// the operator configured `keep_batches`.
///
/// A class retained nowhere beyond its ring (§22.1's `Never`: raw ticks,
/// book deltas, quote updates) keeps one batch, the segment the next produce
/// chains to; every other class keeps the configured window, and an
/// archive-required class is further held behind its archived and indexed
/// boundary by [`Broker::retain_hot`].
pub fn hot_window(class: qip_events::RetentionClass, keep_batches: u64) -> u64 {
    if class.retention() == qip_events::Retention::Never {
        1
    } else {
        keep_batches
    }
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
            managed: None,
            archive: None,
            region: None,
        })
    }

    /// The region this broker stamps into every batch an authenticated
    /// session produces (ADR 0129 §1). Refuses a blank one.
    pub fn with_region(mut self, region: &str) -> Result<Self> {
        let region = region.trim();
        if region.is_empty() {
            return Err(Error::invalid(
                "a broker's region must be named; a blank region would be stamped into every \
                 batch as where it was produced",
            ));
        }
        self.region = Some(region.to_string());
        Ok(self)
    }

    /// The origin stamp for a batch produced by `principal` to `partition`,
    /// when this broker knows its region and the session authenticated
    /// someone; the partition alone otherwise, because every append names
    /// the partition it landed in (R12-FABRIC-8084).
    fn stamp_origin(&self, batch: &mut Batch, partition: u32, principal: Option<&str>) {
        stamp_broker_partition(batch, partition);
        if let (Some(region), Some(principal)) = (&self.region, principal) {
            stamp_broker_origin(
                batch,
                BrokerOrigin {
                    region: region.clone(),
                    partition,
                    principal: principal.to_string(),
                },
            );
        }
    }

    /// Seat this broker at ADR 0111's managed consensus as `instance`.
    ///
    /// From here a P0 or P1 stream must be declared `managed_majority`, and
    /// its produces are acknowledged only once [`ManagedLog::commit`] has
    /// committed them under a lease this instance holds. The holder name is
    /// `instance` joined to this process's leader epoch — see
    /// [`ManagedSeat`] for why the epoch is part of it.
    pub fn with_managed_log(mut self, log: Arc<dyn ManagedLog>, instance: &str) -> Result<Self> {
        if instance.trim().is_empty() || instance.contains('#') {
            return Err(Error::invalid(
                "a managed-log instance name must be non-empty and contain no '#', which \
                 separates it from the process's leader epoch in the lease holder",
            ));
        }
        self.managed = Some(ManagedSeat {
            log,
            holder: format!("{instance}#{}", self.leader_epoch),
            instance: instance.to_string(),
            placement: None,
            leases: Mutex::new(BTreeMap::new()),
        });
        Ok(self)
    }

    /// Prefer rendezvous placement of managed partitions over `instances`
    /// (ADR 0111 Amendment 1 §2, R12-FABRIC-8085): this instance contends at
    /// once only for the partitions [`partition::leader_for`] names it for,
    /// and for any other waits `grace_ms` after the lease has expired. The
    /// lease still decides who holds, so a wrong or stale instance list
    /// changes where leaders settle and never who may commit.
    ///
    /// Refuses a broker not seated at a managed log, a set that does not
    /// include this instance, and an empty, blank or repeated name.
    pub fn with_placement(mut self, instances: Vec<String>, grace_ms: u64) -> Result<Self> {
        let seat = self.managed.as_ref().ok_or_else(|| {
            Error::invalid("placement was asked of a broker with no managed log; seat it first")
        })?;
        if !instances.contains(&seat.instance) {
            return Err(Error::invalid(format!(
                "the placement set {instances:?} does not include this instance '{}'; an \
                 instance outside its own set would never be preferred for anything",
                seat.instance
            )));
        }
        let names: Vec<&str> = instances.iter().map(String::as_str).collect();
        partition::leader_for("placement", 0, &names)?;
        let settled_at_ns = i64::try_from(grace_ms)
            .ok()
            .and_then(|ms| ms.checked_mul(1_000_000))
            .and_then(|ns| self.clock.now().as_nanos().checked_add(ns))
            .ok_or_else(|| {
                Error::invalid(format!("a placement grace of {grace_ms} ms overflows"))
            })?;
        let seat = self.managed.as_mut().ok_or_else(|| {
            Error::invalid("placement was asked of a broker with no managed log; seat it first")
        })?;
        seat.placement = Some(Placement {
            instances,
            grace_ms,
            settled_at_ns,
        });
        Ok(self)
    }

    /// Read archived segments back from `store` when their local copy is
    /// gone (FABRIC-8079). The same store the archiver writes to, with the
    /// manifest index on (`Archiver::with_manifest_index`).
    pub fn with_archive_reader(mut self, store: Arc<dyn BlobStore>) -> Self {
        self.archive = Some(ArchiveReader::new(store));
        self
    }

    /// The lease-holder name this broker commits under, when it is seated.
    pub fn managed_holder(&self) -> Option<&str> {
        self.managed.as_ref().map(|seat| seat.holder.as_str())
    }

    /// This broker's leader epoch: bumped and persisted once, in
    /// [`Broker::open`], for the lifetime of this instance.
    pub fn leader_epoch(&self) -> u64 {
        self.leader_epoch
    }

    /// The broker clock's reading, in nanoseconds: what consumer-group leases
    /// are measured against, so a test advancing a manual clock expires a
    /// member the same way wall time would.
    pub fn now_ns(&self) -> i64 {
        self.clock.now().as_nanos()
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
        // ADR 0111 §1: a `managed_majority` answer means "survives a zone".
        // Given on one broker's disk it would be FABRIC-8086's INCORRECT
        // verdict with a new name, so a broker with no managed log refuses
        // the stream rather than serving it at a weaker durability.
        if policy.is_managed() && self.managed.is_none() {
            return Err(Error::denied(format!(
                "stream '{stream}' is declared managed_majority (ADR 0111) and this broker has no \
                 managed log configured; configure the Spanner commit path, or declare the \
                 stream 'archived' at replication factor one for a single-broker fabric"
            )));
        }
        // And the converse: with the managed consensus configured, a P0 or P1
        // stream left on the local path would be the one control or outcome
        // stream that does not survive a zone while the configuration says
        // the fabric does.
        if self.managed.is_some()
            && matches!(
                policy.qos_class(),
                QosClass::P0Control | QosClass::P1Outcomes
            )
            && !policy.is_managed()
        {
            return Err(Error::denied(format!(
                "stream '{stream}' is {} and declared {:?} while this broker is seated at the \
                 managed consensus; every P0 and P1 stream must then be declared \
                 managed_majority at replication factor 3 (ADR 0111 §1)",
                policy.qos_class().as_str(),
                policy.ack_profile()
            )));
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

    /// Declare the event topics `stream` admits in a record's body, and for
    /// each the schema versions it admits. A produce carrying a record of any
    /// other topic, or of a version outside that topic's set, is refused with
    /// [`Refusal::SchemaRefused`] before anything is appended: the batch
    /// schema registered by [`Self::register_schema`] says the batch is well
    /// formed, and this says what is in it belongs on this stream.
    ///
    /// Admission is by `(topic, version)` and not by the shape of the body
    /// read at produce time: a shape read off a runtime value hides a retyped
    /// field behind `null` and an empty array, so a check on it could pass a
    /// body the lock would refuse (ADR 0156 §1.3). Shape is held at build
    /// time by the lock suite.
    ///
    /// Replaces the set for `stream` rather than adding to it, so a catalogue
    /// that narrows a stream narrows what it accepts. Refuses an empty set,
    /// and a topic with no version: a stream admitting nothing is a catalogue
    /// mistake, not a closed stream.
    pub fn register_body_schemas(
        &self,
        stream: &str,
        topics: BTreeMap<String, BTreeSet<u32>>,
    ) -> Result<()> {
        if topics.is_empty() {
            return Err(Error::invalid(format!(
                "stream '{stream}' was given no body topics; a stream that admits no event \
                 is a catalogue mistake, so declare the topics it carries"
            )));
        }
        if let Some((topic, _)) = topics.iter().find(|(_, versions)| versions.is_empty()) {
            return Err(Error::invalid(format!(
                "stream '{stream}' admits topic '{topic}' at no schema version; a topic the \
                 schema lock holds no row for cannot be streamed"
            )));
        }
        let mut schemas = self.schemas.lock().unwrap_or_else(|e| e.into_inner());
        schemas.entry(stream.to_string()).or_default().body_schemas = Some(topics);
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
    pub fn produce_to(&self, stream: &str, partition: u32, batch: Batch) -> Result<Produced> {
        self.produce_from(stream, partition, batch, None)
    }

    /// [`Self::produce_to`] for a batch an authenticated session produced:
    /// `principal` is the identity the session authenticated as, stamped
    /// into the batch with this broker's region and the partition (ADR 0129
    /// §1). The protocol service is the caller; an in-process producer with
    /// no session has no principal to stamp, and [`Self::produce_to`] stamps
    /// none for it.
    pub fn produce_from(
        &self,
        stream: &str,
        partition: u32,
        mut batch: Batch,
        principal: Option<&str>,
    ) -> Result<Produced> {
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
        if let Err(error) = self.require_admitted_topics(stream, &batch) {
            return Ok(Err(ProduceRefusal::new(Refusal::SchemaRefused, error)));
        }
        // ADR 0129 §1: region, partition and principal are the broker's to
        // stamp. A batch that arrives already carrying them is a producer
        // asserting where it was produced, on a wire that authenticates
        // nobody below the session; refused rather than overwritten, so the
        // producer learns it sent something it may not.
        if let Some(origin) = &batch.origin {
            return Ok(Err(ProduceRefusal::new(
                Refusal::SchemaRefused,
                Error::denied(format!(
                    "the batch arrived already stamped with region {} and principal {}; \
                     those are stamped by the broker that accepts it, never by its producer",
                    origin.region, origin.principal
                )),
            )));
        }
        // The partition alone (R12-FABRIC-8084) under the same rule: the
        // broker stamps the partition it appends to, and a producer that
        // states one is refused rather than overwritten.
        if let Some(stated) = batch.partition {
            return Ok(Err(ProduceRefusal::new(
                Refusal::SchemaRefused,
                Error::denied(format!(
                    "the batch arrived already stamped with partition {stated}; the partition \
                     is stamped by the broker that accepts it, never by its producer"
                )),
            )));
        }
        self.refuse_when_a_group_lags(stream, partition)?;
        let credit = match self.charge_quota(stream, &batch)? {
            Ok(credit) => credit,
            Err(refused) => return Ok(Err(refused)),
        };
        if self.stream_policy(stream)?.is_managed() {
            return self.produce_managed(stream, partition, batch, principal, credit);
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
                    self.stamp_origin(&mut batch, partition, principal);
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
                .map(|ack| Ok(ack.with_credit(credit)))
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
                .map(|ack| Ok(ack.with_credit(credit)))
            }
            refused => refused_admission(refused, &batch, &label),
        }
    }

    /// [`Self::produce_to`] for a stream declared `managed_majority`
    /// (ADR 0111 §1): the batch is committed to the managed log under this
    /// instance's lease **before** it is appended locally, and the producer
    /// is answered only after the commit.
    ///
    /// Commit-first is what keeps the local log a prefix of the managed one.
    /// Appending first would leave a deposed leader's disk holding a batch
    /// the managed log refused, at an offset the new leader then fills with
    /// a different batch. The whole path runs under the partition's produce
    /// lock, so the producer table can be rewound if the commit does not
    /// happen: a table that believed a refused batch appended would answer
    /// its retry as a verified duplicate, an acknowledgement for a record no
    /// store holds.
    ///
    /// A commit whose answer is lost is treated as possibly committed: the
    /// table is rewound, the partition is marked for catch-up, and the next
    /// produce reads the managed log first — finding the batch if it landed,
    /// so the producer's retry is answered as the duplicate it is.
    fn produce_managed(
        &self,
        stream: &str,
        partition: u32,
        mut batch: Batch,
        principal: Option<&str>,
        credit: ProducerCredit,
    ) -> Result<Produced> {
        let seat = self.managed.as_ref().ok_or_else(|| {
            Error::denied(format!(
                "stream '{stream}' is managed and this broker has no managed log; it should never \
                 have been declared here"
            ))
        })?;
        let record_count = u64::try_from(batch.records.len()).map_err(|_| {
            Error::invalid("a batch carries more records than a u64 count can represent")
        })?;
        let payload_hash = payload_fingerprint(&batch);
        let label = partition_label(stream, partition);
        let state = self.partition_state(stream, partition)?;
        let mut cursor = state.cursor.lock().unwrap_or_else(|e| e.into_inner());

        let Some(lease) = seat.lease(stream, partition) else {
            return Err(Error::unavailable(format!(
                "{label} is a managed partition and this fabricd instance ('{}') holds no lease \
                 on it, so it cannot commit; another instance leads it or none does yet. The \
                 producer backs off and retries against the leader (ADR 0111 §1)",
                seat.holder
            )));
        };
        if cursor.caught_up_epoch != Some(lease.epoch) {
            self.catch_up(seat, stream, partition, &state, &mut cursor)?;
            cursor.caught_up_epoch = Some(lease.epoch);
        }

        let (checkpoint, admission) = {
            let mut producers = self.producers.lock().unwrap_or_else(|e| e.into_inner());
            let checkpoint = producers.checkpoint(&batch.producer_id, &label);
            let admission = producers.admit(
                &batch.producer_id,
                &label,
                batch.producer_epoch,
                batch.base_sequence,
                record_count,
                payload_hash,
            )?;
            (checkpoint, admission)
        };
        let producer_id = batch.producer_id.clone();
        let rewind = |checkpoint| {
            self.producers
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .rewind(&producer_id, &label, checkpoint);
        };

        match admission {
            Admission::Appended { .. } => {}
            Admission::Duplicate => {
                // Every batch in a managed partition's local log is committed
                // (the log is a prefix of the managed one), so a verified
                // duplicate was committed under some lease of this partition.
                let base_offset = self
                    .recall_offset(&batch.producer_id, &label, batch.base_sequence)
                    .ok_or_else(|| {
                        Error::io(format!(
                            "producer '{}' sequence {} at {label} was verified as a duplicate \
                             but this broker holds no record of the offset it received",
                            batch.producer_id, batch.base_sequence
                        ))
                    })?;
                let high_watermark = state.log.high_water();
                return ProduceAck::new(
                    stream,
                    partition,
                    base_offset,
                    high_watermark,
                    state.log.archived_through()?,
                )
                .map(|ack| Ok(ack.with_managed_commit(lease.epoch)));
            }
            refused => return refused_admission(refused, &batch, &label),
        }

        let predicted_offset = state.log.high_water();
        let now = self.clock.now();
        let logical_timestamp = to_logical_timestamp(cursor.clock.tick(now)?)?;
        let previous_hash = cursor.last_batch_hash.clone();
        self.stamp_origin(&mut batch, partition, principal);
        // A managed batch carries the lease epoch, not this process's local
        // leader epoch: the lease is what fences, and two instances' local
        // epochs are unrelated numbers.
        stamp_broker(
            &mut batch,
            predicted_offset,
            lease.epoch,
            logical_timestamp,
            previous_hash.clone(),
        );
        let encoded = batch.encode()?;
        let row = CommittedBatch {
            partition: PartitionRef::new(stream, partition),
            offset: predicted_offset,
            lease_epoch: lease.epoch,
            batch_hash: qip_core::hash::sha256_hex(&encoded),
            previous_batch_hash: previous_hash.as_ref().and_then(content_hash_hex),
            bytes: encoded.clone(),
        };
        match seat.log.commit(&lease, &row) {
            Ok(CommitOutcome::Committed) => {}
            Ok(CommitOutcome::Fenced { current_epoch }) => {
                rewind(checkpoint);
                seat.set_lease(stream, partition, None);
                cursor.caught_up_epoch = None;
                return Err(Error::unavailable(format!(
                    "{label}: this instance's lease at epoch {} was superseded by epoch \
                     {current_epoch} before the commit; it no longer leads the partition and \
                     acknowledged nothing. The producer retries against the leader",
                    lease.epoch
                )));
            }
            Ok(CommitOutcome::Conflict { committed_through }) => {
                rewind(checkpoint);
                cursor.caught_up_epoch = None;
                return Err(Error::unavailable(format!(
                    "{label}: the managed log is committed through offset {committed_through} \
                     and this instance's log is at {predicted_offset}; it will catch up before \
                     the next commit. Nothing was acknowledged"
                )));
            }
            Err(error) => {
                rewind(checkpoint);
                cursor.caught_up_epoch = None;
                let message = format!(
                    "{label}: the managed commit's outcome is unknown ({}); nothing is \
                     acknowledged, and the next produce reads the managed log first so a retry \
                     of this batch is answered as the duplicate it is if the commit landed",
                    error.message()
                );
                return Err(error.relabelled(message));
            }
        }

        // Committed. From here the record exists; a local failure leaves this
        // disk behind the managed log, which the next catch-up repairs, and
        // the producer table keeps the batch because the batch is real.
        let appended_offset = match state.log.append(&batch) {
            Ok(offset) => offset,
            Err(error) => {
                cursor.caught_up_epoch = None;
                return Err(error);
            }
        };
        if appended_offset != predicted_offset {
            cursor.caught_up_epoch = None;
            return Err(Error::io(format!(
                "partition {label} assigned offset {appended_offset} after the managed log \
                 committed {predicted_offset}; something else is appending to this partition"
            )));
        }
        cursor.last_batch_hash = Some(ContentHash::sha256_of(&encoded));
        cursor.active_since = if state.log.active_batches() == 0 {
            None
        } else {
            cursor.active_since.or(Some(now))
        };
        drop(cursor);
        self.remember_offset(
            &batch.producer_id,
            &label,
            batch.base_sequence,
            appended_offset,
        );
        ProduceAck::new(
            stream,
            partition,
            appended_offset,
            state.log.high_water(),
            state.log.archived_through()?,
        )
        .map(|ack| Ok(ack.with_managed_commit(lease.epoch).with_credit(credit)))
    }

    /// Append to `state`'s log every row the managed log has committed past
    /// it, verifying each row's hash, density and chain link, and teach the
    /// producer table about each one.
    ///
    /// Refuses a local log that is *ahead* of the managed one: with
    /// commit-first that can only mean this disk holds batches the managed
    /// log never committed (a wrong database, or a restore from the wrong
    /// place), and serving them would serve history that does not exist.
    fn catch_up(
        &self,
        seat: &ManagedSeat,
        stream: &str,
        partition: u32,
        state: &PartitionState,
        cursor: &mut ProduceCursor,
    ) -> Result<u64> {
        let reference = PartitionRef::new(stream, partition);
        let label = partition_label(stream, partition);
        let committed_through = seat.log.committed_through(&reference)?;
        let local = state.log.high_water();
        if local > committed_through {
            return Err(Error::io(format!(
                "{label}: this broker's disk holds offsets up to {local} and the managed log has \
                 committed only {committed_through}; refusing to serve or extend batches the \
                 managed consensus never committed. Check the instance points at the right \
                 database before clearing its data directory"
            )));
        }
        let mut appended = 0u64;
        while state.log.high_water() < committed_through {
            let from = state.log.high_water();
            let rows = seat.log.read(&reference, from, CATCH_UP_PAGE)?;
            if rows.is_empty() {
                return Err(Error::io(format!(
                    "{label}: the managed log reports offsets committed through \
                     {committed_through} but returned none from {from}"
                )));
            }
            for row in rows {
                row.verify()?;
                let expected = state.log.high_water();
                if row.offset != expected {
                    return Err(Error::io(format!(
                        "{label}: the managed log returned offset {} where {expected} was next; \
                         its rows are not dense",
                        row.offset
                    )));
                }
                let chained = cursor.last_batch_hash.as_ref().and_then(content_hash_hex);
                if row.previous_batch_hash != chained {
                    return Err(Error::invalid(format!(
                        "{label}: committed offset {} does not chain to the batch before it; \
                         refusing to hydrate a broken chain (ADR 0104)",
                        row.offset
                    )));
                }
                let batch = match Batch::decode(&row.bytes)? {
                    DecodeOutcome::Complete(batch) => batch,
                    DecodeOutcome::Torn => {
                        return Err(Error::io(format!(
                            "{label}: committed offset {} does not decode complete",
                            row.offset
                        )));
                    }
                };
                if batch.base_offset != row.offset {
                    return Err(Error::invalid(format!(
                        "{label}: the row at offset {} holds a batch stamped for offset {}",
                        row.offset, batch.base_offset
                    )));
                }
                require_stamped_for(&batch, partition, &label)?;
                let offset = state.log.append(&batch)?;
                cursor.last_batch_hash = Some(ContentHash::sha256_of(&row.bytes));
                cursor.active_since = if state.log.active_batches() == 0 {
                    None
                } else {
                    cursor.active_since.or(Some(self.clock.now()))
                };
                if let Ok(record_count) = u64::try_from(batch.records.len())
                    && record_count > 0
                    && !batch.producer_id.is_empty()
                {
                    self.producers
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .restore(
                            &batch.producer_id,
                            &label,
                            batch.producer_epoch,
                            batch.base_sequence,
                            record_count,
                            payload_fingerprint(&batch),
                        )?;
                    self.remember_offset(&batch.producer_id, &label, batch.base_sequence, offset);
                }
                appended += 1;
            }
        }
        Ok(appended)
    }

    /// Every declared managed stream's partitions.
    pub fn managed_partitions(&self) -> Vec<(String, u32)> {
        let declared = self.declared.lock().unwrap_or_else(|e| e.into_inner());
        declared
            .iter()
            .filter(|(_, d)| d.policy.is_managed())
            .flat_map(|(name, d)| (0..d.partition_count).map(move |p| (name.clone(), p)))
            .collect()
    }

    /// One pass of ADR 0111's lease loop over every managed partition: take
    /// or renew the lease for `ttl_ms`, and either lead the partition or
    /// follow its leader's committed log into this instance's own.
    ///
    /// Following is what makes a standby a follower (FABRIC-7035): its disk
    /// holds the committed log up to the last pass, so taking over after the
    /// leader dies costs a short catch-up, not a full hydrate. A partition
    /// whose lease or log cannot be read is reported and left as it was;
    /// its held lease, if any, is kept, because the database — not this
    /// process's view of the time — decides when it has lapsed.
    pub fn tend_leases(&self, ttl_ms: u64) -> Result<LeaseReport> {
        let seat = self.managed.as_ref().ok_or_else(|| {
            Error::invalid("tend_leases was called on a broker with no managed log")
        })?;
        let mut report = LeaseReport::default();
        for (stream, partition) in self.managed_partitions() {
            let key = (stream.clone(), partition);
            let reference = PartitionRef::new(stream.clone(), partition);
            let deferral = seat.deferral(&stream, partition);
            if let Some((_, settled_at_ns)) = deferral
                && self.clock.now().as_nanos() < settled_at_ns
                && seat.lease(&stream, partition).is_none()
            {
                // Settling: the preferred instance has had no time yet to
                // take what is vacant, so this one follows and leaves it.
                match self.follow_partition(seat, &stream, partition) {
                    Ok(()) => {
                        report
                            .following
                            .insert(key, "(deferring to the preferred leader)".to_string());
                    }
                    Err(error) => {
                        report.failed.insert(key, error.to_string());
                    }
                }
                continue;
            }
            let outcome = seat
                .log
                .acquire_after_expiry(
                    &reference,
                    &seat.holder,
                    ttl_ms,
                    deferral.map_or(0, |(grace_ms, _)| grace_ms),
                )
                .and_then(|outcome| {
                    let state = self.partition_state(&stream, partition)?;
                    let mut cursor = state.cursor.lock().unwrap_or_else(|e| e.into_inner());
                    match &outcome {
                        LeaseOutcome::Held(lease) => {
                            seat.set_lease(&stream, partition, Some(lease.clone()));
                            if cursor.caught_up_epoch != Some(lease.epoch) {
                                self.catch_up(seat, &stream, partition, &state, &mut cursor)?;
                                cursor.caught_up_epoch = Some(lease.epoch);
                            }
                        }
                        LeaseOutcome::HeldByOther { .. } => {
                            seat.set_lease(&stream, partition, None);
                            cursor.caught_up_epoch = None;
                            self.catch_up(seat, &stream, partition, &state, &mut cursor)?;
                        }
                    }
                    Ok(outcome)
                });
            match outcome {
                Ok(LeaseOutcome::Held(lease)) => {
                    report.leading.insert(key, lease.epoch);
                }
                Ok(LeaseOutcome::HeldByOther { holder, .. }) => {
                    report.following.insert(key, holder);
                }
                Err(error) => {
                    report.failed.insert(key, error.to_string());
                }
            }
        }
        Ok(report)
    }

    /// Follow `(stream, partition)`'s committed log into this instance's own
    /// without asking the database for a lease.
    fn follow_partition(&self, seat: &ManagedSeat, stream: &str, partition: u32) -> Result<()> {
        let state = self.partition_state(stream, partition)?;
        let mut cursor = state.cursor.lock().unwrap_or_else(|e| e.into_inner());
        // Not leading: a later promotion must catch up afresh.
        cursor.caught_up_epoch = None;
        self.catch_up(seat, stream, partition, &state, &mut cursor)?;
        Ok(())
    }

    /// One pass of a draining instance: follow every managed partition's
    /// committed log into this instance's own, without asking the database
    /// for a lease.
    ///
    /// [`Self::tend_leases`] is the only periodic caller of `catch_up` for a
    /// partition this instance does not lead, so a lease thread that
    /// answered a drain by calling only [`Self::relinquish_leases`] left the
    /// drained instance's disk frozen at the offset it drained at: a fetch
    /// reported a high watermark that stopped advancing while the new leader
    /// committed, and a restart-free rollback to this instance would have
    /// had a whole gap to catch up. Taking a lease is what a drain must not
    /// do, so this never calls `acquire`.
    ///
    /// Every partition is reported as `following`, under a holder string
    /// that says it was not read: learning the holder is what `acquire`
    /// does, and `acquire` would take an expired lease.
    pub fn follow_leases(&self) -> Result<LeaseReport> {
        let seat = self.managed.as_ref().ok_or_else(|| {
            Error::invalid("follow_leases was called on a broker with no managed log")
        })?;
        let mut report = LeaseReport::default();
        for (stream, partition) in self.managed_partitions() {
            let key = (stream.clone(), partition);
            let outcome = self.follow_partition(seat, &stream, partition);
            match outcome {
                Ok(()) => {
                    report
                        .following
                        .insert(key, "(not read while draining)".to_string());
                }
                Err(error) => {
                    report.failed.insert(key, error.to_string());
                }
            }
        }
        Ok(report)
    }

    /// Stop leading every managed partition and hand each lease back, so a
    /// drained instance's follower takes over at once instead of after the
    /// TTL (RES-8103). The instance stops acknowledging a partition the
    /// moment its lease is forgotten here, before the database is asked, so
    /// a release that fails costs the TTL and never an acknowledgement from a
    /// broker that no longer intends to lead.
    ///
    /// Idempotent: a partition with no lease held is skipped. Nothing here
    /// stops [`Self::tend_leases`] from taking the leases again; the caller
    /// that drains must stop calling it, or call this again each pass.
    pub fn relinquish_leases(&self) -> Result<ReleaseReport> {
        let seat = self.managed.as_ref().ok_or_else(|| {
            Error::invalid("relinquish_leases was called on a broker with no managed log")
        })?;
        let mut report = ReleaseReport::default();
        for (stream, partition) in self.managed_partitions() {
            let Some(lease) = seat.lease(&stream, partition) else {
                continue;
            };
            seat.set_lease(&stream, partition, None);
            let reference = PartitionRef::new(stream.clone(), partition);
            match seat.log.release(&reference, &lease) {
                Ok(true) => report.released.push((stream, partition)),
                Ok(false) => report.superseded.push((stream, partition)),
                Err(error) => {
                    report.failed.insert((stream, partition), error.to_string());
                }
            }
        }
        Ok(report)
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
            let Some(batch) = self.read_batch(&state, stream, partition, cursor)? else {
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

    /// The batch at `offset`: from this broker's disk, or — when the local
    /// segment holding it has been retained away and an archive reader is
    /// installed — hydrated from the archive, verified against its manifest
    /// (FABRIC-8079). Only an offset below the high watermark is ever
    /// hydrated, so the archive can never make an unwritten offset readable.
    fn read_batch(
        &self,
        state: &PartitionState,
        stream: &str,
        partition: u32,
        offset: u64,
    ) -> Result<Option<Batch>> {
        if let Some(batch) = state.log.read(offset)? {
            return Ok(Some(batch));
        }
        match &self.archive {
            Some(reader) if offset < state.log.high_water() => {
                let hydrated = reader.read_indexed(stream, partition, offset)?;
                if let Some(batch) = &hydrated
                    && batch.base_offset != offset
                {
                    return Err(Error::invalid(format!(
                        "{stream}:{partition}: the archive answered offset {offset} with a batch \
                         stamped for offset {}; refusing to serve it",
                        batch.base_offset
                    )));
                }
                Ok(hydrated)
            }
            _ => Ok(None),
        }
    }

    /// Rebuild every empty local partition from the archive (RES-9170): the
    /// broker's disk was lost or replaced, the archive was not, and a
    /// partition that restarted at offset zero beside an archive holding
    /// offsets 0..N would hand those offsets to new records, which the next
    /// archive pass would then write over the manifests of the old ones.
    ///
    /// Each batch is read through the archive's manifest index, which
    /// re-proves its content hash, and appended with the stamp it was archived
    /// with, so its offset, epoch and chain link are exactly the original's
    /// and the first offset this broker then assigns continues the chain.
    /// Replay stops at the first offset the archive does not hold. That is
    /// the end of what was archived: records past `archived_through` were
    /// never safe to release (ADR 0100 §3), so their producers still hold
    /// them and re-send. A partition that already holds any local batch is
    /// left alone, and so is a managed one, whose log the database holds and
    /// [`Self::tend_leases`] follows.
    ///
    /// Refuses to run without an archive reader. Call it before the broker
    /// serves, since it assumes nothing has been produced to the partitions
    /// it rebuilds.
    pub fn rebuild_from_archive(&self) -> Result<RebuildReport> {
        let reader = self.archive.as_ref().ok_or_else(|| {
            Error::invalid(
                "rebuild_from_archive was called on a broker with no archive reader; there is \
                 nothing to rebuild from",
            )
        })?;
        let managed: std::collections::BTreeSet<(String, u32)> =
            self.managed_partitions().into_iter().collect();
        let mut report = RebuildReport::default();
        for (stream, partition_count) in self.declared_streams() {
            for partition in 0..partition_count {
                if managed.contains(&(stream.clone(), partition)) {
                    continue;
                }
                let state = self.partition_state(&stream, partition)?;
                if state.log.high_water() != 0 {
                    continue;
                }
                let mut offset = 0u64;
                while let Some((batch, segment_last)) =
                    reader.read_indexed_with_extent(&stream, partition, offset)?
                {
                    if batch.base_offset != offset {
                        return Err(Error::invalid(format!(
                            "{stream}:{partition}: the archive answered offset {offset} with a \
                             batch stamped for offset {}; refusing to rebuild from it",
                            batch.base_offset
                        )));
                    }
                    require_stamped_for(&batch, partition, &partition_label(&stream, partition))?;
                    let assigned = state.log.append(&batch)?;
                    if assigned != offset {
                        return Err(Error::io(format!(
                            "{stream}:{partition}: replaying archived offset {offset} was \
                             assigned offset {assigned}; the rebuilt log has diverged"
                        )));
                    }
                    // Seal where the archive's segment ended. The log rolls on
                    // size alone, and segments the original broker cut by age
                    // end elsewhere: re-archived under different index keys
                    // they would leave the old keys beside the new, and the
                    // reader's largest-start-at-or-below rule would answer an
                    // offset the archive holds from a stale, shorter manifest
                    // as absent.
                    if offset == segment_last {
                        state.log.seal_active()?;
                    }
                    offset += 1;
                }
                if offset == 0 {
                    continue;
                }
                // Fold what was replayed into the producer table and the
                // chain tail, which `partition_state` computed from an empty
                // log: without them a producer restarting against the rebuilt
                // partition is told its sequence is zero and its next batch
                // chains to nothing.
                self.recover_producers(&stream, partition, &state.log)?;
                let tail = recover_last_batch_hash(&state.log)?;
                state
                    .cursor
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .last_batch_hash = tail;
                report.replayed.insert((stream.clone(), partition), offset);
            }
        }
        Ok(report)
    }

    /// Delete the local copy of every archived, indexed sealed segment that
    /// lies wholly more than `keep_batches` behind its partition's high
    /// watermark, returning how many segments were deleted.
    ///
    /// This is what makes the archive the history and the broker's disk a
    /// bounded hot window ("bounded retention", product direction): without
    /// it a broker's disk is the only bound on how much it keeps. It refuses
    /// to run without an archive reader, because a deleted segment nobody can
    /// hydrate is history lost; and it stops each partition's boundary at the
    /// first segment that is not both archived and indexed, so a segment
    /// archived before the index existed is kept rather than orphaned.
    /// Each stream's window comes from its retention class
    /// ([`hot_window`]): a stream whose class retains nothing beyond its
    /// ring keeps only the batch the next produce chains to, and every other
    /// class keeps `keep_batches`. One global window applied the operator's
    /// figure to a stream whose policy says its records are gone in seconds,
    /// so the log held what the policy had already let go.
    pub fn retain_hot(&self, keep_batches: u64) -> Result<u64> {
        let reader = self.archive.as_ref().ok_or_else(|| {
            Error::invalid(
                "retain_hot was called on a broker with no archive reader; without one a \
                 deleted segment could never be read again",
            )
        })?;
        if keep_batches == 0 {
            return Err(Error::invalid(
                "a hot window of zero batches would delete the segment the next produce chains \
                 to; keep at least one",
            ));
        }
        let declared: Vec<(String, u32, bool, u64)> = {
            let declared = self.declared.lock().unwrap_or_else(|e| e.into_inner());
            declared
                .iter()
                .map(|(name, d)| {
                    (
                        name.clone(),
                        d.partition_count,
                        d.policy.archive_required(),
                        hot_window(d.policy.retention(), keep_batches),
                    )
                })
                .collect()
        };
        let mut deleted = 0u64;
        for (stream, partition_count, archive_required, keep) in declared {
            for partition in 0..partition_count {
                let state = self.partition_state(&stream, partition)?;
                let mut boundary = state.log.high_water().saturating_sub(keep);
                if archive_required {
                    for (start, end, archived) in state.log.sealed_segments()? {
                        if end > boundary {
                            break;
                        }
                        if !archived || !reader.is_indexed(&stream, partition, start)? {
                            boundary = start;
                            break;
                        }
                    }
                }
                deleted += state.log.retain_before(boundary)?;
            }
        }
        Ok(deleted)
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
        // A managed partition's checkpoint lives in the managed store first,
        // so a consumer resumes from it whichever instance leads after a
        // failover (RES-7112); the local copy is what the lag check reads.
        if let Some(seat) = self.managed_seat_for(stream) {
            seat.log
                .commit_offset(group, &PartitionRef::new(stream, partition), offset)?;
        }
        self.checkpoints.put(&key, serde_json::to_value(offset)?)?;
        Ok(offset)
    }

    /// The managed seat, when `stream` is a declared managed stream on a
    /// seated broker. An undeclared stream answers `None` here and is refused
    /// wherever it is refused today, so this adds no new error to any path.
    fn managed_seat_for(&self, stream: &str) -> Option<&ManagedSeat> {
        let managed = self
            .declared
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(stream)
            .is_some_and(|d| d.policy.is_managed());
        if managed { self.managed.as_ref() } else { None }
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
        let local = match self.checkpoints.get(&key)? {
            Some(value) => Some(serde_json::from_value::<u64>(value).map_err(|e| {
                Error::schema(format!("stored checkpoint {key} is not an integer: {e}"))
            })?),
            None => None,
        };
        // For a managed partition the managed checkpoint is the record: a
        // group that committed through the previous leader has nothing on
        // this instance's disk. The larger of the two is answered, because a
        // checkpoint only ever moves forward.
        if let Some(seat) = self.managed_seat_for(stream) {
            let managed = seat
                .log
                .committed_offset(group, &PartitionRef::new(stream, partition))?;
            return Ok(local.max(managed));
        }
        Ok(local)
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
        if super::admission::drops_oldest_for_lag(&policy) {
            return self.drop_oldest_past_lag(stream, partition, &policy);
        }
        if !super::admission::refuses_for_lag(&policy) {
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
                    "group '{group}' trails {stream}:{partition} by {lag} batches, past the \
                     stream's lag limit of {}; the producer is refused until the group catches up",
                    policy.lag_limit()
                )));
            }
        }
        Ok(())
    }

    /// FABRIC-8095: on a `sample_or_shed` stream, a group trailing past the
    /// lag limit does not refuse the producer and does not let the backlog
    /// grow: the partition's sealed segments lying wholly more than
    /// `lag_limit` batches behind the high watermark are deleted, oldest
    /// first, and the new batch is admitted. A fetch of a dropped offset
    /// returns nothing, which the lagging group reads as the gap it is.
    /// Segment granularity means up to one segment past the limit is kept.
    fn drop_oldest_past_lag(
        &self,
        stream: &str,
        partition: u32,
        policy: &StreamPolicy,
    ) -> Result<()> {
        let state = self.partition_state(stream, partition)?;
        let high_watermark = state.log.high_water();
        let suffix = format!("|{stream}|{partition}");
        let mut lagging = false;
        for key in self.checkpoints.keys_with_prefix("")? {
            let Some(group) = key.strip_suffix(&suffix) else {
                continue;
            };
            let Some(committed) = self.committed_offset(group, stream, partition)? else {
                continue;
            };
            if high_watermark.saturating_sub(committed.saturating_add(1)) > policy.lag_limit() {
                lagging = true;
                break;
            }
        }
        if lagging {
            state
                .log
                .retain_before(high_watermark.saturating_sub(policy.lag_limit()))?;
        }
        Ok(())
    }

    /// Charge `batch` against its producer's byte and message quota for the
    /// current window, refusing with the time to wait when either would be
    /// exceeded (FABRIC-049). A refused batch is not charged, so a producer
    /// that backs off as told is not penalised twice. Runs before the
    /// producer table sees the batch: a producer over quota must not be
    /// able to consume sequence numbers it was never allowed to use.
    /// Charge `batch` to its producer's per-second window, or refuse it as
    /// the class decides. On admission, the credit left in the window and
    /// when the window refills (R12-FABRIC-8089): the producer's flow
    /// control reads it off the acknowledgement, so the next batch it
    /// cannot fund waits at the producer instead of being refused here.
    fn charge_quota(
        &self,
        stream: &str,
        batch: &Batch,
    ) -> Result<std::result::Result<ProducerCredit, ProduceRefusal>> {
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
                "producer '{}' is over its quota on stream '{stream}' ({} of {} bytes, {} of {} \
                 messages this second, this batch adds {bytes} and {messages}); retry after \
                 {retry_after_ms} ms",
                batch.producer_id,
                entry.1,
                policy.byte_quota_per_producer(),
                entry.2,
                policy.message_quota_per_producer(),
            ));
            // What "over quota" means is the class's decision, not this
            // function's: a wait for everything that may not lose a record,
            // a shed only for telemetry (FABRIC-7056).
            let refusal =
                super::admission::over_quota(&policy, u64::try_from(retry_after_ms).unwrap_or(0));
            return Ok(Err(ProduceRefusal::new(refusal, error)));
        }
        entry.1 += bytes;
        entry.2 += messages;
        let refills_in_ms = u64::try_from((window_start + QUOTA_WINDOW_NS - now) / 1_000_000)
            .map_err(|_| Error::numeric("the quota window's refill instant is before now"))?;
        Ok(Ok(ProducerCredit {
            bytes: policy.byte_quota_per_producer().saturating_sub(entry.1),
            refills_in_ms,
        }))
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

    /// Refuse a batch carrying a record whose event topic the stream does not
    /// admit, or whose body cannot be read to tell. A stream with no
    /// declared topics admits any.
    fn require_admitted_topics(&self, stream: &str, batch: &Batch) -> Result<()> {
        let schemas = self.schemas.lock().unwrap_or_else(|e| e.into_inner());
        let Some(admitted) = schemas.get(stream).and_then(|s| s.body_schemas.as_ref()) else {
            return Ok(());
        };
        for record in &batch.records {
            let event = record.decode_payload(batch.encoding).map_err(|error| {
                Error::denied(format!(
                    "record {} on stream '{stream}' has a body that cannot be read ({}), so \
                     its topic cannot be checked against the stream's; refusing to append it",
                    record.event_id,
                    error.message()
                ))
            })?;
            let Some(versions) = admitted.get(event.topic.name()) else {
                return Err(Error::denied(format!(
                    "record {} carries topic '{}', which stream '{stream}' does not admit \
                     (it admits {}); refusing to append it (FABRIC-024)",
                    record.event_id,
                    event.topic.name(),
                    admitted.keys().cloned().collect::<Vec<_>>().join(", ")
                )));
            };
            if !versions.contains(&event.schema_version) {
                return Err(Error::denied(format!(
                    "record {} carries topic '{}' at schema version {}, which the schema lock \
                     does not hold for it (it holds {}); refusing to append it (FABRIC-024)",
                    record.event_id,
                    event.topic.name(),
                    event.schema_version,
                    versions
                        .iter()
                        .map(u32::to_string)
                        .collect::<Vec<_>>()
                        .join(", ")
                )));
            }
        }
        Ok(())
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
        // The open is serialised by the partitions lock, held from here to
        // the insert. Two produces racing to a partition's first batch used
        // to build their state outside the lock, and the loser did not get
        // the winner's state: `SegmentLog::open`'s one-writer guard refused
        // its second open, and the `?` returned that refusal to a producer
        // whose batch was valid — a P0 or P1 append refused for nothing but
        // timing. The re-check below is now what decides, before any second
        // open is attempted. Opening is once per partition per process, so
        // holding the table's lock across it costs other partitions a wait
        // at their own first open, not on every produce.
        let mut partitions = self.partitions.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(winner) = partitions.get(&key) {
            return Ok(winner.clone());
        }
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
                caught_up_epoch: None,
            }),
        });
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

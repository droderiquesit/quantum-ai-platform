//! The managed-commit port: P0 and P1 batches committed by a managed
//! consensus before a producer is answered, under a leadership lease the
//! database fences at commit (ADR 0111 §1).
//!
//! The broker core holds a [`ManagedLog`] and never knows what is behind it.
//! `qip-fabricd` supplies the Spanner implementation from its composition
//! root, which holds the token and the transport (ADR 0111, "Dependency
//! direction"); [`MemoryManagedLog`] is the same contract in memory, so every
//! property of the commit path — fencing, catch-up, failover — is tested
//! without Google, and two brokers in one test process can contend for one
//! lease the way two `qip-fabricd` instances in two zones do.
//!
//! # What the port promises, and why each promise is the port's
//!
//! * **[`ManagedLog::commit`] commits only under the lease's current epoch.**
//!   Fencing is by the database, not by a clock: a leader whose lease was
//!   taken while it was paused learns so from the commit's answer
//!   ([`CommitOutcome::Fenced`]), never from its own idea of the time. Two
//!   instances committing for one partition in one epoch is ADR 0111's first
//!   "what would make this wrong", and it is unreachable through this trait
//!   by construction of the transaction, not by care in the caller.
//! * **Offsets are dense.** A commit at an offset other than one past the
//!   last committed is [`CommitOutcome::Conflict`], so a leader that missed a
//!   predecessor's tail cannot write over or past it.
//! * **[`ManagedLog::acquire`] takes an expired lease and bumps its epoch.**
//!   The standby that takes over has a strictly higher epoch than anything
//!   the deposed leader could still commit under.
//! * **An unknown outcome is an error, not a refusal.** A commit whose answer
//!   was lost may have committed. The broker treats it as "unknown" and
//!   catches up from [`ManagedLog::read`] before admitting anything else, so
//!   a producer's retry of that batch is answered as the duplicate it is.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use qip_core::Clock;
use qip_core::error::{Error, Result};

/// The partition a lease and a committed row belong to.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct PartitionRef {
    pub stream: String,
    pub partition: u32,
}

impl PartitionRef {
    pub fn new(stream: impl Into<String>, partition: u32) -> Self {
        Self {
            stream: stream.into(),
            partition,
        }
    }
}

/// A leadership lease this instance holds: who it is and the epoch every
/// commit it makes is fenced by.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Lease {
    pub holder: String,
    pub epoch: u64,
}

/// What [`ManagedLog::acquire`] found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LeaseOutcome {
    /// This holder holds the lease at `epoch` until at least the TTL it asked
    /// for has passed in the database's own time.
    Held(Lease),
    /// Another holder's unexpired lease. This instance is a follower.
    HeldByOther { holder: String, epoch: u64 },
}

/// One committed batch as the managed log stores it: keyed `(stream,
/// partition, offset)`, with the lease epoch it was committed under and the
/// batch's own hash and chain link (ADR 0104), beside its encoded bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommittedBatch {
    pub partition: PartitionRef,
    pub offset: u64,
    pub lease_epoch: u64,
    /// Hex SHA-256 of `bytes`: what a reader re-derives before trusting them.
    pub batch_hash: String,
    /// Hex SHA-256 of the previous batch's encoding, `None` at offset zero.
    pub previous_batch_hash: Option<String>,
    pub bytes: Vec<u8>,
}

impl CommittedBatch {
    /// Refuse a row whose bytes do not hash to the hash it carries — a
    /// substituted or truncated row read back from the managed store.
    pub fn verify(&self) -> Result<()> {
        let actual = qip_core::hash::sha256_hex(&self.bytes);
        if actual != self.batch_hash {
            return Err(Error::invalid(format!(
                "the managed log's row {}:{}@{} does not hash to the hash it carries; refusing \
                 to hydrate a batch the managed store did not commit as these bytes",
                self.partition.stream, self.partition.partition, self.offset
            )));
        }
        Ok(())
    }
}

/// What [`ManagedLog::commit`] answered when it answered at all.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CommitOutcome {
    /// The row is committed; acknowledging the producer is now true.
    Committed,
    /// The lease's epoch is no longer the holder's. This instance has been
    /// deposed and must stop acknowledging this partition.
    Fenced { current_epoch: u64 },
    /// The offset is not one past the last committed. `committed_through` is
    /// one past the last committed offset (zero for an empty partition).
    Conflict { committed_through: u64 },
}

/// ADR 0111 §1's port. See the module documentation for what each method
/// promises and why the promise lives behind the trait.
pub trait ManagedLog: Send + Sync + std::fmt::Debug {
    /// Take, renew or observe the lease on `partition` for `holder`, asking
    /// for `ttl_ms` of the database's own time. Renewing a held lease keeps
    /// its epoch; taking an expired one from anybody else bumps it.
    fn acquire(&self, partition: &PartitionRef, holder: &str, ttl_ms: u64) -> Result<LeaseOutcome>;

    /// Commit `row` iff `lease` is still the holder's at `row.lease_epoch`
    /// and `row.offset` is one past the last committed offset.
    fn commit(&self, lease: &Lease, row: &CommittedBatch) -> Result<CommitOutcome>;

    /// Up to `limit` committed rows of `partition` from `from_offset`,
    /// ascending and dense.
    fn read(
        &self,
        partition: &PartitionRef,
        from_offset: u64,
        limit: u32,
    ) -> Result<Vec<CommittedBatch>>;

    /// One past the last committed offset of `partition`; zero when empty.
    fn committed_through(&self, partition: &PartitionRef) -> Result<u64>;

    /// Record `group`'s checkpoint on `partition`, refusing a backwards move,
    /// so a consumer resumes from the same offset whichever instance leads
    /// after a failover (RES-7112).
    fn commit_offset(&self, group: &str, partition: &PartitionRef, offset: u64) -> Result<()>;

    /// `group`'s checkpoint on `partition`, if it has one.
    fn committed_offset(&self, group: &str, partition: &PartitionRef) -> Result<Option<u64>>;
}

#[derive(Debug, Default)]
struct MemoryState {
    leases: BTreeMap<PartitionRef, (String, u64, i64)>,
    rows: BTreeMap<PartitionRef, Vec<CommittedBatch>>,
    checkpoints: BTreeMap<(String, PartitionRef), u64>,
}

/// The [`ManagedLog`] contract held in one process's memory, timed by the
/// clock it is given.
///
/// Not a deployment target: it survives nothing. It is what makes the commit
/// path, the lease and failover testable in CI (FABRIC-7044) — two brokers
/// sharing one `Arc<MemoryManagedLog>` are two `qip-fabricd` instances
/// sharing one Spanner database. [`Self::partition_from_database`] stands in
/// for a broker that has lost its route to the database.
#[derive(Debug)]
pub struct MemoryManagedLog {
    clock: Arc<dyn Clock>,
    state: Mutex<MemoryState>,
    unreachable: Mutex<std::collections::BTreeSet<String>>,
}

impl MemoryManagedLog {
    pub fn new(clock: Arc<dyn Clock>) -> Self {
        Self {
            clock,
            state: Mutex::new(MemoryState::default()),
            unreachable: Mutex::new(std::collections::BTreeSet::new()),
        }
    }

    /// Cut `holder` off from the database: every lease and commit call it
    /// makes fails as a network partition does, with an unknown outcome;
    /// `cut = false` heals it. Reads carry no holder and are not cut.
    pub fn partition_from_database(&self, holder: &str, cut: bool) {
        let mut unreachable = self.unreachable.lock().unwrap_or_else(|e| e.into_inner());
        if cut {
            unreachable.insert(holder.to_string());
        } else {
            unreachable.remove(holder);
        }
    }

    fn reachable(&self, holder: &str) -> Result<()> {
        if self
            .unreachable
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains(holder)
        {
            return Err(Error::unavailable(format!(
                "'{holder}' cannot reach the managed log (injected partition)"
            )));
        }
        Ok(())
    }

    /// Every lease epoch any row of `partition` was committed under, in
    /// offset order — what a test reads to prove no two holders committed in
    /// one epoch.
    pub fn committed_epochs(&self, partition: &PartitionRef) -> Vec<u64> {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state
            .rows
            .get(partition)
            .map(|rows| rows.iter().map(|row| row.lease_epoch).collect())
            .unwrap_or_default()
    }
}

impl ManagedLog for MemoryManagedLog {
    fn acquire(&self, partition: &PartitionRef, holder: &str, ttl_ms: u64) -> Result<LeaseOutcome> {
        self.reachable(holder)?;
        let now = self.clock.now().as_nanos();
        let ttl_ns = i64::try_from(ttl_ms)
            .ok()
            .and_then(|ms| ms.checked_mul(1_000_000))
            .ok_or_else(|| Error::invalid(format!("a lease TTL of {ttl_ms} ms overflows")))?;
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let next = match state.leases.get(partition) {
            None => (holder.to_string(), 1, now + ttl_ns),
            Some((current, epoch, _)) if current == holder => {
                (holder.to_string(), *epoch, now + ttl_ns)
            }
            Some((current, epoch, expires)) if *expires > now => {
                return Ok(LeaseOutcome::HeldByOther {
                    holder: current.clone(),
                    epoch: *epoch,
                });
            }
            Some((_, epoch, _)) => {
                let bumped = epoch
                    .checked_add(1)
                    .ok_or_else(|| Error::numeric("a lease epoch reached u64::MAX"))?;
                (holder.to_string(), bumped, now + ttl_ns)
            }
        };
        let lease = Lease {
            holder: next.0.clone(),
            epoch: next.1,
        };
        state.leases.insert(partition.clone(), next);
        Ok(LeaseOutcome::Held(lease))
    }

    fn commit(&self, lease: &Lease, row: &CommittedBatch) -> Result<CommitOutcome> {
        self.reachable(&lease.holder)?;
        row.verify()?;
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        match state.leases.get(&row.partition) {
            Some((holder, epoch, _)) if *holder == lease.holder && *epoch == lease.epoch => {}
            Some((_, epoch, _)) => {
                return Ok(CommitOutcome::Fenced {
                    current_epoch: *epoch,
                });
            }
            None => return Ok(CommitOutcome::Fenced { current_epoch: 0 }),
        }
        let rows = state.rows.entry(row.partition.clone()).or_default();
        let committed_through = rows.len() as u64;
        if row.offset != committed_through || row.lease_epoch != lease.epoch {
            return Ok(CommitOutcome::Conflict { committed_through });
        }
        rows.push(row.clone());
        Ok(CommitOutcome::Committed)
    }

    fn read(
        &self,
        partition: &PartitionRef,
        from_offset: u64,
        limit: u32,
    ) -> Result<Vec<CommittedBatch>> {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let Some(rows) = state.rows.get(partition) else {
            return Ok(Vec::new());
        };
        let start = usize::try_from(from_offset).unwrap_or(usize::MAX);
        Ok(rows
            .iter()
            .skip(start)
            .take(limit as usize)
            .cloned()
            .collect())
    }

    fn committed_through(&self, partition: &PartitionRef) -> Result<u64> {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        Ok(state
            .rows
            .get(partition)
            .map_or(0, |rows| rows.len() as u64))
    }

    fn commit_offset(&self, group: &str, partition: &PartitionRef, offset: u64) -> Result<()> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let key = (group.to_string(), partition.clone());
        if let Some(held) = state.checkpoints.get(&key)
            && offset < *held
        {
            return Err(Error::denied(format!(
                "group '{group}' tried to move its managed checkpoint on {}:{} back from {held} \
                 to {offset}",
                partition.stream, partition.partition
            )));
        }
        state.checkpoints.insert(key, offset);
        Ok(())
    }

    fn committed_offset(&self, group: &str, partition: &PartitionRef) -> Result<Option<u64>> {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        Ok(state
            .checkpoints
            .get(&(group.to_string(), partition.clone()))
            .copied())
    }
}

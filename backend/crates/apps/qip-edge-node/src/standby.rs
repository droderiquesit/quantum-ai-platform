//! A warm standby, and the fencing lease that keeps it from ever being a
//! second owner (R12-REFLEX-8069, R12-REFLEX-9117, R12-RES-9166,
//! R12-RES-9167).
//!
//! Terraform could declare a standby node in another zone, and its comment
//! said the standby "monitors the primary's liveness via a fencing token".
//! No code held a token. A standby that came up beside a live primary would
//! have traded as a second primary, and two owners of one venue session place
//! every order twice.
//!
//! # The protocol
//!
//! One record in a [`LeaseStore`]: the epoch, the holder, and the instant the
//! holder last renewed. Every write is a compare-and-swap on the record's
//! version, so of two processes that act on the same reading exactly one
//! succeeds.
//!
//! * The **holder** renews every [`LeaseTiming::renew_every`]. A renewal that
//!   lands grants the cell's [`Fence`] a hold until `renewed_at + ttl`. A
//!   renewal that does not land grants nothing, so the hold lapses on the
//!   cell's own clock at that instant whether or not the keeper ever reaches
//!   its store again — the self-fence that makes a partitioned primary safe.
//! * A **standby** promotes only when all three of the checks §17 names pass,
//!   in this order: *liveness* — the holder has not renewed for `ttl +
//!   margin`, so its own hold lapsed at least `margin` ago; *venue session* —
//!   every venue reports the old holder's session ended ([`VenueSessions`]);
//!   *fencing* — the swap to `epoch + 1` lands. Only then is the cell's fence
//!   granted.
//! * A former holder that reads a higher epoch **fences** itself
//!   permanently for every epoch at or below it.
//!
//! The two windows in which either process may place — the holder's
//! `[renewed_at, renewed_at + ttl)` and the standby's `[renewed_at + ttl +
//! margin, …)` — are disjoint so long as the two clocks disagree by less than
//! the margin. That is the stated assumption; `tests/standby.rs` enumerates
//! interleavings of renewal, promotion, partition and time and asserts at
//! most one fence admits at any instant.
//!
//! A standby runs its passes warm: it holds books, features and strategies,
//! and every order it forms is refused at `Cell::send` under `fenced`. Zero-
//! gap failover is not promised (RES-9167): what a promotion costs is
//! measured per venue and reported on [`Promotion`].
//!
//! # The store
//!
//! [`FileLeaseStore`] is a compare-and-swap over one file, exclusive by a
//! lock file created with `create_new`. It is correct between processes that
//! share one filesystem with atomic `rename` and exclusive create — one host,
//! or a drill. **It is not a cross-zone store**: two zones share no such
//! filesystem, and a network filesystem's exclusive create is not to be
//! trusted with this. A cross-zone pair needs a store with a real conditional
//! write (an object generation precondition, or a Spanner transaction — ADR
//! 0111, 0113); that store is not in this tree, and `main.rs` says so when a
//! lease is configured.

use qip_contracts::venue::VenueId;
use qip_core::error::{Error, Result};
use qip_core::{Clock, Duration, Timestamp};
use qip_edge::fencing::{Fence, FenceStatus};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// The lease file. Unset is a node with no standby, announced at start.
pub const LEASE_VARIABLE: &str = "QIP_FENCING_LEASE";
/// `primary` or `standby`. Required when a lease is configured.
pub const ROLE_VARIABLE: &str = "QIP_NODE_ROLE";
/// This process's name in the lease. Required when a lease is configured,
/// and distinct between the primary and its standby.
pub const NAME_VARIABLE: &str = "QIP_NODE_NAME";

/// The one record the lease is.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LeaseRecord {
    pub epoch: u64,
    pub holder: String,
    /// Nanoseconds since the epoch, as the holder's clock read it.
    pub renewed_at_ns: i64,
}

impl LeaseRecord {
    pub fn renewed_at(&self) -> Timestamp {
        Timestamp::from_nanos(self.renewed_at_ns)
    }
}

/// A store holding one [`LeaseRecord`] under a version.
pub trait LeaseStore: Send + Sync + std::fmt::Debug {
    /// The record and its version, or `None` if no process has ever held.
    fn load(&self) -> Result<Option<(LeaseRecord, u64)>>;
    /// Write `next` if and only if the stored version is `expected` (`None`:
    /// no record exists). `Ok(false)` is a lost race, not an error.
    fn swap(&self, expected: Option<u64>, next: &LeaseRecord) -> Result<bool>;
}

/// An in-process store, for tests and drills.
#[derive(Debug, Default)]
pub struct MemoryLeaseStore {
    slot: Mutex<Option<(LeaseRecord, u64)>>,
}

impl MemoryLeaseStore {
    pub fn new() -> Self {
        Self::default()
    }
}

impl LeaseStore for MemoryLeaseStore {
    fn load(&self) -> Result<Option<(LeaseRecord, u64)>> {
        self.slot
            .lock()
            .map(|slot| slot.clone())
            .map_err(|_| Error::io("the lease store's lock is poisoned"))
    }

    fn swap(&self, expected: Option<u64>, next: &LeaseRecord) -> Result<bool> {
        let mut slot = self
            .slot
            .lock()
            .map_err(|_| Error::io("the lease store's lock is poisoned"))?;
        let current = slot.as_ref().map(|(_, version)| *version);
        if current != expected {
            return Ok(false);
        }
        *slot = Some((next.clone(), current.map_or(1, |version| version + 1)));
        Ok(true)
    }
}

/// On disk: `{"version":n,"record":{…}}`.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Stored {
    version: u64,
    record: LeaseRecord,
}

/// How long a lock file may stand before it is taken to belong to a process
/// that died holding it. Far longer than one swap takes.
const STALE_LOCK: std::time::Duration = std::time::Duration::from_secs(10);

/// The lease as one file beside a lock file. See the module documentation
/// for the one host it is correct on.
#[derive(Debug)]
pub struct FileLeaseStore {
    path: PathBuf,
    lock: PathBuf,
}

impl FileLeaseStore {
    /// A store at `path`, whose directory must exist.
    pub fn at(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        let parent = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        if !parent.is_dir() {
            return Err(Error::invalid(format!(
                "configuration: {LEASE_VARIABLE}={} names a file in a directory that does not \
                 exist; mount the lease directory first",
                path.display()
            )));
        }
        let mut lock = path.clone().into_os_string();
        lock.push(".lock");
        Ok(Self {
            path,
            lock: PathBuf::from(lock),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn read(&self) -> Result<Option<Stored>> {
        match std::fs::read(&self.path) {
            Ok(bytes) => serde_json::from_slice(&bytes).map(Some).map_err(|error| {
                Error::invalid(format!(
                    "the lease at {} is not a lease record ({error}); it is left as it is, and \
                     no process promotes over a record it cannot read",
                    self.path.display()
                ))
            }),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(Error::io(format!(
                "the lease at {} cannot be read: {error}",
                self.path.display()
            ))),
        }
    }

    /// Take the lock, breaking one older than [`STALE_LOCK`].
    fn take_lock(&self) -> Result<bool> {
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&self.lock)
        {
            Ok(_) => Ok(true),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                let stale = std::fs::metadata(&self.lock)
                    .and_then(|meta| meta.modified())
                    .ok()
                    .and_then(|modified| modified.elapsed().ok())
                    .is_some_and(|age| age > STALE_LOCK);
                if stale {
                    let _ = std::fs::remove_file(&self.lock);
                }
                Ok(false)
            }
            Err(error) => Err(Error::io(format!(
                "the lease lock at {} cannot be created: {error}",
                self.lock.display()
            ))),
        }
    }
}

impl LeaseStore for FileLeaseStore {
    fn load(&self) -> Result<Option<(LeaseRecord, u64)>> {
        Ok(self.read()?.map(|stored| (stored.record, stored.version)))
    }

    fn swap(&self, expected: Option<u64>, next: &LeaseRecord) -> Result<bool> {
        if !self.take_lock()? {
            return Ok(false);
        }
        let outcome = (|| {
            let current = self.read()?.map(|stored| stored.version);
            if current != expected {
                return Ok(false);
            }
            let stored = Stored {
                version: current.map_or(1, |version| version + 1),
                record: next.clone(),
            };
            let bytes = serde_json::to_vec(&stored)?;
            let mut temporary = self.path.clone().into_os_string();
            temporary.push(".next");
            let temporary = PathBuf::from(temporary);
            let mut file = std::fs::File::create(&temporary).map_err(|error| {
                Error::io(format!("cannot write {}: {error}", temporary.display()))
            })?;
            file.write_all(&bytes)
                .and_then(|()| file.sync_all())
                .map_err(|error| {
                    Error::io(format!("cannot write {}: {error}", temporary.display()))
                })?;
            std::fs::rename(&temporary, &self.path).map_err(|error| {
                Error::io(format!("cannot install {}: {error}", self.path.display()))
            })?;
            Ok(true)
        })();
        let _ = std::fs::remove_file(&self.lock);
        outcome
    }
}

/// Whether a venue reports a holder's session ended.
pub trait VenueSessions: Send + std::fmt::Debug {
    /// `Ok(true)` when `holder` no longer holds a session at `venue`.
    fn released(&self, venue: &VenueId, holder: &str, now: Timestamp) -> Result<bool>;
}

/// The simulated venue's answer: its session is the process.
///
/// A paper node places against an in-process matching engine, so the old
/// holder's session ended when its process did or when its own hold lapsed —
/// no other process can inherit it or be told about it. Answering `true` is
/// therefore a statement about the simulator and not a check, and the type
/// name says so; a real venue adapter has to implement this against the
/// venue's own account of who is logged on, and `main.rs` refuses a standby
/// on any adapter but the simulator until one does.
#[derive(Debug, Default)]
pub struct SimulatedSessions;

impl VenueSessions for SimulatedSessions {
    fn released(&self, _venue: &VenueId, _holder: &str, _now: Timestamp) -> Result<bool> {
        Ok(true)
    }
}

/// Which side of the pair a process starts as. Only bootstrap differs: a
/// primary may take a lease no process has ever held; a standby never does,
/// so a standby that starts first waits rather than trading.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Primary,
    Standby,
}

impl Role {
    pub fn parse(value: &str) -> Result<Self> {
        match value.trim() {
            "primary" => Ok(Self::Primary),
            "standby" => Ok(Self::Standby),
            other => Err(Error::invalid(format!(
                "configuration: {ROLE_VARIABLE}={other} is neither `primary` nor `standby`"
            ))),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Primary => "primary",
            Self::Standby => "standby",
        }
    }
}

/// The lease's timing. `margin` is the most the two processes' clocks may
/// disagree by; the safety argument needs nothing else.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LeaseTiming {
    pub ttl: Duration,
    pub margin: Duration,
    pub renew_every: Duration,
}

impl LeaseTiming {
    /// Refuses a renewal interval that is not well inside the ttl — a keeper
    /// renewing once per ttl lapses its own hold between renewals — and a
    /// zero margin, which is a promise that two clocks agree exactly.
    pub fn new(ttl: Duration, margin: Duration, renew_every: Duration) -> Result<Self> {
        if margin.as_nanos() <= 0 {
            return Err(Error::invalid(
                "a fencing lease needs a positive margin: it is the bound on clock disagreement \
                 the never-two-owners argument rests on",
            ));
        }
        if renew_every.as_nanos() <= 0 || renew_every.as_nanos().saturating_mul(2) > ttl.as_nanos()
        {
            return Err(Error::invalid(
                "a fencing lease must renew at least twice per ttl, or its own hold lapses \
                 between renewals",
            ));
        }
        Ok(Self {
            ttl,
            margin,
            renew_every,
        })
    }

    /// Three seconds held, two of margin, renewed every half second.
    pub fn deployed() -> Result<Self> {
        Self::new(
            Duration::from_secs(3),
            Duration::from_secs(2),
            Duration::from_millis(500),
        )
    }
}

/// What a standby's promotion cost, venue by venue (RES-9167).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Promotion {
    pub epoch: u64,
    pub from: String,
    /// When the old holder's hold lapsed.
    pub lapsed_at: Timestamp,
    pub promoted_at: Timestamp,
    /// Per venue: from the old hold's lapse to the instant that venue's
    /// session check passed. Measured, never promised.
    pub recovery: BTreeMap<String, Duration>,
}

/// What one tick of the keeper did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LeaseEvent {
    Acquired {
        epoch: u64,
    },
    Renewed {
        epoch: u64,
    },
    /// The swap lost a race; nothing was granted and the next tick re-reads.
    Contended,
    /// Another process holds the lease and is live.
    Standing {
        holder: String,
        epoch: u64,
    },
    /// No process has ever held, and this one is a standby.
    AwaitingPrimary,
    /// The holder is not live, and these venues still report its session.
    AwaitingSessions {
        holder: String,
        venues: Vec<String>,
    },
    Promoted(Promotion),
    /// A higher epoch was observed; this process is fenced.
    Fenced {
        by: u64,
    },
}

/// One process's half of the lease.
#[derive(Debug)]
pub struct LeaseKeeper {
    name: String,
    role: Role,
    timing: LeaseTiming,
    store: Arc<dyn LeaseStore>,
    fence: Fence,
    venues: Vec<VenueId>,
    sessions: Box<dyn VenueSessions>,
    held: Option<u64>,
}

impl LeaseKeeper {
    pub fn new(
        name: impl Into<String>,
        role: Role,
        timing: LeaseTiming,
        store: Arc<dyn LeaseStore>,
        fence: Fence,
        venues: Vec<VenueId>,
        sessions: Box<dyn VenueSessions>,
    ) -> Result<Self> {
        let name = name.into();
        if name.trim().is_empty() {
            return Err(Error::invalid(format!(
                "configuration: {NAME_VARIABLE} is empty; the lease names its holder, and a \
                 primary and standby with one name cannot be told apart"
            )));
        }
        Ok(Self {
            name,
            role,
            timing,
            store,
            fence,
            venues,
            sessions,
            held: None,
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn role(&self) -> Role {
        self.role
    }

    pub fn fence(&self) -> &Fence {
        &self.fence
    }

    pub fn timing(&self) -> LeaseTiming {
        self.timing
    }

    /// One step of the protocol at `now`. An `Err` is a store that could not
    /// be read or written; nothing was granted, so the hold lapses on time.
    pub fn tick(&mut self, now: Timestamp) -> Result<LeaseEvent> {
        let current = self.store.load()?;
        let Some((record, version)) = current else {
            if self.role == Role::Standby {
                return Ok(LeaseEvent::AwaitingPrimary);
            }
            let next = LeaseRecord {
                epoch: 1,
                holder: self.name.clone(),
                renewed_at_ns: now.as_nanos(),
            };
            if !self.store.swap(None, &next)? {
                return Ok(LeaseEvent::Contended);
            }
            self.fence.grant(1, now.saturating_add(self.timing.ttl))?;
            self.held = Some(1);
            return Ok(LeaseEvent::Acquired { epoch: 1 });
        };

        // Holder, renewing.
        if self.held == Some(record.epoch) && record.holder == self.name {
            let next = LeaseRecord {
                renewed_at_ns: now.as_nanos(),
                ..record.clone()
            };
            if !self.store.swap(Some(version), &next)? {
                return Ok(LeaseEvent::Contended);
            }
            self.fence
                .grant(record.epoch, now.saturating_add(self.timing.ttl))?;
            return Ok(LeaseEvent::Renewed {
                epoch: record.epoch,
            });
        }

        // Somebody else's epoch. A former holder fences itself first.
        if let Some(held) = self.held
            && record.epoch > held
        {
            self.fence.fence(record.epoch);
            self.held = None;
            return Ok(LeaseEvent::Fenced { by: record.epoch });
        }
        // A process that holds nothing still records the epoch it has seen,
        // so a lapse of its fence can never be read as an old grant.
        if self.held.is_none() {
            self.fence.fence(record.epoch);
        }

        // Liveness: the holder's own hold lapsed at `renewed_at + ttl`, and
        // the margin past that is what two disagreeing clocks may need.
        let lapsed_at = record.renewed_at().saturating_add(self.timing.ttl);
        let deadline = lapsed_at.saturating_add(self.timing.margin);
        if now < deadline {
            return Ok(LeaseEvent::Standing {
                holder: record.holder,
                epoch: record.epoch,
            });
        }

        // Venue sessions: every venue must report the old session ended.
        let mut still = Vec::new();
        let mut recovery = BTreeMap::new();
        for venue in &self.venues {
            if self.sessions.released(venue, &record.holder, now)? {
                recovery.insert(venue.as_str().to_string(), now.since(lapsed_at));
            } else {
                still.push(venue.as_str().to_string());
            }
        }
        if !still.is_empty() {
            return Ok(LeaseEvent::AwaitingSessions {
                holder: record.holder,
                venues: still,
            });
        }

        // Fencing: the swap to the next epoch is the promotion.
        let epoch = record.epoch.saturating_add(1);
        let next = LeaseRecord {
            epoch,
            holder: self.name.clone(),
            renewed_at_ns: now.as_nanos(),
        };
        if !self.store.swap(Some(version), &next)? {
            return Ok(LeaseEvent::Contended);
        }
        self.fence
            .grant(epoch, now.saturating_add(self.timing.ttl))?;
        self.held = Some(epoch);
        Ok(LeaseEvent::Promoted(Promotion {
            epoch,
            from: record.holder,
            lapsed_at,
            promoted_at: now,
            recovery,
        }))
    }
}

/// The keeper on a thread of its own: the decision thread opens no file.
#[derive(Debug)]
pub struct LeaseThread {
    status: Arc<Mutex<Option<LeaseEvent>>>,
    promotions: Arc<Mutex<Vec<Promotion>>>,
    stop: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl LeaseThread {
    /// Tick `keeper` every `renew_every` on `clock` until dropped.
    pub fn spawn(mut keeper: LeaseKeeper, clock: Arc<dyn Clock>) -> Result<Self> {
        let status = Arc::new(Mutex::new(None));
        let promotions = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let interval = std::time::Duration::from_nanos(
            u64::try_from(keeper.timing().renew_every.as_nanos()).unwrap_or(500_000_000),
        );
        let (thread_status, thread_promotions, thread_stop) = (
            Arc::clone(&status),
            Arc::clone(&promotions),
            Arc::clone(&stop),
        );
        let handle = std::thread::Builder::new()
            .name("qip-fencing-lease".to_string())
            .spawn(move || {
                let mut last: Option<LeaseEvent> = None;
                while !thread_stop.load(Ordering::Relaxed) {
                    let now = clock.now();
                    match keeper.tick(now) {
                        Ok(event) => {
                            if let LeaseEvent::Promoted(promotion) = &event {
                                eprintln!(
                                    "qip-edge-node: promoted to fencing epoch {} over {}; per-venue \
                                     recovery {:?}",
                                    promotion.epoch, promotion.from, promotion.recovery
                                );
                                if let Ok(mut all) = thread_promotions.lock() {
                                    all.push(promotion.clone());
                                }
                            } else if !matches!(event, LeaseEvent::Renewed { .. })
                                && last.as_ref() != Some(&event)
                            {
                                eprintln!("qip-edge-node: fencing lease: {event:?}");
                            }
                            if let Ok(mut slot) = thread_status.lock() {
                                *slot = Some(event.clone());
                            }
                            last = Some(event);
                        }
                        Err(error) => eprintln!(
                            "qip-edge-node: fencing lease store: {}; the hold lapses on time",
                            error.message()
                        ),
                    }
                    std::thread::sleep(interval);
                }
            })
            .map_err(|error| Error::io(format!("cannot start the lease keeper: {error}")))?;
        Ok(Self {
            status,
            promotions,
            stop,
            handle: Some(handle),
        })
    }

    pub fn latest(&self) -> Option<LeaseEvent> {
        self.status.lock().ok().and_then(|slot| slot.clone())
    }

    pub fn promotions(&self) -> Vec<Promotion> {
        self.promotions
            .lock()
            .map(|all| all.clone())
            .unwrap_or_default()
    }
}

impl Drop for LeaseThread {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

/// The node's lease configuration, read in `main.rs`.
#[derive(Debug)]
pub struct StandbySettings {
    pub path: PathBuf,
    pub role: Role,
    pub name: String,
}

impl StandbySettings {
    /// `None` when no lease is configured. A role or name with no lease is
    /// refused: a standby that holds no lease is a second primary.
    pub fn read(
        lease: Option<&str>,
        role: Option<&str>,
        name: Option<&str>,
    ) -> Result<Option<Self>> {
        fn present(value: Option<&str>) -> Option<&str> {
            value.map(str::trim).filter(|v| !v.is_empty())
        }
        let (lease, role, name) = (present(lease), present(role), present(name));
        let Some(path) = lease else {
            if role.is_some() || name.is_some() {
                return Err(Error::invalid(format!(
                    "configuration: {ROLE_VARIABLE} or {NAME_VARIABLE} is set and \
                     {LEASE_VARIABLE} is not; a standby with no fencing lease would trade as a \
                     second primary. Set {LEASE_VARIABLE} or unset both"
                )));
            }
            return Ok(None);
        };
        let role = Role::parse(role.ok_or_else(|| {
            Error::invalid(format!(
                "configuration: {LEASE_VARIABLE} is set and {ROLE_VARIABLE} is not; say whether \
                 this process is the `primary` or the `standby`"
            ))
        })?)?;
        let name = name.ok_or_else(|| {
            Error::invalid(format!(
                "configuration: {LEASE_VARIABLE} is set and {NAME_VARIABLE} is not; the lease \
                 names its holder"
            ))
        })?;
        Ok(Some(Self {
            path: PathBuf::from(path),
            role,
            name: name.to_string(),
        }))
    }
}

/// Whether `status` admits, for the health body.
pub fn fence_word(status: &FenceStatus) -> &'static str {
    match status {
        FenceStatus::Unowned => "unowned",
        FenceStatus::Holding { .. } => "holding",
        FenceStatus::Lapsed { .. } => "lapsed",
        FenceStatus::Fenced { .. } => "fenced",
    }
}

//! The append-only event log.
//!
//! Serves three purposes at once: durable history, replay source, and audit
//! trail. Each record commits to its predecessor's hash, so removing or editing
//! a record breaks the chain at that point and every point after it —
//! [`EventLog::verify_chain`] finds exactly where.
//!
//! # Retention is a bound, and the bound has three tiers
//!
//! The in-memory index used to be capped only if a caller asked for a cap, and
//! no production caller ever did: `qip-kernel` builds the log through
//! `EventLog::open`, which left the capacity unset. An event log with no
//! ceiling is a process that dies of memory during exactly the incident it
//! exists to explain.
//!
//! So retention is now always bounded, and what happens at the ceiling depends
//! on what the record is — which every record states through its topic's
//! declared §22.1 retention class, [`Topic::retention_class`] (ADR 0089,
//! blueprint §56.4 rule 33). Until that ADR the tier was derived from the
//! topic's *group* with two topics named by exception, so what the log kept
//! was a fact about which stage of the cycle a topic belonged to rather
//! than a fact about the record; now the log reads the class at both seams
//! (`make_room` and `roll`) and nothing else:
//!
//! 1. **Replaceable records go first.** A record whose class is replaceable
//!    ([`crate::retention::Retention::is_replaceable`]: transient ticks,
//!    quotes and book deltas; derived state such as computed features and
//!    the world model's change stream) is replaced by the next one within
//!    milliseconds or rebuilt from what produced it. These are evicted
//!    oldest-first and counted in [`EventLog::evicted_replaceable`].
//! 2. **Then other observations, reluctantly.** Trades, bars, corporate
//!    actions, news and alternative data — the referenced and series classes
//!    — are not replaceable in the same breath, but they are *observations
//!    of the outside world*: re-readable from the source, and still on disk
//!    for a file-backed log, because the file is the durable copy and this
//!    index is a working set. They are evicted only after every replaceable
//!    record has already gone, and counted separately in
//!    [`EventLog::evicted_observations`] — a non-zero count is the signal
//!    that retention is too small for the traffic, which the first counter
//!    alone would not distinguish.
//! 3. **The audit trail is never evicted; the append is refused instead.**
//!    A record whose class is permanent
//!    ([`crate::retention::Retention::is_permanent`]: every verdict, order,
//!    fill, hypothesis, lesson, and the platform's own lifecycle and control
//!    record) is why this log exists. Dropping one to make room for the next
//!    would leave the platform acting with no account of what it did, which
//!    is worse than stopping. When nothing evictable remains,
//!    [`EventLog::append`] returns a refusal naming the fix and the incoming
//!    record's class, and writes nothing, in memory or to the file.
//!
//! # What this does not fix
//!
//! Two limits are stated here rather than papered over:
//!
//! * **Eviction breaks in-memory chain verification from genesis.** The chain
//!   is over what was written; evicting a record from the middle of the
//!   retained span leaves [`EventLog::verify_chain`] reporting the first link
//!   whose predecessor is gone. That was already true of any capped log and
//!   is a further reason the audit-class records are never evicted. For a
//!   file-backed log the file still verifies end to end, and
//!   [`EventLog::verify_retained_chain`] is the question to ask of the
//!   retained span: it recomputes every retained record's hash and holds
//!   each link only across the sequences the log still has, reading a gap
//!   as the eviction it is. The chain is unkeyed SHA-256 (ADR 0043's
//!   anchoring gap), so what either check proves is that the bytes read are
//!   the bytes written, not that nobody with write access rewrote them.
//! * **The JSONL file is still append-only.** Nothing here truncates or rolls
//!   it, so a file-backed log bounds memory but not disk — the snapshot
//!   window below rolls the *index*, and a reopened file is rolled again as
//!   it loads, but every line ever appended is still on disk. Segmenting the
//!   file — sealing a segment, recording its final hash as the next segment's
//!   genesis, and archiving it — is the remaining half of retention and is a
//!   separate change; it needs an ADR because it changes what "the log" means
//!   to a replay. Until then, disk is bounded only by the refusal above and by
//!   whatever the deployment archives out of band.
//!
//! # The snapshot window rolls replaceable records by age
//!
//! The count ceiling above bounds the working set by *size*; nothing bounded
//! it by *age*. A quiet deployment — one whose traffic never reached the
//! ceiling — held every book snapshot it had ever recorded, so what the
//! platform kept depended on how busy it had been rather than on any stated
//! retention, and the blueprint's stated retention for event-anchored book
//! state is ninety days rolling (§54.2, §22.1). So a second bound can now run
//! beside the first: a record whose declared class is replaceable
//! ([`Topic::retention_class`]) and which is
//! older than [`EventLog::snapshot_window`] behind the newest instant the log
//! has recorded is rolled off the index and counted in
//! [`EventLog::rolled_by_age`]. Four things about its shape are deliberate.
//!
//! * **It is off unless a caller sets it, and that is a constraint rather
//!   than a preference.** `Platform::resume_fabric` in `qip-kernel` and
//!   `FabricJournal::resume` in `qip-capital-fabric` rebuild the fabric's
//!   state by replaying the *retained* records from genesis, and each
//!   refuses a log whose retained sequences have a gap — by design, because
//!   a slice from the middle of a log cannot prove what came before it. A
//!   roll leaves exactly such a gap. With the window on by default, a
//!   platform holding fabric records could not restart once any snapshot in
//!   its log was ninety days old, and `qip-cli replay` refused a one-cycle
//!   journal the moment a platform assembled on it with the real clock
//!   appended one record (three tests, 2026-09-19). Until those consumers
//!   re-anchor across evictions the way [`EventLog::verify_retained_chain`]
//!   does, only a caller that knows its log holds no fabric records may set
//!   the window, and no composition root does today. Stated here so nobody
//!   flips the default to make the bound "reached".
//! * **Only the replaceable class rolls.** A trade, a bar or a filing is an
//!   observation the fallback series keeps for three years, and the audit
//!   trail is never touched by any retention path in this module — the roll
//!   filters on the same predicate the pressure eviction spends first, so
//!   the two bounds cannot disagree about what is cheap to lose.
//! * **Age is measured against the log's own newest `recorded_at`, not a
//!   clock.** A library that read the wall clock could not be replayed, and
//!   a roll driven by the caller's clock would need a caller. The roll runs
//!   at most once per [`ROLL_CADENCE`] of recorded time, so a hot append
//!   path does not scan a million records per event, and only a record at
//!   or past the due instant triggers it — which is what makes that record
//!   the newest the log holds, and what stops an out-of-order older record
//!   from pulling the window back.
//! * **A replay across the roll still verifies.** The chain is over what was
//!   written, so a rolled record leaves a gap in the retained sequence
//!   exactly as a pressure eviction does, and [`EventLog::verify_retained_chain`]
//!   reads it as one: every retained record still hashes to what it claimed
//!   and every retained link still holds.

use qip_core::error::{Error, Result};
use qip_core::hash::sha256_hex;
use qip_core::{CorrelationId, Duration, EventId, Timestamp};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use crate::envelope::{AnyEvent, canonical_json};
use crate::topic::{Topic, TopicGroup};

/// One record as written to storage: the event plus its chain linkage.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LogRecord {
    pub sequence: u64,
    /// Hash of the previous record, or 64 zeroes for the first.
    pub previous_hash: String,
    /// Hash over the sequence, previous hash and canonical event.
    pub record_hash: String,
    pub event: AnyEvent,
}

/// The genesis predecessor hash.
pub const GENESIS_HASH: &str = "0000000000000000000000000000000000000000000000000000000000000000";

/// Append-only, hash-chained event storage.
///
/// Records are held in memory and optionally mirrored to a JSONL file. The file
/// is the durable copy; the in-memory index is what queries run against.
#[derive(Debug)]
pub struct EventLog {
    records: Vec<LogRecord>,
    by_correlation: BTreeMap<String, Vec<usize>>,
    by_topic: BTreeMap<Topic, Vec<usize>>,
    by_event_id: BTreeMap<String, usize>,
    /// The dedup key of every retained record that carries an explicit
    /// idempotency key, so a writer can ask whether the fact it is about to
    /// append is already on the record. Only explicit keys are indexed: a
    /// record without one dedups on its payload hash, which the event id
    /// index already makes unique, and indexing a million hashes to answer a
    /// question nobody asks of them would be memory spent on nothing.
    by_idempotency: BTreeSet<String>,
    path: Option<PathBuf>,
    /// Whether this log was opened by [`Self::inspect`], and so refuses
    /// `append`. An inspected log carries a file's records and no path, and
    /// until 2026-09-12 that was all that distinguished it from an in-memory
    /// log: an append reached memory silently, minted the next sequence
    /// over the file's chain and put nothing on disk, so a tool that
    /// inspected a journal and then, by mistake, wrote to it held a chain
    /// the file did not, with no error to say so. The marker makes the
    /// read-only intent a refusal rather than a convention.
    inspected: bool,
    /// Cap on retained records. Always set: an unbounded default is how this
    /// grew without limit in every production construction.
    capacity: usize,
    /// Whether an appended record is on the platter before `append` returns.
    durability: Durability,
    evicted_replaceable: u64,
    evicted_observations: u64,
    appends_refused: u64,
    /// How long a replaceable record is kept behind the newest recorded
    /// instant, or `None` for a log that never rolls — the default, for the
    /// reason the module doc gives.
    snapshot_window: Option<Duration>,
    /// The recorded instant at or after which the next roll runs. `None`
    /// until the first record arrives.
    next_roll_due: Option<Timestamp>,
    rolled_by_age: u64,
    /// The highest sequence ever indexed, and the hash of the record that
    /// carried it. Held separately from `records` because eviction can empty
    /// the tail: deriving either from the last retained record would restart
    /// the sequence at 1 and re-anchor the chain at genesis, which would make
    /// two different records share a sequence number and the break invisible.
    last_sequence: u64,
    last_hash: String,
    /// The handle holding the exclusive advisory lock on the file, for a
    /// file-backed log; released when the log is dropped. Two processes
    /// appending to one file would each mint the next sequence from the
    /// tail they loaded and write two records under it, and a campaign id
    /// minted from that tail in each would collide — a claim of uniqueness
    /// across restarts that was silently false across concurrent writers
    /// until 2026-09-12. `None` for an in-memory log.
    lock: Option<std::fs::File>,
}

/// Records retained by default.
///
/// Chosen to match the event bus's drain and queue ceilings, so a bus that
/// will accept a million events meets a log that will retain them, and no
/// deployment discovers a new limit merely by upgrading. It is a ceiling, not
/// a target: a long-lived file-backed deployment should set a smaller one
/// deliberately, because a million retained records is a gigabyte-scale
/// working set.
pub const DEFAULT_CAPACITY: usize = 1_000_000;

/// The blueprint's ninety-day rolling snapshot window (§54.2), which its own
/// arithmetic sizes at 4.5 million events for a busy day's order flow. Not a
/// default: no log rolls until [`EventLog::with_snapshot_window`] is called,
/// for the reason the module doc gives. Pass this where a log is known to
/// hold no fabric records; widen it where a model class demonstrably needs
/// longer — the blueprint's stated revisit condition — and say why at the
/// call site.
pub const SNAPSHOT_WINDOW: Duration = Duration::from_days(90);

/// How much recorded time passes between two scans for records past the
/// window. One day against a ninety-day window means a record lives at most
/// ninety-one days, which is the granularity "ninety days rolling" was
/// written at; a scan per append would read the whole index on every tick.
/// A window shorter than a day scans every window instead.
pub const ROLL_CADENCE: Duration = Duration::from_days(1);

/// Whether an appended record has reached the disk when `append` returns.
///
/// The same choice `qip_storage`'s engine offers, stated the same way,
/// because it is the same question: a write that has reached the operating
/// system has not reached the platter, and the difference only shows up when
/// the power does.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Durability {
    /// `fsync` the file before returning. The default, because this log is
    /// the platform's evidence: a decision record that a power cut can
    /// silently remove is not a record anybody can rely on afterwards.
    #[default]
    Synchronous,
    /// Return once the operating system has the bytes. Survives `kill -9`;
    /// does not survive power loss. Choose it only where losing the last
    /// records is acceptable and say why at the call site.
    OsBuffered,
}

impl Durability {
    /// Whether an appended record survives loss of power.
    pub fn survives_power_loss(self) -> bool {
        matches!(self, Self::Synchronous)
    }
}

/// The refusal for a file another handle holds — one text for the writer's
/// open and the inspection, so an operator reading either learns the same
/// two things: who holds it, and what to do instead. The CLI's replay
/// passes it through unwrapped; a refusal wrapped in "not an event log this
/// platform wrote", which is what happened until 2026-09-12, sent an
/// operator looking for a corrupt file when the file was merely in use.
fn held_by_another(path: &Path) -> String {
    format!(
        "the event log at {} is held by another process (or another handle in this one); a \
         second writer would mint the same sequences from the same tail and the file would \
         hold two records under one number, and a reader would read a record mid-append. \
         Stop the process that holds it, or point this one at its own log; to inspect a log \
         a running node holds, copy the file first and inspect the copy",
        path.display()
    )
}

impl Default for EventLog {
    fn default() -> Self {
        Self::in_memory()
    }
}

impl EventLog {
    pub fn in_memory() -> Self {
        Self {
            records: Vec::new(),
            by_correlation: BTreeMap::new(),
            by_topic: BTreeMap::new(),
            by_event_id: BTreeMap::new(),
            by_idempotency: BTreeSet::new(),
            path: None,
            inspected: false,
            capacity: DEFAULT_CAPACITY,
            durability: Durability::Synchronous,
            evicted_replaceable: 0,
            evicted_observations: 0,
            appends_refused: 0,
            snapshot_window: None,
            next_roll_due: None,
            rolled_by_age: 0,
            last_sequence: 0,
            last_hash: GENESIS_HASH.to_string(),
            lock: None,
        }
    }

    /// Open a file-backed log at the default capacity, loading any existing
    /// records.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Self::open_with_capacity(path, DEFAULT_CAPACITY)
    }

    /// Open a file-backed log with an explicit retention ceiling.
    ///
    /// The ceiling has to be given here rather than chained afterwards: a file
    /// holding more audit records than the default retains cannot be loaded at
    /// the default at all, and `EventLog::open(p)?.with_capacity(n)` would have
    /// refused before the caller's larger ceiling was ever applied.
    ///
    /// # One process per file
    ///
    /// The file is taken under an exclusive advisory lock before a line of
    /// it is read, and a log another handle still holds — in another
    /// process, or elsewhere in this one — is refused rather than loaded.
    /// Two writers on one file would each load the same tail, each mint
    /// the next sequence from it and each append a record under that
    /// sequence; the file would then hold two records with one number, and
    /// every fact derived from the tail — a campaign id, a chain link —
    /// would be minted twice. The lock is held for the life of the log and
    /// released when it is dropped, so a crashed process releases it with
    /// its descriptors. It is advisory: it holds against anything that
    /// opens the file through this type, and against nothing that writes
    /// the bytes some other way. It is proven so on Unix only — `flock`
    /// semantics, exercised by the test suite on Linux, which is the only
    /// platform this workspace is built and deployed on (an Alpine image);
    /// there is no Windows CI, and what `std` maps the call to there has
    /// not been exercised here.
    ///
    /// This open is a *writer's* open: the file is created if absent, and
    /// opened for append, so it cannot succeed on a read-only mount. A
    /// reader that only wants to look at a log — an archived one, or one a
    /// running node holds — goes through [`Self::inspect`], which creates
    /// nothing, writes nothing, and takes the *shared* side of the same
    /// lock so that it is refused while a writer holds the file rather than
    /// reading a record mid-append.
    pub fn open_with_capacity(path: impl AsRef<Path>, capacity: usize) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let mut log = Self::in_memory().with_capacity(capacity)?;
        log.path = Some(path.clone());
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let handle = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)?;
        match handle.try_lock() {
            Ok(()) => {}
            Err(std::fs::TryLockError::WouldBlock) => {
                return Err(Error::denied(held_by_another(&path)));
            }
            Err(std::fs::TryLockError::Error(error)) => return Err(error.into()),
        }
        log.lock = Some(handle);
        log.load(std::fs::File::open(&path)?)?;
        Ok(log)
    }

    /// Load a log for inspection only, at the default capacity.
    ///
    /// See [`Self::inspect_with_capacity`].
    pub fn inspect(path: impl AsRef<Path>) -> Result<Self> {
        Self::inspect_with_capacity(path, DEFAULT_CAPACITY)
    }

    /// Load a log for inspection only: read-only, never created, and
    /// refused while a writer holds the file.
    ///
    /// The returned log is *not* file-backed. It carries the records the
    /// file held at the instant it was read and no path, and it refuses
    /// [`Self::append`] by name — it is the shape for a tool that checks a
    /// journal, not one that resumes it, and until 2026-09-12 an append to
    /// it reached memory silently, which is a chain the file does not hold
    /// with nothing to say so. Three things distinguish it from
    /// [`Self::open_with_capacity`], each of which the CLI's replay needed
    /// and the writer's open could not give it until 2026-09-12:
    ///
    /// * **It never creates the file.** The writer's open creates an absent
    ///   path and returns an empty log, and a checker pointed at a mistyped
    ///   path would then have verified nothing against nothing — and left
    ///   an empty file behind where the operator would next look. An absent
    ///   path is refused by name here.
    /// * **It opens the file read-only**, so it succeeds on a read-only
    ///   mount — which is where an archived journal is kept on purpose. The
    ///   writer's open, which needs append, reported such a mount as an
    ///   I/O failure that the CLI then relabelled as "not an event log".
    /// * **It takes the shared side of the advisory lock**, for the read
    ///   and no longer. A writer holds the exclusive side for its life, so a
    ///   log a running node is appending to is refused with a message that
    ///   says so — a reader of a file mid-append reads a partial record, and
    ///   a refusal that names the holder is the honest answer where "corrupt
    ///   record at line n" would send an operator looking for damage that
    ///   is not there. The shared lock is released before this returns, so
    ///   an inspection never stops a node from starting afterwards; a node
    ///   that tries to open the file *during* the read is refused for that
    ///   instant and nothing else.
    ///
    /// The lock is advisory on the same terms as the writer's, and proven
    /// on Unix only; see [`Self::open_with_capacity`].
    pub fn inspect_with_capacity(path: impl AsRef<Path>, capacity: usize) -> Result<Self> {
        let path = path.as_ref();
        let mut log = Self::in_memory().with_capacity(capacity)?;
        log.inspected = true;
        let file = match std::fs::File::open(path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(Error::not_found(format!(
                    "no event log at {}; an inspection reads a log this platform wrote and \
                     will not create one",
                    path.display()
                )));
            }
            Err(error) => return Err(error.into()),
        };
        match file.try_lock_shared() {
            Ok(()) => {}
            Err(std::fs::TryLockError::WouldBlock) => {
                return Err(Error::denied(held_by_another(path)));
            }
            Err(std::fs::TryLockError::Error(error)) => return Err(error.into()),
        }
        log.load(file)?;
        // `file` — and the shared lock with it — is released here, before
        // the log is handed back: an inspection that outlived its read
        // would refuse the next writer for as long as the caller held the
        // result, which is a checker stopping a node.
        Ok(log)
    }

    /// Index every record `file` holds, in file order, refusing a file that
    /// is not an intact append-only log.
    ///
    /// Shared by the writer's open and the inspection so that the two cannot
    /// disagree about what a loadable file is: a refusal one applied and the
    /// other did not would let a file the node refuses to resume be
    /// inspected as sound, or the reverse.
    fn load(&mut self, file: std::fs::File) -> Result<()> {
        let mut expected_sequence: u64 = 1;
        for (line_number, line) in BufReader::new(file).lines().enumerate() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            let record: LogRecord = serde_json::from_str(&line).map_err(|e| {
                Error::schema(format!(
                    "corrupt log record at line {}: {e}",
                    line_number + 1
                ))
            })?;
            // The file is append-only and every append takes the next
            // sequence, so a file whose sequences are not 1, 2, 3, … has
            // had a line removed, duplicated or renumbered. Refused
            // here, before any hash is looked at, because
            // `verify_retained_chain` reads a sequence gap as an
            // eviction this process performed — which it can only be
            // once the file it loaded from had none.
            if record.sequence != expected_sequence {
                return Err(Error::schema(format!(
                    "log record at line {} carries sequence {} where {} was expected; the \
                     file is append-only and its sequences are contiguous from 1, so a gap \
                     or a repeat means a line was removed, duplicated or renumbered. Restore \
                     the file the log was written to, or archive it and start a new one",
                    line_number + 1,
                    record.sequence,
                    expected_sequence
                )));
            }
            expected_sequence = expected_sequence.saturating_add(1);
            // Same duplicate-id refusal as a live append: a file that
            // reused an id (corruption, a hand edit, two processes
            // appending to the same path) must fail to load rather than
            // load with `by_event_id` silently pointing at only the
            // later of the two records.
            self.reject_duplicate_event_id(record.event.event_id.as_str())?;
            // The same seam the append path uses, before the count ceiling
            // is consulted, so a stale snapshot is counted as rolled rather
            // than as evicted under a pressure that never existed. No
            // constructor sets a window before loading, so today this rolls
            // nothing here and `with_snapshot_window` does the work after
            // the load; it stays so that the two paths cannot diverge if
            // one day one does.
            self.roll_if_due(record.event.recorded_at);
            // Make room before indexing, so loading a file larger than the
            // ceiling never puts the whole file in memory first — which is
            // the failure the ceiling exists to prevent, arriving at
            // start-up instead of during the run.
            self.make_room(record.event.topic)?;
            self.index(record);
        }
        Ok(())
    }

    /// Trade the durability guarantee for throughput, deliberately.
    pub fn with_durability(mut self, durability: Durability) -> Self {
        self.durability = durability;
        self
    }

    /// What this log promises about an appended record.
    pub fn durability(&self) -> Durability {
        self.durability
    }

    /// Bound the log's retained records.
    ///
    /// Zero is refused rather than read as one. A log retaining a single
    /// record refuses the second audit record it is given, so the platform
    /// would stop at its first decision — and a log silently promoted from
    /// zero to one would do the same while claiming the operator asked for it.
    /// Neither is a retention policy; both are configuration mistakes, and the
    /// one that stops at construction is the one somebody can fix.
    pub fn with_capacity(mut self, capacity: usize) -> Result<Self> {
        if capacity == 0 {
            return Err(Error::invalid(
                "an event log with zero capacity retains nothing and refuses every audit record; \
                 give with_capacity the number of records this deployment should hold in memory",
            ));
        }
        self.capacity = capacity;
        Ok(self)
    }

    /// The retention ceiling in force.
    pub const fn capacity(&self) -> usize {
        self.capacity
    }

    /// Bound how long a replaceable record is retained behind the newest
    /// recorded instant. See the module doc's snapshot-window section for
    /// what this must not be set on.
    ///
    /// Applied at once to whatever the log already holds, measured from the
    /// newest instant among those records, so `EventLog::open(p)?
    /// .with_snapshot_window(w)?` arrives at the working set the writer held
    /// rather than at the whole file; after that the roll runs on the
    /// append path as records arrive.
    ///
    /// A zero or negative window is refused rather than read as "roll
    /// everything" or "roll nothing": either reading is a retention policy
    /// nobody wrote down, and the refusal at construction is the one
    /// somebody can fix.
    pub fn with_snapshot_window(mut self, window: Duration) -> Result<Self> {
        if window <= Duration::ZERO {
            return Err(Error::invalid(format!(
                "an event log snapshot window of {} nanoseconds retains no replaceable record \
                 at all; give with_snapshot_window a positive duration — the blueprint's \
                 figure is ninety days",
                window.as_nanos()
            )));
        }
        self.snapshot_window = Some(window);
        self.next_roll_due = None;
        if let Some(newest) = self.records.iter().map(|r| r.event.recorded_at).max() {
            self.roll_if_due(newest);
        }
        Ok(self)
    }

    /// How long a replaceable record is retained behind the newest recorded
    /// instant, or `None` for a log that never rolls.
    pub const fn snapshot_window(&self) -> Option<Duration> {
        self.snapshot_window
    }

    /// Replaceable records rolled off the index because they aged past the
    /// snapshot window. Counted apart from [`Self::evicted_replaceable`]
    /// because the two say different things: this is retention working as
    /// stated, and that is retention too small for the traffic.
    pub const fn rolled_by_age(&self) -> u64 {
        self.rolled_by_age
    }

    /// Replaceable records — ticks, quotes, books, features — dropped to stay
    /// inside the ceiling.
    pub const fn evicted_replaceable(&self) -> u64 {
        self.evicted_replaceable
    }

    /// Non-replaceable observations — trades, bars, news, alternative data —
    /// dropped once no replaceable record was left to drop. Non-zero means
    /// retention is too small for this traffic, and it is deliberately a
    /// separate number from the replaceable evictions so that signal is not
    /// buried under the routine one.
    pub const fn evicted_observations(&self) -> u64 {
        self.evicted_observations
    }

    /// Appends refused because the ceiling was reached and every retained
    /// record requires permanent retention. Non-zero means the platform
    /// stopped rather than acted without a record.
    pub const fn appends_refused(&self) -> u64 {
        self.appends_refused
    }

    pub fn len(&self) -> usize {
        self.records.len()
    }

    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    /// Append an event, assigning its sequence number and chain hash.
    ///
    /// Refuses when retention is full of records that may not be evicted, and
    /// refuses when `event.event_id` is already indexed. The latter is not a
    /// courtesy check: `index` keys `by_event_id` with a plain map `insert`,
    /// so a second record arriving under an id already in the log would
    /// silently overwrite that mapping — [`EventLog::get`] would then return
    /// the newer record for a lookup naming the older one's id, with no error
    /// and no signal that the first record became unreachable by identity.
    /// The record itself would still sit in the hash chain (a second append
    /// always gets a new sequence and a new `record_hash`, so the two can
    /// never be byte-identical the way a retried block can), so
    /// `verify_chain` would keep passing while the audit index quietly lied
    /// about which content a given id names — the same shape of gap as a
    /// chain-position hash reused for different contents, one layer up, in
    /// the index rather than the chain. The refusal comes before the file
    /// write, so a refused append leaves no half-recorded event: nothing in
    /// memory, nothing on disk, and a caller that knows its event was not
    /// recorded.
    pub fn append(&mut self, event: &AnyEvent) -> Result<u64> {
        // Before any other check, so a refused append to an inspected log
        // leaves it exactly as read: nothing indexed, no sequence minted.
        if self.inspected {
            return Err(Error::denied(
                "this event log was opened by EventLog::inspect, which is read-only; open it \
                 with EventLog::open to resume it. An append here would reach memory and never \
                 the file, and a chain the file does not hold is not a record of anything",
            ));
        }
        self.reject_duplicate_event_id(event.event_id.as_str())?;
        self.roll_if_due(event.recorded_at);
        self.make_room(event.topic)?;
        let sequence = self.next_sequence();
        let previous_hash = self.last_hash.clone();

        let mut stored = event.clone();
        stored.sequence = sequence;
        let record_hash = compute_record_hash(sequence, &previous_hash, &stored)?;
        let record = LogRecord {
            sequence,
            previous_hash,
            record_hash,
            event: stored,
        };

        if let Some(path) = &self.path {
            let line = serde_json::to_string(&record)?;
            let mut file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)?;
            writeln!(file, "{line}")?;
            if self.durability.survives_power_loss() {
                // The chain is only evidence if the record outlives the
                // machine. Without this the append reaches the page cache and
                // a power cut removes a decision nobody can then account for —
                // and because the chain is over what was *retained*, the
                // surviving log still verifies, so the loss is silent.
                file.sync_all()?;
            }
        }
        self.index(record);
        Ok(sequence)
    }

    fn next_sequence(&self) -> u64 {
        self.last_sequence.saturating_add(1)
    }

    /// Refuse an event id already present in the index.
    ///
    /// Called on every append and on every record loaded from a file, so a
    /// collision is caught at the same seam whether it arrives live or is
    /// discovered replaying disk — a corrupt or tampered file that reused an
    /// id must fail to load rather than load with a silently shadowed record.
    fn reject_duplicate_event_id(&self, event_id: &str) -> Result<()> {
        if self.by_event_id.contains_key(event_id) {
            return Err(Error::invalid(format!(
                "event id {event_id} is already recorded in this log; a second record cannot \
                 reuse it, because doing so would silently replace the lookup index's only path \
                 to the first record while both remained in the hash chain — mint a new event id \
                 for genuinely new content, or if this is a redelivery, suppress it before it \
                 reaches the log"
            )));
        }
        Ok(())
    }

    fn index(&mut self, record: LogRecord) {
        let position = self.records.len();
        self.last_sequence = self.last_sequence.max(record.sequence);
        self.last_hash = record.record_hash.clone();
        self.by_correlation
            .entry(record.event.lineage.correlation_id.as_str().to_string())
            .or_default()
            .push(position);
        self.by_topic
            .entry(record.event.topic)
            .or_default()
            .push(position);
        self.by_event_id
            .insert(record.event.event_id.as_str().to_string(), position);
        if record.event.idempotency_key.is_some() {
            self.by_idempotency.insert(record.event.dedup_key());
        }
        self.records.push(record);
    }

    /// Whether a retained record already carries `dedup_key` — the
    /// `AnyEvent::dedup_key` of a body with an explicit idempotency key.
    ///
    /// The idempotency key was, until 2026-09-12, a fact the envelope carried
    /// and nothing consulted at the log: two journal calls for one fact wrote
    /// two records, each with the key that said they were one. A writer that
    /// asks here first writes one. Only records still retained are known; a
    /// key whose record the log evicted reads as absent, which is the honest
    /// answer for a log that no longer holds it.
    pub fn holds_idempotent(&self, dedup_key: &str) -> bool {
        self.by_idempotency.contains(dedup_key)
    }

    /// Make room for one more record, or refuse.
    ///
    /// Evicts the oldest replaceable record; failing that, the oldest
    /// observation; failing that, refuses, because everything left is the audit
    /// trail. Called *before* a record is written rather than after, so the log
    /// never writes something it is about to drop and never drops something to
    /// make room for what it then refuses.
    fn make_room(&mut self, incoming: Topic) -> Result<()> {
        while self.records.len() >= self.capacity {
            // Two passes rather than one: every replaceable record must be gone
            // before an observation is touched, so the counters mean what they
            // say and the cheap loss is always taken first.
            // The victim is chosen by each record's declared retention class
            // (ADR 0089) and by nothing else: the replaceable tier first,
            // then any class that is not permanent, then a refusal.
            let victim = self
                .records
                .iter()
                .position(|r| r.event.topic.retention_class().is_replaceable())
                .map(|index| (index, true))
                .or_else(|| {
                    self.records
                        .iter()
                        .position(|r| !r.event.topic.retention_class().is_permanent())
                        .map(|index| (index, false))
                });
            let Some((index, replaceable)) = victim else {
                self.appends_refused = self.appends_refused.saturating_add(1);
                return Err(Error::guard(format!(
                    "event log is full at {} records and every retained record's class requires \
                     permanent retention, so none may be dropped to admit a {} record (class {}); \
                     archive the log and start a new one, or open it with a larger capacity — \
                     this log will not discard an audit record to keep running",
                    self.capacity,
                    incoming.name(),
                    incoming.retention_class().as_str()
                )));
            };
            self.records.remove(index);
            if replaceable {
                self.evicted_replaceable = self.evicted_replaceable.saturating_add(1);
            } else {
                self.evicted_observations = self.evicted_observations.saturating_add(1);
            }
            self.rebuild_indexes();
        }
        Ok(())
    }

    /// Roll if `recorded_at` is at or past the instant the next roll fell
    /// due.
    ///
    /// The record that triggers a roll is, by construction, the newest the
    /// log has ever indexed: every record since the last roll was recorded
    /// before the due instant and this one at or after it, and each roll
    /// pushes the due instant a cadence past its own trigger. So the window
    /// is measured from the newest recorded instant without a separate
    /// high-water mark to keep, and a record arriving late — a backfill, a
    /// redelivery — can never trigger a roll and so can never pull the
    /// window back. The first record a log sees always triggers one, which
    /// over an empty index is free and over a loading file is the pass that
    /// applies the window to what the previous process left.
    fn roll_if_due(&mut self, recorded_at: Timestamp) {
        let Some(window) = self.snapshot_window else {
            return;
        };
        if self.next_roll_due.is_some_and(|due| recorded_at < due) {
            return;
        }
        self.roll(window, recorded_at);
        let cadence = if window < ROLL_CADENCE {
            window
        } else {
            ROLL_CADENCE
        };
        self.next_roll_due = Some(recorded_at.saturating_add(cadence));
    }

    /// Drop every replaceable record recorded more than the window before
    /// `newest`. Nothing else is a candidate: an observation is the fallback
    /// series' business and a permanent record is the audit trail's, and
    /// this filters on the same declared class the pressure eviction spends
    /// first so the two bounds agree about what is cheap to lose.
    fn roll(&mut self, window: Duration, newest: Timestamp) {
        let cutoff = newest.saturating_sub(window);
        let before = self.records.len();
        self.records.retain(|record| {
            !record.event.topic.retention_class().is_replaceable()
                || record.event.recorded_at >= cutoff
        });
        let rolled = before.saturating_sub(self.records.len());
        if rolled == 0 {
            return;
        }
        self.rolled_by_age = self.rolled_by_age.saturating_add(rolled as u64);
        self.rebuild_indexes();
    }

    fn rebuild_indexes(&mut self) {
        self.by_correlation.clear();
        self.by_topic.clear();
        self.by_event_id.clear();
        self.by_idempotency.clear();
        for (position, record) in self.records.iter().enumerate() {
            if record.event.idempotency_key.is_some() {
                self.by_idempotency.insert(record.event.dedup_key());
            }
            self.by_correlation
                .entry(record.event.lineage.correlation_id.as_str().to_string())
                .or_default()
                .push(position);
            self.by_topic
                .entry(record.event.topic)
                .or_default()
                .push(position);
            self.by_event_id
                .insert(record.event.event_id.as_str().to_string(), position);
        }
    }

    pub fn records(&self) -> &[LogRecord] {
        &self.records
    }

    pub fn events(&self) -> impl Iterator<Item = &AnyEvent> {
        self.records.iter().map(|r| &r.event)
    }

    pub fn get(&self, event_id: &EventId) -> Option<&AnyEvent> {
        self.by_event_id
            .get(event_id.as_str())
            .and_then(|i| self.records.get(*i))
            .map(|r| &r.event)
    }

    /// Every event in one lineage chain, in log order.
    ///
    /// This is the decision-reconstruction query: given the correlation id of
    /// an originating observation, it returns observation through learning.
    pub fn by_correlation(&self, correlation_id: &CorrelationId) -> Vec<&AnyEvent> {
        self.by_correlation
            .get(correlation_id.as_str())
            .map(|positions| {
                positions
                    .iter()
                    .filter_map(|i| self.records.get(*i))
                    .map(|r| &r.event)
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn by_topic(&self, topic: Topic) -> Vec<&AnyEvent> {
        self.by_topic
            .get(&topic)
            .map(|positions| {
                positions
                    .iter()
                    .filter_map(|i| self.records.get(*i))
                    .map(|r| &r.event)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Direct children of an event, by causation id.
    pub fn children_of(&self, event_id: &EventId) -> Vec<&AnyEvent> {
        self.records
            .iter()
            .filter(|r| {
                r.event
                    .lineage
                    .causation_id
                    .as_ref()
                    .is_some_and(|c| c.as_str() == event_id.as_str())
            })
            .map(|r| &r.event)
            .collect()
    }

    /// Events matching a filter, in log order.
    pub fn query(&self, filter: &EventFilter) -> Vec<&AnyEvent> {
        self.records
            .iter()
            .map(|r| &r.event)
            .filter(|e| filter.matches(e))
            .collect()
    }

    /// The sequence of the newest record this log holds, or zero for a log
    /// that holds none. Monotonic across restarts of a file-backed log, which
    /// is what makes it usable as the process-independent half of an
    /// identifier a caller mints — a per-process counter restarts at one and
    /// collides with everything the previous process minted.
    pub const fn last_sequence(&self) -> u64 {
        self.last_sequence
    }

    /// Verify the hash chain from genesis, contiguously. Returns the
    /// sequence of the first broken link — which, once the log has evicted
    /// anything, is the first retained record after the eviction.
    pub fn verify_chain(&self) -> std::result::Result<(), u64> {
        self.verify_chain_from(GENESIS_HASH, GapPolicy::Break)
    }

    /// Verify the chain over what the log retains, rather than from genesis.
    ///
    /// A capped log evicts its oldest replaceable records at load
    /// ([`Self::open_with_capacity`]) and during a run, so the first retained
    /// record's predecessor is usually gone and [`Self::verify_chain`] reports
    /// it as the first broken link — the limit the module doc states. That
    /// makes `verify_chain` the wrong question for a consumer rebuilding
    /// state from the retained span: it wants to know that every record it
    /// is about to read hashes to what its predecessor committed to, and
    /// that each link it can still check holds. This checks exactly that:
    /// every record's own hash is recomputed; the first retained record is
    /// held to genesis when it is sequence one and to its own claimed
    /// predecessor otherwise; and every later record is held to the one
    /// before it **when the two are consecutive**. Across a gap in the
    /// retained sequence the record is re-anchored on its claimed
    /// predecessor exactly as the head is — the same trust level, no new
    /// assumption — because [`Self::make_room`] evicts the oldest evictable
    /// record *wherever it sits*, and a permanent record older than every
    /// evictable one leaves the gap in the interior. Until 2026-09-12 this
    /// held every retained record to its retained predecessor, so a log
    /// that had interleaved a permanent record with evictable ones failed
    /// here at the first record after the interior gap, and the kernel
    /// refused to restart over a log it had honestly written.
    ///
    /// A gap is read as an eviction because a file-backed log's file cannot
    /// have one: [`Self::open_with_capacity`] refuses a file whose sequences
    /// are not contiguous from one, so a line removed from the file is
    /// refused at load and never reaches this check as a gap. What remains
    /// unprovable is what the chain never proved: it is unkeyed SHA-256, and
    /// a writer who can rewrite the file can rewrite the hashes with it
    /// (ADR 0043). A record whose payload was edited on disk with its hash
    /// left as written fails here whether or not anything around it was
    /// evicted.
    pub fn verify_retained_chain(&self) -> std::result::Result<(), u64> {
        let Some(first) = self.records.first() else {
            return Ok(());
        };
        if first.sequence == 1 {
            return self.verify_chain_from(GENESIS_HASH, GapPolicy::Evicted);
        }
        let claimed_predecessor = first.previous_hash.clone();
        self.verify_chain_from(&claimed_predecessor, GapPolicy::Evicted)
    }

    fn verify_chain_from(&self, genesis: &str, gaps: GapPolicy) -> std::result::Result<(), u64> {
        // The last record checked: its sequence and its hash as written.
        let mut previous: Option<(u64, &str)> = None;
        for record in &self.records {
            let expected_previous = match previous {
                None => genesis,
                Some((sequence, hash)) if record.sequence == sequence.saturating_add(1) => hash,
                Some((_, hash)) => match gaps {
                    GapPolicy::Break => hash,
                    GapPolicy::Evicted => record.previous_hash.as_str(),
                },
            };
            if record.previous_hash != expected_previous {
                return Err(record.sequence);
            }
            let recomputed =
                compute_record_hash(record.sequence, &record.previous_hash, &record.event)
                    .map_err(|_| record.sequence)?;
            if recomputed != record.record_hash {
                return Err(record.sequence);
            }
            previous = Some((record.sequence, record.record_hash.as_str()));
        }
        Ok(())
    }

    /// Replay every event through `handler`, oldest first.
    pub fn replay<F>(&self, mut handler: F) -> Result<usize>
    where
        F: FnMut(&AnyEvent) -> Result<()>,
    {
        for record in &self.records {
            handler(&record.event)?;
        }
        Ok(self.records.len())
    }

    /// Replay only the events matching a filter.
    pub fn replay_filtered<F>(&self, filter: &EventFilter, mut handler: F) -> Result<usize>
    where
        F: FnMut(&AnyEvent) -> Result<()>,
    {
        let mut count = 0;
        for record in &self.records {
            if filter.matches(&record.event) {
                handler(&record.event)?;
                count += 1;
            }
        }
        Ok(count)
    }

    pub fn stats(&self) -> LogStats {
        let mut by_group: BTreeMap<TopicGroup, u64> = BTreeMap::new();
        let mut by_topic: BTreeMap<Topic, u64> = BTreeMap::new();
        for record in &self.records {
            *by_group.entry(record.event.topic.group()).or_insert(0) += 1;
            *by_topic.entry(record.event.topic).or_insert(0) += 1;
        }
        LogStats {
            total: self.records.len() as u64,
            by_group,
            by_topic,
            first_event: self.records.first().map(|r| r.event.occurred_at),
            last_event: self.records.last().map(|r| r.event.occurred_at),
            correlations: self.by_correlation.len() as u64,
        }
    }
}

/// How a gap in the retained sequence is read while verifying the chain.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum GapPolicy {
    /// A gap is a broken link: the record after it is held to the record
    /// before it, which it cannot match. What `verify_chain` asks.
    Break,
    /// A gap is an eviction: the record after it is re-anchored on the
    /// predecessor it claims, as the head is. What `verify_retained_chain`
    /// asks.
    Evicted,
}

/// Hash committing to the record's position, its predecessor and its content.
fn compute_record_hash(sequence: u64, previous_hash: &str, event: &AnyEvent) -> Result<String> {
    let value = serde_json::to_value(event)?;
    let material = format!("{sequence}|{previous_hash}|{}", canonical_json(&value));
    Ok(sha256_hex(material.as_bytes()))
}

/// Query predicate over the log.
#[derive(Clone, Debug, Default)]
pub struct EventFilter {
    pub topics: Vec<Topic>,
    pub groups: Vec<TopicGroup>,
    pub correlation_id: Option<CorrelationId>,
    pub producer: Option<String>,
    /// Inclusive lower bound on `occurred_at`.
    pub from: Option<Timestamp>,
    /// Exclusive upper bound on `occurred_at`.
    pub until: Option<Timestamp>,
}

impl EventFilter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn topic(mut self, topic: Topic) -> Self {
        self.topics.push(topic);
        self
    }

    pub fn group(mut self, group: TopicGroup) -> Self {
        self.groups.push(group);
        self
    }

    pub fn correlation(mut self, id: CorrelationId) -> Self {
        self.correlation_id = Some(id);
        self
    }

    pub fn producer(mut self, producer: impl Into<String>) -> Self {
        self.producer = Some(producer.into());
        self
    }

    /// Restrict to `[from, until)`.
    pub fn between(mut self, from: Timestamp, until: Timestamp) -> Self {
        self.from = Some(from);
        self.until = Some(until);
        self
    }

    /// Everything strictly before `until` — the point-in-time view used to
    /// rebuild what the platform knew at a past instant.
    pub fn as_of(mut self, until: Timestamp) -> Self {
        self.until = Some(until);
        self
    }

    pub fn matches(&self, event: &AnyEvent) -> bool {
        if !self.topics.is_empty() && !self.topics.contains(&event.topic) {
            return false;
        }
        if !self.groups.is_empty() && !self.groups.contains(&event.topic.group()) {
            return false;
        }
        if let Some(correlation) = &self.correlation_id
            && event.lineage.correlation_id.as_str() != correlation.as_str()
        {
            return false;
        }
        if let Some(producer) = &self.producer
            && &event.lineage.producer != producer
        {
            return false;
        }
        if let Some(from) = self.from
            && event.occurred_at < from
        {
            return false;
        }
        if let Some(until) = self.until
            && event.occurred_at >= until
        {
            return false;
        }
        true
    }
}

/// Summary of a log's contents.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LogStats {
    pub total: u64,
    pub by_group: BTreeMap<TopicGroup, u64>,
    pub by_topic: BTreeMap<Topic, u64>,
    pub first_event: Option<Timestamp>,
    pub last_event: Option<Timestamp>,
    pub correlations: u64,
}

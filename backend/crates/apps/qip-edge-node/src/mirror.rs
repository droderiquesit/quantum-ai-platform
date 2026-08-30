//! The cell's journal, shipped to a configured store instead of to a path.
//!
//! [`qip_edge::journal::Mirror`] is the seam a cell drains its decision record
//! through, and the node previously chose between a file and memory with its
//! own environment variable. That put a second, separate answer to "where does
//! this deployment keep its state" beside the one every other binary reads,
//! and the two could disagree — a node writing its journal to a path while the
//! rest of the deployment believed it was configured for memory, or the
//! reverse.
//!
//! # Sessions, and why the chain does not span them
//!
//! A cell's journal is per-session by construction: the cell rebuilds its books
//! from the feed on every start, so its first decision after a restart chains
//! onto [`qip_edge::journal::Journal::GENESIS`] and not onto whatever the
//! previous run ended with. Demanding one chain across restarts would refuse
//! every legitimate restart.
//!
//! So batches are keyed by session and chained *within* one. That keeps both
//! properties that matter: a batch that does not follow its predecessor inside
//! a session is a gap and is refused, and a new session appends beside the old
//! ones rather than over them. The record of what a dead cell did survives; the
//! cell's own state deliberately does not.

use qip_core::Timestamp;
use qip_core::error::{Error, Result};
use qip_edge::journal::{Journal, Mirror, MirrorBatch};
use qip_storage::kv::{KeyValueStore, KeyValueStoreExt};
use std::sync::Arc;

/// The key prefix every shipped batch sits under.
const SESSION_PREFIX: &str = "session/";

/// Width of the zero-padded session and batch numbers in a key.
///
/// The key-value port promises keys back in *lexicographic* order, so without
/// padding batch 10 sorts before batch 9 and a reader replaying the journal
/// would see the cell's decisions out of the order it made them — which, for a
/// record whose whole purpose is answering "why did this happen", is worse
/// than not having it.
///
/// Twenty digits covers every second count a clock in this platform produces.
/// A session stamped before the epoch would sort ahead of the rest, which is
/// still the right place for it and is not a case a deployed cell reaches.
const NUMBER_WIDTH: usize = 20;

/// A [`Mirror`] that ships journal batches into a [`KeyValueStore`].
#[derive(Debug)]
pub struct StoreMirror {
    store: Arc<dyn KeyValueStore>,
    cell: String,
    /// The session's start, in seconds. Only ever compared and printed, never
    /// used as a duration, so the clock's own signed second count is kept
    /// rather than converted into something that would need a range check.
    session: i64,
    next_batch: u64,
    /// The digest the next batch of *this session* must chain onto.
    tail_digest: String,
    shipped_batches: u64,
    shipped_entries: usize,
}

impl StoreMirror {
    /// Open a mirror for `cell`, starting a session stamped `started`.
    ///
    /// Refuses a store that already holds another cell's batches. Two cells
    /// sharing one storage root is a deployment mistake whose symptom is two
    /// cells' decisions interleaved in one record, and an audit trail that
    /// cannot say which cell made a decision is not an audit trail. Each cell
    /// gets its own root.
    pub fn open(
        store: Arc<dyn KeyValueStore>,
        cell: impl Into<String>,
        started: Timestamp,
    ) -> Result<Self> {
        let cell = cell.into();
        let keys = store.keys_with_prefix(SESSION_PREFIX)?;
        if let Some(key) = keys.last() {
            let existing: MirrorBatch = store.get_as(key)?.ok_or_else(|| {
                Error::io(format!(
                    "the journal store lists {key} but does not hold it; a record that cannot be \
                     read cannot be appended to safely"
                ))
            })?;
            if existing.cell != cell {
                return Err(Error::invalid(format!(
                    "this store already holds journal batches for cell {}, and this node is cell \
                     {cell}. Two cells sharing one storage root interleave their decision \
                     records; give each cell its own QIP_STORAGE_ROOT",
                    existing.cell
                )));
            }
        }
        Ok(Self {
            store,
            cell,
            session: started.as_secs(),
            next_batch: 0,
            tail_digest: Journal::GENESIS.to_string(),
            shipped_batches: 0,
            shipped_entries: 0,
        })
    }

    /// How many batches this session has shipped.
    pub fn shipped_batches(&self) -> u64 {
        self.shipped_batches
    }

    /// How many journal entries this session has shipped.
    pub fn shipped_entries(&self) -> usize {
        self.shipped_entries
    }

    /// How many sessions the store holds a record of, this one included once
    /// it has shipped anything.
    ///
    /// Reported at start-up so an operator can see that previous runs were
    /// retained rather than replaced, which is the failure a per-session key
    /// scheme is there to prevent.
    pub fn retained_sessions(&self) -> Result<usize> {
        let mut sessions: Vec<String> = self
            .store
            .keys_with_prefix(SESSION_PREFIX)?
            .into_iter()
            .filter_map(|key| {
                key.strip_prefix(SESSION_PREFIX)
                    .and_then(|rest| rest.split('/').next().map(str::to_string))
            })
            .collect();
        sessions.dedup();
        Ok(sessions.len())
    }

    fn key_for(&self, batch: u64) -> String {
        format!(
            "{SESSION_PREFIX}{session:0width$}/batch/{batch:0width$}",
            session = self.session,
            width = NUMBER_WIDTH,
        )
    }
}

impl Mirror for StoreMirror {
    fn ship(&mut self, batch: MirrorBatch) -> Result<()> {
        if batch.cell != self.cell {
            return Err(Error::invalid(format!(
                "a batch from cell {} was handed to the mirror for cell {}",
                batch.cell, self.cell
            )));
        }
        // The chain check happens before the write, not after. A batch that
        // does not follow its predecessor means entries went missing between
        // the two, and writing it anyway would produce a record that reads as
        // continuous while having a hole in it — the one state an audit trail
        // must never be in.
        batch.verify_against(&self.tail_digest)?;

        let entries = batch.entries.len();
        let tail = batch.tail_digest();
        self.store.put_as(&self.key_for(self.next_batch), &batch)?;
        self.next_batch = self.next_batch.saturating_add(1);
        self.tail_digest = tail;
        self.shipped_batches = self.shipped_batches.saturating_add(1);
        self.shipped_entries = self.shipped_entries.saturating_add(entries);
        Ok(())
    }

    fn required_configuration(&self) -> Vec<String> {
        // A store was supplied, so nothing is outstanding. What the *store*
        // needs was settled at start-up: a managed target refuses to preflight
        // rather than falling back, so a mirror holding one cannot exist.
        Vec::new()
    }
}

/// Read every batch a store holds, in the order the cell made them.
///
/// The centre's side of the mirror, and the only way to check that what was
/// shipped is what can be read back. Public because a test that reconstructs
/// the record through the same path an operator would is worth more than one
/// that inspects keys.
pub fn batches(store: &dyn KeyValueStore) -> Result<Vec<MirrorBatch>> {
    let mut out = Vec::new();
    for key in store.keys_with_prefix(SESSION_PREFIX)? {
        let batch: MirrorBatch = store.get_as(&key)?.ok_or_else(|| {
            Error::io(format!(
                "the journal store lists {key} but does not hold it"
            ))
        })?;
        out.push(batch);
    }
    Ok(out)
}

//! Historical reasoning re-fetches through the manifest (R12-DATA-1037), and
//! ambient watchers keep cursors and fingerprints so change is known without
//! keeping the corpus (R12-AMBIENT-1040).
//!
//! A [`DataReference`] records what was fetched and what it hashed to; the
//! raw bytes are discarded (§22.1). Until 2026-10-11 nothing ever asked a
//! source for an extent again: `RevisionCheck::Unretrievable` existed and no
//! code produced it, the reference's re-fetch instructions were never
//! written, and a backtest built on an extent the vendor had since withdrawn
//! read exactly like one built on an extent it still served.
//!
//! [`ReFetchDriver`] walks references, asks a [`Refetcher`] for each extent
//! again, and returns a [`RevisionCheck`] for each: unchanged, revised (with
//! both hashes), or unretrievable. An extent is unretrievable at once when
//! the source says so — withdrawn (404/410) or denied (401/403) — and after
//! [`UNAVAILABLE_BEFORE_UNRETRIEVABLE`] consecutive failures when it is
//! merely not answering, because one timeout is weather and three are a
//! source that no longer serves the extent. A source is marked unretrievable
//! when every extent of it the driver has tried is.
//!
//! [`SourceWatcher`] is the ambient half: per source it keeps the last
//! cursor and, per locator, the last content fingerprint, so a new fetch is
//! classified new, unchanged or changed without holding any body. It
//! persists through the `qip_core::kv` port, so a restart does not forget
//! what it had seen.
//!
//! Both are bounded. Nothing here opens a socket: [`TransportRefetcher`]
//! drives a `SourceTransport` the composition root built, whose client
//! carries the timeouts.

use crate::reference::{DataReference, RevisionCheck, SourceOrigin};
use qip_core::Timestamp;
use qip_core::error::{Error, Result};
use qip_core::kv::KeyValueStore;
use qip_market_ingestion::connector::transport::{SourceRequest, SourceTransport};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Consecutive unanswered re-fetches before an extent is unretrievable.
pub const UNAVAILABLE_BEFORE_UNRETRIEVABLE: u32 = 3;
/// Extents whose failure count the driver remembers.
pub const DRIVER_MEMORY: usize = 4_096;

/// What asking a source for an extent again produced.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReFetchOutcome {
    /// The source served bytes, with its etag if it sent one.
    ///
    /// Carries the SHA-256 of what was served, not the bytes. A re-fetch only
    /// ever compares hashes, and until 2026-10-24 this variant copied the
    /// vendor's body into a plain `Vec<u8>` to be hashed one line later — a
    /// stranger's bytes held as an ordinary value for no reason the driver
    /// had. The transport hands the body over as `Untrusted`, whose digest
    /// needs neither of its exits (R12-SEC-9065).
    Fetched {
        /// Lower-case hex SHA-256 of the bytes as served.
        content_hash: String,
        etag: Option<String>,
    },
    /// The source says the extent no longer exists.
    Withdrawn,
    /// The source refused this platform access.
    Denied,
    /// The source did not answer usefully; may be transient.
    Unavailable { detail: String },
}

/// Something that can ask a source for an extent again.
pub trait Refetcher {
    fn refetch(&mut self, reference: &DataReference, at: Timestamp) -> Result<ReFetchOutcome>;
}

/// A [`Refetcher`] over a connector transport — the same egress route, the
/// same timeouts, the same credentials the original fetch used.
pub struct TransportRefetcher<'a> {
    transport: &'a mut dyn SourceTransport,
}

impl std::fmt::Debug for TransportRefetcher<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TransportRefetcher")
            .field("transport", &self.transport.describe())
            .finish()
    }
}

impl<'a> TransportRefetcher<'a> {
    pub fn new(transport: &'a mut dyn SourceTransport) -> Self {
        Self { transport }
    }
}

/// Split a reference's locator back into the request it was made with.
fn request_of(locator: &str) -> SourceRequest {
    let (path, query) = locator.split_once('?').unwrap_or((locator, ""));
    let mut request = SourceRequest::get(path);
    for pair in query.split('&').filter(|pair| !pair.is_empty()) {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        request.query.insert(key.to_string(), value.to_string());
    }
    request
}

impl Refetcher for TransportRefetcher<'_> {
    fn refetch(&mut self, reference: &DataReference, at: Timestamp) -> Result<ReFetchOutcome> {
        let response = match self.transport.request(&request_of(reference.locator()), at) {
            Ok(response) => response,
            Err(error) => {
                return Ok(ReFetchOutcome::Unavailable {
                    detail: error.message().to_string(),
                });
            }
        };
        Ok(match response.status {
            200..=299 => ReFetchOutcome::Fetched {
                content_hash: response.body.sha256_hex(),
                etag: response.etag,
            },
            404 | 410 => ReFetchOutcome::Withdrawn,
            401 | 403 => ReFetchOutcome::Denied,
            status => ReFetchOutcome::Unavailable {
                detail: format!("the source answered {status}"),
            },
        })
    }
}

/// One re-fetch's result.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ReFetchFinding {
    pub source_id: String,
    pub locator: String,
    /// `None` while an unanswered extent is still below the threshold.
    pub check: Option<RevisionCheck>,
    pub consecutive_failures: u32,
    pub at: Timestamp,
}

/// Re-fetches referenced extents and marks the ones a source no longer
/// serves.
#[derive(Clone, Debug, Default)]
pub struct ReFetchDriver {
    failures: BTreeMap<(String, String), u32>,
    unretrievable: BTreeMap<String, BTreeSet<String>>,
    retrievable: BTreeMap<String, BTreeSet<String>>,
}

impl ReFetchDriver {
    pub fn new() -> Self {
        Self::default()
    }

    /// Re-fetch at most `budget` of `references`, in the order given.
    ///
    /// A generated stream is skipped — this platform produced it and there
    /// is no vendor to ask — and so is a replay, whose bytes came from a
    /// file whose author's word is all the platform ever had: asking the
    /// vendor for a replay's extent would verify something the reference
    /// never claimed. Neither consumes budget.
    pub fn run<'r>(
        &mut self,
        references: impl IntoIterator<Item = &'r DataReference>,
        fetcher: &mut dyn Refetcher,
        budget: usize,
        at: Timestamp,
    ) -> Result<Vec<ReFetchFinding>> {
        let mut findings = Vec::new();
        for reference in references {
            if findings.len() >= budget {
                break;
            }
            if !matches!(
                reference.origin(),
                SourceOrigin::Discovered | SourceOrigin::CatalogueAdmitted
            ) {
                continue;
            }
            let key = (
                reference.source_id().to_string(),
                reference.locator().to_string(),
            );
            let outcome = fetcher.refetch(reference, at)?;
            let (check, failures) = match outcome {
                ReFetchOutcome::Fetched { content_hash, .. } => {
                    self.failures.remove(&key);
                    (Some(reference.verify_hash(content_hash)), 0)
                }
                ReFetchOutcome::Withdrawn | ReFetchOutcome::Denied => {
                    self.failures.remove(&key);
                    (Some(RevisionCheck::Unretrievable), 0)
                }
                ReFetchOutcome::Unavailable { .. } => {
                    let count = self.failures.entry(key.clone()).or_insert(0);
                    *count = count.saturating_add(1);
                    let count = *count;
                    if count >= UNAVAILABLE_BEFORE_UNRETRIEVABLE {
                        (Some(RevisionCheck::Unretrievable), count)
                    } else {
                        (None, count)
                    }
                }
            };
            let (source, locator) = key;
            match &check {
                Some(RevisionCheck::Unretrievable) => {
                    self.unretrievable
                        .entry(source.clone())
                        .or_default()
                        .insert(locator.clone());
                    if let Some(served) = self.retrievable.get_mut(&source) {
                        served.remove(&locator);
                    }
                }
                Some(_) => {
                    self.retrievable
                        .entry(source.clone())
                        .or_default()
                        .insert(locator.clone());
                    if let Some(gone) = self.unretrievable.get_mut(&source) {
                        gone.remove(&locator);
                    }
                }
                None => {}
            }
            findings.push(ReFetchFinding {
                source_id: source,
                locator,
                check,
                consecutive_failures: failures,
                at,
            });
        }
        self.bound();
        Ok(findings)
    }

    /// Extents of `source` the driver found unretrievable.
    pub fn unretrievable_extents(&self, source: &str) -> BTreeSet<String> {
        self.unretrievable.get(source).cloned().unwrap_or_default()
    }

    /// Sources every tried extent of which is unretrievable — what any
    /// knowledge derived from them must be marked as resting on.
    pub fn unretrievable_sources(&self) -> BTreeSet<String> {
        self.unretrievable
            .iter()
            .filter(|(source, gone)| {
                !gone.is_empty() && self.retrievable.get(*source).is_none_or(BTreeSet::is_empty)
            })
            .map(|(source, _)| source.clone())
            .collect()
    }

    fn bound(&mut self) {
        while self.failures.len() > DRIVER_MEMORY {
            let Some(first) = self.failures.keys().next().cloned() else {
                break;
            };
            self.failures.remove(&first);
        }
        for map in [&mut self.unretrievable, &mut self.retrievable] {
            for set in map.values_mut() {
                while set.len() > DRIVER_MEMORY {
                    let Some(first) = set.iter().next().cloned() else {
                        break;
                    };
                    set.remove(&first);
                }
            }
            map.retain(|_, set| !set.is_empty());
        }
    }
}

/// What a watcher saw in one new reference.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "change", rename_all = "snake_case")]
pub enum WatchDelta {
    /// An extent the watcher had not seen.
    New,
    /// The same extent hashes as before.
    Unchanged,
    /// The same extent hashes differently: the knowledge delta to act on.
    Changed { was: String, now: String },
}

/// One source's ambient state: its last cursor and the fingerprint of each
/// extent it has served, never the bodies.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WatchState {
    pub cursor: Option<String>,
    pub fingerprints: BTreeMap<String, String>,
    pub last_seen: Option<Timestamp>,
}

/// Extents remembered per source before the oldest-named is forgotten.
pub const WATCH_EXTENTS_PER_SOURCE: usize = 1_024;
const WATCH_KEY_PREFIX: &str = "data-finder/watch/";

/// Ambient source watchers, one state per source (R12-AMBIENT-1040).
#[derive(Clone, Debug, Default)]
pub struct SourceWatcher {
    states: BTreeMap<String, WatchState>,
}

impl SourceWatcher {
    pub fn new() -> Self {
        Self::default()
    }

    /// Classify a newly recorded reference against what was last seen for
    /// its extent, and advance the source's cursor and fingerprint.
    pub fn observe(&mut self, reference: &DataReference) -> WatchDelta {
        let state = self
            .states
            .entry(reference.source_id().to_string())
            .or_default();
        if let Some(cursor) = reference.cursor() {
            state.cursor = Some(cursor.to_string());
        }
        state.last_seen = Some(reference.retrieved_at());
        let now = reference.content_hash().to_string();
        let delta = match state
            .fingerprints
            .insert(reference.locator().to_string(), now.clone())
        {
            None => WatchDelta::New,
            Some(was) if was == now => WatchDelta::Unchanged,
            Some(was) => WatchDelta::Changed { was, now },
        };
        while state.fingerprints.len() > WATCH_EXTENTS_PER_SOURCE {
            let Some(first) = state.fingerprints.keys().next().cloned() else {
                break;
            };
            state.fingerprints.remove(&first);
        }
        delta
    }

    pub fn state(&self, source: &str) -> Option<&WatchState> {
        self.states.get(source)
    }

    /// Persist every source's state, one key per source.
    pub fn save(&self, store: &dyn KeyValueStore) -> Result<()> {
        for (source, state) in &self.states {
            let value = serde_json::to_value(state).map_err(|error| {
                Error::invalid(format!(
                    "the watch state for `{source}` did not serialise: {error}"
                ))
            })?;
            store.put(&format!("{WATCH_KEY_PREFIX}{source}"), value)?;
        }
        Ok(())
    }

    /// Restore every persisted source's state. A value that does not decode
    /// is refused rather than skipped: a watcher that silently forgot one
    /// source would report its next fetch as new and miss a revision.
    pub fn load(store: &dyn KeyValueStore) -> Result<Self> {
        let mut states = BTreeMap::new();
        for key in store.keys_with_prefix(WATCH_KEY_PREFIX)? {
            let Some(value) = store.get(&key)? else {
                continue;
            };
            let state: WatchState = serde_json::from_value(value).map_err(|error| {
                Error::invalid(format!(
                    "the watch state under `{key}` does not decode: {error}"
                ))
            })?;
            states.insert(key[WATCH_KEY_PREFIX.len()..].to_string(), state);
        }
        Ok(Self { states })
    }
}

//! The bounded episode store and its approximate-nearest-neighbour index.
//!
//! Retrieval is locality-sensitive hashing by random hyperplanes, in
//! [`TABLES`] independent tables of [`BITS`] planes each: an episode's
//! embedding is hashed per table to the bit pattern of its signs against that
//! table's planes, episodes sharing a pattern share a bucket, and a query
//! probes every table's home bucket and then, table by table, the buckets one
//! bit away in bit order, until it has gathered
//! [`EpisodicMemory::candidate_bound`] eligible candidates or run out of
//! probes. The candidates are then re-ranked by exact cosine. Everything here
//! is pure Rust over `BTreeMap`, because the workspace permits two
//! dependencies and neither is an index.
//!
//! Why several short tables rather than one long one: a query that differs
//! from a stored episode only in its scalar block — the same name, regime and
//! claim, a different conviction — flips a couple of bits against a single
//! long hash and is missed by a one-bit probe. Four six-bit tables make that
//! miss improbable while keeping the probe set fixed at
//! `TABLES * (1 + BITS)` bucket lookups.
//!
//! Why hyperplanes from a stated seed rather than from an entropy source: a
//! replay of the cycle journal must recall the same neighbours the live
//! process recalled, and a test that constructs the memory twice must get
//! the same answer both times. [`LSH_SEED`] is the whole of the randomness.
//!
//! Why a candidate bound: the index exists so retrieval is bounded whatever
//! the memory holds, and a probe that walked every bucket on a miss would be
//! the linear scan the index was built to avoid, at the moment — a novel
//! situation — when the cycle can least afford it.

use super::episode::{Episode, EpisodeQuery};
use crate::embedding::Embedding;
use qip_core::error::{Error, Result};
use qip_core::time::Timestamp;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Independent hash tables.
pub const TABLES: usize = 4;

/// Hyperplanes per table, and so bits in a bucket key: `2^6` buckets each.
///
/// Few enough that a near-twin shares or neighbours the home bucket in at
/// least one table with high probability; enough that a few thousand
/// episodes spread to tens per bucket rather than hundreds.
pub const BITS: usize = 6;

/// Bucket lookups a recall performs at most: every table's home bucket and
/// every one-bit neighbour of it.
pub const PROBES: usize = TABLES * (1 + BITS);

/// The seed every hyperplane is drawn from. Changing it changes every
/// bucket assignment, so it is a constant here and not a parameter.
pub const LSH_SEED: u64 = 0x5149_505F_4550_4953;

/// Default capacity: oldest-first eviction beyond this many episodes.
pub const DEFAULT_CAPACITY: usize = 4_096;

/// Default bound on candidates examined per query.
pub const DEFAULT_CANDIDATE_BOUND: usize = 256;

/// Insertion order within the store: by `known_at`, then arrival.
///
/// The map is ordered on this key so the oldest-known episode is always
/// `first_key_value`, which is what makes eviction by age one `pop_first`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Slot {
    known_at: Timestamp,
    sequence: u64,
}

#[derive(Clone, Debug)]
struct Stored {
    episode: Episode,
    embedding: Embedding,
    /// The bucket key in each table, so eviction can unlink without
    /// rehashing.
    buckets: Vec<u32>,
}

/// One recalled episode and how near it was.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Recalled {
    pub episode: Episode,
    /// Exact cosine similarity to the query, in `[-1, 1]`.
    pub similarity: f32,
}

/// What a recall examined and what it returned.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Recall {
    /// Candidates the probe gathered before re-ranking, never above the
    /// store's candidate bound.
    pub examined: usize,
    /// Buckets probed, including empty ones.
    pub probed: usize,
    /// The nearest, best first, at most `k`.
    pub nearest: Vec<Recalled>,
}

/// How the nearest resolved episodes' outcomes sat against a claim.
///
/// A statistic about precedent, recorded beside a hypothesis. It is not a
/// confidence and nothing here feeds one; see the module documentation for
/// the ADR 0005 route by which it could.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PrecedentDigest {
    /// Episodes recalled.
    pub nearest: usize,
    /// Of those, episodes with an outcome that has a sign.
    pub resolved: usize,
    /// Of those, outcomes that went the claim's way.
    pub agreeing: usize,
    /// `agreeing / resolved`, or `None` where nothing resolved has a sign —
    /// a share of nothing is not zero agreement, it is no evidence.
    pub agreement: Option<f64>,
}

impl PrecedentDigest {
    /// Digest the recalled episodes against the direction a claim implies.
    pub fn of(recalled: &[Recalled], direction: f64) -> Self {
        let mut resolved = 0usize;
        let mut agreeing = 0usize;
        for entry in recalled {
            if let Some(agreed) = entry
                .episode
                .outcome
                .as_ref()
                .and_then(|outcome| outcome.agrees_with(direction))
            {
                resolved += 1;
                agreeing += usize::from(agreed);
            }
        }
        Self {
            nearest: recalled.len(),
            resolved,
            agreeing,
            // Count to statistic: the share is a float from here on.
            agreement: (resolved > 0).then(|| agreeing as f64 / resolved as f64),
        }
    }
}

/// The bounded, bitemporal episode store.
#[derive(Clone, Debug)]
pub struct EpisodicMemory {
    capacity: usize,
    candidate_bound: usize,
    /// `tables[t][b]` is plane `b` of table `t`.
    tables: Vec<Vec<Vec<f32>>>,
    stored: BTreeMap<Slot, Stored>,
    /// Keyed by `(table, bucket)`.
    buckets: BTreeMap<(usize, u32), BTreeSet<Slot>>,
    ids: BTreeMap<String, Slot>,
    next_sequence: u64,
}

impl Default for EpisodicMemory {
    /// The defaults above, which are non-zero by construction.
    fn default() -> Self {
        Self::build(DEFAULT_CAPACITY, DEFAULT_CANDIDATE_BOUND)
    }
}

impl EpisodicMemory {
    /// A memory holding at most `capacity` episodes and examining at most
    /// `candidate_bound` per query.
    ///
    /// Zero is refused for either rather than raised to one: a memory that
    /// forgets everything, or a recall that examines nothing, is a
    /// configuration mistake the caller needs to hear about, and a
    /// silently corrected one would run every cycle with no precedent and
    /// no complaint.
    pub fn new(capacity: usize, candidate_bound: usize) -> Result<Self> {
        if capacity == 0 {
            return Err(Error::invalid(
                "an episodic memory with capacity zero forgets everything; give it a capacity",
            ));
        }
        if candidate_bound == 0 {
            return Err(Error::invalid(
                "a candidate bound of zero makes every recall empty; give it a bound",
            ));
        }
        Ok(Self::build(capacity, candidate_bound))
    }

    fn build(capacity: usize, candidate_bound: usize) -> Self {
        Self {
            capacity,
            candidate_bound,
            tables: hyperplanes(),
            stored: BTreeMap::new(),
            buckets: BTreeMap::new(),
            ids: BTreeMap::new(),
            next_sequence: 0,
        }
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    pub fn candidate_bound(&self) -> usize {
        self.candidate_bound
    }

    pub fn len(&self) -> usize {
        self.stored.len()
    }

    pub fn is_empty(&self) -> bool {
        self.stored.is_empty()
    }

    /// Whether an episode with this id is held.
    pub fn contains(&self, episode_id: &str) -> bool {
        self.ids.contains_key(episode_id)
    }

    /// Every episode, oldest-known first.
    pub fn episodes(&self) -> impl Iterator<Item = &Episode> {
        self.stored.values().map(|stored| &stored.episode)
    }

    /// The bucket an embedding hashes to in each table, in table order.
    /// Exposed so a test can prove two constructions agree on the index and
    /// not only on the answer.
    pub fn buckets_of(&self, embedding: &Embedding) -> Vec<u32> {
        self.tables
            .iter()
            .map(|planes| {
                let mut key = 0u32;
                for (bit, plane) in planes.iter().enumerate() {
                    let dot: f32 = plane
                        .iter()
                        .zip(&embedding.values)
                        .map(|(p, v)| p * v)
                        .sum();
                    if dot >= 0.0 {
                        key |= 1 << bit;
                    }
                }
                key
            })
            .collect()
    }

    /// Keep an episode, evicting the oldest-known beyond capacity.
    ///
    /// Refuses an invalid episode and a duplicate id: the second record
    /// under an id is a replay bug or a double resolution, and overwriting
    /// the first would hide whichever it was.
    pub fn remember(&mut self, episode: Episode) -> Result<()> {
        episode.validate()?;
        if self.ids.contains_key(&episode.episode_id) {
            return Err(Error::invalid(format!(
                "episode {} is already remembered; an outcome is recorded once",
                episode.episode_id
            )));
        }
        let embedding = episode.embedding();
        let buckets = self.buckets_of(&embedding);
        let slot = Slot {
            known_at: episode.known_at,
            sequence: self.next_sequence,
        };
        self.next_sequence += 1;
        self.ids.insert(episode.episode_id.clone(), slot);
        for (table, bucket) in buckets.iter().enumerate() {
            self.buckets
                .entry((table, *bucket))
                .or_default()
                .insert(slot);
        }
        self.stored.insert(
            slot,
            Stored {
                episode,
                embedding,
                buckets,
            },
        );
        while self.stored.len() > self.capacity {
            self.evict_oldest();
        }
        Ok(())
    }

    fn evict_oldest(&mut self) {
        let Some((slot, stored)) = self.stored.pop_first() else {
            return;
        };
        self.ids.remove(&stored.episode.episode_id);
        for (table, bucket) in stored.buckets.iter().enumerate() {
            let key = (table, *bucket);
            if let Some(members) = self.buckets.get_mut(&key) {
                members.remove(&slot);
                if members.is_empty() {
                    self.buckets.remove(&key);
                }
            }
        }
    }

    /// The `k` episodes nearest to `query` among those known before `now`.
    ///
    /// Strictly before: an episode whose `known_at` equals `now` is not yet
    /// knowledge. The deterministic clock can hand two cycles the same
    /// instant, and the resolution the first cycle's LEARN stamped must not
    /// be visible to the second cycle's REASON on that reading — refusing
    /// the boundary is the fail-closed answer, and it costs nothing on a
    /// clock that advances. The point-in-time filter is applied as
    /// candidates are gathered, so an episode not yet known neither appears
    /// in the answer nor occupies a slot in the candidate bound. Ties in
    /// similarity break
    /// toward the more recently known episode, then arrival order, so the
    /// answer is a total order and a replay reproduces it.
    pub fn recall(&self, query: &EpisodeQuery, now: Timestamp, k: usize) -> Recall {
        let embedding = query.embedding();
        let homes = self.buckets_of(&embedding);

        let mut seen: BTreeSet<Slot> = BTreeSet::new();
        let mut candidates: Vec<(Slot, f32)> = Vec::new();
        let mut probed = 0usize;
        for key in probe_order(&homes) {
            if candidates.len() >= self.candidate_bound {
                break;
            }
            probed += 1;
            let Some(members) = self.buckets.get(&key) else {
                continue;
            };
            // Newest-known first within a bucket, so when the bound cuts a
            // bucket it keeps the most recent precedent rather than the
            // oldest.
            for slot in members.iter().rev() {
                if candidates.len() >= self.candidate_bound {
                    break;
                }
                if slot.known_at >= now || !seen.insert(*slot) {
                    continue;
                }
                let Some(stored) = self.stored.get(slot) else {
                    continue;
                };
                candidates.push((*slot, stored.embedding.cosine_similarity(&embedding)));
            }
        }
        let examined = candidates.len();

        candidates.sort_by(|(a_slot, a_sim), (b_slot, b_sim)| {
            b_sim
                .total_cmp(a_sim)
                .then_with(|| b_slot.known_at.cmp(&a_slot.known_at))
                .then_with(|| b_slot.sequence.cmp(&a_slot.sequence))
        });
        let nearest = candidates
            .into_iter()
            .take(k)
            .filter_map(|(slot, similarity)| {
                self.stored.get(&slot).map(|stored| Recalled {
                    episode: stored.episode.clone(),
                    similarity,
                })
            })
            .collect();
        Recall {
            examined,
            probed,
            nearest,
        }
    }
}

/// Every table's home bucket first, then each table's one-bit neighbours in
/// table order and bit order: [`PROBES`] keys, always in this sequence.
fn probe_order(homes: &[u32]) -> Vec<(usize, u32)> {
    let mut order = Vec::with_capacity(PROBES);
    order.extend(homes.iter().copied().enumerate());
    for (table, home) in homes.iter().enumerate() {
        order.extend((0..BITS).map(|bit| (table, home ^ (1 << bit))));
    }
    order
}

/// The fixed hyperplanes, components uniform in `[-1, 1]` from a splitmix64
/// stream seeded with [`LSH_SEED`], drawn table by table then plane by plane.
fn hyperplanes() -> Vec<Vec<Vec<f32>>> {
    let mut state = LSH_SEED;
    (0..TABLES)
        .map(|_| {
            (0..BITS)
                .map(|_| {
                    (0..super::episode::EPISODE_DIMENSIONS)
                        .map(|_| {
                            let word = splitmix64(&mut state);
                            // Top 24 bits to a float in [0, 1), then to [-1, 1).
                            let unit = (word >> 40) as f32 / (1u64 << 24) as f32;
                            unit * 2.0 - 1.0
                        })
                        .collect()
                })
                .collect()
        })
        .collect()
}

fn splitmix64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

//! Which episode a bounded memory gives up — blueprint §54.2's episode
//! sampling.
//!
//! §54.2 closes episode sampling as "dense at high surprise, every action,
//! regime transitions; sparse in calm", reasoned "most information is in the
//! tails". This module is that decision made structural, and the first thing
//! to say is **where** it had to be made, because the obvious place cannot
//! work.
//!
//! # Why this is the eviction seam and not an admission gate
//!
//! The obvious reading is a gate in front of [`super::EpisodicMemory::remember`]
//! that turns episodes away. At this platform's one production seam that gate
//! could never fire. `Platform::remember_resolved` is the only production
//! caller, and it moves an episode into memory when a **thesis** resolves; a
//! thesis exists only where `record_precedent` was handed
//! [`super::DecisionTaken::Approved`], because `RejectedOnReview` and
//! `NotSizeable` never reach `pending_theses` and so never produce an outcome
//! to resolve against. Every episode arriving at the store is therefore an
//! action — and §54.2 keeps **every** action. A gate there admits one hundred
//! per cent of what it sees, for as long as that seam stays the only one.
//! That is the `MaxExpectedShortfall` defect in a new place: a control that
//! reads as sampling while being unable to decline anything.
//!
//! Under a fixed capacity the density §54.2 asks for is decided somewhere
//! else, and only somewhere else. A memory of 4,096 that admits everything
//! and evicts oldest-first holds the last 4,096 episodes whatever they were:
//! a high-surprise episode is spent to make room for a calm one purely for
//! being older, which is precisely backwards from "most information is in the
//! tails". What the memory *retains* is the sample. So the sampler grades
//! episodes and names the victim when the bound binds, and the shape of the
//! retained set — dense in the tail, sparse in the calm — is the result.
//!
//! # The bound, and why this number
//!
//! Two bounds, and both are hard.
//!
//! The memory's own capacity is unchanged and is still
//! [`super::store::DEFAULT_CAPACITY`]; this module never raises it and never
//! declines to evict. [`EpisodeSampler::victim`] returns `None` only for an
//! empty memory, so a store over capacity always loses exactly one episode
//! per insert, whatever grades it holds. An episodic memory that grew with
//! the stream would be the defect, and a "protect the tail" rule with no
//! second bound is exactly how one is built: every episode would eventually
//! be a tail episode of something.
//!
//! The second bound is the reserve — [`EpisodeSampler::reserve`], capacity
//! over [`TAIL_RESERVE_DIVISOR`]. It bounds the number of **seats**: at most
//! half a memory may be held past the point recency alone would have spent
//! it. Said precisely, because the looser reading is wrong in a case the
//! tests drive — a memory holding nothing but tail episodes is entirely tail
//! and does not breach the reserve, because none of those episodes is being
//! held *against* a calmer one. There is no calmer one. What the reserve
//! forbids is a third, fourth and fifth surprising episode outranking calm
//! episodes when only two seats were reserved; over the reserve, the excess
//! tail is spent oldest-first like anything else, and a stream carrying both
//! grades settles at exactly `reserve` tail episodes held. Half is argued,
//! not rounded to. A
//! reserve of zero is the oldest-first policy this module replaces. A reserve
//! of the whole capacity lets a bad month fill the store and never leave:
//! every recall, including one made on an ordinary morning, would return only
//! disasters, and the precedent a reviewer reads would describe a market that
//! is not the one in front of them. Half is the largest share at which a
//! query about an ordinary situation still has an even chance of an ordinary
//! neighbour, and it is the share at which the tail can never be outvoted by
//! the calm either. Past the reserve the tail is spent oldest-first like
//! anything else: the reserve bounds how much of a bounded memory surprise
//! may hold, and is not a promise to keep every surprising episode.
//!
//! # Reproducible from the log, with no seed at all
//!
//! Every input to [`EpisodeSampler::grade`] is a field of the episode the
//! event log already holds — `realised_move_bps` and `expected_move_bps`,
//! through [`Episode::surprise_bps`] — and the sampler makes no random
//! choice, reads no clock, and holds no state of its own. Two processes
//! replaying the same episodes in the same order retain the same set, because
//! there is nothing else for the answer to depend on. That is deliberately
//! stronger than seeding a sampler from something the log holds: a seeded
//! sampler is reproducible only for as long as everyone agrees on the seed,
//! and this one has no seed to disagree about. [`super::store::LSH_SEED`] is
//! still the whole of the randomness anywhere in this module tree.
//!
//! The store keeps the grades in `BTreeSet`s ordered by the same slot key the
//! episodes are, so "the oldest calm episode" is a total order and not an
//! iteration accident. A `HashSet` there would make the retained set differ
//! between two memories in one process, which is what
//! `two_memories_fed_the_same_episodes_retain_the_same_set` exists to catch.
//!
//! # What this does not implement, said rather than counted
//!
//! §54.2's third clause is "regime transitions", and it is **not** here. A
//! transition is a property of an episode *against its predecessor for the
//! same instrument*, and the predecessor is exactly what eviction removes: an
//! episode that is a transition today stops being one the moment the episode
//! before it is spent, so a grade computed that way would change under the
//! store's own bookkeeping and a replay would not reproduce it. Holding the
//! flag at admission instead would mean a per-instrument regime map beside
//! the store, and a second bound to keep that map from growing with the
//! universe. Both are real designs; neither is this one, and grading on a
//! transition the store cannot state stably would be a number nobody
//! computed. The first clause — dense at high surprise — is the one §54.2's
//! own reasoning rests on, and it is the one implemented.

use super::episode::Episode;
use qip_core::error::{Error, Result};
use serde::{Deserialize, Serialize};

/// The surprise at which an episode is held for its information rather than
/// for its recency, in basis points of the reference its claim was made
/// against.
///
/// One hundred basis points is a one per cent miss. Below it the outcome is
/// noise around the claim — the platform expected forty basis points and got
/// ninety, which is the ordinary business of a horizon; above it the outcome
/// is a different story from the claim, and it is that story §54.2 means by
/// "the tails". The figure is in the same unit
/// [`super::EpisodeOutcome::surprise_bps`] returns, so the threshold and the
/// measurement cannot drift into different scales.
///
/// Compared on magnitude: an outcome that fell a per cent short of its claim
/// is as informative as one that overshot by the same, and the sign is kept
/// on the record for a reader rather than spent on the grade.
pub const HIGH_SURPRISE_BPS: f64 = 100.0;

/// Capacity divided by this is the most of a memory the tail may hold.
///
/// Two — half. See the module documentation for why half and not all.
pub const TAIL_RESERVE_DIVISOR: usize = 2;

/// What an episode is kept for.
///
/// Two arms and not three: "every action" is the whole population at the one
/// production seam, so an `Action` grade would be a label every episode
/// carried and nothing could be sorted by. The module documentation says why.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EpisodeGrade {
    /// The outcome landed near enough to the claim, or was never gradeable
    /// for surprise at all. Spent first when the bound binds.
    ///
    /// An ungradeable episode — a claim with no `expected_move_bps`, which a
    /// `RegimeShift` produces — grades `Calm` and not `Tail`. An absent
    /// expectation is not a large surprise any more than it is a zero one,
    /// and reserving a seat for an episode whose surprise nobody could
    /// compute would fill the reserve with records that carry no tail.
    Calm,
    /// The outcome was at least [`HIGH_SURPRISE_BPS`] from what the claim
    /// expected. Held against the reserve.
    Tail,
}

impl EpisodeGrade {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Calm => "calm",
            Self::Tail => "tail",
        }
    }
}

/// §54.2's episode sampling: the grade, the reserve, and the victim.
///
/// Stateless by construction — it holds its two parameters and nothing about
/// the episodes it has seen — which is what makes a replay reproduce the
/// retained set from the episodes alone.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EpisodeSampler {
    high_surprise_bps: f64,
    reserve_divisor: usize,
}

impl Default for EpisodeSampler {
    fn default() -> Self {
        Self {
            high_surprise_bps: HIGH_SURPRISE_BPS,
            reserve_divisor: TAIL_RESERVE_DIVISOR,
        }
    }
}

impl EpisodeSampler {
    /// A sampler on a stated threshold and reserve divisor.
    ///
    /// Both are refused rather than corrected where they make no sense. A
    /// non-finite or negative threshold grades every episode `Tail` or none
    /// of them, and a divisor of zero has no reserve to divide — each is a
    /// configuration mistake whose silent correction would run every cycle
    /// with a sampling policy nobody chose and no complaint.
    pub fn new(high_surprise_bps: f64, reserve_divisor: usize) -> Result<Self> {
        if !high_surprise_bps.is_finite() || high_surprise_bps < 0.0 {
            return Err(Error::invalid(format!(
                "a high-surprise threshold of {high_surprise_bps} basis points is not a \
                 magnitude; give a finite, non-negative figure in the unit \
                 EpisodeOutcome::surprise_bps returns"
            )));
        }
        if reserve_divisor == 0 {
            return Err(Error::invalid(
                "a tail reserve divisor of zero divides a capacity by nothing; give a divisor of \
                 at least one, where one reserves the whole memory for the tail",
            ));
        }
        Ok(Self {
            high_surprise_bps,
            reserve_divisor,
        })
    }

    pub fn high_surprise_bps(&self) -> f64 {
        self.high_surprise_bps
    }

    pub fn reserve_divisor(&self) -> usize {
        self.reserve_divisor
    }

    /// What an episode is kept for, from the episode alone.
    pub fn grade(&self, episode: &Episode) -> EpisodeGrade {
        match episode.surprise_bps() {
            Some(surprise) if surprise.abs() >= self.high_surprise_bps => EpisodeGrade::Tail,
            _ => EpisodeGrade::Calm,
        }
    }

    /// How many episodes of `capacity` may be held for their surprise rather
    /// than for their recency — the seat count, not a share of the store.
    /// See the module documentation for the difference and why it bites.
    pub fn reserve(&self, capacity: usize) -> usize {
        capacity / self.reserve_divisor
    }

    /// Which slot a memory at `capacity` gives up, given its two grade
    /// indices in slot order, oldest first.
    ///
    /// Takes the two oldest rather than the store so the decision is testable
    /// without building an index, and so the store cannot hand it anything
    /// but the two orders it is allowed to read.
    ///
    /// `None` only when both are absent, which is only an empty memory. A
    /// memory over capacity always loses one — see the module documentation
    /// on the two bounds.
    pub fn victim<S: Copy>(
        &self,
        capacity: usize,
        oldest_calm: Option<S>,
        oldest_tail: Option<S>,
        tail_held: usize,
    ) -> Option<S> {
        if tail_held > self.reserve(capacity) {
            // Over its reserve the tail is ordinary: spent oldest-first like
            // anything else, because the reserve bounds what surprise may
            // hold and is not a promise to keep every surprising episode.
            return oldest_tail.or(oldest_calm);
        }
        // Sparse in calm. The fallback is not decoration: a memory holding
        // nothing but tail episodes inside its reserve — which a small
        // capacity reaches easily — has no calm episode to spend, and a
        // sampler that returned `None` there would let the store grow with
        // the stream.
        oldest_calm.or(oldest_tail)
    }
}

//! Typed, lexicographically sortable identifiers.
//!
//! An [`Id`] is a 26-character Crockford base-32 string: 48 bits of millisecond
//! timestamp followed by 80 bits of entropy (the ULID layout). Sorting ids
//! sorts by creation time, which keeps event logs and database indexes ordered
//! without a secondary key.
//!
//! Ids are typed by a marker so an `Id<Order>` can never be passed where an
//! `Id<Hypothesis>` is expected — a class of bug that plain `String` ids invite.

use crate::rng::{Rng, Xoshiro256};
use crate::time::Timestamp;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::marker::PhantomData;
use std::sync::Mutex;

const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// Marker trait for id namespaces.
pub trait IdKind: Copy + Clone + fmt::Debug + Send + Sync + 'static {
    /// Short prefix used in logs and URLs, e.g. `ord`.
    const PREFIX: &'static str;
}

/// Declare a new id namespace.
#[macro_export]
macro_rules! id_kind {
    ($($name:ident => $prefix:literal),+ $(,)?) => {
        $(
            #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
            pub struct $name;
            impl $crate::ids::IdKind for $name {
                const PREFIX: &'static str = $prefix;
            }
        )+
    };
}

/// A typed identifier.
#[derive(Serialize, Deserialize)]
#[serde(transparent)]
pub struct Id<K: IdKind> {
    value: String,
    #[serde(skip)]
    _kind: PhantomData<K>,
}

impl<K: IdKind> Id<K> {
    /// Wrap an existing string. Used when reading persisted records.
    pub fn from_string(value: impl Into<String>) -> Self {
        Self {
            value: value.into(),
            _kind: PhantomData,
        }
    }

    pub fn as_str(&self) -> &str {
        &self.value
    }

    pub fn into_string(self) -> String {
        self.value
    }

    /// Namespace prefix, e.g. `ord`.
    pub fn prefix() -> &'static str {
        K::PREFIX
    }

    /// Reinterpret in another namespace. Deliberately explicit and rare —
    /// used only where one record is projected into another's identity space.
    pub fn retype<T: IdKind>(&self) -> Id<T> {
        Id::from_string(self.value.clone())
    }
}

impl<K: IdKind> Clone for Id<K> {
    fn clone(&self) -> Self {
        Self {
            value: self.value.clone(),
            _kind: PhantomData,
        }
    }
}

impl<K: IdKind> PartialEq for Id<K> {
    fn eq(&self, other: &Self) -> bool {
        self.value == other.value
    }
}

impl<K: IdKind> Eq for Id<K> {}

impl<K: IdKind> PartialOrd for Id<K> {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl<K: IdKind> Ord for Id<K> {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.value.cmp(&other.value)
    }
}

impl<K: IdKind> std::hash::Hash for Id<K> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.value.hash(state);
    }
}

impl<K: IdKind> fmt::Display for Id<K> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.value)
    }
}

impl<K: IdKind> fmt::Debug for Id<K> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", K::PREFIX, self.value)
    }
}

impl<K: IdKind> AsRef<str> for Id<K> {
    fn as_ref(&self) -> &str {
        &self.value
    }
}

/// Mints identifiers deterministically from a seed.
///
/// Two runs with the same seed that mint ids in the same order produce the same
/// ids, which is what lets a replayed run be diffed byte-for-byte against the
/// original.
#[derive(Debug)]
pub struct IdGenerator {
    rng: Mutex<Xoshiro256>,
}

impl IdGenerator {
    pub fn new(seed: u64) -> Self {
        Self {
            rng: Mutex::new(Xoshiro256::seeded(seed)),
        }
    }

    /// Mint an id stamped with `at`.
    pub fn generate<K: IdKind>(&self, at: Timestamp) -> Id<K> {
        let millis = at.as_millis().max(0) as u128;
        let entropy = {
            let mut rng = self.rng.lock().unwrap_or_else(|e| e.into_inner());
            (u128::from(rng.next_u64()) << 16) | u128::from(rng.next_u64() as u16)
        };
        let raw = ((millis & 0xFFFF_FFFF_FFFF) << 80) | (entropy & ((1u128 << 80) - 1));
        Id::from_string(encode_base32(raw))
    }

    /// Discard `draws` outputs from the entropy stream without minting an id.
    ///
    /// A generator is seeded purely from a configured seed, so two
    /// generators built from the same seed and asked for an id stamped with
    /// the same millisecond produce the same id — deliberate, since it is
    /// what makes a replay reproducible. That guarantee turns into a
    /// collision the moment a caller resumes an existing log rather than
    /// starting one from scratch: the resumed run's first id would coincide
    /// with the id a from-scratch run at the same instant would have minted,
    /// even though the resumed run's record sits at a different position in
    /// a longer chain. Advancing the stream by however much history was
    /// already inherited moves a resumed run's entropy off that fresh-run
    /// starting point, while a from-scratch caller (nothing to advance past)
    /// is unaffected and keeps minting exactly the ids it always has.
    pub fn advance(&self, draws: u64) {
        let mut rng = self.rng.lock().unwrap_or_else(|e| e.into_inner());
        for _ in 0..draws {
            let _ = rng.next_u64();
        }
    }
}

/// Encode 128 bits as 26 Crockford base-32 characters, most significant first.
fn encode_base32(mut value: u128) -> String {
    let mut buf = [0u8; 26];
    for slot in buf.iter_mut().rev() {
        *slot = ALPHABET[(value & 0x1f) as usize];
        value >>= 5;
    }
    String::from_utf8(buf.to_vec()).unwrap_or_default()
}

/// Recover the creation time encoded in an id.
pub fn timestamp_of<K: IdKind>(id: &Id<K>) -> Option<Timestamp> {
    let bytes = id.as_str().as_bytes();
    if bytes.len() != 26 {
        return None;
    }
    let mut value: u128 = 0;
    for &c in bytes {
        let idx = ALPHABET.iter().position(|&a| a == c)?;
        value = (value << 5) | idx as u128;
    }
    let millis = (value >> 80) & 0xFFFF_FFFF_FFFF;
    Some(Timestamp::from_millis(millis as i64))
}

// Every identifier namespace in the platform. Keeping them in one place makes
// the domain's nouns legible at a glance.
crate::id_kind! {
    ObjectKind        => "obj",
    EntityKind        => "ent",
    EventKind         => "evt",
    SignalKind        => "sig",
    AnomalyKind       => "anm",
    OpportunityKind   => "opp",
    HypothesisKind    => "hyp",
    EvidenceKind      => "evd",
    ChallengeKind     => "chl",
    SimulationKind    => "sim",
    ScenarioKind      => "scn",
    StrategyKind      => "str",
    ProposalKind      => "pps",
    OptimizationKind  => "opt",
    PortfolioKind     => "prt",
    PositionKind      => "pos",
    OrderKind         => "ord",
    ExecutionKind     => "exe",
    FillKind          => "fil",
    DecisionKind      => "dec",
    AttributionKind   => "atr",
    LessonKind        => "lsn",
    AgentRunKind      => "run",
    ModelKind         => "mdl",
    TraceKind         => "trc",
    RequestKind       => "req",
    AuditKind         => "aud",
    JobKind           => "job",
}

pub type ObjectId = Id<ObjectKind>;
pub type EntityId = Id<EntityKind>;
pub type EventId = Id<EventKind>;
pub type SignalId = Id<SignalKind>;
pub type AnomalyId = Id<AnomalyKind>;
pub type OpportunityId = Id<OpportunityKind>;
pub type HypothesisId = Id<HypothesisKind>;
pub type EvidenceId = Id<EvidenceKind>;
pub type ChallengeId = Id<ChallengeKind>;
pub type SimulationId = Id<SimulationKind>;
pub type ScenarioId = Id<ScenarioKind>;
pub type StrategyId = Id<StrategyKind>;
pub type ProposalId = Id<ProposalKind>;
pub type OptimizationId = Id<OptimizationKind>;
pub type PortfolioId = Id<PortfolioKind>;
pub type PositionId = Id<PositionKind>;
pub type OrderId = Id<OrderKind>;
pub type ExecutionId = Id<ExecutionKind>;
pub type FillId = Id<FillKind>;
pub type DecisionId = Id<DecisionKind>;
pub type AttributionId = Id<AttributionKind>;
pub type LessonId = Id<LessonKind>;
pub type AgentRunId = Id<AgentRunKind>;
pub type ModelId = Id<ModelKind>;
pub type RequestId = Id<RequestKind>;
pub type AuditId = Id<AuditKind>;
pub type JobId = Id<JobKind>;

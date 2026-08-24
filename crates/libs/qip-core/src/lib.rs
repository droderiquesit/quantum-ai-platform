//! `qip-core` — the deterministic substrate every other crate is built on.
//!
//! Three rules hold platform-wide and are enforced here:
//!
//! 1. **No ambient nondeterminism.** Wall-clock time and randomness are only
//!    reachable through [`Clock`] and [`Rng`], which are injected. A replay of
//!    the same event log with the same seed produces byte-identical output.
//! 2. **Money is exact.** [`Decimal`] is fixed-point over `i128`; accounting
//!    identities hold to the cent. Floating point is reserved for statistics
//!    (returns, volatility, risk), never for balances or quantities.
//! 3. **Everything is traceable.** [`Lineage`] threads a correlation id through
//!    every event so any investment decision can be reconstructed end to end.

pub mod config;
pub mod decimal;
pub mod error;
pub mod hash;
pub mod ids;
pub mod kv;
pub mod lineage;
pub mod money;
pub mod rng;
pub mod testing;
pub mod time;

pub use config::{Config, ConfigError};
pub use decimal::Decimal;
pub use error::{Error, Result};
pub use hash::{Hasher256, hmac_sha256, sha256, sha256_hex};
pub use ids::{
    AgentRunId, AnomalyId, AttributionId, AuditId, ChallengeId, DecisionId, EntityId, EventId,
    EvidenceId, ExecutionId, FillId, HypothesisId, Id, IdGenerator, JobId, LessonId, ModelId,
    ObjectId, OpportunityId, OptimizationId, OrderId, PortfolioId, PositionId, ProposalId,
    RequestId, ScenarioId, SignalId, SimulationId, StrategyId,
};
pub use kv::{KeyValueStore, KeyValueStoreExt};
pub use lineage::{CausationId, CorrelationId, Lineage, TraceId};
pub use money::{Currency, Money};
pub use rng::{Rng, Xoshiro256};
pub use time::{Clock, Duration, ManualClock, SystemClock, Timestamp};

/// Bundle of the ambient capabilities a deterministic component may use.
///
/// Components take a `&Context` rather than reaching for `SystemTime::now()` or
/// a thread-local RNG. Swapping in a [`ManualClock`] and a fixed seed is what
/// makes simulation, replay and tests reproducible.
#[derive(Debug)]
pub struct Context {
    clock: std::sync::Arc<dyn Clock>,
    ids: IdGenerator,
    seed: u64,
}

impl Context {
    pub fn new(clock: std::sync::Arc<dyn Clock>, seed: u64) -> Self {
        let ids = IdGenerator::new(seed);
        Self { clock, ids, seed }
    }

    /// A context wired to a [`ManualClock`] — the default for tests and replay.
    pub fn deterministic(start: Timestamp, seed: u64) -> (Self, std::sync::Arc<ManualClock>) {
        let clock = std::sync::Arc::new(ManualClock::new(start));
        (Self::new(clock.clone(), seed), clock)
    }

    pub fn now(&self) -> Timestamp {
        self.clock.now()
    }

    pub fn clock(&self) -> &std::sync::Arc<dyn Clock> {
        &self.clock
    }

    pub fn ids(&self) -> &IdGenerator {
        &self.ids
    }

    pub fn seed(&self) -> u64 {
        self.seed
    }

    /// A fresh RNG stream derived from the context seed and a domain label.
    ///
    /// Deriving per-domain streams keeps one component's random draws from
    /// shifting another's, which would otherwise make replays diverge whenever
    /// unrelated code changed how much randomness it consumed.
    pub fn rng_for(&self, domain: &str) -> Xoshiro256 {
        let mut h = Hasher256::new();
        h.update(&self.seed.to_le_bytes());
        h.update(domain.as_bytes());
        let digest = h.finish();
        let mut s = [0u8; 8];
        s.copy_from_slice(&digest[..8]);
        Xoshiro256::seeded(u64::from_le_bytes(s))
    }
}

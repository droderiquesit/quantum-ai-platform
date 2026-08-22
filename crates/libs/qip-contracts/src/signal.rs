//! What a compiled strategy emits.

use qip_core::{Decimal, ObjectId, Timestamp};
use serde::{Deserialize, Serialize};
use std::fmt;

/// A deployed strategy's stable identity.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct StrategyId(String);

impl StrategyId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for StrategyId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// What the strategy wants done.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SignalKind {
    /// Open or increase a position.
    Enter,
    /// Reduce or close a position.
    Exit,
    /// Offset existing exposure without changing the view.
    Hedge,
    /// The conditions no longer hold; stop acting on prior signals.
    Stand,
}

impl SignalKind {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Enter => "enter",
            Self::Exit => "exit",
            Self::Hedge => "hedge",
            Self::Stand => "stand",
        }
    }

    /// Whether acting on this can increase risk.
    ///
    /// The risk gate blocks these when the book is impaired and lets the
    /// others through, because refusing an exit during a drawdown is how a
    /// safety control becomes the accident.
    pub const fn increases_risk(&self) -> bool {
        matches!(self, Self::Enter)
    }
}

/// How much the strategy believes its own signal.
///
/// A probability in `[0, 1]` with the sample size behind it. The sample size
/// is not decoration: a 0.9 from four observations and a 0.9 from four
/// thousand size very differently, and a conviction without one invites the
/// allocator to treat them the same.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Conviction {
    probability: f64,
    observations: u32,
}

impl Conviction {
    /// Clamps rather than refusing: a strategy that computes 1.01 has a bug
    /// worth finding, but dropping its signal loses information too.
    pub fn new(probability: f64, observations: u32) -> Self {
        Self {
            probability: probability.clamp(0.0, 1.0),
            observations,
        }
    }

    pub const fn probability(&self) -> f64 {
        self.probability
    }

    pub const fn observations(&self) -> u32 {
        self.observations
    }

    /// The probability shrunk toward a coin flip by how little evidence
    /// supports it.
    ///
    /// With no observations this returns 0.5 whatever the strategy claimed,
    /// which is the honest answer for a belief with nothing behind it.
    pub fn shrunk(&self) -> f64 {
        let n = f64::from(self.observations);
        let weight = n / (n + 30.0);
        0.5 + weight * (self.probability - 0.5)
    }

    /// Whether the belief clears a bar after shrinkage.
    pub fn clears(&self, bar: f64) -> bool {
        self.shrunk() >= bar
    }
}

/// A strategy's output for one instrument at one instant.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Signal {
    pub strategy: StrategyId,
    pub object_id: ObjectId,
    pub kind: SignalKind,
    pub conviction: Conviction,
    /// Size the strategy wants, before the allocator has its say. Always the
    /// strategy's own view; the capital envelope is what makes it real.
    pub desired_quantity: Decimal,
    /// How long the signal remains actionable. A signal without an expiry gets
    /// acted on at the worst possible moment.
    pub valid_until: Timestamp,
    /// The feature revisions the signal was computed from, so a fill can be
    /// attributed to exactly the inputs that produced it.
    pub inputs: Vec<(String, u64)>,
    pub at: Timestamp,
}

impl Signal {
    pub fn is_live(&self, now: Timestamp) -> bool {
        now <= self.valid_until
    }
}

//! A position's life history, discover to attribute (blueprint ASSET-005).
//!
//! [`crate::PositionLifecycle`] says what the desk is *doing* about a held
//! position. This says where the position is in its life, and the one thing it
//! exists to enforce: nothing reaches `Attribute` without having been settled,
//! so no result is credited for a position whose ownership never cleared.
//! Hold, hedge, monitor and resize repeat for as long as the position lives;
//! the other stages happen once, in order.

use qip_core::{Error, Result};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LifeStage {
    Discover,
    Evaluate,
    Acquire,
    /// Held, including hedged: hedging changes the exposure, not the stage.
    Hold,
    Monitor,
    Resize,
    /// Exit or divestment.
    Exit,
    Settle,
    Attribute,
}

impl LifeStage {
    pub const ALL: [Self; 9] = [
        Self::Discover,
        Self::Evaluate,
        Self::Acquire,
        Self::Hold,
        Self::Monitor,
        Self::Resize,
        Self::Exit,
        Self::Settle,
        Self::Attribute,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Discover => "discover",
            Self::Evaluate => "evaluate",
            Self::Acquire => "acquire",
            Self::Hold => "hold",
            Self::Monitor => "monitor",
            Self::Resize => "resize",
            Self::Exit => "exit",
            Self::Settle => "settle",
            Self::Attribute => "attribute",
        }
    }

    /// Whether `next` may follow `self`. One table, so the legal moves read
    /// as a list and a stage-skip is the absence of a line.
    pub fn allows(self, next: Self) -> bool {
        use LifeStage::{
            Acquire, Attribute, Discover, Evaluate, Exit, Hold, Monitor, Resize, Settle,
        };
        matches!(
            (self, next),
            (Discover, Evaluate)
                | (Evaluate, Acquire)
                | (Acquire, Hold)
                | (Hold, Hold)
                | (Hold, Monitor)
                | (Monitor, Monitor)
                | (Monitor, Hold)
                | (Monitor, Resize)
                | (Resize, Resize)
                | (Resize, Hold)
                | (Resize, Monitor)
                | (Hold, Exit)
                | (Monitor, Exit)
                | (Resize, Exit)
                | (Exit, Settle)
                | (Settle, Attribute)
        )
    }

    pub fn transition(self, next: Self) -> Result<Self> {
        if !self.allows(next) {
            return Err(Error::invalid(format!(
                "a position cannot move from {} to {}; follow the life order \
                 discover, evaluate, acquire, hold, exit, settle, attribute",
                self.as_str(),
                next.as_str()
            )));
        }
        Ok(next)
    }

    /// Rebuild the stage from the recorded sequence, which must begin at
    /// `Discover`. Replaying the same log always gives the same stage, and a
    /// log with a skipped step is refused rather than repaired.
    pub fn replay(history: &[Self]) -> Result<Self> {
        let (first, rest) = history.split_first().ok_or_else(|| {
            Error::invalid("an empty stage history names no position; supply at least discover")
        })?;
        if *first != Self::Discover {
            return Err(Error::invalid(format!(
                "a stage history begins at {}; every position begins at discover",
                first.as_str()
            )));
        }
        rest.iter()
            .try_fold(*first, |at, next| at.transition(*next))
    }
}

#[cfg(test)]
#[allow(clippy::panic_in_result_fn)] // the assertion is the deliverable in a test
mod tests {
    use super::*;
    use LifeStage::*;

    #[test]
    fn only_the_sixteen_named_edges_are_legal_and_every_other_pair_is_refused() {
        let legal: Vec<(LifeStage, LifeStage)> = LifeStage::ALL
            .iter()
            .flat_map(|a| LifeStage::ALL.iter().map(move |b| (*a, *b)))
            .filter(|(a, b)| a.allows(*b))
            .collect();
        // Premise: the table is neither empty nor everything.
        assert_eq!(legal.len(), 16);
        for a in LifeStage::ALL {
            for b in LifeStage::ALL {
                assert_eq!(a.transition(b).is_ok(), legal.contains(&(a, b)));
            }
        }
    }

    #[test]
    fn exit_cannot_reach_attribute_without_settling_first() {
        assert!(Exit.transition(Attribute).is_err());
        assert!(Exit.transition(Settle).is_ok());
        assert!(Settle.transition(Attribute).is_ok());
        for stage in [Discover, Evaluate, Acquire, Hold, Monitor, Resize] {
            assert!(stage.transition(Attribute).is_err());
        }
    }

    #[test]
    fn a_full_life_with_repeated_hold_monitor_and_resize_replays_to_attribute() -> Result<()> {
        let life = [
            Discover, Evaluate, Acquire, Hold, Hold, Monitor, Resize, Hold, Monitor, Exit, Settle,
            Attribute,
        ];
        assert_eq!(LifeStage::replay(&life)?, Attribute);
        // Replay is a pure function of the history: same log, same stage.
        assert_eq!(LifeStage::replay(&life)?, LifeStage::replay(&life)?);
        assert_eq!(LifeStage::replay(&life[..7])?, Resize);
        Ok(())
    }

    #[test]
    fn a_history_that_skips_a_stage_or_starts_late_is_refused() {
        assert!(LifeStage::replay(&[]).is_err());
        assert!(LifeStage::replay(&[Evaluate]).is_err());
        assert!(LifeStage::replay(&[Discover, Acquire]).is_err());
        assert!(LifeStage::replay(&[Discover, Evaluate, Acquire, Hold, Exit, Attribute]).is_err());
    }
}

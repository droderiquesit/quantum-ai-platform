//! World model branch spawning mechanism.
//!
//! When a world model's residual (unexplained component of P&L) exceeds an
//! insufficiency threshold, the platform spawns a competing hypothesis branch
//! to explore alternative explanations. This module manages the creation and
//! tracking of those branches.

use crate::federation::Federation;
use qip_core::error::{Error, Result};
use qip_core::{Decimal, Timestamp};
use serde::{Deserialize, Serialize};

/// A proposed branch of the world model, triggered by residual insufficiency.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WorldModelBranch {
    /// Unique identifier for this branch.
    pub id: String,
    /// The parent world model or branch this was spawned from.
    pub parent_id: String,
    /// Hypothesis this branch tests (alternative explanation for the residual).
    pub hypothesis: String,
    /// The residual that triggered this branch proposal.
    pub trigger_residual: Decimal,
    /// Insufficient residual threshold that was exceeded.
    pub insufficiency_threshold: Decimal,
    /// When the branch was proposed.
    pub proposed_at: Timestamp,
}

impl WorldModelBranch {
    pub fn new(
        id: String,
        parent_id: String,
        hypothesis: String,
        trigger_residual: Decimal,
        insufficiency_threshold: Decimal,
        proposed_at: Timestamp,
    ) -> Self {
        Self {
            id,
            parent_id,
            hypothesis,
            trigger_residual,
            insufficiency_threshold,
            proposed_at,
        }
    }
}

/// Spawner orchestrates branch creation when residuals exceed insufficiency.
#[derive(Debug)]
pub struct WorldModelSpawner {
    /// Threshold (in absolute Decimal value) at which residuals trigger branching.
    insufficiency_threshold: Decimal,
}

impl WorldModelSpawner {
    pub fn new(insufficiency_threshold: Decimal) -> Result<Self> {
        if insufficiency_threshold <= Decimal::ZERO {
            return Err(Error::invalid("insufficiency threshold must be positive"));
        }
        Ok(Self {
            insufficiency_threshold,
        })
    }

    /// Check if residual exceeds the insufficiency threshold.
    pub fn should_spawn(&self, residual: Decimal) -> bool {
        residual.abs() > self.insufficiency_threshold
    }

    /// Propose a new branch when residual exceeds insufficiency threshold.
    ///
    /// The branch is NOT automatically added to the federation here; it is
    /// proposed and must enter service only through the gated pipeline
    /// (validated, approved, and integrated). The caller is responsible for
    /// checking should_spawn() before calling this.
    pub fn propose_branch(
        &self,
        parent_id: &str,
        residual: Decimal,
        branch_id: &str,
        hypothesis: &str,
        at: Timestamp,
    ) -> Result<WorldModelBranch> {
        if !self.should_spawn(residual) {
            return Err(Error::invalid(format!(
                "residual {residual} does not exceed insufficiency threshold {}",
                self.insufficiency_threshold
            )));
        }

        if branch_id.trim().is_empty() || hypothesis.trim().is_empty() {
            return Err(Error::invalid("branch id and hypothesis must be non-empty"));
        }

        Ok(WorldModelBranch::new(
            branch_id.to_string(),
            parent_id.to_string(),
            hypothesis.to_string(),
            residual,
            self.insufficiency_threshold,
            at,
        ))
    }

    /// Register a proposed branch into the federation's journal.
    ///
    /// This records the branch with its parent and triggering evidence,
    /// entering the platform's decision pipeline. The branch is marked Live
    /// but must pass approval gates before being used for inference.
    pub fn register_branch(
        &self,
        federation: &mut Federation,
        proposed: &WorldModelBranch,
    ) -> Result<()> {
        let trigger_evidence = format!(
            "residual_insufficiency: {} exceeds {}",
            proposed.trigger_residual, proposed.insufficiency_threshold
        );

        federation.branch(
            &proposed.parent_id,
            &proposed.id,
            &proposed.hypothesis,
            &trigger_evidence,
            proposed.proposed_at,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_spawner_rejects_non_positive_thresholds() {
        assert!(WorldModelSpawner::new(Decimal::ZERO).is_err());
        assert!(WorldModelSpawner::new(Decimal::from_scaled(-1, 0).unwrap()).is_err());
    }

    #[test]
    fn a_spawner_correctly_identifies_residuals_exceeding_threshold() {
        let spawner = WorldModelSpawner::new(Decimal::from_scaled(5, 1).unwrap()).expect("spawner");
        // Threshold is 0.5
        assert!(!spawner.should_spawn(Decimal::from_scaled(4, 1).unwrap())); // 0.4 < 0.5
        assert!(!spawner.should_spawn(Decimal::from_scaled(5, 1).unwrap())); // 0.5 = 0.5 (not >)
        assert!(spawner.should_spawn(Decimal::from_scaled(6, 1).unwrap())); // 0.6 > 0.5
        // Negative residuals also trigger on absolute value
        assert!(spawner.should_spawn(Decimal::from_scaled(-6, 1).unwrap())); // |-0.6| > 0.5
    }

    #[test]
    fn proposing_a_branch_requires_exceeding_threshold() {
        let spawner = WorldModelSpawner::new(Decimal::from_scaled(1, 0).unwrap()).expect("spawner");
        let now = Timestamp::from_millis(1_000_000_000);

        // Below threshold: rejected
        let below = spawner.propose_branch(
            "parent",
            Decimal::from_scaled(5, 1).unwrap(),
            "branch-1",
            "hypo",
            now,
        );
        assert!(below.is_err());

        // Above threshold: accepted
        let above = spawner.propose_branch(
            "parent",
            Decimal::from_scaled(15, 1).unwrap(),
            "branch-2",
            "hypo",
            now,
        );
        assert!(above.is_ok());
        let branch = above.unwrap();
        assert_eq!(branch.parent_id, "parent");
        assert_eq!(branch.hypothesis, "hypo");
        assert_eq!(
            branch.trigger_residual,
            Decimal::from_scaled(15, 1).unwrap()
        );
    }

    #[test]
    fn proposing_a_branch_requires_non_empty_id_and_hypothesis() {
        let spawner = WorldModelSpawner::new(Decimal::from_scaled(1, 0).unwrap()).expect("spawner");
        let now = Timestamp::from_millis(1_000_000_000);
        let residual = Decimal::from_scaled(2, 0).unwrap(); // Above threshold

        let no_id = spawner.propose_branch("parent", residual, "", "hypo", now);
        assert!(no_id.is_err());

        let no_hypo = spawner.propose_branch("parent", residual, "branch", "", now);
        assert!(no_hypo.is_err());

        let valid = spawner.propose_branch("parent", residual, "branch", "hypo", now);
        assert!(valid.is_ok());
    }

    #[test]
    fn mutation_verify_threshold_is_exclusive() {
        let spawner = WorldModelSpawner::new(Decimal::from_scaled(1, 0).unwrap()).expect("spawner");
        // If mutation deletes the `>`, test passes when it should fail at 1.0
        assert!(!spawner.should_spawn(Decimal::from_scaled(1, 0).unwrap())); // Exactly at threshold (1.0)
        assert!(spawner.should_spawn(Decimal::from_scaled(11, 1).unwrap())); // Just above (1.1)
    }
}

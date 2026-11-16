//! One score per refused cell cycle, with the legs' regret summed
//! (R12-MESH-5068, blueprint §17.2).
//!
//! The centre prices each leg of a refused cycle alone
//! ([`crate::platform::CellDeclinedScore`]). A cycle is one decision, and the
//! question the row asks, whether the coordination rule that refused it was too
//! conservative, is about the cycle: a triangle whose first leg would have
//! earned and whose other two would have lost more is a refusal that was right,
//! and reading its legs separately reports a regret that the cycle did not
//! have. So the cycle's score is the sum of its legs' simulated earnings, and
//! it is a regret only when that sum is positive.
//!
//! A pure roll-up over the scores already held: it prices nothing and writes
//! nothing, so a replay of the same log rolls up to the same cycles.

use crate::platform::CellDeclinedScore;
use qip_core::{Decimal, Timestamp};
use qip_twin::value::Simulated;
use std::collections::BTreeMap;

/// The suffix [`crate::platform::Platform`] gives each queued leg:
/// `<cell>/<reported_at>/refusal-<i>/leg-<n>`.
const LEG_MARKER: &str = "/leg-";

/// The cycle a leg's order id belongs to: the id without its `/leg-<n>` tail.
pub fn cycle_key(order_id: &str) -> &str {
    order_id
        .rsplit_once(LEG_MARKER)
        .map_or(order_id, |(head, _)| head)
}

/// What one refused cycle would have earned, legs together.
#[derive(Clone, Debug, PartialEq)]
pub struct CellCycleScore {
    /// `<cell>/<reported_at>/refusal-<i>`.
    pub cycle: String,
    pub cell: String,
    /// The cell's gate that refused the cycle, kept on the score and never a
    /// metric label.
    pub gate: String,
    pub declined_at: Timestamp,
    /// The latest instant any leg was priced.
    pub scored_at: Timestamp,
    /// How many legs are in this roll-up. A cycle whose later legs are still
    /// waiting for their horizon shows fewer than it will.
    pub legs_scored: usize,
    /// The sum of the legs' simulated earnings.
    pub would_have_earned: Simulated<Decimal>,
    /// Whether taking the whole cycle would have beaten standing aside:
    /// the sum is positive, not any one leg.
    pub regret: bool,
}

/// Roll the per-leg scores up to one score per cycle, ordered by cycle key.
pub fn roll_up(scores: &[CellDeclinedScore]) -> Vec<CellCycleScore> {
    let mut cycles: BTreeMap<String, CellCycleScore> = BTreeMap::new();
    for score in scores {
        let key = cycle_key(score.order_id.as_str()).to_string();
        let entry = cycles.entry(key.clone()).or_insert_with(|| CellCycleScore {
            cycle: key,
            cell: score.cell.clone(),
            gate: score.gate.clone(),
            declined_at: score.declined_at,
            scored_at: score.scored_at,
            legs_scored: 0,
            would_have_earned: Simulated::ZERO,
            regret: false,
        });
        entry.legs_scored += 1;
        entry.would_have_earned = entry.would_have_earned + score.would_have_earned;
        entry.scored_at = entry.scored_at.max(score.scored_at);
    }
    cycles
        .into_values()
        .map(|mut cycle| {
            cycle.regret = cycle.would_have_earned.is_positive();
            cycle
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use qip_core::ids::OrderId;
    use qip_core::{ObjectId, dec};
    use qip_twin::counterfactual::EstimationMethod;

    fn leg(cycle: &str, n: usize, earned: Decimal) -> CellDeclinedScore {
        CellDeclinedScore {
            order_id: OrderId::from_string(format!("{cycle}/leg-{n}")),
            cell: "cell-a".to_string(),
            gate: "arbitrage_cycle".to_string(),
            object_id: ObjectId::from_string("AAA".to_string()),
            venue: "XLON".to_string(),
            declined_at: Timestamp::from_secs(1_760_000_000),
            scored_at: Timestamp::from_secs(1_760_000_000 + n as i64),
            would_have_earned: Simulated::of(earned),
            // Each leg's own flag would say regret for a positive leg: the
            // roll-up must not read it.
            regret: earned.is_positive(),
            method: EstimationMethod::Unrecorded,
        }
    }

    /// A cycle whose first leg earned 10 and whose other two lost 7 each was a
    /// refusal that was right. Read leg by leg it shows one regret; read as the
    /// cycle it is not one.
    ///
    /// Mutation: `regret` taken from any leg's own flag (or from the first
    /// leg) fails the final assertion; summing only the positive legs fails
    /// the sum.
    #[test]
    fn a_cycle_is_a_regret_only_when_its_legs_together_would_have_earned() {
        let r0 = "cell-a/2025-10-09T08:53:20Z/refusal-0";
        let r1 = "cell-a/2025-10-09T08:53:20Z/refusal-1";
        let scores = vec![
            leg(r0, 0, dec!("10")),
            leg(r0, 1, dec!("-7")),
            leg(r0, 2, dec!("-7")),
            leg(r1, 0, dec!("4")),
            leg(r1, 1, dec!("-1")),
        ];
        assert!(
            scores.iter().filter(|s| s.regret).count() == 2,
            "premise: two legs read as regrets one at a time"
        );
        let cycles = roll_up(&scores);
        assert_eq!(cycles.len(), 2, "one score per cycle: {cycles:?}");
        assert_eq!(cycles[0].cycle, r0);
        assert_eq!(cycles[0].legs_scored, 3);
        assert_eq!(cycles[0].would_have_earned, Simulated::of(dec!("-4")));
        assert!(
            !cycles[0].regret,
            "a cycle that would have lost was a regret"
        );
        assert_eq!(cycles[1].would_have_earned, Simulated::of(dec!("3")));
        assert!(cycles[1].regret);
        assert_eq!(
            cycles[1].scored_at,
            Timestamp::from_secs(1_760_000_001),
            "the cycle is scored when its last leg was"
        );
    }

    #[test]
    fn the_cycle_key_drops_only_the_leg_suffix() {
        assert_eq!(cycle_key("c/t/refusal-3/leg-12"), "c/t/refusal-3");
        assert_eq!(cycle_key("no-leg-here"), "no-leg-here");
    }
}

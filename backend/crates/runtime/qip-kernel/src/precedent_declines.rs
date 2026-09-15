//! Blueprint §10.3's last query: "what did we decline in situations like
//! this, and should we have?", answered by joining the episodes REASON
//! recalled to the counterfactual scores the twin has already produced.
//!
//! # What was missing, and it was not the arithmetic
//!
//! The platform already prices what a veto cost: [`DeclinedScore`] carries
//! the gate that refused, what the path would have earned, and the `regret`
//! bit saying whether it would have beaten standing aside.
//! [`crate::rule_review`] accumulates those per rule across everything the
//! platform ever declined. What nothing could ask was the *situation-scoped*
//! form of the same question — of the episodes that resemble the one in
//! front of us now, which ones were refused, by which control, and did the
//! refusal hold up — because no code joined an episode to a score.
//!
//! # The join is exact, and that is why it is worth making
//!
//! An episode's id is its hypothesis's id under one prefix
//! (`platform::EPISODE_ID_PREFIX`), and a refused order carries the
//! hypotheses it implements — `Order::hypotheses`, required at construction
//! because "an untraceable order is one nobody can explain after the fact".
//! So a recalled episode and a scored refusal meet on a hypothesis id that
//! both sides already hold. Nothing here matches on instrument, on a time
//! window, or on anything else a reader would have to be told the tolerance
//! of: an unjoinable score is left out rather than attached to the nearest
//! plausible episode, because a join that guesses produces gate evidence
//! nobody can check.
//!
//! # What this must never become
//!
//! Evidence, and only evidence. Blueprint §12.4 is explicit that "a veto
//! rule may only be loosened through the full approval path, never
//! automatically from counterfactual evidence", so nothing here returns a
//! multiplier, a bound or a suggestion: it returns counts, they are recorded
//! beside the precedent, and a person reads them. A function in this module
//! that widened anything would be the one seam by which an unreviewed
//! statistic reached a limit.

use std::collections::{BTreeMap, BTreeSet};

use qip_ai::memory::Recalled;
use qip_core::ids::OrderId;
use serde::{Deserialize, Serialize};

use crate::platform::{DeclinedScore, EPISODE_ID_PREFIX};

/// The hypothesis an episode was written for, or `None` where the id was not
/// written by this platform's own episode writer.
///
/// `None` rather than the whole id, which is what a `trim_start_matches` or
/// an unchecked slice would have given: an id from another writer that
/// happened to contain a hypothesis id would join a score to an episode
/// nobody reasoned, and the count would read exactly like a real one.
pub fn hypothesis_of(episode_id: &str) -> Option<&str> {
    episode_id.strip_prefix(EPISODE_ID_PREFIX)
}

/// One control's scored refusals among the recalled analogues.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GateDeclines {
    /// Scored refusals this gate produced on the analogues' own hypotheses.
    pub declines: usize,
    /// Of those, the ones the twin says would have beaten standing aside.
    pub regretted: usize,
}

/// What the platform declined in the situations it has just recalled.
///
/// Counts and nothing else, for the reason the module doc gives.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrecedentDeclines {
    /// Episodes recalled. **The premise of every figure below**: with none
    /// recalled, `declines` is zero because nothing was asked, not because
    /// nothing was declined, and the two must never read alike.
    pub analogues: usize,
    /// Of those, the ones whose hypothesis at least one scored refusal names.
    pub analogues_matched: usize,
    /// Scored refusals joined to those analogues. A refused order naming two
    /// of them is one refusal, counted once and marking both as matched:
    /// counting it twice would inflate the evidence a gate is judged on with
    /// the arity of a proposal's legs.
    pub declines: usize,
    /// Of those, the ones the twin says would have beaten standing aside —
    /// §10.3's "and should we have?".
    pub regretted: usize,
    /// The same split per control, in gate order so a replay renders it the
    /// way the cycle did.
    pub by_gate: BTreeMap<String, GateDeclines>,
}

impl PrecedentDeclines {
    /// Whether this is evidence of anything.
    ///
    /// An empty `by_gate` means "nothing analogous was ever refused" only
    /// when something analogous was recalled *and* the twin had scored
    /// something to join against. A caller that reported the first while
    /// holding the second would have built the control that cannot fire and
    /// reads as protection — the shape this repository already records one
    /// example of in `MaxExpectedShortfall`.
    pub fn was_answerable(&self) -> bool {
        self.analogues > 0 && self.declines > 0
    }
}

/// Join the recalled analogues to the scored refusals, on the hypothesis id
/// both sides carry.
///
/// `hypotheses_of` answers what a refused order was implementing; it is
/// supplied rather than reached for because the order book lives in the
/// platform and this module holds the arithmetic. An order the book no
/// longer has contributes nothing rather than a guess.
pub fn join<F>(
    recalled: &[Recalled],
    scores: &[DeclinedScore],
    hypotheses_of: F,
) -> PrecedentDeclines
where
    F: Fn(&OrderId) -> Vec<String>,
{
    let analogues: BTreeSet<&str> = recalled
        .iter()
        .filter_map(|entry| hypothesis_of(&entry.episode.episode_id))
        .collect();
    let mut joined = PrecedentDeclines {
        analogues: recalled.len(),
        ..PrecedentDeclines::default()
    };
    if analogues.is_empty() {
        return joined;
    }
    let mut matched: BTreeSet<&str> = BTreeSet::new();
    for score in scores {
        let carried = hypotheses_of(&score.order_id);
        let hits: Vec<&str> = carried
            .iter()
            .filter_map(|hypothesis| analogues.get(hypothesis.as_str()).copied())
            .collect();
        if hits.is_empty() {
            continue;
        }
        matched.extend(hits);
        joined.declines += 1;
        joined.regretted += usize::from(score.regret);
        let gate = joined.by_gate.entry(score.gate.clone()).or_default();
        gate.declines += 1;
        gate.regretted += usize::from(score.regret);
    }
    joined.analogues_matched = matched.len();
    joined
}
#[cfg(test)]
mod tests {
    //! The join's own arithmetic. The wiring — that the REASON stage
    //! actually calls it and records what it returns — is proven in
    //! `qip-kernel/tests/episodic.rs`, because a test on this side passes
    //! happily while the stage that should call it does not, which this
    //! repository has already paid for once in the episode index.

    use super::*;
    use qip_ai::memory::{
        ClaimRecord, DecisionTaken, Episode, FindingsSummary, Recalled, RegimeLabel,
    };
    use qip_core::ids::{ObjectId, OrderId};
    use qip_core::time::{Duration, Timestamp};
    use qip_twin::value::Simulated;

    fn at() -> Timestamp {
        Timestamp::from_secs(1_760_000_000)
    }

    /// An episode as the cycle writes one, under the id its hypothesis gives
    /// it. Everything but the id is a well-typed placeholder: the join reads
    /// the id and nothing else.
    fn episode(episode_id: &str) -> Recalled {
        Recalled {
            episode: Episode {
                episode_id: episode_id.to_string(),
                instrument: "obj-AAA".to_string(),
                regime: RegimeLabel {
                    market: "trending".to_string(),
                    volatility: "normal".to_string(),
                },
                state: None,
                causal_context: Vec::new(),
                findings: FindingsSummary {
                    runs: 1,
                    findings: 1,
                    coverage: 1.0,
                    contested: false,
                },
                stances: Vec::new(),
                claim: ClaimRecord {
                    class: "price_dislocation".to_string(),
                    claim: "overvalued".to_string(),
                    direction: -1.0,
                    confidence: 0.6,
                },
                horizon: Duration::from_days(1),
                decision: DecisionTaken::Approved,
                outcome: None,
                at: at(),
                known_at: at(),
            },
            similarity: 0.9,
        }
    }

    fn score(order: &str, gate: &str, regret: bool) -> DeclinedScore {
        DeclinedScore {
            order_id: OrderId::from_string(order),
            object_id: ObjectId::from_string("obj-AAA"),
            gate: gate.to_string(),
            declined_at: at(),
            scored_at: at(),
            would_have_earned: Simulated::ZERO,
            regret,
            alternatives: 4,
            rules: Vec::new(),
            readings: Vec::new(),
            venue: None,
        }
    }

    #[test]
    fn a_refusal_scored_on_a_recalled_episodes_hypothesis_is_joined_to_it_and_charged_to_its_gate()
    {
        let recalled = [episode("ep-HYP-1")];
        let scores = [score("ord-1", "pre-trade-risk", true)];
        // Premise: there is an analogue to join to and a score to join, so
        // an empty answer below is the join failing rather than the fixture
        // being empty.
        assert_eq!(recalled.len(), 1);
        assert_eq!(scores.len(), 1);

        let joined = join(&recalled, &scores, |_| vec!["HYP-1".to_string()]);
        assert!(joined.was_answerable(), "{joined:?}");
        assert_eq!(joined.analogues, 1);
        assert_eq!(joined.analogues_matched, 1);
        assert_eq!(joined.declines, 1);
        assert_eq!(joined.regretted, 1, "the twin's regret bit was dropped");
        assert_eq!(
            joined.by_gate.get("pre-trade-risk"),
            Some(&GateDeclines {
                declines: 1,
                regretted: 1
            }),
            "the refusal is not charged to the control that made it: {:?}",
            joined.by_gate
        );
    }

    #[test]
    fn a_refusal_on_a_hypothesis_nobody_recalled_is_left_out_rather_than_attached_to_an_analogue() {
        // The failure this guards: a join loose enough to attach any scored
        // refusal on the same instrument to any recalled episode. Every
        // field but the hypothesis matches here — same instrument, same
        // gate, same instant — so a count above zero means the join matched
        // on something it must not.
        let recalled = [episode("ep-HYP-1")];
        let scores = [score("ord-1", "pre-trade-risk", true)];
        assert_eq!(scores.len(), 1, "premise: there is a score to be joined");

        let joined = join(&recalled, &scores, |_| vec!["HYP-OTHER".to_string()]);
        assert_eq!(joined.analogues, 1, "premise: an analogue was recalled");
        assert_eq!(joined.declines, 0);
        assert_eq!(joined.analogues_matched, 0);
        assert!(joined.by_gate.is_empty(), "{:?}", joined.by_gate);
        assert!(
            !joined.was_answerable(),
            "a join that found nothing must not read as a clean answer"
        );
    }

    #[test]
    fn an_episode_id_this_platform_did_not_write_joins_nothing_however_well_the_rest_matches() {
        // The failure this guards: stripping the prefix with something that
        // answers even where it is absent. The order below carries exactly
        // the episode's whole id, so a join built on `trim_start_matches` or
        // an unchecked slice matches it and books a refusal against an
        // episode no hypothesis of this platform wrote.
        let recalled = [episode("HYP-1")];
        let scores = [score("ord-1", "pre-trade-risk", false)];
        assert_eq!(scores.len(), 1, "premise: there is a score to be joined");

        let joined = join(&recalled, &scores, |_| vec!["HYP-1".to_string()]);
        assert_eq!(joined.analogues, 1, "premise: an analogue was recalled");
        assert_eq!(joined.declines, 0);
        assert_eq!(hypothesis_of("HYP-1"), None);
        assert_eq!(hypothesis_of("ep-HYP-1"), Some("HYP-1"));
    }

    #[test]
    fn one_refusal_naming_two_recalled_episodes_is_one_refusal_against_two_matched_analogues() {
        // The failure this guards: counting a multi-leg proposal's single
        // refusal once per leg, which would inflate the evidence a gate is
        // judged on by the arity of the proposal rather than by how often
        // the gate was wrong.
        let recalled = [episode("ep-HYP-1"), episode("ep-HYP-2")];
        let scores = [score("ord-1", "pre-trade-risk", true)];

        let joined = join(&recalled, &scores, |_| {
            vec!["HYP-1".to_string(), "HYP-2".to_string()]
        });
        assert_eq!(joined.declines, 1);
        assert_eq!(joined.regretted, 1);
        assert_eq!(joined.analogues_matched, 2);
        assert_eq!(
            joined
                .by_gate
                .get("pre-trade-risk")
                .map(|gate| gate.declines),
            Some(1)
        );
    }
}

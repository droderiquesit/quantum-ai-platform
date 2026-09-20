//! Blueprint §13.1's regime-experience and blind-spot rows, derived from the
//! episodic store rather than kept beside it.
//!
//! # Why a view and not a table
//!
//! "How many distinct regimes have I traded through, and how recently?" and
//! "Which parts of the state space have no episodes at all?" are both
//! questions about the episodes the platform holds. A counter incremented
//! beside the store would be a second record of that fact, and two records
//! of one fact disagree — the store is bounded and evicts, a counter does
//! not, and the day they differ is the day an operator is told the platform
//! has experience it has forgotten. So this module reads the store and holds
//! nothing.
//!
//! # What "traded through" means here, said precisely
//!
//! An episode is written for every hypothesis REASON forms, approved or not,
//! and entered into memory when its claim resolves. A regime the platform
//! *reasoned in* is not a regime it *traded in*: a claim vetoed on review
//! taught the platform something about that regime and nothing about its
//! own execution there. Both counts are carried, and the row's question is
//! answered by the second — [`RegimeExperience::traded`] counts episodes
//! whose decision was `Approved` and whose outcome is recorded, which is the
//! narrowest reading of "traded through" the record supports. A wider one
//! would let a regime the platform only ever declined to act in read as one
//! it has survived.
//!
//! # Bitemporality is the store's, not this module's
//!
//! [`EpisodicMemory::episodes`] yields only what is knowable strictly before
//! the instant asked about. This module takes that iterator and reaches no
//! further, so a replay asking what the platform had experienced on Monday
//! is not told about Tuesday's resolution.
//!
//! # The state space is the caller's
//!
//! This crate holds no regime enum — [`super::episode::RegimeLabel`] says
//! why: a library below the services may not depend on the crate that
//! defines the closed sets. A blind spot is a label in the *caller's* known
//! set with no episode, so the known set is an argument. A label observed in
//! memory that the caller does not know is not silently folded into the
//! count: it is reported under its own name, because a regime the store
//! holds and the enums do not is a drift between two records of one
//! vocabulary, and the caller has to be able to see it.

use std::collections::{BTreeMap, BTreeSet};

use qip_core::error::{Error, Result};
use qip_core::time::Timestamp;
use serde::{Deserialize, Serialize};

use super::episode::{DecisionTaken, Episode, RegimeLabel};
use super::store::EpisodicMemory;

/// What memory holds for one regime.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegimeExperience {
    /// Episodes reasoned in this regime, whatever their decision.
    pub episodes: usize,
    /// Episodes approved on review with a recorded outcome — the narrowest
    /// reading of "traded through" the record supports.
    pub traded: usize,
    /// The newest instant at which anything in this regime became knowable.
    pub last_seen: Timestamp,
}

/// §13.1's regime-experience and blind-spot rows at one instant.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExperienceReport {
    /// Every regime memory holds an episode for, keyed `market/volatility`,
    /// whether or not the caller's state space names it.
    pub regimes: BTreeMap<String, RegimeExperience>,
    /// Labels the caller's state space names that memory holds no episode
    /// for — §13.1's "which parts of the state space have no episodes at
    /// all?"
    pub blind_spots: BTreeSet<String>,
    /// Labels memory holds that the caller's state space does not name. A
    /// drift between the store's vocabulary and the enums', reported rather
    /// than folded in.
    pub unknown: BTreeSet<String>,
    /// How many episodes were knowable at the instant asked. The premise of
    /// every figure above: a report over an empty memory has a full set of
    /// blind spots and has established nothing.
    pub episodes_examined: usize,
}

impl ExperienceReport {
    /// How many distinct regimes the platform has traded through — the count
    /// §13.1 asks for, over regimes with at least one approved, resolved
    /// episode.
    pub fn regimes_traded(&self) -> usize {
        self.regimes
            .values()
            .filter(|experience| experience.traded > 0)
            .count()
    }

    /// The newest instant any traded regime was seen, or `None` where the
    /// platform has traded through none.
    pub fn most_recent_trade(&self) -> Option<Timestamp> {
        self.regimes
            .values()
            .filter(|experience| experience.traded > 0)
            .map(|experience| experience.last_seen)
            .max()
    }

    /// The line an operator reads.
    pub fn describe(&self) -> String {
        if self.episodes_examined == 0 {
            return format!(
                "regime experience: none, memory holds no knowable episode; {} of {} regime(s) \
                 are blind spots",
                self.blind_spots.len(),
                self.blind_spots.len() + self.regimes.len()
            );
        }
        let recent = self
            .most_recent_trade()
            .map_or_else(|| "never".to_string(), |at| format!("{at:?}"));
        let unknown = if self.unknown.is_empty() {
            String::new()
        } else {
            format!(
                "; {} regime label(s) in memory the state space does not name: {}",
                self.unknown.len(),
                self.unknown
                    .iter()
                    .map(String::as_str)
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        };
        format!(
            "regime experience: traded through {} of {} regime(s) (reasoned in {}), most recent \
             trade {recent}; {} blind spot(s){unknown}",
            self.regimes_traded(),
            self.known_regimes(),
            self.regimes.len(),
            self.blind_spots.len(),
        )
    }

    /// The size of the caller's state space, recovered from what was
    /// reported: every known label is either observed or a blind spot.
    fn known_regimes(&self) -> usize {
        self.blind_spots.len()
            + self
                .regimes
                .keys()
                .filter(|key| !self.unknown.contains(*key))
                .count()
    }
}

/// The key a regime is reported under.
pub fn regime_key(label: &RegimeLabel) -> String {
    format!("{}/{}", label.market, label.volatility)
}

impl EpisodicMemory {
    /// §13.1's regime-experience and blind-spot rows over everything
    /// knowable at `now`, against the caller's state space.
    ///
    /// Refuses an empty state space: a report with no known labels has no
    /// blind spots by construction, and would say the platform has covered
    /// everything on the strength of nobody having named anything. Refuses a
    /// known label that is blank on either axis for the same reason — it
    /// could never match an episode and would be a permanent blind spot
    /// nobody could close.
    pub fn experience<'a>(
        &self,
        known: impl IntoIterator<Item = &'a RegimeLabel>,
        now: Timestamp,
    ) -> Result<ExperienceReport> {
        let known: BTreeSet<String> = known
            .into_iter()
            .map(|label| {
                if label.market.trim().is_empty() || label.volatility.trim().is_empty() {
                    return Err(Error::invalid(format!(
                        "regime label {:?}/{:?} is blank on one axis; it could never match an \
                         episode and would be a blind spot nobody could close",
                        label.market, label.volatility
                    )));
                }
                Ok(regime_key(label))
            })
            .collect::<Result<_>>()?;
        if known.is_empty() {
            return Err(Error::invalid(
                "the regime state space is empty; an experience report over nothing would read \
                 as full coverage, so name the regimes before asking",
            ));
        }

        let mut regimes: BTreeMap<String, RegimeExperience> = BTreeMap::new();
        let mut examined = 0usize;
        for episode in self.episodes(now) {
            examined += 1;
            let key = regime_key(&episode.regime);
            let traded = usize::from(is_traded(episode));
            regimes
                .entry(key)
                .and_modify(|experience| {
                    experience.episodes += 1;
                    experience.traded += traded;
                    if episode.known_at > experience.last_seen {
                        experience.last_seen = episode.known_at;
                    }
                })
                .or_insert(RegimeExperience {
                    episodes: 1,
                    traded,
                    last_seen: episode.known_at,
                });
        }

        let observed: BTreeSet<&String> = regimes.keys().collect();
        let blind_spots = known
            .iter()
            .filter(|key| !observed.contains(key))
            .cloned()
            .collect();
        let unknown = regimes
            .keys()
            .filter(|key| !known.contains(*key))
            .cloned()
            .collect();
        Ok(ExperienceReport {
            regimes,
            blind_spots,
            unknown,
            episodes_examined: examined,
        })
    }
}

/// Whether an episode is one the platform traded through: approved on
/// review and resolved. A rejected claim is experience of reasoning, not of
/// trading, and an approved claim still open has not been through anything
/// yet.
fn is_traded(episode: &Episode) -> bool {
    episode.decision == DecisionTaken::Approved && episode.outcome.is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::{ClaimRecord, EpisodeOutcome, FindingsSummary};
    use qip_core::time::Duration;

    fn at(secs: i64) -> Timestamp {
        Timestamp::from_secs(secs)
    }

    fn label(market: &str, volatility: &str) -> RegimeLabel {
        RegimeLabel {
            market: market.to_string(),
            volatility: volatility.to_string(),
        }
    }

    fn episode(
        id: &str,
        regime: RegimeLabel,
        decision: DecisionTaken,
        resolved: bool,
        known_at: Timestamp,
    ) -> Episode {
        Episode {
            episode_id: id.to_string(),
            instrument: "AAA".to_string(),
            regime,
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
                class: "price_move".to_string(),
                claim: "undervalued".to_string(),
                direction: 1.0,
                confidence: 0.6,
            },
            horizon: Duration::from_hours(24),
            decision,
            outcome: resolved.then_some(EpisodeOutcome {
                resolved_at: known_at,
                expected_move_bps: Some(12.0),
                realised_move_bps: 10.0,
                realised_pnl: 1.0,
            }),
            at: known_at.saturating_sub(Duration::from_hours(24)),
            known_at,
        }
    }

    fn known() -> Vec<RegimeLabel> {
        vec![
            label("trending", "low"),
            label("trending", "high"),
            label("quiet", "low"),
        ]
    }

    #[test]
    fn an_empty_memory_has_every_regime_as_a_blind_spot_and_no_experience() {
        let memory = EpisodicMemory::default();
        let report = memory
            .experience(known().iter(), at(10_000))
            .expect("a state space of three");
        assert_eq!(report.episodes_examined, 0, "premise: nothing knowable");
        assert_eq!(report.blind_spots.len(), 3);
        assert_eq!(report.regimes_traded(), 0);
        assert_eq!(report.most_recent_trade(), None);
        assert!(
            report.describe().contains("regime experience: none"),
            "{}",
            report.describe()
        );
    }

    #[test]
    fn a_regime_only_reasoned_in_is_not_one_the_platform_has_traded_through() {
        // The distinction the module doc names: a vetoed claim teaches the
        // platform about a regime and nothing about its execution there. A
        // count that folded the two would let a regime the platform only
        // ever declined to act in read as one it has survived.
        let mut memory = EpisodicMemory::default();
        memory
            .remember(episode(
                "ep-vetoed",
                label("trending", "high"),
                DecisionTaken::RejectedOnReview,
                true,
                at(9_000),
            ))
            .expect("remembered");
        memory
            .remember(episode(
                "ep-open",
                label("quiet", "low"),
                DecisionTaken::Approved,
                false,
                at(9_100),
            ))
            .expect("remembered");
        memory
            .remember(episode(
                "ep-traded",
                label("trending", "low"),
                DecisionTaken::Approved,
                true,
                at(9_200),
            ))
            .expect("remembered");
        let report = memory
            .experience(known().iter(), at(10_000))
            .expect("three known regimes");
        assert_eq!(report.episodes_examined, 3, "premise: all three knowable");
        assert_eq!(report.regimes.len(), 3, "reasoned in all three");
        assert_eq!(
            report.regimes_traded(),
            1,
            "traded through one: {:?}",
            report.regimes
        );
        assert_eq!(report.regimes["trending/low"].traded, 1);
        assert_eq!(report.regimes["trending/high"].traded, 0);
        assert_eq!(report.regimes["quiet/low"].traded, 0);
        assert_eq!(report.most_recent_trade(), Some(at(9_200)));
        assert!(report.blind_spots.is_empty());
        assert!(
            report
                .describe()
                .contains("traded through 1 of 3 regime(s) (reasoned in 3)"),
            "{}",
            report.describe()
        );
    }

    #[test]
    fn a_blind_spot_is_a_known_regime_with_no_episode_and_an_unknown_label_is_reported_not_folded()
    {
        let mut memory = EpisodicMemory::default();
        memory
            .remember(episode(
                "ep-1",
                label("trending", "low"),
                DecisionTaken::Approved,
                true,
                at(9_000),
            ))
            .expect("remembered");
        // A label the enums do not know — the drift case.
        memory
            .remember(episode(
                "ep-2",
                label("sideways", "low"),
                DecisionTaken::Approved,
                true,
                at(9_100),
            ))
            .expect("remembered");
        let report = memory
            .experience(known().iter(), at(10_000))
            .expect("three known regimes");
        assert_eq!(report.episodes_examined, 2, "premise");
        assert_eq!(
            report.blind_spots,
            ["trending/high", "quiet/low"]
                .into_iter()
                .map(String::from)
                .collect::<BTreeSet<_>>()
        );
        assert_eq!(
            report.unknown,
            std::iter::once("sideways/low".to_string()).collect::<BTreeSet<_>>()
        );
        assert!(
            report.describe().contains("2 blind spot(s)")
                && report.describe().contains("does not name: sideways/low"),
            "{}",
            report.describe()
        );
    }

    #[test]
    fn an_episode_not_yet_knowable_is_not_experience() {
        // The store's bitemporal filter, proven at this read path rather
        // than assumed: a replay asking about Monday must not be told the
        // platform had traded a regime it first learned about on Tuesday.
        let mut memory = EpisodicMemory::default();
        memory
            .remember(episode(
                "ep-later",
                label("trending", "low"),
                DecisionTaken::Approved,
                true,
                at(10_000),
            ))
            .expect("remembered");
        let before = memory
            .experience(known().iter(), at(10_000))
            .expect("asked at the very instant");
        assert_eq!(before.episodes_examined, 0);
        assert_eq!(before.blind_spots.len(), 3);
        let after = memory
            .experience(known().iter(), at(10_001))
            .expect("asked a second later");
        assert_eq!(after.regimes_traded(), 1);
        assert_eq!(after.blind_spots.len(), 2);
    }

    #[test]
    fn an_empty_or_blank_state_space_is_refused_rather_than_read_as_full_coverage() {
        let memory = EpisodicMemory::default();
        let error = memory
            .experience(std::iter::empty(), at(10_000))
            .expect_err("no known regimes");
        assert!(
            error.to_string().contains("state space is empty"),
            "{error}"
        );
        let blank = [label("", "low")];
        let error = memory
            .experience(blank.iter(), at(10_000))
            .expect_err("a blank axis");
        assert!(error.to_string().contains("blank on one axis"), "{error}");
    }
}

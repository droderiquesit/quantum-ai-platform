//! Champion against challenger: the comparison the search never made.
//!
//! Until this module the evolution brain could search, score and register, and
//! it did all three without ever asking the only question that decides
//! anything: **is this better than what we already have?** Candidates entered
//! the factory on the bottom rung and stayed there. No strategy was ever named
//! the one that speaks for an instrument, so none could ever be replaced.
//!
//! The whole comparison apparatus was unreachable, and by construction rather
//! than by oversight. [`qip_evolution::mutate::Challenger`] has private fields
//! and no constructor: the only way to obtain one is
//! [`qip_evolution::mutate::Mutator::mutate`], which derives it from a champion
//! by a single edit. Nothing in a running process ever called that. So
//! [`ChallengerTest::evaluate`], [`ChampionBook::crown`] and
//! [`qip_evolution::promotion::advance_challenger`] could not be reached from
//! any deployed path — three controls that read, in the source, exactly like
//! controls that work.
//!
//! # Why a champion is mutated rather than out-generated
//!
//! The generative search writes strategies from the grammar with no reference
//! to what already works. That finds new shapes and is worth doing, but it is
//! not a comparison: the best of a fresh sample beats the incumbent about as
//! often as chance and the search size says it should, which is what the trial
//! ledger exists to deflate away.
//!
//! A challenger is the incumbent plus one edit. When it wins, the edit is what
//! won, and that is a statement a person can act on. When it loses, the
//! incumbent survives having been attacked at a specific point rather than
//! having merely not been displaced.
//!
//! # The two things this module refuses to do quietly
//!
//! **Every challenger minted counts as a trial.** The mutation run is folded
//! into the same [`qip_kernel::central::foundry::StrategyFoundry`] ledger the
//! holdout gate deflates by. Minting twenty challengers and reporting the best
//! one, deflated only by the generative search that produced their parent, is
//! the multiple-comparisons problem wearing a different hat.
//!
//! **Champion and challenger are scored on the same window, in the same
//! round.** The champion is re-run on the current bars rather than compared
//! against a Sharpe computed whenever it was crowned. A number from an older,
//! shorter, different window is not evidence about this one, and
//! [`ChallengerTest::evaluate`] refuses two series of different lengths for
//! exactly that reason — so a desk that stored the old number would be a desk
//! whose comparison could never run.
//!
//! # What it does not do
//!
//! It does not promote. `crown` records who speaks for an instrument; the
//! ladder is still the factory's and the gates' to climb, and
//! [`ChampionBook::crown`] refuses a challenger the lifecycle ledger cannot
//! show standing on every rung beneath it. A search that could crown *and*
//! promote would be a search that could grant itself capital.

use qip_contracts::gate::GateStage;
use qip_core::Timestamp;
use qip_core::error::{Error, Result};
use qip_core::ids::ObjectId;
use qip_evolution::challenger::{ChallengerEntry, ChallengerPolicy, ChallengerTest};
use qip_evolution::cost_model::NetReturns;
use qip_evolution::generate::Candidate;
use qip_evolution::grammar::Grammar;
use qip_evolution::mutate::{MutationRun, Mutator};
use qip_evolution::promotion::{ChampionBook, Succession};
use qip_kernel::central::foundry::StrategyFoundry;
use qip_lifecycle::LifecycleLedger;
use qip_strategy::compile::StrategyCompiler;
use std::collections::BTreeMap;

/// What one challenge round concluded, for the operator's cycle line.
#[derive(Clone, Debug, Default)]
pub struct ChallengeSummary {
    pub subject: String,
    /// Challengers minted from the champion. Every one is a trial.
    pub minted: usize,
    /// Challengers the compiler refused. Still trials.
    pub refused: usize,
    /// Challengers actually compared against the champion. Fewer than
    /// `minted` whenever a comparison could not be made honestly -- too short
    /// a window, a costless series -- and the gap between the two numbers is
    /// the thing worth noticing.
    pub compared: usize,
    /// Challengers whose verdict said they beat the champion.
    pub winners: usize,
    /// The succession, where one happened.
    pub crowned: Option<String>,
    /// Why nothing was crowned, where a comparison ran and none won.
    pub refusals: Vec<String>,
}

impl ChallengeSummary {
    pub fn describe(&self) -> String {
        match &self.crowned {
            Some(champion) => format!(
                "succession: {champion} takes {} after {} challenger(s) compared of {} minted",
                self.subject, self.compared, self.minted
            ),
            None => format!(
                "succession: {} holds its ground; {} challenger(s) compared of {} minted, {} \
                 refused",
                self.subject, self.compared, self.minted, self.refused
            ),
        }
    }
}

/// Running totals across the node's lifetime, for the shutdown report.
#[derive(Clone, Copy, Debug, Default)]
pub struct SuccessionStats {
    pub installations: u64,
    pub challenges: u64,
    pub comparisons: u64,
    pub successions: u64,
}

/// Holds the champion of each instrument and runs the challenge that can
/// replace it.
pub struct SuccessionDesk {
    book: ChampionBook,
    test: ChallengerTest,
    /// The reigning champion of each subject.
    reigning: BTreeMap<String, Reign>,
    mutators: BTreeMap<String, Mutator>,
    /// Challengers minted per round. Small for the same reason the generative
    /// search is: every one deflates every holdout.
    per_round: usize,
    stats: SuccessionStats,
}

impl std::fmt::Debug for SuccessionDesk {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SuccessionDesk")
            .field("champions", &self.book.len())
            .field("per_round", &self.per_round)
            .field("stats", &self.stats)
            .finish_non_exhaustive()
    }
}

impl SuccessionDesk {
    /// Build a desk.
    ///
    /// Refuses a round of zero challengers rather than treating it as "off".
    /// A desk that mints nothing still holds champions and still reports
    /// rounds, which reads as a comparison happening and finding nothing --
    /// the most expensive kind of silence. Disabling the evolution loop is
    /// what turns this off, and it does so visibly.
    pub fn new(per_round: usize, policy: ChallengerPolicy) -> Result<Self> {
        if per_round == 0 {
            return Err(Error::invalid(
                "a challenge round of no challengers is not a smaller round; it would report a \
                 comparison that never happened. Disable the evolution loop instead",
            ));
        }
        Ok(Self {
            book: ChampionBook::new(),
            test: ChallengerTest::new(policy),
            reigning: BTreeMap::new(),
            mutators: BTreeMap::new(),
            per_round,
            stats: SuccessionStats::default(),
        })
    }

    pub const fn stats(&self) -> SuccessionStats {
        self.stats
    }

    pub fn champions(&self) -> usize {
        self.book.len()
    }

    /// The candidate currently speaking for `subject`, where there is one.
    pub fn champion(&self, subject: &str) -> Option<&Candidate> {
        self.reigning.get(subject).map(|reign| &reign.candidate)
    }

    /// Record the first champion of an instrument.
    ///
    /// The bootstrap: the first strategy on an instrument has nothing to beat.
    /// [`ChampionBook::install_first`] still refuses one the lifecycle ledger
    /// cannot show standing on every rung beneath its stage, and still refuses
    /// to displace an existing champion -- a book that already has one is not
    /// bootstrapping.
    pub fn install_first(
        &mut self,
        ledger: &LifecycleLedger,
        subject: &ObjectId,
        candidate: &Candidate,
        now: Timestamp,
    ) -> Result<Succession> {
        let succession = self.book.install_first(ledger, subject, candidate, now)?;
        self.reigning.insert(
            subject.as_str().to_string(),
            Reign {
                candidate: candidate.clone(),
                crowned_at: succession.stage,
            },
        );
        self.stats.installations += 1;
        Ok(succession)
    }

    /// Mint a round of challengers from the reigning champion.
    ///
    /// The run is folded into the foundry's trial ledger before it is returned,
    /// so a caller cannot look at the challengers and then decide how many to
    /// declare. `None` means there is no champion to mutate, which is the
    /// normal state of a subject nothing has been crowned on yet.
    pub fn mint(
        &mut self,
        subject: &str,
        foundry: &mut StrategyFoundry,
        compiler: &mut StrategyCompiler,
        grammar: Grammar,
        seed: u64,
    ) -> Option<MutationRun> {
        let champion = &self.reigning.get(subject)?.candidate;
        let mutator = self
            .mutators
            .entry(subject.to_string())
            .or_insert_with(|| Mutator::new(grammar, seed));
        let run = mutator.mutate(champion, self.per_round, compiler);
        foundry.record_mutation(&run);
        self.stats.challenges += 1;
        Some(run)
    }

    /// Compare one challenger against the champion and crown it if it wins.
    ///
    /// Both return series must come from the same window -- the caller scores
    /// them in the same round on the same bars -- and the trial count comes
    /// from the foundry's ledger rather than from the caller, so the deflation
    /// is by the search that actually ran.
    ///
    /// A refusal from [`ChallengerTest::evaluate`] is returned as an error
    /// rather than swallowed as a loss. "The comparison could not be made"
    /// and "the challenger lost" are different facts about the platform, and a
    /// desk that reported the first as the second would look like a desk whose
    /// champions keep surviving.
    pub fn judge(
        &mut self,
        ledger: &LifecycleLedger,
        subject: &ObjectId,
        challenger: &qip_evolution::mutate::Challenger,
        champion_returns: NetReturns,
        challenger_returns: NetReturns,
        periods_per_year: f64,
        foundry: &StrategyFoundry,
        now: Timestamp,
    ) -> Result<Option<Succession>> {
        let key = subject.as_str().to_string();
        let reigning = &self
            .reigning
            .get(&key)
            .ok_or_else(|| {
                Error::not_found(format!(
                    "{} has no champion, so there is nothing for {} to challenge",
                    subject.as_str(),
                    challenger.id()
                ))
            })?
            .candidate;
        if challenger.champion() != reigning.id() {
            return Err(Error::invalid(format!(
                "{} was derived from {} but {} is held by {}; a challenger that mutated a \
                 different parent is not evidence about this champion",
                challenger.id(),
                challenger.champion(),
                subject.as_str(),
                reigning.id()
            )));
        }

        let entry = ChallengerEntry::from_ledger(
            reigning.id().clone(),
            challenger.candidate(),
            champion_returns,
            challenger_returns,
            periods_per_year,
            foundry.ledger(),
        );
        let verdict = self.test.evaluate(&entry)?;
        self.stats.comparisons += 1;
        if !verdict.challenger_wins() {
            return Ok(None);
        }

        let succession = self
            .book
            .crown(ledger, subject, challenger, &verdict, now)?;
        self.reigning.insert(
            key,
            Reign {
                candidate: challenger.candidate().clone(),
                crowned_at: succession.stage,
            },
        );
        self.stats.successions += 1;
        Ok(Some(succession))
    }

    /// Drop champions the ledger has withdrawn or pushed back down.
    ///
    /// A demoted champion must stop being the parent every challenger is
    /// mutated from. A search descending from a strategy the platform has
    /// already stopped trusting is the quiet way an automated loop keeps
    /// investing in a dead line.
    ///
    /// The criterion is deliberately *not* [`ChampionBook::stale`], which
    /// returns champions whose stage no longer holds capital. That is the right
    /// question for a book whose champions are live; it is the wrong one here,
    /// because this book crowns at the bottom rung, where nothing holds capital
    /// yet. Applied to a candidate-stage champion it dethrones the incumbent
    /// every single round -- which it did, in the first version of this module:
    /// three rounds produced three installations, zero challenges, and a
    /// contest that could never run because no champion survived long enough to
    /// be challenged.
    ///
    /// So a champion is withdrawn when the ledger retired it, or when it stands
    /// *below* the rung it was crowned on. Both are the ledger saying something
    /// changed about that strategy. Standing where it was crowned is not.
    pub fn retire_demoted(&mut self, ledger: &LifecycleLedger) -> Vec<String> {
        let withdrawn: Vec<(String, ObjectId, GateStage)> = self
            .reigning
            .iter()
            .filter_map(|(subject, reign)| {
                let stage = ledger.stage_of(reign.candidate.id());
                (stage == GateStage::Retired || stage < reign.crowned_at)
                    .then(|| (subject.clone(), ObjectId::from_string(subject), stage))
            })
            .collect();
        let mut retired = Vec::with_capacity(withdrawn.len());
        for (subject, object, stage) in withdrawn {
            if let Some(previous) = self.book.dethrone(&object) {
                self.reigning.remove(&subject);
                self.mutators.remove(&subject);
                retired.push(format!(
                    "{previous} no longer holds {subject}: the ledger has it at {}",
                    stage.as_str()
                ));
            }
        }
        retired
    }
}

/// A champion and the rung it was crowned on.
///
/// The stage is kept because "has this champion been demoted" cannot be
/// answered from the current stage alone -- a strategy at the bottom rung is
/// either newly crowned or pushed back down, and those need opposite responses.
struct Reign {
    candidate: Candidate,
    crowned_at: GateStage,
}

#[cfg(test)]
#[allow(clippy::panic_in_result_fn)]
mod tests {
    use super::*;

    #[test]
    fn a_challenge_round_of_no_challengers_is_refused() {
        // A desk that mints nothing still holds champions and still reports
        // rounds, which reads as a comparison that happened and found nothing.
        let error = SuccessionDesk::new(0, ChallengerPolicy::default())
            .expect_err("a zero-challenger desk was built");
        assert!(
            error.message().contains("comparison that never happened"),
            "the refusal does not say why zero is not a smaller round: {}",
            error.message()
        );
    }

    #[test]
    fn a_desk_with_no_champion_has_nothing_to_mutate() {
        // The normal state of a subject nothing has been crowned on. It must be
        // distinguishable from "mutated and produced nothing", which is a
        // failing grammar.
        let desk = SuccessionDesk::new(4, ChallengerPolicy::default()).expect("a desk");
        assert_eq!(desk.champions(), 0);
        assert!(desk.champion("obj-ACME").is_none());
        assert_eq!(desk.stats().challenges, 0);
    }

    #[test]
    fn a_crowning_names_the_new_champion_and_the_round_it_won() {
        let summary = ChallengeSummary {
            subject: "OBJ-AAPL".to_string(),
            minted: 4,
            refused: 1,
            compared: 3,
            winners: 1,
            crowned: Some("challenger-7".to_string()),
            refusals: Vec::new(),
        };
        let text = summary.describe();
        assert!(
            text.contains("challenger-7 takes OBJ-AAPL"),
            "the crowning does not name who won and what they now hold: {text}"
        );
        assert!(
            text.contains("3 challenger(s) compared of 4 minted"),
            "the crowning drops the count a comparison was made against: {text}"
        );
    }

    #[test]
    fn a_round_with_no_crowning_does_not_report_the_instrument_as_holding_itself() {
        // Regression: the uncrowned branch used to read
        // "succession: OBJ-AAPL holds OBJ-AAPL", which names no champion at
        // all -- an instrument does not hold itself. The only identity this
        // branch has to report is the subject, so the fix is in the wording,
        // not in a field this summary does not carry.
        let summary = ChallengeSummary {
            subject: "OBJ-AAPL".to_string(),
            minted: 4,
            refused: 1,
            compared: 3,
            winners: 0,
            crowned: None,
            refusals: Vec::new(),
        };
        let text = summary.describe();
        assert!(
            !text.contains("OBJ-AAPL holds OBJ-AAPL"),
            "the summary still claims the instrument holds itself: {text}"
        );
        assert!(
            text.contains("3 challenger(s) compared of 4 minted, 1 refused"),
            "the uncrowned branch dropped the round's own counts: {text}"
        );
    }
}

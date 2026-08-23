//! How a challenger becomes the champion.
//!
//! There is one path to capital in this platform and it is not in this crate.
//! [`qip_lifecycle`] already owns it: [`attempt_promotion`] computes the target
//! rung from [`qip_contracts::gate::GateStage::next`] so a rung cannot be
//! skipped, [`qip_lifecycle::AuthorisedPromotion`] has no representation for a
//! capital-holding rung without a dual [`Approval`], and the gates recompute
//! the evidence rather than trusting it. Every function here calls that path
//! and adds exactly two things a generated strategy needs on top of it:
//!
//! * the challenger must have **won its comparison** before a promotion is
//!   even attempted, so a search result cannot walk the ladder on the strength
//!   of having been generated; and
//! * the [`ChampionBook`] refuses to crown a challenger that has not itself
//!   **stood on every rung** the ledger records, which is a property of the
//!   ledger and therefore checkable after the fact.
//!
//! Nothing here records a promotion, issues an approval, or decides a stage.
//! A second path to capital is exactly the thing this design exists to
//! prevent, and the way not to build one is not to write the code.

use crate::challenger::ChallengerVerdict;
use crate::generate::Candidate;
use crate::mutate::Challenger;
use qip_contracts::gate::{GateStage, Promotion};
use qip_contracts::governance::Approval;
use qip_contracts::signal::StrategyId;
use qip_core::error::{Error, Result};
use qip_core::{ObjectId, Timestamp};
use qip_lifecycle::evidence::StrategyEvidence;
use qip_lifecycle::ledger::{LifecycleLedger, attempt_promotion};
use std::collections::BTreeMap;

/// Advance a compiled candidate one rung, through the existing gates.
///
/// A thin call. It exists so that this crate has a single named entry point to
/// the ladder — one place to read, and one place a reviewer can confirm does
/// nothing but delegate.
pub fn advance(
    ledger: &mut LifecycleLedger,
    candidate: &Candidate,
    evidence: &StrategyEvidence,
    approval: Option<Approval>,
    rationale: impl Into<String>,
    now: Timestamp,
) -> Result<Promotion> {
    attempt_promotion(ledger, candidate.id(), evidence, approval, rationale, now)
}

/// Advance a challenger one rung, but only if it beat the champion first.
///
/// The verdict must belong to this challenger. A verdict about some other
/// strategy is not evidence about this one, and accepting one would make the
/// whole comparison decorative.
pub fn advance_challenger(
    ledger: &mut LifecycleLedger,
    challenger: &Challenger,
    verdict: &ChallengerVerdict,
    evidence: &StrategyEvidence,
    approval: Option<Approval>,
    rationale: impl Into<String>,
    now: Timestamp,
) -> Result<Promotion> {
    if verdict.challenger != *challenger.id() {
        return Err(Error::invalid(format!(
            "the verdict is about {} but the challenger is {}",
            verdict.challenger,
            challenger.id()
        )));
    }
    if !verdict.challenger_wins() {
        let failures: Vec<String> = verdict
            .findings
            .iter()
            .filter(|(_, passed, _)| !passed)
            .map(|(name, _, detail)| format!("{name}: {detail}"))
            .collect();
        return Err(Error::guard(format!(
            "{} did not beat {}: {}",
            challenger.id(),
            verdict.champion,
            failures.join("; ")
        )));
    }
    advance(
        ledger,
        challenger.candidate(),
        evidence,
        approval,
        rationale,
        now,
    )
}

/// The moment one strategy replaces another.
#[derive(Clone, Debug, PartialEq)]
pub struct Succession {
    pub subject: ObjectId,
    /// The strategy that held the place, where there was one.
    pub deposed: Option<StrategyId>,
    pub champion: StrategyId,
    /// The rung the new champion stands on, read from the ledger.
    pub stage: GateStage,
    pub at: Timestamp,
}

impl Succession {
    pub fn describe(&self) -> String {
        match &self.deposed {
            Some(previous) => format!(
                "{} replaces {} on {} at {}",
                self.champion,
                previous,
                self.subject.as_str(),
                self.stage.as_str()
            ),
            None => format!(
                "{} becomes the first champion on {} at {}",
                self.champion,
                self.subject.as_str(),
                self.stage.as_str()
            ),
        }
    }
}

/// Which strategy currently speaks for each instrument.
///
/// A register, not an authority. It records who won; the ledger decides what
/// the winner is allowed to do, and the book refuses to record a champion the
/// ledger does not support.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ChampionBook {
    champions: BTreeMap<ObjectId, StrategyId>,
}

impl ChampionBook {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn champion(&self, subject: &ObjectId) -> Option<&StrategyId> {
        self.champions.get(subject)
    }

    pub fn subjects(&self) -> impl Iterator<Item = &ObjectId> {
        self.champions.keys()
    }

    pub fn len(&self) -> usize {
        self.champions.len()
    }

    pub fn is_empty(&self) -> bool {
        self.champions.is_empty()
    }

    /// Install a challenger as champion.
    ///
    /// Four refusals, each about something the caller cannot assert for
    /// itself:
    ///
    /// * the verdict must say the challenger won, and be about this
    ///   challenger;
    /// * the challenger must be about the instrument it is being crowned on,
    ///   because a strategy that emits signals for one subject cannot speak
    ///   for another however well it scored;
    /// * the ledger must show the challenger standing on every rung between
    ///   candidate and where it now is, so a strategy that arrived at pilot
    ///   without a shadow run cannot be crowned even if some other path
    ///   somehow put it there; and
    /// * the challenger must stand at least as high as the champion it
    ///   replaces, because replacing a scaled strategy with one that has only
    ///   reached paper is a demotion of the book wearing a promotion's name.
    pub fn crown(
        &mut self,
        ledger: &LifecycleLedger,
        subject: &ObjectId,
        challenger: &Challenger,
        verdict: &ChallengerVerdict,
        at: Timestamp,
    ) -> Result<Succession> {
        if verdict.challenger != *challenger.id() {
            return Err(Error::invalid(format!(
                "the verdict is about {} but the challenger is {}",
                verdict.challenger,
                challenger.id()
            )));
        }
        if !verdict.challenger_wins() {
            return Err(Error::guard(format!(
                "{} has not beaten {}",
                challenger.id(),
                verdict.champion
            )));
        }
        if challenger.candidate().spec().subject != *subject {
            return Err(Error::invalid(format!(
                "{} is about {} and cannot be champion of {}",
                challenger.id(),
                challenger.candidate().spec().subject.as_str(),
                subject.as_str()
            )));
        }

        let stage = ledger.stage_of(challenger.id());
        walked_every_rung(ledger, challenger.id(), stage)?;

        let deposed = self.champions.get(subject).cloned();
        if let Some(previous) = deposed.as_ref() {
            let held = ledger.stage_of(previous);
            if stage < held {
                return Err(Error::denied(format!(
                    "{} stands at {} and cannot replace {} at {}",
                    challenger.id(),
                    stage.as_str(),
                    previous,
                    held.as_str()
                )));
            }
        }

        self.champions
            .insert(subject.clone(), challenger.id().clone());
        Ok(Succession {
            subject: subject.clone(),
            deposed,
            champion: challenger.id().clone(),
            stage,
            at,
        })
    }

    /// Record a first champion that was not the product of a comparison.
    ///
    /// The bootstrap case: the first strategy on an instrument has nothing to
    /// beat. It still has to have walked the ladder, and it still cannot
    /// displace an existing champion — a book that already has one is not
    /// bootstrapping.
    pub fn install_first(
        &mut self,
        ledger: &LifecycleLedger,
        subject: &ObjectId,
        candidate: &Candidate,
        at: Timestamp,
    ) -> Result<Succession> {
        if let Some(existing) = self.champions.get(subject) {
            return Err(Error::denied(format!(
                "{} already holds {}; a replacement is a succession, not an installation",
                existing,
                subject.as_str()
            )));
        }
        if candidate.spec().subject != *subject {
            return Err(Error::invalid(format!(
                "{} is about {} and cannot be champion of {}",
                candidate.id(),
                candidate.spec().subject.as_str(),
                subject.as_str()
            )));
        }
        let stage = ledger.stage_of(candidate.id());
        walked_every_rung(ledger, candidate.id(), stage)?;
        self.champions
            .insert(subject.clone(), candidate.id().clone());
        Ok(Succession {
            subject: subject.clone(),
            deposed: None,
            champion: candidate.id().clone(),
            stage,
            at,
        })
    }

    /// Remove whoever holds `subject`, returning them.
    ///
    /// Takes no authority and no reason, matching
    /// [`qip_lifecycle::LifecycleLedger::demote`]: anything that notices a
    /// champion has stopped working may stop it being the champion.
    pub fn dethrone(&mut self, subject: &ObjectId) -> Option<StrategyId> {
        self.champions.remove(subject)
    }

    /// Champions whose ledger stage no longer holds capital.
    ///
    /// The book does not update itself when a demotion trigger fires; this is
    /// how a caller finds the ones to remove.
    pub fn stale(&self, ledger: &LifecycleLedger) -> Vec<(&ObjectId, &StrategyId)> {
        self.champions
            .iter()
            .filter(|(_, strategy)| !ledger.stage_of(strategy).holds_capital())
            .collect()
    }
}

/// Refuse a strategy that has not stood on every rung up to `stage`.
///
/// Checked against the ledger's own history rather than recomputed, so it is a
/// statement about what happened and not about what this crate intended.
fn walked_every_rung(
    ledger: &LifecycleLedger,
    strategy: &StrategyId,
    stage: GateStage,
) -> Result<()> {
    if stage == GateStage::Retired {
        return Err(Error::denied(format!(
            "{strategy} is retired and cannot be a champion"
        )));
    }
    let missed: Vec<&'static str> = GateStage::all()
        .into_iter()
        .filter(|rung| *rung > GateStage::Candidate && *rung <= stage)
        .filter(|rung| !ledger.reached(strategy, *rung))
        .map(|rung| rung.as_str())
        .collect();
    if missed.is_empty() {
        return Ok(());
    }
    Err(Error::denied(format!(
        "{strategy} stands at {} without ever having stood at {}",
        stage.as_str(),
        missed.join(", ")
    )))
}

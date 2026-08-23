//! Where generated strategies become registered candidates.
//!
//! `qip-evolution` writes strategies and `central::factory` walks them up the
//! promotion ladder, and until this module existed nothing joined the two:
//! both crates were dependencies of this one, neither was named by a line of
//! its code, and a search could run for a thousand candidates without any of
//! them ever reaching a gate.
//!
//! The join is four lines of plumbing and one invariant that is the entire
//! reason this is a type rather than a function.
//!
//! # The trial count must not be lost in the handoff
//!
//! `qip-evolution` is built around one number. Generate ten thousand
//! strategies, keep the best backtest, and you have promoted noise — the best
//! of ten thousand coin-flippers shows an annualised Sharpe above three. What
//! separates a discovery from that arithmetic is how many were tried, and
//! [`qip_evolution::challenger::TrialLedger`] counts it.
//!
//! [`qip_lifecycle::evidence::HoldoutEvidence::trials`] is where the gate
//! reads that number, and its own documentation says understating it is the
//! easiest way to make a search result look like a discovery. So the two ends
//! agree, and the danger is entirely in the middle: a handoff that passed the
//! *round's* count instead of the *search's* would understate trials by
//! however many rounds have run, and would do it silently, because a candidate
//! carrying `trials: 40` is indistinguishable from an honest one except in
//! being wrong.
//!
//! This module therefore never takes a trial count as an argument. It holds
//! the ledger, folds every round into it, and reads it when a candidate is
//! promoted to the factory. A caller cannot supply the number, so a caller
//! cannot supply the wrong one.
//!
//! # What it deliberately does not do
//!
//! * **It does not evaluate.** Producing the holdout returns is a backtest,
//!   and this module takes them as evidence rather than generating them. A
//!   foundry that scored its own candidates would be marking its own homework.
//! * **It does not promote.** Registration puts a candidate on the bottom rung
//!   with its evidence attached; every move up is the factory's and the gates'
//!   decision, unchanged by anything here.
//! * **It does not deduplicate across rounds.** Two rounds may generate the
//!   same strategy, and both count as trials, because both were scored and
//!   both contributed a draw to the maximum that was picked from.
//!
//! # Why the seed is part of the identity prefix
//!
//! [`StrategyGenerator`] names candidates `{lineage}-g{counter}`, which depends
//! on neither the seed nor what the strategy actually does. Two searches
//! sharing a lineage therefore mint *identical ids for different strategies* —
//! and [`StrategyFactory`] keys candidates by [`StrategyId`], so the second
//! search's work would collide with the first's on the ladder. Worse, a lookup
//! by id would silently find the wrong candidate, which is how this was found:
//! a test registered one foundry's strategy through another and it succeeded.
//!
//! So the prefix handed to the generator is `{lineage}@{seed}`. Same lineage
//! and same seed is the same search by definition and keeps the same ids,
//! which is what makes a run reproducible; a different seed is a different
//! search and cannot collide with it. This does not fix the underlying
//! namespacing in `qip-evolution` — a caller using that crate directly can
//! still collide two searches — it makes it impossible for anything registered
//! through this seam, which is the path that reaches capital.

use crate::central::factory::{StrategyCandidate, StrategyFactory};
use qip_contracts::signal::StrategyId;
use qip_contracts::venue::VenueId;
use qip_core::Timestamp;
use qip_core::error::{Error, Result};
use qip_evolution::challenger::TrialLedger;
use qip_evolution::generate::{Candidate, GenerationRun, StrategyGenerator};
use qip_evolution::grammar::Grammar;
use qip_lifecycle::evidence::{
    CrossValidationRun, HoldoutEvidence, LeakageAudit, StrategyEvidence,
};
use qip_strategy::catalogue::FeatureCatalogue;
use qip_strategy::compile::StrategyCompiler;

/// What one round of the search produced, for an operator and for a test.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FoundryRound {
    /// Candidates asked for.
    pub requested: usize,
    /// Candidates that compiled and are therefore evaluable.
    pub accepted: usize,
    /// Candidates the compiler refused. Counted, and never folded into the
    /// trial count: a candidate that was never scored did not contribute a
    /// draw to the maximum the best was picked from.
    pub refused: usize,
    /// The search's cumulative trial count *after* this round.
    pub trials: usize,
}

/// Generates strategies and hands the survivors to the factory with the
/// search's own trial count attached.
#[derive(Debug)]
pub struct StrategyFoundry {
    generator: StrategyGenerator,
    compiler: StrategyCompiler,
    ledger: TrialLedger,
    /// The cell that would run whatever this foundry produces.
    cell: String,
    venue: VenueId,
    /// Candidates generated and not yet registered, oldest first.
    pending: Vec<Candidate>,
    rounds: u32,
}

impl StrategyFoundry {
    /// Build a foundry over a feature vocabulary.
    ///
    /// The catalogue is given twice by construction — once to the grammar that
    /// writes strategies over it and once to the compiler that type-checks
    /// them against it — and they must be the same vocabulary or the generator
    /// writes strategies the compiler will refuse for reasons that look like
    /// bugs. Taking it once here is what makes that impossible.
    pub fn new(
        catalogue: FeatureCatalogue,
        grammar: Grammar,
        cell: impl Into<String>,
        venue: VenueId,
        lineage: impl Into<String>,
        seed: u64,
    ) -> Result<Self> {
        let cell = cell.into();
        if cell.trim().is_empty() {
            return Err(Error::invalid(
                "a foundry must name the cell its candidates would run in; capital is granted \
                 per cell and an unnamed cell cannot be recalled",
            ));
        }
        Ok(Self {
            generator: StrategyGenerator::new(grammar, format!("{}@{seed}", lineage.into()), seed),
            compiler: StrategyCompiler::new(catalogue),
            ledger: TrialLedger::new(),
            cell,
            venue,
            pending: Vec::new(),
            rounds: 0,
        })
    }

    /// The search's cumulative trial count.
    ///
    /// Monotonic, because [`TrialLedger`] is: there is no way to lower it, and
    /// every reason someone would want to is a reason to distrust the answer.
    pub const fn trials(&self) -> usize {
        self.ledger.trials()
    }

    /// Candidates the compiler refused across the whole search.
    pub const fn refused(&self) -> usize {
        self.ledger.refused()
    }

    pub const fn rounds(&self) -> u32 {
        self.rounds
    }

    /// Candidates generated and not yet registered.
    pub fn pending(&self) -> &[Candidate] {
        &self.pending
    }

    /// Run one round: propose `count` strategies, compile them, count them.
    ///
    /// The count folds into the ledger here, before any of the results are
    /// looked at. Counting after a selection would let the selection decide
    /// what to count, which is the failure the ledger exists to prevent.
    pub fn search(&mut self, count: usize) -> Result<FoundryRound> {
        if count == 0 {
            return Err(Error::invalid(
                "a search of no candidates is not a smaller search; it produces nothing and \
                 would still be recorded as a round",
            ));
        }
        let run: GenerationRun = self.generator.generate(count, &mut self.compiler);
        self.ledger.record_generation(&run);
        self.rounds += 1;

        let round = FoundryRound {
            requested: run.requested(),
            accepted: run.evaluable(),
            refused: run.discarded().len(),
            trials: self.ledger.trials(),
        };
        self.pending.extend(run.accepted().iter().cloned());
        Ok(round)
    }

    /// Register one pending candidate with the factory, carrying the search's
    /// trial count into the evidence the gate will read.
    ///
    /// `holdout_returns` are the candidate's out-of-sample returns, produced
    /// by a backtest this module does not run — see the module documentation
    /// on why it does not score its own work.
    ///
    /// There is deliberately no parameter for the trial count. It comes from
    /// the ledger, so the number the gate deflates by is the number the search
    /// actually ran and not one a caller computed.
    pub fn register(
        &mut self,
        factory: &mut StrategyFactory,
        strategy: &StrategyId,
        holdout: HoldoutInputs,
        now: Timestamp,
    ) -> Result<()> {
        let position = self
            .pending
            .iter()
            .position(|candidate| candidate.id() == strategy)
            .ok_or_else(|| {
                Error::not_found(format!(
                    "{strategy} is not pending in this foundry; a candidate must come from the \
                     search whose trial count will be attached to it, or the count describes a \
                     different search"
                ))
            })?;
        let candidate = self.pending.remove(position);

        let evidence = StrategyEvidence::new().with_holdout(HoldoutEvidence {
            holdout_returns: holdout.returns,
            in_sample_folds: holdout.in_sample_folds,
            out_of_sample_folds: holdout.out_of_sample_folds,
            // The invariant this whole module exists for.
            trials: self.ledger.trials(),
            periods_per_year: holdout.periods_per_year,
            cross_validation: holdout.cross_validation,
            leakage: holdout.leakage,
        });

        let registered = StrategyCandidate::new(
            candidate.compiled().clone(),
            self.compiler.program().clone(),
            self.cell.clone(),
            self.venue.clone(),
            now,
        )?
        .with_evidence(evidence);

        factory.register(registered)
    }

    /// Discard a pending candidate without registering it.
    ///
    /// The trial count does not go down: it was generated, it was looked at,
    /// and it contributed a draw to the distribution the survivors were picked
    /// from. That is the whole point of counting.
    pub fn discard(&mut self, strategy: &StrategyId) -> bool {
        let before = self.pending.len();
        self.pending.retain(|candidate| candidate.id() != strategy);
        self.pending.len() < before
    }
}

/// The out-of-sample evidence for one candidate, produced elsewhere.
///
/// A struct rather than six positional arguments, because five of them are
/// sequences of floats and transposing two would be a silent change of meaning
/// rather than a compile error.
#[derive(Clone, Debug)]
pub struct HoldoutInputs {
    pub returns: Vec<f64>,
    pub in_sample_folds: Vec<Vec<f64>>,
    pub out_of_sample_folds: Vec<Vec<f64>>,
    pub periods_per_year: f64,
    pub cross_validation: CrossValidationRun,
    pub leakage: LeakageAudit,
}

//! Producing candidate strategies, and refusing to let an unchecked one
//! through.
//!
//! The safety story for a generated strategy is short: it is IR, and IR
//! becomes a program only by passing [`StrategyCompiler`]. Everything the
//! compiler enforces — types, declared inputs, a stated worst-case cost, and
//! the structural absence of any call out of the process — applies to a
//! machine-written strategy exactly as it applies to a hand-written one,
//! because it is the same compiler and there is no second path.
//!
//! That is made structural here rather than promised. [`Candidate`] has no
//! public constructor and holds a [`CompiledStrategy`], so a value of that
//! type is a proof that the compiler accepted the spec inside it. Everything
//! downstream in this crate — mutation, challenger testing, promotion — takes
//! a `Candidate` and not a [`StrategySpec`], so there is no signature through
//! which an uncompiled strategy could reach an evaluation.
//!
//! A candidate the compiler refuses becomes a [`Discarded`] carrying the
//! compiler's own [`Error`]. It is not dropped, not counted as "skipped", and
//! not summarised into a generic message: a search that quietly loses a
//! quarter of its budget to an unregistered feature looks exactly like a
//! search that is working, and the only difference is the reason nobody kept.
//!
//! Generation reads no clock and draws from a seeded [`Xoshiro256`]. Two runs
//! from the same seed over the same palette produce byte-identical specs.

use crate::grammar::Grammar;
use qip_contracts::StrategyId;
use qip_core::error::Error;
use qip_core::rng::Xoshiro256;
use qip_strategy::compile::{CompiledStrategy, StrategyCompiler};
use qip_strategy::ir::StrategySpec;
use serde::Serialize;

/// A strategy the compiler accepted, with the compiled form it produced.
///
/// The private fields and the absent constructor are the point: holding one of
/// these is the only evidence in this crate that a spec passed the type
/// checker and the cost bound, and it cannot be manufactured. It is
/// [`Serialize`] and deliberately not `Deserialize` — a candidate rebuilt from
/// JSON would be a proof of nothing.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Candidate {
    spec: StrategySpec,
    compiled: CompiledStrategy,
}

impl Candidate {
    pub const fn spec(&self) -> &StrategySpec {
        &self.spec
    }

    pub const fn compiled(&self) -> &CompiledStrategy {
        &self.compiled
    }

    pub const fn id(&self) -> &StrategyId {
        &self.spec.id
    }

    /// The worst-case node count the compiler proved for this candidate.
    pub const fn cost(&self) -> usize {
        self.compiled.cost()
    }
}

/// A candidate the compiler refused, and what it said.
///
/// The reason is the compiler's [`Error`] rather than a string this crate
/// invented, so a discard reads the same as a refusal of a hand-written
/// strategy and stays correct when the compiler's checks change.
#[derive(Clone, Debug, PartialEq)]
pub struct Discarded {
    spec: StrategySpec,
    reason: Error,
}

impl Discarded {
    pub const fn spec(&self) -> &StrategySpec {
        &self.spec
    }

    pub const fn reason(&self) -> &Error {
        &self.reason
    }

    /// The refusal, as a line an operator can read.
    pub fn explain(&self) -> String {
        format!(
            "{} discarded ({}): {}",
            self.spec.id,
            self.reason.code(),
            self.reason.message()
        )
    }
}

/// Offer a spec to the compiler, keeping whichever answer comes back.
///
/// The single place in this crate where a `StrategySpec` becomes a
/// `Candidate`, so there is one place to read to know that nothing skips the
/// compiler.
pub(crate) fn admit(
    spec: StrategySpec,
    compiler: &mut StrategyCompiler,
) -> std::result::Result<Candidate, Discarded> {
    match compiler.compile(&spec) {
        Ok(compiled) => Ok(Candidate { spec, compiled }),
        Err(reason) => Err(Discarded { spec, reason }),
    }
}

/// What one generation run produced.
///
/// Accepted and discarded together account for every candidate proposed. A
/// caller can assert that, and the assertion is what makes "silently skipped"
/// impossible to reach.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GenerationRun {
    accepted: Vec<Candidate>,
    discarded: Vec<Discarded>,
    requested: usize,
}

impl GenerationRun {
    pub fn accepted(&self) -> &[Candidate] {
        &self.accepted
    }

    pub fn discarded(&self) -> &[Discarded] {
        &self.discarded
    }

    /// How many candidates were asked for.
    pub const fn requested(&self) -> usize {
        self.requested
    }

    /// Whether every proposed candidate ended up in one list or the other.
    pub fn accounted_for(&self) -> bool {
        self.accepted.len() + self.discarded.len() == self.requested
    }

    /// The specs that compiled, for a byte-identity check across runs.
    pub fn accepted_specs(&self) -> Vec<&StrategySpec> {
        self.accepted.iter().map(Candidate::spec).collect()
    }

    /// Every refusal, in the order it happened.
    pub fn refusals(&self) -> Vec<String> {
        self.discarded.iter().map(Discarded::explain).collect()
    }

    /// Candidates that survived the compiler, and therefore the number of
    /// distinct strategies this run put in front of an evaluation.
    ///
    /// This, and not the number proposed, is what
    /// [`crate::challenger::TrialLedger`] counts: a candidate the compiler
    /// refused was never scored, so it cannot have contributed to the maximum
    /// of a distribution of scores.
    pub fn evaluable(&self) -> usize {
        self.accepted.len()
    }
}

/// Writes candidate strategies from a grammar, and compiles every one.
///
/// Takes its randomness as a seed and its identity prefix as a string. It
/// reads no clock: the strategies it writes are a function of the seed, the
/// palette and the grammar, and of nothing else.
#[derive(Clone, Debug)]
pub struct StrategyGenerator {
    grammar: Grammar,
    rng: Xoshiro256,
    lineage: String,
    minted: usize,
}

impl StrategyGenerator {
    /// A generator over `grammar`, naming its output `{lineage}-g{n}`.
    pub fn new(grammar: Grammar, lineage: impl Into<String>, seed: u64) -> Self {
        Self {
            grammar,
            rng: Xoshiro256::seeded(seed),
            lineage: lineage.into(),
            minted: 0,
        }
    }

    pub const fn grammar(&self) -> &Grammar {
        &self.grammar
    }

    /// How many candidates this generator has named so far.
    pub const fn minted(&self) -> usize {
        self.minted
    }

    /// One candidate spec, before the compiler has seen it.
    ///
    /// Public because a caller may want to look at what the grammar wrote, and
    /// harmless because a `StrategySpec` is inert — it is not a
    /// [`Candidate`] and nothing in this crate will evaluate it.
    pub fn propose(&mut self) -> StrategySpec {
        let id = StrategyId::new(format!("{}-g{:04}", self.lineage, self.minted));
        self.minted += 1;
        self.grammar.spec(&mut self.rng, id)
    }

    /// Propose `count` candidates and put each one through the compiler.
    pub fn generate(&mut self, count: usize, compiler: &mut StrategyCompiler) -> GenerationRun {
        let mut run = GenerationRun {
            accepted: Vec::new(),
            discarded: Vec::new(),
            requested: count,
        };
        for _ in 0..count {
            let spec = self.propose();
            match admit(spec, compiler) {
                Ok(candidate) => run.accepted.push(candidate),
                Err(discarded) => run.discarded.push(discarded),
            }
        }
        run
    }
}

//! `qip-evolution` — the evolution brain: where new strategies come from, and
//! what stops the platform believing them.
//!
//! The loop is short. A grammar writes candidate strategies over a vocabulary
//! of declared features; the compiler accepts or refuses each one; a mutator
//! turns a champion into challengers; a challenger is compared with the
//! champion on the same out-of-sample window; and a winner walks the existing
//! promotion ladder. Alongside it, feature discovery proposes new vocabulary,
//! attribution says where the money actually came from, and the cost model is
//! corrected from what execution actually cost.
//!
//! Every one of those steps is easy to write and worthless if written
//! carelessly, for a single reason.
//!
//! ## The central risk is multiple testing
//!
//! Generate ten thousand strategies, keep the one with the best backtest, and
//! you have promoted noise. This is not a subtle failure — the best of ten
//! thousand coin-flippers over two years of daily data shows an annualised
//! Sharpe above three — and it is not one that discipline prevents, because
//! the arithmetic that produces it looks exactly like the arithmetic that
//! produces a discovery. The only difference is a number nobody was obliged to
//! write down: how many were tried.
//!
//! So the crate makes that number structural rather than optional, in four
//! places, and each one is a type rather than a rule.
//!
//! **A candidate is a proof, not a proposal.** [`generate::Candidate`] has no
//! public constructor and holds a [`qip_strategy::compile::CompiledStrategy`],
//! so holding one is evidence that the platform's own type checker and cost
//! bound accepted the spec inside it. Every downstream signature —
//! [`mutate::Mutator::mutate`], [`challenger::ChallengerEntry::from_ledger`],
//! [`promotion::advance`] — takes a `Candidate`, so there is no path by which
//! an unchecked strategy reaches an evaluation, and there is exactly one
//! crate-private function, `generate::admit`, where a spec becomes one.
//!
//! **A trial count is counted, not asserted.**
//! [`challenger::TrialCount`] has a private field and no constructor at all.
//! The only expression in the platform that produces one is
//! [`challenger::TrialLedger::declared`], and the only way a ledger's tally
//! moves is by folding in a [`generate::GenerationRun`] or a
//! [`mutate::MutationRun`] — values that exist only because a generator or a
//! mutator actually ran. Writing `trials: 1` is not a temptation to resist; it
//! does not typecheck.
//!
//! ```compile_fail
//! # use qip_evolution::challenger::TrialCount;
//! let flattering = TrialCount(1);
//! ```
//!
//! ```
//! # use qip_evolution::challenger::TrialLedger;
//! // A ledger that has counted nothing declares nothing, rather than one.
//! assert!(TrialLedger::new().declared().is_none());
//! ```
//!
//! And a search whose size nobody recorded is refused rather than scored:
//! [`challenger::ChallengerTest::evaluate`] returns an error, not a low score,
//! because a low score is a position on a scale and an undeclared search is not
//! on the scale at all.
//!
//! **A score is net of costs.** [`cost_model::NetReturns`] cannot be built from
//! a return series alone — every constructor requires a cost series beside it —
//! and [`challenger::ChallengerEntry`] takes `NetReturns` rather than
//! `Vec<f64>`. Gross-alpha promotion is the most common way a paper strategy
//! becomes a losing live one, and it almost always happens because somebody
//! forgot rather than because somebody lied, so forgetting is the thing that
//! had to be made impossible. A window that was charged nothing at all is
//! refused outright.
//!
//! **There is one path to capital, and it is not here.** [`qip_lifecycle`]
//! owns it. [`promotion`] calls [`qip_lifecycle::attempt_promotion`] and adds
//! exactly two conditions on top: the challenger must have won its comparison,
//! and [`promotion::ChampionBook`] refuses to crown a strategy the ledger does
//! not show standing on every rung. Nothing in this crate records a promotion,
//! issues an approval, or decides a stage. A generated strategy walks the same
//! ladder as a hand-written one, at the same speed.
//!
//! The same correction appears once more, in a different currency.
//! [`discovery::FeatureScreen`] refuses an undeclared screen size and raises
//! the correlation a feature must clear by `sqrt(2·ln(m)/n)` — the largest
//! correlation `m` noise series would show against `n` observations — then
//! checks the sign again on each fold of a purged, embargoed split. Screening a
//! thousand features and keeping the best correlation is the same mistake in
//! different clothes.
//!
//! ## What is deliberately *not* prevented
//!
//! Understating a trial count remains possible for somebody who sets out to do
//! it: run two searches, report one ledger. Passing a cost series of near-zeros
//! remains possible too. Neither is defended against, because a defence would
//! have to guess at intent, and a check that can be satisfied by a determined
//! caller and confuses an honest one is worse than none. What is defended
//! against is the accident — the missing argument, the forgotten deduction, the
//! comfortable default — which is how selection bias actually reaches
//! production.
//!
//! ## Determinism and exactness
//!
//! Nothing here reads a clock or draws from ambient randomness. Generation and
//! mutation take a `u64` seed and draw from [`qip_core::rng::Xoshiro256`], so
//! the same seed over the same palette produces byte-identical strategies; the
//! set of edits available for a spec is computed from the spec
//! ([`mutate`]'s `applicable_kinds`) rather than attempted and abandoned, so
//! the stream advances identically whatever shape a champion turned out to
//! have. Where a time is needed it is passed in as a [`qip_core::Timestamp`].
//!
//! Money, prices and sizes are [`qip_core::Decimal`] — exact `i128` at nine
//! decimal places — and [`attribution::Attribution::identity_holds`] asserts
//! that the parts sum to the whole with no tolerance term, because an
//! attribution that needs a tolerance is one nobody trusts and one nobody
//! trusts gets replaced by a story. `f64` appears only for statistics —
//! Sharpe ratios, correlations, shrinkage weights, cost errors in basis points
//! — and is named so.

pub mod attribution;
pub mod challenger;
pub mod cost_model;
pub mod discovery;
pub mod generate;
pub mod grammar;
pub mod mutate;
pub mod palette;
pub mod promotion;
pub mod scoring;

pub use attribution::{Attribution, AttributionLedger, RealisedTrade};
pub use challenger::{
    ChallengerEntry, ChallengerPolicy, ChallengerTest, ChallengerVerdict, TrialCount, TrialLedger,
};
pub use cost_model::{CostModelCalibration, CostModelUpdate, CostObservation, NetReturns};
pub use discovery::{DiscoveryPolicy, FeatureCandidate, FeatureProposal, FeatureScreen};
pub use generate::{Candidate, Discarded, GenerationRun, StrategyGenerator};
pub use grammar::{Grammar, GrammarLimits};
pub use mutate::{Challenger, Mutation, MutationKind, MutationRun, Mutator, RejectedMutation};
pub use palette::FeaturePalette;
pub use promotion::{ChampionBook, Succession, advance, advance_challenger};
pub use scoring::{ContextualScore, Outcome, ScoreBand, ScoreDomain, Scoreboard, evidence_weight};

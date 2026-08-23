//! The grammar candidate strategies are written in.
//!
//! Everything here builds [`qip_strategy::ir`] values and nothing here checks
//! them. That division is deliberate. The grammar is type-directed where it
//! can cheaply be — it puts a statistic literal opposite a statistic feature
//! rather than opposite an exact one — but it does not attempt to predict the
//! compiler's answer, because a second implementation of the type rules is a
//! second implementation that can drift. A candidate is a proposal; the
//! compiler decides.
//!
//! Two grammar decisions carry a reason worth stating.
//!
//! **A freshly generated rule declares zero observations.** [`Rule`] carries
//! the sample size behind its conviction into
//! [`qip_contracts::Conviction`], which shrinks a belief toward a coin flip by
//! how little evidence supports it. A generated strategy has no evidence at
//! all, so any other number would be a claim the search cannot make. The
//! consequence is that a candidate's conviction reads as 0.5 until a live run
//! has given it observations, which is the honest reading.
//!
//! **The first rule protects and later rules open.** Rules are tried in order
//! and the first whose condition holds wins, so an exit written behind an
//! entry that fires can never be reached. Putting the protective rule first is
//! not a stylistic preference; it is the only ordering in which both rules can
//! matter.

use crate::palette::FeaturePalette;
use qip_contracts::{SignalKind, StrategyId};
use qip_core::rng::{Rng, Xoshiro256};
use qip_core::{Decimal, Duration};
use qip_strategy::ir::{CompareOp, Expr, LogicalOp, Rule, StrategySpec, Type};

/// The shape and the constants the grammar draws from.
///
/// Every field is a finite grid rather than a range, so a threshold is one of
/// a handful of readable numbers instead of an arbitrary 17-digit float that
/// nobody can review and no second run reproduces after a rounding change.
#[derive(Clone, Debug, PartialEq)]
pub struct GrammarLimits {
    /// Most rules one generated strategy may carry.
    pub max_rules: usize,
    /// Most comparisons one condition may combine.
    pub max_condition_terms: usize,
    /// Thresholds a statistic may be compared against.
    pub statistic_thresholds: Vec<f64>,
    /// Thresholds an exact quantity may be compared against.
    pub exact_thresholds: Vec<Decimal>,
    /// Thresholds a count may be compared against.
    pub count_thresholds: Vec<u64>,
    /// Sizes a rule may ask for.
    pub sizes: Vec<Decimal>,
    /// Convictions a rule may assert, before shrinkage.
    pub convictions: Vec<f64>,
    /// How long an emitted signal stays actionable — the horizon, as the IR
    /// expresses it.
    pub validities: Vec<Duration>,
}

impl Default for GrammarLimits {
    fn default() -> Self {
        Self {
            // Three rules is enough for an exit, an entry and a hedge. More
            // than that and the later ones are usually unreachable, which the
            // compiler reports and nobody reads.
            max_rules: 3,
            max_condition_terms: 2,
            statistic_thresholds: vec![-1.5, -1.0, -0.5, -0.25, 0.0, 0.25, 0.5, 1.0, 1.5],
            exact_thresholds: [1, 5, 10, 25, 50, 100]
                .into_iter()
                .map(Decimal::from_int)
                .collect(),
            count_thresholds: vec![1, 3, 5, 10, 25, 100],
            sizes: [10, 25, 50, 100, 250]
                .into_iter()
                .map(Decimal::from_int)
                .collect(),
            convictions: vec![0.52, 0.55, 0.60, 0.65, 0.70, 0.80],
            validities: vec![
                Duration::from_millis(250),
                Duration::from_secs(1),
                Duration::from_secs(30),
                Duration::from_mins(5),
                Duration::from_hours(1),
                Duration::from_days(1),
            ],
        }
    }
}

/// Draw one item, or `None` from an empty slice.
///
/// Consumes exactly one number from the stream whenever the slice is
/// non-empty, so two runs over the same palette make the same draws.
pub(crate) fn pick<'a, T>(rng: &mut Xoshiro256, items: &'a [T]) -> Option<&'a T> {
    if items.is_empty() {
        return None;
    }
    let index = rng.below(items.len() as u64) as usize;
    items.get(index)
}

/// The comparison operators a generated condition may use.
///
/// Equality is absent. Two statistics are almost never exactly equal, the
/// compiler warns about it, and a search that spends part of its budget
/// writing conditions that never hold is a search with a smaller budget.
const ORDERED_COMPARISONS: [CompareOp; 4] = [
    CompareOp::Less,
    CompareOp::AtMost,
    CompareOp::Greater,
    CompareOp::AtLeast,
];

/// Builds candidate IR from a palette.
///
/// Stateless with respect to randomness: every method takes the stream, so one
/// generator and one mutator can share a grammar without sharing a position in
/// the stream.
#[derive(Clone, Debug, PartialEq)]
pub struct Grammar {
    palette: FeaturePalette,
    limits: GrammarLimits,
}

impl Grammar {
    pub fn new(palette: FeaturePalette, limits: GrammarLimits) -> Self {
        Self { palette, limits }
    }

    /// A grammar over `palette` with the default grids.
    pub fn over(palette: FeaturePalette) -> Self {
        Self::new(palette, GrammarLimits::default())
    }

    pub const fn palette(&self) -> &FeaturePalette {
        &self.palette
    }

    pub const fn limits(&self) -> &GrammarLimits {
        &self.limits
    }

    /// A whole candidate strategy under `id`.
    pub fn spec(&self, rng: &mut Xoshiro256, id: StrategyId) -> StrategySpec {
        let rules = 1 + rng.below(self.limits.max_rules.max(1) as u64) as usize;
        let mut spec = StrategySpec::new(id, self.palette.subject().clone(), self.validity(rng));
        for index in 0..rules {
            spec = spec.with_rule(self.rule(rng, index));
        }
        spec
    }

    /// One rule, named for its position so a mutation report can point at it.
    pub fn rule(&self, rng: &mut Xoshiro256, index: usize) -> Rule {
        Rule::new(
            format!("r{index}"),
            self.signal_kind(rng, index),
            self.condition(rng),
            self.size(rng),
            self.conviction(rng),
            // No observations: see the module documentation. A generated rule
            // has never been right about anything yet.
            0,
        )
    }

    /// What the rule at `index` asks for.
    ///
    /// The first rule protects, later rules open or offset. See the module
    /// documentation for why the order is not cosmetic.
    pub fn signal_kind(&self, rng: &mut Xoshiro256, index: usize) -> SignalKind {
        let choices: &[SignalKind] = if index == 0 {
            &[SignalKind::Exit, SignalKind::Stand]
        } else {
            &[SignalKind::Enter, SignalKind::Hedge]
        };
        pick(rng, choices).copied().unwrap_or(SignalKind::Stand)
    }

    /// A flag expression: one comparison, or several combined.
    pub fn condition(&self, rng: &mut Xoshiro256) -> Expr {
        let terms = 1 + rng.below(self.limits.max_condition_terms.max(1) as u64) as usize;
        let mut condition = self.comparison(rng);
        for _ in 1..terms {
            let op = if rng.bernoulli(0.5) {
                LogicalOp::And
            } else {
                LogicalOp::Or
            };
            let next = self.comparison(rng);
            condition = match op {
                LogicalOp::And => condition.and(next),
                LogicalOp::Or => condition.or(next),
            };
        }
        condition
    }

    /// One comparison, or a flag feature read directly.
    fn comparison(&self, rng: &mut Xoshiro256) -> Expr {
        let types = self.palette.types();
        let Some(value_type) = pick(rng, &types).copied() else {
            // Unreachable for a palette built through `FeaturePalette`, which
            // refuses to be empty. A literal keeps the grammar total rather
            // than making every caller handle an impossible `None`.
            return Expr::Flag(true);
        };
        let keys = self.palette.of_type(value_type);
        let Some(key) = pick(rng, keys).cloned() else {
            return Expr::Flag(true);
        };

        if value_type == Type::Flag {
            let feature = Expr::feature(key);
            return if rng.bernoulli(0.5) {
                feature.inverted()
            } else {
                feature
            };
        }

        let op = pick(rng, &ORDERED_COMPARISONS)
            .copied()
            .unwrap_or(CompareOp::Greater);
        // Three quarters of comparisons are against a constant and a quarter
        // against another feature of the same type. A grammar that only ever
        // compared against constants could not express "one venue's pressure
        // above another's", which is most of what a spread strategy is.
        let right = if keys.len() > 1 && rng.bernoulli(0.25) {
            pick(rng, keys)
                .cloned()
                .map_or_else(|| self.literal(rng, value_type), Expr::feature)
        } else {
            self.literal(rng, value_type)
        };
        Expr::feature(key).compare(op, right)
    }

    /// A literal of the requested type, drawn from the grid for that type.
    pub fn literal(&self, rng: &mut Xoshiro256, value_type: Type) -> Expr {
        match value_type {
            Type::Statistic => pick(rng, &self.limits.statistic_thresholds)
                .copied()
                .map_or(Expr::Statistic(0.0), Expr::Statistic),
            Type::Exact => pick(rng, &self.limits.exact_thresholds)
                .copied()
                .map_or_else(|| Expr::Exact(Decimal::ZERO), Expr::Exact),
            Type::Count => pick(rng, &self.limits.count_thresholds)
                .copied()
                .map_or(Expr::Count(0), Expr::Count),
            Type::Flag => Expr::Flag(rng.bernoulli(0.5)),
        }
    }

    /// An exact expression for the size a rule asks for.
    ///
    /// A size is a quantity and never a statistic — the compiler enforces
    /// that, and the grammar has no way to write one that is not, because
    /// every branch here produces exact operands.
    pub fn size(&self, rng: &mut Xoshiro256) -> Expr {
        let exacts = self.palette.of_type(Type::Exact);
        let base = pick(rng, &self.limits.sizes)
            .copied()
            .unwrap_or_else(|| Decimal::from_int(1));
        if exacts.is_empty() || !rng.bernoulli(0.5) {
            return Expr::Exact(base);
        }
        let Some(key) = pick(rng, exacts).cloned() else {
            return Expr::Exact(base);
        };
        let ceiling = pick(rng, &self.limits.sizes).copied().unwrap_or(base);
        let (low, high) = if base <= ceiling {
            (base, ceiling)
        } else {
            (ceiling, base)
        };
        // A size read from the book, confined to a range the strategy is
        // willing to trade. Unbounded sizing off a feature is how one bad
        // print becomes one enormous order.
        Expr::feature(key).bounded(Expr::Exact(low), Expr::Exact(high))
    }

    /// A statistic expression for how much the rule believes itself.
    ///
    /// Where it reads a feature it confines the result to `[0, 1]`: a
    /// conviction outside that range is not a belief, and
    /// [`qip_contracts::Conviction`] would clamp it later without saying so.
    pub fn conviction(&self, rng: &mut Xoshiro256) -> Expr {
        let flat = pick(rng, &self.limits.convictions).copied().unwrap_or(0.55);
        let numeric = self.palette.numeric_types();
        if numeric.is_empty() || !rng.bernoulli(0.35) {
            return Expr::Statistic(flat);
        }
        let Some(value_type) = pick(rng, &numeric).copied() else {
            return Expr::Statistic(flat);
        };
        let Some(key) = pick(rng, self.palette.of_type(value_type)).cloned() else {
            return Expr::Statistic(flat);
        };
        Expr::feature(key)
            .widened()
            .bounded(Expr::Statistic(0.0), Expr::Statistic(1.0))
    }

    /// How long an emitted signal stays actionable.
    pub fn validity(&self, rng: &mut Xoshiro256) -> Duration {
        pick(rng, &self.limits.validities)
            .copied()
            .unwrap_or_else(|| Duration::from_secs(1))
    }
}

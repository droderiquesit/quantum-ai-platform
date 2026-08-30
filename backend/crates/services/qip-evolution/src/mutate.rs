//! Turning a champion into challengers.
//!
//! A mutation is a small, named edit to a strategy that already compiles:
//! move a threshold, read a different feature, add a rule, drop a rule, change
//! the horizon. Each one is chosen so that the edit is type-preserving *by
//! construction* wherever the information to do so is available — a threshold
//! is replaced by a value of its own type, and a feature is replaced by
//! another feature the palette declares at the same type.
//!
//! Where that information is not available the mutation is made anyway and the
//! compiler refuses it. That is the honest arrangement: the palette knows the
//! declared type of the features it holds and nothing about a feature the
//! champion reads from somewhere else, and a mutator that quietly declined to
//! touch those would leave part of a champion permanently unexplored while
//! reporting a clean run. A refusal costs one compilation and says exactly
//! what was wrong.
//!
//! Like generation, mutation reads no clock and draws from a seeded stream, so
//! the same champion under the same seed yields byte-identical challengers.

use crate::generate::{Candidate, Discarded, admit};
use crate::grammar::{Grammar, pick};
use qip_contracts::{FeatureKey, StrategyId};
use qip_core::rng::{Rng, Xoshiro256};
use qip_core::{Decimal, Duration};
use qip_strategy::compile::StrategyCompiler;
use qip_strategy::ir::{Expr, StrategySpec, Type};
use serde::{Deserialize, Serialize};

/// The five edits a champion can be put through.
///
/// Deliberately small and deliberately named. A mutation nobody can describe
/// in four words produces a challenger nobody can explain, and an unexplained
/// challenger that wins is worse than one that loses.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MutationKind {
    /// Move one constant inside a rule's condition.
    ShiftThreshold,
    /// Read a different feature in place of one the champion reads.
    SwapFeature,
    /// Insert a new rule written by the grammar.
    AddRule,
    /// Drop one of the champion's rules.
    RemoveRule,
    /// Change how long an emitted signal stays actionable.
    AlterHorizon,
}

impl MutationKind {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::ShiftThreshold => "shift_threshold",
            Self::SwapFeature => "swap_feature",
            Self::AddRule => "add_rule",
            Self::RemoveRule => "remove_rule",
            Self::AlterHorizon => "alter_horizon",
        }
    }
}

/// One edit, with enough detail to reconstruct why a challenger differs.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Mutation {
    pub kind: MutationKind,
    /// Where the edit landed, e.g. `rule 'r1'.condition`.
    pub site: String,
    pub detail: String,
    /// Whether the edit preserved the expression's type by construction.
    ///
    /// `false` does not mean the challenger is wrong — it means this crate did
    /// not have the evidence to keep it right, and the compiler is the one
    /// that will say.
    pub type_preserving: bool,
}

impl Mutation {
    pub fn describe(&self) -> String {
        format!("{} at {}: {}", self.kind.as_str(), self.site, self.detail)
    }
}

/// A compiled challenger, and the single edit that produced it.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Challenger {
    candidate: Candidate,
    champion: StrategyId,
    mutation: Mutation,
}

impl Challenger {
    pub const fn candidate(&self) -> &Candidate {
        &self.candidate
    }

    /// The champion this challenger was derived from.
    pub const fn champion(&self) -> &StrategyId {
        &self.champion
    }

    pub const fn mutation(&self) -> &Mutation {
        &self.mutation
    }

    pub const fn id(&self) -> &StrategyId {
        self.candidate.id()
    }
}

/// A mutation the compiler refused, kept with the edit that caused it.
#[derive(Clone, Debug, PartialEq)]
pub struct RejectedMutation {
    mutation: Mutation,
    discarded: Discarded,
}

impl RejectedMutation {
    pub const fn mutation(&self) -> &Mutation {
        &self.mutation
    }

    pub const fn discarded(&self) -> &Discarded {
        &self.discarded
    }

    pub fn explain(&self) -> String {
        format!(
            "{} -> {}",
            self.mutation.describe(),
            self.discarded.explain()
        )
    }
}

/// What one mutation run produced.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MutationRun {
    accepted: Vec<Challenger>,
    rejected: Vec<RejectedMutation>,
    requested: usize,
}

impl MutationRun {
    pub fn accepted(&self) -> &[Challenger] {
        &self.accepted
    }

    pub fn rejected(&self) -> &[RejectedMutation] {
        &self.rejected
    }

    pub const fn requested(&self) -> usize {
        self.requested
    }

    /// Whether every attempted mutation ended up in one list or the other.
    pub fn accounted_for(&self) -> bool {
        self.accepted.len() + self.rejected.len() == self.requested
    }

    /// The challengers' specs, for a byte-identity check across runs.
    pub fn accepted_specs(&self) -> Vec<&StrategySpec> {
        self.accepted
            .iter()
            .map(|challenger| challenger.candidate().spec())
            .collect()
    }

    /// Challengers that survived the compiler, and therefore the number of
    /// distinct strategies this run put in front of an evaluation.
    pub fn evaluable(&self) -> usize {
        self.accepted.len()
    }
}

/// Mutates a compiled champion into compiled challengers.
#[derive(Clone, Debug)]
pub struct Mutator {
    grammar: Grammar,
    rng: Xoshiro256,
    minted: usize,
}

impl Mutator {
    pub fn new(grammar: Grammar, seed: u64) -> Self {
        Self {
            grammar,
            rng: Xoshiro256::seeded(seed),
            minted: 0,
        }
    }

    pub const fn grammar(&self) -> &Grammar {
        &self.grammar
    }

    pub const fn minted(&self) -> usize {
        self.minted
    }

    /// Produce `count` challengers from `champion`, compiling every one.
    ///
    /// Takes a [`Candidate`] rather than a [`StrategySpec`] so that a champion
    /// that never compiled cannot be the parent of a generation.
    pub fn mutate(
        &mut self,
        champion: &Candidate,
        count: usize,
        compiler: &mut StrategyCompiler,
    ) -> MutationRun {
        let mut run = MutationRun {
            accepted: Vec::new(),
            rejected: Vec::new(),
            requested: count,
        };
        for _ in 0..count {
            let (spec, mutation) = self.mutate_once(champion.spec());
            match admit(spec, compiler) {
                Ok(candidate) => run.accepted.push(Challenger {
                    candidate,
                    champion: champion.id().clone(),
                    mutation,
                }),
                Err(discarded) => run.rejected.push(RejectedMutation {
                    mutation,
                    discarded,
                }),
            }
        }
        run
    }

    /// One edit, on a copy of the champion, under a fresh identity.
    ///
    /// A challenger always gets a new [`StrategyId`]. Reusing the champion's
    /// would make the lifecycle ledger's history describe two different
    /// strategies, and the ledger's whole purpose is that it does not.
    fn mutate_once(&mut self, champion: &StrategySpec) -> (StrategySpec, Mutation) {
        let mut spec = champion.clone();
        spec.id = StrategyId::new(format!("{}-m{:04}", champion.id, self.minted));
        self.minted += 1;

        let applicable = applicable_kinds(&spec);
        let kind = pick(&mut self.rng, &applicable)
            .copied()
            .unwrap_or(MutationKind::AlterHorizon);
        let mutation = match kind {
            MutationKind::ShiftThreshold => self.shift_threshold(&mut spec),
            MutationKind::SwapFeature => self.swap_feature(&mut spec),
            MutationKind::AddRule => self.add_rule(&mut spec),
            MutationKind::RemoveRule => self.remove_rule(&mut spec),
            MutationKind::AlterHorizon => self.alter_horizon(&mut spec),
        };
        (spec, mutation)
    }

    /// Move one constant inside one rule's condition.
    ///
    /// Type-preserving by construction: the replacement is built from the same
    /// [`Expr`] variant it replaces, so a statistic threshold stays a
    /// statistic and an exact one stays exact.
    fn shift_threshold(&mut self, spec: &mut StrategySpec) -> Mutation {
        let Some(rule_index) = self.choose_rule(spec) else {
            return self.alter_horizon(spec);
        };
        let total = spec
            .rules
            .get(rule_index)
            .map_or(0, |rule| count_leaves(&rule.condition, &is_literal));
        if total == 0 {
            return self.alter_horizon(spec);
        }
        // Both draws happen before the tree is touched, so the stream advances
        // by the same amount whatever shape the condition turned out to have.
        let target = self.rng.below(total as u64) as usize;
        let entropy = self.rng.next_u64();

        let mut site = String::new();
        let mut detail = String::new();
        if let Some(rule) = spec.rules.get_mut(rule_index) {
            let mut seen = 0usize;
            let updated = replace_nth_leaf(
                &rule.condition,
                &is_literal,
                target,
                &mut seen,
                &mut |expr| {
                    let shifted = shift_literal(expr, entropy);
                    detail = format!("{expr:?} -> {shifted:?}");
                    shifted
                },
            );
            rule.condition = updated;
            site = format!("rule '{}'.condition", rule.name);
        }
        Mutation {
            kind: MutationKind::ShiftThreshold,
            site,
            detail,
            type_preserving: true,
        }
    }

    /// Read a different feature somewhere in one rule.
    ///
    /// Type-preserving where the palette declares a second feature at the same
    /// type as the one being replaced. Where it does not, the swap is made
    /// across types and the compiler refuses the result — see the module
    /// documentation for why that is preferred to silently doing nothing.
    fn swap_feature(&mut self, spec: &mut StrategySpec) -> Mutation {
        const PARTS: [&str; 3] = ["condition", "size", "conviction"];
        let Some(rule_index) = self.choose_rule(spec) else {
            return self.alter_horizon(spec);
        };
        let counts = spec.rules.get(rule_index).map_or([0usize; 3], |rule| {
            [
                count_leaves(&rule.condition, &is_feature),
                count_leaves(&rule.size, &is_feature),
                count_leaves(&rule.conviction, &is_feature),
            ]
        });
        let total: usize = counts.iter().sum();
        if total == 0 {
            return self.alter_horizon(spec);
        }
        let mut offset = self.rng.below(total as u64) as usize;
        let mut part = 0usize;
        while part < counts.len() && offset >= counts[part] {
            offset -= counts[part];
            part += 1;
        }
        let part = part.min(PARTS.len() - 1);

        let replaced_key = spec
            .rules
            .get(rule_index)
            .and_then(|rule| nth_feature(rule_part(rule, part), offset));
        let declared = replaced_key
            .as_ref()
            .and_then(|key| self.grammar.palette().type_of(key));
        let (replacement, type_preserving) = self.draw_replacement(declared, replaced_key.as_ref());
        let Some(replacement) = replacement else {
            return self.alter_horizon(spec);
        };

        let mut site = String::new();
        if let Some(rule) = spec.rules.get_mut(rule_index) {
            site = format!("rule '{}'.{}", rule.name, PARTS[part]);
            let expression = match part {
                0 => &mut rule.condition,
                1 => &mut rule.size,
                _ => &mut rule.conviction,
            };
            let mut seen = 0usize;
            let updated = replace_nth_leaf(expression, &is_feature, offset, &mut seen, &mut |_| {
                Expr::feature(replacement.clone())
            });
            *expression = updated;
        }
        Mutation {
            kind: MutationKind::SwapFeature,
            site,
            detail: format!(
                "{} -> {}",
                replaced_key.map_or_else(|| "?".to_string(), |key| key.canonical()),
                replacement.canonical()
            ),
            type_preserving,
        }
    }

    /// Pick a replacement feature, preferring one of the same declared type.
    ///
    /// Returns whether the choice was type-preserving. A palette holding a
    /// single feature of the required type has no alternative to offer, so the
    /// swap crosses types and the compiler decides.
    fn draw_replacement(
        &mut self,
        declared: Option<Type>,
        replaced: Option<&FeatureKey>,
    ) -> (Option<FeatureKey>, bool) {
        let same_type: Vec<FeatureKey> = declared
            .map(|value_type| {
                self.grammar
                    .palette()
                    .of_type(value_type)
                    .iter()
                    .filter(|key| replaced.is_none_or(|old| key.canonical() != old.canonical()))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        if !same_type.is_empty() {
            return (pick(&mut self.rng, &same_type).cloned(), true);
        }
        let anything: Vec<FeatureKey> = self
            .grammar
            .palette()
            .types()
            .into_iter()
            .flat_map(|value_type| self.grammar.palette().of_type(value_type).to_vec())
            .filter(|key| replaced.is_none_or(|old| key.canonical() != old.canonical()))
            .collect();
        (pick(&mut self.rng, &anything).cloned(), false)
    }

    /// Insert a rule the grammar wrote.
    fn add_rule(&mut self, spec: &mut StrategySpec) -> Mutation {
        let position = self.rng.below((spec.rules.len() + 1) as u64) as usize;
        let mut rule = self.grammar.rule(&mut self.rng, position);
        rule.name = format!("r{position}-a{}", self.minted);
        let name = rule.name.clone();
        spec.rules.insert(position.min(spec.rules.len()), rule);
        Mutation {
            kind: MutationKind::AddRule,
            site: format!("rule '{name}'"),
            detail: format!("inserted at position {position}"),
            type_preserving: true,
        }
    }

    /// Drop one rule.
    ///
    /// Only offered when the champion has two, because a strategy with no
    /// rules is refused by the compiler for a reason that has nothing to do
    /// with the edit being interesting.
    fn remove_rule(&mut self, spec: &mut StrategySpec) -> Mutation {
        if spec.rules.len() < 2 {
            return self.alter_horizon(spec);
        }
        let position = self.rng.below(spec.rules.len() as u64) as usize;
        let removed = spec.rules.remove(position.min(spec.rules.len() - 1));
        Mutation {
            kind: MutationKind::RemoveRule,
            site: format!("rule '{}'", removed.name),
            detail: format!("removed from position {position}"),
            type_preserving: true,
        }
    }

    /// Change how long a signal stays actionable.
    ///
    /// The horizon is not a decoration on a strategy: it decides whether the
    /// same edge is worth the cost of trading it. Mutating it is mutating the
    /// strategy.
    fn alter_horizon(&mut self, spec: &mut StrategySpec) -> Mutation {
        let current = spec.validity;
        let alternatives: Vec<Duration> = self
            .grammar
            .limits()
            .validities
            .iter()
            .copied()
            .filter(|candidate| *candidate != current)
            .collect();
        let chosen = pick(&mut self.rng, &alternatives)
            .copied()
            .unwrap_or(current);
        spec.validity = chosen;
        Mutation {
            kind: MutationKind::AlterHorizon,
            site: "validity".to_string(),
            detail: format!("{}ms -> {}ms", current.as_millis(), chosen.as_millis()),
            type_preserving: true,
        }
    }

    fn choose_rule(&mut self, spec: &StrategySpec) -> Option<usize> {
        if spec.rules.is_empty() {
            return None;
        }
        Some(self.rng.below(spec.rules.len() as u64) as usize)
    }
}

/// The edits that can be applied to this spec at all.
///
/// Computed from the spec rather than attempted and abandoned, so the stream
/// is consumed identically on every run over the same champion.
fn applicable_kinds(spec: &StrategySpec) -> Vec<MutationKind> {
    let mut kinds = vec![MutationKind::AddRule, MutationKind::AlterHorizon];
    if spec
        .rules
        .iter()
        .any(|rule| count_leaves(&rule.condition, &is_literal) > 0)
    {
        kinds.push(MutationKind::ShiftThreshold);
    }
    if spec.rules.iter().any(|rule| {
        count_leaves(&rule.condition, &is_feature)
            + count_leaves(&rule.size, &is_feature)
            + count_leaves(&rule.conviction, &is_feature)
            > 0
    }) {
        kinds.push(MutationKind::SwapFeature);
    }
    if spec.rules.len() >= 2 {
        kinds.push(MutationKind::RemoveRule);
    }
    kinds.sort_by_key(|kind| kind.as_str());
    kinds
}

/// One of the three expressions a rule is made of.
fn rule_part(rule: &qip_strategy::ir::Rule, part: usize) -> &Expr {
    match part {
        0 => &rule.condition,
        1 => &rule.size,
        _ => &rule.conviction,
    }
}

fn is_literal(expr: &Expr) -> bool {
    matches!(
        expr,
        Expr::Exact(_) | Expr::Statistic(_) | Expr::Count(_) | Expr::Flag(_)
    )
}

fn is_feature(expr: &Expr) -> bool {
    matches!(expr, Expr::Feature(_))
}

/// Move a constant, keeping its variant and therefore its type.
fn shift_literal(expr: &Expr, entropy: u64) -> Expr {
    // A small ladder of offsets rather than a continuous jitter: a threshold a
    // reviewer can read is worth more than one tuned to the fourth decimal,
    // and a continuous search over a noisy objective is where overfitting
    // lives.
    const STATISTIC_STEPS: [f64; 6] = [-0.5, -0.25, -0.1, 0.1, 0.25, 0.5];
    const EXACT_STEPS: [i64; 6] = [-25, -10, -1, 1, 10, 25];
    let index = (entropy % 6) as usize;
    match expr {
        Expr::Statistic(value) => {
            Expr::Statistic(value + STATISTIC_STEPS.get(index).copied().unwrap_or(0.1))
        }
        Expr::Exact(value) => {
            let step = Decimal::from_int(EXACT_STEPS.get(index).copied().unwrap_or(1));
            Expr::Exact(value.checked_add(step).unwrap_or(*value))
        }
        Expr::Count(value) => {
            let up = index >= 3;
            Expr::Count(if up {
                value.saturating_add(1)
            } else {
                value.saturating_sub(1)
            })
        }
        Expr::Flag(value) => Expr::Flag(!value),
        other => other.clone(),
    }
}

/// How many leaves satisfy `wanted`.
fn count_leaves(expr: &Expr, wanted: &dyn Fn(&Expr) -> bool) -> usize {
    if wanted(expr) {
        return 1;
    }
    expr.children()
        .iter()
        .map(|child| count_leaves(child, wanted))
        .sum()
}

/// The key of the `target`-th feature leaf, in the same pre-order the rewrite
/// walks.
fn nth_feature(expr: &Expr, target: usize) -> Option<FeatureKey> {
    fn walk(expr: &Expr, target: usize, seen: &mut usize) -> Option<FeatureKey> {
        if let Expr::Feature(key) = expr {
            let index = *seen;
            *seen += 1;
            return (index == target).then(|| key.clone());
        }
        for child in expr.children() {
            if let Some(found) = walk(child, target, seen) {
                return Some(found);
            }
        }
        None
    }
    walk(expr, target, &mut 0)
}

/// Rebuild an expression with each child passed through `step`.
///
/// One place that knows the shape of every [`Expr`] variant, so a new variant
/// is a compile error here rather than a silently unvisited subtree.
fn rebuild(expr: &Expr, step: &mut dyn FnMut(&Expr) -> Expr) -> Expr {
    match expr {
        Expr::Exact(_) | Expr::Statistic(_) | Expr::Count(_) | Expr::Flag(_) | Expr::Feature(_) => {
            expr.clone()
        }
        Expr::Negate(inner) => Expr::Negate(Box::new(step(inner))),
        Expr::Magnitude(inner) => Expr::Magnitude(Box::new(step(inner))),
        Expr::Invert(inner) => Expr::Invert(Box::new(step(inner))),
        Expr::Widen(inner) => Expr::Widen(Box::new(step(inner))),
        Expr::Ratio {
            numerator,
            denominator,
        } => Expr::Ratio {
            numerator: Box::new(step(numerator)),
            denominator: Box::new(step(denominator)),
        },
        Expr::Arithmetic { op, left, right } => Expr::Arithmetic {
            op: *op,
            left: Box::new(step(left)),
            right: Box::new(step(right)),
        },
        Expr::Compare { op, left, right } => Expr::Compare {
            op: *op,
            left: Box::new(step(left)),
            right: Box::new(step(right)),
        },
        Expr::Logical { op, left, right } => Expr::Logical {
            op: *op,
            left: Box::new(step(left)),
            right: Box::new(step(right)),
        },
        Expr::Select {
            condition,
            when_true,
            when_false,
        } => Expr::Select {
            condition: Box::new(step(condition)),
            when_true: Box::new(step(when_true)),
            when_false: Box::new(step(when_false)),
        },
        Expr::Bounded { value, low, high } => Expr::Bounded {
            value: Box::new(step(value)),
            low: Box::new(step(low)),
            high: Box::new(step(high)),
        },
        Expr::Extremum { op, left, right } => Expr::Extremum {
            op: *op,
            left: Box::new(step(left)),
            right: Box::new(step(right)),
        },
        Expr::Model { model, inputs } => Expr::Model {
            model: model.clone(),
            inputs: inputs.iter().map(&mut *step).collect(),
        },
    }
}

/// Replace the `target`-th leaf satisfying `wanted`, leaving the rest alone.
fn replace_nth_leaf(
    expr: &Expr,
    wanted: &dyn Fn(&Expr) -> bool,
    target: usize,
    seen: &mut usize,
    make: &mut dyn FnMut(&Expr) -> Expr,
) -> Expr {
    if wanted(expr) {
        let index = *seen;
        *seen += 1;
        return if index == target {
            make(expr)
        } else {
            expr.clone()
        };
    }
    let mut rebuilt = |child: &Expr| replace_nth_leaf(child, wanted, target, seen, make);
    rebuild(expr, &mut rebuilt)
}

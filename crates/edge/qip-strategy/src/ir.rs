//! The typed intermediate representation a strategy is written in.
//!
//! **There is no node in this enum that calls out of the process.** No
//! network, no file, no model service, no language model. That is not a policy
//! enforced by a check somewhere else — it is the absence of a constructor, so
//! there is nothing for a review to miss and nothing for a later change to
//! quietly re-enable without touching this type. The only learned function
//! that reaches the execution path is [`Expr::Model`], and that carries its
//! own coefficients inline: see [`crate::model::DistilledModel`].
//!
//! The IR is a finite tree. It has no loop, no recursion, no call and no
//! binding form, so a strategy's worst-case cost is the size of its own
//! syntax. The compiler measures exactly that and refuses anything it cannot
//! bound.
//!
//! Types are checked, not coerced. An exact quantity and a statistic are
//! different types and no operator mixes them; crossing between them takes
//! [`Expr::Widen`] or [`Expr::Ratio`], written out, visible in review and
//! preserved in the compiled form. Silently promoting a `Decimal` to an `f64`
//! is how a rounding error ends up in a price.

use crate::model::DistilledModel;
use qip_contracts::{FeatureKey, FeatureValue, SignalKind, StrategyId};
use qip_core::{Decimal, Duration, ObjectId};
use serde::{Deserialize, Serialize};
use std::fmt;

/// The type of an expression.
///
/// The same four shapes [`FeatureValue`] carries, as a type rather than a
/// value. `Undefined` is deliberately absent: it is the absence of a value,
/// not a type an expression can have, and a strategy is refused before
/// evaluation rather than typed around it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Type {
    /// An exact quantity: a price, a size, a notional.
    Exact,
    /// A statistic: a volatility, a correlation, a probability.
    Statistic,
    /// A count.
    Count,
    /// A boolean condition.
    Flag,
}

impl Type {
    /// The type of a computed value, or `None` for
    /// [`FeatureValue::Undefined`].
    pub const fn of(value: &FeatureValue) -> Option<Self> {
        match value {
            FeatureValue::Exact(_) => Some(Self::Exact),
            FeatureValue::Statistic(_) => Some(Self::Statistic),
            FeatureValue::Count(_) => Some(Self::Count),
            FeatureValue::Flag(_) => Some(Self::Flag),
            FeatureValue::Undefined => None,
        }
    }

    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Exact => "exact",
            Self::Statistic => "statistic",
            Self::Count => "count",
            Self::Flag => "flag",
        }
    }

    /// Whether arithmetic is meaningful on this type.
    pub const fn is_numeric(&self) -> bool {
        !matches!(self, Self::Flag)
    }

    /// Whether the type has an order, and so can be compared with `<`.
    pub const fn is_ordered(&self) -> bool {
        !matches!(self, Self::Flag)
    }
}

impl fmt::Display for Type {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Arithmetic on two values of one type.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArithmeticOp {
    Add,
    Subtract,
    Multiply,
    Divide,
}

impl ArithmeticOp {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Add => "add",
            Self::Subtract => "subtract",
            Self::Multiply => "multiply",
            Self::Divide => "divide",
        }
    }

    /// Whether the operands may be exchanged without changing the result.
    /// The compiler orders the operands of these so `a + b` and `b + a`
    /// become one shared node.
    pub const fn is_commutative(&self) -> bool {
        matches!(self, Self::Add | Self::Multiply)
    }
}

/// Comparison producing a flag.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompareOp {
    Less,
    AtMost,
    Greater,
    AtLeast,
    Equal,
    Different,
}

impl CompareOp {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Less => "less",
            Self::AtMost => "at_most",
            Self::Greater => "greater",
            Self::AtLeast => "at_least",
            Self::Equal => "equal",
            Self::Different => "different",
        }
    }

    /// Whether the comparison needs an ordering rather than just equality.
    pub const fn needs_order(&self) -> bool {
        !matches!(self, Self::Equal | Self::Different)
    }
}

/// Boolean combination.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LogicalOp {
    And,
    Or,
}

impl LogicalOp {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::And => "and",
            Self::Or => "or",
        }
    }
}

/// Which of two values to keep.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExtremumOp {
    Smaller,
    Larger,
}

impl ExtremumOp {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Smaller => "smaller",
            Self::Larger => "larger",
        }
    }
}

/// An expression over feature values.
///
/// Every variant is total in its arity and finite in its depth. Adding a
/// variant that reached outside the process would be visible here and nowhere
/// else, which is the point of keeping the surface this small.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Expr {
    /// An exact literal.
    Exact(Decimal),
    /// A statistic literal.
    Statistic(f64),
    /// A count literal.
    Count(u64),
    /// A flag literal.
    Flag(bool),
    /// The current value of a registered feature.
    Feature(FeatureKey),
    /// Arithmetic negation.
    Negate(Box<Expr>),
    /// Magnitude.
    Magnitude(Box<Expr>),
    /// Boolean negation.
    Invert(Box<Expr>),
    /// Widen an exact quantity or a count into a statistic.
    ///
    /// The only implicit-looking conversion in the language, made explicit.
    /// After this the value is no longer exact and must not go back into a
    /// price.
    Widen(Box<Expr>),
    /// The ratio of two exact quantities.
    ///
    /// Dimensionless, and therefore a statistic: this is the one place where
    /// exactness is legitimately spent rather than lost by accident.
    Ratio {
        numerator: Box<Expr>,
        denominator: Box<Expr>,
    },
    Arithmetic {
        op: ArithmeticOp,
        left: Box<Expr>,
        right: Box<Expr>,
    },
    Compare {
        op: CompareOp,
        left: Box<Expr>,
        right: Box<Expr>,
    },
    Logical {
        op: LogicalOp,
        left: Box<Expr>,
        right: Box<Expr>,
    },
    /// One of two values of the same type, by a flag.
    Select {
        condition: Box<Expr>,
        when_true: Box<Expr>,
        when_false: Box<Expr>,
    },
    /// Confine a value to a range. The idiom for keeping a denominator away
    /// from zero.
    Bounded {
        value: Box<Expr>,
        low: Box<Expr>,
        high: Box<Expr>,
    },
    Extremum {
        op: ExtremumOp,
        left: Box<Expr>,
        right: Box<Expr>,
    },
    /// A distilled model over statistics, carrying its own coefficients.
    Model {
        model: DistilledModel,
        inputs: Vec<Expr>,
    },
}

impl Expr {
    /// The value of a registered feature.
    pub fn feature(key: FeatureKey) -> Self {
        Self::Feature(key)
    }

    /// An exact literal parsed from its decimal form, or zero if unparseable.
    pub fn exact(value: Decimal) -> Self {
        Self::Exact(value)
    }

    pub fn plus(self, other: Self) -> Self {
        Self::Arithmetic {
            op: ArithmeticOp::Add,
            left: Box::new(self),
            right: Box::new(other),
        }
    }

    pub fn minus(self, other: Self) -> Self {
        Self::Arithmetic {
            op: ArithmeticOp::Subtract,
            left: Box::new(self),
            right: Box::new(other),
        }
    }

    pub fn times(self, other: Self) -> Self {
        Self::Arithmetic {
            op: ArithmeticOp::Multiply,
            left: Box::new(self),
            right: Box::new(other),
        }
    }

    pub fn divided_by(self, other: Self) -> Self {
        Self::Arithmetic {
            op: ArithmeticOp::Divide,
            left: Box::new(self),
            right: Box::new(other),
        }
    }

    pub fn greater_than(self, other: Self) -> Self {
        self.compare(CompareOp::Greater, other)
    }

    pub fn at_least(self, other: Self) -> Self {
        self.compare(CompareOp::AtLeast, other)
    }

    pub fn less_than(self, other: Self) -> Self {
        self.compare(CompareOp::Less, other)
    }

    pub fn at_most(self, other: Self) -> Self {
        self.compare(CompareOp::AtMost, other)
    }

    pub fn equals(self, other: Self) -> Self {
        self.compare(CompareOp::Equal, other)
    }

    pub fn differs_from(self, other: Self) -> Self {
        self.compare(CompareOp::Different, other)
    }

    pub fn compare(self, op: CompareOp, other: Self) -> Self {
        Self::Compare {
            op,
            left: Box::new(self),
            right: Box::new(other),
        }
    }

    pub fn and(self, other: Self) -> Self {
        Self::Logical {
            op: LogicalOp::And,
            left: Box::new(self),
            right: Box::new(other),
        }
    }

    pub fn or(self, other: Self) -> Self {
        Self::Logical {
            op: LogicalOp::Or,
            left: Box::new(self),
            right: Box::new(other),
        }
    }

    pub fn inverted(self) -> Self {
        Self::Invert(Box::new(self))
    }

    pub fn negated(self) -> Self {
        Self::Negate(Box::new(self))
    }

    pub fn magnitude(self) -> Self {
        Self::Magnitude(Box::new(self))
    }

    pub fn widened(self) -> Self {
        Self::Widen(Box::new(self))
    }

    pub fn over(self, denominator: Self) -> Self {
        Self::Ratio {
            numerator: Box::new(self),
            denominator: Box::new(denominator),
        }
    }

    pub fn bounded(self, low: Self, high: Self) -> Self {
        Self::Bounded {
            value: Box::new(self),
            low: Box::new(low),
            high: Box::new(high),
        }
    }

    pub fn smaller_of(self, other: Self) -> Self {
        Self::Extremum {
            op: ExtremumOp::Smaller,
            left: Box::new(self),
            right: Box::new(other),
        }
    }

    pub fn larger_of(self, other: Self) -> Self {
        Self::Extremum {
            op: ExtremumOp::Larger,
            left: Box::new(self),
            right: Box::new(other),
        }
    }

    /// Choose between two values of the same type.
    pub fn select(condition: Self, when_true: Self, when_false: Self) -> Self {
        Self::Select {
            condition: Box::new(condition),
            when_true: Box::new(when_true),
            when_false: Box::new(when_false),
        }
    }

    /// The operator's name, for error messages and reports.
    pub const fn label(&self) -> &'static str {
        match self {
            Self::Exact(_) => "exact literal",
            Self::Statistic(_) => "statistic literal",
            Self::Count(_) => "count literal",
            Self::Flag(_) => "flag literal",
            Self::Feature(_) => "feature",
            Self::Negate(_) => "negate",
            Self::Magnitude(_) => "magnitude",
            Self::Invert(_) => "invert",
            Self::Widen(_) => "widen",
            Self::Ratio { .. } => "ratio",
            Self::Arithmetic { .. } => "arithmetic",
            Self::Compare { .. } => "compare",
            Self::Logical { .. } => "logical",
            Self::Select { .. } => "select",
            Self::Bounded { .. } => "bounded",
            Self::Extremum { .. } => "extremum",
            Self::Model { .. } => "model",
        }
    }

    /// The subexpressions this one is built from.
    pub fn children(&self) -> Vec<&Self> {
        match self {
            Self::Exact(_) | Self::Statistic(_) | Self::Count(_) | Self::Flag(_) | Self::Feature(_) => {
                Vec::new()
            }
            Self::Negate(inner)
            | Self::Magnitude(inner)
            | Self::Invert(inner)
            | Self::Widen(inner) => vec![inner.as_ref()],
            Self::Ratio {
                numerator: left,
                denominator: right,
            }
            | Self::Arithmetic { left, right, .. }
            | Self::Compare { left, right, .. }
            | Self::Logical { left, right, .. }
            | Self::Extremum { left, right, .. } => vec![left.as_ref(), right.as_ref()],
            Self::Select {
                condition: first,
                when_true: second,
                when_false: third,
            }
            | Self::Bounded {
                value: first,
                low: second,
                high: third,
            } => vec![first.as_ref(), second.as_ref(), third.as_ref()],
            Self::Model { inputs, .. } => inputs.iter().collect(),
        }
    }

    /// Every feature this expression reads, in first-seen order.
    pub fn features(&self) -> Vec<FeatureKey> {
        let mut found = Vec::new();
        self.collect_features(&mut found);
        found
    }

    fn collect_features(&self, found: &mut Vec<FeatureKey>) {
        match self {
            Self::Feature(key) => {
                if !found.iter().any(|seen: &FeatureKey| seen == key) {
                    found.push(key.clone());
                }
            }
            Self::Exact(_) | Self::Statistic(_) | Self::Count(_) | Self::Flag(_) => {}
            Self::Negate(inner)
            | Self::Magnitude(inner)
            | Self::Invert(inner)
            | Self::Widen(inner) => inner.collect_features(found),
            Self::Ratio {
                numerator: left,
                denominator: right,
            }
            | Self::Arithmetic { left, right, .. }
            | Self::Compare { left, right, .. }
            | Self::Logical { left, right, .. }
            | Self::Extremum { left, right, .. } => {
                left.collect_features(found);
                right.collect_features(found);
            }
            Self::Select {
                condition: first,
                when_true: second,
                when_false: third,
            }
            | Self::Bounded {
                value: first,
                low: second,
                high: third,
            } => {
                first.collect_features(found);
                second.collect_features(found);
                third.collect_features(found);
            }
            Self::Model { inputs, .. } => {
                for input in inputs {
                    input.collect_features(found);
                }
            }
        }
    }
}

/// One condition and what to do when it holds.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Rule {
    /// Names the rule in warnings and in the compiled form.
    pub name: String,
    pub kind: SignalKind,
    /// Must type as [`Type::Flag`].
    pub condition: Expr,
    /// Must type as [`Type::Exact`] — a size is a quantity, never a statistic.
    pub size: Expr,
    /// Must type as [`Type::Statistic`] — a belief is never exact.
    pub conviction: Expr,
    /// Observations behind the conviction. Carried into
    /// [`qip_contracts::Conviction`], which shrinks a belief by how little
    /// evidence supports it.
    pub observations: u32,
}

impl Rule {
    pub fn new(
        name: impl Into<String>,
        kind: SignalKind,
        condition: Expr,
        size: Expr,
        conviction: Expr,
        observations: u32,
    ) -> Self {
        Self {
            name: name.into(),
            kind,
            condition,
            size,
            conviction,
            observations,
        }
    }
}

/// A strategy as written, before it has been checked.
///
/// Rules are tried in order and the first whose condition holds emits the
/// signal. Order is part of the strategy, not an implementation detail: it is
/// what lets an exit rule sit in front of an entry rule and win.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StrategySpec {
    pub id: StrategyId,
    /// The instrument the signals are about.
    pub subject: ObjectId,
    pub rules: Vec<Rule>,
    /// How long an emitted signal stays actionable. A signal without an expiry
    /// gets acted on at the worst possible moment.
    pub validity: Duration,
}

impl StrategySpec {
    pub fn new(id: StrategyId, subject: ObjectId, validity: Duration) -> Self {
        Self {
            id,
            subject,
            rules: Vec::new(),
            validity,
        }
    }

    pub fn with_rule(mut self, rule: Rule) -> Self {
        self.rules.push(rule);
        self
    }
}

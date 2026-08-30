//! The compiled form: a flat, topologically ordered arena of shared nodes.
//!
//! Two properties hold of every program that exists, and both are checked
//! rather than assumed:
//!
//! * **Every child index is strictly smaller than its parent's.** A node can
//!   only refer backwards, so the arena is acyclic by arithmetic rather than
//!   by search. That is what bounds evaluation: one pass over the nodes a
//!   strategy reaches, no revisits, no recursion, no possibility of a loop.
//! * **Identical subexpressions are one node.** Interning happens on the way
//!   in, children first, so two strategies that ask the same question share
//!   the answer.
//!
//! A program deserialised from somewhere else gets the same check, because a
//! forward reference in a stored artifact is exactly the unbounded construct
//! the compiler exists to refuse.

use crate::ir::{ArithmeticOp, CompareOp, ExtremumOp, LogicalOp, Type};
use crate::model::DistilledModel;
use qip_contracts::{FeatureKey, FeatureValue};
use qip_core::Decimal;
use qip_core::error::{Error, Result};
use serde::{Deserialize, Serialize};

/// A node's position in the arena.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct NodeRef(u32);

impl NodeRef {
    pub const fn new(index: u32) -> Self {
        Self(index)
    }

    pub const fn index(self) -> usize {
        self.0 as usize
    }
}

/// What a compiled node does.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Op {
    /// A value fixed at compile time, including anything the compiler folded.
    Literal(FeatureValue),
    /// A read from the feature vector.
    Feature(FeatureKey),
    Negate(NodeRef),
    Magnitude(NodeRef),
    Invert(NodeRef),
    Widen(NodeRef),
    Ratio {
        numerator: NodeRef,
        denominator: NodeRef,
    },
    Arithmetic {
        op: ArithmeticOp,
        left: NodeRef,
        right: NodeRef,
    },
    Compare {
        op: CompareOp,
        left: NodeRef,
        right: NodeRef,
    },
    Logical {
        op: LogicalOp,
        left: NodeRef,
        right: NodeRef,
    },
    Select {
        condition: NodeRef,
        when_true: NodeRef,
        when_false: NodeRef,
    },
    Bounded {
        value: NodeRef,
        low: NodeRef,
        high: NodeRef,
    },
    Extremum {
        op: ExtremumOp,
        left: NodeRef,
        right: NodeRef,
    },
    Model {
        model: DistilledModel,
        inputs: Vec<NodeRef>,
    },
}

impl Op {
    /// The nodes this one reads.
    pub fn children(&self) -> Vec<NodeRef> {
        match self {
            Self::Literal(_) | Self::Feature(_) => Vec::new(),
            Self::Negate(inner)
            | Self::Magnitude(inner)
            | Self::Invert(inner)
            | Self::Widen(inner) => vec![*inner],
            Self::Ratio {
                numerator: left,
                denominator: right,
            }
            | Self::Arithmetic { left, right, .. }
            | Self::Compare { left, right, .. }
            | Self::Logical { left, right, .. }
            | Self::Extremum { left, right, .. } => vec![*left, *right],
            Self::Select {
                condition: first,
                when_true: second,
                when_false: third,
            }
            | Self::Bounded {
                value: first,
                low: second,
                high: third,
            } => vec![*first, *second, *third],
            Self::Model { inputs, .. } => inputs.clone(),
        }
    }

    /// Worst-case steps this node costs, which is one for everything except a
    /// model, whose size is its cost.
    pub fn cost(&self) -> usize {
        match self {
            Self::Model { model, .. } => model.cost(),
            _ => 1,
        }
    }
}

/// One compiled node.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Node {
    pub op: Op,
    /// The type the checker proved, kept so a consumer of the compiled form
    /// does not have to re-derive it.
    pub value_type: Type,
}

/// The shared arena every compiled strategy points into.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Program {
    nodes: Vec<Node>,
}

impl Program {
    /// Adopt a set of nodes, refusing any that could evaluate unboundedly.
    pub fn from_nodes(nodes: Vec<Node>) -> Result<Self> {
        let program = Self { nodes };
        program.validate()?;
        Ok(program)
    }

    /// Refuse a program whose references do not all point strictly backwards.
    ///
    /// A self- or forward reference is a cycle, and a cycle has no worst-case
    /// cost — which is the one thing the hot path cannot accept.
    pub fn validate(&self) -> Result<()> {
        for (index, node) in self.nodes.iter().enumerate() {
            for child in node.op.children() {
                if child.index() >= index {
                    return Err(Error::guard(format!(
                        "compiled node {index} reads node {}, which is not strictly earlier — \
                         evaluation would not be bounded",
                        child.index()
                    )));
                }
            }
        }
        Ok(())
    }

    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    pub fn node(&self, node: NodeRef) -> Option<&Node> {
        self.nodes.get(node.index())
    }

    pub fn nodes(&self) -> &[Node] {
        &self.nodes
    }

    /// Drop everything after `length`, used to undo a failed compilation.
    pub(crate) fn truncate(&mut self, length: usize) {
        self.nodes.truncate(length);
    }

    pub(crate) fn push(&mut self, node: Node) -> NodeRef {
        let index = self.nodes.len() as u32;
        self.nodes.push(node);
        NodeRef::new(index)
    }

    /// The nodes reachable from `roots`, in evaluation order.
    ///
    /// One descending pass suffices because every reference points backwards.
    pub fn reachable_from(&self, roots: &[NodeRef]) -> Vec<NodeRef> {
        let mut needed = vec![false; self.nodes.len()];
        for root in roots {
            if root.index() < needed.len() {
                needed[root.index()] = true;
            }
        }
        for index in (0..self.nodes.len()).rev() {
            if !needed[index] {
                continue;
            }
            for child in self.nodes[index].op.children() {
                if child.index() < needed.len() {
                    needed[child.index()] = true;
                }
            }
        }
        needed
            .iter()
            .enumerate()
            .filter(|&(_, &wanted)| wanted)
            .map(|(index, _)| NodeRef::new(index as u32))
            .collect()
    }

    /// Worst-case evaluation steps if every node were reached.
    ///
    /// The ceiling any strategy in this program can cost, and so the default
    /// runtime budget. It is larger than the node count because a distilled
    /// model costs its own size rather than one step.
    pub fn total_cost(&self) -> usize {
        self.nodes.iter().map(|node| node.op.cost()).sum()
    }

    /// Worst-case evaluation steps for a set of roots.
    pub fn cost_of(&self, roots: &[NodeRef]) -> usize {
        self.reachable_from(roots)
            .iter()
            .filter_map(|node| self.node(*node))
            .map(|node| node.op.cost())
            .sum()
    }
}

/// Evaluate one node against already-computed operand values.
///
/// Shared by the constant folder and the runtime, so a value folded at compile
/// time and the same value computed at run time cannot disagree.
pub(crate) fn evaluate_op(op: &Op, values: &[Option<FeatureValue>]) -> Result<FeatureValue> {
    let operand = |node: NodeRef| -> Result<FeatureValue> {
        values.get(node.index()).copied().flatten().ok_or_else(|| {
            Error::invalid(format!(
                "node {} was read before it was computed",
                node.index()
            ))
        })
    };

    match op {
        Op::Literal(value) => Ok(*value),
        Op::Feature(key) => Err(Error::not_found(format!(
            "feature {} has no value in this evaluation",
            key.canonical()
        ))),
        Op::Negate(inner) => match operand(*inner)? {
            FeatureValue::Exact(value) => Decimal::ZERO
                .checked_sub(value)
                .map(FeatureValue::Exact)
                .ok_or_else(|| Error::numeric("negation overflowed")),
            FeatureValue::Statistic(value) => Ok(FeatureValue::Statistic(-value)),
            other => Err(mismatch("negate", other)),
        },
        Op::Magnitude(inner) => match operand(*inner)? {
            FeatureValue::Exact(value) => Ok(FeatureValue::Exact(value.abs())),
            FeatureValue::Statistic(value) => Ok(FeatureValue::Statistic(value.abs())),
            FeatureValue::Count(value) => Ok(FeatureValue::Count(value)),
            other => Err(mismatch("magnitude", other)),
        },
        Op::Invert(inner) => match operand(*inner)? {
            FeatureValue::Flag(value) => Ok(FeatureValue::Flag(!value)),
            other => Err(mismatch("invert", other)),
        },
        Op::Widen(inner) => match operand(*inner)? {
            FeatureValue::Exact(value) => Ok(FeatureValue::Statistic(value.to_f64())),
            FeatureValue::Count(value) => Ok(FeatureValue::Statistic(value as f64)),
            FeatureValue::Statistic(value) => Ok(FeatureValue::Statistic(value)),
            other => Err(mismatch("widen", other)),
        },
        Op::Ratio {
            numerator,
            denominator,
        } => match (operand(*numerator)?, operand(*denominator)?) {
            (FeatureValue::Exact(top), FeatureValue::Exact(bottom)) => {
                if bottom.is_zero() {
                    return Err(Error::numeric(
                        "ratio divided by zero — bound the denominator away from it",
                    ));
                }
                Ok(FeatureValue::Statistic(top.to_f64() / bottom.to_f64()))
            }
            (left, _) => Err(mismatch("ratio", left)),
        },
        Op::Arithmetic { op, left, right } => arithmetic(*op, operand(*left)?, operand(*right)?),
        Op::Compare { op, left, right } => Ok(FeatureValue::Flag(compare(
            *op,
            operand(*left)?,
            operand(*right)?,
        )?)),
        Op::Logical { op, left, right } => match (operand(*left)?, operand(*right)?) {
            (FeatureValue::Flag(a), FeatureValue::Flag(b)) => Ok(FeatureValue::Flag(match op {
                LogicalOp::And => a && b,
                LogicalOp::Or => a || b,
            })),
            (left, _) => Err(mismatch("logical", left)),
        },
        Op::Select {
            condition,
            when_true,
            when_false,
        } => match operand(*condition)? {
            // Both branches are already computed: the cost of a strategy must
            // not depend on which way the market went, or its latency becomes
            // a function of the thing it is trying to react to.
            FeatureValue::Flag(true) => operand(*when_true),
            FeatureValue::Flag(false) => operand(*when_false),
            other => Err(mismatch("select", other)),
        },
        Op::Bounded { value, low, high } => {
            let (value, low, high) = (operand(*value)?, operand(*low)?, operand(*high)?);
            if compare(CompareOp::Greater, low, high)? {
                return Err(Error::invalid("bounded range is inverted"));
            }
            let lifted = if compare(CompareOp::Less, value, low)? {
                low
            } else {
                value
            };
            Ok(if compare(CompareOp::Greater, lifted, high)? {
                high
            } else {
                lifted
            })
        }
        Op::Extremum { op, left, right } => {
            let (left, right) = (operand(*left)?, operand(*right)?);
            let take_left = match op {
                ExtremumOp::Smaller => compare(CompareOp::AtMost, left, right)?,
                ExtremumOp::Larger => compare(CompareOp::AtLeast, left, right)?,
            };
            Ok(if take_left { left } else { right })
        }
        Op::Model { model, inputs } => {
            let mut arguments = Vec::with_capacity(inputs.len());
            for input in inputs {
                match operand(*input)? {
                    FeatureValue::Statistic(value) => arguments.push(value),
                    other => return Err(mismatch("model input", other)),
                }
            }
            Ok(FeatureValue::Statistic(model.evaluate(&arguments)?))
        }
    }
}

fn mismatch(operation: &str, value: FeatureValue) -> Error {
    Error::invalid(format!(
        "{operation} was given a {} value, which the type checker should have refused",
        Type::of(&value).map_or("undefined", |kind| kind.as_str())
    ))
}

fn arithmetic(op: ArithmeticOp, left: FeatureValue, right: FeatureValue) -> Result<FeatureValue> {
    match (left, right) {
        (FeatureValue::Exact(a), FeatureValue::Exact(b)) => {
            let result = match op {
                ArithmeticOp::Add => a.checked_add(b),
                ArithmeticOp::Subtract => a.checked_sub(b),
                ArithmeticOp::Multiply => a.checked_mul(b),
                ArithmeticOp::Divide => a.checked_div(b),
            };
            result.map(FeatureValue::Exact).ok_or_else(|| {
                Error::numeric(format!(
                    "exact {} overflowed or divided by zero",
                    op.as_str()
                ))
            })
        }
        (FeatureValue::Statistic(a), FeatureValue::Statistic(b)) => {
            if matches!(op, ArithmeticOp::Divide) && (!b.is_finite() || b.abs() <= 0.0) {
                return Err(Error::numeric(
                    "statistic divided by zero or by a non-finite value",
                ));
            }
            Ok(FeatureValue::Statistic(match op {
                ArithmeticOp::Add => a + b,
                ArithmeticOp::Subtract => a - b,
                ArithmeticOp::Multiply => a * b,
                ArithmeticOp::Divide => a / b,
            }))
        }
        (FeatureValue::Count(a), FeatureValue::Count(b)) => {
            let result = match op {
                ArithmeticOp::Add => a.checked_add(b),
                // A count has no negative values; saturating is the only
                // meaning subtraction can carry here, and it is stated rather
                // than left to wrap.
                ArithmeticOp::Subtract => Some(a.saturating_sub(b)),
                ArithmeticOp::Multiply => a.checked_mul(b),
                ArithmeticOp::Divide => a.checked_div(b),
            };
            result.map(FeatureValue::Count).ok_or_else(|| {
                Error::numeric(format!(
                    "count {} overflowed or divided by zero",
                    op.as_str()
                ))
            })
        }
        (left, _) => Err(mismatch("arithmetic", left)),
    }
}

/// Exact bit equality on two statistics.
///
/// Confined to one function with its own justification: comparing computed
/// floating-point values for equality is almost always a mistake, and the
/// compiler warns about it. It is implemented anyway because a strategy
/// comparing a statistic against a literal it also wrote is entitled to an
/// answer rather than a refusal.
#[allow(clippy::float_cmp)]
fn statistics_equal(left: f64, right: f64) -> bool {
    left == right
}

fn compare(op: CompareOp, left: FeatureValue, right: FeatureValue) -> Result<bool> {
    let ordering = match (left, right) {
        (FeatureValue::Exact(a), FeatureValue::Exact(b)) => Some(a.cmp(&b)),
        (FeatureValue::Count(a), FeatureValue::Count(b)) => Some(a.cmp(&b)),
        (FeatureValue::Flag(a), FeatureValue::Flag(b)) => {
            return match op {
                CompareOp::Equal => Ok(a == b),
                CompareOp::Different => Ok(a != b),
                _ => Err(Error::invalid("flags have no order to compare")),
            };
        }
        (FeatureValue::Statistic(a), FeatureValue::Statistic(b)) => {
            if a.is_nan() || b.is_nan() {
                // A NaN compares false against everything, which would read as
                // a considered decision not to act. It is not; it is a broken
                // input, and it says so.
                return Err(Error::numeric("comparison against a non-finite statistic"));
            }
            return Ok(match op {
                CompareOp::Equal => statistics_equal(a, b),
                CompareOp::Different => !statistics_equal(a, b),
                CompareOp::Less => a < b,
                CompareOp::AtMost => a <= b,
                CompareOp::Greater => a > b,
                CompareOp::AtLeast => a >= b,
            });
        }
        (left, _) => return Err(mismatch("compare", left)),
    };
    let Some(ordering) = ordering else {
        return Err(Error::invalid(
            "values of different types cannot be compared",
        ));
    };
    Ok(match op {
        CompareOp::Equal => ordering.is_eq(),
        CompareOp::Different => !ordering.is_eq(),
        CompareOp::Less => ordering.is_lt(),
        CompareOp::AtMost => ordering.is_le(),
        CompareOp::Greater => ordering.is_gt(),
        CompareOp::AtLeast => ordering.is_ge(),
    })
}

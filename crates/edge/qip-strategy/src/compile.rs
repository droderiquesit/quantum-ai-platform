//! The compiler: static checks, common subexpression elimination, and a
//! refusal to emit anything whose cost cannot be stated.
//!
//! Four things are checked, and a strategy that fails any of them does not
//! compile:
//!
//! 1. **Types.** Every operator's operands agree, and an exact quantity is
//!    never silently compared against or mixed with a statistic.
//! 2. **Declared inputs.** Every feature named exists in the
//!    [`FeatureCatalogue`]. A feature the graph does not have is a miss inside
//!    the latency budget, discovered in production.
//! 3. **Bounded cost.** The IR has no loop to forbid, so the bound is the
//!    computed worst-case node count. Anything above the budget is refused
//!    rather than trimmed, because a strategy that is nearly affordable is not
//!    affordable.
//! 4. **No reachable call out of the process.** Structural: see
//!    [`crate::ir::Expr`], which has no such node.
//!
//! Dead code and conditions that cannot vary are reported as warnings rather
//! than errors. They are usually a mistake — a threshold written the wrong way
//! round, a rule shadowed by the one above it — but they are not unsafe, and a
//! compiler that refuses them makes people work around it.

use crate::catalogue::FeatureCatalogue;
use crate::ir::{Expr, Rule, StrategySpec, Type};
use crate::program::{Node, NodeRef, Op, Program, evaluate_op};
use qip_contracts::{FeatureKey, FeatureValue, SignalKind, StrategyId};
use qip_core::error::{Error, Result};
use qip_core::{Duration, ObjectId};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// What the compiler will accept.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompilerLimits {
    /// Worst-case evaluated nodes for one strategy.
    pub max_nodes: usize,
    /// How deeply an expression may nest. Bounds the compiler's own recursion
    /// as much as the strategy's cost.
    pub max_depth: usize,
}

impl Default for CompilerLimits {
    fn default() -> Self {
        Self {
            max_nodes: 512,
            max_depth: 32,
        }
    }
}

/// Something worth telling the author about that is not a refusal.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WarningKind {
    /// A condition that is true whatever the market does.
    AlwaysTrue,
    /// A condition that is false whatever the market does.
    AlwaysFalse,
    /// A rule that can never be reached, because an earlier one always fires
    /// or asks exactly the same question.
    Unreachable,
    /// A branch that is chosen at compile time, so the other one is dead.
    DeadBranch,
    /// Exact equality between two computed statistics — almost always a
    /// threshold that was meant to be an inequality.
    ExactEqualityOnStatistic,
}

impl WarningKind {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::AlwaysTrue => "always_true",
            Self::AlwaysFalse => "always_false",
            Self::Unreachable => "unreachable",
            Self::DeadBranch => "dead_branch",
            Self::ExactEqualityOnStatistic => "exact_equality_on_statistic",
        }
    }
}

/// One compiler diagnostic.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Warning {
    pub kind: WarningKind,
    /// Where it was found, e.g. `rule 'enter'.condition`.
    pub site: String,
    pub detail: String,
}

impl Warning {
    fn new(kind: WarningKind, site: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            kind,
            site: site.into(),
            detail: detail.into(),
        }
    }
}

/// One compiled rule, as node references into the shared program.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CompiledRule {
    name: String,
    kind: SignalKind,
    condition: NodeRef,
    size: NodeRef,
    conviction: NodeRef,
    observations: u32,
}

impl CompiledRule {
    pub fn name(&self) -> &str {
        &self.name
    }
    pub const fn kind(&self) -> SignalKind {
        self.kind
    }
    pub const fn condition(&self) -> NodeRef {
        self.condition
    }
    pub const fn size(&self) -> NodeRef {
        self.size
    }
    pub const fn conviction(&self) -> NodeRef {
        self.conviction
    }
    pub const fn observations(&self) -> u32 {
        self.observations
    }
}

/// A strategy that has passed every static check.
///
/// It holds node references into a shared [`Program`] rather than a tree of
/// its own, which is how two strategies come to share a subexpression instead
/// of each computing it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CompiledStrategy {
    id: StrategyId,
    subject: ObjectId,
    rules: Vec<CompiledRule>,
    /// Every feature the strategy reads, in canonical order. The runtime
    /// checks these against the vector before evaluating anything.
    inputs: Vec<FeatureKey>,
    /// The nodes this strategy evaluates, in order.
    plan: Vec<NodeRef>,
    /// Worst-case steps, which for a loop-free program is also the exact cost.
    cost: usize,
    validity: Duration,
    warnings: Vec<Warning>,
}

impl CompiledStrategy {
    pub const fn id(&self) -> &StrategyId {
        &self.id
    }
    pub const fn subject(&self) -> &ObjectId {
        &self.subject
    }
    pub fn rules(&self) -> &[CompiledRule] {
        &self.rules
    }
    pub fn inputs(&self) -> &[FeatureKey] {
        &self.inputs
    }
    pub fn plan(&self) -> &[NodeRef] {
        &self.plan
    }
    /// The worst-case node count the compiler proved.
    pub const fn cost(&self) -> usize {
        self.cost
    }
    pub const fn validity(&self) -> Duration {
        self.validity
    }
    pub fn warnings(&self) -> &[Warning] {
        &self.warnings
    }
}

/// How much sharing the compiler achieved.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct CompilerReport {
    pub strategies: usize,
    /// Expression nodes offered across every compilation.
    pub submitted_nodes: usize,
    /// Nodes that actually exist in the shared program.
    pub unique_nodes: usize,
}

impl CompilerReport {
    /// The share of submitted nodes that did not need a node of their own,
    /// because an identical one already existed or the compiler folded them
    /// into a constant.
    pub fn deduplication_ratio(&self) -> f64 {
        if self.submitted_nodes == 0 {
            return 0.0;
        }
        1.0 - (self.unique_nodes as f64 / self.submitted_nodes as f64)
    }
}

/// Compiles strategies into one shared program.
///
/// The compiler is stateful on purpose: the arena it builds is shared across
/// every strategy it compiles, so the second strategy to ask for a
/// microprice-versus-mid comparison gets the node the first one created.
#[derive(Debug)]
pub struct StrategyCompiler {
    catalogue: FeatureCatalogue,
    limits: CompilerLimits,
    program: Program,
    /// Structural key to node, children first. Two expressions with the same
    /// key are the same computation.
    interned: BTreeMap<String, NodeRef>,
    /// The value of each node where it is known at compile time.
    constants: Vec<Option<FeatureValue>>,
    submitted: usize,
    strategies: usize,
}

impl StrategyCompiler {
    pub fn new(catalogue: FeatureCatalogue) -> Self {
        Self::with_limits(catalogue, CompilerLimits::default())
    }

    pub fn with_limits(catalogue: FeatureCatalogue, limits: CompilerLimits) -> Self {
        Self {
            catalogue,
            limits,
            program: Program::default(),
            interned: BTreeMap::new(),
            constants: Vec::new(),
            submitted: 0,
            strategies: 0,
        }
    }

    /// The shared program every compiled strategy points into.
    pub const fn program(&self) -> &Program {
        &self.program
    }

    /// Take the program, for handing to a runtime.
    pub fn into_program(self) -> Program {
        self.program
    }

    pub const fn limits(&self) -> CompilerLimits {
        self.limits
    }

    pub fn report(&self) -> CompilerReport {
        CompilerReport {
            strategies: self.strategies,
            submitted_nodes: self.submitted,
            unique_nodes: self.program.len(),
        }
    }

    /// Check a strategy and lower it into the shared program.
    pub fn compile(&mut self, spec: &StrategySpec) -> Result<CompiledStrategy> {
        if spec.rules.is_empty() {
            return Err(Error::invalid(format!(
                "strategy {} has no rules, so it can never say anything",
                spec.id
            )));
        }

        // Everything before the first mutation, so a refusal leaves the shared
        // program exactly as it was.
        let mut cost = 0usize;
        for rule in &spec.rules {
            for (label, expr) in [
                ("condition", &rule.condition),
                ("size", &rule.size),
                ("conviction", &rule.conviction),
            ] {
                let site = format!("rule '{}'.{label}", rule.name);
                self.measure(expr, 0, &mut cost, &site)?;
            }
        }

        let snapshot = self.program.len();
        let submitted = self.submitted;
        match self.lower_rules(spec) {
            Ok(compiled) => {
                self.strategies += 1;
                Ok(compiled)
            }
            Err(error) => {
                self.rollback(snapshot, submitted);
                Err(error)
            }
        }
    }

    fn lower_rules(&mut self, spec: &StrategySpec) -> Result<CompiledStrategy> {
        let mut warnings = Vec::new();
        let mut rules = Vec::with_capacity(spec.rules.len());

        for rule in &spec.rules {
            let condition = self.lower_typed(
                &rule.condition,
                Type::Flag,
                &format!("rule '{}'.condition", rule.name),
                &mut warnings,
            )?;
            let size = self.lower_typed(
                &rule.size,
                Type::Exact,
                &format!("rule '{}'.size", rule.name),
                &mut warnings,
            )?;
            let conviction = self.lower_typed(
                &rule.conviction,
                Type::Statistic,
                &format!("rule '{}'.conviction", rule.name),
                &mut warnings,
            )?;
            rules.push(CompiledRule {
                name: rule.name.clone(),
                kind: rule.kind,
                condition,
                size,
                conviction,
                observations: rule.observations,
            });
        }

        self.diagnose_rules(&spec.rules, &rules, &mut warnings);

        let roots: Vec<NodeRef> = rules
            .iter()
            .flat_map(|rule| [rule.condition, rule.size, rule.conviction])
            .collect();
        let plan = self.program.reachable_from(&roots);
        let cost = self.program.cost_of(&roots);
        if cost > self.limits.max_nodes {
            return Err(Error::guard(format!(
                "strategy {} evaluates {cost} nodes, above the budget of {}",
                spec.id, self.limits.max_nodes
            )));
        }

        let mut inputs: Vec<FeatureKey> = plan
            .iter()
            .filter_map(|node| self.program.node(*node))
            .filter_map(|node| match &node.op {
                Op::Feature(key) => Some(key.clone()),
                _ => None,
            })
            .collect();
        inputs.sort_by_key(FeatureKey::canonical);
        inputs.dedup_by_key(|key| key.canonical());

        Ok(CompiledStrategy {
            id: spec.id.clone(),
            subject: spec.subject.clone(),
            rules,
            inputs,
            plan,
            cost,
            validity: spec.validity,
            warnings,
        })
    }

    /// Undo everything a failed compilation interned.
    fn rollback(&mut self, nodes: usize, submitted: usize) {
        self.program.truncate(nodes);
        self.constants.truncate(nodes);
        self.interned.retain(|_, node| node.index() < nodes);
        self.submitted = submitted;
    }

    /// Refuse anything whose worst case cannot be stated, before touching the
    /// program.
    ///
    /// Recursion here is bounded by `max_depth` because it bails the moment it
    /// is exceeded — the check that protects the compiler is the same one that
    /// protects the strategy.
    fn measure(&self, expr: &Expr, depth: usize, cost: &mut usize, site: &str) -> Result<()> {
        if depth > self.limits.max_depth {
            return Err(Error::guard(format!(
                "{site}: nests deeper than {} levels, so its cost cannot be bounded",
                self.limits.max_depth
            )));
        }
        *cost += match expr {
            Expr::Model { model, .. } => model.cost(),
            _ => 1,
        };
        if *cost > self.limits.max_nodes {
            return Err(Error::guard(format!(
                "{site}: worst-case cost exceeds the budget of {} nodes",
                self.limits.max_nodes
            )));
        }
        for child in expr.children() {
            self.measure(child, depth + 1, cost, site)?;
        }
        Ok(())
    }

    fn lower_typed(
        &mut self,
        expr: &Expr,
        expected: Type,
        site: &str,
        warnings: &mut Vec<Warning>,
    ) -> Result<NodeRef> {
        let (node, actual) = self.lower(expr, site, warnings)?;
        if actual != expected {
            return Err(Error::invalid(format!(
                "{site}: expected {expected}, found {actual}"
            )));
        }
        Ok(node)
    }

    fn lower(
        &mut self,
        expr: &Expr,
        site: &str,
        warnings: &mut Vec<Warning>,
    ) -> Result<(NodeRef, Type)> {
        self.submitted += 1;
        match expr {
            Expr::Exact(value) => {
                Ok(self.literal(FeatureValue::Exact(*value), Type::Exact))
            }
            Expr::Statistic(value) => {
                Ok(self.literal(FeatureValue::Statistic(*value), Type::Statistic))
            }
            Expr::Count(value) => Ok(self.literal(FeatureValue::Count(*value), Type::Count)),
            Expr::Flag(value) => Ok(self.literal(FeatureValue::Flag(*value), Type::Flag)),
            Expr::Feature(key) => {
                let Some(value_type) = self.catalogue.type_of(key) else {
                    return Err(Error::not_found(format!(
                        "{site}: feature {} is not registered in the feature graph",
                        key.canonical()
                    )));
                };
                Ok((self.intern(Op::Feature(key.clone()), value_type), value_type))
            }
            Expr::Negate(inner) => {
                let (node, value_type) = self.lower(inner, site, warnings)?;
                if !matches!(value_type, Type::Exact | Type::Statistic) {
                    return Err(Error::invalid(format!(
                        "{site}: cannot negate a {value_type}"
                    )));
                }
                Ok((self.intern(Op::Negate(node), value_type), value_type))
            }
            Expr::Magnitude(inner) => {
                let (node, value_type) = self.lower(inner, site, warnings)?;
                if !value_type.is_numeric() {
                    return Err(Error::invalid(format!(
                        "{site}: cannot take the magnitude of a {value_type}"
                    )));
                }
                Ok((self.intern(Op::Magnitude(node), value_type), value_type))
            }
            Expr::Invert(inner) => {
                let (node, value_type) = self.lower(inner, site, warnings)?;
                if value_type != Type::Flag {
                    return Err(Error::invalid(format!(
                        "{site}: cannot invert a {value_type}; only a flag has a negation"
                    )));
                }
                Ok((self.intern(Op::Invert(node), Type::Flag), Type::Flag))
            }
            Expr::Widen(inner) => {
                let (node, value_type) = self.lower(inner, site, warnings)?;
                if value_type == Type::Flag {
                    return Err(Error::invalid(format!(
                        "{site}: a flag is not a quantity and cannot be widened"
                    )));
                }
                Ok((
                    self.intern(Op::Widen(node), Type::Statistic),
                    Type::Statistic,
                ))
            }
            Expr::Ratio {
                numerator,
                denominator,
            } => {
                let (top, top_type) = self.lower(numerator, site, warnings)?;
                let (bottom, bottom_type) = self.lower(denominator, site, warnings)?;
                if top_type != Type::Exact || bottom_type != Type::Exact {
                    return Err(Error::invalid(format!(
                        "{site}: a ratio divides one exact quantity by another, \
                         given {top_type} and {bottom_type}"
                    )));
                }
                Ok((
                    self.intern(
                        Op::Ratio {
                            numerator: top,
                            denominator: bottom,
                        },
                        Type::Statistic,
                    ),
                    Type::Statistic,
                ))
            }
            Expr::Arithmetic { op, left, right } => {
                let (left_node, left_type) = self.lower(left, site, warnings)?;
                let (right_node, right_type) = self.lower(right, site, warnings)?;
                if left_type != right_type {
                    return Err(Error::invalid(format!(
                        "{site}: cannot {} a {left_type} and a {right_type}; \
                         convert explicitly if that is what was meant",
                        op.as_str()
                    )));
                }
                if !left_type.is_numeric() {
                    return Err(Error::invalid(format!(
                        "{site}: cannot {} two flags",
                        op.as_str()
                    )));
                }
                let (left_node, right_node) =
                    order_operands(op.is_commutative(), left_node, right_node);
                Ok((
                    self.intern(
                        Op::Arithmetic {
                            op: *op,
                            left: left_node,
                            right: right_node,
                        },
                        left_type,
                    ),
                    left_type,
                ))
            }
            Expr::Compare { op, left, right } => {
                let (left_node, left_type) = self.lower(left, site, warnings)?;
                let (right_node, right_type) = self.lower(right, site, warnings)?;
                if left_type != right_type {
                    return Err(Error::invalid(format!(
                        "{site}: cannot compare a {left_type} with a {right_type}; \
                         an exact quantity and a statistic are different types on purpose"
                    )));
                }
                if op.needs_order() && !left_type.is_ordered() {
                    return Err(Error::invalid(format!(
                        "{site}: a {left_type} has no order to compare"
                    )));
                }
                if left_type == Type::Statistic && !op.needs_order() {
                    warnings.push(Warning::new(
                        WarningKind::ExactEqualityOnStatistic,
                        site,
                        "two statistics are compared for exact equality; \
                         an inequality is almost always what was meant",
                    ));
                }
                Ok((
                    self.intern(
                        Op::Compare {
                            op: *op,
                            left: left_node,
                            right: right_node,
                        },
                        Type::Flag,
                    ),
                    Type::Flag,
                ))
            }
            Expr::Logical { op, left, right } => {
                let (left_node, left_type) = self.lower(left, site, warnings)?;
                let (right_node, right_type) = self.lower(right, site, warnings)?;
                if left_type != Type::Flag || right_type != Type::Flag {
                    return Err(Error::invalid(format!(
                        "{site}: {} combines two flags, given {left_type} and {right_type}",
                        op.as_str()
                    )));
                }
                let (left_node, right_node) = order_operands(true, left_node, right_node);
                Ok((
                    self.intern(
                        Op::Logical {
                            op: *op,
                            left: left_node,
                            right: right_node,
                        },
                        Type::Flag,
                    ),
                    Type::Flag,
                ))
            }
            Expr::Select {
                condition,
                when_true,
                when_false,
            } => {
                let (condition_node, condition_type) = self.lower(condition, site, warnings)?;
                if condition_type != Type::Flag {
                    return Err(Error::invalid(format!(
                        "{site}: a selection is made on a flag, not a {condition_type}"
                    )));
                }
                // Both branches are checked whichever is taken. Dead code that
                // does not type-check is still wrong, and it will be reached
                // the day the condition stops being constant.
                let (true_node, true_type) = self.lower(when_true, site, warnings)?;
                let (false_node, false_type) = self.lower(when_false, site, warnings)?;
                if true_type != false_type {
                    return Err(Error::invalid(format!(
                        "{site}: the branches of a selection must agree, \
                         found {true_type} and {false_type}"
                    )));
                }
                if let Some(FeatureValue::Flag(taken)) = self.constant_of(condition_node) {
                    warnings.push(Warning::new(
                        WarningKind::DeadBranch,
                        site,
                        format!(
                            "the condition is fixed at compile time, so the {} branch is dead",
                            if taken { "false" } else { "true" }
                        ),
                    ));
                    return Ok((if taken { true_node } else { false_node }, true_type));
                }
                Ok((
                    self.intern(
                        Op::Select {
                            condition: condition_node,
                            when_true: true_node,
                            when_false: false_node,
                        },
                        true_type,
                    ),
                    true_type,
                ))
            }
            Expr::Bounded { value, low, high } => {
                let (value_node, value_type) = self.lower(value, site, warnings)?;
                let (low_node, low_type) = self.lower(low, site, warnings)?;
                let (high_node, high_type) = self.lower(high, site, warnings)?;
                if value_type != low_type || value_type != high_type {
                    return Err(Error::invalid(format!(
                        "{site}: a bound must be the same type as what it bounds, \
                         found {value_type}, {low_type} and {high_type}"
                    )));
                }
                if !value_type.is_ordered() {
                    return Err(Error::invalid(format!(
                        "{site}: a {value_type} has no order to bound"
                    )));
                }
                Ok((
                    self.intern(
                        Op::Bounded {
                            value: value_node,
                            low: low_node,
                            high: high_node,
                        },
                        value_type,
                    ),
                    value_type,
                ))
            }
            Expr::Extremum { op, left, right } => {
                let (left_node, left_type) = self.lower(left, site, warnings)?;
                let (right_node, right_type) = self.lower(right, site, warnings)?;
                if left_type != right_type {
                    return Err(Error::invalid(format!(
                        "{site}: cannot take the {} of a {left_type} and a {right_type}",
                        op.as_str()
                    )));
                }
                if !left_type.is_ordered() {
                    return Err(Error::invalid(format!(
                        "{site}: a {left_type} has no order"
                    )));
                }
                let (left_node, right_node) = order_operands(true, left_node, right_node);
                Ok((
                    self.intern(
                        Op::Extremum {
                            op: *op,
                            left: left_node,
                            right: right_node,
                        },
                        left_type,
                    ),
                    left_type,
                ))
            }
            Expr::Model { model, inputs } => {
                if inputs.len() != model.arity() {
                    return Err(Error::invalid(format!(
                        "{site}: model {} takes {} inputs, given {}",
                        model.name(),
                        model.arity(),
                        inputs.len()
                    )));
                }
                let mut lowered = Vec::with_capacity(inputs.len());
                for (position, input) in inputs.iter().enumerate() {
                    let (node, value_type) = self.lower(input, site, warnings)?;
                    if value_type != Type::Statistic {
                        return Err(Error::invalid(format!(
                            "{site}: model {} input {position} is a {value_type}; \
                             widen it explicitly if that is what was meant",
                            model.name()
                        )));
                    }
                    lowered.push(node);
                }
                Ok((
                    self.intern(
                        Op::Model {
                            model: model.clone(),
                            inputs: lowered,
                        },
                        Type::Statistic,
                    ),
                    Type::Statistic,
                ))
            }
        }
    }

    fn diagnose_rules(
        &self,
        source: &[Rule],
        compiled: &[CompiledRule],
        warnings: &mut Vec<Warning>,
    ) {
        let mut certain: Option<&str> = None;
        let mut seen: BTreeMap<usize, &str> = BTreeMap::new();

        for (rule, source_rule) in compiled.iter().zip(source) {
            let site = format!("rule '{}'", rule.name);
            if let Some(earlier) = certain {
                warnings.push(Warning::new(
                    WarningKind::Unreachable,
                    site.as_str(),
                    format!("rule '{earlier}' always fires first"),
                ));
            }
            match self.constant_of(rule.condition) {
                Some(FeatureValue::Flag(true)) => {
                    warnings.push(Warning::new(
                        WarningKind::AlwaysTrue,
                        site.as_str(),
                        "the condition holds whatever the market does",
                    ));
                    if certain.is_none() {
                        certain = Some(source_rule.name.as_str());
                    }
                }
                Some(FeatureValue::Flag(false)) => warnings.push(Warning::new(
                    WarningKind::AlwaysFalse,
                    site.as_str(),
                    "the condition can never hold, so the rule is dead",
                )),
                _ => {}
            }
            match seen.entry(rule.condition.index()) {
                std::collections::btree_map::Entry::Vacant(slot) => {
                    slot.insert(&source_rule.name);
                }
                std::collections::btree_map::Entry::Occupied(slot) => {
                    warnings.push(Warning::new(
                        WarningKind::Unreachable,
                        &site,
                        format!("rule '{}' asks exactly the same question first", slot.get()),
                    ));
                }
            }
        }
    }

    fn literal(&mut self, value: FeatureValue, value_type: Type) -> (NodeRef, Type) {
        (self.intern(Op::Literal(value), value_type), value_type)
    }

    fn constant_of(&self, node: NodeRef) -> Option<FeatureValue> {
        self.constants.get(node.index()).copied().flatten()
    }

    /// Give an operation a node, reusing one if the same operation is already
    /// there, and folding it to a literal when every operand is known.
    fn intern(&mut self, op: Op, value_type: Type) -> NodeRef {
        let op = match self.fold(&op) {
            Some(value) => Op::Literal(value),
            None => op,
        };
        let key = structural_key(&op);
        if let Some(existing) = self.interned.get(&key) {
            return *existing;
        }
        let constant = match &op {
            Op::Literal(value) => Some(*value),
            _ => None,
        };
        let node = self.program.push(Node { op, value_type });
        self.constants.push(constant);
        self.interned.insert(key, node);
        node
    }

    /// The value of an operation whose operands are all known at compile time.
    fn fold(&self, op: &Op) -> Option<FeatureValue> {
        if matches!(op, Op::Literal(_) | Op::Feature(_)) {
            return None;
        }
        let children = op.children();
        if children
            .iter()
            .any(|child| self.constant_of(*child).is_none())
        {
            return None;
        }
        // A fold that fails — a division by zero, say — is left as a node so
        // the failure surfaces at evaluation with the strategy that caused it,
        // rather than as a compile error on an expression nobody may reach.
        evaluate_op(op, &self.constants).ok()
    }
}

/// Put the operands of a commutative operation in a fixed order, so `a + b`
/// and `b + a` are one node rather than two.
fn order_operands(commutative: bool, left: NodeRef, right: NodeRef) -> (NodeRef, NodeRef) {
    if commutative && right < left {
        (right, left)
    } else {
        (left, right)
    }
}

/// A key that is equal exactly when two operations compute the same thing.
///
/// Children appear as node indices, and children are always interned first, so
/// structural equality of the whole subtree reduces to equality of this one
/// string.
fn structural_key(op: &Op) -> String {
    match op {
        Op::Literal(FeatureValue::Exact(value)) => format!("L:e:{value}"),
        Op::Literal(FeatureValue::Statistic(value)) => format!("L:s:{}", value.to_bits()),
        Op::Literal(FeatureValue::Count(value)) => format!("L:c:{value}"),
        Op::Literal(FeatureValue::Flag(value)) => format!("L:f:{value}"),
        Op::Literal(FeatureValue::Undefined) => "L:u".to_string(),
        Op::Feature(key) => format!("F:{}", key.canonical()),
        Op::Negate(inner) => format!("N:{}", inner.index()),
        Op::Magnitude(inner) => format!("M:{}", inner.index()),
        Op::Invert(inner) => format!("I:{}", inner.index()),
        Op::Widen(inner) => format!("W:{}", inner.index()),
        Op::Ratio {
            numerator,
            denominator,
        } => format!("R:{}:{}", numerator.index(), denominator.index()),
        Op::Arithmetic { op, left, right } => {
            format!("A:{}:{}:{}", op.as_str(), left.index(), right.index())
        }
        Op::Compare { op, left, right } => {
            format!("C:{}:{}:{}", op.as_str(), left.index(), right.index())
        }
        Op::Logical { op, left, right } => {
            format!("O:{}:{}:{}", op.as_str(), left.index(), right.index())
        }
        Op::Select {
            condition,
            when_true,
            when_false,
        } => format!(
            "S:{}:{}:{}",
            condition.index(),
            when_true.index(),
            when_false.index()
        ),
        Op::Bounded { value, low, high } => {
            format!("B:{}:{}:{}", value.index(), low.index(), high.index())
        }
        Op::Extremum { op, left, right } => {
            format!("X:{}:{}:{}", op.as_str(), left.index(), right.index())
        }
        Op::Model { model, inputs } => {
            let arguments: Vec<String> = inputs.iter().map(|n| n.index().to_string()).collect();
            format!("D:{}:{}", model.digest(), arguments.join(","))
        }
    }
}

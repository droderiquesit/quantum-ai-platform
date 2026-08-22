//! Evaluating a compiled strategy against one feature vector.
//!
//! The runtime does three things the compiler cannot: it refuses to act on an
//! undefined input, it records which feature revisions a signal came from, and
//! it enforces the node budget a second time. The compiler already proved the
//! cost; the runtime counts it anyway, because a budget that is only checked
//! where the proof was written is a budget that stops applying the moment
//! anything else builds a program.
//!
//! Evaluation cost does not depend on the market. Every node the strategy
//! reaches is computed, whichever way its conditions go, so the latency of a
//! decision is a property of the strategy rather than of the news.

use crate::compile::CompiledStrategy;
use crate::program::{Op, Program, evaluate_op};
use qip_contracts::{Conviction, FeatureValue, FeatureVector, Signal};
use qip_core::error::{Error, Result};
use qip_core::Timestamp;

/// Runs compiled strategies against feature vectors.
#[derive(Debug)]
pub struct StrategyRuntime {
    program: Program,
    /// The most nodes any one run may evaluate.
    budget: usize,
    /// Reused value buffer, so a run allocates nothing.
    values: Vec<Option<FeatureValue>>,
}

impl StrategyRuntime {
    /// A runtime over a compiled program, refusing one that could not be
    /// evaluated in bounded time.
    pub fn new(program: Program) -> Result<Self> {
        program.validate()?;
        let budget = program.len().max(1);
        let values = vec![None; program.len()];
        Ok(Self {
            program,
            budget,
            values,
        })
    }

    /// The same, with a tighter ceiling than the program's own size.
    pub fn with_budget(program: Program, budget: usize) -> Result<Self> {
        let mut runtime = Self::new(program)?;
        runtime.budget = budget;
        Ok(runtime)
    }

    pub const fn program(&self) -> &Program {
        &self.program
    }

    pub const fn budget(&self) -> usize {
        self.budget
    }

    /// Evaluate a strategy and emit a signal if one of its rules fires.
    ///
    /// Returns `Ok(None)` when nothing fires, and — importantly — also when
    /// any input the strategy reads is undefined. A strategy cannot opt out of
    /// that check, because the check is not in the strategy.
    pub fn run(
        &mut self,
        strategy: &CompiledStrategy,
        vector: &FeatureVector,
        as_of: Timestamp,
    ) -> Result<Option<Signal>> {
        if strategy.cost() > self.budget {
            return Err(Error::guard(format!(
                "strategy {} needs {} nodes, above the runtime budget of {}",
                strategy.id(),
                strategy.cost(),
                self.budget
            )));
        }

        // Every input must be present. A feature the vector does not carry is
        // a different failure from one it carries as undefined, and conflating
        // them would hide a strategy pointed at a graph that never had it.
        for key in strategy.inputs() {
            if vector.get(key).is_none() {
                return Err(Error::not_found(format!(
                    "strategy {} reads {}, which this vector does not carry",
                    strategy.id(),
                    key.canonical()
                )));
            }
        }
        let undefined = vector.undefined();
        if strategy.inputs().iter().any(|key| undefined.contains(&key)) {
            return Ok(None);
        }

        self.evaluate_plan(strategy, vector)?;

        for rule in strategy.rules() {
            let fired = match self.value_at(rule.condition())? {
                FeatureValue::Flag(fired) => fired,
                other => {
                    return Err(Error::invalid(format!(
                        "rule '{}' condition evaluated to {other:?}, not a flag",
                        rule.name()
                    )));
                }
            };
            if !fired {
                continue;
            }
            let FeatureValue::Exact(quantity) = self.value_at(rule.size())? else {
                return Err(Error::invalid(format!(
                    "rule '{}' size is not an exact quantity",
                    rule.name()
                )));
            };
            let FeatureValue::Statistic(probability) = self.value_at(rule.conviction())? else {
                return Err(Error::invalid(format!(
                    "rule '{}' conviction is not a statistic",
                    rule.name()
                )));
            };

            // The revisions are the point: a fill can later be attributed to
            // exactly the feature values that produced the signal, not to
            // whatever those features say by the time anyone looks.
            let inputs = strategy
                .inputs()
                .iter()
                .map(|key| {
                    (
                        key.canonical(),
                        vector.revision_of(key).map_or(0, |revision| revision.get()),
                    )
                })
                .collect();

            return Ok(Some(Signal {
                strategy: strategy.id().clone(),
                object_id: strategy.subject().clone(),
                kind: rule.kind(),
                conviction: Conviction::new(probability, rule.observations()),
                desired_quantity: quantity,
                valid_until: as_of.saturating_add(strategy.validity()),
                inputs,
                at: as_of,
            }));
        }
        Ok(None)
    }

    /// Compute every node the strategy reaches, in order.
    fn evaluate_plan(
        &mut self,
        strategy: &CompiledStrategy,
        vector: &FeatureVector,
    ) -> Result<()> {
        self.values.clear();
        self.values.resize(self.program.len(), None);

        let mut spent = 0usize;
        for node in strategy.plan() {
            let Some(compiled) = self.program.node(*node) else {
                return Err(Error::invalid(format!(
                    "strategy {} plans node {}, which the program does not have",
                    strategy.id(),
                    node.index()
                )));
            };
            spent += compiled.op.cost();
            if spent > self.budget {
                return Err(Error::guard(format!(
                    "strategy {} exceeded its {}-node budget while evaluating",
                    strategy.id(),
                    self.budget
                )));
            }
            let value = match &compiled.op {
                Op::Feature(key) => vector.get(key).ok_or_else(|| {
                    Error::not_found(format!(
                        "feature {} vanished between the check and the read",
                        key.canonical()
                    ))
                })?,
                op => evaluate_op(op, &self.values)?,
            };
            if !value.is_defined() {
                return Err(Error::guard(format!(
                    "strategy {} computed an undefined value at node {}",
                    strategy.id(),
                    node.index()
                )));
            }
            self.values[node.index()] = Some(value);
        }
        Ok(())
    }

    fn value_at(&self, node: crate::program::NodeRef) -> Result<FeatureValue> {
        self.values
            .get(node.index())
            .copied()
            .flatten()
            .ok_or_else(|| {
                Error::invalid(format!(
                    "node {} was read before the plan computed it",
                    node.index()
                ))
            })
    }
}

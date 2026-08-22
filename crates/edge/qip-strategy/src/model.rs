//! Distilled models: what "AI on the hot path" is allowed to mean.
//!
//! A distilled model is a small fixed-size function that carries its own
//! coefficients. It loads nothing, calls nothing, and its worst-case cost is a
//! property of the value itself — a linear model costs one pass over its
//! coefficients, a decision tree at most one step per node. That is what makes
//! it admissible inside a latency budget, and it is the only form in which a
//! learned function reaches the execution path at all.
//!
//! What is deliberately absent: any way to fetch weights, consult a service,
//! or evaluate a model whose size is not known at compile time. A large model
//! is trained and evaluated off the hot path; what arrives here is its
//! distillate, reviewed and versioned like any other constant.

use qip_core::error::{Error, Result};
use serde::{Deserialize, Serialize};

/// One node of a decision tree.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TreeNode {
    /// Compare one input against a threshold and descend.
    ///
    /// Both targets must lie *after* this node, which is what makes a descent
    /// terminate in fewer steps than the tree has nodes without needing a
    /// visited set to prove it.
    Branch {
        input: usize,
        threshold: f64,
        /// Taken when the input is below the threshold.
        below: usize,
        /// Taken when it is at or above it.
        at_or_above: usize,
    },
    /// The tree's answer.
    Leaf { value: f64 },
}

/// The functional form of a distilled model.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelForm {
    /// `intercept + sum(coefficient_i * input_i)`.
    Linear {
        intercept: f64,
        coefficients: Vec<f64>,
    },
    /// A decision tree over the inputs, rooted at node zero.
    Tree { arity: usize, nodes: Vec<TreeNode> },
}

/// A bounded model with its coefficients inside it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DistilledModel {
    name: String,
    form: ModelForm,
}

impl DistilledModel {
    /// A linear model. Refuses non-finite coefficients: a NaN weight produces
    /// a NaN score, which compares false against every threshold and so reads
    /// as a quiet decision not to trade.
    pub fn linear(
        name: impl Into<String>,
        intercept: f64,
        coefficients: Vec<f64>,
    ) -> Result<Self> {
        if coefficients.is_empty() {
            return Err(Error::invalid("a linear model needs at least one input"));
        }
        if !intercept.is_finite() || coefficients.iter().any(|c| !c.is_finite()) {
            return Err(Error::invalid("model coefficients must all be finite"));
        }
        Ok(Self {
            name: name.into(),
            form: ModelForm::Linear {
                intercept,
                coefficients,
            },
        })
    }

    /// A decision tree over `arity` inputs, rooted at node zero.
    ///
    /// Every branch must descend to a strictly later node. A tree that could
    /// point backwards is a loop, and a loop has no worst-case cost to put in
    /// a latency budget.
    pub fn tree(name: impl Into<String>, arity: usize, nodes: Vec<TreeNode>) -> Result<Self> {
        if nodes.is_empty() {
            return Err(Error::invalid("a decision tree needs at least one node"));
        }
        if arity == 0 {
            return Err(Error::invalid("a decision tree needs at least one input"));
        }
        for (index, node) in nodes.iter().enumerate() {
            match node {
                TreeNode::Branch {
                    input,
                    threshold,
                    below,
                    at_or_above,
                } => {
                    if *input >= arity {
                        return Err(Error::invalid(format!(
                            "tree node {index} reads input {input} of {arity}"
                        )));
                    }
                    if !threshold.is_finite() {
                        return Err(Error::invalid(format!(
                            "tree node {index} has a non-finite threshold"
                        )));
                    }
                    for target in [*below, *at_or_above] {
                        if target <= index || target >= nodes.len() {
                            return Err(Error::invalid(format!(
                                "tree node {index} descends to {target}, which is not a later node \
                                 — the descent would not be bounded"
                            )));
                        }
                    }
                }
                TreeNode::Leaf { value } => {
                    if !value.is_finite() {
                        return Err(Error::invalid(format!(
                            "tree leaf {index} has a non-finite value"
                        )));
                    }
                }
            }
        }
        Ok(Self {
            name: name.into(),
            form: ModelForm::Tree { arity, nodes },
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub const fn form(&self) -> &ModelForm {
        &self.form
    }

    /// How many inputs the model takes.
    pub fn arity(&self) -> usize {
        match &self.form {
            ModelForm::Linear { coefficients, .. } => coefficients.len(),
            ModelForm::Tree { arity, .. } => *arity,
        }
    }

    /// Worst-case evaluation steps, charged against the strategy's budget.
    pub fn cost(&self) -> usize {
        match &self.form {
            ModelForm::Linear { coefficients, .. } => coefficients.len() + 1,
            ModelForm::Tree { nodes, .. } => nodes.len(),
        }
    }

    /// A stable digest of the model's contents, so two identical models
    /// compile to one shared node and two different ones never do.
    pub fn digest(&self) -> String {
        let mut parts = vec![self.name.clone()];
        match &self.form {
            ModelForm::Linear {
                intercept,
                coefficients,
            } => {
                parts.push("linear".into());
                parts.push(intercept.to_bits().to_string());
                parts.extend(coefficients.iter().map(|c| c.to_bits().to_string()));
            }
            ModelForm::Tree { arity, nodes } => {
                parts.push("tree".into());
                parts.push(arity.to_string());
                for node in nodes {
                    match node {
                        TreeNode::Branch {
                            input,
                            threshold,
                            below,
                            at_or_above,
                        } => parts.push(format!(
                            "b{input}:{}:{below}:{at_or_above}",
                            threshold.to_bits()
                        )),
                        TreeNode::Leaf { value } => parts.push(format!("l{}", value.to_bits())),
                    }
                }
            }
        }
        parts.join("|")
    }

    /// Evaluate against inputs already reduced to statistics.
    pub fn evaluate(&self, inputs: &[f64]) -> Result<f64> {
        if inputs.len() != self.arity() {
            return Err(Error::invalid(format!(
                "model {} takes {} inputs, given {}",
                self.name,
                self.arity(),
                inputs.len()
            )));
        }
        if inputs.iter().any(|value| !value.is_finite()) {
            return Err(Error::numeric(format!(
                "model {} was given a non-finite input",
                self.name
            )));
        }
        match &self.form {
            ModelForm::Linear {
                intercept,
                coefficients,
            } => {
                let mut total = *intercept;
                for (coefficient, input) in coefficients.iter().zip(inputs) {
                    total += coefficient * input;
                }
                Ok(total)
            }
            ModelForm::Tree { nodes, .. } => {
                let mut cursor = 0usize;
                // Bounded by construction — every branch descends forward —
                // and bounded again here, so a hand-built value cannot spin.
                for _ in 0..nodes.len() {
                    match nodes.get(cursor) {
                        Some(TreeNode::Leaf { value }) => return Ok(*value),
                        Some(TreeNode::Branch {
                            input,
                            threshold,
                            below,
                            at_or_above,
                        }) => {
                            let value = inputs.get(*input).copied().unwrap_or(0.0);
                            cursor = if value < *threshold {
                                *below
                            } else {
                                *at_or_above
                            };
                        }
                        None => break,
                    }
                }
                Err(Error::numeric(format!(
                    "model {} did not reach a leaf",
                    self.name
                )))
            }
        }
    }
}

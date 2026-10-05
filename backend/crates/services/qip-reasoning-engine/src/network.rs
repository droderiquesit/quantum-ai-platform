//! Bayesian networks over discrete variables (REASON-012).
//!
//! [`crate::bayes`] updates one hypothesis in log-odds. That cannot weigh
//! *competing* hypotheses: three exclusive explanations updated one at a time
//! do not sum to one, and nothing there says how much of the probability the
//! third lost when the first gained. A [`Network`] holds the structure — each
//! node a distribution over its states given its parents — and
//! [`Network::posterior`] returns the whole distribution over the queried
//! node's states given the evidence, so every answer is a probability and the
//! answers over an exhaustive set sum to one.
//!
//! An uncertain rule is the two-row case: [`Node::uncertain_rule`] is "the
//! conclusion holds with this probability when the premise does, and with that
//! one when it does not".
//!
//! Inference is exact, by enumerating the joint distribution, and so is
//! bounded: a network whose joint has more than [`MAX_JOINT`] assignments is
//! refused rather than approximated. Probabilities are `f64` — these are
//! statistics, and no money is computed from them here.

use std::collections::{BTreeMap, BTreeSet};

use qip_core::error::{Error, Result};
use serde::{Deserialize, Serialize};

/// The largest joint distribution [`Network::posterior`] will enumerate.
pub const MAX_JOINT: u64 = 1 << 16;

/// How far a row of a table may be from summing to one, and how far a
/// posterior over an exhaustive set is promised to be.
pub const TOLERANCE: f64 = 1e-9;

/// One variable: its states, its parents, and its distribution given them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Node {
    pub name: String,
    /// Mutually exclusive and exhaustive.
    pub states: Vec<String>,
    pub parents: Vec<String>,
    /// One distribution over `states` per combination of parent states, the
    /// last parent varying fastest. A node with no parents has one row.
    pub table: Vec<Vec<f64>>,
}

impl Node {
    /// An uncertain rule over two-state variables: `conclusion` is `"true"`
    /// with probability `when_holds` if `premise` is `"true"`, and with
    /// probability `otherwise` if it is not.
    pub fn uncertain_rule(
        conclusion: impl Into<String>,
        premise: impl Into<String>,
        when_holds: f64,
        otherwise: f64,
    ) -> Self {
        Self {
            name: conclusion.into(),
            states: vec!["true".into(), "false".into()],
            parents: vec![premise.into()],
            table: vec![
                vec![when_holds, 1.0 - when_holds],
                vec![otherwise, 1.0 - otherwise],
            ],
        }
    }
}

/// A validated network. Nodes are held parents-first.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Network {
    nodes: Vec<Node>,
}

impl Network {
    /// Refuses a node declared before one of its parents (which also rules
    /// out a cycle), a table of the wrong shape, and any row that is not a
    /// distribution — a row summing to 0.9 would be silently renormalised by
    /// the query and the missing tenth would never be anyone's error.
    pub fn new(nodes: Vec<Node>) -> Result<Self> {
        let mut sizes: BTreeMap<&str, usize> = BTreeMap::new();
        let mut joint = 1u64;
        for node in &nodes {
            let refuse = |what: String| Err(Error::invalid(format!("node '{}' {what}", node.name)));
            let distinct: BTreeSet<&String> = node.states.iter().collect();
            if node.states.is_empty() || distinct.len() != node.states.len() {
                return refuse("needs at least one state and no state twice".into());
            }
            let mut rows = 1usize;
            for parent in &node.parents {
                let Some(size) = sizes.get(parent.as_str()) else {
                    return refuse(format!(
                        "names parent '{parent}', which is not declared before it; \
                         declare every parent first"
                    ));
                };
                rows = rows.saturating_mul(*size);
            }
            if node.table.len() != rows {
                return refuse(format!(
                    "has {} table rows where its parents' states require {rows}",
                    node.table.len()
                ));
            }
            for row in &node.table {
                let valid = row.len() == node.states.len()
                    && row.iter().all(|p| p.is_finite() && (0.0..=1.0).contains(p))
                    && (row.iter().sum::<f64>() - 1.0).abs() <= TOLERANCE;
                if !valid {
                    return refuse(format!(
                        "has the row {row:?}, which is not a distribution over its {} states; \
                         each row must hold one probability per state and sum to 1",
                        node.states.len()
                    ));
                }
            }
            if sizes
                .insert(node.name.as_str(), node.states.len())
                .is_some()
            {
                return refuse("is declared twice; give each node a unique name".into());
            }
            joint = joint.saturating_mul(node.states.len() as u64);
        }
        if joint > MAX_JOINT {
            return Err(Error::invalid(format!(
                "the network's joint distribution has {joint} assignments, above the \
                 {MAX_JOINT} enumerated exactly; split the network or merge states"
            )));
        }
        Ok(Self { nodes })
    }

    /// The probability of each state of `query` given `evidence`, a map from
    /// node name to its observed state.
    ///
    /// Refuses evidence the network gives probability zero: there is no
    /// posterior to report, and the observation contradicts the model, which
    /// is a finding about the model rather than a number.
    pub fn posterior(
        &self,
        query: &str,
        evidence: &BTreeMap<String, String>,
    ) -> Result<BTreeMap<String, f64>> {
        let index: BTreeMap<&str, usize> = self
            .nodes
            .iter()
            .enumerate()
            .map(|(i, node)| (node.name.as_str(), i))
            .collect();
        let Some(&queried) = index.get(query) else {
            return Err(Error::invalid(format!(
                "'{query}' is not a node of the network; query a declared node"
            )));
        };
        let mut observed: Vec<Option<usize>> = vec![None; self.nodes.len()];
        for (name, state) in evidence {
            let position = index
                .get(name.as_str())
                .and_then(|&i| Some((i, self.nodes[i].states.iter().position(|s| s == state)?)));
            let Some((node, state)) = position else {
                return Err(Error::invalid(format!(
                    "the evidence '{name}' = '{state}' names no declared node and state; \
                     supply evidence in the network's own vocabulary"
                )));
            };
            observed[node] = Some(state);
        }
        let parents: Vec<Vec<usize>> = self
            .nodes
            .iter()
            .map(|node| {
                node.parents
                    .iter()
                    .filter_map(|parent| index.get(parent.as_str()).copied())
                    .collect()
            })
            .collect();

        let mut weights = vec![0.0f64; self.nodes[queried].states.len()];
        let mut states = vec![0usize; self.nodes.len()];
        'joint: loop {
            let consistent = observed
                .iter()
                .zip(&states)
                .all(|(seen, state)| seen.is_none_or(|seen| seen == *state));
            if consistent {
                let mut weight = 1.0;
                for (i, node) in self.nodes.iter().enumerate() {
                    let row = parents[i]
                        .iter()
                        .fold(0, |row, &p| row * self.nodes[p].states.len() + states[p]);
                    weight *= node.table[row][states[i]];
                }
                weights[states[queried]] += weight;
            }
            // Advance the mixed-radix counter over every node's states.
            let mut digit = 0;
            loop {
                let Some(state) = states.get_mut(digit) else {
                    break 'joint;
                };
                *state += 1;
                if *state < self.nodes[digit].states.len() {
                    break;
                }
                *state = 0;
                digit += 1;
            }
        }
        let total: f64 = weights.iter().sum();
        if total <= 0.0 {
            return Err(Error::denied(
                "the evidence has probability zero under the network, so there is no posterior; \
                 the observation contradicts the model and the model needs revising",
            ));
        }
        Ok(self.nodes[queried]
            .states
            .iter()
            .zip(&weights)
            .map(|(state, weight)| (state.clone(), weight / total))
            .collect())
    }
}

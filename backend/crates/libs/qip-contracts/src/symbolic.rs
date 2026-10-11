//! `SymbolicDerivation` (blueprint v12.0 §27, R12-CONTRACT-7077): a
//! conclusion together with the trace that reaches it, in a form a machine
//! can re-check without trusting the solver that produced it.
//!
//! "Machine-checkable" is enforced, not described: every step in the trace
//! cites only the declared rules, the declared evidence, or steps before it,
//! and the last step states the conclusion. A trace that cites a premise it
//! never declared, or reaches forward to a later step, is circular or
//! incomplete and is refused here — a derivation whose proof depends on
//! itself proves nothing, however confident the solver was.

use crate::validated::{
    Contract, Validated, basis_points, each_named, listed, named, names, refuse,
};
use qip_core::error::Result;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// What the solver or prover concluded about the goal.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProofResult {
    Proved,
    Refuted,
    /// Satisfiable under the constraints, with a witness in the trace.
    Satisfiable,
    Unsatisfiable,
    /// The solver stopped without an answer (timeout, budget).
    Unknown,
}

/// What a trace step relies on.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum Premise {
    Rule {
        id: String,
    },
    Evidence {
        id: String,
    },
    /// An earlier step, by zero-based index.
    Step {
        index: u32,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TraceStep {
    /// The statement this step establishes.
    pub statement: String,
    /// The inference rule applied (`modus_ponens`, `constraint_propagation`,
    /// a graph-path id, …).
    pub inference: String,
    /// At least one.
    pub premises: Vec<Premise>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SymbolicDerivationFields {
    pub schema_version: u32,
    pub derivation_id: String,
    /// What was asked.
    pub goal: String,
    /// What was concluded; must be the last step's statement.
    pub conclusion: String,
    /// Rule, constraint and graph-path ids the trace may cite; at least one.
    pub rules: Vec<String>,
    /// Evidence ids the trace may cite; may be empty for a pure deduction.
    pub evidence: Vec<String>,
    /// The solver or prover and its version.
    pub solver: String,
    pub result: ProofResult,
    /// Assumptions the derivation found violated; may be empty.
    pub violated_assumptions: Vec<String>,
    /// Residual uncertainty in the conclusion, in basis points — non-zero
    /// when evidence premises are themselves uncertain.
    pub uncertainty_bp: u32,
    /// At least one step.
    pub trace: Vec<TraceStep>,
}

/// The §27 symbolic derivation. Built only through [`Validated::new`].
pub type SymbolicDerivation = Validated<SymbolicDerivationFields>;

impl Contract for SymbolicDerivationFields {
    const NAME: &'static str = "SymbolicDerivation";
    const SCHEMA_VERSION: u32 = 1;

    fn schema_version(&self) -> u32 {
        self.schema_version
    }

    /// Refuses blank names, no rule, no trace, an uncertainty above one, a
    /// step with no premise, a premise naming an undeclared rule or evidence
    /// id, a step citing itself or a later step, and a last step that does
    /// not state the conclusion. A `Proved` result with uncertainty is
    /// admitted: the logic can be certain while its evidence is not.
    fn check(&self) -> Result<()> {
        let c = Self::NAME;
        named(c, "derivation_id", &self.derivation_id)?;
        named(c, "goal", &self.goal)?;
        named(c, "conclusion", &self.conclusion)?;
        names(c, "rules", &self.rules)?;
        each_named(c, "evidence", &self.evidence)?;
        named(c, "solver", &self.solver)?;
        each_named(c, "violated_assumptions", &self.violated_assumptions)?;
        basis_points(c, "uncertainty_bp", self.uncertainty_bp)?;
        listed(c, "trace", &self.trace)?;
        let rules: BTreeSet<&str> = self.rules.iter().map(String::as_str).collect();
        let evidence: BTreeSet<&str> = self.evidence.iter().map(String::as_str).collect();
        for (index, step) in self.trace.iter().enumerate() {
            named(c, "trace.statement", &step.statement)?;
            named(c, "trace.inference", &step.inference)?;
            listed(c, "trace.premises", &step.premises)?;
            for premise in &step.premises {
                let admitted = match premise {
                    Premise::Rule { id } => rules.contains(id.as_str()),
                    Premise::Evidence { id } => evidence.contains(id.as_str()),
                    Premise::Step { index: cited } => (*cited as usize) < index,
                };
                if !admitted {
                    return refuse(
                        c,
                        format!(
                            "has trace step {index} citing {premise:?}, which is undeclared or not \
                             an earlier step; a trace may rest only on what precedes it"
                        ),
                    );
                }
            }
        }
        match self.trace.last() {
            Some(last) if last.statement == self.conclusion => Ok(()),
            _ => refuse(
                c,
                "has a trace whose last step does not state the conclusion; the trace must reach \
                 what it claims",
            ),
        }
    }
}

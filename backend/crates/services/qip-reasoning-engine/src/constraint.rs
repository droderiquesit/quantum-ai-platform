//! Constraint satisfaction over finite domains (REASON-015).
//!
//! A [`Problem`] is variables with declared finite domains and constraints in
//! two forms: a boolean [`Cond`] in the rule packs' own language — which is a
//! SAT instance when the domains are boolean and the conditions are clauses —
//! and a linear inequality over integer variables. [`Problem::solve`] returns a
//! satisfying assignment, or [`Answer::Unsatisfiable`] with a conflicting
//! subset of the constraints.
//!
//! **The conflict is minimal and is itself unsatisfiable.** "No solution" on
//! its own leaves the caller to find which requirements collide; the subset is
//! what a person can act on. It is found by deletion: drop each constraint in
//! turn and keep it dropped only if what remains is still unsatisfiable.
//!
//! This is an in-tree backtracking search, not Z3 (refused under ADR 0099
//! C5), and it decides only what it can enumerate: a problem whose search
//! space exceeds [`MAX_ASSIGNMENTS`] is refused rather than left to run. There
//! are no SMT theories beyond bounded linear integer arithmetic.
//!
//! [`Problem::minimise`] is the same search carried to the best assignment
//! under a linear objective, and [`Problem::to_qubo`] expresses the problems
//! that admit it as a QUBO (REASON-017) — checked against `minimise` on the
//! problem as posed, never against the QUBO's own enumeration.

use std::collections::{BTreeMap, BTreeSet};

use qip_core::error::{Error, Result};
use qip_numerics::anneal::Qubo;
use serde::{Deserialize, Serialize};

use crate::rules::{Cond, Op, Value};

/// The largest search space [`Problem::solve`] will enumerate.
pub const MAX_ASSIGNMENTS: u64 = 1 << 20;

/// A variable and the values it may take, tried in the order declared.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Variable {
    pub name: String,
    pub domain: Vec<Value>,
}

/// What a constraint requires.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Form {
    /// The condition must hold of the assignment.
    Holds(Cond),
    /// `sum(coefficient * variable) op bound`, over integer variables.
    Linear {
        terms: Vec<(i64, String)>,
        op: Op,
        bound: i64,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Constraint {
    pub id: String,
    pub form: Form,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Problem {
    pub variables: Vec<Variable>,
    pub constraints: Vec<Constraint>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Answer {
    Satisfied(BTreeMap<String, Value>),
    /// No assignment satisfies every constraint. `conflict` holds the ids of a
    /// subset that cannot be satisfied together, and stops being
    /// unsatisfiable if any one of them is removed.
    Unsatisfiable {
        conflict: Vec<String>,
    },
}

impl Constraint {
    fn variables(&self) -> BTreeSet<String> {
        let mut out = BTreeSet::new();
        match &self.form {
            Form::Holds(cond) => cond.facts(&mut out),
            Form::Linear { terms, .. } => out.extend(terms.iter().map(|(_, v)| v.clone())),
        }
        out
    }

    fn holds(&self, assignment: &BTreeMap<String, Value>) -> Result<bool> {
        match &self.form {
            Form::Holds(cond) => Ok(cond.holds(assignment)),
            Form::Linear { terms, op, bound } => {
                let sum = linear_sum(&format!("constraint '{}'", self.id), terms, assignment)?;
                Ok(match op {
                    Op::Eq => sum == *bound,
                    Op::Ne => sum != *bound,
                    Op::Lt => sum < *bound,
                    Op::Le => sum <= *bound,
                    Op::Gt => sum > *bound,
                    Op::Ge => sum >= *bound,
                })
            }
        }
    }
}

impl Problem {
    /// A satisfying assignment, or a minimal conflicting subset.
    ///
    /// Refuses a malformed problem rather than reporting it unsatisfiable: a
    /// duplicate name, an empty domain, or a constraint over an undeclared
    /// variable would each produce a verdict about a problem nobody posed.
    pub fn solve(&self) -> Result<Answer> {
        self.validate()?;
        let all: Vec<&Constraint> = self.constraints.iter().collect();
        if let Some(assignment) = self.search(&all)? {
            return Ok(Answer::Satisfied(assignment));
        }
        let mut core = all;
        core.sort_by(|a, b| a.id.cmp(&b.id));
        let mut index = 0;
        while index < core.len() {
            let mut without = core.clone();
            without.remove(index);
            if self.search(&without)?.is_none() {
                core = without;
            } else {
                index += 1;
            }
        }
        Ok(Answer::Unsatisfiable {
            conflict: core.iter().map(|c| c.id.clone()).collect(),
        })
    }

    fn validate(&self) -> Result<()> {
        let mut names = BTreeSet::new();
        let mut space = 1u64;
        for variable in &self.variables {
            if !names.insert(variable.name.as_str()) {
                return Err(Error::invalid(format!(
                    "variable '{}' is declared twice; give each variable a unique name",
                    variable.name
                )));
            }
            if variable.domain.is_empty() {
                return Err(Error::invalid(format!(
                    "variable '{}' has an empty domain; declare the values it may take",
                    variable.name
                )));
            }
            space = space.saturating_mul(variable.domain.len() as u64);
        }
        if space > MAX_ASSIGNMENTS {
            return Err(Error::invalid(format!(
                "the problem has {space} candidate assignments, above the {MAX_ASSIGNMENTS} \
                 this solver enumerates; narrow the domains or split the problem"
            )));
        }
        let mut ids = BTreeSet::new();
        for constraint in &self.constraints {
            if !ids.insert(constraint.id.as_str()) {
                return Err(Error::invalid(format!(
                    "constraint '{}' is declared twice; give each constraint a unique id",
                    constraint.id
                )));
            }
            if let Some(unknown) = constraint
                .variables()
                .iter()
                .find(|v| !names.contains(v.as_str()))
            {
                return Err(Error::invalid(format!(
                    "constraint '{}' names '{unknown}', which is not a declared variable; \
                     declare it with a domain",
                    constraint.id
                )));
            }
        }
        Ok(())
    }

    /// Backtrack over the variables in declared order. A constraint is tested
    /// the moment its last variable is assigned, which is the earliest its
    /// truth is fixed.
    fn search(&self, active: &[&Constraint]) -> Result<Option<BTreeMap<String, Value>>> {
        let mut first = None;
        self.visit(active, &mut |assignment| {
            first = Some(assignment.clone());
            Ok(true)
        })?;
        Ok(first)
    }

    /// Hand every assignment satisfying `active` to `found`, until it answers
    /// `true`.
    fn visit(&self, active: &[&Constraint], found: &mut Visitor<'_>) -> Result<()> {
        let mut due: Vec<Vec<&Constraint>> = vec![Vec::new(); self.variables.len() + 1];
        for constraint in active {
            let names = constraint.variables();
            let depth = self
                .variables
                .iter()
                .rposition(|v| names.contains(&v.name))
                .map_or(0, |last| last + 1);
            due[depth].push(constraint);
        }
        let mut assignment = BTreeMap::new();
        for constraint in &due[0] {
            if !constraint.holds(&assignment)? {
                return Ok(());
            }
        }
        self.extend(0, &due, &mut assignment, found)?;
        Ok(())
    }

    /// Whether `found` asked the search to stop.
    fn extend(
        &self,
        depth: usize,
        due: &[Vec<&Constraint>],
        assignment: &mut BTreeMap<String, Value>,
        found: &mut Visitor<'_>,
    ) -> Result<bool> {
        let Some(variable) = self.variables.get(depth) else {
            return found(assignment);
        };
        for value in &variable.domain {
            assignment.insert(variable.name.clone(), value.clone());
            let mut consistent = true;
            for constraint in &due[depth + 1] {
                if !constraint.holds(assignment)? {
                    consistent = false;
                    break;
                }
            }
            if consistent && self.extend(depth + 1, due, assignment, found)? {
                return Ok(true);
            }
        }
        assignment.remove(&variable.name);
        Ok(false)
    }

    /// The satisfying assignment with the smallest `sum(coefficient *
    /// variable)`, or `None` when nothing satisfies the constraints.
    ///
    /// This is the classical baseline (ADR 0006) a QUBO answer is held
    /// against, and it is computed on the problem as posed — not on any
    /// encoding of it — so an encoding that decodes to something infeasible or
    /// worse cannot agree with it by construction. The search is exhaustive,
    /// so the optimum is proven; ties go to the first found, in declared order.
    pub fn minimise(&self, objective: &[(i64, String)]) -> Result<Option<Optimum>> {
        self.validate()?;
        let all: Vec<&Constraint> = self.constraints.iter().collect();
        let mut best: Option<Optimum> = None;
        self.visit(&all, &mut |assignment| {
            let cost = linear_sum("the objective", objective, assignment)?;
            if best.as_ref().is_none_or(|held| cost < held.objective) {
                best = Some(Optimum {
                    assignment: assignment.clone(),
                    objective: cost,
                });
            }
            Ok(false)
        })?;
        Ok(best)
    }

    /// The problem as a QUBO whose minimum, decoded, is the optimum of this
    /// problem — where the problem admits one (REASON-017).
    ///
    /// It admits one when every variable takes 0 or 1 and every constraint is
    /// a linear equality. Bit `i` is the `i`-th declared variable. Each
    /// equality becomes `penalty * (sum - bound)^2`, with a penalty larger
    /// than the objective's whole range: any assignment breaking a constraint
    /// then costs more than every assignment that breaks none, and on the
    /// feasible ones the energy *is* the objective. Anything else is refused
    /// by name rather than approximated — an inequality needs slack bits this
    /// encoder does not add, and saying so is better than a QUBO whose minimum
    /// solves a different problem.
    ///
    /// The result is the type the quantum path's solver port takes, so the
    /// formulation can be offered to it as a candidate search.
    pub fn to_qubo(&self, objective: &[(i64, String)]) -> Result<Qubo> {
        self.validate()?;
        let binary = [Value::Int(0), Value::Int(1)];
        let mut bit = BTreeMap::new();
        for (index, variable) in self.variables.iter().enumerate() {
            let values: BTreeSet<&Value> = variable.domain.iter().collect();
            if values != binary.iter().collect() {
                return Err(Error::invalid(format!(
                    "variable '{}' does not take exactly 0 and 1, and a QUBO has one bit per \
                     variable; restate it over 0/1 variables or solve it classically",
                    variable.name
                )));
            }
            bit.insert(variable.name.as_str(), index);
        }
        let position = |name: &String| {
            bit.get(name.as_str()).copied().ok_or_else(|| {
                Error::invalid(format!(
                    "the objective names '{name}', which is not a declared variable; declare it \
                     with a domain"
                ))
            })
        };
        let mut qubo = Qubo::new(self.variables.len());
        let mut range = 0.0f64;
        for (coefficient, name) in objective {
            qubo.add_linear(position(name)?, *coefficient as f64);
            range += (*coefficient as f64).abs();
        }
        let penalty = range + 1.0;
        for constraint in &self.constraints {
            let Form::Linear {
                terms,
                op: Op::Eq,
                bound,
            } = &constraint.form
            else {
                return Err(Error::invalid(format!(
                    "constraint '{}' is not a linear equality, the one form this encoder admits; \
                     restate it as one or solve the problem classically",
                    constraint.id
                )));
            };
            // One coefficient per bit: a variable named twice is summed.
            let mut weights: BTreeMap<usize, f64> = BTreeMap::new();
            for (coefficient, name) in terms {
                *weights.entry(position(name)?).or_default() += *coefficient as f64;
            }
            let bound = *bound as f64;
            // (sum - bound)^2, with x^2 = x on every bit.
            for (&i, &a) in &weights {
                qubo.add_linear(i, penalty * (a * a - 2.0 * bound * a));
                for (&j, &b) in weights.range(i + 1..) {
                    qubo.add(i, j, penalty * 2.0 * a * b);
                }
            }
            qubo.offset += penalty * bound * bound;
        }
        Ok(qubo)
    }

    /// Read a QUBO assignment back as a solution of this problem.
    ///
    /// Refuses bits that break a constraint. A sampler returns the best it
    /// found, not the minimum, and the minimum itself breaks a constraint
    /// when the problem has no solution; neither may come back looking like
    /// an answer.
    pub fn decode(&self, objective: &[(i64, String)], bits: &[u8]) -> Result<Optimum> {
        if bits.len() != self.variables.len() || bits.iter().any(|b| *b > 1) {
            return Err(Error::invalid(format!(
                "{} bits were supplied for {} variables, or one is not 0 or 1; supply one bit \
                 per declared variable",
                bits.len(),
                self.variables.len()
            )));
        }
        let assignment: BTreeMap<String, Value> = self
            .variables
            .iter()
            .zip(bits)
            .map(|(variable, b)| (variable.name.clone(), Value::Int(i64::from(*b))))
            .collect();
        for constraint in &self.constraints {
            if !constraint.holds(&assignment)? {
                return Err(Error::denied(format!(
                    "the decoded assignment violates constraint '{}', so it is not a solution; \
                     discard the sample, or treat the problem as unsatisfiable if this was the \
                     proven minimum",
                    constraint.id
                )));
            }
        }
        Ok(Optimum {
            objective: linear_sum("the objective", objective, &assignment)?,
            assignment,
        })
    }
}

/// What the search calls with each satisfying assignment; `true` stops it.
type Visitor<'a> = dyn FnMut(&BTreeMap<String, Value>) -> Result<bool> + 'a;

/// A satisfying assignment and the objective value it attains.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Optimum {
    pub assignment: BTreeMap<String, Value>,
    pub objective: i64,
}

/// `sum(coefficient * variable)` under `assignment`. `what` names the sum in
/// a refusal.
fn linear_sum(
    what: &str,
    terms: &[(i64, String)],
    assignment: &BTreeMap<String, Value>,
) -> Result<i64> {
    let mut sum = 0i64;
    for (coefficient, variable) in terms {
        let Some(Value::Int(value)) = assignment.get(variable) else {
            return Err(Error::invalid(format!(
                "{what} sums '{variable}', which is not an integer; give it an integer domain"
            )));
        };
        sum = coefficient
            .checked_mul(*value)
            .and_then(|term| sum.checked_add(term))
            .ok_or_else(|| {
                Error::numeric(format!("{what} overflows a 64-bit sum; narrow the domains"))
            })?;
    }
    Ok(sum)
}

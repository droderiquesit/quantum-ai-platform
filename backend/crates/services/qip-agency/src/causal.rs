//! R12-AGENCY-6083 / 6084 / 6088: interventions as `do(action)` on a
//! structural causal model, effect sizes learned from data, and every
//! candidate simulated across competing models and scenario branches.
//!
//! The failure prevented is correlation mistaken for control. Before this
//! module a lever was "a controllable variable with a causal path to the
//! target" (`AffordanceGraph::levers`, reachability), and its effect was
//! whatever number the operator's proposal claimed. Reachability says a lever
//! *can* matter; it says nothing about how much, and an effect read off an
//! observational association carries every confounder with it. Here:
//!
//! * a [`StructuralModel`] is a linear SCM over a DAG — each variable is an
//!   intercept, a weighted sum of its parents and an exogenous term — and is
//!   refused if it has a cycle, a non-finite coefficient or a negative noise
//!   variance;
//! * [`StructuralModel::intervene`] is graph surgery: the intervened
//!   variable's incoming edges are cut and its value fixed, so
//!   [`StructuralModel::interventional_effect`] is `E[y | do(x)]`, and
//!   [`StructuralModel::observational_slope`] — the regression of `y` on `x`
//!   the model implies — is a different number whenever a confounder opens a
//!   back-door path. The test that the two differ is the test that this is
//!   not reachability with a coefficient bolted on;
//! * [`StructuralModel::counterfactual`] is abduction–action–prediction: the
//!   exogenous terms are recovered from what was observed, the intervention
//!   is applied, and the same world is replayed under it;
//! * [`StructuralModel::fit`] learns the coefficients of a declared structure
//!   from observations by least squares on each variable's parents — the
//!   "learned causal model" 6083 asks for. What it learns is an effect size;
//!   [`crate::plan::select`] still refuses a plan on a hard constraint before
//!   any estimate is read, so no learned figure, however large, buys its way
//!   past one;
//! * [`simulate`] runs one candidate's interventions through every competing
//!   model under every scenario branch, as counterfactuals against that
//!   branch's own no-action world, and returns one [`SimulationResult`] per
//!   (model, scenario) — never an average that hides the branch where it
//!   fails.
//!
//! Everything here is a statistic, so `f64`; where a figure crosses into a
//! [`qip_core::Decimal`] comparison axis, `crate::generate` states the
//! crossing. Nothing here acts: a simulation is a number about a model.
//!
//! The model's structure is the world model's: [`StructuralModel::from_links`]
//! takes `(cause, effect, signed strength)` triples, which is the shape
//! `qip_world_model::causal::CausalEdge` exposes as `cause`, `effect` and
//! `signed_transmission()`.

use qip_core::Error;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// One structural edge: `effect` moves by `coefficient` per unit of `cause`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StructuralLink {
    pub cause: String,
    pub effect: String,
    pub coefficient: f64,
}

/// A model as declared in a document.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StructuralModelDecl {
    pub name: String,
    pub links: Vec<StructuralLink>,
    /// Variables with no incoming link may still be named here, so a root
    /// with no parents is part of the model.
    #[serde(default)]
    pub variables: Vec<String>,
    #[serde(default)]
    pub intercepts: BTreeMap<String, f64>,
    /// Variance of each variable's exogenous term; absent is zero.
    #[serde(default)]
    pub noise_variance: BTreeMap<String, f64>,
}

/// A linear structural causal model over a DAG.
#[derive(Debug, Clone, PartialEq)]
pub struct StructuralModel {
    name: String,
    /// Topological order: every parent before its child.
    order: Vec<String>,
    parents: BTreeMap<String, Vec<(String, f64)>>,
    intercepts: BTreeMap<String, f64>,
    noise_variance: BTreeMap<String, f64>,
    /// Variables an intervention has fixed, and the value fixed.
    fixed: BTreeMap<String, f64>,
}

fn finite(what: &str, value: f64) -> Result<f64, Error> {
    if value.is_finite() {
        Ok(value)
    } else {
        Err(Error::invalid(format!(
            "{what} is {value}; a structural model takes finite numbers only"
        )))
    }
}

fn blank(name: &str) -> bool {
    name.trim().is_empty()
}

impl StructuralModel {
    /// Build and validate a declared model.
    pub fn new(decl: StructuralModelDecl) -> Result<Self, Error> {
        if blank(&decl.name) {
            return Err(Error::invalid("a structural model needs a name"));
        }
        let mut variables: BTreeSet<String> = BTreeSet::new();
        for v in &decl.variables {
            if blank(v) {
                return Err(Error::invalid("a structural variable name is blank"));
            }
            variables.insert(v.clone());
        }
        let mut parents: BTreeMap<String, Vec<(String, f64)>> = BTreeMap::new();
        for link in &decl.links {
            if blank(&link.cause) || blank(&link.effect) {
                return Err(Error::invalid("a structural link names a blank variable"));
            }
            if link.cause == link.effect {
                return Err(Error::invalid(format!(
                    "`{}` is declared to cause itself; a structural model is acyclic",
                    link.cause
                )));
            }
            let coefficient = finite(
                &format!("the coefficient {} -> {}", link.cause, link.effect),
                link.coefficient,
            )?;
            let entry = parents.entry(link.effect.clone()).or_default();
            if entry.iter().any(|(p, _)| p == &link.cause) {
                return Err(Error::invalid(format!(
                    "{} -> {} is declared twice; state one coefficient",
                    link.cause, link.effect
                )));
            }
            entry.push((link.cause.clone(), coefficient));
            variables.insert(link.cause.clone());
            variables.insert(link.effect.clone());
        }
        for (name, value) in &decl.intercepts {
            if !variables.contains(name) {
                return Err(Error::invalid(format!(
                    "an intercept is given for `{name}`, which the model does not contain"
                )));
            }
            finite(&format!("the intercept of `{name}`"), *value)?;
        }
        for (name, value) in &decl.noise_variance {
            if !variables.contains(name) {
                return Err(Error::invalid(format!(
                    "a noise variance is given for `{name}`, which the model does not contain"
                )));
            }
            if finite(&format!("the noise variance of `{name}`"), *value)? < 0.0 {
                return Err(Error::invalid(format!(
                    "the noise variance of `{name}` is negative; it is refused, not clamped"
                )));
            }
        }
        let order = topological(&variables, &parents)?;
        Ok(Self {
            name: decl.name,
            order,
            parents,
            intercepts: decl.intercepts,
            noise_variance: decl.noise_variance,
            fixed: BTreeMap::new(),
        })
    }

    /// A model from `(cause, effect, signed strength)` triples — the world
    /// model's causal edges — with zero intercepts and unit noise.
    pub fn from_links(
        name: &str,
        links: impl IntoIterator<Item = (String, String, f64)>,
    ) -> Result<Self, Error> {
        let links: Vec<StructuralLink> = links
            .into_iter()
            .map(|(cause, effect, coefficient)| StructuralLink {
                cause,
                effect,
                coefficient,
            })
            .collect();
        let mut noise = BTreeMap::new();
        for link in &links {
            noise.insert(link.cause.clone(), 1.0);
            noise.insert(link.effect.clone(), 1.0);
        }
        Self::new(StructuralModelDecl {
            name: name.to_string(),
            links,
            variables: Vec::new(),
            intercepts: BTreeMap::new(),
            noise_variance: noise,
        })
    }

    /// R12-AGENCY-6083: learn every coefficient, intercept and noise variance
    /// of `structure` from `data` by least squares of each variable on its
    /// declared parents.
    ///
    /// The structure is assumed, not discovered: which variable causes which
    /// is the world model's claim, and fitting cannot tell a cause from a
    /// consequence. What fitting does supply is the *size* of each effect,
    /// which is the uncertain half. Refused: an observation missing a
    /// variable (a regression over whichever rows happened to be complete is
    /// a different population), too few observations to fit a variable's
    /// parents with a residual left over, and a design whose parents are
    /// collinear.
    pub fn fit(
        name: &str,
        structure: &[(String, String)],
        data: &[BTreeMap<String, f64>],
    ) -> Result<Self, Error> {
        let skeleton = Self::new(StructuralModelDecl {
            name: name.to_string(),
            links: structure
                .iter()
                .map(|(cause, effect)| StructuralLink {
                    cause: cause.clone(),
                    effect: effect.clone(),
                    coefficient: 0.0,
                })
                .collect(),
            variables: Vec::new(),
            intercepts: BTreeMap::new(),
            noise_variance: BTreeMap::new(),
        })?;
        for (row, observation) in data.iter().enumerate() {
            for variable in &skeleton.order {
                match observation.get(variable) {
                    Some(value) => {
                        finite(&format!("observation {row} of `{variable}`"), *value)?;
                    }
                    None => {
                        return Err(Error::invalid(format!(
                            "observation {row} does not record `{variable}`; every row must \
                             observe every variable in the structure"
                        )));
                    }
                }
            }
        }
        let mut parents = BTreeMap::new();
        let mut intercepts = BTreeMap::new();
        let mut noise_variance = BTreeMap::new();
        for variable in &skeleton.order {
            let names: Vec<String> = skeleton
                .parents
                .get(variable)
                .map(|ps| ps.iter().map(|(p, _)| p.clone()).collect())
                .unwrap_or_default();
            let (intercept, coefficients, residual) = least_squares(variable, &names, data)?;
            intercepts.insert(variable.clone(), intercept);
            noise_variance.insert(variable.clone(), residual);
            if !names.is_empty() {
                parents.insert(
                    variable.clone(),
                    names.into_iter().zip(coefficients).collect::<Vec<_>>(),
                );
            }
        }
        Ok(Self {
            name: skeleton.name,
            order: skeleton.order,
            parents,
            intercepts,
            noise_variance,
            fixed: BTreeMap::new(),
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn contains(&self, variable: &str) -> bool {
        self.order.iter().any(|v| v == variable)
    }

    /// The fitted or declared coefficient of `cause -> effect`, if the edge
    /// exists and has not been cut by an intervention.
    pub fn coefficient(&self, cause: &str, effect: &str) -> Option<f64> {
        if self.fixed.contains_key(effect) {
            return None;
        }
        self.parents
            .get(effect)?
            .iter()
            .find(|(p, _)| p == cause)
            .map(|(_, c)| *c)
    }

    /// Every variable's value given the exogenous terms (absent is zero).
    pub fn evaluate(&self, exogenous: &BTreeMap<String, f64>) -> BTreeMap<String, f64> {
        let mut values: BTreeMap<String, f64> = BTreeMap::new();
        for variable in &self.order {
            let value = match self.fixed.get(variable) {
                Some(fixed) => *fixed,
                None => {
                    let mut v = self.intercepts.get(variable).copied().unwrap_or(0.0)
                        + exogenous.get(variable).copied().unwrap_or(0.0);
                    for (parent, coefficient) in self.parents.get(variable).into_iter().flatten() {
                        v += coefficient * values.get(parent).copied().unwrap_or(0.0);
                    }
                    v
                }
            };
            values.insert(variable.clone(), value);
        }
        values
    }

    /// `do(set)`: the model with each set variable's incoming edges cut and
    /// its value fixed. The original is untouched.
    pub fn intervene(&self, set: &BTreeMap<String, f64>) -> Result<Self, Error> {
        let mut surgery = self.clone();
        for (variable, value) in set {
            if !self.contains(variable) {
                return Err(Error::invalid(format!(
                    "do({variable}) names a variable model `{}` does not contain",
                    self.name
                )));
            }
            surgery.fixed.insert(
                variable.clone(),
                finite(&format!("do({variable})"), *value)?,
            );
        }
        Ok(surgery)
    }

    /// `dE[effect | do(cause = x)] / dx`, by surgery on `cause` at two values
    /// with every exogenous term at zero. Linear, so the difference is the
    /// slope everywhere.
    pub fn interventional_effect(&self, cause: &str, effect: &str) -> Result<f64, Error> {
        if !self.contains(effect) {
            return Err(Error::invalid(format!(
                "model `{}` does not contain `{effect}`",
                self.name
            )));
        }
        let none = BTreeMap::new();
        let at = |x: f64| -> Result<f64, Error> {
            let world = self
                .intervene(&BTreeMap::from([(cause.to_string(), x)]))?
                .evaluate(&none);
            Ok(world.get(effect).copied().unwrap_or(0.0))
        };
        Ok(at(1.0)? - at(0.0)?)
    }

    /// The slope of `effect` on `cause` the model implies observationally:
    /// `Cov(cause, effect) / Var(cause)`, from the covariance the exogenous
    /// variances propagate. Equal to [`Self::interventional_effect`] only
    /// when no back-door path joins the two; the gap is the confounding.
    pub fn observational_slope(&self, cause: &str, effect: &str) -> Result<f64, Error> {
        let cov = self.covariance();
        let index = |v: &str| {
            self.order.iter().position(|o| o == v).ok_or_else(|| {
                Error::invalid(format!("model `{}` does not contain `{v}`", self.name))
            })
        };
        let (c, e) = (index(cause)?, index(effect)?);
        let variance = cov[c][c];
        if variance <= 0.0 {
            return Err(Error::numeric(format!(
                "`{cause}` has no variance in model `{}`; an association with a constant is \
                 undefined",
                self.name
            )));
        }
        Ok(cov[c][e] / variance)
    }

    /// The implied covariance matrix, in topological order.
    // Indexed loops on purpose: the recursion reads row and column of the
    // same matrix it is writing, and the indices are the formula.
    #[allow(clippy::needless_range_loop)]
    fn covariance(&self) -> Vec<Vec<f64>> {
        let n = self.order.len();
        let mut cov = vec![vec![0.0; n]; n];
        let position: BTreeMap<&str, usize> = self
            .order
            .iter()
            .enumerate()
            .map(|(i, v)| (v.as_str(), i))
            .collect();
        for (i, variable) in self.order.iter().enumerate() {
            if self.fixed.contains_key(variable) {
                continue;
            }
            let parents: Vec<(usize, f64)> = self
                .parents
                .get(variable)
                .into_iter()
                .flatten()
                .filter_map(|(p, b)| position.get(p.as_str()).map(|j| (*j, *b)))
                .collect();
            // Cov(v, u) for every u before v: v's exogenous term is
            // independent of everything earlier, so only the parents carry.
            for u in 0..i {
                let c: f64 = parents.iter().map(|(p, b)| b * cov[*p][u]).sum();
                cov[i][u] = c;
                cov[u][i] = c;
            }
            let mut variance = self.noise_variance.get(variable).copied().unwrap_or(0.0);
            for (p, bp) in &parents {
                for (q, bq) in &parents {
                    variance += bp * bq * cov[*p][*q];
                }
            }
            cov[i][i] = variance;
        }
        cov
    }

    /// Abduction: the exogenous term of every variable, from a complete
    /// observation of the world.
    pub fn abduce(&self, observed: &BTreeMap<String, f64>) -> Result<BTreeMap<String, f64>, Error> {
        let mut noise = BTreeMap::new();
        for variable in &self.order {
            let value = *observed.get(variable).ok_or_else(|| {
                Error::invalid(format!(
                    "the observation does not record `{variable}`; a counterfactual needs the \
                     whole factual world"
                ))
            })?;
            let mut predicted = self.intercepts.get(variable).copied().unwrap_or(0.0);
            for (parent, coefficient) in self.parents.get(variable).into_iter().flatten() {
                predicted += coefficient * observed.get(parent).copied().unwrap_or(0.0);
            }
            noise.insert(variable.clone(), value - predicted);
        }
        Ok(noise)
    }

    /// Abduction, action, prediction: what `observed` would have been under
    /// `do(set)`, holding every exogenous term at what the observation
    /// implies.
    pub fn counterfactual(
        &self,
        observed: &BTreeMap<String, f64>,
        set: &BTreeMap<String, f64>,
    ) -> Result<BTreeMap<String, f64>, Error> {
        let noise = self.abduce(observed)?;
        Ok(self.intervene(set)?.evaluate(&noise))
    }
}

/// Kahn's algorithm, ties broken by name so the order replays.
fn topological(
    variables: &BTreeSet<String>,
    parents: &BTreeMap<String, Vec<(String, f64)>>,
) -> Result<Vec<String>, Error> {
    let mut indegree: BTreeMap<&str, usize> = variables.iter().map(|v| (v.as_str(), 0)).collect();
    let mut children: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for (child, ps) in parents {
        for (parent, _) in ps {
            *indegree.entry(child.as_str()).or_default() += 1;
            children
                .entry(parent.as_str())
                .or_default()
                .push(child.as_str());
        }
    }
    let mut ready: BTreeSet<&str> = indegree
        .iter()
        .filter(|(_, d)| **d == 0)
        .map(|(v, _)| *v)
        .collect();
    let mut order = Vec::with_capacity(variables.len());
    while let Some(next) = ready.pop_first() {
        order.push(next.to_string());
        for child in children.get(next).into_iter().flatten() {
            if let Some(d) = indegree.get_mut(child) {
                *d -= 1;
                if *d == 0 {
                    ready.insert(child);
                }
            }
        }
    }
    if order.len() != variables.len() {
        let stuck: Vec<&str> = indegree
            .iter()
            .filter(|(_, d)| **d > 0)
            .map(|(v, _)| *v)
            .collect();
        return Err(Error::invalid(format!(
            "the structure has a cycle through {stuck:?}; a structural causal model is acyclic"
        )));
    }
    Ok(order)
}

/// Ordinary least squares of `target` on `regressors` with an intercept.
/// Returns the intercept, the coefficients in `regressors` order, and the
/// residual variance (divided by `n - k - 1`).
fn least_squares(
    target: &str,
    regressors: &[String],
    data: &[BTreeMap<String, f64>],
) -> Result<(f64, Vec<f64>, f64), Error> {
    let k = regressors.len() + 1;
    if data.len() <= k {
        return Err(Error::invalid(format!(
            "`{target}` has {} parents and {} observations; fitting needs more observations \
             than parameters, with a residual left over",
            regressors.len(),
            data.len()
        )));
    }
    let row = |o: &BTreeMap<String, f64>| -> Vec<f64> {
        std::iter::once(1.0)
            .chain(regressors.iter().map(|r| o.get(r).copied().unwrap_or(0.0)))
            .collect()
    };
    // Normal equations: (XᵀX) β = Xᵀy.
    let mut xtx = vec![vec![0.0; k]; k];
    let mut xty = vec![0.0; k];
    for o in data {
        let x = row(o);
        let y = o.get(target).copied().unwrap_or(0.0);
        for i in 0..k {
            xty[i] += x[i] * y;
            for j in 0..k {
                xtx[i][j] += x[i] * x[j];
            }
        }
    }
    let beta = solve(xtx, xty).ok_or_else(|| {
        Error::numeric(format!(
            "the parents of `{target}` are collinear in the data, or constant; their separate \
             effects cannot be told apart"
        ))
    })?;
    let mut sse = 0.0;
    for o in data {
        let x = row(o);
        let fitted: f64 = x.iter().zip(&beta).map(|(a, b)| a * b).sum();
        let r = o.get(target).copied().unwrap_or(0.0) - fitted;
        sse += r * r;
    }
    let dof = (data.len() - k) as f64;
    let intercept = beta[0];
    Ok((intercept, beta[1..].to_vec(), sse / dof))
}

/// Gaussian elimination with partial pivoting. `None` when singular.
// Indexed loops on purpose: elimination reads the pivot row while writing
// the rows below it.
#[allow(clippy::needless_range_loop)]
pub(crate) fn solve(mut a: Vec<Vec<f64>>, mut b: Vec<f64>) -> Option<Vec<f64>> {
    let n = b.len();
    for col in 0..n {
        let pivot = (col..n).max_by(|x, y| a[*x][col].abs().total_cmp(&a[*y][col].abs()))?;
        let scale: f64 = a[col..].iter().map(|r| r[col].abs()).fold(0.0, f64::max);
        if a[pivot][col].abs() <= 1e-12 * scale.max(1.0) {
            return None;
        }
        a.swap(col, pivot);
        b.swap(col, pivot);
        for r in col + 1..n {
            let factor = a[r][col] / a[col][col];
            for c in col..n {
                a[r][c] -= factor * a[col][c];
            }
            b[r] -= factor * b[col];
        }
    }
    let mut x = vec![0.0; n];
    for r in (0..n).rev() {
        let mut s = b[r];
        for c in r + 1..n {
            s -= a[r][c] * x[c];
        }
        x[r] = s / a[r][r];
    }
    x.iter().all(|v| v.is_finite()).then_some(x)
}

/// Which way the goal wants its objective variable to move.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    Increase,
    Decrease,
}

/// The variable the goal is measured on, and which way is better.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Objective {
    pub variable: String,
    pub direction: Direction,
}

/// One scenario branch: the exogenous terms that make it this world.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Scenario {
    pub name: String,
    pub exogenous: BTreeMap<String, f64>,
}

/// One candidate under one model in one scenario. Simulated, never observed:
/// nothing here is an outcome.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SimulationResult {
    pub model: String,
    pub scenario: String,
    /// The objective in this branch had nothing been done.
    pub no_action: f64,
    /// The objective in the same branch under the candidate's interventions.
    pub with_action: f64,
    /// `with_action - no_action`, signed so that positive is toward the
    /// goal.
    pub effect: f64,
}

/// R12-AGENCY-6088: one result per (model, scenario) for a candidate whose
/// interventions shift each variable in `shifts` by the given amount from
/// where that branch would have put it.
///
/// Each result is a counterfactual against its own branch: the no-action
/// world is the model evaluated on the scenario's exogenous terms, and the
/// action world is the same terms under `do(x := x_no_action + shift)`. A
/// shift is used rather than an absolute level because a tool moves a
/// variable from wherever it is; a level would claim the same end state in
/// every branch.
///
/// Refused: no model or no scenario (a candidate judged against no world has
/// not been judged), duplicate names (two results nobody could tell apart),
/// and a model lacking the objective or a shifted variable (its silence
/// would read as "no effect").
pub fn simulate(
    models: &[StructuralModel],
    scenarios: &[Scenario],
    objective: &Objective,
    shifts: &BTreeMap<String, f64>,
) -> Result<Vec<SimulationResult>, Error> {
    if models.is_empty() || scenarios.is_empty() {
        return Err(Error::invalid(
            "a candidate is simulated against at least one model and one scenario branch",
        ));
    }
    let names: BTreeSet<&str> = models.iter().map(StructuralModel::name).collect();
    if names.len() != models.len() {
        return Err(Error::invalid("two competing models share a name"));
    }
    let branches: BTreeSet<&str> = scenarios.iter().map(|s| s.name.as_str()).collect();
    if branches.len() != scenarios.len() || branches.iter().any(|b| blank(b)) {
        return Err(Error::invalid(
            "every scenario branch needs a distinct, non-blank name",
        ));
    }
    let sign = match objective.direction {
        Direction::Increase => 1.0,
        Direction::Decrease => -1.0,
    };
    let mut results = Vec::with_capacity(models.len() * scenarios.len());
    for model in models {
        if !model.contains(&objective.variable) {
            return Err(Error::invalid(format!(
                "model `{}` does not contain the objective `{}`",
                model.name(),
                objective.variable
            )));
        }
        if let Some(missing) = shifts.keys().find(|v| !model.contains(v)) {
            return Err(Error::invalid(format!(
                "model `{}` does not contain `{missing}`, which the candidate moves",
                model.name()
            )));
        }
        for scenario in scenarios {
            let factual = model.evaluate(&scenario.exogenous);
            let mut set = BTreeMap::new();
            for (variable, shift) in shifts {
                let base = factual.get(variable).copied().unwrap_or(0.0);
                set.insert(variable.clone(), base + finite("a shift", *shift)?);
            }
            let acted = model.intervene(&set)?.evaluate(&scenario.exogenous);
            let no_action = factual.get(&objective.variable).copied().unwrap_or(0.0);
            let with_action = acted.get(&objective.variable).copied().unwrap_or(0.0);
            results.push(SimulationResult {
                model: model.name().to_string(),
                scenario: scenario.name.clone(),
                no_action,
                with_action,
                effect: sign * (with_action - no_action),
            });
        }
    }
    Ok(results)
}

//! Turning a teacher into something the hot path is allowed to evaluate.
//!
//! [`qip_strategy::model::DistilledModel`] is the only learned function the
//! execution path will run: it carries its own coefficients, loads nothing,
//! calls nothing, and its worst-case cost is a property of the value. Nothing
//! in the platform produced one until this module.
//!
//! Distillation here is the real thing rather than a conversion. The student
//! is fitted to the *teacher's outputs* on a probe set — soft targets, not the
//! observed labels — which is what lets a linear student learn the average
//! behaviour of a sixty-stump ensemble rather than re-learning the original
//! problem badly. Then the gap between the two is measured, on the same probe
//! set, on four axes at once: how far apart the outputs are, how much of the
//! teacher's variation the student reproduces, whether they rank the inputs
//! the same way, and — the one that decides trades — how often they fall on
//! the same side of the decision threshold.
//!
//! ## The structural part
//!
//! **A distilled model whose fidelity is unmeasured must not be promotable.**
//! That is enforced by construction, in two steps:
//!
//! * [`Distillation`] has private fields and exactly one constructor,
//!   [`distil`], which measures fidelity before it returns. There is no value
//!   of this type carrying an unmeasured student.
//! * The only accessor that hands out a student *for use* is
//!   [`Distillation::approved_student`], which re-checks the measurement
//!   against a [`FidelityPolicy`] on every call and refuses with the
//!   shortfalls named. [`Distillation::student`] exists for inspection and
//!   serialisation and is not the same door.
//!
//! What that does **not** do, stated plainly. `DistilledModel` lives in
//! `qip-strategy` and anybody may construct one there and hand it to the
//! strategy compiler directly; this crate cannot prevent that and does not
//! claim to. Nor does this crate promote anything — it holds no registry and
//! writes nothing. What it enforces is that a student which came out of *this*
//! pipeline arrives with its distance from its teacher already measured, so
//! the gate that does decide promotion has a number to gate on instead of an
//! assurance.
//!
//! The same door is shut on the wire. [`Distillation`] serialises, so the
//! measurement is a record that outlives the process that took it, and it does
//! not deserialise — a value of this type is evidence that `distil` ran, and
//! parsing one out of a file would be evidence of nothing:
//!
//! ```compile_fail
//! fn only_if_deserializable<T: serde::de::DeserializeOwned>() {}
//! only_if_deserializable::<qip_training::distill::Distillation>();
//! ```
//!
//! The report inside it is ordinary data and round-trips freely:
//!
//! ```
//! fn only_if_deserializable<T: serde::de::DeserializeOwned>() {}
//! only_if_deserializable::<qip_training::distill::FidelityReport>();
//! ```

use crate::dataset::TrainingDataset;
use crate::local::TrainedTeacher;
use qip_core::error::{Error, Result};
use qip_numerics::Matrix;
use qip_numerics::stats;
use qip_strategy::model::{DistilledModel, TreeNode};
use serde::{Deserialize, Serialize};

/// The shape the student takes.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StudentForm {
    /// A linear model. One pass over the coefficients, and the cheapest thing
    /// the hot path can evaluate.
    Linear { ridge: f64 },
    /// A shallow decision tree, fitted by greedy variance reduction. Keeps a
    /// non-linearity a linear student would flatten, at the cost of a
    /// branch per level.
    Tree {
        max_depth: usize,
        min_samples_leaf: usize,
        candidate_splits: usize,
    },
}

impl StudentForm {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Linear { .. } => "linear",
            Self::Tree { .. } => "tree",
        }
    }

    /// A reasonable shallow tree, so a caller does not have to invent three
    /// numbers.
    pub const fn shallow_tree() -> Self {
        Self::Tree {
            max_depth: 3,
            min_samples_leaf: 20,
            candidate_splits: 24,
        }
    }
}

/// How far the student is from its teacher.
///
/// Four numbers rather than one, because they fail in different ways. A
/// student can track the teacher's level closely and rank inputs differently;
/// it can rank them identically and sit on the wrong side of the threshold
/// every time; and the mean gap can look tiny while the worst case is where
/// all the money is.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct FidelityReport {
    pub probe_samples: usize,
    /// Mean absolute difference between teacher and student outputs.
    pub mean_absolute_gap: f64,
    /// The worst single difference.
    pub maximum_absolute_gap: f64,
    /// Fraction of the teacher's variation the student reproduces. One is a
    /// perfect copy; zero is the teacher's mean; negative is worse than that.
    pub agreement_r2: f64,
    /// Spearman correlation between the two outputs. What a ranking strategy
    /// actually consumes.
    pub rank_correlation: f64,
    /// Fraction of probe rows where teacher and student fall on the same side
    /// of [`Self::decision_threshold`]. The number that decides trades.
    pub decision_agreement: f64,
    pub decision_threshold: f64,
    /// The student's worst-case evaluation steps, charged against the
    /// strategy's latency budget.
    pub student_cost: usize,
}

impl FidelityReport {
    pub fn summarise(&self) -> String {
        format!(
            "over {} probe row(s) the student reproduces {:.4} of the teacher's variation, ranks \
             at {:.4}, agrees with it on {:.2}% of decisions at a threshold of {:.4}, and differs \
             by {:.5} on average and {:.5} at worst; it costs {} step(s)",
            self.probe_samples,
            self.agreement_r2,
            self.rank_correlation,
            self.decision_agreement * 100.0,
            self.decision_threshold,
            self.mean_absolute_gap,
            self.maximum_absolute_gap,
            self.student_cost
        )
    }
}

/// How close a student has to be before it may stand in for its teacher.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FidelityPolicy {
    pub minimum_agreement_r2: f64,
    pub minimum_rank_correlation: f64,
    pub minimum_decision_agreement: f64,
    pub maximum_mean_gap: f64,
    /// Worst-case evaluation steps the hot path will accept.
    pub maximum_student_cost: usize,
    pub minimum_probe_samples: usize,
}

impl Default for FidelityPolicy {
    fn default() -> Self {
        // Decision agreement is the strictest of the four on purpose. A
        // student that reproduces 90% of the teacher's variance but disagrees
        // with it on one decision in ten is a different strategy, and the
        // backtest that justified the teacher describes trades that would not
        // happen.
        Self {
            minimum_agreement_r2: 0.80,
            minimum_rank_correlation: 0.85,
            minimum_decision_agreement: 0.95,
            maximum_mean_gap: f64::INFINITY,
            maximum_student_cost: 64,
            minimum_probe_samples: 50,
        }
    }
}

impl FidelityPolicy {
    /// Every way this report falls short, phrased so an operator can act.
    pub fn shortfalls(&self, report: &FidelityReport) -> Vec<String> {
        let mut out = Vec::new();
        if report.probe_samples < self.minimum_probe_samples {
            out.push(format!(
                "the fidelity gap was measured on {} row(s), below the {} needed for the \
                 measurement to mean anything",
                report.probe_samples, self.minimum_probe_samples
            ));
        }
        if report.agreement_r2 < self.minimum_agreement_r2 {
            out.push(format!(
                "the student reproduces {:.4} of the teacher's variation, below the {:.4} bar",
                report.agreement_r2, self.minimum_agreement_r2
            ));
        }
        if report.rank_correlation < self.minimum_rank_correlation {
            out.push(format!(
                "rank correlation {:.4} is below the {:.4} bar",
                report.rank_correlation, self.minimum_rank_correlation
            ));
        }
        if report.decision_agreement < self.minimum_decision_agreement {
            out.push(format!(
                "teacher and student disagree on {:.2}% of decisions, above the {:.2}% tolerated",
                (1.0 - report.decision_agreement) * 100.0,
                (1.0 - self.minimum_decision_agreement) * 100.0
            ));
        }
        if report.mean_absolute_gap > self.maximum_mean_gap {
            out.push(format!(
                "the mean gap of {:.5} exceeds the {:.5} allowed",
                report.mean_absolute_gap, self.maximum_mean_gap
            ));
        }
        if report.student_cost > self.maximum_student_cost {
            out.push(format!(
                "the student costs {} step(s), above the {} the latency budget allows",
                report.student_cost, self.maximum_student_cost
            ));
        }
        out
    }
}

/// A student, its teacher, and the measured gap between them.
///
/// Private fields and one constructor — see the module note. A `Distillation`
/// that exists has had its fidelity measured, so "unmeasured fidelity" is not
/// a state the promotion path has to check for; it is a state that does not
/// occur.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Distillation {
    student: DistilledModel,
    teacher_reference: String,
    teacher_dataset: String,
    probe_dataset: String,
    form: StudentForm,
    fidelity: FidelityReport,
    /// Whether the teacher itself still carries a measured fit. A teacher
    /// whose structure was replaced and never refitted has none, and a
    /// faithful copy of an unevaluated model is an unevaluated model.
    teacher_was_evaluated: bool,
}

impl Distillation {
    pub fn student(&self) -> &DistilledModel {
        &self.student
    }

    pub fn teacher_reference(&self) -> &str {
        &self.teacher_reference
    }

    pub fn teacher_dataset(&self) -> &str {
        &self.teacher_dataset
    }

    pub fn probe_dataset(&self) -> &str {
        &self.probe_dataset
    }

    pub fn form(&self) -> StudentForm {
        self.form
    }

    /// The measured gap. Always present.
    pub fn fidelity(&self) -> &FidelityReport {
        &self.fidelity
    }

    pub fn teacher_was_evaluated(&self) -> bool {
        self.teacher_was_evaluated
    }

    /// The student, if the gap is small enough to let it stand in.
    ///
    /// The only accessor that hands out a model for use. A caller can still
    /// read [`Self::student`] to inspect or serialise one, but the promotion
    /// path calls this, and this refuses.
    pub fn approved_student(&self, policy: &FidelityPolicy) -> Result<&DistilledModel> {
        if !self.teacher_was_evaluated {
            return Err(Error::denied(format!(
                "{} was distilled from a teacher with no measured fit — its structure was \
                 replaced and never refitted — so a faithful copy of it is a faithful copy of \
                 something nobody has evaluated",
                self.teacher_reference
            )));
        }
        let shortfalls = policy.shortfalls(&self.fidelity);
        if !shortfalls.is_empty() {
            return Err(Error::denied(format!(
                "the distillate of {} may not stand in for it: {}",
                self.teacher_reference,
                shortfalls.join("; ")
            )));
        }
        Ok(&self.student)
    }

    pub fn is_promotable(&self, policy: &FidelityPolicy) -> bool {
        self.approved_student(policy).is_ok()
    }
}

/// Fit a student to a teacher's outputs and measure the gap.
///
/// The `decision_threshold` is the level a strategy would compare the model's
/// output against. It is a parameter rather than an assumption because a
/// student's decision agreement is only meaningful at the threshold it will
/// actually be used at.
pub fn distil(
    teacher: &TrainedTeacher,
    probe: &TrainingDataset,
    form: StudentForm,
    decision_threshold: f64,
) -> Result<Distillation> {
    if probe.arity() != teacher.arity() {
        return Err(Error::invalid(format!(
            "{} reads {} feature(s) and the probe set carries {}",
            teacher.reference(),
            teacher.arity(),
            probe.arity()
        )));
    }
    if !decision_threshold.is_finite() {
        return Err(Error::invalid("a decision threshold must be finite"));
    }

    // Soft targets: the teacher's own outputs, not the observed labels. This
    // is what distillation is; fitting the student to the labels again would
    // just be training a second, worse model.
    let soft_targets = teacher.predict_all(probe)?;
    let relabelled = probe.with_targets(soft_targets.clone())?;

    let name = format!("{}#{}", teacher.reference(), form.as_str());
    let student = match form {
        StudentForm::Linear { ridge } => fit_linear_student(&name, &relabelled, ridge)?,
        StudentForm::Tree {
            max_depth,
            min_samples_leaf,
            candidate_splits,
        } => fit_tree_student(
            &name,
            &relabelled,
            max_depth,
            min_samples_leaf,
            candidate_splits,
        )?,
    };

    let student_outputs: Vec<f64> = probe
        .rows()
        .iter()
        .map(|row| student.evaluate(row))
        .collect::<Result<Vec<f64>>>()?;

    let fidelity = measure_fidelity(
        &soft_targets,
        &student_outputs,
        decision_threshold,
        student.cost(),
    );

    Ok(Distillation {
        student,
        teacher_reference: teacher.reference(),
        teacher_dataset: teacher.dataset().to_string(),
        probe_dataset: probe.name().to_string(),
        form,
        fidelity,
        teacher_was_evaluated: teacher.fit().is_some(),
    })
}

/// The gap, on the four axes the report carries.
fn measure_fidelity(
    teacher: &[f64],
    student: &[f64],
    decision_threshold: f64,
    student_cost: usize,
) -> FidelityReport {
    let samples = teacher.len().min(student.len());
    let mut absolute_total = 0.0;
    let mut maximum = 0.0_f64;
    let mut agreements = 0usize;
    for (t, s) in teacher.iter().zip(student).take(samples) {
        let gap = (t - s).abs();
        absolute_total += gap;
        maximum = maximum.max(gap);
        if (*t >= decision_threshold) == (*s >= decision_threshold) {
            agreements += 1;
        }
    }

    let teacher_mean = stats::mean(&teacher[..samples]);
    let mut residual = 0.0;
    let mut total = 0.0;
    for (t, s) in teacher.iter().zip(student).take(samples) {
        residual += (t - s) * (t - s);
        total += (t - teacher_mean) * (t - teacher_mean);
    }
    // A teacher whose output never varies is reproduced perfectly by any
    // constant, and reporting a ratio against zero variance would be a
    // division by nothing rather than a fidelity.
    let agreement_r2 = if total > 1e-18 {
        1.0 - residual / total
    } else {
        f64::from(u8::from(residual <= 1e-18))
    };

    FidelityReport {
        probe_samples: samples,
        mean_absolute_gap: if samples == 0 {
            0.0
        } else {
            absolute_total / samples as f64
        },
        maximum_absolute_gap: maximum,
        agreement_r2,
        rank_correlation: stats::rank_correlation(&teacher[..samples], &student[..samples]),
        decision_agreement: if samples == 0 {
            0.0
        } else {
            agreements as f64 / samples as f64
        },
        decision_threshold,
        student_cost,
    }
}

/// Least squares against the soft targets.
fn fit_linear_student(name: &str, data: &TrainingDataset, ridge: f64) -> Result<DistilledModel> {
    if ridge < 0.0 {
        return Err(Error::invalid("a ridge penalty cannot be negative"));
    }
    let n = data.len();
    let k = data.arity();
    if n <= k + 1 {
        return Err(Error::invalid(format!(
            "{n} probe row(s) cannot determine {k} coefficient(s) and an intercept"
        )));
    }
    let mut design = Matrix::zeros(n, k + 1);
    for (row_index, row) in data.rows().iter().enumerate() {
        design.set(row_index, 0, 1.0);
        for (column, value) in row.iter().enumerate() {
            design.set(row_index, column + 1, *value);
        }
    }
    let transpose = design.transpose();
    let mut normal = transpose.matmul(&design)?;
    for index in 1..=k {
        normal.add_to(index, index, ridge);
    }
    let rhs = transpose.mul_vec(data.targets())?;
    let beta = normal.solve(&rhs)?;
    DistilledModel::linear(name, beta[0], beta[1..].to_vec())
}

/// A shallow regression tree over the soft targets, laid out so every branch
/// points at a strictly later node.
///
/// That layout is not an implementation detail: it is the invariant
/// [`DistilledModel::tree`] checks, and it is what gives the model a
/// worst-case cost to charge against a latency budget.
fn fit_tree_student(
    name: &str,
    data: &TrainingDataset,
    max_depth: usize,
    min_samples_leaf: usize,
    candidate_splits: usize,
) -> Result<DistilledModel> {
    if max_depth == 0 {
        return Err(Error::invalid("a distilled tree needs at least one level"));
    }
    // Zero is not "no minimum" and refusing it is the point: a caller that
    // passed zero either meant one and should say so, or built the value from
    // something upstream that computed zero by mistake — clamping either case
    // to a working default would distil a student from a configuration
    // nobody asked for.
    if min_samples_leaf == 0 {
        return Err(Error::invalid(
            "a minimum leaf size of zero admits a split that isolates a single row into its own \
             leaf; every split this crate fits requires at least one observation on each side",
        ));
    }
    if candidate_splits == 0 {
        return Err(Error::invalid(
            "a tree with zero candidate splits considers no threshold at all, so it can never \
             split; that is a caller error, not a configuration to fall back from",
        ));
    }
    let indices: Vec<usize> = (0..data.len()).collect();
    let mut nodes = Vec::new();
    grow(
        data,
        &indices,
        0,
        max_depth,
        min_samples_leaf,
        candidate_splits,
        &mut nodes,
    );
    DistilledModel::tree(name, data.arity(), nodes)
}

/// Grow one subtree, returning the index of its root.
///
/// Children are always pushed after the parent, so every branch target is a
/// strictly later index by construction rather than by a later check.
fn grow(
    data: &TrainingDataset,
    indices: &[usize],
    depth: usize,
    max_depth: usize,
    min_samples_leaf: usize,
    candidate_splits: usize,
    nodes: &mut Vec<TreeNode>,
) -> usize {
    let index = nodes.len();
    let mean = mean_of(data, indices);
    nodes.push(TreeNode::Leaf { value: mean });

    if depth >= max_depth || indices.len() < 2 * min_samples_leaf {
        return index;
    }
    let Some((feature, threshold)) = best_split(data, indices, min_samples_leaf, candidate_splits)
    else {
        return index;
    };

    let (below, at_or_above): (Vec<usize>, Vec<usize>) = indices
        .iter()
        .partition(|row| data.rows()[**row][feature] < threshold);
    if below.len() < min_samples_leaf || at_or_above.len() < min_samples_leaf {
        return index;
    }

    let below_index = grow(
        data,
        &below,
        depth + 1,
        max_depth,
        min_samples_leaf,
        candidate_splits,
        nodes,
    );
    let above_index = grow(
        data,
        &at_or_above,
        depth + 1,
        max_depth,
        min_samples_leaf,
        candidate_splits,
        nodes,
    );
    nodes[index] = TreeNode::Branch {
        input: feature,
        threshold,
        below: below_index,
        at_or_above: above_index,
    };
    index
}

fn mean_of(data: &TrainingDataset, indices: &[usize]) -> f64 {
    if indices.is_empty() {
        return 0.0;
    }
    let total: f64 = indices.iter().map(|row| data.targets()[*row]).sum();
    total / indices.len() as f64
}

/// The split that removes the most variance, or `None` when none does.
fn best_split(
    data: &TrainingDataset,
    indices: &[usize],
    min_samples_leaf: usize,
    candidate_splits: usize,
) -> Option<(usize, f64)> {
    let total: f64 = indices.iter().map(|row| data.targets()[*row]).sum();
    let count = indices.len() as f64;
    let mut best: Option<(f64, usize, f64)> = None;

    for feature in 0..data.arity() {
        let mut values: Vec<f64> = indices
            .iter()
            .map(|row| data.rows()[*row][feature])
            .collect();
        values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        values.dedup();
        if values.len() < 2 {
            continue;
        }
        let steps = candidate_splits.min(values.len() - 1).max(1);
        for step in 1..=steps {
            let position = (values.len() * step) / (steps + 1);
            let threshold = values[position.clamp(1, values.len() - 1)];

            let mut below_sum = 0.0;
            let mut below_count = 0usize;
            for row in indices {
                if data.rows()[*row][feature] < threshold {
                    below_sum += data.targets()[*row];
                    below_count += 1;
                }
            }
            let above_count = indices.len() - below_count;
            if below_count < min_samples_leaf || above_count < min_samples_leaf {
                continue;
            }
            let above_sum = total - below_sum;
            let gain = below_sum * below_sum / below_count as f64
                + above_sum * above_sum / above_count as f64
                - total * total / count;
            if gain > 1e-12 && best.as_ref().is_none_or(|(current, _, _)| gain > *current) {
                best = Some((gain, feature, threshold));
            }
        }
    }
    best.map(|(_, feature, threshold)| (feature, threshold))
}

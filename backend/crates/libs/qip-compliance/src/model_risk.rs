//! Control 3 — model risk and explainability.
//!
//! `qip_ai::registry` already answers "is this model fit to be used": staged,
//! evaluated, not drifted, not retired. That is model *performance*. This
//! module adds the other half, model *risk*: what the model is for, what it is
//! known to be bad at, who validated it and when a human must look again.
//! [`ModelRiskRegister::admit`] composes the two, so a model can be perfectly
//! well-evaluated and still be refused because nobody has reviewed it this
//! year.
//!
//! The explainability half is a reconciliation, not a narrative. An
//! [`Explanation`] whose contributions do not add up to the output it claims
//! to explain cannot be constructed — the constructor computes the residual
//! in exact [`Decimal`] arithmetic and refuses anything non-zero. An
//! explanation that does not add up is worse than none, because it invites a
//! reviewer to sign off on arithmetic that was never done.
//!
//! [`AdmittedOutput`] is the only type carrying a model number that this crate
//! will hand out, and the only way to obtain one is [`ModelRiskRegister::admit`].
//! A decision written against `AdmittedOutput` therefore cannot be fed a raw
//! model output at all.

use qip_ai::registry::ModelRegistry;
use qip_core::error::{Error, Result};
use qip_core::{Decimal, HypothesisId, Timestamp};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// What kind of validation was performed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValidationKind {
    /// Measured on data held out of its own fitting.
    HoldOut,
    /// Replayed over history with point-in-time inputs.
    Backtest,
    /// Compared against a simpler model that must be beaten to justify it.
    Benchmark,
    /// Behaviour under conditions outside the training distribution.
    StressTest,
    /// Reviewed by somebody who did not build it.
    IndependentReview,
}

impl ValidationKind {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::HoldOut => "hold_out",
            Self::Backtest => "backtest",
            Self::Benchmark => "benchmark",
            Self::StressTest => "stress_test",
            Self::IndependentReview => "independent_review",
        }
    }
}

/// One piece of evidence that a model was validated.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ValidationEvidence {
    pub kind: ValidationKind,
    /// The dataset the validation ran against.
    pub dataset: String,
    /// Who performed it. For [`ValidationKind::IndependentReview`] this is the
    /// name the reviewer is accountable under.
    pub performed_by: String,
    pub at: Timestamp,
    pub summary: String,
}

impl ValidationEvidence {
    pub fn new(
        kind: ValidationKind,
        dataset: impl Into<String>,
        performed_by: impl Into<String>,
        at: Timestamp,
        summary: impl Into<String>,
    ) -> Result<Self> {
        let performed_by = performed_by.into();
        if performed_by.trim().is_empty() {
            return Err(Error::invalid(
                "validation evidence must name who produced it",
            ));
        }
        Ok(Self {
            kind,
            dataset: dataset.into(),
            performed_by,
            at,
            summary: summary.into(),
        })
    }
}

/// A range of conditions a model was validated over.
///
/// The point of recording it is that a model used outside its boundary is not
/// a model with a wider error bar — it is a model nobody has measured.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PerformanceBoundary {
    /// The input or regime this bounds, matched by name against the inputs of
    /// an [`Explanation`].
    pub dimension: String,
    pub minimum: Option<Decimal>,
    pub maximum: Option<Decimal>,
}

impl PerformanceBoundary {
    pub fn new(
        dimension: impl Into<String>,
        minimum: Option<Decimal>,
        maximum: Option<Decimal>,
    ) -> Result<Self> {
        let dimension = dimension.into();
        if dimension.trim().is_empty() {
            return Err(Error::invalid(
                "a performance boundary must name a dimension",
            ));
        }
        if let (Some(lo), Some(hi)) = (minimum, maximum)
            && lo > hi
        {
            return Err(Error::invalid(format!(
                "the boundary for {dimension} has a minimum {lo} above its maximum {hi}"
            )));
        }
        if minimum.is_none() && maximum.is_none() {
            return Err(Error::invalid(format!(
                "the boundary for {dimension} bounds nothing"
            )));
        }
        Ok(Self {
            dimension,
            minimum,
            maximum,
        })
    }

    /// Whether a value falls inside the validated range.
    pub fn contains(&self, value: Decimal) -> bool {
        self.minimum.is_none_or(|lo| value >= lo) && self.maximum.is_none_or(|hi| value <= hi)
    }

    pub fn describe(&self) -> String {
        match (self.minimum, self.maximum) {
            (Some(lo), Some(hi)) => format!("{} in [{lo}, {hi}]", self.dimension),
            (Some(lo), None) => format!("{} at least {lo}", self.dimension),
            (None, Some(hi)) => format!("{} at most {hi}", self.dimension),
            (None, None) => format!("{} unbounded", self.dimension),
        }
    }
}

/// The risk file for one deployed model.
///
/// Fields are private and the constructor is the only way in, because a risk
/// file whose review date can be pushed out in place is a review that never
/// happens. Extending the review is [`ModelRiskFile::reviewed`], which demands
/// a named reviewer and fresh evidence.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ModelRiskFile {
    /// `name@version`, matching `qip_ai::registry::ModelCard::reference`, so a
    /// risk file and a model card cannot drift apart silently.
    model_reference: String,
    intended_use: String,
    /// What the model is known to be bad at. Never empty — a model with no
    /// recorded limitation has not been reviewed, it has been described.
    limitations: Vec<String>,
    validation_evidence: Vec<ValidationEvidence>,
    boundaries: Vec<PerformanceBoundary>,
    owner: String,
    approved_at: Timestamp,
    /// When a human must look at this again. Past this, the model is refused.
    review_due: Timestamp,
    /// Every review that has extended the file, so the history of who kept the
    /// model alive is not overwritten by the latest one.
    reviews: Vec<ModelReview>,
}

/// One recorded periodic review of a risk file.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ModelReview {
    pub at: Timestamp,
    pub reviewer: String,
    pub next_due: Timestamp,
    pub findings: String,
}

impl ModelRiskFile {
    /// Open a risk file, refusing one that does not carry enough to review.
    ///
    /// Each refusal below corresponds to a real failure mode: a model with no
    /// stated intended use gets applied to a different problem, one with no
    /// limitations gets trusted outside its range, and one with no validation
    /// evidence is an opinion.
    pub fn open(
        model_reference: impl Into<String>,
        intended_use: impl Into<String>,
        owner: impl Into<String>,
        approved_at: Timestamp,
        review_due: Timestamp,
        limitations: Vec<String>,
        validation_evidence: Vec<ValidationEvidence>,
        boundaries: Vec<PerformanceBoundary>,
    ) -> Result<Self> {
        let model_reference = model_reference.into();
        let intended_use = intended_use.into();
        let owner = owner.into();
        if model_reference.trim().is_empty() {
            return Err(Error::invalid("a risk file must name the model it covers"));
        }
        if intended_use.trim().len() < 10 {
            return Err(Error::invalid(format!(
                "the risk file for {model_reference} must state an intended use somebody can \
                 compare an actual use against"
            )));
        }
        if owner.trim().is_empty() {
            return Err(Error::invalid(format!(
                "the risk file for {model_reference} must name an accountable owner"
            )));
        }
        if limitations.iter().all(|l| l.trim().is_empty()) {
            return Err(Error::invalid(format!(
                "the risk file for {model_reference} records no limitation; every model has at \
                 least one, and a file without one has not been reviewed"
            )));
        }
        if validation_evidence.is_empty() {
            return Err(Error::invalid(format!(
                "the risk file for {model_reference} records no validation evidence"
            )));
        }
        if review_due <= approved_at {
            return Err(Error::invalid(format!(
                "the risk file for {model_reference} is due for review at {review_due}, \
                 at or before it was approved at {approved_at}"
            )));
        }
        Ok(Self {
            model_reference,
            intended_use,
            limitations,
            validation_evidence,
            boundaries,
            owner,
            approved_at,
            review_due,
            reviews: Vec::new(),
        })
    }

    pub fn model_reference(&self) -> &str {
        &self.model_reference
    }

    pub fn intended_use(&self) -> &str {
        &self.intended_use
    }

    pub fn limitations(&self) -> &[String] {
        &self.limitations
    }

    pub fn validation_evidence(&self) -> &[ValidationEvidence] {
        &self.validation_evidence
    }

    pub fn boundaries(&self) -> &[PerformanceBoundary] {
        &self.boundaries
    }

    pub fn owner(&self) -> &str {
        &self.owner
    }

    pub fn approved_at(&self) -> Timestamp {
        self.approved_at
    }

    pub fn review_due(&self) -> Timestamp {
        self.review_due
    }

    pub fn reviews(&self) -> &[ModelReview] {
        &self.reviews
    }

    /// Whether the periodic review is overdue at `now`.
    pub fn is_overdue(&self, now: Timestamp) -> bool {
        now >= self.review_due
    }

    /// Whether the file has independent validation — somebody who did not
    /// build the model having looked at it.
    pub fn has_independent_review(&self) -> bool {
        self.validation_evidence
            .iter()
            .any(|e| e.kind == ValidationKind::IndependentReview)
    }

    /// Record a review, moving the due date forward.
    ///
    /// The only way the due date moves. It demands a reviewer and a finding,
    /// and refuses to move the date backwards or to sit still, so "reviewed"
    /// always means something happened.
    pub fn reviewed(
        &mut self,
        reviewer: impl Into<String>,
        at: Timestamp,
        next_due: Timestamp,
        findings: impl Into<String>,
    ) -> Result<()> {
        let reviewer = reviewer.into();
        let findings = findings.into();
        if reviewer.trim().is_empty() {
            return Err(Error::denied("a model review must name its reviewer"));
        }
        if findings.trim().len() < 10 {
            return Err(Error::invalid(
                "a model review must record what was found; the record is the point",
            ));
        }
        if next_due <= at {
            return Err(Error::invalid(format!(
                "a review at {at} cannot set the next review to {next_due}"
            )));
        }
        self.review_due = next_due;
        self.reviews.push(ModelReview {
            at,
            reviewer,
            next_due,
            findings,
        });
        Ok(())
    }

    /// Boundaries the supplied conditions fall outside of.
    pub fn breached_boundaries(
        &self,
        conditions: &BTreeMap<String, Decimal>,
    ) -> Vec<&PerformanceBoundary> {
        self.boundaries
            .iter()
            .filter(|b| {
                conditions
                    .get(&b.dimension)
                    .is_some_and(|value| !b.contains(*value))
            })
            .collect()
    }
}

/// One input's share of a model output.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Contribution {
    /// The input's name, matched against [`PerformanceBoundary::dimension`].
    pub input: String,
    /// What the input was.
    pub value: Decimal,
    /// How much of the output this input accounts for, in output units.
    pub contribution: Decimal,
}

/// Why a model produced the number it produced.
///
/// Private fields with one fallible constructor: the contributions must
/// reconcile to the output exactly, in [`Decimal`] arithmetic, or the value
/// does not exist. Approximate reconciliation was considered and rejected —
/// a tolerance is a place for a systematic error to hide, and every
/// attribution method the platform uses is exact by construction with a
/// residual term where it is not.
///
/// `upstream` is one additive hop toward the blueprint's full attribution
/// chain (fill → strategy → family → mandate; intent → belief → causal edge
/// → world event → entity): the claim or hypothesis whose belief produced
/// this model's inputs, if one drove it. It is `Option` because not every
/// model output is downstream of a hypothesis — some are computed straight
/// from market data — but the constructor takes it as a required argument
/// rather than defaulting it, so a caller who never considered the question
/// cannot end up recording `None` by omission the way a `#[derive(Default)]`
/// field would let them.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Explanation {
    model_reference: String,
    output: Decimal,
    /// The value the model produces from no information — the intercept,
    /// prior, or unconditional mean. Contributions are deviations from it.
    baseline: Decimal,
    contributions: Vec<Contribution>,
    at: Timestamp,
    /// The claim/hypothesis id that produced this output's inputs, or `None`
    /// if none did. Stated explicitly at construction — see the struct doc.
    upstream: Option<HypothesisId>,
}

impl Explanation {
    /// Build an explanation, refusing one whose parts do not sum to its whole.
    ///
    /// `upstream` must be stated even when it is `None`, so a caller who has
    /// not thought about provenance cannot silently produce an explanation
    /// that looks the same as one who deliberately found none.
    pub fn reconciled(
        model_reference: impl Into<String>,
        output: Decimal,
        baseline: Decimal,
        contributions: Vec<Contribution>,
        at: Timestamp,
        upstream: Option<HypothesisId>,
    ) -> Result<Self> {
        let model_reference = model_reference.into();
        if contributions.is_empty() {
            return Err(Error::invalid(format!(
                "an explanation of {model_reference} must name at least one input that drove it"
            )));
        }
        let mut total = baseline;
        for c in &contributions {
            if c.input.trim().is_empty() {
                return Err(Error::invalid(format!(
                    "an explanation of {model_reference} has an unnamed contribution"
                )));
            }
            total = total.checked_add(c.contribution).ok_or_else(|| {
                Error::numeric(format!(
                    "the contributions to {model_reference} overflow when summed"
                ))
            })?;
        }
        if total != output {
            let residual = output - total;
            return Err(Error::invalid(format!(
                "the explanation of {model_reference} does not reconcile: baseline {baseline} \
                 plus {} contributions gives {total}, but the output is {output} \
                 (residual {residual}). Add the missing term or correct the attribution",
                contributions.len()
            )));
        }
        Ok(Self {
            model_reference,
            output,
            baseline,
            contributions,
            at,
            upstream,
        })
    }

    pub fn model_reference(&self) -> &str {
        &self.model_reference
    }

    pub fn output(&self) -> Decimal {
        self.output
    }

    pub fn baseline(&self) -> Decimal {
        self.baseline
    }

    pub fn contributions(&self) -> &[Contribution] {
        &self.contributions
    }

    pub fn at(&self) -> Timestamp {
        self.at
    }

    /// The claim or hypothesis whose belief produced this output's inputs,
    /// if the explanation names one.
    pub fn upstream(&self) -> Option<&HypothesisId> {
        self.upstream.as_ref()
    }

    /// The inputs and their values, for checking against a risk file's
    /// performance boundaries.
    pub fn conditions(&self) -> BTreeMap<String, Decimal> {
        self.contributions
            .iter()
            .map(|c| (c.input.clone(), c.value))
            .collect()
    }

    /// Contributions ordered by absolute size — what actually drove it.
    pub fn drivers(&self) -> Vec<&Contribution> {
        let mut ordered: Vec<&Contribution> = self.contributions.iter().collect();
        ordered.sort_by_key(|c| std::cmp::Reverse(c.contribution.abs().raw()));
        ordered
    }
}

/// A model output that has passed every model-risk control.
///
/// There is no public constructor. The only source is
/// [`ModelRiskRegister::admit`], so a function that takes an `AdmittedOutput`
/// cannot be handed a number that skipped the checks — not by a mistake, and
/// not by a caller in a hurry.
#[derive(Clone, Debug, PartialEq)]
pub struct AdmittedOutput {
    model_reference: String,
    output: Decimal,
    explanation: Explanation,
    admitted_at: Timestamp,
}

impl AdmittedOutput {
    pub fn model_reference(&self) -> &str {
        &self.model_reference
    }

    /// The number, now safe to act on.
    pub fn output(&self) -> Decimal {
        self.output
    }

    pub fn explanation(&self) -> &Explanation {
        &self.explanation
    }

    pub fn admitted_at(&self) -> Timestamp {
        self.admitted_at
    }
}

/// One recorded admission or refusal of a model output.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AdmissionRecord {
    pub at: Timestamp,
    pub model_reference: String,
    pub admitted: bool,
    pub refusal: String,
}

/// The risk files for every deployed model.
#[derive(Debug, Default)]
pub struct ModelRiskRegister {
    files: BTreeMap<String, ModelRiskFile>,
    admissions: Vec<AdmissionRecord>,
}

impl ModelRiskRegister {
    pub fn new() -> Self {
        Self::default()
    }

    /// File a model's risk record, replacing any earlier one.
    pub fn file(&mut self, file: ModelRiskFile) {
        self.files.insert(file.model_reference.clone(), file);
    }

    pub fn get(&self, model_reference: &str) -> Option<&ModelRiskFile> {
        self.files.get(model_reference)
    }

    pub fn get_mut(&mut self, model_reference: &str) -> Option<&mut ModelRiskFile> {
        self.files.get_mut(model_reference)
    }

    pub fn len(&self) -> usize {
        self.files.len()
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &ModelRiskFile> {
        self.files.values()
    }

    /// Risk files whose review has fallen due.
    pub fn overdue(&self, now: Timestamp) -> Vec<&ModelRiskFile> {
        self.files.values().filter(|f| f.is_overdue(now)).collect()
    }

    /// Every admission decision, admitted or refused.
    pub fn admissions(&self) -> &[AdmissionRecord] {
        &self.admissions
    }

    /// The single gate a model output passes to reach a decision.
    ///
    /// Four things must hold, and each maps to a way models have caused losses
    /// elsewhere: the model is registered and eligible (`qip_ai`'s check —
    /// staged, evaluated, undrifted), a risk file exists, its review is not
    /// overdue, and the conditions the output was produced under are inside
    /// the range the model was validated over.
    ///
    /// The explanation is required rather than optional. Its existence already
    /// proves it reconciles, so requiring it here means no output reaches a
    /// decision without an attribution somebody can inspect afterwards.
    pub fn admit(
        &mut self,
        models: &ModelRegistry,
        explanation: Explanation,
        now: Timestamp,
    ) -> Result<AdmittedOutput> {
        let reference = explanation.model_reference().to_string();
        let outcome = self.evaluate(models, &explanation, now);
        self.admissions.push(AdmissionRecord {
            at: now,
            model_reference: reference.clone(),
            admitted: outcome.is_ok(),
            refusal: outcome
                .as_ref()
                .err()
                .map(|e| e.message().to_string())
                .unwrap_or_default(),
        });
        outcome?;
        Ok(AdmittedOutput {
            model_reference: reference,
            output: explanation.output(),
            explanation,
            admitted_at: now,
        })
    }

    /// The checks behind [`ModelRiskRegister::admit`], factored out so the
    /// admission record is written whichever way they go.
    fn evaluate(
        &self,
        models: &ModelRegistry,
        explanation: &Explanation,
        now: Timestamp,
    ) -> Result<()> {
        let reference = explanation.model_reference();
        models.require_for_decision(reference, now)?;
        let file = self.files.get(reference).ok_or_else(|| {
            Error::denied(format!(
                "{reference} has no model risk file; a model with no recorded intended use, \
                 limitations and validation evidence may not drive a decision"
            ))
        })?;
        if file.is_overdue(now) {
            return Err(Error::denied(format!(
                "the risk file for {reference} was due for review at {} and it is now {now}; \
                 review it before the model is used again",
                file.review_due()
            )));
        }
        let conditions = explanation.conditions();
        let breached = file.breached_boundaries(&conditions);
        if !breached.is_empty() {
            let detail: Vec<String> = breached
                .iter()
                .map(|b| {
                    let actual = conditions
                        .get(&b.dimension)
                        .map_or_else(|| "unknown".to_string(), |v| v.to_string());
                    format!("{} (actual {actual})", b.describe())
                })
                .collect();
            return Err(Error::denied(format!(
                "{reference} was validated only within {}; the output was produced outside it",
                detail.join(", ")
            )));
        }
        Ok(())
    }
}

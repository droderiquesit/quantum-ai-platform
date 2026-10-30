//! R12-AGENCY-6102: the caused change after an intervention, estimated by an
//! identified method, and labelled with how it was identified.
//!
//! The failure prevented is crediting an action with what would have
//! happened anyway. Before this module [`crate::attribution`] *recorded* an
//! [`Identification`] — experiment, observational strategy, not identified —
//! but nothing computed an effect by any of the methods §24.6 lists, so the
//! label was whatever the author typed beside a number from elsewhere.
//!
//! Each estimator here computes its effect and its label together, and
//! [`Estimate`]'s fields are private: there is no way to hold an estimate
//! without the identification the method that produced it assigns. A plain
//! correlation is an estimator too ([`correlation`]), and the only one whose
//! label is [`Identification::NotIdentified`] — so the thing
//! [`crate::autonomy::ActionPolicy::widen`] refuses is something this module
//! can name, not only something it omits.
//!
//! [`Estimate::supports`] is the seam a composition root uses: an operator's
//! [`EffectAttribution`] is admitted as widening evidence only when an
//! estimator, run here on the data the operator supplies, produced the same
//! identification and the same effect. An attribution written by hand and
//! labelled "experiment" is a claim; one this module reproduced is evidence.
//!
//! All figures are statistics, so `f64`.

use crate::attribution::{EffectAttribution, Identification};
use crate::causal::StructuralModel;
use qip_core::{Error, sha256_hex};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Which method produced an estimate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EstimatorKind {
    /// Two groups the caller formed ([`operator_formed_comparison`]). The
    /// name is kept for the wire; the label it earns is
    /// [`Identification::NotIdentified`].
    RandomisedExperiment,
    /// A hold-out whose assignment this module drew ([`Experiment`]).
    HoldOutExperiment,
    /// A staged rollout whose waves this module drew ([`staged_experiment`]).
    StagedExperiment,
    MatchedControl,
    BayesianCausal,
    DifferenceInDifferences,
    SyntheticControl,
    InstrumentalVariable,
    StructuralModel,
    NoActionBaseline,
    Correlation,
}

/// An effect, the method that produced it, and the identification that
/// method earns. Only the functions in this module construct one.
#[derive(Debug, Clone, PartialEq)]
pub struct Estimate {
    kind: EstimatorKind,
    identification: Identification,
    effect: f64,
    standard_error: Option<f64>,
    observations: usize,
}

impl Estimate {
    pub fn kind(&self) -> EstimatorKind {
        self.kind
    }
    pub fn identification(&self) -> &Identification {
        &self.identification
    }
    pub fn effect(&self) -> f64 {
        self.effect
    }
    pub fn standard_error(&self) -> Option<f64> {
        self.standard_error
    }
    pub fn observations(&self) -> usize {
        self.observations
    }

    /// Whether `attribution` claims what this estimate shows: the same
    /// identification, and the same mean effect (to a relative 1e-9, the
    /// precision a JSON round trip keeps).
    pub fn supports(&self, attribution: &EffectAttribution) -> Result<(), Error> {
        if attribution.identification != self.identification {
            return Err(Error::denied(format!(
                "the attribution claims {:?}, but the {:?} estimator identifies {:?}; a label is \
                 earned by the method, not stated beside it",
                attribution.identification, self.kind, self.identification
            )));
        }
        let claimed = attribution.effect.mean;
        let tolerance = 1e-9 * self.effect.abs().max(1.0);
        if (claimed - self.effect).abs() > tolerance {
            return Err(Error::denied(format!(
                "the attribution claims an effect of {claimed}, but the {:?} estimator computes \
                 {} from the data supplied",
                self.kind, self.effect
            )));
        }
        Ok(())
    }
}

fn sample(what: &str, values: &[f64], at_least: usize) -> Result<(f64, f64), Error> {
    if values.len() < at_least {
        return Err(Error::invalid(format!(
            "{what} has {} observations; this estimator needs at least {at_least}",
            values.len()
        )));
    }
    if let Some(bad) = values.iter().find(|v| !v.is_finite()) {
        return Err(Error::invalid(format!(
            "{what} holds {bad}; estimators take finite observations only"
        )));
    }
    let n = values.len() as f64;
    let mean = values.iter().sum::<f64>() / n;
    let variance = if values.len() > 1 {
        values.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / (n - 1.0)
    } else {
        0.0
    };
    Ok((mean, variance))
}

/// The difference in means between treated and control, with its standard
/// error, labelled with what the caller's method earns. Private on purpose:
/// the label is the caller's claim, so it is written at each call site where
/// it is earned ([`Experiment::analyse`], from an assignment drawn here) and
/// where it is not ([`operator_formed_comparison`]), never defaulted.
fn difference_in_means(
    treated: &[f64],
    control: &[f64],
    kind: EstimatorKind,
    identification: Identification,
) -> Result<Estimate, Error> {
    let (mt, vt) = sample("the treated group", treated, 2)?;
    let (mc, vc) = sample("the control group", control, 2)?;
    Ok(Estimate {
        kind,
        identification,
        effect: mt - mc,
        standard_error: Some((vt / treated.len() as f64 + vc / control.len() as f64).sqrt()),
        observations: treated.len() + control.len(),
    })
}

/// R12-COVERAGE-7188: two groups the caller formed, compared by difference
/// in means. The estimate is labelled [`Identification::NotIdentified`].
///
/// The failure prevented is an experiment that earns its label from its
/// author. This function used to label the comparison
/// `Identification::Experiment` on the strength of the caller saying the
/// groups were randomised; an operator whose hold-out had been refused (a
/// held-out cell dropped) could resubmit the same numbers as two groups and
/// widen autonomy on a selection effect. Nothing here can see how the groups
/// were formed, so the label is the one [`correlation`] gets, and
/// `ActionPolicy::widen` refuses it. Evidence that may widen comes from
/// [`Experiment`] or [`staged_experiment`], where the assignment is a pure
/// function of a committed seed.
pub fn operator_formed_comparison(treated: &[f64], control: &[f64]) -> Result<Estimate, Error> {
    difference_in_means(
        treated,
        control,
        EstimatorKind::RandomisedExperiment,
        Identification::NotIdentified,
    )
}

/// R12-COVERAGE-7188: a randomised hold-out across units (cells, venues or
/// time windows) whose assignment is drawn here, from a seed, and not
/// chosen by whoever supplies the outcomes.
///
/// The failure prevented is an experiment that earns its label from its
/// author. [`operator_formed_comparison`] takes two groups it cannot check,
/// and so earns no label; an operator who put the units that did well in the
/// treated group would otherwise get `Identification::Experiment` for a
/// selection effect.
/// Here the split is a pure function of the seed and the unit names: units
/// are ranked by the SHA-256 of `seed`, a newline and the unit, and the
/// first `held_out` are the control. Anyone holding the seed and the unit
/// list recomputes who was held out, and the outcomes are refused unless
/// they cover exactly those units, so the analysis cannot drop an
/// inconvenient one. The seed must be committed (journaled) before any
/// outcome exists; that ordering is the caller's, and nothing here can
/// prove it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Experiment {
    treated: BTreeSet<String>,
    control: BTreeSet<String>,
}

impl Experiment {
    /// Draw the hold-out. `hold_out_bps` is the share of units held out, in
    /// basis points `1..=9999`; at least two units must land on each side,
    /// because a group of one has no variance to compare with. Refused, not
    /// rounded to fit, when they cannot.
    pub fn design(seed: &str, units: &[String], hold_out_bps: u32) -> Result<Self, Error> {
        if seed.trim().is_empty() {
            return Err(Error::invalid(
                "an experiment needs a seed committed before its outcomes exist; got a blank one",
            ));
        }
        if !(1..=9_999).contains(&hold_out_bps) {
            return Err(Error::invalid(format!(
                "hold_out_bps {hold_out_bps} is outside 1..=9999; a hold-out of none or of all \
                 is not an experiment"
            )));
        }
        let mut distinct = BTreeSet::new();
        for unit in units {
            if unit.trim().is_empty() || !distinct.insert(unit.as_str()) {
                return Err(Error::invalid(format!(
                    "experiment unit `{unit}` is blank or listed twice; units are distinct"
                )));
            }
        }
        let held_out = units.len() * hold_out_bps as usize / 10_000;
        if held_out < 2 || units.len() - held_out < 2 {
            return Err(Error::invalid(format!(
                "{} unit(s) held out {hold_out_bps} bps leaves {held_out} in control and {} \
                 treated; each side needs at least two",
                units.len(),
                units.len().saturating_sub(held_out)
            )));
        }
        let mut ranked: Vec<(String, &str)> = distinct
            .into_iter()
            .map(|unit| (sha256_hex(format!("{seed}\n{unit}").as_bytes()), unit))
            .collect();
        ranked.sort();
        let control = ranked
            .iter()
            .take(held_out)
            .map(|(_, unit)| (*unit).to_string())
            .collect();
        let treated = ranked
            .iter()
            .skip(held_out)
            .map(|(_, unit)| (*unit).to_string())
            .collect();
        Ok(Self { treated, control })
    }

    pub fn treated(&self) -> &BTreeSet<String> {
        &self.treated
    }

    pub fn control(&self) -> &BTreeSet<String> {
        &self.control
    }

    /// The effect of treatment on `outcomes`, one figure per unit. Refused
    /// unless the outcomes cover exactly the designed units: a missing one
    /// is a unit dropped after the fact, an extra one was never assigned.
    pub fn analyse(&self, outcomes: &BTreeMap<String, f64>) -> Result<Estimate, Error> {
        if let Some(unit) = outcomes
            .keys()
            .find(|u| !self.treated.contains(*u) && !self.control.contains(*u))
        {
            return Err(Error::denied(format!(
                "outcome for `{unit}`, which the design never assigned"
            )));
        }
        let group = |units: &BTreeSet<String>| -> Result<Vec<f64>, Error> {
            units
                .iter()
                .map(|unit| {
                    outcomes.get(unit).copied().ok_or_else(|| {
                        Error::denied(format!(
                            "no outcome for `{unit}`, which the design assigned; a unit is not \
                             dropped after the fact"
                        ))
                    })
                })
                .collect()
        };
        let treated = group(&self.treated)?;
        let control = group(&self.control)?;
        difference_in_means(
            &treated,
            &control,
            EstimatorKind::HoldOutExperiment,
            Identification::Experiment,
        )
    }

    /// [`Self::analyse`] at each horizon (seconds after the intervention),
    /// every figure identified by the same assignment.
    pub fn analyse_horizons(
        &self,
        horizons: &BTreeMap<u64, BTreeMap<String, f64>>,
    ) -> Result<BTreeMap<u64, Estimate>, Error> {
        if horizons.is_empty() {
            return Err(Error::invalid(
                "an experiment is analysed at one horizon or more",
            ));
        }
        horizons
            .iter()
            .map(|(horizon, outcomes)| Ok((*horizon, self.analyse(outcomes)?)))
            .collect()
    }
}

/// R12-AGENCY-6102: the stage each unit is rolled out in, drawn from `seed`
/// the way [`Experiment::design`] draws a hold-out: units ranked by the
/// SHA-256 of the seed and the unit, then cut into `stages` consecutive
/// waves of as equal a size as the unit count allows (earlier waves take
/// the remainder). Stage 0 is rolled out first.
pub fn stage_assignment(
    seed: &str,
    units: &[String],
    stages: usize,
) -> Result<BTreeMap<String, usize>, Error> {
    if seed.trim().is_empty() {
        return Err(Error::invalid(
            "a staged experiment needs a seed committed before its outcomes exist; got a blank one",
        ));
    }
    if stages < 2 {
        return Err(Error::invalid(
            "a staged rollout has at least two stages; one stage is everyone at once",
        ));
    }
    let mut distinct = BTreeSet::new();
    for unit in units {
        if unit.trim().is_empty() || !distinct.insert(unit.as_str()) {
            return Err(Error::invalid(format!(
                "staged unit `{unit}` is blank or listed twice; units are distinct"
            )));
        }
    }
    // The smallest wave is both the fewest treated in the first period and
    // the fewest not-yet-treated in the last; a group of one has no variance.
    let narrowest = units.len() / stages;
    if narrowest < 2 {
        return Err(Error::invalid(format!(
            "{} unit(s) in {stages} stages leaves a wave of {narrowest}; each period needs at \
             least two units treated and two not yet treated",
            units.len()
        )));
    }
    let mut ranked: Vec<(String, &str)> = distinct
        .into_iter()
        .map(|unit| (sha256_hex(format!("{seed}\n{unit}").as_bytes()), unit))
        .collect();
    ranked.sort();
    let (base, extra) = (units.len() / stages, units.len() % stages);
    let mut out = BTreeMap::new();
    let mut at = 0;
    for stage in 0..stages {
        let size = base + usize::from(stage < extra);
        for (_, unit) in &ranked[at..at + size] {
            out.insert((*unit).to_string(), stage);
        }
        at += size;
    }
    Ok(out)
}

/// A staged (stepped) rollout whose waves were drawn here: in period `p`
/// (`1..stages`) the units of stages `0..p` are treated and the rest are not
/// yet, so every period is a randomised comparison; the effect is the mean
/// of the periods' differences in means. `outcomes` maps each period to one
/// figure per unit and must hold exactly periods `1..stages`, each covering
/// exactly the units — a period or a unit left out is a result left out.
pub fn staged_experiment(
    seed: &str,
    units: &[String],
    stages: usize,
    outcomes: &BTreeMap<usize, BTreeMap<String, f64>>,
) -> Result<Estimate, Error> {
    let stage_of = stage_assignment(seed, units, stages)?;
    let wanted: BTreeSet<usize> = (1..stages).collect();
    let given: BTreeSet<usize> = outcomes.keys().copied().collect();
    if wanted != given {
        return Err(Error::denied(format!(
            "a {stages}-stage rollout is measured in periods {wanted:?}; outcomes were given \
             for {given:?}"
        )));
    }
    let mut differences = Vec::with_capacity(outcomes.len());
    let mut observations = 0;
    for (period, per_unit) in outcomes {
        if let Some(unit) = per_unit.keys().find(|u| !stage_of.contains_key(*u)) {
            return Err(Error::denied(format!(
                "period {period} holds an outcome for `{unit}`, which the design never assigned"
            )));
        }
        let (mut treated, mut waiting) = (Vec::new(), Vec::new());
        for (unit, stage) in &stage_of {
            let value = per_unit.get(unit).copied().ok_or_else(|| {
                Error::denied(format!(
                    "period {period} has no outcome for `{unit}`; a unit is not dropped after \
                     the fact"
                ))
            })?;
            if *stage < *period {
                treated.push(value);
            } else {
                waiting.push(value);
            }
        }
        let (mt, vt) = sample("the treated units", &treated, 2)?;
        let (mw, vw) = sample("the not-yet-treated units", &waiting, 2)?;
        differences.push((
            mt - mw,
            vt / treated.len() as f64 + vw / waiting.len() as f64,
        ));
        observations += treated.len() + waiting.len();
    }
    let periods = differences.len() as f64;
    Ok(Estimate {
        kind: EstimatorKind::StagedExperiment,
        identification: Identification::Experiment,
        effect: differences.iter().map(|(d, _)| d).sum::<f64>() / periods,
        // Periods share units, so their errors are not independent; summing
        // the variances as if they were understates nothing the caller may
        // rely on, and a tighter figure would need the covariance.
        standard_error: Some(differences.iter().map(|(_, v)| v).sum::<f64>().sqrt() / periods),
        observations,
    })
}

/// Nearest-neighbour matching on observed covariates: each treated unit is
/// paired, without replacement and in the order given, with the closest
/// unused unit of the pool (Euclidean distance on the covariates as given),
/// and the effect is the mean treated-minus-matched outcome.
///
/// Identified only conditionally on the covariates: it assumes nothing
/// unmeasured moves both treatment and outcome, which the label says. A pair
/// farther apart than `caliper` is refused, not dropped — dropping the
/// treated units with no comparable control would estimate the effect on the
/// easy cases and call it the effect.
pub fn matched_control(
    treated_covariates: &[Vec<f64>],
    treated_outcomes: &[f64],
    pool_covariates: &[Vec<f64>],
    pool_outcomes: &[f64],
    caliper: f64,
) -> Result<Estimate, Error> {
    if !caliper.is_finite() || caliper <= 0.0 {
        return Err(Error::invalid(
            "the caliper is the farthest apart a matched pair may be; it must be finite and positive",
        ));
    }
    if treated_covariates.len() != treated_outcomes.len()
        || pool_covariates.len() != pool_outcomes.len()
    {
        return Err(Error::invalid(
            "every unit needs one outcome beside its covariates",
        ));
    }
    sample("the treated outcomes", treated_outcomes, 2)?;
    if pool_outcomes.len() < treated_outcomes.len() {
        return Err(Error::invalid(format!(
            "{} candidate controls cannot match {} treated units without replacement",
            pool_outcomes.len(),
            treated_outcomes.len()
        )));
    }
    sample("the pool outcomes", pool_outcomes, 1)?;
    let dims = treated_covariates.first().map_or(0, Vec::len);
    if dims == 0 {
        return Err(Error::invalid(
            "matching needs at least one observed covariate; with none every unit matches every other",
        ));
    }
    for row in treated_covariates.iter().chain(pool_covariates) {
        if row.len() != dims || row.iter().any(|v| !v.is_finite()) {
            return Err(Error::invalid(format!(
                "every unit needs {dims} finite covariate(s)"
            )));
        }
    }
    let mut used = vec![false; pool_covariates.len()];
    let mut gaps = Vec::with_capacity(treated_covariates.len());
    for (i, row) in treated_covariates.iter().enumerate() {
        let best = pool_covariates
            .iter()
            .enumerate()
            .filter(|(j, _)| !used[*j])
            .map(|(j, other)| {
                let distance = row
                    .iter()
                    .zip(other)
                    .map(|(a, b)| (a - b).powi(2))
                    .sum::<f64>()
                    .sqrt();
                (distance, j)
            })
            .min_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
        match best {
            Some((distance, j)) if distance <= caliper => {
                used[j] = true;
                gaps.push(treated_outcomes[i] - pool_outcomes[j]);
            }
            _ => {
                return Err(Error::denied(format!(
                    "treated unit {i} has no unused control within {caliper} on its covariates; \
                     the effect is not estimated over the units that happen to have a match"
                )));
            }
        }
    }
    let (effect, variance) = sample("the matched gaps", &gaps, 2)?;
    Ok(Estimate {
        kind: EstimatorKind::MatchedControl,
        identification: Identification::Observational {
            strategy: "nearest-neighbour matching on observed covariates within a caliper, \
                       assuming no unmeasured confounder"
                .into(),
        },
        effect,
        standard_error: Some((variance / gaps.len() as f64).sqrt()),
        observations: treated_outcomes.len() + gaps.len(),
    })
}

/// R12-MODEL-4013: an effect estimated by back-door adjustment, or an
/// abstention when the design does not identify it.
///
/// `covariates` names the adjustment set: each name a variable of `model`,
/// each column observed on the same units as `treatment` and `outcome`. The
/// design is checked against the model's graph first
/// ([`StructuralModel::backdoor_open_path`]); a set that leaves a back-door
/// path open, or adjusts for a descendant of the treatment, is refused with
/// the reason and no number, because an effect estimated from it is the
/// association with a causal label. Identified, the effect is
/// [`bayesian_causal`]'s on those columns, labelled with the adjustment set
/// and the model it was checked against rather than "assuming no
/// unmeasured confounder".
pub fn backdoor_adjusted(
    model: &StructuralModel,
    treatment_variable: &str,
    outcome_variable: &str,
    covariates: &BTreeMap<String, Vec<f64>>,
    treatment: &[f64],
    outcome: &[f64],
    prior_sd: f64,
) -> Result<Estimate, Error> {
    let adjustment: BTreeSet<String> = covariates.keys().cloned().collect();
    if let Some(open) =
        model.backdoor_open_path(treatment_variable, outcome_variable, &adjustment)?
    {
        return Err(Error::denied(format!(
            "abstains: the effect of `{treatment_variable}` on `{outcome_variable}` is not \
             identified by this design: {open}"
        )));
    }
    let n = outcome.len();
    if covariates.values().any(|column| column.len() != n) {
        return Err(Error::invalid(
            "every covariate column must be observed on the same units as the outcome",
        ));
    }
    let rows: Vec<Vec<f64>> = (0..n)
        .map(|i| covariates.values().map(|column| column[i]).collect())
        .collect();
    let estimate = bayesian_causal(treatment, &rows, outcome, prior_sd)?;
    let set = adjustment.iter().cloned().collect::<Vec<_>>().join(", ");
    Ok(Estimate {
        identification: Identification::Observational {
            strategy: format!(
                "back-door adjustment for {{{set}}}, checked against the graph of the declared \
                 structural model"
            ),
        },
        ..estimate
    })
}

/// A Bayesian linear causal model, `y = a + tau d + beta x + noise`, with an
/// independent `Normal(0, prior_sd^2)` prior on `tau` and on every `beta`
/// and a flat prior on the intercept. The effect is the posterior mean of
/// `tau` and its standard error the posterior standard deviation, with the
/// noise variance set to the least-squares residual variance.
///
/// The prior is stated by the caller and does the shrinking: a narrow one
/// pulls the effect toward zero however the data lean. Identified only
/// conditionally on `covariates` capturing every confounder, which the label
/// says; the Bayesian arithmetic adds uncertainty, not identification.
pub fn bayesian_causal(
    treatment: &[f64],
    covariates: &[Vec<f64>],
    outcome: &[f64],
    prior_sd: f64,
) -> Result<Estimate, Error> {
    if !prior_sd.is_finite() || prior_sd <= 0.0 {
        return Err(Error::invalid(
            "prior_sd is the width of the prior on the effect; it must be finite and positive",
        ));
    }
    let n = outcome.len();
    if treatment.len() != n || covariates.len() != n {
        return Err(Error::invalid(
            "treatment, covariates and outcome must be observed on the same units",
        ));
    }
    sample("the outcome", outcome, 5)?;
    sample("the treatment indicator", treatment, 5)?;
    // Exact bit patterns: an indicator is 0 or 1, not something near them.
    let (zero, one) = (0.0f64.to_bits(), 1.0f64.to_bits());
    if treatment
        .iter()
        .any(|d| d.to_bits() != zero && d.to_bits() != one)
    {
        return Err(Error::invalid(
            "the treatment is an indicator: 0 or 1 per unit",
        ));
    }
    let treated = treatment.iter().filter(|d| d.to_bits() == one).count();
    if treated < 2 || n - treated < 2 {
        return Err(Error::invalid(format!(
            "{treated} treated of {n}: each side needs at least two units"
        )));
    }
    let k = covariates.first().map_or(0, Vec::len);
    if covariates
        .iter()
        .any(|row| row.len() != k || row.iter().any(|v| !v.is_finite()))
    {
        return Err(Error::invalid(
            "every unit needs the same number of finite covariates",
        ));
    }
    // Centre every column so the flat intercept drops out.
    let p = 1 + k;
    let mut columns: Vec<Vec<f64>> = Vec::with_capacity(p);
    columns.push(treatment.to_vec());
    for c in 0..k {
        columns.push(covariates.iter().map(|row| row[c]).collect());
    }
    for column in &mut columns {
        let mean = column.iter().sum::<f64>() / n as f64;
        column.iter_mut().for_each(|v| *v -= mean);
    }
    let y_mean = outcome.iter().sum::<f64>() / n as f64;
    let y: Vec<f64> = outcome.iter().map(|v| v - y_mean).collect();
    let gram = |ridge: f64| -> Vec<Vec<f64>> {
        (0..p)
            .map(|i| {
                (0..p)
                    .map(|j| {
                        columns[i]
                            .iter()
                            .zip(&columns[j])
                            .map(|(a, b)| a * b)
                            .sum::<f64>()
                            + if i == j { ridge } else { 0.0 }
                    })
                    .collect()
            })
            .collect()
    };
    let moment: Vec<f64> = (0..p)
        .map(|i| columns[i].iter().zip(&y).map(|(a, b)| a * b).sum())
        .collect();
    let collinear = || {
        Error::numeric(
            "the treatment is collinear with the covariates; its effect is not separable",
        )
    };
    let ols = crate::causal::solve(gram(0.0), moment.clone()).ok_or_else(collinear)?;
    let dof = n as f64 - 1.0 - p as f64;
    if dof < 1.0 {
        return Err(Error::invalid(format!(
            "{n} units leave {dof} residual degree(s) of freedom for {p} coefficient(s)"
        )));
    }
    let residual: f64 = (0..n)
        .map(|r| {
            let fit: f64 = (0..p).map(|i| ols[i] * columns[i][r]).sum();
            (y[r] - fit).powi(2)
        })
        .sum::<f64>()
        / dof;
    // Posterior precision / sigma^2 = X'X + (sigma^2 / prior_sd^2) I.
    let ridge = residual / (prior_sd * prior_sd);
    let posterior = crate::causal::solve(gram(ridge), moment).ok_or_else(collinear)?;
    let mut unit = vec![0.0; p];
    unit[0] = 1.0;
    let column = crate::causal::solve(gram(ridge), unit).ok_or_else(collinear)?;
    Ok(Estimate {
        kind: EstimatorKind::BayesianCausal,
        identification: Identification::Observational {
            strategy: "Bayesian linear model adjusting for the observed covariates under a stated \
                       Gaussian prior, assuming no unmeasured confounder"
                .into(),
        },
        effect: posterior[0],
        standard_error: Some((residual * column[0]).max(0.0).sqrt()),
        observations: n,
    })
}

/// Difference-in-differences: the treated group's change less the control
/// group's over the same interval. Identified under parallel trends, which
/// the label names so a reviewer can challenge it.
pub fn difference_in_differences(
    treated_before: &[f64],
    treated_after: &[f64],
    control_before: &[f64],
    control_after: &[f64],
) -> Result<Estimate, Error> {
    let (tb, vtb) = sample("treated before", treated_before, 2)?;
    let (ta, vta) = sample("treated after", treated_after, 2)?;
    let (cb, vcb) = sample("control before", control_before, 2)?;
    let (ca, vca) = sample("control after", control_after, 2)?;
    let se = (vtb / treated_before.len() as f64
        + vta / treated_after.len() as f64
        + vcb / control_before.len() as f64
        + vca / control_after.len() as f64)
        .sqrt();
    Ok(Estimate {
        kind: EstimatorKind::DifferenceInDifferences,
        identification: Identification::Observational {
            strategy: "difference-in-differences under parallel trends".into(),
        },
        effect: (ta - tb) - (ca - cb),
        standard_error: Some(se),
        observations: treated_before.len()
            + treated_after.len()
            + control_before.len()
            + control_after.len(),
    })
}

/// Synthetic control: donor weights fitted by least squares to the treated
/// unit's pre-period, and the effect the mean post-period gap between the
/// treated unit and its synthetic twin.
///
/// The weights are unconstrained least squares, not the simplex-constrained
/// weights of the textbook method; a donor pool that needs negative weight to
/// match the treated unit is reported by the fit rather than hidden by a
/// projection. Refused: a pre-period no longer than the donor count (any
/// donors then fit perfectly and the twin is noise) and donor series of the
/// wrong length.
pub fn synthetic_control(
    treated_pre: &[f64],
    treated_post: &[f64],
    donors_pre: &[Vec<f64>],
    donors_post: &[Vec<f64>],
) -> Result<Estimate, Error> {
    let k = donors_pre.len();
    if k == 0 || donors_post.len() != k {
        return Err(Error::invalid(
            "a synthetic control needs at least one donor, observed before and after",
        ));
    }
    sample("the treated pre-period", treated_pre, k + 1)?;
    sample("the treated post-period", treated_post, 1)?;
    for (i, (pre, post)) in donors_pre.iter().zip(donors_post).enumerate() {
        if pre.len() != treated_pre.len() || post.len() != treated_post.len() {
            return Err(Error::invalid(format!(
                "donor {i} is not observed over the same periods as the treated unit"
            )));
        }
        sample(&format!("donor {i}"), pre, 1)?;
        sample(&format!("donor {i}"), post, 1)?;
    }
    let mut xtx = vec![vec![0.0; k]; k];
    let mut xty = vec![0.0; k];
    for (t, y) in treated_pre.iter().enumerate() {
        for i in 0..k {
            xty[i] += donors_pre[i][t] * y;
            for j in 0..k {
                xtx[i][j] += donors_pre[i][t] * donors_pre[j][t];
            }
        }
    }
    let weights = crate::causal::solve(xtx, xty).ok_or_else(|| {
        Error::numeric("the donors are collinear over the pre-period; no unique twin exists")
    })?;
    let gaps: Vec<f64> = treated_post
        .iter()
        .enumerate()
        .map(|(t, y)| y - (0..k).map(|i| weights[i] * donors_post[i][t]).sum::<f64>())
        .collect();
    let (effect, variance) = sample("the post-period gap", &gaps, 1)?;
    Ok(Estimate {
        kind: EstimatorKind::SyntheticControl,
        identification: Identification::Observational {
            strategy: "synthetic control fitted on the pre-period".into(),
        },
        effect,
        standard_error: (gaps.len() > 1).then(|| (variance / gaps.len() as f64).sqrt()),
        observations: treated_pre.len() + treated_post.len(),
    })
}

/// The Wald instrumental-variable estimator, `Cov(z, y) / Cov(z, x)`.
/// Refused when the instrument does not move the treatment: a zero first
/// stage divides by nothing, and a near-zero one is noise.
pub fn instrumental_variable(
    instrument: &[f64],
    treatment: &[f64],
    outcome: &[f64],
) -> Result<Estimate, Error> {
    let n = instrument.len();
    if treatment.len() != n || outcome.len() != n {
        return Err(Error::invalid(
            "instrument, treatment and outcome must be observed on the same units",
        ));
    }
    let (mz, vz) = sample("the instrument", instrument, 3)?;
    let (mx, vx) = sample("the treatment", treatment, 3)?;
    let (my, _) = sample("the outcome", outcome, 3)?;
    let cov = |a: &[f64], ma: f64, b: &[f64], mb: f64| {
        a.iter()
            .zip(b)
            .map(|(x, y)| (x - ma) * (y - mb))
            .sum::<f64>()
            / (n as f64 - 1.0)
    };
    let first = cov(instrument, mz, treatment, mx);
    let strength = if vz > 0.0 && vx > 0.0 {
        first / (vz * vx).sqrt()
    } else {
        0.0
    };
    if strength.abs() < 0.1 {
        return Err(Error::denied(format!(
            "the instrument's correlation with the treatment is {strength:.3}; below 0.1 it is \
             a weak instrument and the ratio is noise"
        )));
    }
    Ok(Estimate {
        kind: EstimatorKind::InstrumentalVariable,
        identification: Identification::Observational {
            strategy: "instrumental variable (Wald)".into(),
        },
        effect: cov(instrument, mz, outcome, my) / first,
        standard_error: None,
        observations: n,
    })
}

/// The effect of `do(cause)` on `effect` in a structural model: identified
/// by the graph the model declares, which the label says.
pub fn structural(model: &StructuralModel, cause: &str, effect: &str) -> Result<Estimate, Error> {
    Ok(Estimate {
        kind: EstimatorKind::StructuralModel,
        identification: Identification::Observational {
            strategy: format!(
                "structural causal model `{}`, adjusted by its declared graph",
                model.name()
            ),
        },
        effect: model.interventional_effect(cause, effect)?,
        standard_error: None,
        observations: 0,
    })
}

/// What was observed after acting, less the no-action counterfactual a
/// structural model computes for the same world. Identified only as well as
/// the model is, which the label says.
pub fn no_action_baseline(
    model: &StructuralModel,
    observed_after: &std::collections::BTreeMap<String, f64>,
    acted_on: &std::collections::BTreeMap<String, f64>,
    outcome: &str,
) -> Result<Estimate, Error> {
    // The counterfactual of not acting: release each acted-on variable back
    // to what its own parents and abducted noise would have made it, by
    // abducing on the observed world and evaluating the un-intervened model
    // with those exogenous terms, except at the acted-on variables, whose
    // exogenous terms the action overwrote and so are taken as zero.
    let mut noise = model.abduce(observed_after)?;
    for variable in acted_on.keys() {
        noise.insert(variable.clone(), 0.0);
    }
    let without = model.evaluate(&noise);
    let observed = *observed_after
        .get(outcome)
        .ok_or_else(|| Error::invalid(format!("the observation does not record `{outcome}`")))?;
    let baseline = *without
        .get(outcome)
        .ok_or_else(|| Error::invalid(format!("model does not contain `{outcome}`")))?;
    Ok(Estimate {
        kind: EstimatorKind::NoActionBaseline,
        identification: Identification::Observational {
            strategy: format!("no-action counterfactual from model `{}`", model.name()),
        },
        effect: observed - baseline,
        standard_error: None,
        observations: 1,
    })
}

/// The regression slope of `y` on `x`. A correlation, and labelled so:
/// [`Identification::NotIdentified`], however strong.
pub fn correlation(x: &[f64], y: &[f64]) -> Result<Estimate, Error> {
    if x.len() != y.len() {
        return Err(Error::invalid("x and y must be observed on the same units"));
    }
    let (mx, vx) = sample("x", x, 2)?;
    let (my, _) = sample("y", y, 2)?;
    if vx <= 0.0 {
        return Err(Error::numeric("x is constant; no slope exists"));
    }
    let cov = x
        .iter()
        .zip(y)
        .map(|(a, b)| (a - mx) * (b - my))
        .sum::<f64>()
        / (x.len() as f64 - 1.0);
    Ok(Estimate {
        kind: EstimatorKind::Correlation,
        identification: Identification::NotIdentified,
        effect: cov / vx,
        standard_error: None,
        observations: x.len(),
    })
}

/// The data an operator supplies for one estimator, as a document.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum EstimateInput {
    /// A hold-out drawn here from `seed` ([`Experiment`]); the operator
    /// supplies the units and their outcomes, never the assignment.
    HoldOutExperiment {
        seed: String,
        units: Vec<String>,
        hold_out_bps: u32,
        outcomes: BTreeMap<String, f64>,
    },
    StagedExperiment {
        seed: String,
        units: Vec<String>,
        stages: usize,
        outcomes: BTreeMap<usize, BTreeMap<String, f64>>,
    },
    MatchedControl {
        treated_covariates: Vec<Vec<f64>>,
        treated_outcomes: Vec<f64>,
        pool_covariates: Vec<Vec<f64>>,
        pool_outcomes: Vec<f64>,
        caliper: f64,
    },
    BayesianCausal {
        treatment: Vec<f64>,
        covariates: Vec<Vec<f64>>,
        outcome: Vec<f64>,
        prior_sd: f64,
    },
    /// Two groups the operator formed. Runs, and earns
    /// [`Identification::NotIdentified`], so it widens nothing; use
    /// `HoldOutExperiment` for evidence that may.
    RandomisedExperiment {
        treated: Vec<f64>,
        control: Vec<f64>,
    },
    DifferenceInDifferences {
        treated_before: Vec<f64>,
        treated_after: Vec<f64>,
        control_before: Vec<f64>,
        control_after: Vec<f64>,
    },
    SyntheticControl {
        treated_pre: Vec<f64>,
        treated_post: Vec<f64>,
        donors_pre: Vec<Vec<f64>>,
        donors_post: Vec<Vec<f64>>,
    },
    InstrumentalVariable {
        instrument: Vec<f64>,
        treatment: Vec<f64>,
        outcome: Vec<f64>,
    },
    Correlation {
        x: Vec<f64>,
        y: Vec<f64>,
    },
}

impl EstimateInput {
    /// Run the estimator the input names.
    pub fn estimate(&self) -> Result<Estimate, Error> {
        match self {
            Self::HoldOutExperiment {
                seed,
                units,
                hold_out_bps,
                outcomes,
            } => Experiment::design(seed, units, *hold_out_bps)?.analyse(outcomes),
            Self::StagedExperiment {
                seed,
                units,
                stages,
                outcomes,
            } => staged_experiment(seed, units, *stages, outcomes),
            Self::MatchedControl {
                treated_covariates,
                treated_outcomes,
                pool_covariates,
                pool_outcomes,
                caliper,
            } => matched_control(
                treated_covariates,
                treated_outcomes,
                pool_covariates,
                pool_outcomes,
                *caliper,
            ),
            Self::BayesianCausal {
                treatment,
                covariates,
                outcome,
                prior_sd,
            } => bayesian_causal(treatment, covariates, outcome, *prior_sd),
            Self::RandomisedExperiment { treated, control } => {
                operator_formed_comparison(treated, control)
            }
            Self::DifferenceInDifferences {
                treated_before,
                treated_after,
                control_before,
                control_after,
            } => difference_in_differences(
                treated_before,
                treated_after,
                control_before,
                control_after,
            ),
            Self::SyntheticControl {
                treated_pre,
                treated_post,
                donors_pre,
                donors_post,
            } => synthetic_control(treated_pre, treated_post, donors_pre, donors_post),
            Self::InstrumentalVariable {
                instrument,
                treatment,
                outcome,
            } => instrumental_variable(instrument, treatment, outcome),
            Self::Correlation { x, y } => correlation(x, y),
        }
    }
}

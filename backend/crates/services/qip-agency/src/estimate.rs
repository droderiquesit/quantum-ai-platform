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
use qip_core::Error;
use serde::{Deserialize, Serialize};

/// Which method produced an estimate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EstimatorKind {
    RandomisedExperiment,
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

/// A randomised (or staged, randomly assigned) experiment: the difference in
/// means between treated and control, with its standard error.
pub fn randomised_experiment(treated: &[f64], control: &[f64]) -> Result<Estimate, Error> {
    let (mt, vt) = sample("the treated group", treated, 2)?;
    let (mc, vc) = sample("the control group", control, 2)?;
    Ok(Estimate {
        kind: EstimatorKind::RandomisedExperiment,
        identification: Identification::Experiment,
        effect: mt - mc,
        standard_error: Some((vt / treated.len() as f64 + vc / control.len() as f64).sqrt()),
        observations: treated.len() + control.len(),
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
            Self::RandomisedExperiment { treated, control } => {
                randomised_experiment(treated, control)
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

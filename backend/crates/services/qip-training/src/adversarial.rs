//! An adversarial perturbation suite a model must survive before acceptance
//! (R12-MODEL-4021, blueprint §10.1).
//!
//! Holdout error says how well a model predicts the rows it was shown. It
//! says nothing about what the model does to a row nudged by less than the
//! data's own noise, and that is where a fragile model hurts: a tree whose
//! split sits in the middle of the data, or a fit that leaned on one
//! collinear feature, flips its output when a price ticks. The suite is an
//! attacker with a small budget: for every probe row, every feature is moved
//! by a fixed fraction of that feature's spread in each direction, the worst
//! output movement is kept, and a row whose worst movement exceeds a fixed
//! fraction of the output's own spread is fragile. A model with too many
//! fragile rows, or with any perturbed output that is not a finite number,
//! is refused.
//!
//! Coordinate-wise rather than a gradient attack on purpose: the platform's
//! models are piecewise (stumps) as well as smooth, a gradient is zero almost
//! everywhere on a piecewise model, and the coordinate search finds exactly
//! the threshold crossings a gradient would miss. It is deterministic, so a
//! replay refuses the same models.

use qip_core::error::{Error, Result};

/// How hard the suite attacks and what it tolerates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PerturbationPolicy {
    /// The attacker's budget per feature, as a fraction of that feature's
    /// standard deviation over the probe rows.
    pub input_step_sd: f64,
    /// The largest output movement a row may show, as a fraction of the
    /// output's standard deviation over the probe rows.
    pub max_movement_sd: f64,
    /// The share of probe rows that may be fragile.
    pub max_fragile_share: f64,
    /// The share of probe rows that may be fragile to a nudge of any one
    /// feature. Separate from the pooled share because the case this suite
    /// exists for — one split in the middle of the data — makes only the
    /// rows within a nudge of that one threshold fragile, about six in a
    /// hundred for a uniform feature, which a pooled tenth lets through.
    pub max_single_feature_share: f64,
}

impl Default for PerturbationPolicy {
    /// A nudge of a tenth of a feature's spread — well inside a day's noise
    /// for any feature the desk builds — may not move the output by more
    /// than half its spread on more than a tenth of the rows. A smooth model
    /// moves by about a tenth of its spread under that nudge and passes with
    /// room; a model whose output jumps by its whole spread across a
    /// threshold dense in the data does not.
    fn default() -> Self {
        Self {
            input_step_sd: 0.1,
            max_movement_sd: 0.5,
            max_fragile_share: 0.1,
            max_single_feature_share: 0.02,
        }
    }
}

impl PerturbationPolicy {
    fn validate(&self) -> Result<()> {
        let fraction = |value: f64| value.is_finite() && value > 0.0;
        let valid = fraction(self.input_step_sd)
            && fraction(self.max_movement_sd)
            && (0.0..=1.0).contains(&self.max_fragile_share)
            && (0.0..=1.0).contains(&self.max_single_feature_share);
        if !valid {
            return Err(Error::invalid(format!(
                "a perturbation policy needs a positive input step and movement bound and a \
                 fragile share in [0, 1]; {self:?} is not one"
            )));
        }
        Ok(())
    }
}

/// What the suite found.
#[derive(Clone, Debug, PartialEq)]
pub struct PerturbationReport {
    pub rows: usize,
    /// Rows whose worst movement exceeded the bound.
    pub fragile_rows: usize,
    /// For each feature, the rows a nudge of that feature alone made
    /// fragile, in feature order.
    pub fragile_by_feature: Vec<usize>,
    /// The worst movement found anywhere, as a fraction of the output's
    /// standard deviation.
    pub worst_movement_sd: f64,
    /// Perturbed evaluations that refused or returned a non-finite number.
    pub failures: usize,
}

impl PerturbationReport {
    pub fn fragile_share(&self) -> f64 {
        if self.rows == 0 {
            0.0
        } else {
            self.fragile_rows as f64 / self.rows as f64
        }
    }

    /// Why the model fails `policy`, or `None` if it survives.
    pub fn refusal(&self, policy: &PerturbationPolicy) -> Option<String> {
        if self.failures > 0 {
            return Some(format!(
                "{} perturbed evaluation(s) refused or returned a non-finite output",
                self.failures
            ));
        }
        if let Some((feature, rows)) = self
            .fragile_by_feature
            .iter()
            .enumerate()
            .max_by_key(|(_, rows)| **rows)
            .filter(|(_, rows)| {
                self.rows > 0 && **rows as f64 / self.rows as f64 > policy.max_single_feature_share
            })
        {
            return Some(format!(
                "a nudge of feature {feature} alone moves {rows} of {} probe row(s) by more than \
                 {:.2} of the output's spread, above the {:.0}% one feature may move: the model \
                 hangs on a split inside its own data",
                self.rows,
                policy.max_movement_sd,
                policy.max_single_feature_share * 100.0
            ));
        }
        if self.fragile_share() > policy.max_fragile_share {
            return Some(format!(
                "{} of {} probe row(s) move by more than {:.2} of the output's spread when a \
                 feature is nudged by {:.2} of its own (worst {:.2}), above the {:.0}% tolerated",
                self.fragile_rows,
                self.rows,
                policy.max_movement_sd,
                policy.input_step_sd,
                self.worst_movement_sd,
                policy.max_fragile_share * 100.0
            ));
        }
        None
    }
}

fn deviation(values: impl Iterator<Item = f64> + Clone) -> f64 {
    let n = values.clone().count() as f64;
    if n < 2.0 {
        return 0.0;
    }
    let mean = values.clone().sum::<f64>() / n;
    (values.map(|v| (v - mean) * (v - mean)).sum::<f64>() / (n - 1.0)).sqrt()
}

/// Attack `model` on `rows` under `policy`.
///
/// Refuses fewer than two rows, rows of unequal arity, a policy that is not
/// one, and a model whose unperturbed outputs on the probe rows do not vary:
/// with no spread there is no scale to measure a movement against, and a
/// constant model is a different finding than a robust one.
pub fn attack(
    model: impl Fn(&[f64]) -> Result<f64>,
    rows: &[Vec<f64>],
    policy: &PerturbationPolicy,
) -> Result<PerturbationReport> {
    policy.validate()?;
    let Some(first) = rows.first() else {
        return Err(Error::invalid("a perturbation suite needs probe rows"));
    };
    if rows.len() < 2 {
        return Err(Error::invalid(
            "a perturbation suite needs at least two probe rows to measure a spread",
        ));
    }
    let arity = first.len();
    if rows.iter().any(|row| row.len() != arity) {
        return Err(Error::invalid("probe rows of unequal arity"));
    }
    let baseline: Vec<f64> = rows
        .iter()
        .map(|row| model(row))
        .collect::<Result<Vec<f64>>>()?;
    if baseline.iter().any(|value| !value.is_finite()) {
        return Err(Error::numeric(
            "the model returns a non-finite output on an unperturbed probe row",
        ));
    }
    let output_sd = deviation(baseline.iter().copied());
    if output_sd <= 0.0 {
        return Err(Error::invalid(
            "the model's output does not vary over the probe rows, so no movement can be \
             measured against its spread",
        ));
    }
    let steps: Vec<f64> = (0..arity)
        .map(|column| deviation(rows.iter().map(|row| row[column])) * policy.input_step_sd)
        .collect();
    let mut fragile_rows = 0;
    let mut fragile_by_feature = vec![0usize; arity];
    let mut failures = 0;
    let mut worst = 0.0_f64;
    for (row, base) in rows.iter().zip(&baseline) {
        let mut row_worst = 0.0_f64;
        for (column, step) in steps.iter().enumerate() {
            if *step <= 0.0 {
                continue;
            }
            let mut feature_worst = 0.0_f64;
            for direction in [1.0, -1.0] {
                let mut nudged = row.clone();
                nudged[column] += direction * step;
                match model(&nudged) {
                    Ok(value) if value.is_finite() => {
                        feature_worst = feature_worst.max((value - base).abs() / output_sd);
                    }
                    _ => failures += 1,
                }
            }
            if feature_worst > policy.max_movement_sd {
                fragile_by_feature[column] += 1;
            }
            row_worst = row_worst.max(feature_worst);
        }
        if row_worst > policy.max_movement_sd {
            fragile_rows += 1;
        }
        worst = worst.max(row_worst);
    }
    Ok(PerturbationReport {
        rows: rows.len(),
        fragile_rows,
        fragile_by_feature,
        worst_movement_sd: worst,
        failures,
    })
}

#[cfg(test)]
#[allow(clippy::panic_in_result_fn)]
mod tests {
    use super::*;

    fn rows(n: usize) -> Vec<Vec<f64>> {
        (0..n)
            .map(|i| {
                let x = i as f64 / n as f64;
                vec![x, (x * 7.0).sin()]
            })
            .collect()
    }

    /// A smooth model survives: a tenth of a spread in moves a linear output
    /// by at most about a tenth of its spread.
    #[test]
    fn a_smooth_model_survives_the_attack() -> Result<()> {
        let report = attack(
            |r| Ok(2.0 * r[0] - 0.5 * r[1]),
            &rows(200),
            &PerturbationPolicy::default(),
        )?;
        assert_eq!(report.failures, 0);
        assert_eq!(report.fragile_rows, 0, "{report:?}");
        assert!(report.worst_movement_sd < 0.2, "{report:?}");
        assert_eq!(report.refusal(&PerturbationPolicy::default()), None);
        Ok(())
    }

    /// A model whose output flips sign across thresholds dense in the data
    /// is refused: almost every row sits within a nudge of a flip. Mutation:
    /// compare `row_worst` with `<` in `attack` — no row is fragile and the
    /// refusal assertion fails.
    #[test]
    fn a_model_that_flips_inside_the_data_noise_is_refused() -> Result<()> {
        let report = attack(
            |r| Ok((r[0] * 300.0).sin().signum()),
            &rows(200),
            &PerturbationPolicy::default(),
        )?;
        assert!(report.fragile_share() > 0.5, "{report:?}");
        // The pooled bar on its own, with the single-feature bar opened.
        let pooled_only = PerturbationPolicy {
            max_single_feature_share: 1.0,
            ..PerturbationPolicy::default()
        };
        assert!(
            report
                .refusal(&pooled_only)
                .is_some_and(|why| why.contains("probe row(s) move")),
            "{report:?}"
        );
        Ok(())
    }

    /// The case the suite exists for: one split in the middle of the data.
    /// Only the rows within a nudge of the threshold are fragile — under the
    /// pooled tenth — and the single-feature share refuses it. An
    /// independent skeptic showed the pooled share alone let it through.
    /// Mutation: drop the single-feature check in `refusal` — the model is
    /// accepted and the assertion fails.
    #[test]
    fn a_model_hanging_on_one_split_inside_its_data_is_refused() -> Result<()> {
        let data: Vec<Vec<f64>> = (0..400)
            .map(|i| {
                vec![
                    (i as f64 * 0.618_033_988_7) % 1.0,
                    (i as f64 * 0.414_213_562) % 1.0,
                ]
            })
            .collect();
        let report = attack(
            |r| Ok(if r[0] > 0.5 { 1.0 } else { 0.0 }),
            &data,
            &PerturbationPolicy::default(),
        )?;
        assert!(
            report.fragile_share() <= PerturbationPolicy::default().max_fragile_share,
            "premise: the pooled share alone would pass it: {report:?}"
        );
        assert!(
            report
                .refusal(&PerturbationPolicy::default())
                .is_some_and(|why| why.contains("feature 0 alone")),
            "{report:?}"
        );
        Ok(())
    }

    /// A model that fails off the probe rows is refused for that alone.
    #[test]
    fn a_model_that_returns_a_non_finite_output_when_nudged_is_refused() -> Result<()> {
        let data = rows(50);
        let edge = data.iter().map(|r| r[0]).fold(f64::NEG_INFINITY, f64::max);
        let report = attack(
            move |r| Ok(if r[0] > edge { f64::NAN } else { r[0] }),
            &data,
            &PerturbationPolicy::default(),
        )?;
        assert!(report.failures > 0);
        assert!(
            report
                .refusal(&PerturbationPolicy::default())
                .is_some_and(|why| why.contains("non-finite"))
        );
        Ok(())
    }

    #[test]
    fn a_constant_model_or_an_empty_probe_is_refused_rather_than_passed() {
        assert!(attack(|_| Ok(1.0), &rows(20), &PerturbationPolicy::default()).is_err());
        assert!(attack(|r| Ok(r[0]), &[], &PerturbationPolicy::default()).is_err());
        let bad = PerturbationPolicy {
            input_step_sd: 0.0,
            ..PerturbationPolicy::default()
        };
        assert!(attack(|r| Ok(r[0]), &rows(20), &bad).is_err());
    }
}

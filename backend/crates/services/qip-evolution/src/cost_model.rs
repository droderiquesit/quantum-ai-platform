//! Improving the cost model from what execution actually cost.
//!
//! A cost model is the deduction that decides whether an edge is real, and it
//! is the part of a strategy most likely to be quietly wrong: the backtest
//! assumes a number, the market charges another, and the difference is
//! subtracted from the alpha every single fill. This module turns the gap
//! between the two into a proposed correction.
//!
//! Two disciplines keep the correction from becoming its own overfit.
//!
//! **The correction is shrunk.** The bias applied is the observed mean error
//! scaled by [`crate::scoring::evidence_weight`], the same weight
//! [`qip_contracts::Conviction::shrunk`] uses. Five expensive fills move the
//! model by a seventh of what they seem to say; five hundred move it by
//! almost all of it. A cost model that chases its last twenty fills is a cost
//! model that is always describing the week before last.
//!
//! **The correction is checked forward, not backward.** Any bias fitted on a
//! sample reduces that sample's error by construction, so an in-sample
//! improvement is not evidence of anything.
//! [`qip_simulation_engine::validation::WalkForward`] already builds the only
//! split that matches deployment — fit on the past, measure on the future —
//! and [`CostModelUpdate::out_of_sample_improvement_bps`] is measured on it.
//! An update that does not help out of sample is reported and refused, not
//! quietly applied.

use crate::scoring::evidence_weight;
use qip_core::error::{Error, Result};
use qip_numerics::stats;
use qip_simulation_engine::validation::WalkForward;
use std::collections::BTreeMap;

/// One fill's modelled cost against what it actually cost.
#[derive(Clone, Debug, PartialEq)]
pub struct CostObservation {
    /// Venue, size bucket, session — whatever the model is conditioned on. A
    /// correction fitted across contexts corrects none of them.
    pub context: String,
    pub modelled_bps: f64,
    pub realised_bps: f64,
}

impl CostObservation {
    pub fn new(context: impl Into<String>, modelled_bps: f64, realised_bps: f64) -> Self {
        Self {
            context: context.into(),
            modelled_bps,
            realised_bps,
        }
    }

    /// Realised less modelled. Positive means the fill cost more than the
    /// model said it would.
    pub fn error_bps(&self) -> f64 {
        self.realised_bps - self.modelled_bps
    }
}

/// A proposed revision to one context's cost model.
#[derive(Clone, Debug, PartialEq)]
pub struct CostModelUpdate {
    pub context: String,
    pub observations: u32,
    /// Mean realised-minus-modelled error over the sample.
    pub mean_error_bps: f64,
    /// Mean absolute error, which is what the correction is trying to reduce.
    pub mean_absolute_error_bps: f64,
    /// Slope of realised on modelled. One means the model scales correctly and
    /// is only offset; anything else means it is wrong about size as well.
    pub slope: f64,
    /// Intercept of the same fit, in basis points.
    pub intercept_bps: f64,
    /// The bias actually proposed: the mean error, shrunk by how much evidence
    /// stands behind it.
    pub applied_bias_bps: f64,
    /// Reduction in mean absolute error on windows strictly after the ones the
    /// bias was fitted on. `None` where the sample is too short to walk
    /// forward at all — which is itself a reason not to apply an update.
    pub out_of_sample_improvement_bps: Option<f64>,
}

impl CostModelUpdate {
    /// Whether this revision has earned the right to be applied.
    ///
    /// Three conditions, all of which have to hold: the sample is not tiny,
    /// the correction is not smaller than the noise it is correcting, and it
    /// helped on data it was not fitted on.
    pub fn is_worth_applying(&self, minimum_observations: u32) -> bool {
        self.observations >= minimum_observations
            && self.applied_bias_bps.abs() > f64::EPSILON
            && self
                .out_of_sample_improvement_bps
                .is_some_and(|improvement| improvement > 0.0)
    }

    /// The modelled cost with this revision applied.
    pub fn corrected(&self, modelled_bps: f64) -> f64 {
        modelled_bps + self.applied_bias_bps
    }

    pub fn summarise(&self) -> String {
        format!(
            "{}: {} fill(s) ran {:+.2}bp against the model (mean absolute {:.2}bp); \
             proposing {:+.2}bp after shrinkage, {}",
            self.context,
            self.observations,
            self.mean_error_bps,
            self.mean_absolute_error_bps,
            self.applied_bias_bps,
            match self.out_of_sample_improvement_bps {
                Some(improvement) => format!("worth {improvement:+.2}bp out of sample"),
                None => "with too little history to check it forward".to_string(),
            }
        )
    }
}

/// Modelled-against-realised costs, kept per context.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CostModelCalibration {
    /// Ordered in time, because the forward check depends on the order.
    cells: BTreeMap<String, Vec<(f64, f64)>>,
}

impl CostModelCalibration {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record one fill. Order matters: observations are appended, and the
    /// forward check reads them as a time series.
    pub fn observe(&mut self, observation: &CostObservation) {
        self.cells
            .entry(observation.context.clone())
            .or_default()
            .push((observation.modelled_bps, observation.realised_bps));
    }

    pub fn observe_all<'a>(&mut self, observations: impl IntoIterator<Item = &'a CostObservation>) {
        for observation in observations {
            self.observe(observation);
        }
    }

    pub fn contexts(&self) -> Vec<&str> {
        self.cells.keys().map(String::as_str).collect()
    }

    pub fn len(&self) -> usize {
        self.cells.values().map(Vec::len).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.cells.is_empty()
    }

    /// Propose a revision for one context.
    ///
    /// Errors only when there is nothing to propose from. A weak proposal is
    /// returned rather than withheld, because [`CostModelUpdate`] carries
    /// everything needed to see that it is weak and
    /// [`CostModelUpdate::is_worth_applying`] is where that judgement belongs.
    pub fn propose(&self, context: &str) -> Result<CostModelUpdate> {
        let Some(pairs) = self.cells.get(context) else {
            return Err(Error::not_found(format!(
                "no fills have been recorded in {context}"
            )));
        };
        if pairs.is_empty() {
            return Err(Error::not_found(format!(
                "no fills have been recorded in {context}"
            )));
        }

        let modelled: Vec<f64> = pairs.iter().map(|(modelled, _)| *modelled).collect();
        let realised: Vec<f64> = pairs.iter().map(|(_, realised)| *realised).collect();
        let errors: Vec<f64> = pairs
            .iter()
            .map(|(modelled, realised)| realised - modelled)
            .collect();
        let absolute: Vec<f64> = errors.iter().map(|error| error.abs()).collect();

        let observations = u32::try_from(pairs.len()).unwrap_or(u32::MAX);
        let mean_error = stats::mean(&errors);
        // The whole point of the shrinkage: a handful of expensive fills is a
        // handful of expensive fills, not a new cost model.
        let applied_bias = mean_error * evidence_weight(observations);

        let (slope, intercept) = match stats::linear_fit(&modelled, &realised) {
            Ok(regression) => (
                regression.coefficients.get(1).copied().unwrap_or(1.0),
                regression.coefficients.first().copied().unwrap_or(0.0),
            ),
            // A degenerate fit — every modelled cost identical, say — is not a
            // failure of the calibration. It means the model has no slope to
            // check, and the bias is still meaningful.
            Err(_) => (f64::NAN, f64::NAN),
        };

        Ok(CostModelUpdate {
            context: context.to_string(),
            observations,
            mean_error_bps: mean_error,
            mean_absolute_error_bps: stats::mean(&absolute),
            slope,
            intercept_bps: intercept,
            applied_bias_bps: applied_bias,
            out_of_sample_improvement_bps: forward_improvement(&errors),
        })
    }

    /// Propose a revision for every context, in canonical order.
    pub fn propose_all(&self) -> Vec<CostModelUpdate> {
        self.cells
            .keys()
            .filter_map(|context| self.propose(context).ok())
            .collect()
    }
}

/// How much a bias fitted on the past would have reduced absolute error on the
/// future.
///
/// Uses [`WalkForward`] rather than a fold split, because a cost model is
/// deployed forwards: the only question that matters is whether last quarter's
/// correction helps this quarter. Returns `None` when the series is too short
/// to have a future to test on, which is a reason to leave the model alone.
fn forward_improvement(errors: &[f64]) -> Option<f64> {
    let n = errors.len();
    // Half the sample to fit on and a tenth to test on, with a floor so the
    // windows are never so small that their means are noise.
    let train = (n / 2).max(20);
    let test = (n / 10).max(5);
    let splits = WalkForward::new(train, test, true, 0)
        .and_then(|walk| walk.split(n))
        .ok()?;

    let mut improvements = Vec::new();
    for split in &splits {
        let fitted: Vec<f64> = split
            .train
            .iter()
            .filter_map(|index| errors.get(*index).copied())
            .collect();
        let held: Vec<f64> = split
            .test
            .iter()
            .filter_map(|index| errors.get(*index).copied())
            .collect();
        if fitted.is_empty() || held.is_empty() {
            continue;
        }
        let bias =
            stats::mean(&fitted) * evidence_weight(u32::try_from(fitted.len()).unwrap_or(u32::MAX));
        let before = stats::mean(&held.iter().map(|error| error.abs()).collect::<Vec<_>>());
        let after = stats::mean(
            &held
                .iter()
                .map(|error| (error - bias).abs())
                .collect::<Vec<_>>(),
        );
        improvements.push(before - after);
    }
    (!improvements.is_empty()).then(|| stats::mean(&improvements))
}

/// A return series that has already had the cost of earning it taken out.
///
/// The type exists so that "net of costs" is a property of a value rather than
/// a promise in a doc comment. There is no constructor that takes a return
/// series on its own: every way in requires a cost series beside it, of the
/// same length, and the charge is kept so that a reader can see what was
/// deducted. [`crate::challenger::ChallengerEntry`] takes `NetReturns` and not
/// `Vec<f64>`, so a challenger cannot be tested on gross alpha — not because
/// somebody remembered to subtract, but because there is no value of the
/// required type that has not had something subtracted.
///
/// Costs are non-negative, following [`qip_contracts::edge::Deduction`]: a
/// charge that *adds* to a return is a rebate, and belongs in the model as a
/// smaller cost rather than as a negative one. A negative charge is the shape
/// a gross series takes when somebody is trying to make one look net.
#[derive(Clone, Debug, PartialEq)]
pub struct NetReturns {
    net: Vec<f64>,
    charged: Vec<f64>,
}

impl NetReturns {
    /// Gross returns less what each period cost, both in return units.
    ///
    /// Refuses a cost series of a different length. A cost series that does not
    /// line up with the returns is being charged to the wrong periods, and the
    /// resulting series is neither gross nor net.
    pub fn of(gross: &[f64], cost: &[f64]) -> Result<Self> {
        if gross.len() != cost.len() {
            return Err(Error::invalid(format!(
                "{} return(s) against {} charge(s); a cost series that does not line up \
                 with the returns is being charged to the wrong periods",
                gross.len(),
                cost.len()
            )));
        }
        if gross.is_empty() {
            return Err(Error::invalid(
                "a series with no observations is not a result",
            ));
        }
        if let Some((index, value)) = gross
            .iter()
            .enumerate()
            .find(|(_, value)| !value.is_finite())
        {
            return Err(Error::numeric(format!(
                "the return in period {index} is {value}, which cannot be scored"
            )));
        }
        if let Some((index, charge)) = cost
            .iter()
            .enumerate()
            .find(|(_, charge)| !charge.is_finite() || **charge < 0.0)
        {
            return Err(Error::invalid(format!(
                "period {index} was charged {charge}; a cost that adds to a return is a \
                 rebate and belongs in the model as a smaller cost, never as a negative one"
            )));
        }
        Ok(Self {
            net: gross
                .iter()
                .zip(cost)
                .map(|(gross, charge)| gross - charge)
                .collect(),
            charged: cost.to_vec(),
        })
    }

    /// Gross returns less a cost quoted in basis points of the period's
    /// notional, which is how a cost model states itself.
    pub fn charged_bps(gross: &[f64], cost_bps: &[f64]) -> Result<Self> {
        let cost: Vec<f64> = cost_bps.iter().map(|bps| bps / 10_000.0).collect();
        Self::of(gross, &cost)
    }

    /// Gross returns less the same charge every period, in basis points.
    ///
    /// The crude case, kept because it is honest about being crude: one number
    /// a reviewer can see and argue with. A caller passing zero here is
    /// claiming the strategy traded for free, in one visible place, and
    /// [`Self::is_costless`] is what notices.
    pub fn flat_bps(gross: &[f64], cost_bps: f64) -> Result<Self> {
        let cost = vec![cost_bps / 10_000.0; gross.len()];
        Self::of(gross, &cost)
    }

    /// Gross returns less the cost a calibrated model says each period cost.
    ///
    /// The path from [`CostModelUpdate`] to a score: the modelled charge with
    /// the calibration's shrunken bias applied, so a strategy is scored against
    /// what execution actually costs rather than what the backtest assumed.
    ///
    /// Refuses an update that has not earned the right to be applied by
    /// [`CostModelUpdate::is_worth_applying`]. The module documentation
    /// promises that a correction which does not help out of sample is
    /// "refused, not quietly applied" — this is the one place in the crate
    /// where a bias actually reaches a return series, so it is the one place
    /// that promise has to be kept structurally rather than left to a caller
    /// who remembers to check first. Without this, a bias fitted on five
    /// fills with no out-of-sample evidence at all could still be baked into
    /// the series a challenger is scored against, which is the same failure
    /// this crate refuses everywhere else: a number that has not earned its
    /// keep, applied anyway.
    pub fn corrected_by(
        gross: &[f64],
        modelled_bps: &[f64],
        update: &CostModelUpdate,
        minimum_observations: u32,
    ) -> Result<Self> {
        if !update.is_worth_applying(minimum_observations) {
            return Err(Error::guard(format!(
                "{} has not earned the right to be applied: {}",
                update.context,
                update.summarise()
            )));
        }
        let corrected: Vec<f64> = modelled_bps
            .iter()
            .map(|modelled| update.corrected(*modelled))
            .collect();
        Self::charged_bps(gross, &corrected)
    }

    /// The net series, in time order.
    pub fn as_slice(&self) -> &[f64] {
        &self.net
    }

    /// What each period was charged, in time order.
    pub fn charges(&self) -> &[f64] {
        &self.charged
    }

    pub fn len(&self) -> usize {
        self.net.len()
    }

    pub fn is_empty(&self) -> bool {
        self.net.is_empty()
    }

    /// Everything deducted over the window, in return units.
    pub fn total_cost(&self) -> f64 {
        self.charged.iter().sum()
    }

    /// Whether nothing at all was deducted.
    ///
    /// A strategy that traded for a year and paid nothing has a backtest, not a
    /// result. [`crate::challenger::ChallengerTest`] refuses such a series
    /// rather than scoring it, because the number it would produce is gross
    /// alpha wearing a net label.
    pub fn is_costless(&self) -> bool {
        self.total_cost() <= 0.0
    }

    /// The series before costs, recovered from what was charged.
    ///
    /// Reported so that the size of the deduction is visible next to the result
    /// it changed, which is the only way anyone notices a cost model that is
    /// doing nothing.
    pub fn gross(&self) -> Vec<f64> {
        self.net
            .iter()
            .zip(&self.charged)
            .map(|(net, charge)| net + charge)
            .collect()
    }
}

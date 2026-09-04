//! The unconditional baseline, and the out-of-sample comparison Gate 8 asks
//! for: does regime-conditional allocation beat unconditional allocation on
//! data neither arm was fitted on?
//!
//! Until this module existed the platform had a regime detector and no
//! comparison arm. "We sized on a regime model" was the whole result, which
//! is the failure ADR 0006 names for the quantum path: a method is not a
//! finding, and a finding is a difference against a baseline computed every
//! time. [`RegimeComparison::run`] computes both arms, always, from one
//! declared split, and returns the difference beside them.
//!
//! # One sizing rule, one term removed
//!
//! Both arms size through [`size`]: `base_weight × regime_term`. The
//! conditional arm's term is the ratio of the calm state's volatility to the
//! volatility the filtered regime posterior implies, capped at one — it never
//! sizes *up* on a regime, only down. The baseline is the identical arithmetic
//! with the term removed ([`RegimeTerm::Removed`], a multiplier of one). There
//! is no third allocator, because a baseline that differed from the treatment
//! in more than the thing under test would measure the other differences too.
//!
//! # The split is declared, and the holdout is what was not knowable
//!
//! A [`SplitDeclaration`] is a boundary instant and the instant it was fixed.
//! There is no default: an undeclared split is one the reader cannot check
//! against the event log, and a comparison whose split could have been chosen
//! after looking at the result is unfalsifiable. Observations are bitemporal
//! ([`ReturnObservation::known_at`] beside [`ReturnObservation::at`]), and
//! the run refuses a holdout observation that was knowable before the
//! boundary — the shape a bar keyed on its open time takes, where a day of
//! the future is readable at the fit.
//!
//! # What is reused rather than restated
//!
//! The lifecycle's holdout gate scores a holdout with
//! [`crate::validation::deflated_sharpe`] and cuts folds as a
//! [`crate::validation::Split`]; both arms here are scored by the same
//! function and the split is exposed as the same type, so there is one notion
//! of out-of-sample in the platform rather than two that will one day
//! disagree. The regime model is [`GaussianHmm`], fitted on the training side
//! only and filtered forward — the online form that reads no future.
//!
//! Everything here is a statistic — weights are fractions of equity and
//! returns are per-period fractions, as in
//! [`crate::backtest::BacktestResult::returns`] — so `f64` throughout, and no
//! money is represented. Nothing here crosses into `Decimal`.

use crate::validation::{DeflatedSharpe, Split, deflated_sharpe};
use qip_core::error::{Error, Result};
use qip_core::time::Timestamp;
use qip_numerics::hmm::GaussianHmm;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The fewest observations either side of the boundary may hold.
///
/// Twenty is the floor both [`GaussianHmm::fit`] and [`deflated_sharpe`]
/// impose; stated here so the refusal names the split rather than surfacing
/// as an arithmetic complaint from a function the caller never called.
pub const MINIMUM_OBSERVATIONS: usize = 20;

/// One period's return, with the instant it was true and the instant it
/// became knowable.
///
/// Two instants rather than one because a backtest that cannot answer "what
/// did we know then" is a backtest of a world where data arrives before it
/// exists. The pair is not validated against each other on purpose: an
/// observation knowable before its own instant is exactly the mis-stamped
/// record the split refuses, and refusing it here would leave the split
/// with a check that could never fire.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ReturnObservation {
    /// The instant the period this return describes ended.
    pub at: Timestamp,
    /// The instant the value became readable.
    pub known_at: Timestamp,
    /// The per-period return, a fraction.
    pub value: f64,
}

impl ReturnObservation {
    /// Refuses a value that is not a number; a NaN return is not a bad day,
    /// it is a missing one, and averaging it in would poison every statistic
    /// downstream without an error.
    pub fn new(at: Timestamp, known_at: Timestamp, value: f64) -> Result<Self> {
        if !value.is_finite() {
            return Err(Error::invalid(format!(
                "the return at {at} is {value}, which is not a number; leave the period out rather than record it"
            )));
        }
        Ok(Self {
            at,
            known_at,
            value,
        })
    }
}

/// A holdout boundary fixed before the comparison runs.
///
/// Serialisable on its own so it can be written to the event log at the
/// moment it is declared, and embedded in the [`RegimeComparison`] so the
/// result names the declaration it was scored against. A comparison cannot be
/// run without one — there is no `Default` and no `Option` — because a split
/// chosen after the result is known is a result chosen after the split.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SplitDeclaration {
    /// Observations at or after this instant are the holdout; before it,
    /// the training side the regime model is fitted on.
    pub boundary: Timestamp,
    /// When the boundary was fixed. Must precede the run.
    pub declared_at: Timestamp,
}

impl SplitDeclaration {
    pub fn new(boundary: Timestamp, declared_at: Timestamp) -> Self {
        Self {
            boundary,
            declared_at,
        }
    }
}

/// The parameters both arms share.
///
/// Every field is required and validated; none has a default. The base
/// weight and cost are the same for both arms by construction — they are read
/// from this one value — so the comparison can differ only in the regime term.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ComparisonPolicy {
    /// The fraction of equity the unconditional arm holds every period, and
    /// the most the conditional arm may hold. In `(0, 1]`.
    pub base_weight: f64,
    /// Cost charged per unit of weight turned over, in basis points. Zero is
    /// permitted and is the frictionless reading, which measures the regime
    /// term's information content rather than an achievable return.
    pub turnover_cost_bps: f64,
    /// For annualising the Sharpe ratio.
    pub periods_per_year: f64,
    /// How many conditional variants were tried before this one was scored.
    /// The baseline is charged one trial: it is a fixed rule, and nothing was
    /// searched to find it.
    pub conditional_trials: usize,
}

impl ComparisonPolicy {
    pub fn new(
        base_weight: f64,
        turnover_cost_bps: f64,
        periods_per_year: f64,
        conditional_trials: usize,
    ) -> Result<Self> {
        if !base_weight.is_finite() || base_weight <= 0.0 || base_weight > 1.0 {
            return Err(Error::invalid(format!(
                "a base weight of {base_weight} is not a position bound: it must lie in (0, 1]; \
                 zero compares nothing and above one is leverage this comparison does not model"
            )));
        }
        if !turnover_cost_bps.is_finite() || turnover_cost_bps < 0.0 {
            return Err(Error::invalid(format!(
                "a turnover cost of {turnover_cost_bps}bp is not a cost; state a non-negative figure"
            )));
        }
        if !periods_per_year.is_finite() || periods_per_year <= 0.0 {
            return Err(Error::invalid(format!(
                "{periods_per_year} periods per year cannot annualise anything; state the bar frequency"
            )));
        }
        if conditional_trials == 0 {
            return Err(Error::invalid(
                "at least one conditional variant must have been tried for one to be scored",
            ));
        }
        Ok(Self {
            base_weight,
            turnover_cost_bps,
            periods_per_year,
            conditional_trials,
        })
    }
}

/// The regime term of the sizing rule.
///
/// An enum rather than an `f64` that happens to be one, so the baseline's
/// arm cannot be handed a computed term by accident and the record can say
/// which arithmetic produced each weight.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum RegimeTerm {
    /// The multiplier the regime posterior implies, in `[0, 1]`.
    Conditional(f64),
    /// The baseline: the same rule with the term removed.
    Removed,
}

impl RegimeTerm {
    fn multiplier(self) -> f64 {
        match self {
            Self::Conditional(term) => term,
            Self::Removed => 1.0,
        }
    }
}

/// The one sizing rule both arms use.
///
/// Kept as a free function so that a reader checking the baseline is the
/// treatment minus its term can see the whole of the arithmetic in one line.
pub fn size(base_weight: f64, term: RegimeTerm) -> f64 {
    base_weight * term.multiplier()
}

/// The regime term from a filtered two-state posterior.
///
/// The calm state's volatility over the volatility the posterior mixture
/// implies, capped at one. Fully calm gives exactly one — the conditional arm
/// holds the base weight — and fully turbulent gives `σ_calm / σ_turbulent`,
/// the scaling that would restore the calm state's risk. Never above one: a
/// regime model that levered up on its own confidence would be sizing on the
/// estimate whose error this whole comparison exists to expose.
fn regime_term(model: &GaussianHmm, posterior: [f64; 2]) -> f64 {
    let high = model.high_volatility_state();
    let low = 1 - high;
    let calm_variance = model.variances[low].max(0.0);
    let mixture_variance = posterior[low].max(0.0) * calm_variance
        + posterior[high].max(0.0) * model.variances[high].max(0.0);
    if mixture_variance <= 0.0 || calm_variance <= 0.0 {
        return 1.0;
    }
    (calm_variance / mixture_variance).sqrt().min(1.0)
}

/// What one arm did on the holdout.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ArmResult {
    pub arm: String,
    /// Scored exactly as the lifecycle's holdout gate scores a holdout.
    pub sharpe: DeflatedSharpe,
    /// Mean weight held across the holdout.
    pub average_weight: f64,
    /// Total absolute change in weight, the quantity the cost was charged on.
    pub turnover: f64,
    /// Compounded return over the holdout, after costs.
    pub total_return: f64,
}

/// Both arms, the split they were scored on, and the difference.
///
/// Serialisable in field order, so the record written to the event log is
/// byte-stable, and carrying the fitted model so the conditional arm's
/// weights can be recomputed from the log and the observations alone.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RegimeComparison {
    pub split: SplitDeclaration,
    pub policy: ComparisonPolicy,
    pub run_at: Timestamp,
    /// SHA-256 of the serialised observations, so the record pins the input
    /// it was computed from without carrying it.
    pub observations_digest: String,
    pub training_observations: usize,
    pub holdout_observations: usize,
    /// The regime model as fitted on the training side.
    pub model: GaussianHmm,
    /// Holdout decisions by the regime the filter favoured when they were
    /// taken. A holdout with no turbulent step gave the conditional arm
    /// nothing to condition on, and a verdict from it says nothing about
    /// regimes; this is how a reader tells.
    pub regime_occupancy: BTreeMap<String, usize>,
    pub conditional: ArmResult,
    pub unconditional: ArmResult,
    /// The conditional arm's annualised Sharpe less the baseline's.
    pub advantage: f64,
}

impl RegimeComparison {
    /// The gate's question, answered on the holdout only.
    ///
    /// A strict comparison of the annualised Sharpe ratios. Whether the
    /// margin exceeds its own noise is read from each arm's
    /// [`DeflatedSharpe`], which carries the standard error and the
    /// selection correction; this flag says which arm was ahead, not that
    /// the lead is credible.
    pub fn conditional_beats_unconditional(&self) -> bool {
        self.advantage > 0.0
    }

    /// The fold this comparison scored, in the shape the walk-forward and
    /// purged splits produce, so a consumer that already reads a
    /// [`Split`] reads this one the same way.
    pub fn fold(&self) -> Split {
        let train_end = self.training_observations;
        Split {
            train: (0..train_end).collect(),
            test: (train_end..train_end + self.holdout_observations).collect(),
            purged: 0,
            embargoed: 0,
        }
    }

    pub fn summarise(&self) -> String {
        format!(
            "holdout from {} ({} observations, declared {}): regime-conditional Sharpe {:.2} against unconditional {:.2}, an advantage of {:+.2}; {} turbulent decision(s)",
            self.split.boundary,
            self.holdout_observations,
            self.split.declared_at,
            self.conditional.sharpe.observed,
            self.unconditional.sharpe.observed,
            self.advantage,
            self.regime_occupancy.get("turbulent").copied().unwrap_or(0)
        )
    }

    /// Run both arms on the holdout the declaration fixes.
    ///
    /// Refuses, in order: a declaration not made before `run_at`; two
    /// observations at one instant; a holdout observation knowable before the
    /// boundary; a training observation knowable only after it; and either
    /// side too short to fit or to score.
    pub fn run(
        observations: &[ReturnObservation],
        split: &SplitDeclaration,
        policy: &ComparisonPolicy,
        run_at: Timestamp,
    ) -> Result<Self> {
        if split.declared_at >= run_at {
            return Err(Error::denied(format!(
                "the split was declared at {} and this run is at {}; a boundary fixed at or after \
                 the run was not fixed in advance — declare it, record the declaration, then run",
                split.declared_at, run_at
            )));
        }

        let mut ordered: Vec<ReturnObservation> = observations.to_vec();
        ordered.sort_by_key(|observation| observation.at);
        for pair in ordered.windows(2) {
            if pair[0].at == pair[1].at {
                return Err(Error::invalid(format!(
                    "two observations at {}; a comparison scores one return series, and which of \
                     the two is the period's return is not the comparison's to decide",
                    pair[0].at
                )));
            }
        }

        let train_end = ordered.partition_point(|observation| observation.at < split.boundary);
        let (training, holdout) = ordered.split_at(train_end);

        if let Some(leaked) = holdout
            .iter()
            .find(|observation| observation.known_at < split.boundary)
        {
            return Err(Error::denied(format!(
                "the holdout observation at {} was knowable at {}, before the boundary at {}; \
                 the fit could have read the holdout, so this is not out of sample — move the \
                 boundary to or before {} or correct the observation's knowable instant",
                leaked.at, leaked.known_at, split.boundary, leaked.known_at
            )));
        }
        if let Some(restated) = training
            .iter()
            .find(|observation| observation.known_at > split.boundary)
        {
            return Err(Error::denied(format!(
                "the training observation at {} became knowable at {}, after the boundary at {}; \
                 a fit at the boundary could not have used it — move the boundary to or after {} \
                 or leave the restated observation out explicitly",
                restated.at, restated.known_at, split.boundary, restated.known_at
            )));
        }
        if training.len() < MINIMUM_OBSERVATIONS {
            return Err(Error::invalid(format!(
                "{} observation(s) precede the boundary at {} and the regime model needs at least \
                 {MINIMUM_OBSERVATIONS} to fit; declare a later boundary",
                training.len(),
                split.boundary
            )));
        }
        if holdout.len() < MINIMUM_OBSERVATIONS {
            return Err(Error::invalid(format!(
                "{} observation(s) follow the boundary at {} and a holdout needs at least \
                 {MINIMUM_OBSERVATIONS} to score; declare an earlier boundary",
                holdout.len(),
                split.boundary
            )));
        }

        let values: Vec<f64> = ordered
            .iter()
            .map(|observation| observation.value)
            .collect();
        let model = GaussianHmm::fit(&values[..train_end], 200, 1e-6)?;

        // The filtered posterior at every index, from the observations up to
        // and including it. Which index a decision may read is decided below
        // by knowability, not by position.
        let filtered = model.filter(&values);

        // The longest prefix knowable at each decision. A late observation
        // stalls the prefix until it arrives, so the posterior the decision
        // reads is the one the data at hand supported — never one that
        // quietly used a value published later.
        let mut latest_known = Vec::with_capacity(ordered.len());
        let mut running = Timestamp::EPOCH;
        for observation in &ordered {
            running = running.max(observation.known_at);
            latest_known.push(running);
        }

        let mut conditional_weights = Vec::with_capacity(holdout.len());
        let mut unconditional_weights = Vec::with_capacity(holdout.len());
        let mut occupancy: BTreeMap<String, usize> = BTreeMap::new();
        occupancy.insert("calm".to_string(), 0);
        occupancy.insert("turbulent".to_string(), 0);
        let high = model.high_volatility_state();

        for index in train_end..ordered.len() {
            // The weight for a period is set as the previous period ends, so
            // the posterior it reads is over what was knowable then.
            let decided_at = ordered[index - 1].at;
            let knowable = latest_known
                .partition_point(|known| *known <= decided_at)
                .min(index);
            let posterior = if knowable == 0 {
                model.initial
            } else {
                filtered[knowable - 1]
            };
            let term = regime_term(&model, posterior);
            let label = if posterior[high] > 0.5 {
                "turbulent"
            } else {
                "calm"
            };
            if let Some(count) = occupancy.get_mut(label) {
                *count += 1;
            }
            conditional_weights.push(size(policy.base_weight, RegimeTerm::Conditional(term)));
            unconditional_weights.push(size(policy.base_weight, RegimeTerm::Removed));
        }

        let holdout_returns: Vec<f64> = holdout
            .iter()
            .map(|observation| observation.value)
            .collect();
        let conditional = score_arm(
            "regime_conditional",
            &conditional_weights,
            &holdout_returns,
            policy,
            policy.conditional_trials,
        )?;
        let unconditional = score_arm(
            "unconditional",
            &unconditional_weights,
            &holdout_returns,
            policy,
            1,
        )?;
        let advantage = conditional.sharpe.observed - unconditional.sharpe.observed;

        let encoded = serde_json::to_vec(&ordered).map_err(|error| {
            Error::numeric(format!(
                "the observations could not be serialised for the record's digest: {error}"
            ))
        })?;

        Ok(Self {
            split: *split,
            policy: *policy,
            run_at,
            observations_digest: qip_core::sha256_hex(&encoded),
            training_observations: train_end,
            holdout_observations: holdout.len(),
            model,
            regime_occupancy: occupancy,
            conditional,
            unconditional,
            advantage,
        })
    }
}

/// Apply one arm's weights to the holdout returns, charge its turnover, and
/// score the series the way the holdout gate does.
///
/// Both arms start flat, so the first period charges each its entry; a
/// baseline assumed to be already positioned would have been given a free
/// trade the treatment had to pay for.
fn score_arm(
    arm: &str,
    weights: &[f64],
    returns: &[f64],
    policy: &ComparisonPolicy,
    trials: usize,
) -> Result<ArmResult> {
    let cost_per_unit = policy.turnover_cost_bps / 10_000.0;
    let mut previous = 0.0;
    let mut turnover = 0.0;
    let mut growth = 1.0;
    let mut series = Vec::with_capacity(returns.len());
    for (weight, period_return) in weights.iter().zip(returns) {
        let traded = (weight - previous).abs();
        turnover += traded;
        let net = weight * period_return - cost_per_unit * traded;
        growth *= 1.0 + net;
        series.push(net);
        previous = *weight;
    }
    let sharpe = deflated_sharpe(&series, trials, policy.periods_per_year).map_err(|error| {
        Error::invalid(format!(
            "the {arm} arm's holdout series could not be scored: {}",
            error.message()
        ))
    })?;
    let average_weight = if weights.is_empty() {
        0.0
    } else {
        weights.iter().sum::<f64>() / weights.len() as f64
    };
    Ok(ArmResult {
        arm: arm.to_string(),
        sharpe,
        average_weight,
        turnover,
        total_return: growth - 1.0,
    })
}

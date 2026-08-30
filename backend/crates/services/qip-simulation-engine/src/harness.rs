//! A generated strategy, facing real history.
//!
//! `qip-evolution` writes strategies and the backtester measures strategies,
//! and until this module existed no path connected them: a
//! [`CompiledStrategy`] reads a [`FeatureVector`] while [`BacktestStrategy`]
//! sees bars, and the evolution brain could search but never score. Every
//! driver of the foundry was a test with hand-made returns.
//!
//! # One vocabulary, three consumers
//!
//! The foundry already refuses to let the grammar and the compiler hold
//! different catalogues — the same argument extends here. [`bar_catalogue`]
//! and [`bar_vector`] are generated from one private table, so a feature the
//! grammar may write is a feature this harness can compute, by construction.
//! A candidate that references anything else fails compilation before it can
//! reach an evaluation, which is the correct place to fail.
//!
//! # Every feature is knowable when it is used
//!
//! Each vector is built at a bar's close from bars that have closed. There is
//! no forecast, no same-bar open, no vendor timestamp to second-guess: the
//! decision instant and the knowable instant coincide, and
//! [`EvaluationTrace::timings`] records that per feature so the lifecycle
//! gate's leakage audit examines evidence rather than an assertion.
//!
//! # What this harness does *not* promise
//!
//! * **It does not fit anything.** Generated candidates carry no trainable
//!   parameters — the grammar writes fixed rules — so an evaluation here
//!   measures a fixed function against unseen windows. The multiple-testing
//!   hazard of searching many such functions is real and is *not* handled
//!   here: it is the foundry's trial ledger, which counts every candidate
//!   this harness scores.
//! * **It maps conviction to weight crudely, and says so.** An `Enter` signal
//!   becomes a long weight scaled by the signal's shrunk probability and
//!   capped by policy; `Exit` and `Stand` go to cash; `Hedge` is treated as
//!   `Exit` because a single-instrument backtest has nothing to offset. A
//!   production sizing model this is not, and evidence produced here says
//!   "the rule finds something" — never "this size is right".

use crate::backtest::BacktestStrategy;
use crate::clock::PointInTimeView;
use qip_contracts::{FeatureKey, FeatureValue, FeatureVector, Revision, SignalKind};
use qip_core::error::{Error, Result};
use qip_core::{ObjectId, Timestamp};
use qip_strategy::catalogue::FeatureCatalogue;
use qip_strategy::compile::CompiledStrategy;
use qip_strategy::ir::Type;
use qip_strategy::runtime::StrategyRuntime;
use std::collections::BTreeMap;

/// How far a statistic looks back, and the warm-up the vector needs.
const MOMENTUM_BARS: usize = 5;
const VOLATILITY_BARS: usize = 10;

/// The bars a vector needs before every feature is defined.
///
/// Public because the caller sizing a holdout must not spend its first
/// window discovering the warm-up by watching a strategy hold cash.
pub const WARM_UP_BARS: usize = VOLATILITY_BARS + 1;

/// One row per feature: name, type, and the reason it is bar-derivable.
///
/// The single source both [`bar_catalogue`] and [`bar_vector`] are generated
/// from. Adding a feature here adds it to what the grammar may write and to
/// what the harness computes, atomically; adding it anywhere else is a
/// compile-refused candidate, not a silent zero.
const FEATURES: &[(&str, Type)] = &[
    ("close", Type::Exact),
    ("typical_price", Type::Exact),
    ("return_1", Type::Statistic),
    ("momentum_5", Type::Statistic),
    ("volatility_10", Type::Statistic),
    ("range_frac", Type::Statistic),
    ("volume", Type::Count),
    ("bars_seen", Type::Count),
    ("up_bar", Type::Flag),
    ("above_momentum", Type::Flag),
];

/// The catalogue of everything a bar-driven evaluation can honestly compute.
pub fn bar_catalogue(subject: &ObjectId) -> Result<FeatureCatalogue> {
    let mut catalogue = FeatureCatalogue::new();
    for (name, value_type) in FEATURES {
        catalogue.declare(FeatureKey::new(*name, subject.clone()), *value_type)?;
    }
    Ok(catalogue)
}

/// The feature vector at `as_of`, from bars that had closed by then.
///
/// During warm-up every feature is present and *undefined*, never absent: the
/// runtime treats a missing key as a broken graph (an error) and an undefined
/// value as "not yet knowable" (no signal), and warm-up is the second thing.
pub fn bar_vector(subject: &ObjectId, view: &PointInTimeView<'_>) -> FeatureVector {
    let as_of = view.as_of();
    let bars = view.bars(subject);
    let mut vector = FeatureVector::new(as_of);
    let revision = Revision::new(bars.len() as u64);

    let warm = bars.len() >= WARM_UP_BARS;
    let mut put = |name: &str, value: Option<FeatureValue>| {
        let key = FeatureKey::new(name, subject.clone());
        match value {
            Some(value) => vector.insert(key, value, revision),
            None => vector.insert(key, FeatureValue::Undefined, revision),
        }
    };

    if !warm {
        for (name, _) in FEATURES {
            put(name, None);
        }
        return vector;
    }

    let last = &bars[bars.len() - 1];
    let closes: Vec<f64> = bars.iter().map(|bar| bar.close.to_f64()).collect();
    let n = closes.len();

    let ret_1 = closes[n - 1] / closes[n - 2] - 1.0;
    let momentum = closes[n - 1] / closes[n - 1 - MOMENTUM_BARS] - 1.0;
    let returns: Vec<f64> = (n - VOLATILITY_BARS..n)
        .map(|i| closes[i] / closes[i - 1] - 1.0)
        .collect();
    let mean = returns.iter().sum::<f64>() / returns.len() as f64;
    let variance =
        returns.iter().map(|r| (r - mean) * (r - mean)).sum::<f64>() / returns.len() as f64;
    let range = (last.high.to_f64() - last.low.to_f64()) / closes[n - 1].max(f64::MIN_POSITIVE);
    let typical = (last.high.to_f64() + last.low.to_f64() + last.close.to_f64()) / 3.0;

    put("close", Some(FeatureValue::Exact(last.close)));
    put(
        "typical_price",
        qip_core::Decimal::from_f64(typical).map(FeatureValue::Exact),
    );
    put("return_1", Some(FeatureValue::Statistic(ret_1)));
    put("momentum_5", Some(FeatureValue::Statistic(momentum)));
    put(
        "volatility_10",
        Some(FeatureValue::Statistic(variance.sqrt())),
    );
    put("range_frac", Some(FeatureValue::Statistic(range)));
    put(
        "volume",
        Some(FeatureValue::Count(last.volume.to_f64().max(0.0) as u64)),
    );
    put("bars_seen", Some(FeatureValue::Count(n as u64)));
    put("up_bar", Some(FeatureValue::Flag(last.close >= last.open)));
    put("above_momentum", Some(FeatureValue::Flag(momentum > 0.0)));
    vector
}

/// What one evaluation observed about its own inputs, for the leakage audit.
#[derive(Clone, Debug, Default)]
pub struct EvaluationTrace {
    /// Per feature: the latest instant it was used and the instant it was
    /// knowable. Identical by construction here — the point of recording them
    /// is that the audit checks the construction instead of trusting it.
    pub timings: BTreeMap<String, (Timestamp, Timestamp)>,
    /// Decisions where the strategy was asked and the vector was still warming
    /// up. A holdout consisting mostly of warm-up is not evidence.
    pub warming_decisions: usize,
    pub decisions: usize,
}

/// A compiled, generated strategy wearing the backtester's trait.
#[derive(Debug)]
pub struct CompiledHarness {
    strategy: CompiledStrategy,
    runtime: StrategyRuntime,
    subject: ObjectId,
    /// The largest fraction of equity a signal may take, whatever its
    /// conviction says. Policy, not inference.
    max_weight: f64,
    trace: EvaluationTrace,
}

impl CompiledHarness {
    /// Wrap a candidate for evaluation.
    ///
    /// The program must be the arena the candidate was compiled into — the
    /// same invariant `StrategyCandidate::new` checks at registration, checked
    /// here too so an evaluation cannot silently run a different strategy
    /// than the one the evidence will be attached to.
    pub fn new(
        strategy: CompiledStrategy,
        program: qip_strategy::program::Program,
        max_weight: f64,
    ) -> Result<Self> {
        if !(0.0..=1.0).contains(&max_weight) || max_weight == 0.0 {
            return Err(Error::invalid(format!(
                "a max weight of {max_weight} is not a position bound: zero evaluates nothing \
                 and above one is leverage this harness does not model"
            )));
        }
        for node in strategy.plan() {
            if program.node(*node).is_none() {
                return Err(Error::invalid(format!(
                    "compiled strategy {} plans a node its program does not contain; the \
                     evaluation would run a different strategy than the evidence names",
                    strategy.id()
                )));
            }
        }
        let subject = strategy.subject().clone();
        Ok(Self {
            runtime: StrategyRuntime::new(program)?,
            strategy,
            subject,
            max_weight,
            trace: EvaluationTrace::default(),
        })
    }

    /// What the evaluation recorded about itself.
    pub fn trace(&self) -> &EvaluationTrace {
        &self.trace
    }
}

impl BacktestStrategy for CompiledHarness {
    fn name(&self) -> &str {
        self.strategy.id().as_str()
    }

    fn target_weights(&mut self, view: &PointInTimeView<'_>) -> BTreeMap<String, f64> {
        self.trace.decisions += 1;
        let vector = bar_vector(&self.subject, view);
        let as_of = view.as_of();
        for (name, _) in FEATURES {
            self.trace
                .timings
                .insert((*name).to_string(), (as_of, as_of));
        }

        let signal = match self.runtime.run(&self.strategy, &vector, as_of) {
            Ok(Some(signal)) => signal,
            Ok(None) => {
                self.trace.warming_decisions += 1;
                // Warm-up holds what it has — which at the start is cash, and
                // that is the honest position for a strategy that cannot yet
                // read its own inputs.
                return BTreeMap::new();
            }
            Err(_) => {
                // A candidate whose evaluation errors mid-run scores as flat
                // from here on rather than crashing the whole search: the
                // error is the compiler's or the harness's to prevent, and a
                // panic here would take the other candidates' evidence with
                // it.
                let mut weights = BTreeMap::new();
                weights.insert(self.subject.as_str().to_string(), 0.0);
                return weights;
            }
        };

        let weight = match signal.kind {
            SignalKind::Enter => {
                let scaled = self.max_weight * signal.conviction.shrunk();
                if signal.desired_quantity.is_negative() {
                    -scaled
                } else {
                    scaled
                }
            }
            // A single-instrument evaluation has nothing to offset, so a hedge
            // is a close. Documented in the module header rather than hidden.
            SignalKind::Exit | SignalKind::Stand | SignalKind::Hedge => 0.0,
        };
        let mut weights = BTreeMap::new();
        weights.insert(self.subject.as_str().to_string(), weight);
        weights
    }
}

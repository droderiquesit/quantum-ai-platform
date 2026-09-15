//! Blueprint §14.3's `Testing` gate, wired into the LEARN stage.
//!
//! The gap this closes is narrow and total: `Platform::record_prediction`
//! writes a thesis's falsifiers down at formation, `ThesisOutcome` carries a
//! `falsifiers_triggered` field for them, and every construction site of that
//! field in the kernel builds it as `Vec::new()`. Nothing has ever evaluated a
//! falsifier against anything. A falsifier nothing evaluates is
//! indistinguishable from one that passed, so every hypothesis this platform
//! has formed has read, to everything downstream of it, as though it cleared a
//! gate that was never run.
//!
//! [`Book::review`] runs it, once per cycle, from `Platform::stage_learn`.
//!
//! # What "held out" means here, and why it is the knowable instant
//!
//! The held-out sample for a claim is the part of the world model's `close`
//! series that **became knowable after the claim was formed**. That is a
//! statement about [`qip_world_model::features::FeatureValue::available_at`]
//! and not about `valid_at`, and the difference is the whole point: a daily
//! bar whose bucket opened the morning a claim was written and closed that
//! evening is held-out data, because its close could not be read until the
//! bucket shut; a restatement describing last March but published this morning
//! is *not* held-out data for a claim formed last week, because it is knowable
//! now and was not then. A store with one timestamp cannot tell those apart,
//! which is why this reads the store that has two.
//!
//! The admissibility rules, the refusal on a leaked record and the count of
//! what a partition withheld all live in
//! [`qip_world_model::falsification`]; this module is the wiring, and it
//! deliberately holds no second copy of the rule.
//!
//! # Where the falsifier comes from
//!
//! A hypothesis states its falsifiers as prose, which no evaluation can read.
//! What *is* machine-readable is the claim the same hypothesis already
//! recorded: `Platform::record_prediction` writes a
//! [`ResolutionCriteria::Threshold`] naming the series, the direction and the
//! reference level, beside a `ThesisClaim` stating the magnitude in basis
//! points. The falsifier is that claim's own mirror — the same magnitude, the
//! same reference, the other side — and the magnitude is therefore one the
//! platform chose rather than one this module invented. A claim that the close
//! rises fifty basis points from 100 is contradicted by the close reaching
//! 99.5; nothing here picks the fifty.
//!
//! The prose falsifiers are carried into the record as the statement the
//! threshold stands for, so the refutation register names the sentence a
//! person wrote and not only the arithmetic.
//!
//! # What this does not do
//!
//! It does not promote a surviving hypothesis, size anything, or move capital.
//! §14.3's rows after `Testing` — a supported hypothesis becoming a candidate
//! causal edge, a strategy expressing it, a canary — are not built, and a
//! module that quietly did half of one of them would be worse than the gap.
//! What changes is that a falsifier is now evaluated, that the evaluation is
//! charged against its family's cumulative trial budget, and that a refutation
//! is recorded where the next proposal of the same idea can find it.

use crate::cycle::StageOutcome;
use crate::platform::RecordedPrediction;
use qip_core::time::Timestamp;
use qip_prediction::resolution::{Comparison, ResolutionCriteria};
use qip_world_model::falsification::{
    Breach, FalsificationPass, Falsifier, HeldOut, HypothesisSource, SourceCensus, TrialLedger,
    rolling_statistic,
};
use qip_world_model::world::WorldModel;

/// Held-out observations a falsifier needs before it may report survival.
///
/// Five, because the series this reads is built from bars: fewer than five new
/// closes is under a trading week, and a falsifier that reported survival on
/// one newly knowable close would clear the `Testing` gate on a single tick.
/// Below this the verdict is `Undetermined`, which is a different claim from
/// `Survived` and is reported as one.
pub const FALSIFIER_MIN_OBSERVATIONS: usize = 5;

/// How a claim's observable is read out of the bitemporal store.
///
/// `spread` has no arm, deliberately: the platform publishes a spread
/// observation from its own single-timestamped `spread_history` and records no
/// spread feature in the world model, so a spread claim has no bitemporal
/// series to be held out of. Such a claim is counted and named in the summary
/// rather than settled against a series that cannot say when it was knowable.
enum Observable {
    /// Read the `close` series directly.
    Close,
    /// Derive realised volatility from held-out closes, one window at a time.
    ///
    /// **Not the world model's own `realised_volatility_20d`.** That feature
    /// has no production writer — `WorldModel::recompute_volatility` is called
    /// from nothing in the kernel — so a gate that read it would evaluate an
    /// always-empty series and report `Undetermined` forever. A control that
    /// cannot fire reads as protection and is not, and this platform's
    /// detectors raise a volatility-shift claim far more often than a price
    /// one, so that arm is the *common* case rather than an edge.
    VolatilityFromCloses,
}

impl Observable {
    fn parse(observable: &str) -> Option<Self> {
        match observable {
            "close" => Some(Self::Close),
            "volatility" => Some(Self::VolatilityFromCloses),
            _ => None,
        }
    }

    /// The name the falsifier records for the series it read, so a refusal
    /// names what was actually read rather than what a claim asked for.
    const fn series(&self) -> &'static str {
        match self {
            Self::Close => "close",
            Self::VolatilityFromCloses => "volatility_from_close",
        }
    }
}

/// Closes needed for one realised-volatility observation.
///
/// [`crate::platform::VOLATILITY_CLAIM_WINDOW`] log returns, and *n* returns
/// need *n + 1* closes. By reference rather than restated, because a claim
/// settled over a different window from the one the detector measured is
/// settled against a number nobody claimed anything about.
const VOLATILITY_WINDOW_CLOSES: usize = crate::platform::VOLATILITY_CLAIM_WINDOW + 1;

/// Realised volatility over one window of closes.
///
/// The standard deviation of the window's log returns, unannualised — the
/// same arithmetic `Platform::published_observations` settles a volatility
/// claim with, deliberately, so that the falsifier and the settlement are
/// about one quantity rather than two that happen to share a name. An
/// annualised falsifier against an unannualised claim would be out by a
/// factor of nearly sixteen and would refute almost everything.
fn realised_volatility(closes: &[f64]) -> f64 {
    let returns = qip_numerics::stats::log_returns(closes);
    qip_numerics::stats::stddev(&returns)
}

/// The trial book the §14.3 `Testing` gate keeps across cycles.
///
/// Held by [`crate::platform::Platform`] rather than rebuilt per cycle,
/// because a *cumulative* trial budget that reset every cycle would be a
/// budget of one and a control that cannot fire. The census is rebuilt every
/// cycle: it describes what proposed this cycle, not since assembly.
#[derive(Debug)]
pub struct Book {
    ledger: TrialLedger,
    last: Option<FalsificationPass>,
}

impl Default for Book {
    fn default() -> Self {
        Self::new()
    }
}

impl Book {
    pub fn new() -> Self {
        Self {
            ledger: TrialLedger::new(),
            last: None,
        }
    }

    /// Cumulative trials and recorded refutations, for an operator and for a
    /// test. Nothing in the loop reads it to decide anything.
    pub const fn ledger(&self) -> &TrialLedger {
        &self.ledger
    }

    /// What the most recent pass found. `None` before the first LEARN stage.
    pub const fn last_pass(&self) -> Option<&FalsificationPass> {
        self.last.as_ref()
    }

    /// Evaluate every open claim's falsifier against held-out data and say
    /// what happened.
    ///
    /// **Always appends a line, including when it tested nothing.** A platform
    /// whose claims have not yet reached a second cycle is the only state a
    /// fresh deployment is ever in, and a gate that went silent there would be
    /// indistinguishable from a gate nobody wired — which is precisely the
    /// defect this module exists to end, so reproducing it in the reporting
    /// would be the same bug one layer out.
    ///
    /// A refusal from the ledger is a stage problem rather than a stage
    /// failure, for the reason every other LEARN step gives: the cycle has
    /// happened, and an evaluation that could not run is a fact about this
    /// cycle rather than a reason to lose the rest of the stage's account.
    pub fn review(
        &mut self,
        world: &WorldModel,
        predictions: &[RecordedPrediction],
        now: Timestamp,
        outcome: StageOutcome,
    ) -> StageOutcome {
        let mut pass = FalsificationPass::default();
        let mut census = SourceCensus::new();
        let mut problems: Vec<String> = Vec::new();

        let open: Vec<&RecordedPrediction> = predictions
            .iter()
            .filter(|prediction| prediction.is_open())
            .collect();
        pass.open_claims = open.len();

        for prediction in open {
            // Every hypothesis that reaches a recorded prediction was
            // synthesised from a DISCOVER-stage anomaly — `Platform::synthesise`
            // returns `Ok(None)` without one — so the provenance is structural
            // rather than guessed. A second producer of hypotheses would have
            // to record its own source here; until one exists, claiming any
            // other source would put a number against a path that does not run.
            census.record(HypothesisSource::DetectedAnomaly);

            let Some(claim) = prediction.claim.as_ref() else {
                pass.unevaluable += 1;
                continue;
            };
            let ResolutionCriteria::Threshold {
                metric,
                comparison,
                value,
            } = &prediction.proposition.criteria
            else {
                pass.unevaluable += 1;
                continue;
            };
            let Some((observable, subject)) = metric.split_once(':') else {
                pass.unevaluable += 1;
                continue;
            };
            let Some(observable) = Observable::parse(observable) else {
                pass.unevaluable += 1;
                continue;
            };
            // A claim formed this cycle has no window yet: nothing has become
            // knowable since it was written. Not a problem — it is what the
            // first cycle of every claim looks like — and counted so the
            // summary says how many are waiting rather than falling silent.
            let Ok(boundary) = HeldOut::between(claim.formed_at, now) else {
                pass.unevaluable += 1;
                continue;
            };

            let Some(falsifier) = mirror_falsifier(claim, observable.series(), *comparison, value)
            else {
                pass.unevaluable += 1;
                continue;
            };

            // The store answers with everything knowable by `now`; the
            // boundary then withholds everything the claim could already see
            // when it was formed. Two filters, because they are two different
            // questions, and the second is the one no single-timestamp store
            // can ask.
            let closes = world.features().history("close", subject, now);
            let (held_out_closes, leakage) = boundary.partition(&closes);
            pass.leakage.absorb(leakage);

            // Kept alive across the match so the borrowed sample below can
            // point into it. Empty and unread on the `Close` arm.
            let derived: Vec<qip_world_model::features::FeatureValue>;
            let sample: Vec<&qip_world_model::features::FeatureValue> = match observable {
                Observable::Close => held_out_closes,
                Observable::VolatilityFromCloses => {
                    // Every close in every window is already held out, so
                    // every statistic derived from them is too — which is the
                    // property `rolling_statistic` exists to keep and a
                    // per-record filter applied after the window would not.
                    match rolling_statistic(
                        &held_out_closes,
                        VOLATILITY_WINDOW_CLOSES,
                        realised_volatility,
                    ) {
                        Ok(values) => {
                            derived = values;
                            derived.iter().collect()
                        }
                        Err(error) => {
                            problems.push(format!(
                                "the held-out volatility for {} could not be derived: {}",
                                claim.hypothesis_id,
                                error.message()
                            ));
                            continue;
                        }
                    }
                }
            };

            match self
                .ledger
                .test(&claim.class, &falsifier, &boundary, &sample)
            {
                Ok(verdict) => pass.observe(&verdict),
                Err(error) => problems.push(format!(
                    "the falsifier for {} was not evaluated: {}",
                    claim.hypothesis_id,
                    error.message()
                )),
            }
        }

        let detail = format!(
            "{}; {}; {}",
            outcome.detail,
            pass.describe(),
            census.describe()
        );
        self.last = Some(pass);
        let mut outcome = StageOutcome { detail, ..outcome };
        for problem in problems {
            outcome = outcome.with_problem(problem);
        }
        outcome
    }
}

/// The claim's own mirror, as something evaluable.
///
/// The claim says the series reaches `reference × (1 ± m)`; the falsifier says
/// it reached `reference × (1 ∓ m)` instead. The magnitude `m` is the claim's
/// own `expected_move_bps`, so the level is derived from what the platform
/// stated and not from a tolerance chosen here.
///
/// `None` for a comparison with no direction and for a zero reference. A claim
/// against a series standing at zero has no magnitude to mirror — every move
/// is infinitely many basis points of it — and inventing a level would make
/// the platform refutable on a question it never asked.
///
/// **Money to statistic.** `reference` is a [`qip_core::Decimal`] because it is
/// a price. The world model's feature store holds `close` as `f64`
/// (`WorldModel::absorb_bars` converts it on the way in), so the comparison
/// has to happen in `f64` and this is the line where the exact figure becomes
/// a statistic. Nothing downstream of here is money, and nothing here is
/// arithmetic on a position, a cost or a P&L.
fn mirror_falsifier(
    claim: &qip_learning_engine::evaluation::ThesisClaim,
    feature: &str,
    comparison: Comparison,
    reference: &qip_core::Decimal,
) -> Option<Falsifier> {
    let breach = match comparison {
        // The claim expects a rise; it is contradicted by a fall of the same
        // size.
        Comparison::GreaterThan | Comparison::AtLeast => Breach::FallsTo,
        Comparison::LessThan | Comparison::AtMost => Breach::RisesTo,
        // Listed rather than wildcarded so that an equality claim added to
        // `record_prediction`'s table has to say which way its mirror points.
        Comparison::EqualTo => return None,
    };
    let reference = reference.to_f64();
    if !reference.is_finite() || reference.abs() <= f64::EPSILON {
        return None;
    }
    let magnitude = claim.expected_move_bps.abs() / 10_000.0;
    if !magnitude.is_finite() || magnitude <= 0.0 {
        return None;
    }
    let level = match breach {
        Breach::FallsTo => reference * (1.0 - magnitude),
        Breach::RisesTo => reference * (1.0 + magnitude),
    };
    // The prose the hypothesis's author wrote, where there is any, so the
    // refutation register names a sentence and not only a number. The
    // arithmetic is appended rather than substituted: a register holding only
    // prose could not tell two differently-sized versions of the same idea
    // apart.
    let statement = match claim.falsifiers.first() {
        Some(stated) => format!("{stated} ({} {level:.6})", breach.as_str()),
        None => format!("{} {} {level:.6}", claim.subject, breach.as_str()),
    };
    // The series named on the falsifier is the one the sample was read from,
    // so a refusal from the ledger names the series that was actually read
    // rather than the one this function assumed.
    Falsifier::new(
        statement,
        feature,
        &claim.subject,
        breach,
        level,
        FALSIFIER_MIN_OBSERVATIONS,
    )
    .ok()
}

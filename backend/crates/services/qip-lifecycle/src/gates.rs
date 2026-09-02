//! One gate per rung, each demanding the evidence that rung is about.
//!
//! Every gate answers the same question — may this strategy take one step up —
//! and answers it the same way: run every check, and fail if any single check
//! failed. There is deliberately no score. A weighted score lets a strong
//! backtest buy its way past a leakage finding, and the whole reason a
//! leakage finding matters is that it makes the backtest meaningless. Nothing
//! a strong number can be weighed against.
//!
//! Gates recompute rather than trust. [`HoldoutGate`] takes the holdout return
//! series and the cross-validation parameters and re-derives the deflated
//! Sharpe and the fold structure itself, using
//! [`qip_simulation_engine::validation`]. A gate that read a submitted Sharpe
//! ratio would be checking a spreadsheet.
//!
//! The one number the holdout gate cannot recompute from the evidence is the
//! trial count, and it is the number that decides what the deflated Sharpe
//! means. So the gate does not take it from the evidence's own run: it reads
//! the family's lifetime count from the [`TrialAccount`] the ledger's trial
//! book charged for this evaluation, and an evaluation with no account fails
//! the `lifetime_trial_count_known` check. Unknown is not zero.
//!
//! Missing evidence fails rather than errors. "Not submitted" and "submitted
//! and inadequate" are the same answer at a gate, and returning an error for
//! one of them would tempt a caller to treat it as a retryable problem.

use crate::band::HoldoutBand;
use crate::evidence::{HoldoutEvidence, StrategyEvidence};
use crate::trials::TrialAccount;
use qip_contracts::gate::{GateOutcome, GateStage};
use qip_contracts::governance::Approval;
use qip_core::error::{Error, Result};
use qip_core::{Decimal, Duration, Timestamp};
use qip_numerics::stats;
use qip_simulation_engine::validation::{
    DeflatedSharpe, PurgedSplit, assess_overfitting, deflated_sharpe,
};

/// A rung's admission test.
///
/// Implementors return a [`GateOutcome`] rather than a `Result` because a gate
/// has no failure mode distinct from "the strategy does not pass": evidence it
/// cannot parse, evidence it was not given and evidence that falls short all
/// mean the same thing to the caller, which is that the promotion does not
/// happen.
pub trait Gate {
    /// The rung this gate admits to.
    fn stage(&self) -> GateStage;

    /// Run every check and report each one.
    fn evaluate(&self, evidence: &StrategyEvidence, now: Timestamp) -> GateOutcome;

    /// Run every check and hand back what the run produced besides a verdict.
    ///
    /// Only the holdout gate produces anything: the [`HoldoutBand`] its
    /// validation defines. The default is the outcome alone, so a gate that
    /// produces nothing does not have to say so.
    fn admit(&self, evidence: &StrategyEvidence, now: Timestamp) -> Admission {
        Admission {
            outcome: self.evaluate(evidence, now),
            band: None,
        }
    }
}

/// A gate's verdict together with what its validation produced.
///
/// The ledger takes this rather than a bare [`GateOutcome`] so the holdout
/// band travels with the admission it belongs to and cannot be recorded
/// against a different one — or forgotten, since the ledger refuses a holdout
/// admission without it.
#[derive(Clone, Debug, PartialEq)]
pub struct Admission {
    pub outcome: GateOutcome,
    /// Present only for a holdout admission whose Sharpe was deflated.
    pub band: Option<HoldoutBand>,
}

/// Thresholds the holdout rung applies.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HoldoutPolicy {
    /// Observations below which a Sharpe ratio is not worth deflating.
    pub minimum_observations: usize,
    /// Fraction of in-sample performance that must survive out of sample.
    pub minimum_retained: f64,
    /// Observations that must be embargoed after each test fold.
    pub minimum_embargo: usize,
    pub minimum_folds: usize,
}

impl Default for HoldoutPolicy {
    fn default() -> Self {
        // A year of daily observations is the smallest sample where a Sharpe
        // ratio's standard error is small enough to say anything; three folds
        // is the smallest k where a fold can be dropped and two remain; a
        // one-observation embargo is the smallest that is not zero, and zero
        // is the case this rung exists to catch.
        Self {
            minimum_observations: 250,
            minimum_retained: 0.3,
            minimum_embargo: 1,
            minimum_folds: 3,
        }
    }
}

/// Performance on data held out of fitting, honestly measured.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct HoldoutGate {
    pub policy: HoldoutPolicy,
}

impl HoldoutGate {
    pub fn new(policy: HoldoutPolicy) -> Self {
        Self { policy }
    }

    /// The count this evaluation deflates against, or why it has none.
    ///
    /// Three refusals, each naming the act that would clear it. No account:
    /// the family's lifetime count is unknown, and the gate will not stand in
    /// for it with the run's own number. An account whose charge disagrees
    /// with the evidence: one of the two describes a different run. A count
    /// too large for the deflation arithmetic: refuse rather than truncate.
    fn charged_trials(holdout: &HoldoutEvidence, account: Option<&TrialAccount>) -> Result<usize> {
        let account = account.ok_or_else(|| {
            Error::denied(
                "the lifetime trial count is unknown: no trial account was charged for this \
                 evaluation. Enrol the strategy in its family with `TrialBook::enrol` and \
                 promote through `attempt_promotion`, which charges this run's trials to the \
                 family's lifetime count before the gate reads it; an unknown count is not zero",
            )
        })?;
        let reported = u64::try_from(holdout.trials).map_err(|_| {
            Error::numeric(format!(
                "{} trials does not fit the trial account",
                holdout.trials
            ))
        })?;
        if account.this_run() != reported {
            return Err(Error::invalid(format!(
                "the trial account charged {} trial(s) for this run but the holdout evidence \
                 reports {}; the account and the evidence describe different runs",
                account.this_run(),
                reported
            )));
        }
        usize::try_from(account.lifetime()).map_err(|_| {
            Error::numeric(format!(
                "a lifetime count of {} trials cannot be deflated against",
                account.lifetime()
            ))
        })
    }

    /// The deflated Sharpe exactly as the gate reads it: the holdout series,
    /// corrected against the family's lifetime trial count.
    ///
    /// Public so a caller can see the statistic behind the verdict rather
    /// than re-deriving it, and so a test can prove the count used was the
    /// lifetime one.
    pub fn deflated(&self, evidence: &StrategyEvidence) -> Result<DeflatedSharpe> {
        let holdout = evidence
            .holdout
            .as_ref()
            .ok_or_else(|| Error::invalid("no holdout evidence was submitted"))?;
        let trials = Self::charged_trials(holdout, evidence.trial_account.as_ref())?;
        deflated_sharpe(&holdout.holdout_returns, trials, holdout.periods_per_year)
    }

    /// Report what the deflated Sharpe says, respecting the two regimes its
    /// probability has.
    ///
    /// Below the selection threshold the probability rises with uncertainty
    /// rather than falling, so it is not a confidence and is not quoted —
    /// see [`DeflatedSharpe::clears_selection_threshold`]. The gate reads a
    /// sub-threshold result as a failure of the test, full stop.
    fn record_deflated(outcome: GateOutcome, deflated: &DeflatedSharpe) -> GateOutcome {
        let clears = deflated.clears_selection_threshold();
        let outcome = outcome.record(
            "deflated_sharpe_above_selection",
            clears,
            format!(
                "Sharpe {:.2} against {:.2} expected from {} trial(s) alone",
                deflated.observed, deflated.expected_maximum, deflated.trials
            ),
        );
        if !clears {
            return outcome.record(
                "deflated_sharpe_credible",
                false,
                "below the selection threshold the deflated probability rises with \
                 uncertainty instead of falling, so it is not a confidence and is not read"
                    .to_string(),
            );
        }
        outcome.record(
            "deflated_sharpe_credible",
            deflated.is_credible(),
            deflated.summarise(),
        )
    }
}

impl Gate for HoldoutGate {
    fn stage(&self) -> GateStage {
        GateStage::Holdout
    }

    fn evaluate(&self, evidence: &StrategyEvidence, now: Timestamp) -> GateOutcome {
        self.admit(evidence, now).outcome
    }

    fn admit(&self, evidence: &StrategyEvidence, now: Timestamp) -> Admission {
        let outcome = GateOutcome::new(GateStage::Holdout, now);
        let Some(holdout) = evidence.holdout.as_ref() else {
            return Admission {
                outcome: outcome.record(
                    "holdout_evidence_present",
                    false,
                    "no holdout evidence was submitted",
                ),
                band: None,
            };
        };

        let observations = holdout.holdout_returns.len();
        let mut outcome = outcome.record(
            "holdout_sample_adequate",
            observations >= self.policy.minimum_observations,
            format!(
                "{observations} held-out observations against a {} minimum",
                self.policy.minimum_observations
            ),
        );

        // The count comes from the trial book's charge, never from the run.
        // Recorded as its own check so a refusal for an unknown count reads
        // as what it is rather than as a Sharpe that could not be computed.
        let charged = Self::charged_trials(holdout, evidence.trial_account.as_ref());
        outcome = outcome.record(
            "lifetime_trial_count_known",
            charged.is_ok(),
            match (&charged, evidence.trial_account.as_ref()) {
                (Ok(_), Some(account)) => account.describe(),
                (Ok(_), None) => "no account".to_string(),
                (Err(error), _) => error.message().to_string(),
            },
        );

        // The band is defined from the same statistic the admission rests
        // on, so the interval the strategy is later held inside of is the
        // interval this evidence supports and not one written afterwards.
        let mut band = None;
        outcome = match charged.and_then(|trials| {
            deflated_sharpe(&holdout.holdout_returns, trials, holdout.periods_per_year)
        }) {
            Ok(deflated) => {
                let outcome = Self::record_deflated(outcome, &deflated);
                match HoldoutBand::from_deflated(&deflated, holdout.periods_per_year, now) {
                    Ok(defined) => {
                        let outcome =
                            outcome.record("holdout_band_defined", true, defined.describe());
                        band = Some(defined);
                        outcome
                    }
                    Err(error) => outcome.record(
                        "holdout_band_defined",
                        false,
                        format!("no holdout band could be defined: {}", error.message()),
                    ),
                }
            }
            Err(error) => outcome
                .record(
                    "deflated_sharpe_above_selection",
                    false,
                    format!(
                        "the Sharpe ratio could not be deflated: {}",
                        error.message()
                    ),
                )
                .record(
                    "deflated_sharpe_credible",
                    false,
                    "no deflated Sharpe to read",
                )
                .record(
                    "holdout_band_defined",
                    false,
                    "no deflated Sharpe to define a band from",
                ),
        };

        // Rebuild the folds the run claims to have used. A run that reports no
        // purging and no embargo produced folds a k-fold splitter would not
        // have produced, and the comparison is what makes the claim
        // falsifiable rather than decorative.
        let run = holdout.cross_validation;
        outcome = match PurgedSplit::new(run.folds, run.label_horizon, run.embargo)
            .and_then(|split| split.split(run.observations))
        {
            Ok(splits) => {
                let purged: usize = splits.iter().map(|s| s.purged).sum();
                let embargoed: usize = splits.iter().map(|s| s.embargoed).sum();
                let matches = purged == run.purged && embargoed == run.embargoed;
                outcome.record(
                    "purging_and_embargo_applied",
                    matches
                        && run.embargo >= self.policy.minimum_embargo
                        && run.folds >= self.policy.minimum_folds,
                    format!(
                        "reported {} purged and {} embargoed; a {}-fold split with a \
                         {}-observation label horizon and a {}-observation embargo over {} \
                         observations drops {purged} and {embargoed}",
                        run.purged,
                        run.embargoed,
                        run.folds,
                        run.label_horizon,
                        run.embargo,
                        run.observations
                    ),
                )
            }
            Err(error) => outcome.record(
                "purging_and_embargo_applied",
                false,
                format!(
                    "the reported cross-validation cannot be reconstructed: {}",
                    error.message()
                ),
            ),
        };

        outcome = match assess_overfitting(&holdout.in_sample_folds, &holdout.out_of_sample_folds) {
            Ok(report) => outcome.record(
                "out_of_sample_performance_retained",
                !report.looks_overfitted() && report.degradation >= self.policy.minimum_retained,
                report.summarise(),
            ),
            Err(error) => outcome.record(
                "out_of_sample_performance_retained",
                false,
                format!("folds could not be compared: {}", error.message()),
            ),
        };

        let leakage = &holdout.leakage;
        let outcome = outcome.record(
            "no_leakage",
            leakage.is_clean(),
            if leakage.timings.is_empty() {
                "no feature timings were audited, so the run has not been checked for leakage"
                    .to_string()
            } else {
                let findings = leakage.findings();
                if findings.is_empty() {
                    format!(
                        "{} feature timings audited, none leaking",
                        leakage.timings.len()
                    )
                } else {
                    findings.join("; ")
                }
            },
        );
        Admission { outcome, band }
    }
}

/// Thresholds the paper rung applies.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PaperPolicy {
    pub minimum_fills: usize,
    /// How far mean realised cost may exceed the backtest's assumption.
    pub mean_cost_tolerance: f64,
    /// How far the ninetieth-percentile realised cost may exceed it.
    pub tail_cost_tolerance: f64,
    /// Share of intended orders the simulator may refuse as too large.
    pub maximum_unfillable_share: f64,
}

impl Default for PaperPolicy {
    fn default() -> Self {
        // A quarter over the assumption on the mean and double on the tail:
        // costs are the deduction that decides whether an edge is real, and a
        // strategy priced at 5bp that pays 10bp is a different strategy. The
        // tail gets the looser bound because a handful of expensive fills is
        // normal and a consistently expensive book is not.
        Self {
            minimum_fills: 100,
            mean_cost_tolerance: 0.25,
            tail_cost_tolerance: 1.0,
            maximum_unfillable_share: 0.10,
        }
    }
}

/// Simulated execution against live data, priced honestly.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PaperGate {
    pub policy: PaperPolicy,
}

impl PaperGate {
    pub fn new(policy: PaperPolicy) -> Self {
        Self { policy }
    }
}

impl Gate for PaperGate {
    fn stage(&self) -> GateStage {
        GateStage::Paper
    }

    fn evaluate(&self, evidence: &StrategyEvidence, now: Timestamp) -> GateOutcome {
        let outcome = GateOutcome::new(GateStage::Paper, now);
        let Some(paper) = evidence.paper.as_ref() else {
            return outcome.record(
                "paper_evidence_present",
                false,
                "no paper-trading evidence was submitted",
            );
        };

        let outcome = outcome
            .record(
                "ran_against_live_data",
                paper.against_live_data,
                if paper.against_live_data {
                    "the run consumed the live feed".to_string()
                } else {
                    "the run consumed a recording, which re-tests the research path rather \
                     than the live one"
                        .to_string()
                },
            )
            .record(
                "sample_of_fills_adequate",
                paper.filled_orders >= self.policy.minimum_fills,
                format!(
                    "{} fills against a {} minimum",
                    paper.filled_orders, self.policy.minimum_fills
                ),
            )
            .record(
                "costs_were_modelled",
                paper.assumed_cost_bps > 0.0,
                format!(
                    "the backtest assumed {:.2}bp per fill",
                    paper.assumed_cost_bps
                ),
            );

        // The comparison the rung exists for: what the backtest assumed
        // against what the run actually paid. A strategy whose edge is
        // smaller than the gap between the two has no edge.
        let realised = &paper.realised_cost_bps;
        let outcome = if realised.is_empty() {
            outcome.record(
                "realised_cost_matches_assumption",
                false,
                "no realised costs were recorded to compare against the assumption",
            )
        } else {
            let mean = stats::mean(realised);
            let tail = stats::quantile(realised, 0.9);
            let mean_bound = paper.assumed_cost_bps * (1.0 + self.policy.mean_cost_tolerance);
            let tail_bound = paper.assumed_cost_bps * (1.0 + self.policy.tail_cost_tolerance);
            outcome.record(
                "realised_cost_matches_assumption",
                mean <= mean_bound && tail <= tail_bound,
                format!(
                    "realised {mean:.2}bp mean and {tail:.2}bp at the 90th percentile against \
                     an assumed {:.2}bp (bounds {mean_bound:.2}bp and {tail_bound:.2}bp)",
                    paper.assumed_cost_bps
                ),
            )
        };

        let unfillable = paper.unfillable_share();
        outcome
            .record(
                "within_modelled_capacity",
                paper.peak_participation <= paper.modelled_participation_limit,
                format!(
                    "peak participation {:.1}% against a {:.1}% calibration limit",
                    paper.peak_participation * 100.0,
                    paper.modelled_participation_limit * 100.0
                ),
            )
            .record(
                "orders_are_fillable",
                unfillable <= self.policy.maximum_unfillable_share,
                format!(
                    "{:.1}% of intended orders were too large to price, against a {:.1}% bound",
                    unfillable * 100.0,
                    self.policy.maximum_unfillable_share * 100.0
                ),
            )
    }
}

/// Thresholds the shadow rung applies.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShadowPolicy {
    pub minimum_decisions: usize,
    /// Fraction of paired decisions whose direction must match.
    pub minimum_agreement: f64,
    /// Median relative size divergence tolerated.
    pub maximum_size_divergence: f64,
    /// Decision latency beyond which the live path is not the path that was
    /// tested.
    pub maximum_decision_latency: Duration,
}

impl Default for ShadowPolicy {
    fn default() -> Self {
        // 98% agreement, not 90%: a one-in-ten disagreement between the live
        // and research paths means a tenth of the backtest describes trades
        // that would not have happened, and the backtest is the entire
        // argument for the strategy. The residual 2% is for genuine ties at a
        // threshold, not for a path that computes something else.
        Self {
            minimum_decisions: 200,
            minimum_agreement: 0.98,
            maximum_size_divergence: 0.05,
            maximum_decision_latency: Duration::from_millis(250),
        }
    }
}

/// Live decisions computed and discarded, checked against the backtest.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ShadowGate {
    pub policy: ShadowPolicy,
}

impl ShadowGate {
    pub fn new(policy: ShadowPolicy) -> Self {
        Self { policy }
    }
}

impl Gate for ShadowGate {
    fn stage(&self) -> GateStage {
        GateStage::Shadow
    }

    fn evaluate(&self, evidence: &StrategyEvidence, now: Timestamp) -> GateOutcome {
        let outcome = GateOutcome::new(GateStage::Shadow, now);
        let Some(shadow) = evidence.shadow.as_ref() else {
            return outcome.record(
                "shadow_evidence_present",
                false,
                "no shadow evidence was submitted",
            );
        };

        let agreement = shadow.agreement_rate();
        let divergence = shadow.median_size_divergence();
        outcome
            .record(
                "orders_were_discarded",
                !shadow.orders_reached_a_venue,
                if shadow.orders_reached_a_venue {
                    "a shadow order reached a venue, so this run was unapproved live trading \
                     rather than a shadow run"
                        .to_string()
                } else {
                    "no shadow order reached a venue".to_string()
                },
            )
            .record(
                "sample_of_decisions_adequate",
                shadow.decisions.len() >= self.policy.minimum_decisions,
                format!(
                    "{} paired decisions against a {} minimum",
                    shadow.decisions.len(),
                    self.policy.minimum_decisions
                ),
            )
            .record(
                "live_path_agrees_with_backtest",
                !shadow.decisions.is_empty() && agreement >= self.policy.minimum_agreement,
                format!(
                    "{:.2}% direction agreement against a {:.2}% bar; {} disagreement(s)",
                    agreement * 100.0,
                    self.policy.minimum_agreement * 100.0,
                    shadow.disagreements().len()
                ),
            )
            .record(
                "live_path_agrees_on_size",
                divergence <= self.policy.maximum_size_divergence,
                format!(
                    "median size divergence {:.2}% against a {:.2}% bound",
                    divergence * 100.0,
                    self.policy.maximum_size_divergence * 100.0
                ),
            )
            .record(
                "decision_latency_within_bound",
                shadow.decision_latency_p99 <= self.policy.maximum_decision_latency,
                format!(
                    "p99 decision latency {}ms against a {}ms bound",
                    shadow.decision_latency_p99.as_millis(),
                    self.policy.maximum_decision_latency.as_millis()
                ),
            )
    }
}

/// Thresholds the pilot rung applies.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PilotPolicy {
    /// Kill conditions that must be stated before capital moves.
    pub minimum_kill_conditions: usize,
    /// Longest envelope life a pilot may be granted.
    pub maximum_envelope_life: Duration,
    /// Largest envelope a pilot may be granted, whatever the allocator says.
    pub maximum_pilot_gross: Decimal,
}

impl Default for PilotPolicy {
    fn default() -> Self {
        Self {
            // One kill condition is a single point of failure in the stopping
            // logic; two is the minimum that can cover both a loss path and a
            // behaviour path.
            minimum_kill_conditions: 2,
            // A month is long enough to gather live evidence and short enough
            // that renewing is a decision somebody makes rather than a
            // formality. A ceiling on the grant itself, rather than only on
            // what the allocator computes, means a mis-sized allocation cannot
            // reach a pilot: the gate is the second, independent bound.
            maximum_envelope_life: Duration::from_days(30),
            maximum_pilot_gross: Decimal::from_int(1_000_000),
        }
    }
}

/// Live with capital, bounded, and with a person's name on it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PilotGate {
    pub policy: PilotPolicy,
}

impl PilotGate {
    pub fn new(policy: PilotPolicy) -> Self {
        Self { policy }
    }
}

/// Describe an approval against the dual-control requirement.
fn dual_approval_detail(approval: Option<&Approval>) -> (bool, String) {
    match approval {
        None => (false, "no human approval was recorded".to_string()),
        Some(approval) if !approval.is_dual() => (
            false,
            format!(
                "{} approved alone; a stage that can lose money needs two names",
                approval.approver
            ),
        ),
        Some(approval) => (
            true,
            format!(
                "{} approved, countersigned by {}",
                approval.approver,
                approval.second_approver.as_deref().unwrap_or("nobody")
            ),
        ),
    }
}

impl Gate for PilotGate {
    fn stage(&self) -> GateStage {
        GateStage::Pilot
    }

    fn evaluate(&self, evidence: &StrategyEvidence, now: Timestamp) -> GateOutcome {
        let outcome = GateOutcome::new(GateStage::Pilot, now);
        let Some(pilot) = evidence.pilot.as_ref() else {
            return outcome.record(
                "pilot_evidence_present",
                false,
                "no pilot evidence was submitted",
            );
        };

        let (dual, detail) = dual_approval_detail(pilot.approval.as_ref());
        let outcome = outcome.record("dual_human_approval", dual, detail);

        let outcome = match pilot.envelope.as_ref() {
            None => outcome.record(
                "bounded_capital_envelope",
                false,
                "no capital envelope was issued, so nothing bounds what the cell may commit",
            ),
            Some(envelope) => {
                let life = envelope.expires_at().since(now);
                let bounded = envelope.gross_limit() <= self.policy.maximum_pilot_gross
                    && envelope.is_live(now)
                    && life <= self.policy.maximum_envelope_life;
                outcome.record(
                    "bounded_capital_envelope",
                    bounded,
                    format!(
                        "gross limit {} against a {} pilot ceiling, expiring in {:.1} day(s) \
                         against a {:.1} day ceiling",
                        envelope.gross_limit(),
                        self.policy.maximum_pilot_gross,
                        life.as_days_f64(),
                        self.policy.maximum_envelope_life.as_days_f64()
                    ),
                )
            }
        };

        outcome.record(
            "kill_conditions_stated",
            pilot.kill_conditions.len() >= self.policy.minimum_kill_conditions,
            if pilot.kill_conditions.is_empty() {
                "no kill conditions were stated".to_string()
            } else {
                pilot
                    .kill_conditions
                    .iter()
                    .map(super::evidence::KillCondition::describe)
                    .collect::<Vec<_>>()
                    .join("; ")
            },
        )
    }
}

/// Thresholds the scaled rung applies.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScaledPolicy {
    /// How long the pilot must have run before scaling is considered.
    pub minimum_pilot_duration: Duration,
    pub minimum_pilot_observations: usize,
    /// Minimum realised Sharpe over the pilot.
    pub minimum_pilot_sharpe: f64,
    /// Fraction of modelled capacity the proposed size may occupy.
    pub maximum_capacity_utilisation: f64,
}

impl Default for ScaledPolicy {
    fn default() -> Self {
        // Half of modelled capacity, not all of it: capacity is an estimate
        // from an impact model calibrated on other people's trades, and
        // sizing to the estimate leaves no room for it to be wrong in the one
        // direction that matters.
        Self {
            minimum_pilot_duration: Duration::from_days(90),
            minimum_pilot_observations: 60,
            minimum_pilot_sharpe: 0.5,
            maximum_capacity_utilisation: 0.5,
        }
    }
}

/// Live at size — a new decision, not the continuation of an old one.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ScaledGate {
    pub policy: ScaledPolicy,
}

impl ScaledGate {
    pub fn new(policy: ScaledPolicy) -> Self {
        Self { policy }
    }
}

impl Gate for ScaledGate {
    fn stage(&self) -> GateStage {
        GateStage::Scaled
    }

    fn evaluate(&self, evidence: &StrategyEvidence, now: Timestamp) -> GateOutcome {
        let outcome = GateOutcome::new(GateStage::Scaled, now);
        let Some(scaled) = evidence.scaled.as_ref() else {
            return outcome.record(
                "scaled_evidence_present",
                false,
                "no scaling evidence was submitted",
            );
        };

        let duration = now.since(scaled.pilot_started_at);
        // The engine's Sharpe, not a local one: the pilot's realised figure
        // must be on the scale the holdout was validated on, or the two
        // cannot be compared and the bar means something different here.
        let realised = crate::scoring::periodic_sharpe(&scaled.pilot_returns);
        let realised_sharpe = realised.as_ref().copied().unwrap_or(f64::NAN);

        let outcome = outcome
            .record(
                "pilot_ran_long_enough",
                duration >= self.policy.minimum_pilot_duration
                    && scaled.pilot_returns.len() >= self.policy.minimum_pilot_observations,
                format!(
                    "{:.1} day(s) and {} observation(s) against {:.1} day and {} minimums",
                    duration.as_days_f64(),
                    scaled.pilot_returns.len(),
                    self.policy.minimum_pilot_duration.as_days_f64(),
                    self.policy.minimum_pilot_observations
                ),
            )
            .record(
                "pilot_performance_sustained",
                realised.is_ok() && realised_sharpe >= self.policy.minimum_pilot_sharpe,
                match &realised {
                    Ok(_) => format!(
                        "realised pilot Sharpe {realised_sharpe:.2} against a {:.2} bar",
                        self.policy.minimum_pilot_sharpe
                    ),
                    Err(error) => format!(
                        "the pilot Sharpe could not be computed: {}",
                        error.message()
                    ),
                },
            )
            .record(
                // A pilot that sent no orders produced no live evidence, only
                // an elapsed calendar. Scaling on it would be scaling on the
                // shadow run with extra steps.
                "pilot_actually_traded",
                scaled.pilot_utilisation.orders_sent > 0
                    && scaled.pilot_utilisation.gross_committed.is_positive(),
                format!(
                    "{} order(s) sent committing {}, {} realised loss",
                    scaled.pilot_utilisation.orders_sent,
                    scaled.pilot_utilisation.gross_committed,
                    scaled.pilot_utilisation.realised_loss
                ),
            );

        // Capacity headroom. Beyond the modelled capacity the strategy's own
        // impact is larger than its edge, so the extra capital is not a
        // smaller return, it is a negative one.
        let headroom_ok = scaled.modelled_capacity.is_positive()
            && Decimal::from_f64(self.policy.maximum_capacity_utilisation)
                .and_then(|fraction| scaled.modelled_capacity.checked_mul(fraction))
                .is_some_and(|ceiling| scaled.proposed_notional <= ceiling);
        let outcome = outcome.record(
            "capacity_headroom",
            headroom_ok,
            format!(
                "{} proposed against {} modelled capacity, at a {:.0}% utilisation ceiling",
                scaled.proposed_notional,
                scaled.modelled_capacity,
                self.policy.maximum_capacity_utilisation * 100.0
            ),
        );

        let (dual, detail) = dual_approval_detail(scaled.scaling_approval.as_ref());
        let outcome = outcome.record("second_dual_human_approval", dual, detail);

        // The scaling approval must be its own decision. An approval reused
        // from the pilot would mean nobody looked at the pilot's results
        // before committing more capital against them.
        let distinct = match (
            scaled.pilot_approval.as_ref(),
            scaled.scaling_approval.as_ref(),
        ) {
            (Some(pilot), Some(scaling)) => {
                scaling.at > pilot.at && scaling.rationale != pilot.rationale
            }
            _ => false,
        };
        outcome.record(
            "scaling_decided_separately",
            distinct,
            match (
                scaled.pilot_approval.as_ref(),
                scaled.scaling_approval.as_ref(),
            ) {
                (Some(pilot), Some(scaling)) if !distinct => format!(
                    "the scaling approval at {} does not postdate and restate the pilot \
                     approval at {}",
                    scaling.at.to_rfc3339(),
                    pilot.at.to_rfc3339()
                ),
                (Some(_), Some(scaling)) => {
                    format!("scaling approved separately at {}", scaling.at.to_rfc3339())
                }
                _ => "both the pilot approval and a separate scaling approval are required"
                    .to_string(),
            },
        )
    }
}

/// The gate for a rung, with default policy.
///
/// Candidate and retired have no gate: nothing is promoted *to* candidate, and
/// nothing is promoted to retired at all — retirement is a demotion, and
/// demotions do not pass gates.
pub fn gate_for(stage: GateStage) -> Option<Box<dyn Gate>> {
    match stage {
        GateStage::Holdout => Some(Box::new(HoldoutGate::default())),
        GateStage::Paper => Some(Box::new(PaperGate::default())),
        GateStage::Shadow => Some(Box::new(ShadowGate::default())),
        GateStage::Pilot => Some(Box::new(PilotGate::default())),
        GateStage::Scaled => Some(Box::new(ScaledGate::default())),
        GateStage::Candidate | GateStage::Retired => None,
    }
}

//! What a strategy must be able to show at each rung of the ladder.
//!
//! Evidence is inert data. It records what was run and what came out, never a
//! verdict — the verdict is the gate's, and a gate that trusted a submitted
//! conclusion would be checking the researcher's arithmetic rather than the
//! strategy. So [`HoldoutEvidence`] carries the holdout return series and the
//! parameters of the cross-validation run rather than a Sharpe ratio and a
//! claim that folds were purged; the gate recomputes both.
//!
//! Every field here is something a strategy either has or has not got. There
//! is no field a researcher can fill in with an opinion.

use crate::trials::TrialAccount;
use qip_contracts::gate::GateStage;
use qip_contracts::governance::Approval;
use qip_contracts::signal::{SignalKind, StrategyId};
use qip_contracts::{CapitalEnvelope, Utilisation};
use qip_core::{Decimal, Duration, ObjectId, Timestamp};
use serde::{Deserialize, Serialize};

/// When one feature's value became knowable, against the instant it was used.
///
/// The pair is the whole leakage question. A feature whose value was only
/// knowable after the decision it fed cannot have fed that decision in
/// production, however good the backtest looked.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FeatureTiming {
    pub feature: String,
    /// The earliest instant this value could have been read by a live system.
    pub known_at: Timestamp,
    /// The decision instant the value was fed into.
    pub used_at: Timestamp,
}

impl FeatureTiming {
    /// Whether this feature was used before it could have been known.
    pub fn leaks(&self) -> bool {
        self.known_at > self.used_at
    }
}

/// The record of a leakage audit over one fitting run.
///
/// An audit that examined nothing is not a clean audit, it is an absent one,
/// and [`Self::is_clean`] refuses to conflate the two. That distinction is the
/// point of the type: "we found no leakage" and "we did not look" produce the
/// same empty findings list and must not produce the same answer.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct LeakageAudit {
    pub timings: Vec<FeatureTiming>,
    /// Datasets whose history is restated by the vendor and which were read
    /// without a point-in-time snapshot. A restated series is tomorrow's
    /// numbers wearing yesterday's timestamp, which is leakage that no
    /// per-feature timing check can see.
    pub restated_without_snapshots: Vec<String>,
}

impl LeakageAudit {
    /// Everything wrong with the run, phrased so an operator can act on it.
    pub fn findings(&self) -> Vec<String> {
        let mut out: Vec<String> = self
            .timings
            .iter()
            .filter(|t| t.leaks())
            .map(|t| {
                format!(
                    "{} was used at {} but only knowable at {}",
                    t.feature,
                    t.used_at.to_rfc3339(),
                    t.known_at.to_rfc3339()
                )
            })
            .collect();
        out.extend(
            self.restated_without_snapshots
                .iter()
                .map(|d| format!("{d} is restated and was read without a point-in-time snapshot")),
        );
        out
    }

    pub fn is_clean(&self) -> bool {
        !self.timings.is_empty() && self.findings().is_empty()
    }
}

/// The parameters and outturn of a purged k-fold run.
///
/// The gate rebuilds the splits from these numbers and compares, so a run that
/// says it purged and did not is caught. Recording only `purged: 400` would
/// have been unfalsifiable.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct CrossValidationRun {
    pub folds: usize,
    /// Observations a label spans, which sets the purge width.
    pub label_horizon: usize,
    /// Observations embargoed after each test fold.
    pub embargo: usize,
    /// Ordered observations the folds were built over.
    pub observations: usize,
    /// Observations dropped for overlapping a test fold, summed over folds.
    pub purged: usize,
    /// Observations dropped immediately after a test fold, summed over folds.
    pub embargoed: usize,
}

/// Performance on data held out of fitting, and how that data was held out.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HoldoutEvidence {
    /// Returns on the held-out data, in time order.
    pub holdout_returns: Vec<f64>,
    /// Per-fold returns inside the training windows.
    pub in_sample_folds: Vec<Vec<f64>>,
    /// Per-fold returns on the test windows.
    pub out_of_sample_folds: Vec<Vec<f64>>,
    /// How many configurations were tried, in this run, before this one was
    /// put forward.
    ///
    /// Not the number that deflates the Sharpe on its own. Understating it is
    /// the easiest way to make a search result look like a discovery, and
    /// splitting a sweep across runs understates it without lying, so the
    /// gate deflates against the family's lifetime count in
    /// [`StrategyEvidence::trial_account`] and this number is what one run
    /// adds to it.
    pub trials: usize,
    pub periods_per_year: f64,
    pub cross_validation: CrossValidationRun,
    pub leakage: LeakageAudit,
}

/// A simulated run against the live feed, and what it cost.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PaperEvidence {
    /// Whether the run consumed the live feed rather than a recording.
    ///
    /// A paper run against recorded data is a backtest with extra steps: it
    /// re-tests the research path instead of testing the live one.
    pub against_live_data: bool,
    /// Cost per fill, in basis points of notional, that the backtest assumed.
    pub assumed_cost_bps: f64,
    /// Cost per fill, in basis points, the paper run actually realised.
    pub realised_cost_bps: Vec<f64>,
    /// Highest fraction of daily volume any single order represented.
    pub peak_participation: f64,
    /// Participation beyond which the impact model was not calibrated.
    pub modelled_participation_limit: f64,
    /// Orders the simulator refused as too large to price.
    pub unfillable_orders: usize,
    pub filled_orders: usize,
}

impl PaperEvidence {
    /// Share of intended orders the simulator would not price.
    ///
    /// A high share is a capacity finding, not a simulator complaint: the
    /// strategy wants to trade more than the market will absorb.
    pub fn unfillable_share(&self) -> f64 {
        let total = self.filled_orders + self.unfillable_orders;
        if total == 0 {
            return 0.0;
        }
        self.unfillable_orders as f64 / total as f64
    }
}

/// One live decision, paired with what the research path said for the same
/// instrument at the same instant.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ShadowDecision {
    pub at: Timestamp,
    pub object_id: ObjectId,
    /// What the live path decided, computed from the live feed.
    pub live: SignalKind,
    /// What the research path predicted for the same instant.
    pub predicted: SignalKind,
    pub live_quantity: Decimal,
    pub predicted_quantity: Decimal,
}

impl ShadowDecision {
    pub fn directions_agree(&self) -> bool {
        self.live == self.predicted
    }

    /// How far the live size is from the predicted one, as a fraction of the
    /// prediction. `None` when both paths wanted nothing, where a ratio has no
    /// meaning and reporting zero would flatter the agreement rate.
    pub fn size_divergence(&self) -> Option<f64> {
        let predicted = self.predicted_quantity.abs().to_f64();
        let live = self.live_quantity.abs().to_f64();
        if predicted <= 0.0 && live <= 0.0 {
            return None;
        }
        if predicted <= 0.0 {
            return Some(1.0);
        }
        Some((live - predicted).abs() / predicted)
    }
}

/// Live decisions computed and discarded, compared against the backtest.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ShadowEvidence {
    pub decisions: Vec<ShadowDecision>,
    /// Whether any shadow order reached a venue.
    ///
    /// Recorded because it must be false. A shadow run whose orders escaped
    /// was live trading without an approval, and its results are evidence of
    /// a control failure rather than of a strategy.
    pub orders_reached_a_venue: bool,
    /// Observed feed-to-decision latency at the 99th percentile.
    pub decision_latency_p99: Duration,
}

impl ShadowEvidence {
    /// Fraction of paired decisions where the live and research paths agreed
    /// on direction.
    ///
    /// This is the number the shadow rung exists to produce. Everything
    /// upstream — the backtest, the holdout, the paper costs — describes the
    /// research path. If the live path decides something else, none of it
    /// describes what will trade.
    pub fn agreement_rate(&self) -> f64 {
        if self.decisions.is_empty() {
            return 0.0;
        }
        let agreed = self
            .decisions
            .iter()
            .filter(|d| d.directions_agree())
            .count();
        agreed as f64 / self.decisions.len() as f64
    }

    /// Median relative size divergence over decisions where a size was wanted.
    pub fn median_size_divergence(&self) -> f64 {
        let divergences: Vec<f64> = self
            .decisions
            .iter()
            .filter_map(ShadowDecision::size_divergence)
            .collect();
        if divergences.is_empty() {
            return 0.0;
        }
        qip_numerics::stats::median(&divergences)
    }

    pub fn disagreements(&self) -> Vec<&ShadowDecision> {
        self.decisions
            .iter()
            .filter(|d| !d.directions_agree())
            .collect()
    }
}

/// A condition under which the strategy stops itself.
///
/// Stated before capital is committed, evaluated afterwards without a human.
/// A kill condition that only a person can evaluate is a plan to be paged,
/// not a control.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum KillCondition {
    /// Cumulative realised loss.
    RealisedLoss(Decimal),
    /// Peak-to-trough drawdown, as a fraction of the pilot's high-water mark.
    Drawdown(f64),
    /// Consecutive sessions closing down.
    ConsecutiveLosingDays(u32),
    /// Realised cost per fill running above what the strategy was priced on.
    CostOverrun {
        modelled_bps: f64,
        tolerance_bps: f64,
    },
}

impl KillCondition {
    pub fn describe(&self) -> String {
        match self {
            Self::RealisedLoss(limit) => format!("realised loss reaches {limit}"),
            Self::Drawdown(fraction) => {
                format!("drawdown reaches {:.1}%", fraction * 100.0)
            }
            Self::ConsecutiveLosingDays(days) => format!("{days} consecutive losing sessions"),
            Self::CostOverrun {
                modelled_bps,
                tolerance_bps,
            } => format!(
                "realised cost exceeds the modelled {modelled_bps:.1}bp by more than {tolerance_bps:.1}bp"
            ),
        }
    }
}

/// Everything the pilot rung demands: a person, a bound, and a way to stop.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PilotEvidence {
    /// The human decision to commit capital. Dual, per
    /// [`Approval::countersigned_by`], which already refuses a self-approval.
    pub approval: Option<Approval>,
    /// The bound the cell will trade inside. Signed and expiring by
    /// construction — see [`CapitalEnvelope`].
    pub envelope: Option<CapitalEnvelope>,
    /// What stops the strategy, stated before it starts.
    pub kill_conditions: Vec<KillCondition>,
}

/// Sustained pilot performance, headroom, and a fresh decision to scale.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ScaledEvidence {
    /// Returns realised while at pilot, in time order.
    pub pilot_returns: Vec<f64>,
    pub pilot_started_at: Timestamp,
    /// What the pilot actually committed against its envelope.
    pub pilot_utilisation: Utilisation,
    /// Notional the strategy would run at once scaled.
    pub proposed_notional: Decimal,
    /// Notional beyond which its own impact eats its edge.
    pub modelled_capacity: Decimal,
    /// The approval that authorised the pilot.
    pub pilot_approval: Option<Approval>,
    /// A separate, later approval to scale.
    ///
    /// Two fields rather than one, because the gate's job is to establish that
    /// scaling was decided rather than inherited.
    pub scaling_approval: Option<Approval>,
}

/// Everything known about one strategy, at whatever rung it has reached.
///
/// Assembled by the research and operations paths and read by the gates. The
/// per-rung fields are optional because a candidate has none of them; a gate
/// treats its own missing evidence as a failed check rather than an error, so
/// "not submitted" and "submitted and inadequate" both stop the promotion.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct StrategyEvidence {
    pub holdout: Option<HoldoutEvidence>,
    /// The lifetime trial count this evaluation was charged under, issued by
    /// [`crate::trials::TrialBook::charge`].
    ///
    /// `None` means the count is unknown, and the holdout gate reads unknown
    /// as a failed check rather than as zero. [`crate::attempt_promotion`]
    /// charges the ledger's book itself and sets this, overwriting anything
    /// the submitted package carried, so on the ordinary path the number
    /// here is the book's and not the researcher's.
    #[serde(default)]
    pub trial_account: Option<TrialAccount>,
    pub paper: Option<PaperEvidence>,
    pub shadow: Option<ShadowEvidence>,
    pub pilot: Option<PilotEvidence>,
    pub scaled: Option<ScaledEvidence>,
}

impl StrategyEvidence {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_holdout(mut self, evidence: HoldoutEvidence) -> Self {
        self.holdout = Some(evidence);
        self
    }

    pub fn with_paper(mut self, evidence: PaperEvidence) -> Self {
        self.paper = Some(evidence);
        self
    }

    pub fn with_shadow(mut self, evidence: ShadowEvidence) -> Self {
        self.shadow = Some(evidence);
        self
    }

    pub fn with_pilot(mut self, evidence: PilotEvidence) -> Self {
        self.pilot = Some(evidence);
        self
    }

    pub fn with_scaled(mut self, evidence: ScaledEvidence) -> Self {
        self.scaled = Some(evidence);
        self
    }

    pub fn with_trial_account(mut self, account: TrialAccount) -> Self {
        self.trial_account = Some(account);
        self
    }

    /// Which rungs have any evidence submitted at all.
    pub fn stages_evidenced(&self) -> Vec<GateStage> {
        let mut out = Vec::new();
        if self.holdout.is_some() {
            out.push(GateStage::Holdout);
        }
        if self.paper.is_some() {
            out.push(GateStage::Paper);
        }
        if self.shadow.is_some() {
            out.push(GateStage::Shadow);
        }
        if self.pilot.is_some() {
            out.push(GateStage::Pilot);
        }
        if self.scaled.is_some() {
            out.push(GateStage::Scaled);
        }
        out
    }
}

/// A strategy's evidence with the strategy it belongs to.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EvidencePackage {
    pub strategy: StrategyId,
    pub evidence: StrategyEvidence,
    /// When the package was assembled. Gates take `now` separately, so a stale
    /// package is visible rather than silently current.
    pub assembled_at: Timestamp,
}

impl EvidencePackage {
    pub fn new(strategy: StrategyId, evidence: StrategyEvidence, assembled_at: Timestamp) -> Self {
        Self {
            strategy,
            evidence,
            assembled_at,
        }
    }
}

//! Steps 4 to 7 carried out: a started item's acquisition, its campaign
//! across the four environments, its nine-criterion evaluation, and the
//! measured result the desk concludes on (R12-EXPAND-6025, 6049, 6050, 6051,
//! 6069).
//!
//! # The failure this prevents
//!
//! The desk could queue, start and time out research, and
//! [`crate::promotion`] could refuse an incomplete campaign or a partial
//! evaluation — but nothing ever produced either, so `ExpansionDesk::conclude`
//! had no caller, every started item closed "out of time" or "gap quiet", and
//! research converted nothing into capability. A queue of questions with no
//! one answering them reads as a research programme and is a to-do list.
//!
//! # What runs, and what is honestly not run
//!
//! One campaign is carried out end to end, because one is measurable from
//! what the platform already records: **a venue's fill bias**. A gap raised
//! from the twin's fill residuals (`fill_scores:<venue>` in its evidence) is
//! researched by fitting a constant correction — the mean signed residual —
//! on the fills recorded *before* the item started, and judging it on the
//! fills recorded *after*, against the uncorrected twin as champion:
//!
//! | Stage | Data | What it establishes |
//! |---|---|---|
//! | historical | residuals before the item started | the fitted bias |
//! | simulated | a seeded bootstrap of the historical residuals | whether the bias's sign survives resampling |
//! | paper | the first half of the residuals since start | out-of-sample improvement |
//! | shadow | the second half | improvement again, later, beside the champion |
//!
//! Every other acquisition is recorded as [`AcquisitionOutcome::NotExecutable`]
//! naming why, rather than simulated: a campaign this module cannot measure
//! is one it does not pretend to have run.
//!
//! # Research only
//!
//! [`AcquisitionEffect`] has one variant. An acquisition reads what the
//! platform recorded and produces a dataset digest; it places nothing,
//! fetches nothing from a vendor and registers nothing. The capability a
//! passing campaign becomes is a correction to a *twin's prediction* — a
//! `Simulate` or `Reason` verb, never one that moves money — and its
//! eligibility is paper and shadow (`promotion::promote`).

use crate::desk::Lineage;
use crate::plan::{AcquisitionAction, Environment};
use crate::promotion::{
    Criterion, EvaluationRecord, Finding, Provenance, ResearchCampaign, verb_for,
};
use qip_contracts::expansion::{CapabilityVerb, ExperimentPack};
use qip_core::error::{Error, Result};
use qip_core::hash::sha256_hex;
use qip_core::rng::{Rng, Xoshiro256};
use qip_core::{Decimal, Timestamp};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The evidence prefix a fill-residual gap carries (`detectors.rs`).
pub const FILL_EVIDENCE_PREFIX: &str = "fill_scores:";

/// Residuals one campaign reads at most, newest kept. Bounded so a venue
/// with a long history cannot make one sweep's campaign unbounded.
pub const MAX_RESIDUALS: usize = 4_096;

/// One recorded fill residual: how far the twin's entry was from the
/// venue's, in signed basis points (`FillScore::trade_error_bps`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FillResidual {
    pub venue: String,
    pub at: Timestamp,
    pub error_bps: f64,
}

/// What an acquisition may do. One variant, on purpose: see the module doc.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AcquisitionEffect {
    ResearchOnly,
}

/// What carrying out an acquisition produced.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum AcquisitionOutcome {
    /// Recorded data gathered into a dataset with this digest.
    Gathered {
        dataset_digest: String,
        records: usize,
    },
    /// The acquisition has no executor in this build; stated, not faked.
    NotExecutable { reason: String },
}

impl AcquisitionOutcome {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Gathered { .. } => "gathered",
            Self::NotExecutable { .. } => "not_executable",
        }
    }
}

/// One acquisition carried out for a started item (R12-EXPAND-6049).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcquisitionRecord {
    pub question: String,
    pub action: AcquisitionAction,
    pub effect: AcquisitionEffect,
    pub outcome: AcquisitionOutcome,
    pub at: Timestamp,
}

/// The venue a lineage's evidence names fill residuals for, if any.
pub fn fill_venue(lineage: &Lineage) -> Option<&str> {
    lineage
        .derivation
        .evidence
        .iter()
        .find_map(|e| e.strip_prefix(FILL_EVIDENCE_PREFIX))
        .filter(|venue| !venue.trim().is_empty())
}

fn digest_of(residuals: &[&FillResidual]) -> Result<String> {
    let bytes = serde_json::to_vec(residuals)
        .map_err(|e| Error::invalid(format!("a residual set did not serialise: {e}")))?;
    Ok(sha256_hex(&bytes))
}

fn for_venue<'a>(residuals: &'a [FillResidual], venue: &str) -> Vec<&'a FillResidual> {
    let mut mine: Vec<&FillResidual> = residuals
        .iter()
        .filter(|r| r.venue == venue && r.error_bps.is_finite())
        .collect();
    mine.sort_by(|a, b| a.at.cmp(&b.at).then(a.error_bps.total_cmp(&b.error_bps)));
    let excess = mine.len().saturating_sub(MAX_RESIDUALS);
    mine.drain(..excess);
    mine
}

/// Carry out the acquisition a started item's lineage names, research only.
pub fn acquire(
    question: &str,
    lineage: &Lineage,
    residuals: &[FillResidual],
    at: Timestamp,
) -> Result<AcquisitionRecord> {
    let outcome = match (lineage.action, fill_venue(lineage)) {
        (AcquisitionAction::SelectModel | AcquisitionAction::ReplayTicks, Some(venue)) => {
            let gathered = for_venue(residuals, venue);
            AcquisitionOutcome::Gathered {
                dataset_digest: digest_of(&gathered)?,
                records: gathered.len(),
            }
        }
        (action, _) => AcquisitionOutcome::NotExecutable {
            reason: format!(
                "`{}` has no executor in this build for evidence {:?}; the item runs to its stop \
                 condition rather than reporting a campaign nobody ran",
                action.as_str(),
                lineage.derivation.evidence
            ),
        },
    };
    Ok(AcquisitionRecord {
        question: question.to_string(),
        action: lineage.action,
        effect: AcquisitionEffect::ResearchOnly,
        outcome,
        at,
    })
}

/// How a fill-bias campaign is judged.
#[derive(Clone, Debug, PartialEq)]
pub struct CampaignPolicy {
    /// Historical residuals the fit needs.
    pub min_fit: usize,
    /// Residuals each of paper and shadow needs.
    pub min_eval: usize,
    /// Bootstrap resamples in the simulated stage.
    pub bootstrap_draws: usize,
    /// The least shadow improvement, in basis points of mean absolute error,
    /// that is incremental value over the champion.
    pub min_improvement_bps: f64,
    /// Seed for the simulated stage, recorded in its provenance.
    pub seed: u64,
    /// Operations the correction may cost per prediction on the reflex
    /// path.
    pub max_ops: u64,
}

impl Default for CampaignPolicy {
    fn default() -> Self {
        Self {
            min_fit: 8,
            min_eval: 4,
            bootstrap_draws: 200,
            min_improvement_bps: 1.0,
            seed: 0x5EED_F111,
            max_ops: 4,
        }
    }
}

/// The correction a passing campaign researched: subtract `bias_bps` from
/// the twin's predicted entry at `venue`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FillBiasCorrection {
    pub venue: String,
    pub bias_bps: f64,
    pub fitted_on: usize,
    pub fitted_through: Timestamp,
    /// Operations one prediction pays to apply it: a single subtraction.
    pub ops: u64,
}

/// A campaign carried out and evaluated, ready for `ExpansionDesk::conclude`.
#[derive(Clone, Debug, PartialEq)]
pub struct CampaignResult {
    pub campaign: ResearchCampaign,
    pub evaluation: EvaluationRecord,
    pub correction: FillBiasCorrection,
    /// Champion minus challenger mean absolute error in the shadow stage.
    pub shadow_improvement_bps: f64,
    /// The same in the paper stage; zero when there was none.
    pub paper_improvement_bps: f64,
    /// The bootstrap's 5th and 95th percentile of the bias, in basis points.
    pub bias_band_bps: (f64, f64),
    /// That the correction was installed for the paper window, once shadow
    /// cleared (ADR 0142 d5). `None` is a candidate never installed, which
    /// has no paper stage.
    pub installation: Option<Installation>,
    /// That the installed correction was withdrawn because paper did not
    /// clear (ADR 0142 d5). Never `Some` without `installation`.
    pub withdrawal: Option<Withdrawal>,
}

/// The record that a candidate correction was installed in the paper fill
/// model at the close of its shadow window (ADR 0142 d5): the proof the ADR
/// asks for is that no installation falls inside the shadow window and that
/// every paper fill arrives after this instant.
impl CampaignResult {
    /// The campaign's experiment pack (R12-EXPAND-6035): what it researched,
    /// what it built, the backtest it rests on and what it proposes, as the
    /// validated contract record. The backtest is the Historical stage — the
    /// residuals the correction was fitted on, by digest and count — and the
    /// improvement is the Shadow stage's, the first data the correction did
    /// not see. A campaign with no Historical stage has no backtest and is
    /// refused rather than given an empty one.
    pub fn experiment_pack(&self, now: Timestamp) -> Result<ExperimentPack> {
        let historical = self
            .campaign
            .stages()
            .iter()
            .find(|(environment, _)| *environment == Environment::Historical)
            .map(|(_, provenance)| provenance)
            .ok_or_else(|| {
                Error::invalid(format!(
                    "`{}` ran no Historical stage, so it has no backtest to pack",
                    self.campaign.question()
                ))
            })?;
        let findings = self
            .evaluation
            .findings()
            .iter()
            .map(|(criterion, finding)| {
                format!(
                    "{}: {} — {}",
                    format!("{criterion:?}").to_ascii_lowercase(),
                    if finding.passed { "passed" } else { "failed" },
                    finding.evidence
                )
            })
            .collect();
        let proposal = match (&self.installation, &self.withdrawal) {
            (Some(installation), None) => format!(
                "install a {:.1} bp entry correction at `{}`; paper improved by {:.2} bp",
                installation.bias_bps, installation.venue, self.paper_improvement_bps
            ),
            (Some(_), Some(withdrawal)) => format!("none: {}", withdrawal.reason),
            _ => format!(
                "none: the shadow window improved by {:.2} bp, short of the bar, so the \
                 correction was never installed",
                self.shadow_improvement_bps
            ),
        };
        let pack = ExperimentPack {
            campaign_id: self.campaign.id()?,
            hypothesis: self.campaign.question().to_string(),
            feature: format!(
                "fill-bias correction at `{}`: {:.1} bp over {} fill(s) fitted through {}",
                self.correction.venue,
                self.correction.bias_bps,
                self.correction.fitted_on,
                self.correction.fitted_through.to_rfc3339()
            ),
            backtest_digest: historical.dataset_digest.clone(),
            backtest_samples: self.correction.fitted_on,
            backtest_improvement_bps: self.shadow_improvement_bps,
            findings,
            experiments: self.campaign.experiment_refs()?,
            proposal,
            produced_at: now,
        };
        pack.validate()?;
        Ok(pack)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Installation {
    pub venue: String,
    pub bias_bps: f64,
    /// The last shadow fill's instant; the paper window opens after it.
    pub installed_at: Timestamp,
    /// The shadow window the installation was decided on.
    pub shadow_window: (Timestamp, Timestamp),
    /// The paper window the installed correction acted over.
    pub paper_window: (Timestamp, Timestamp),
}

/// The record that an installed correction was withdrawn because its paper
/// window did not clear (ADR 0142 d5).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Withdrawal {
    pub venue: String,
    pub withdrawn_at: Timestamp,
    pub reason: String,
}

/// ADR 0142 d4: the shadow window lies wholly before the paper window. Two
/// fills at one instant on either side of the split would put the same
/// instant in both windows, and a candidate scored "forward" on a fill it
/// was also installed for is scored on nothing.
fn disjoint_windows(shadow: &[&FillResidual], paper: &[&FillResidual]) -> Result<()> {
    match (shadow.last(), paper.first()) {
        (Some(last_shadow), Some(first_paper)) if last_shadow.at >= first_paper.at => {
            Err(Error::invalid(format!(
                "the shadow window ends at {} and the paper window opens at {}; the two must be \
                 disjoint and shadow first, so the campaign is refused",
                last_shadow.at.to_rfc3339(),
                first_paper.at.to_rfc3339()
            )))
        }
        _ => Ok(()),
    }
}

fn mean(values: impl Iterator<Item = f64>) -> f64 {
    let (sum, n) = values.fold((0.0, 0usize), |(s, n), v| (s + v, n + 1));
    if n == 0 { 0.0 } else { sum / n as f64 }
}

fn mae(set: &[&FillResidual], correction: f64) -> f64 {
    mean(set.iter().map(|r| (r.error_bps - correction).abs()))
}

fn finding(passed: bool, evidence: String) -> Finding {
    Finding { passed, evidence }
}

/// Run the fill-bias campaign for `question` at `venue`. `Ok(None)` is "not
/// enough data yet" — the item keeps running — and is distinct from a
/// campaign that ran and failed, which returns a result whose evaluation
/// names the failed criteria.
///
/// `charged` is the compute the market actually charged this item across
/// its windows and `budget` the most its curriculum item authorised; the
/// cost criterion compares the two, so research that cost more than it was
/// allowed fails by name rather than by nobody looking.
#[allow(clippy::too_many_arguments)]
pub fn run_fill_bias_campaign(
    question: &str,
    lineage: &Lineage,
    residuals: &[FillResidual],
    started_at: Timestamp,
    policy: &CampaignPolicy,
    jurisdiction_eligible: bool,
    charged: Decimal,
    budget: Decimal,
) -> Result<Option<CampaignResult>> {
    let Some(venue) = fill_venue(lineage) else {
        return Err(Error::invalid(format!(
            "`{question}` names no fill residuals in its evidence; it is not a fill-bias campaign"
        )));
    };
    if policy.min_fit == 0 || policy.min_eval == 0 || policy.bootstrap_draws == 0 {
        return Err(Error::invalid(
            "a campaign needs a positive fit, evaluation and bootstrap size",
        ));
    }
    let mine = for_venue(residuals, venue);
    let (historical, post): (Vec<&FillResidual>, Vec<&FillResidual>) =
        mine.into_iter().partition(|r| r.at < started_at);
    if historical.len() < policy.min_fit || post.len() < 2 * policy.min_eval {
        return Ok(None);
    }
    // Shadow first, then paper (ADR 0142 d3): the earlier half is the
    // forward window the candidate is scored on without acting, the later
    // half the window it acts in once installed. The two must not share an
    // instant (d4).
    let half = post.len() / 2;
    let (shadow, paper) = post.split_at(half);
    disjoint_windows(shadow, paper)?;

    let mut campaign = ResearchCampaign::open(question)?;
    let lineage_of = |set: &[&FillResidual]| -> Vec<String> {
        match (set.first(), set.last()) {
            (Some(first), Some(last)) => vec![format!(
                "{FILL_EVIDENCE_PREFIX}{venue}@{}..{}",
                first.at.to_rfc3339(),
                last.at.to_rfc3339()
            )],
            _ => Vec::new(),
        }
    };

    // Historical: the fit.
    let bias = mean(historical.iter().map(|r| r.error_bps));
    let fitted_through = historical.last().map_or(started_at, |r| r.at);
    let historical_digest = digest_of(&historical)?;
    campaign.record(
        Environment::Historical,
        Provenance {
            dataset_digest: historical_digest.clone(),
            lineage: lineage_of(&historical),
            seed: 0,
        },
    )?;

    // Simulated: a seeded bootstrap of the fit sample.
    let mut rng = Xoshiro256::seeded(policy.seed);
    let mut draws: Vec<f64> = (0..policy.bootstrap_draws)
        .map(|_| {
            mean((0..historical.len()).map(|_| {
                let index = rng.below(historical.len() as u64) as usize;
                historical.get(index).map_or(0.0, |r| r.error_bps)
            }))
        })
        .collect();
    draws.sort_by(f64::total_cmp);
    let low = draws.get(draws.len() / 20).copied().unwrap_or(0.0);
    let high = draws
        .get(draws.len() - 1 - draws.len() / 20)
        .copied()
        .unwrap_or(0.0);
    campaign.record(
        Environment::Simulated,
        Provenance {
            dataset_digest: historical_digest,
            lineage: vec![format!(
                "bootstrap of {} draw(s) over {FILL_EVIDENCE_PREFIX}{venue}",
                policy.bootstrap_draws
            )],
            seed: policy.seed,
        },
    )?;

    // Shadow: out of sample and never installed, champion against
    // challenger on fills the candidate did not touch.
    let shadow_improvement = mae(shadow, 0.0) - mae(shadow, bias);
    campaign.record(
        Environment::Shadow,
        Provenance {
            dataset_digest: digest_of(shadow)?,
            lineage: lineage_of(shadow),
            seed: 0,
        },
    )?;
    // Phase two (ADR 0142 d5): the correction is installed only when shadow
    // clears the policy's bar, and the paper stage exists only for an
    // installed correction. A candidate shadow refused has three stages and
    // no paper window, which is the fact the ADR wants on the record rather
    // than a paper score for a candidate that never acted.
    let window = |set: &[&FillResidual]| -> (Timestamp, Timestamp) {
        (
            set.first().map_or(started_at, |r| r.at),
            set.last().map_or(started_at, |r| r.at),
        )
    };
    let installed = shadow_improvement >= policy.min_improvement_bps;
    let mut installation = None;
    let mut withdrawal = None;
    let mut paper_improvement = 0.0;
    if installed {
        let installed_at = shadow.last().map_or(started_at, |r| r.at);
        installation = Some(Installation {
            venue: venue.to_string(),
            bias_bps: bias,
            installed_at,
            shadow_window: window(shadow),
            paper_window: window(paper),
        });
        paper_improvement = mae(paper, 0.0) - mae(paper, bias);
        campaign.record(
            Environment::Paper,
            Provenance {
                dataset_digest: digest_of(paper)?,
                lineage: lineage_of(paper),
                seed: 0,
            },
        )?;
        if paper_improvement <= 0.0 {
            withdrawal = Some(Withdrawal {
                venue: venue.to_string(),
                withdrawn_at: paper.last().map_or(installed_at, |r| r.at),
                reason: format!(
                    "paper improvement {paper_improvement:.2} bp did not clear zero; the \
                     correction installed at {} is withdrawn",
                    installed_at.to_rfc3339()
                ),
            });
        }
    }

    let correction = FillBiasCorrection {
        venue: venue.to_string(),
        bias_bps: bias,
        fitted_on: historical.len(),
        fitted_through,
        ops: 1,
    };
    let paper_mean = mean(paper.iter().map(|r| r.error_bps));
    let corrected_mean = mean(paper.iter().map(|r| r.error_bps - bias));
    let first_post = post.first().map_or(started_at, |r| r.at);
    let money_moving = matches!(
        verb_for(lineage.class),
        CapabilityVerb::Trade
            | CapabilityVerb::Settle
            | CapabilityVerb::Hedge
            | CapabilityVerb::Transfer
            | CapabilityVerb::Purchase
    );
    let mut findings = BTreeMap::new();
    findings.insert(
        Criterion::Calibration,
        if installed {
            finding(
                corrected_mean.abs() < paper_mean.abs(),
                format!(
                    "paper mean residual {paper_mean:.2} bp uncorrected, {corrected_mean:.2} bp \
                     corrected"
                ),
            )
        } else {
            finding(
                false,
                "no paper stage: shadow did not clear and the correction was never installed"
                    .to_string(),
            )
        },
    );
    findings.insert(
        Criterion::Robustness,
        finding(
            (low > 0.0 && high > 0.0) || (low < 0.0 && high < 0.0),
            format!(
                "bootstrap 5th..95th percentile of the bias {low:.2}..{high:.2} bp over {} draw(s), seed {}",
                policy.bootstrap_draws, policy.seed
            ),
        ),
    );
    findings.insert(
        Criterion::Leakage,
        finding(
            fitted_through < started_at && started_at <= first_post,
            format!(
                "fit through {} strictly before the item started at {}; evaluation from {}",
                fitted_through.to_rfc3339(),
                started_at.to_rfc3339(),
                first_post.to_rfc3339()
            ),
        ),
    );
    findings.insert(
        Criterion::RegimeSensitivity,
        finding(
            installed && paper_improvement > 0.0 && shadow_improvement > 0.0,
            if installed {
                format!(
                    "improvement {shadow_improvement:.2} bp in shadow and {paper_improvement:.2} \
                     bp in paper after installation"
                )
            } else {
                format!(
                    "improvement {shadow_improvement:.2} bp in shadow; never installed, so no \
                     paper window"
                )
            },
        ),
    );
    findings.insert(
        Criterion::Cost,
        finding(
            charged <= budget,
            format!("campaign charged {charged} against an authorised budget of {budget}"),
        ),
    );
    findings.insert(
        Criterion::Latency,
        finding(
            correction.ops <= policy.max_ops,
            format!(
                "{} operation(s) per prediction against a reflex bound of {}",
                correction.ops, policy.max_ops
            ),
        ),
    );
    findings.insert(
        Criterion::Safety,
        finding(
            !money_moving,
            format!(
                "capability verb {:?}; acquisitions research-only",
                verb_for(lineage.class)
            ),
        ),
    );
    findings.insert(
        Criterion::LegalEligibility,
        finding(
            jurisdiction_eligible,
            "the desk's jurisdiction re-asked against the eligible set at evaluation".to_string(),
        ),
    );
    findings.insert(
        Criterion::IncrementalValue,
        finding(
            shadow_improvement >= policy.min_improvement_bps,
            format!(
                "shadow mean absolute error {:.2} bp champion, {:.2} bp challenger; improvement \
                 {shadow_improvement:.2} against a bar of {:.2}",
                mae(shadow, 0.0),
                mae(shadow, bias),
                policy.min_improvement_bps
            ),
        ),
    );
    let evaluation = EvaluationRecord::new(question, findings)?;
    Ok(Some(CampaignResult {
        campaign,
        evaluation,
        correction,
        shadow_improvement_bps: shadow_improvement,
        paper_improvement_bps: paper_improvement,
        bias_band_bps: (low, high),
        installation,
        withdrawal,
    }))
}

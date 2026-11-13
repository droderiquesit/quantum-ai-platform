//! The World Model Arbitration Brain (R12-WORLD-3025, ADR 0488): the
//! specialist that reports how the world model federation scored and
//! reconciled its competing members.
//!
//! The kernel's `FederationStage` arbitrates every UNDERSTAND pass — branch,
//! merge, abstain or request more data — and journals a `FederationReview`
//! carrying each arbitration with its disagreement attached. Until this
//! agent existed no specialist in the organisation read that record, so the
//! chief convened a panel that could not say whether the world model it was
//! reasoning on agreed with itself, and the deep brain's registry covered
//! world-model arbitration with a hand-written entry nothing in the
//! organisation stood behind.
//!
//! It reads the desk's arbitration slot, which the kernel writes from the
//! review it has just journaled (`Platform::brief_the_desk_on_arbitration`),
//! so every decision it reports is the federation's and not this agent's:
//! it decides nothing, re-scores nothing and merges no number the federation
//! did not merge. What it adds is a finding in the organisation's record
//! that names each decision, keeps every disagreeing view in the count, and
//! states each member's calibration as the federation scored it.
//!
//! **No review is no data, never a federation that agreed.** Before the
//! kernel has run a pass, or when a pass arbitrated nothing, the finding is a
//! no-view naming why, so the scope is not covered by an empty record.

use crate::desk::Desk;
use crate::support::{FindingBuilder, computed, no_data};
use qip_agents::finding::{AgentBrief, AgentFinding, Direction};
use qip_agents::manifest::AgentManifest;
use qip_agents::runtime::{Agent, AgentContext};
use qip_core::error::Result;
use qip_world_model::federation::{Calibration, Decision};
use std::sync::Arc;

/// The input every fact this agent states is computed from.
const SOURCE: &[&str] = &["world-model-federation"];

/// Reads the federation's latest review: decisions, disagreement, calibration.
#[derive(Debug)]
pub struct WorldModelArbitrationAnalyst {
    manifest: AgentManifest,
    desk: Arc<Desk>,
}

impl WorldModelArbitrationAnalyst {
    pub fn new(manifest: AgentManifest, desk: Arc<Desk>) -> Self {
        Self { manifest, desk }
    }
}

impl Agent for WorldModelArbitrationAnalyst {
    fn manifest(&self) -> &AgentManifest {
        &self.manifest
    }

    fn analyse(&self, ctx: &mut AgentContext, brief: &AgentBrief) -> Result<AgentFinding> {
        let view = self.desk.arbitration.get(ctx)?;
        if !view.observed {
            return Ok(no_data(
                ctx,
                brief.as_of,
                "no world model federation review has been written: the kernel has not run an \
                 UNDERSTAND pass, so there is no arbitration to read",
            ));
        }
        if view.arbitrations.is_empty() {
            return Ok(no_data(
                ctx,
                brief.as_of,
                format!(
                    "the federation review of cycle {} arbitrated nothing: no proposition held \
                     two live views, so there is no branch, merge, abstention or request to report",
                    view.cycle
                ),
            ));
        }

        let (mut branch, mut merge, mut abstain, mut request) = (0usize, 0usize, 0usize, 0usize);
        let mut views_kept = 0usize;
        let mut widest = 0.0_f64;
        for arbitration in &view.arbitrations {
            match arbitration.decision {
                Decision::Branch => branch += 1,
                Decision::Merge => merge += 1,
                Decision::Abstain => abstain += 1,
                Decision::RequestMoreInformation(_) => request += 1,
            }
            views_kept += arbitration.disagreement.views.len();
            if arbitration.disagreement.spread.is_finite() {
                widest = widest.max(arbitration.disagreement.spread);
            }
        }

        // Statistics, not money: Brier scores are probabilities' squared
        // error and nothing here sizes.
        let (mut scored, mut unscored, mut outside_claim) = (0usize, 0usize, 0usize);
        let mut brier_sum = 0.0_f64;
        for calibration in view.calibrations.values() {
            match calibration {
                Calibration::Unscored => unscored += 1,
                Calibration::Brier {
                    score,
                    within_claim,
                    ..
                } => {
                    scored += 1;
                    brier_sum += score;
                    if !within_claim {
                        outside_claim += 1;
                    }
                }
            }
        }

        let claim = format!(
            "federation review of cycle {} (journal position {}): {} arbitration(s) — {branch} \
             branch, {merge} merge, {abstain} abstain, {request} request more data — keeping \
             {views_kept} view(s), the widest spread {widest:.3}; {scored} member(s) calibrated, \
             {unscored} unscored, {outside_claim} outside their claimed Brier",
            view.cycle,
            view.journal_base,
            view.arbitrations.len()
        );
        let mut missing = Vec::new();
        if !view.uncovered.is_empty() {
            missing.push(format!(
                "no live member covers {}",
                view.uncovered.join(", ")
            ));
        }
        let mut builder = FindingBuilder::new(ctx, brief.as_of, claim)
            .direction(Direction::Neutral, 0.0)
            .fact(computed(
                ctx,
                "arbitrations_decided",
                view.arbitrations.len() as f64,
                "count",
                SOURCE,
            ))
            .fact(computed(
                ctx,
                "arbitrations_branch",
                branch as f64,
                "count",
                SOURCE,
            ))
            .fact(computed(
                ctx,
                "arbitrations_merge",
                merge as f64,
                "count",
                SOURCE,
            ))
            .fact(computed(
                ctx,
                "arbitrations_abstain",
                abstain as f64,
                "count",
                SOURCE,
            ))
            .fact(computed(
                ctx,
                "arbitrations_request_more_data",
                request as f64,
                "count",
                SOURCE,
            ))
            .fact(computed(
                ctx,
                "disagreeing_views_kept",
                views_kept as f64,
                "count",
                SOURCE,
            ))
            .fact(computed(
                ctx,
                "widest_disagreement_spread",
                widest,
                "probability",
                SOURCE,
            ))
            .fact(computed(
                ctx,
                "members_calibrated",
                scored as f64,
                "count",
                SOURCE,
            ))
            .fact(computed(
                ctx,
                "members_unscored",
                unscored as f64,
                "count",
                SOURCE,
            ))
            .fact(computed(
                ctx,
                "members_outside_calibration_claim",
                outside_claim as f64,
                "count",
                SOURCE,
            ))
            .caveats(vec![
                "reports the federation's own decisions; it re-scores and merges nothing"
                    .to_string(),
                "an unscored member has no resolved forecast: unscored is not a Brier of zero"
                    .to_string(),
            ]);
        // A mean over no scored member would be a number nobody measured.
        if scored > 0 {
            builder = builder.fact(computed(
                ctx,
                "mean_member_brier",
                brier_sum / scored as f64,
                "brier",
                SOURCE,
            ));
        }
        if !missing.is_empty() {
            builder = builder.missing(missing);
        }
        builder.build()
    }
}

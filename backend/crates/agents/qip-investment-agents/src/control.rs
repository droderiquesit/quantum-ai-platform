//! The control functions.
//!
//! Two agents hold the veto: [`RiskControl`] and [`ComplianceControl`]. Both
//! decide by arithmetic against declared rules, and neither can form an
//! investment view or move the limit it is checking against.
//!
//! Compliance has no language model at all. A compliance decision that depends
//! on how a sentence was phrased is not a compliance decision, and the manifest
//! withholds the capability so the question cannot arise.

use crate::desk::Desk;
use crate::support::{FindingBuilder, computed, no_data};
use qip_agents::finding::{AgentBrief, AgentFinding, Direction};
use qip_agents::manifest::AgentManifest;
use qip_agents::runtime::{Agent, AgentContext};
use qip_core::error::Result;
use std::sync::Arc;

/// Evaluates the book against its limits.
#[derive(Debug)]
pub struct RiskControl {
    manifest: AgentManifest,
    desk: Arc<Desk>,
}

impl RiskControl {
    pub fn new(manifest: AgentManifest, desk: Arc<Desk>) -> Self {
        Self { manifest, desk }
    }
}

impl Agent for RiskControl {
    fn manifest(&self) -> &AgentManifest {
        &self.manifest
    }

    fn analyse(&self, ctx: &mut AgentContext, brief: &AgentBrief) -> Result<AgentFinding> {
        let risk = self.desk.risk.get(ctx)?;
        let check = risk.limits.check(&risk.state);

        let blocking = check.blocking();
        let warnings = check.warnings();

        // A veto is a statement of fact about a limit, so it is reported with
        // full conviction. A warning is a statement that a limit is close,
        // which is information rather than a decision.
        let (direction, conviction, claim) = if check.is_blocked() {
            (
                Direction::Negative,
                1.0,
                format!("{} limit(s) block: {}", blocking.len(), check.reason()),
            )
        } else if check.requires_reduction() {
            (
                Direction::Negative,
                0.8,
                format!(
                    "no limit blocks, but {} require(s) a reduction: {}",
                    blocking.len().max(1),
                    check.reason()
                ),
            )
        } else if !warnings.is_empty() {
            (
                Direction::Neutral,
                0.0,
                format!(
                    "no limit is breached; {} within warning distance",
                    warnings.len()
                ),
            )
        } else {
            (
                Direction::Neutral,
                0.0,
                "no limit is breached or approached".to_string(),
            )
        };

        let mut builder = FindingBuilder::new(ctx, brief.as_of, claim)
            .direction(direction, conviction)
            .fact(computed(
                ctx,
                "blocking_breach_count",
                blocking.len() as f64,
                "count",
                &["risk_state", "limit_set"],
            ))
            .fact(computed(
                ctx,
                "warning_count",
                warnings.len() as f64,
                "count",
                &["risk_state", "limit_set"],
            ))
            .fact(computed(
                ctx,
                "gross_exposure",
                risk.state.gross_exposure.to_f64(),
                "currency",
                &["risk_state"],
            ))
            .fact(computed(
                ctx,
                "drawdown",
                risk.state.drawdown,
                "ratio",
                &["risk_state"],
            ))
            .evidence(vec![format!(
                "limit-set:{} evaluated against risk state",
                risk.limits.len()
            )])
            .caveats(vec![
                "a limit check is arithmetic against declared limits; it is not a view on whether the position is a good idea"
                    .to_string(),
            ]);

        if direction == Direction::Negative {
            builder = builder.falsifiers(vec![
                "the risk state changes so the breached limit is no longer breached".to_string(),
                "an authorised operator changes the limit, which no agent can do".to_string(),
            ]);
        }
        builder.build()
    }
}

// --- compliance -------------------------------------------------------------

/// Checks proposals against restricted lists, jurisdiction and venue rules.
#[derive(Debug)]
pub struct ComplianceControl {
    manifest: AgentManifest,
    desk: Arc<Desk>,
}

impl ComplianceControl {
    pub fn new(manifest: AgentManifest, desk: Arc<Desk>) -> Self {
        Self { manifest, desk }
    }
}

impl Agent for ComplianceControl {
    fn manifest(&self) -> &AgentManifest {
        &self.manifest
    }

    fn analyse(&self, ctx: &mut AgentContext, brief: &AgentBrief) -> Result<AgentFinding> {
        if brief.objects.is_empty() {
            return Ok(no_data(ctx, brief.as_of, "no instrument in scope to check"));
        }

        let compliance = self.desk.compliance.get(ctx)?;

        let mut prohibited = Vec::new();
        let mut buy_blocked = Vec::new();
        let mut short_blocked = Vec::new();
        let mut uncovered = Vec::new();
        let mut evidence = Vec::new();

        for object in &brief.objects {
            let id = object.as_str();
            if compliance.is_prohibited(id) {
                prohibited.push(id.to_string());
                evidence.push(format!("prohibition:{id}"));
                continue;
            }
            let Some(constraints) = compliance.constraints_for(id) else {
                // No record is not permission. An instrument the compliance
                // register does not cover is escalated, not waved through.
                uncovered.push(id.to_string());
                continue;
            };
            evidence.push(format!("constraints:{id}"));
            let buy = constraints.buy_blockers();
            if !buy.is_empty() {
                buy_blocked.push(format!(
                    "{id}: {}",
                    buy.iter()
                        .map(|r| r.describe())
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
            let short = constraints.short_blockers();
            if !short.is_empty() {
                short_blocked.push(format!(
                    "{id}: {}",
                    short
                        .iter()
                        .map(|r| r.describe())
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
        }

        let blocked = !prohibited.is_empty() || !buy_blocked.is_empty();
        let claim = if !prohibited.is_empty() {
            format!(
                "{} name(s) are under an absolute prohibition: {}",
                prohibited.len(),
                prohibited.join(", ")
            )
        } else if blocked {
            format!("buying is blocked for: {}", buy_blocked.join("; "))
        } else if !uncovered.is_empty() {
            format!(
                "{} name(s) are not covered by the compliance register and must be escalated: {}",
                uncovered.len(),
                uncovered.join(", ")
            )
        } else {
            format!("{} name(s) carry no buy restriction", brief.objects.len())
        };

        let mut builder = FindingBuilder::new(ctx, brief.as_of, claim)
            .direction(
                if blocked {
                    Direction::Negative
                } else {
                    Direction::Neutral
                },
                if blocked { 1.0 } else { 0.0 },
            )
            .fact(computed(
                ctx,
                "prohibited_count",
                prohibited.len() as f64,
                "count",
                &["compliance_register"],
            ))
            .fact(computed(
                ctx,
                "buy_blocked_count",
                buy_blocked.len() as f64,
                "count",
                &["compliance_register"],
            ))
            .fact(computed(
                ctx,
                "short_blocked_count",
                short_blocked.len() as f64,
                "count",
                &["compliance_register"],
            ))
            .fact(computed(
                ctx,
                "uncovered_count",
                uncovered.len() as f64,
                "count",
                &["compliance_register"],
            ))
            .evidence(evidence)
            .caveats(vec![
                "restriction checks are deterministic lookups; anything the register does not cover is escalated rather than decided"
                    .to_string(),
            ]);

        if blocked {
            builder = builder.falsifiers(vec![
                "the restriction is lifted from the register by an authorised change".to_string(),
            ]);
        }
        if !uncovered.is_empty() {
            builder = builder.missing(
                uncovered
                    .iter()
                    .map(|id| format!("compliance register entry for {id}"))
                    .collect(),
            );
        }
        builder.build()
    }
}

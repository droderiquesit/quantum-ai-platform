//! The Private Markets Brain (R12-ASSET-3018): the specialist that reads the
//! private-markets book — unfunded commitments, the capital calls standing
//! against them, and the dated marks on illiquid assets.
//!
//! The kernel holds commitments, capital calls and illiquid marks and
//! narrows sizing on them, and no specialist reported any of it, so a call
//! past due or a mark past its review date reached the chief as nothing. It
//! reads the desk's private-markets slot, which the kernel writes from its
//! own books every REASON, so every figure is the kernel's fact and not this
//! agent's inference.
//!
//! **A book with no private holding is a finding, an unwritten slot is no
//! data.** The first states zero commitments and zero stale marks; the
//! second says the kernel has not briefed the desk. Fund and company
//! lifecycle stage has no per-holding source in the kernel, and the
//! finding's caveats say so rather than reporting a stage.

use crate::desk::Desk;
use crate::support::{FindingBuilder, computed, no_data};
use qip_agents::finding::{AgentBrief, AgentFinding, Direction};
use qip_agents::manifest::AgentManifest;
use qip_agents::runtime::{Agent, AgentContext};
use qip_core::error::Result;
use std::sync::Arc;

/// Reads unfunded commitments, capital calls and dated illiquid marks.
#[derive(Debug)]
pub struct PrivateMarketsAnalyst {
    manifest: AgentManifest,
    desk: Arc<Desk>,
}

impl PrivateMarketsAnalyst {
    pub fn new(manifest: AgentManifest, desk: Arc<Desk>) -> Self {
        Self { manifest, desk }
    }
}

impl Agent for PrivateMarketsAnalyst {
    fn manifest(&self) -> &AgentManifest {
        &self.manifest
    }

    fn analyse(&self, ctx: &mut AgentContext, brief: &AgentBrief) -> Result<AgentFinding> {
        let view = self.desk.private_markets.get(ctx)?;
        if !view.observed {
            return Ok(no_data(
                ctx,
                brief.as_of,
                "the kernel has not written the private-markets slot: there is no commitment, \
                 capital call or illiquid mark to read",
            ));
        }
        let open: usize = view.commitments.iter().map(|c| c.calls_open).sum();
        let overdue: usize = view.commitments.iter().map(|c| c.calls_overdue).sum();
        let stale = view.marks.iter().filter(|mark| mark.stale).count();
        // Money summed exactly, then crossed to a statistic once, here.
        let unfunded = view
            .commitments
            .iter()
            .try_fold(qip_core::Decimal::ZERO, |total, c| {
                total.checked_add(c.unfunded)
            });
        let unfunded = unfunded.map_or(f64::NAN, |total| total.to_f64());
        let claim = format!(
            "{} private commitment(s) with {unfunded:.2} unfunded; {open} capital call(s) \
             open, {overdue} overdue; {} illiquid mark(s), {stale} past review",
            view.commitments.len(),
            view.marks.len()
        );
        let mut builder = FindingBuilder::new(ctx, brief.as_of, claim)
            .direction(Direction::Neutral, 0.0)
            .fact(computed(
                ctx,
                "private_commitments",
                view.commitments.len() as f64,
                "count",
                &["private-markets"],
            ))
            .fact(computed(
                ctx,
                "capital_calls_open",
                open as f64,
                "count",
                &["private-markets"],
            ))
            .fact(computed(
                ctx,
                "capital_calls_overdue",
                overdue as f64,
                "count",
                &["private-markets"],
            ))
            .fact(computed(
                ctx,
                "illiquid_marks_stale",
                stale as f64,
                "count",
                &["private-markets"],
            ))
            .caveats(vec![
                "reads the commitment book and marks as the kernel last wrote them".to_string(),
                "fund and company lifecycle stage: the kernel keeps none per holding".to_string(),
            ]);
        // An overflowing total is no total: the fact is left out rather than
        // stated as a number nobody could represent.
        if unfunded.is_finite() {
            builder = builder.fact(computed(
                ctx,
                "private_unfunded_commitment",
                unfunded,
                "currency",
                &["private-markets"],
            ));
        }
        builder.build()
    }
}

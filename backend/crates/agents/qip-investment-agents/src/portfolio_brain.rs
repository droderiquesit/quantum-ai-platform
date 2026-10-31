//! The Portfolio/Asset Brain (R12-ASSET-3019): the specialist that reads the
//! book as held and says what it is exposed to, what nobody has stated a
//! mandate for, and what is being divested.
//!
//! Every other specialist reads the market or the world model; construction
//! reads the book only to price its turnover. Nothing in the organisation
//! said what the book *is*, so a held position whose mandate the ledger no
//! longer recognised, or a book being unwound, reached the chief as nothing.
//!
//! It reads the desk's book slot, which the kernel writes from its position
//! lifecycle register every cycle (`Platform::brief_the_desk_on_book`), so a
//! position the register flagged for want of a mandate (R12-ASSET-4066) or
//! marked unwinding is the register's fact and not this agent's inference.
//! An empty book is said as no data, never as a clean one.

use crate::desk::Desk;
use crate::support::{FindingBuilder, computed, no_data};
use qip_agents::finding::{AgentBrief, AgentFinding, Direction};
use qip_agents::manifest::AgentManifest;
use qip_agents::runtime::{Agent, AgentContext};
use qip_core::error::Result;
use qip_portfolio::lifecycle::PositionLifecycle;
use std::sync::Arc;

/// Reads the held book: exposure, unmandated positions, divestment.
#[derive(Debug)]
pub struct PortfolioAssetAnalyst {
    manifest: AgentManifest,
    desk: Arc<Desk>,
}

impl PortfolioAssetAnalyst {
    pub fn new(manifest: AgentManifest, desk: Arc<Desk>) -> Self {
        Self { manifest, desk }
    }
}

impl Agent for PortfolioAssetAnalyst {
    fn manifest(&self) -> &AgentManifest {
        &self.manifest
    }

    fn analyse(&self, ctx: &mut AgentContext, brief: &AgentBrief) -> Result<AgentFinding> {
        let book = self.desk.book.get(ctx)?;
        let positions: Vec<_> = book.portfolio.positions().collect();
        if positions.is_empty() {
            return Ok(no_data(
                ctx,
                brief.as_of,
                "the book holds no position, so there is no exposure, mandate or divestment to \
                 report",
            ));
        }
        let valuation = book.portfolio.value(&book.marks, brief.as_of);
        let equity = valuation.equity.to_f64();
        // Statistics, not money: the ratio crosses to f64 here and is read by
        // nothing that sizes.
        let mut gross = 0.0;
        let mut unmarked = 0usize;
        for position in &positions {
            match book.marks.get(position.object_id.as_str()) {
                Some(mark) => gross += (position.quantity().to_f64() * mark.to_f64()).abs(),
                None => unmarked += 1,
            }
        }
        let flagged = positions
            .iter()
            .filter(|position| position.lifecycle() == PositionLifecycle::Flagged)
            .count();
        let unwinding = positions
            .iter()
            .filter(|position| position.lifecycle() == PositionLifecycle::Unwinding)
            .count();
        let claim = format!(
            "{} held position(s); {flagged} held with no mandate on record, {unwinding} being \
             divested, {unmarked} unmarked",
            positions.len()
        );
        let mut builder = FindingBuilder::new(ctx, brief.as_of, claim)
            .direction(Direction::Neutral, 0.0)
            .fact(computed(
                ctx,
                "positions_held",
                positions.len() as f64,
                "count",
                &["portfolio"],
            ))
            .fact(computed(
                ctx,
                "positions_flagged_without_mandate",
                flagged as f64,
                "count",
                &["portfolio"],
            ))
            .fact(computed(
                ctx,
                "positions_unwinding",
                unwinding as f64,
                "count",
                &["portfolio"],
            ))
            .fact(computed(
                ctx,
                "positions_unmarked",
                unmarked as f64,
                "count",
                &["portfolio"],
            ))
            .caveats(vec![
                "reads the book as the kernel last wrote it into the desk's slot".to_string(),
            ]);
        // A gross ratio over a non-positive equity is not a number anybody
        // computed; it is left out rather than reported as infinite.
        if equity > 0.0 {
            builder = builder.fact(computed(
                ctx,
                "book_gross_exposure_ratio",
                gross / equity,
                "ratio",
                &["portfolio", "marks"],
            ));
        }
        builder.build()
    }
}

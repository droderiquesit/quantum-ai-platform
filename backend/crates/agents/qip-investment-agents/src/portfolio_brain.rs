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

use crate::desk::{Desk, HedgeStanding};
use crate::support::{FindingBuilder, computed, no_data};
use qip_agents::finding::{AgentBrief, AgentFinding, Direction};
use qip_agents::manifest::AgentManifest;
use qip_agents::runtime::{Agent, AgentContext};
use qip_core::error::Result;
use qip_portfolio::lifecycle::PositionLifecycle;
use std::collections::BTreeMap;
use std::sync::Arc;

/// Reads the held book: exposure, unmandated positions, divestment,
/// horizons, hedges and its causal-driver and factor allocation.
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
        let purpose = &book.purpose;
        // Horizons (R12-ASSET-3019): a position with no horizon on record,
        // one whose sizing instant is unknown, and one held past the horizon
        // of the hypothesis it was sized on are three different facts.
        let mut without_horizon = 0usize;
        let mut horizon_age_unknown = 0usize;
        let mut past_horizon = 0usize;
        let mut nearest_remaining: Option<f64> = None;
        // Hedges: hedged, unhedged (looked at, nothing covers it) and
        // undeclared (nobody stated) are kept apart.
        let (mut hedged, mut unhedged, mut hedge_undeclared) = (0usize, 0usize, 0usize);
        // Causal drivers (ADR 0468): marked gross by the hypothesis each
        // position was last sized on; a position with none is unattributed.
        let mut by_driver: BTreeMap<&str, f64> = BTreeMap::new();
        let mut unattributed = 0.0;
        for position in &positions {
            let object = position.object_id.as_str();
            let horizon = purpose.horizons.get(object);
            match horizon {
                None => without_horizon += 1,
                Some(held) => match held.sized_at {
                    None => horizon_age_unknown += 1,
                    Some(sized_at) => {
                        let span = held.horizon.as_nanos() as f64;
                        let elapsed = brief.as_of.since(sized_at).as_nanos() as f64;
                        // A horizon is positive by construction in the
                        // register; a non-positive one is counted as having
                        // no horizon rather than divided by.
                        if span > 0.0 {
                            if elapsed > span {
                                past_horizon += 1;
                            }
                            let remaining = (span - elapsed) / span;
                            nearest_remaining =
                                Some(nearest_remaining.map_or(remaining, |n| n.min(remaining)));
                        } else {
                            without_horizon += 1;
                        }
                    }
                },
            }
            match purpose.hedges.get(object) {
                Some(HedgeStanding::Hedged(_)) => hedged += 1,
                Some(HedgeStanding::Unhedged { .. }) => unhedged += 1,
                None => hedge_undeclared += 1,
            }
            if let Some(mark) = book.marks.get(object) {
                let value = (position.quantity().to_f64() * mark.to_f64()).abs();
                match horizon {
                    Some(held) => *by_driver.entry(held.hypothesis.as_str()).or_default() += value,
                    None => unattributed += value,
                }
            }
        }
        let claim = format!(
            "{} held position(s); {flagged} held with no mandate on record, {unwinding} being \
             divested, {unmarked} unmarked; {past_horizon} past horizon, {without_horizon} with \
             no horizon; {hedged} hedged, {unhedged} unhedged, {hedge_undeclared} hedge \
             undeclared; allocated across {} causal driver(s)",
            positions.len(),
            by_driver.len()
        );
        let mut caveats =
            vec!["reads the book as the kernel last wrote it into the desk's slot".to_string()];
        if let Some(reason) = &purpose.hedges_refused {
            caveats.push(format!("no hedge standing was stated this cycle: {reason}"));
        }
        if let Some(reason) = &purpose.factors_refused {
            caveats.push(format!("the factor allocation was refused: {reason}"));
        }
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
            .fact(computed(
                ctx,
                "positions_without_horizon",
                without_horizon as f64,
                "count",
                &["portfolio", "position_register"],
            ))
            .fact(computed(
                ctx,
                "positions_horizon_age_unknown",
                horizon_age_unknown as f64,
                "count",
                &["portfolio", "position_register"],
            ))
            .fact(computed(
                ctx,
                "positions_past_horizon",
                past_horizon as f64,
                "count",
                &["portfolio", "position_register"],
            ))
            .fact(computed(
                ctx,
                "positions_hedged",
                hedged as f64,
                "count",
                &["portfolio", "position_register"],
            ))
            .fact(computed(
                ctx,
                "positions_unhedged",
                unhedged as f64,
                "count",
                &["portfolio", "position_register"],
            ))
            .fact(computed(
                ctx,
                "positions_hedge_undeclared",
                hedge_undeclared as f64,
                "count",
                &["portfolio", "position_register"],
            ))
            .fact(computed(
                ctx,
                "causal_drivers_allocated",
                by_driver.len() as f64,
                "count",
                &["portfolio", "position_register"],
            ))
            .fact(computed(
                ctx,
                "factors_loaded",
                purpose.factor_exposure.len() as f64,
                "count",
                &["portfolio", "universe"],
            ))
            .caveats(caveats);
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
        if let Some(remaining) = nearest_remaining {
            builder = builder.fact(computed(
                ctx,
                "nearest_horizon_remaining_fraction",
                remaining,
                "ratio",
                &["portfolio", "position_register"],
            ));
        }
        let marked_gross = by_driver.values().sum::<f64>() + unattributed;
        if marked_gross > 0.0 {
            let largest = by_driver.values().copied().fold(0.0, f64::max);
            builder = builder
                .fact(computed(
                    ctx,
                    "largest_causal_driver_gross_share",
                    largest / marked_gross,
                    "ratio",
                    &["portfolio", "marks", "position_register"],
                ))
                .fact(computed(
                    ctx,
                    "unattributed_gross_share",
                    unattributed / marked_gross,
                    "ratio",
                    &["portfolio", "marks", "position_register"],
                ));
        }
        if equity > 0.0 {
            // Statistics, not money: the largest absolute factor exposure
            // crosses to f64 here as a ratio of equity, read by nothing that
            // sizes.
            if let Some(largest) = purpose
                .factor_exposure
                .values()
                .map(|exposure| exposure.to_f64().abs())
                .reduce(f64::max)
            {
                builder = builder.fact(computed(
                    ctx,
                    "largest_factor_exposure_ratio",
                    largest / equity,
                    "ratio",
                    &["portfolio", "universe", "marks"],
                ));
            }
        }
        builder.build()
    }
}

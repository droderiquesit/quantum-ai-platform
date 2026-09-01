//! The fast-path agent.
//!
//! Runs on the microsecond-to-millisecond path with no language model and no
//! world model. The manifest's [`qip_agents::budget::Budget::fast_path`] grant
//! is what enforces that: zero language-model calls, fifty milliseconds, and a
//! capability set that does not include `call_language_model` at all.
//!
//! Everything here is arithmetic on the current book and recent tape.

use crate::desk::Desk;
use crate::support::{FindingBuilder, computed, no_data, observed_book, out_of_scope};
use qip_agents::finding::{AgentBrief, AgentFinding, Direction};
use qip_agents::manifest::AgentManifest;
use qip_agents::runtime::{Agent, AgentContext};
use qip_core::Decimal;
use qip_core::error::Result;
use qip_market::book::Side;
use std::sync::Arc;

/// Depth levels the imbalance is measured over.
///
/// Five levels rather than the touch alone: touch imbalance is trivially
/// spoofable and flickers on every quote update.
const IMBALANCE_LEVELS: usize = 5;

/// Reads the order book for liquidity, imbalance and the cost of trading now.
#[derive(Debug)]
pub struct MicrostructureAnalyst {
    manifest: AgentManifest,
    desk: Arc<Desk>,
    /// The size, in units, that cost estimates are quoted for.
    reference_size: Decimal,
}

impl MicrostructureAnalyst {
    pub fn new(manifest: AgentManifest, desk: Arc<Desk>, reference_size: Decimal) -> Self {
        Self {
            manifest,
            desk,
            reference_size,
        }
    }
}

impl Agent for MicrostructureAnalyst {
    fn manifest(&self) -> &AgentManifest {
        &self.manifest
    }

    fn accepts(&self, brief: &AgentBrief) -> bool {
        !brief.objects.is_empty()
    }

    fn analyse(&self, ctx: &mut AgentContext, brief: &AgentBrief) -> Result<AgentFinding> {
        let Some(subject) = brief.objects.first() else {
            return Ok(out_of_scope(
                ctx,
                brief.as_of,
                "a liquidity view needs an instrument",
            ));
        };
        let market = self.desk.market.get(ctx)?;
        let Some(state) = market.snapshot.get(subject) else {
            return Ok(no_data(
                ctx,
                brief.as_of,
                format!("no market state for {}", subject.as_str()),
            ));
        };
        let Some(book) = state.book.as_ref() else {
            return Ok(no_data(
                ctx,
                brief.as_of,
                format!("no order book for {}", subject.as_str()),
            ));
        };

        // A crossed book is a data problem, and trading on one is how a data
        // problem becomes a loss.
        if book.is_crossed() {
            return Ok(no_data(
                ctx,
                brief.as_of,
                format!(
                    "{} book is crossed; the feed is wrong and no cost estimate from it is usable",
                    subject.as_str()
                ),
            ));
        }

        let Some(mid) = book.mid() else {
            return Ok(no_data(
                ctx,
                brief.as_of,
                format!("{} book has no two-sided quote", subject.as_str()),
            ));
        };
        let mid_f = mid.to_f64();
        if mid_f <= 0.0 {
            return Ok(no_data(
                ctx,
                brief.as_of,
                format!("{} mid is not positive", subject.as_str()),
            ));
        }

        // The touch is what this agent reads; everything below is what it
        // derives from it. Both sides exist here — `mid` needed them — and
        // are recorded as observed from the venue so the derived numbers
        // name a real record as their input.
        let touch = [
            observed_book("best_bid", "price", book, |b| b.best_bid().map(|l| l.price)),
            observed_book("best_ask", "price", book, |b| b.best_ask().map(|l| l.price)),
        ];

        let spread_bps = book
            .spread()
            .map(|s| s.to_f64() / mid_f * 10_000.0)
            .unwrap_or(f64::NAN);
        let imbalance = book.imbalance(IMBALANCE_LEVELS);
        let buy_cost_bps = book.sweep_cost_bps(Side::Buy, self.reference_size);
        let sell_cost_bps = book.sweep_cost_bps(Side::Sell, self.reference_size);
        let depth = book.total_depth().to_f64();

        let mut missing = Vec::new();
        if buy_cost_bps.is_none() {
            missing.push(format!(
                "insufficient ask depth to fill {} units of {}",
                self.reference_size,
                subject.as_str()
            ));
        }
        if sell_cost_bps.is_none() {
            missing.push(format!(
                "insufficient bid depth to fill {} units of {}",
                self.reference_size,
                subject.as_str()
            ));
        }

        let mut builder = FindingBuilder::new(
            ctx,
            brief.as_of,
            format!(
                "{} quotes {spread_bps:.1}bp wide with {:+.2} depth imbalance over {IMBALANCE_LEVELS} levels",
                subject.as_str(),
                imbalance
            ),
        )
        // Deliberately no direction. Depth imbalance predicts the next tick,
        // not the next month, and this agent's job is to say what trading
        // costs — not whether to trade.
        .direction(Direction::Neutral, 0.0)
        .fact(computed(
            ctx,
            "mid_price",
            mid_f,
            "price",
            &["best_bid", "best_ask"],
        ))
        .fact(computed(
            ctx,
            "quoted_spread_bps",
            spread_bps,
            "bps",
            &["best_bid", "best_ask", "mid_price"],
        ))
        .fact(computed(ctx, "depth_imbalance", imbalance, "ratio", &["book"]))
        .fact(computed(ctx, "total_depth", depth, "units", &["book"]))
        .evidence(vec![format!("book:{}@{}", subject.as_str(), brief.as_of)])
        .falsifiers(vec![
            "the book is replaced within the next tick, which it usually is".to_string(),
        ])
        .caveats(vec![
            "a book snapshot is a statement about this instant only".to_string(),
        ]);

        for fact in touch.into_iter().flatten() {
            builder = builder.fact(fact);
        }
        if let Some(cost) = buy_cost_bps {
            builder = builder.fact(computed(
                ctx,
                "buy_sweep_cost_bps",
                cost,
                "bps",
                &["book", "reference_size"],
            ));
        }
        if let Some(cost) = sell_cost_bps {
            builder = builder.fact(computed(
                ctx,
                "sell_sweep_cost_bps",
                cost,
                "bps",
                &["book", "reference_size"],
            ));
        }
        if !missing.is_empty() {
            builder = builder.missing(missing);
        }
        builder.build()
    }
}

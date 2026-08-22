//! The execution trader.
//!
//! Reaches venues; originates nothing. The manifest denies it
//! `publish_hypothesis`, and `AgentManifest::validate` refuses the combination
//! outright — an execution agent that could originate the theses it trades is
//! the classic conflict, and it is closed structurally rather than by policy.
//!
//! What it produces is a cost estimate: what working a given size would cost
//! against the arrival price, and how long it would take at a tolerable
//! participation rate.

use crate::desk::Desk;
use crate::support::{FindingBuilder, computed, no_data, out_of_scope};
use qip_agents::finding::{AgentBrief, AgentFinding, Direction};
use qip_agents::manifest::AgentManifest;
use qip_agents::runtime::{Agent, AgentContext};
use qip_core::Decimal;
use qip_core::error::Result;
use qip_market::book::Side;
use std::sync::Arc;

/// Share of volume above which an order starts moving the price against
/// itself. Twenty percent is the conventional working assumption and is stated
/// here rather than buried in a formula.
const MAXIMUM_PARTICIPATION: f64 = 0.20;

/// Coefficient in the square-root impact law, in basis points.
///
/// Impact ≈ k·σ·sqrt(size / ADV). The square root, rather than a linear term,
/// is the well-replicated empirical shape; `k` near one is the usual calibrated
/// value. It is a working assumption, not a measurement, and the finding says
/// so.
const IMPACT_COEFFICIENT: f64 = 1.0;

/// Estimates the cost of working an order.
#[derive(Debug)]
pub struct ExecutionTrader {
    manifest: AgentManifest,
    desk: Arc<Desk>,
}

impl ExecutionTrader {
    pub fn new(manifest: AgentManifest, desk: Arc<Desk>) -> Self {
        Self { manifest, desk }
    }
}

impl Agent for ExecutionTrader {
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
                "a cost estimate needs an instrument",
            ));
        };

        // The size to work comes from the brief's questions, in the form
        // `size=<units>`. Parsed rather than guessed: an execution estimate for
        // an assumed size is worse than no estimate.
        let size = brief.questions.iter().find_map(|q| {
            q.strip_prefix("size=")
                .and_then(|value| value.trim().parse::<f64>().ok())
        });
        let Some(size) = size.filter(|s| s.is_finite() && *s > 0.0) else {
            return Ok(no_data(
                ctx,
                brief.as_of,
                "the brief carries no size to work; an execution estimate for an assumed size is not an estimate",
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

        let bars = state.bars.as_of(brief.as_of);
        let adv: f64 = if bars.is_empty() {
            0.0
        } else {
            let window = bars.len().min(20);
            bars[bars.len() - window..]
                .iter()
                .map(|b| b.volume.to_f64())
                .sum::<f64>()
                / window as f64
        };

        let volatility = state.bars.annualised_volatility();
        // Daily volatility is what a same-day execution is exposed to.
        let daily_volatility = volatility / 252.0_f64.sqrt();

        let mut missing = Vec::new();
        if adv <= 0.0 {
            missing.push(format!(
                "average daily volume for {}: impact cannot be estimated without it",
                subject.as_str()
            ));
        }
        if !daily_volatility.is_finite() || daily_volatility <= 0.0 {
            missing.push(format!(
                "realised volatility for {}: impact scales with it",
                subject.as_str()
            ));
        }

        let participation = if adv > 0.0 { size / adv } else { f64::NAN };
        let impact_bps = if adv > 0.0 && daily_volatility.is_finite() && daily_volatility > 0.0 {
            IMPACT_COEFFICIENT * daily_volatility * participation.sqrt() * 10_000.0
        } else {
            f64::NAN
        };
        let days_to_complete = if participation.is_finite() {
            (participation / MAXIMUM_PARTICIPATION).max(1.0)
        } else {
            f64::NAN
        };

        // The book gives the immediate alternative: cross the spread now.
        let immediate_bps = state.book.as_ref().and_then(|book| {
            Decimal::from_f64(size).and_then(|q| book.sweep_cost_bps(Side::Buy, q))
        });

        let mut builder = FindingBuilder::new(
            ctx,
            brief.as_of,
            match (impact_bps.is_finite(), days_to_complete.is_finite()) {
                (true, true) => format!(
                    "working {size:.0} units of {} costs about {impact_bps:.1}bp over {days_to_complete:.1} day(s) at {:.0}% participation",
                    subject.as_str(),
                    MAXIMUM_PARTICIPATION * 100.0
                ),
                _ => format!(
                    "cannot estimate the cost of working {size:.0} units of {}",
                    subject.as_str()
                ),
            },
        )
        // Never a directional view: the trader works what it is given.
        .direction(Direction::Neutral, 0.0)
        .fact(computed(ctx, "order_size", size, "units", &["brief"]))
        .fact(computed(ctx, "average_daily_volume", adv, "units", &["bars"]))
        .fact(computed(
            ctx,
            "participation_of_adv",
            participation,
            "ratio",
            &["order_size", "average_daily_volume"],
        ))
        .fact(computed(
            ctx,
            "estimated_impact_bps",
            impact_bps,
            "bps",
            &["daily_volatility", "participation_of_adv"],
        ))
        .fact(computed(
            ctx,
            "days_to_complete",
            days_to_complete,
            "days",
            &["participation_of_adv"],
        ))
        .evidence(vec![format!("bars:{}@{}", subject.as_str(), brief.as_of)])
        .falsifiers(vec![
            "realised slippage on the completed order differs from the estimate by more than a factor of two"
                .to_string(),
        ])
        .caveats(vec![
            "the square-root impact law is a calibrated working assumption, not a measurement of this instrument"
                .to_string(),
            "the estimate assumes no other participant is working the same direction".to_string(),
        ]);

        if let Some(immediate) = immediate_bps {
            builder = builder.fact(computed(
                ctx,
                "immediate_sweep_cost_bps",
                immediate,
                "bps",
                &["book", "order_size"],
            ));
        } else {
            missing.push(format!(
                "sufficient book depth to price immediate execution of {size:.0} units"
            ));
        }
        if !missing.is_empty() {
            builder = builder.missing(missing);
        }
        builder.build()
    }
}
